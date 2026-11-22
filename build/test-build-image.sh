#!/usr/bin/env bash
# test-build-image.sh - tests for the SSH key validation (KEY_ONLY=1) and the
# generated flags kas file of build-image.sh. No clone, cargo or kas is run.
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

# key_only <name> <expected: pass|fail> <key file>
key_only() {
  local name="$1" want="$2" file="$3" rc=0
  THING_REVIVED_SSH_PUBKEY_FILE="${file}" KEY_ONLY=1 "${BUILD}" >"${WORK}/out" 2>&1 || rc=$?
  if { [ "${want}" = pass ] && [ "${rc}" -eq 0 ]; } || { [ "${want}" = fail ] && [ "${rc}" -ne 0 ]; }; then
    ok "${name}"
  else
    bad "${name} (exit ${rc}: $(tr '\n' ' ' <"${WORK}/out"))"
  fi
}

ssh-keygen -q -t ed25519 -N '' -C 'test@example' -f "${WORK}/id_a"
ssh-keygen -q -t ed25519 -N '' -C 'other@example' -f "${WORK}/id_b"

: >"${WORK}/empty"
cat "${WORK}/id_a.pub" "${WORK}/id_b.pub" >"${WORK}/multi.pub"
printf '# my key\n\n%s\n' "$(cat "${WORK}/id_a.pub")" >"${WORK}/commented.pub"
printf 'command="/bin/true" %s\n' "$(cat "${WORK}/id_a.pub")" >"${WORK}/options.pub"
printf 'ssh-ed25519 not-base64-@@@ x\n' >"${WORK}/garbage.pub"
printf '# only a comment\n' >"${WORK}/comment-only"

key_only "valid key" pass "${WORK}/id_a.pub"
key_only "valid key with comment and blank lines" pass "${WORK}/commented.pub"
key_only "missing file" fail "${WORK}/nope.pub"
key_only "empty file" fail "${WORK}/empty"
key_only "private key file" fail "${WORK}/id_a"
key_only "two keys" fail "${WORK}/multi.pub"
key_only "key with authorized_keys options" fail "${WORK}/options.pub"
key_only "garbage key" fail "${WORK}/garbage.pub"
key_only "comment-only file" fail "${WORK}/comment-only"

# The default path is ~/.ssh/id_ed25519.pub: point HOME at an empty dir.
rc=0
HOME="${WORK}/nohome" KEY_ONLY=1 "${BUILD}" >"${WORK}/out" 2>&1 || rc=$?
if [ "${rc}" -ne 0 ] && grep -q "nohome/.ssh/id_ed25519.pub" "${WORK}/out"; then ok "default path used when variable unset"; else bad "default path"; fi

# CONFIG_ONLY must not need a key.
rc=0
THING_REVIVED_SSH_PUBKEY_FILE="${WORK}/nope.pub" CONFIG_ONLY=1 CONFIG_OUT_DIR="${WORK}/cfg" "${BUILD}" >"${WORK}/out" 2>&1 || rc=$?
if [ "${rc}" -eq 0 ]; then ok "CONFIG_ONLY skips the key check"; else bad "CONFIG_ONLY (exit ${rc})"; fi

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
