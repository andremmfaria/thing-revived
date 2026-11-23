#!/usr/bin/env bash
# flash-image.sh - flash a built flashthing zip to a device in Amlogic burn-mode.
#
# Usage:
#   build/flash-image.sh [bus:device | any] [path/to/flashthing.zip]
#
#   arg1 (optional): a specific USB bus:device address (as shown by `lsusb`,
#     e.g. "003:012" or "3:12" - zero padding is optional) to disambiguate
#     when more than one 1b8e:c003 device is present. Omit it, or pass "any",
#     to auto-detect - auto-detect requires exactly one match, and errors
#     clearly otherwise.
#   arg2 (optional): path to a specific artifact zip. Defaults to the newest
#     *.zip file in build/artifact/.
#
# flashthing-cli exits 0 even when it fails (observed with "USB error:
# Operation timed out" and "USB error: Entity not found"), so success is
# decided from its output: the final "flashthing_cli: done!" line must be
# present and no "ERROR flashthing_cli" / "failed to flash device" line may
# appear. A non-zero exit status is also treated as failure.
#
# Entering burn-mode (hold buttons 1+4 while plugging in USB) and the
# post-flash boot (unplug, then plug back in normally with NO buttons held)
# are both physical actions only a human at the device can do. This script
# does not attempt either.
#
# Known transient failure modes (see docs/yocto-build-and-flash.md ("Flashing") and
# the thing-revived-dev-workflow skill):
#   1. Mid-write "USB error: Operation timed out" - the chip drops back to
#      bare BootROM state and accepts a fresh connection immediately. This
#      script retries flashthing-cli automatically (up to MAX_RETRIES) with
#      no physical action needed.
#   2. A wedged USB host controller - "USB error: Entity not found" that
#      persists across retries, or repeated "xhci_hcd: Trying to add
#      endpoint 0x81 without dropping it" in dmesg. This is NOT recoverable
#      by software retry; the script detects it, asks the human to physically
#      unplug/replug while holding burn-mode buttons (1+4) again, waits for
#      the device to reappear in lsusb, and only then retries.

set -uo pipefail

VID_PID="1b8e:c003"
MAX_RETRIES=2
FLASHTHING_CLI="${FLASHTHING_CLI:-${HOME}/.cargo/bin/flashthing-cli}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
ARTIFACT_DIR="${REPO_ROOT}/build/artifact"

BUS_DEV_ARG="${1:-any}"
ZIP_ARG="${2:-}"

# Poll/wait knobs (overridable so tests do not sleep for minutes).
POLL_INTERVAL="${FLASH_POLL_INTERVAL:-2}"
WEDGE_GONE_WAIT="${FLASH_WEDGE_GONE_WAIT:-60}"
WEDGE_BACK_WAIT="${FLASH_WEDGE_BACK_WAIT:-120}"
MAX_WEDGE_RECOVERIES=2

log() { echo "[flash-image] $*" >&2; }
die() { log "ERROR: $*"; exit 1; }

# --- normalise the bus:device argument ------------------------------------
# Accept "any"/empty (auto-detect), "001:046" and "1:46". Force base 10 so
# "008:009" is not parsed as invalid octal, then hand lsusb a padded form.
if [ "${BUS_DEV_ARG}" = "any" ]; then
  BUS_DEV_ARG=""
elif [[ "${BUS_DEV_ARG}" =~ ^([0-9]{1,3}):([0-9]{1,3})$ ]]; then
  BUS_DEV_ARG="$(printf '%03d:%03d' "$((10#${BASH_REMATCH[1]}))" "$((10#${BASH_REMATCH[2]}))")"
else
  die "bad bus:device '${BUS_DEV_ARG}' (expected 'any', '001:046' or '1:46')"
fi

# --- resolve the artifact zip ----------------------------------------------
if [ -n "${ZIP_ARG}" ]; then
  ZIP_PATH="${ZIP_ARG}"
else
  ZIP_PATH="$(find "${ARTIFACT_DIR}" -maxdepth 1 -name '*.zip' -printf '%T@ %p\n' 2>/dev/null \
    | sort -rn | head -n1 | cut -d' ' -f2-)"
fi
[ -n "${ZIP_PATH}" ] && [ -f "${ZIP_PATH}" ] || die "no artifact zip found (looked in ${ARTIFACT_DIR}, arg2=${ZIP_ARG:-<unset>})"
log "artifact: ${ZIP_PATH}"

[ -x "${FLASHTHING_CLI}" ] || die "flashthing-cli not found/executable at ${FLASHTHING_CLI} (set FLASHTHING_CLI=... to override)"

# --- find the target device -------------------------------------------------
find_device() {
  if [ -n "${BUS_DEV_ARG}" ]; then
    lsusb -s "${BUS_DEV_ARG}" 2>/dev/null | grep -i "${VID_PID}" | head -n1
  else
    lsusb 2>/dev/null | grep -i "${VID_PID}"
  fi
}

resolve_device() {
  local matches count
  matches="$(find_device)"
  count="$(printf '%s' "${matches}" | grep -c . || true)"
  if [ "${count}" -eq 0 ]; then
    if [ -n "${BUS_DEV_ARG}" ]; then
      die "no ${VID_PID} device found at bus:device ${BUS_DEV_ARG}"
    fi
    die "no ${VID_PID} device found via lsusb (is the device in burn-mode? hold buttons 1+4 while plugging in USB)"
  elif [ "${count}" -gt 1 ]; then
    log "multiple ${VID_PID} matches found:"
    echo "${matches}" >&2
    die "ambiguous - pass a specific bus:device address as arg1 (e.g. 003:012)"
  fi
  echo "${matches}"
}

wait_for_device() {
  # Wait for the human to unplug (device vanishes), then replug (reappears).
  # If it never vanishes within WEDGE_GONE_WAIT we carry on anyway: a wedged
  # controller can keep listing the device.
  log "waiting for device to disappear (unplug it)..."
  local waited=0
  while [ "${waited}" -lt "${WEDGE_GONE_WAIT}" ] && [ -n "$(find_device)" ]; do
    sleep "${POLL_INTERVAL}"
    waited=$((waited + POLL_INTERVAL))
  done
  log "waiting for device to reappear (${VID_PID})..."
  waited=0
  while [ "${waited}" -lt "${WEDGE_BACK_WAIT}" ]; do
    if [ -n "$(find_device)" ]; then
      log "device reappeared."
      return 0
    fi
    sleep "${POLL_INTERVAL}"
    waited=$((waited + POLL_INTERVAL))
  done
  return 1
}

is_wedged_xhci() {
  # True if dmesg shows the repeated xhci endpoint-add symptom (needs root
  # for `dmesg` on most systems; falls back to `sudo dmesg` if plain dmesg
  # is restricted).
  local out
  out="$(dmesg 2>/dev/null | tail -n 200)"
  if [ -z "${out}" ]; then
    out="$(sudo -n dmesg 2>/dev/null | tail -n 200)"
  fi
  echo "${out}" | grep -q "Trying to add endpoint 0x81 without dropping it"
}

DEVICE_LINE="$(resolve_device)" || exit 1
log "target device: ${DEVICE_LINE}"

OUTPUT_FILE="$(mktemp)"
trap 'rm -f "${OUTPUT_FILE}"' EXIT

attempt=0
wedge_recoveries=0
while :; do
  attempt=$((attempt + 1))
  log "flashthing-cli attempt ${attempt} (${ZIP_PATH})"
  # pipefail is on, so rc is flashthing-cli's status (not tee's). It is only
  # one input: the output is authoritative because the CLI exits 0 on failure.
  "${FLASHTHING_CLI}" "${ZIP_PATH}" 2>&1 | tee "${OUTPUT_FILE}"
  rc=${PIPESTATUS[0]}

  # flashthing-cli colours its log with ANSI escapes even when stdout is a
  # pipe ("ERROR" and the target name are wrapped in escape codes), so strip
  # them before matching or neither the success nor the error line is found.
  sed -i 's/\x1b\[[0-9;]*m//g' "${OUTPUT_FILE}"

  if [ "${rc}" -eq 0 ] \
    && grep -q 'flashthing_cli: done!' "${OUTPUT_FILE}" \
    && ! grep -qE 'ERROR flashthing_cli|failed to flash device' "${OUTPUT_FILE}"; then
    log "SUCCESS: image written. Unplug the device, then plug it back in with NO buttons held to boot it."
    log "After boot: the kiosk password is unset. The screen asks you to press the M button to generate it, shows it once, and a second M press confirms you saved it. Then ssh kiosk@<device> works with that password (password SSH for kiosk only, root login is disabled, no SSH key is baked in). A lost password needs a factory reset or a reflash."
    exit 0
  fi
  log "flashthing-cli did not report success (exit status ${rc}; need 'flashthing_cli: done!' and no ERROR line)"

  OUT="$(cat "${OUTPUT_FILE}")"

  if echo "${OUT}" | grep -qi "Operation timed out"; then
    log "transient failure (Operation timed out) - chip dropped to bare BootROM, retrying immediately"
    if [ "${attempt}" -gt "${MAX_RETRIES}" ]; then
      die "FAILURE: exceeded ${MAX_RETRIES} retries on transient 'Operation timed out' failures"
    fi
    continue
  fi

  if echo "${OUT}" | grep -qi "Entity not found" || is_wedged_xhci; then
    log "WEDGED USB HOST CONTROLLER detected ('Entity not found' and/or xhci endpoint-add errors in dmesg)."
    log "This is NOT auto-recoverable by software."
    if [ "${wedge_recoveries}" -ge "${MAX_WEDGE_RECOVERIES}" ]; then
      die "FAILURE: still wedged after ${MAX_WEDGE_RECOVERIES} replug attempts"
    fi
    wedge_recoveries=$((wedge_recoveries + 1))
    log ">>> ACTION NEEDED: physically unplug the device, wait a few seconds, then replug it"
    log ">>> while holding burn-mode buttons (1+4) again. Waiting for it to reappear..."
    if wait_for_device; then
      log "device back - retrying flashthing-cli"
      continue
    else
      die "FAILURE: device did not reappear after ${WEDGE_BACK_WAIT}s - check the physical connection and retry the script"
    fi
  fi

  die "FAILURE: flashthing-cli failed with an unrecognized error; see output above. Not auto-retrying."
done
