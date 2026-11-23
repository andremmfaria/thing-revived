#!/usr/bin/env bash
# test-build-image.sh - tests for CONFIG_ONLY mode and the generated flags kas
# file of build-image.sh. No clone, cargo or kas is run.
#
# Usage: build/test-build-image.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD="${SCRIPT_DIR}/build-image.sh"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/thing-revived-test.XXXXXX")"
trap 'rm -rf -- "${WORK}"' EXIT

pass=0
fail=0
ok() { pass=$((pass + 1)); echo "ok   $1"; }
bad() { fail=$((fail + 1)); echo "FAIL $1"; }

# CONFIG_ONLY must work with no SSH key anywhere (none is baked any more).
rc=0
HOME="${WORK}/nohome" CONFIG_ONLY=1 CONFIG_OUT_DIR="${WORK}/cfg" "${BUILD}" >"${WORK}/out" 2>&1 || rc=$?
if [ "${rc}" -eq 0 ] && [ -f "${WORK}/cfg/config.generated.js" ]; then ok "CONFIG_ONLY needs no key"; else bad "CONFIG_ONLY (exit ${rc}: $(tr '\n' ' ' <"${WORK}/out"))"; fi

# The key flow is gone: no key variable or mode may be left in the script.
if grep -Eq 'SSH_PUB''KEY|KEY_''ONLY|authorized_''keys' "${BUILD}"; then bad "build-image.sh still mentions the SSH key flow"; else ok "no SSH key flow in build-image.sh"; fi

# Generated kas file: pull the two helpers out of the script and run them.
sed -n '/^flag_value()/p;/^write_flags_kas()/,/^}/p' "${BUILD}" >"${WORK}/helpers.sh"
# shellcheck source=/dev/null
. "${WORK}/helpers.sh"
check_kas() {
  local name="$1" in_adb="$2" in_uart="$3" want_adb="$4" want_uart="$5" a u
  a="$(flag_value "${in_adb}")"
  u="$(flag_value "${in_uart}")"
  write_flags_kas "${WORK}/flags.yml" "${a}" "${u}"
  if grep -qx "  THING_REVIVED_ENABLE_ADB: \"${want_adb}\"" "${WORK}/flags.yml" \
    && grep -qx "  THING_REVIVED_ENABLE_UART: \"${want_uart}\"" "${WORK}/flags.yml" \
    && grep -qx "    THING_REVIVED_ENABLE_ADB = \"${want_adb}\"" "${WORK}/flags.yml" \
    && grep -qx "    THING_REVIVED_ENABLE_UART = \"${want_uart}\"" "${WORK}/flags.yml"; then
    ok "${name}"
  else
    bad "${name}"
  fi
}
check_kas "kas flags default off" "" "" 0 0
check_kas "kas flags adb only" 1 "" 1 0
check_kas "kas flags uart only" "" 1 0 1
check_kas "kas flags both" 1 1 1 1
check_kas "kas flags non-1 values are off" true yes 0 0

echo "${pass} passed, ${fail} failed"
[ "${fail}" -eq 0 ]
