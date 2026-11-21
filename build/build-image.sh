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
# and multiple hours of wall clock - see docs/yocto-build-and-flash.md §2.
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

log() { echo "[build-image] $*" >&2; }

log "upstream:    ${UPSTREAM_URL}"
log "checkout:    ${CHECKOUT}"
log "target:      ${KAS_TARGET} (${KAS_FILE})"

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
    RUSTFLAGS="-C target-feature=+crt-static" cargo build --release --target "${TARGET}"
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

# terminal-agent-config.json lives at daemon/terminal-agent-config.json (next
# to the daemon's own source) as the single source of truth - copy it fresh
# here rather than keeping a second hand-maintained copy under build/overlay/,
# same reasoning as the binary and frontend/ sync just above.
install -m 0644 "${REPO_ROOT}/daemon/terminal-agent-config.json" "${TERM_FILES_DIR}/terminal-agent-config.json"

# --- 4. sync this repo's frontend/ into the webapp recipe's files/ --------
log "syncing frontend/ -> ${WEBAPP_FILES_DIR}"
rsync -a --delete "${REPO_ROOT}/frontend/" "${WEBAPP_FILES_DIR}/"

# --- 5. copy build/overlay/ on top of the example layer -------------------
log "copying build/overlay/ -> ${CHECKOUT}/${EXAMPLE_LAYER}"
cp -a "${REPO_ROOT}/build/overlay/." "${CHECKOUT}/${EXAMPLE_LAYER}/"

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

log "running: ${KAS_CONTAINER} build --target ${KAS_TARGET} ${KAS_FILE}"
(
  cd "${CHECKOUT}"
  "${KAS_CONTAINER}" build --target "${KAS_TARGET}" "${KAS_FILE}"
)

# --- 7. stage the flashthing zip into this repo's build/artifact/ ---------
# do_flashthing_zip writes TWO files per build: the real multi-hundred-MB
# image zip ("*-flashthing.zip") and a ~9KB metadata-only companion
# ("*-flashthing-env-only.zip"). Both match a naive '*flashthing*.zip' glob
# and both can share the same mtime (written by the same task), so mtime
# alone doesn't reliably pick the real one - exclude "-env-only" explicitly,
# then still pick the newest-by-mtime among what's left (the deploy dir
# accumulates one pair per past build, not just this run's).
DEPLOY_DIR="${CHECKOUT}/build/tmp/deploy/images/superbird"
ZIP_SRC="$(find "${DEPLOY_DIR}" -maxdepth 1 -name '*flashthing*.zip' ! -name '*-env-only.zip' -printf '%T@ %p\n' 2>/dev/null \
  | sort -rn | head -n1 | cut -d' ' -f2-)"
if [ -z "${ZIP_SRC}" ]; then
  log "ERROR: no flashthing zip found under ${DEPLOY_DIR}"
  exit 1
fi
log "selected artifact: ${ZIP_SRC} ($(stat -c%s "${ZIP_SRC}" 2>/dev/null || stat -f%z "${ZIP_SRC}") bytes)"

mkdir -p "${ARTIFACT_DIR}"
DATESTAMP="$(date +%Y%m%d)"
ZIP_DEST="${ARTIFACT_DIR}/superbird-kiosk-prod-image-flashthing-${DATESTAMP}.zip"
cp "${ZIP_SRC}" "${ZIP_DEST}"

log "done: ${ZIP_DEST}"
