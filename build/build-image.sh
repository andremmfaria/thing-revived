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
  if ! file "${out}" | grep -q "static-pie linked"; then
    log "ERROR: ${out} is not static-pie linked (missing +crt-static?) - got: $(file "${out}")"
    exit 1
  fi
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

# --- 4. sync this repo's frontend/ into the webapp recipe's files/ --------
log "syncing frontend/ -> ${WEBAPP_FILES_DIR}"
rsync -a --delete "${REPO_ROOT}/frontend/" "${WEBAPP_FILES_DIR}/"

# --- 5. copy build/overlay/ on top of the example layer -------------------
log "copying build/overlay/ -> ${CHECKOUT}/${EXAMPLE_LAYER}"
cp -a "${REPO_ROOT}/build/overlay/." "${CHECKOUT}/${EXAMPLE_LAYER}/"

# --- 6. run the real build (orchestrator runs this personally) ------------
log "running: kas-container build --target ${KAS_TARGET} ${KAS_FILE}"
(
  cd "${CHECKOUT}"
  kas-container build --target "${KAS_TARGET}" "${KAS_FILE}"
)

# --- 7. stage the flashthing zip into this repo's build/artifact/ ---------
DEPLOY_DIR="${CHECKOUT}/build/tmp/deploy/images/superbird"
ZIP_SRC="$(find "${DEPLOY_DIR}" -maxdepth 1 -name '*flashthing*.zip' -print -quit)"
if [ -z "${ZIP_SRC}" ]; then
  log "ERROR: no flashthing zip found under ${DEPLOY_DIR}"
  exit 1
fi

mkdir -p "${ARTIFACT_DIR}"
DATESTAMP="$(date +%Y%m%d)"
ZIP_DEST="${ARTIFACT_DIR}/superbird-kiosk-prod-image-flashthing-${DATESTAMP}.zip"
cp "${ZIP_SRC}" "${ZIP_DEST}"

log "done: ${ZIP_DEST}"
