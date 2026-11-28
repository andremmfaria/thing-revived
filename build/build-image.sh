#!/usr/bin/env bash
# build-image.sh - build a flashable yocto-superbird kiosk image for thing-revived.
#
# This script does NOT vendor or commit any yocto-superbird source tree. It
# clones (or reuses) the upstream yocto-superbird repo into a scratch
# checkout, cross-compiles this repo's controller/ and daemon/ agents fresh,
# syncs this repo's frontend/ and build/overlay/ into the upstream checkout's
# EXISTING examples/meta-superbird-kiosk-example layer (already wired into
# upstream's own kas config - no new layer is created), runs the real
# kas-container build, and stages the resulting flashthing zip into
# build/artifact/ in THIS repo.
#
# Usage:
#   build/build-image.sh
#   STAGE_ONLY=1 [DEPLOY_DIR=...] build/build-image.sh   # only re-stage the zip
#   CONFIG_ONLY=1 [CONFIG_DIR=...] [CONFIG_OUT_DIR=...] build/build-image.sh
#                                   # only validate config/*.toml and generate
#                                   # config.generated.js (into a temp dir)
#
# Dev flags (see build/README.md). No SSH key is baked into the image: the
# kiosk user logs in with a password generated on first boot.
#   THING_REVIVED_ENABLE_ADB=1      adbd (unauthenticated root shell) starts enabled at boot
#   THING_REVIVED_ENABLE_UART=1     the UART root autologin starts enabled at boot
#
# These two flags are the first-boot SEED defaults, on every build including prod, not
# "feature present vs absent": they are written into the read-only seed file
# /usr/share/thing-revived/device-seed.json. The on-device Device settings menu then persists
# the user's choices in /var/lib/thing-revived/device-settings.json, and the flags only seed
# that file (first boot, or when the image id below changes); they do not override it on later
# boots (docs/research/device-settings-plan.md).
#
# THING_REVIVED_IMAGE_ID is computed here on every run (UTC timestamp + short git rev, or
# "nogit"), never set by the caller. It is written into the seed file and drives the reseed
# rule: a new image id means the persisted settings are reseeded from the new flags.
#
# Checkout location (two modes):
#   - Default (YOCTO_CHECKOUT unset): a FRESH clone every run, into a unique
#     temp dir (/tmp/yocto-superbird-build-$$), left in place after a
#     successful build so the sstate/downloads can be reused next time but
#     never implicitly reused across runs unless you point YOCTO_CHECKOUT at
#     it yourself.
#   - YOCTO_CHECKOUT=/path/to/existing/checkout: reuse an existing checkout
#     at that path. If the path doesn't exist yet, it is created by cloning
#     there instead of to a temp dir - set this to a persistent location
#     (e.g. on a build host with real disk) to avoid re-cloning/re-fetching
#     sstate-cacheable upstream history on every run.
#
# Requires: cargo + the aarch64-unknown-linux-gnu target and
# gcc-aarch64-linux-gnu (matching controller/.cargo/config.toml and
# daemon/.cargo/config.toml's existing linker convention), git, and
# kas-container (docker-based) on PATH for the actual build step.
#
# NOTE: a real Yocto build needs 50-100GB+ free disk for tmp/sstate/downloads
# and multiple hours of wall clock - see docs/yocto-build-and-flash.md ("The build/flash scripts").
# Don't run this on a disk-constrained host.

set -euo pipefail

UPSTREAM_URL="https://github.com/JoeyEamigh/yocto-superbird.git"
TARGET="aarch64-unknown-linux-gnu"
KAS_TARGET="superbird-kiosk-prod-image"
KAS_FILE="kas/example-kiosk.yml"
EXAMPLE_LAYER="examples/meta-superbird-kiosk-example"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
ARTIFACT_DIR="${REPO_ROOT}/build/artifact"

CHECKOUT="${YOCTO_CHECKOUT:-/tmp/yocto-superbird-build-$$}"

# Config source (one TOML per component). CONFIG_DIR is overridable so the
# CONFIG_ONLY mode can be tested against copies without touching config/.
CONFIG_DIR="${CONFIG_DIR:-${REPO_ROOT}/config}"
TOML_TOOL="${REPO_ROOT}/build/tools/toml_to_js.py"
CONFIG_FILES=(controller.toml terminal.toml frontend.toml)

log() { echo "[build-image] $*" >&2; }

# Fail early (before any cargo build) when a config file is missing or is not
# valid TOML. Uses stdlib tomllib via the generator's --check mode (Python 3.11+).
validate_config() {
  local f
  for f in "${CONFIG_FILES[@]}"; do
    if [ ! -f "${CONFIG_DIR}/${f}" ]; then
      log "ERROR: missing config file ${CONFIG_DIR}/${f}"
      return 1
    fi
  done
  if ! python3 "${TOML_TOOL}" --check "${CONFIG_FILES[@]/#/${CONFIG_DIR}/}"; then
    log "ERROR: config validation failed (see message above); fix the TOML under ${CONFIG_DIR}"
    return 1
  fi
  log "config OK: ${CONFIG_FILES[*]}"
}

# Convert config/frontend.toml into the classic script the kiosk loads. Written
# into the given directory only; the committed frontend/config.generated.js
# stub in the repo is never modified.
generate_frontend_config() {
  local out_dir="$1"
  mkdir -p "${out_dir}"
  python3 "${TOML_TOOL}" "${CONFIG_DIR}/frontend.toml" -o "${out_dir}/config.generated.js"
  if ! grep -q '^window\.THING_CONFIG = ' "${out_dir}/config.generated.js"; then
    log "ERROR: ${out_dir}/config.generated.js does not look generated"
    return 1
  fi
  log "generated ${out_dir}/config.generated.js"
}

# Only the value 1 enables a dev flag; anything else is 0.
flag_value() { if [ "${1:-}" = "1" ]; then echo 1; else echo 0; fi; }

# Image id: <yyyymmddThhmmssZ>-<8 hex git rev of the repo at $1>, or
# <yyyymmddThhmmssZ>-nogit when $1 is not a git checkout (or has no commit).
# Never empty. The ceiling stops git from picking up an enclosing repository
# above $1, so a plain source export inside some other checkout reports nogit.
compute_image_id() {
  local root="$1" ts rev=""
  ts="$(date -u +%Y%m%dT%H%M%SZ)"
  rev="$(GIT_CEILING_DIRECTORIES="$(dirname -- "${root}")" git -C "${root}" rev-parse --short=8 HEAD 2>/dev/null || true)"
  if [ -z "${rev}" ]; then rev="nogit"; fi
  echo "${ts}-${rev}"
}

# Write the extra kas config that carries the dev flags and the image id into
# bitbake. The env map makes kas add the names to BB_ENV_PASSTHROUGH_ADDITIONS;
# the local_conf_header literals are the fallback for when kas-container does
# not forward the host variables, so the recipes see the value either way.
#
# Layout flags (both are "?=" in meta-superbird/conf/distro/superbird.conf, so a
# local_conf_header assignment wins; a recipe's own "=" would not):
#   SUPERBIRD_ROOT_PART_SIZE=770        root_a and root_b slots of 770 MiB (upstream 516).
#   SUPERBIRD_BANDAID_PART_SIZE=0       the image has no bandaid partition (project wks
#                                       without the part line, no superbird-kiosk-bandaid
#                                       recipe in EXTRA_IMAGEDEPENDS, both set in the prod
#                                       image bbappend). The value only makes the OTA
#                                       headroom check count the real layout; the check
#                                       accepts 0. Never build the bandaid recipe with 0
#                                       (truncate/mkfs.ext4 on an empty file fails).
# OTA headroom check (superbird-headroom-check.bbclass, SUPERBIRD_REQUIRE_OTA_HEADROOM
# stays at its upstream default 1, MIN_EMMC 3600, margin 64), with root 770 and no bandaid:
#   required   = boot 64 + root 770 + margin 64                  =  898 MiB
#   wic fixed  = env 8 + 2*boot 64 + 2*root 770 + bandaid 0      = 1676 MiB
#   data floor = 3600 - 1676                                     = 1924 MiB  (>= 898, passes)
# The real data partition is a little smaller than 1924 MiB (partition table and 4 MiB
# alignment slack), still far above the 898 MiB an OTA write needs.
write_flags_kas() {
  local out="$1" adb="$2" uart="$3" image_id="$4"
  cat >"${out}" <<KASEOF
header:
  version: 8

env:
  THING_REVIVED_ENABLE_ADB: "${adb}"
  THING_REVIVED_ENABLE_UART: "${uart}"
  THING_REVIVED_IMAGE_ID: "${image_id}"

local_conf_header:
  thing_revived_flags: |
    THING_REVIVED_ENABLE_ADB = "${adb}"
    THING_REVIVED_ENABLE_UART = "${uart}"
    THING_REVIVED_IMAGE_ID = "${image_id}"
    SUPERBIRD_ROOT_PART_SIZE = "770"
    SUPERBIRD_BANDAID_PART_SIZE = "0"
KASEOF
}

log "upstream:    ${UPSTREAM_URL}"
log "checkout:    ${CHECKOUT}"
log "target:      ${KAS_TARGET} (${KAS_FILE})"

# --- 7. stage the flashthing zip into this repo's build/artifact/ ---------
# do_flashthing_zip writes TWO files per build: the real image zip
# ("*-flashthing.zip", ~390MB) and a ~9KB metadata-only companion
# ("*-flashthing-env-only.zip"). The deploy dir also holds un-dated SYMLINKS
# ("superbird-kiosk-prod-image-superbird-flashthing.zip", 66 bytes) pointing at
# the dated real files; a naive glob + cp used to pick such a symlink and
# produce a 0-byte artifact. So: resolve every candidate to its real file,
# drop env-only, dangling and tiny (< MIN_ZIP_BYTES) ones, and pick the newest
# by the real file's mtime. The copy goes to a ".partial" file (not *.zip, so
# flash-image.sh can never pick it up), is size- and integrity-checked, and
# only then renamed into place, so a failure never leaves a bogus zip behind.
# Nothing here depends on git; the date stamp comes from `date`.
MIN_ZIP_BYTES="${MIN_ZIP_BYTES:-1048576}"

zip_integrity_ok() {
  if command -v unzip >/dev/null 2>&1; then
    unzip -tq "$1" >/dev/null
  else
    python3 -m zipfile -t "$1" >/dev/null
  fi
}

stage_artifact() {
  local deploy_dir="${DEPLOY_DIR:-${CHECKOUT}/build/tmp/deploy/images/superbird}"
  local cand real size mtime best="" best_mtime=-1
  local seen=$'\n'

  if [ ! -d "${deploy_dir}" ]; then
    log "ERROR: deploy dir ${deploy_dir} does not exist"
    return 1
  fi

  for cand in "${deploy_dir}"/*flashthing*.zip; do
    [ -e "${cand}" ] || [ -L "${cand}" ] || continue
    real="$(readlink -f -- "${cand}" 2>/dev/null || true)"
    if [ -z "${real}" ] || [ ! -f "${real}" ]; then
      log "skip ${cand}: dangling symlink or not a regular file"
      continue
    fi
    case "$(basename -- "${real}")" in
      *-env-only.zip) log "skip ${cand}: env-only metadata zip"; continue ;;
    esac
    size="$(stat -c%s -- "${real}")"
    if [ "${size}" -lt "${MIN_ZIP_BYTES}" ]; then
      log "skip ${cand}: ${size} bytes < ${MIN_ZIP_BYTES}"
      continue
    fi
    case "${seen}" in *$'\n'"${real}"$'\n'*) continue ;; esac
    seen="${seen}${real}"$'\n'
    mtime="$(stat -c%Y -- "${real}")"
    if [ "${mtime}" -gt "${best_mtime}" ]; then
      best="${real}"
      best_mtime="${mtime}"
    fi
  done

  if [ -z "${best}" ]; then
    log "ERROR: no usable flashthing zip (real file >= ${MIN_ZIP_BYTES} bytes, not env-only) under ${deploy_dir}"
    return 1
  fi
  local src_size
  src_size="$(stat -c%s -- "${best}")"
  log "selected artifact: ${best} (${src_size} bytes)"

  mkdir -p "${ARTIFACT_DIR}"
  local datestamp dest partial
  datestamp="$(date +%Y%m%d)"
  dest="${ARTIFACT_DIR}/superbird-kiosk-prod-image-flashthing-${datestamp}.zip"
  partial="${dest}.partial"
  rm -f -- "${partial}"

  if ! cp -L -- "${best}" "${partial}"; then
    rm -f -- "${partial}"
    log "ERROR: copy to ${partial} failed"
    return 1
  fi
  local dest_size
  dest_size="$(stat -c%s -- "${partial}")"
  if [ "${dest_size}" -ne "${src_size}" ]; then
    rm -f -- "${partial}"
    log "ERROR: copied size ${dest_size} != source size ${src_size}"
    return 1
  fi
  if ! zip_integrity_ok "${partial}"; then
    rm -f -- "${partial}"
    log "ERROR: zip integrity test failed for the staged copy of ${best}"
    return 1
  fi
  mv -f -- "${partial}" "${dest}"
  log "done: ${dest} (${dest_size} bytes, zip test OK)"
}

# STAGE_ONLY=1 re-runs just the staging step against an existing deploy dir
# (DEPLOY_DIR overrides its location); no clone, cargo or kas is run.
if [ "${STAGE_ONLY:-0}" = "1" ]; then
  stage_artifact
  exit $?
fi

# CONFIG_ONLY=1 runs just the validate + generate steps, into CONFIG_OUT_DIR
# (default: a fresh temp dir, left in place and printed); no clone/cargo/kas.
if [ "${CONFIG_ONLY:-0}" = "1" ]; then
  validate_config
  generate_frontend_config "${CONFIG_OUT_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/thing-revived-config.XXXXXX")}"
  exit 0
fi

# --- 0. validate config before spending time on cargo/kas -----------------
validate_config

# --- 1. clone or reuse the upstream checkout -------------------------------
if [ -d "${CHECKOUT}/.git" ]; then
  log "reusing existing checkout at ${CHECKOUT}"
else
  log "cloning fresh upstream checkout into ${CHECKOUT}"
  git clone --depth 1 "${UPSTREAM_URL}" "${CHECKOUT}"
fi

if [ ! -d "${CHECKOUT}/${EXAMPLE_LAYER}" ]; then
  log "ERROR: ${CHECKOUT}/${EXAMPLE_LAYER} not found - checkout layout unexpected"
  exit 1
fi

# --- 2. cross-compile controller/ and daemon/ fresh from this repo --------
build_agent() {
  local crate_dir="$1" bin_name="$2"
  log "cross-compiling ${crate_dir} for ${TARGET} (+crt-static)"
  (
    cd "${REPO_ROOT}/${crate_dir}"
    RUSTFLAGS="-C target-feature=+crt-static" cargo build --release --target "${TARGET}" >&2
  )
  local out="${REPO_ROOT}/${crate_dir}/target/${TARGET}/release/${bin_name}"
  if [ ! -f "${out}" ]; then
    log "ERROR: expected binary not found at ${out}"
    exit 1
  fi
  # Accept either "statically linked" or "static-pie linked" - both mean no
  # dynamic glibc dependency (the actual risk +crt-static exists to avoid: a
  # glibc-version mismatch between the cross-toolchain and the device's own
  # glibc). Whether the linker also produces a PIE on top of that is a
  # cosmetic ASLR difference driven by the local aarch64-linux-gnu-gcc
  # build's own defaults (observed to differ between hosts), not a defect.
  local file_out
  file_out="$(file "${out}")"
  if ! echo "${file_out}" | grep -qE "statically linked|static-pie linked"; then
    log "ERROR: ${out} is dynamically linked (missing +crt-static?) - got: ${file_out}"
    exit 1
  fi
  log "linked: $(echo "${file_out}" | grep -oE 'statically linked|static-pie linked')"
  echo "${out}"
}

CONTROLLER_BIN="$(build_agent controller controller-agent)"
TERMINAL_BIN="$(build_agent daemon terminal-agent)"

log "controller-agent binary: ${CONTROLLER_BIN}"
log "terminal-agent binary:   ${TERMINAL_BIN}"

# --- 3. copy binaries into the checkout's recipe files/ dirs --------------
CTRL_FILES_DIR="${CHECKOUT}/${EXAMPLE_LAYER}/recipes-extended/controller-agent/files"
TERM_FILES_DIR="${CHECKOUT}/${EXAMPLE_LAYER}/recipes-extended/thing-revived-terminal-agent/files"
WEBAPP_FILES_DIR="${CHECKOUT}/${EXAMPLE_LAYER}/recipes-extended/superbird-kiosk-default-webapp/files"

mkdir -p "${CTRL_FILES_DIR}" "${TERM_FILES_DIR}" "${WEBAPP_FILES_DIR}"

install -m 0755 "${CONTROLLER_BIN}" "${CTRL_FILES_DIR}/controller-agent"
install -m 0755 "${TERMINAL_BIN}" "${TERM_FILES_DIR}/terminal-agent"

# config/*.toml are the single source of truth for the agents' runtime config
# (installed to /etc/thing-revived/ by the recipes) - copy them fresh here
# rather than keeping second hand-maintained copies under build/overlay/.
install -m 0644 "${CONFIG_DIR}/controller.toml" "${CTRL_FILES_DIR}/controller.toml"
install -m 0644 "${CONFIG_DIR}/terminal.toml" "${TERM_FILES_DIR}/terminal.toml"
rm -f "${TERM_FILES_DIR}/terminal-agent-config.json"

# --- 4. sync this repo's frontend/ into the webapp recipe's files/ --------
# The recipe installs the whole files/webapp tree recursively, so new frontend
# subdirectories (src/..., vendor/) need no recipe change.
WEBAPP_TREE="${WEBAPP_FILES_DIR}/webapp"
mkdir -p "${WEBAPP_TREE}"
log "syncing frontend/ -> ${WEBAPP_TREE}"
# tests/ and node_modules/ are dev-only and must not ship on the device. The
# committed config.generated.js stub is copied along and then overwritten below.
rsync -a --delete --exclude 'tests/' --exclude 'node_modules/' --exclude '.git*' \
  "${REPO_ROOT}/frontend/" "${WEBAPP_TREE}/"
generate_frontend_config "${WEBAPP_TREE}"

# --- 5. copy build/overlay/ on top of the example layer -------------------
log "copying build/overlay/ -> ${CHECKOUT}/${EXAMPLE_LAYER}"
cp -a "${REPO_ROOT}/build/overlay/." "${CHECKOUT}/${EXAMPLE_LAYER}/"

# Dev flags: normalise (only 1 enables), warn loudly, and hand them to bitbake
# through an extra kas config next to upstream's kas file.
ENABLE_ADB="$(flag_value "${THING_REVIVED_ENABLE_ADB:-}")"
ENABLE_UART="$(flag_value "${THING_REVIVED_ENABLE_UART:-}")"
export THING_REVIVED_ENABLE_ADB="${ENABLE_ADB}" THING_REVIVED_ENABLE_UART="${ENABLE_UART}"
if [ "${ENABLE_ADB}" = "1" ]; then
  log "WARNING: THING_REVIVED_ENABLE_ADB=1: this image exposes an unauthenticated root shell over adb. Development builds only."
fi
if [ "${ENABLE_UART}" = "1" ]; then
  log "WARNING: THING_REVIVED_ENABLE_UART=1: this image auto-logs in root on the UART console. Development builds only."
fi
FLAGS_KAS_REL="kas/thing-revived-flags.yml"
IMAGE_ID="$(compute_image_id "${REPO_ROOT}")"
export THING_REVIVED_IMAGE_ID="${IMAGE_ID}"
write_flags_kas "${CHECKOUT}/${FLAGS_KAS_REL}" "${ENABLE_ADB}" "${ENABLE_UART}" "${IMAGE_ID}"
log "flags: ADB=${ENABLE_ADB} UART=${ENABLE_UART} IMAGE_ID=${IMAGE_ID} (${FLAGS_KAS_REL})"

# --- 6. run the real build (orchestrator runs this personally) ------------
# `kas-container` (installed via `pip install --user kas`) lands in
# ~/.local/bin, which non-interactive shells (e.g. `ssh host "cmd"`, as this
# project's build host is normally driven) do not source onto PATH. Resolve
# it explicitly instead of assuming it's already reachable.
KAS_CONTAINER="$(command -v kas-container || true)"
if [ -z "${KAS_CONTAINER}" ] && [ -x "${HOME}/.local/bin/kas-container" ]; then
  KAS_CONTAINER="${HOME}/.local/bin/kas-container"
fi
if [ -z "${KAS_CONTAINER}" ]; then
  log "ERROR: kas-container not found on PATH or at ~/.local/bin/kas-container"
  exit 1
fi

log "running: ${KAS_CONTAINER} build --target ${KAS_TARGET} ${KAS_FILE}:${FLAGS_KAS_REL}"
(
  cd "${CHECKOUT}"
  "${KAS_CONTAINER}" build --target "${KAS_TARGET}" "${KAS_FILE}:${FLAGS_KAS_REL}"
)

# stage_artifact returns non-zero on failure; set -e is reliable here because
# it is called as a plain command, not inside an if/&&/|| context.
stage_artifact
