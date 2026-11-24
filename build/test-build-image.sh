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
sed -n '/^flag_value()/p;/^compute_image_id()/,/^}/p;/^write_flags_kas()/,/^}/p' "${BUILD}" >"${WORK}/helpers.sh"
# shellcheck source=/dev/null
. "${WORK}/helpers.sh"
check_kas() {
  local name="$1" in_adb="$2" in_uart="$3" want_adb="$4" want_uart="$5" a u
  a="$(flag_value "${in_adb}")"
  u="$(flag_value "${in_uart}")"
  write_flags_kas "${WORK}/flags.yml" "${a}" "${u}" "20260101T000000Z-nogit"
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

# Image id: shape, non-empty, git and nogit cases, and presence in both places
# of the generated kas file.
ID_RE='^[0-9]{8}T[0-9]{6}Z-([0-9a-f]{8}|nogit)$'
check_id() {
  local name="$1" root="$2" want="$3" id suffix
  id="$(compute_image_id "${root}")"
  suffix="${id#*Z-}"
  if [ -z "${id}" ] || ! printf '%s\n' "${id}" | grep -Eq "${ID_RE}"; then bad "${name}: bad shape '${id}'"; return; fi
  if [ "${want}" = "nogit" ] && [ "${suffix}" != "nogit" ]; then bad "${name}: expected nogit, got '${id}'"; return; fi
  if [ "${want}" = "git" ] && [ "${suffix}" = "nogit" ]; then bad "${name}: expected a git rev, got '${id}'"; return; fi
  write_flags_kas "${WORK}/flags.yml" 0 0 "${id}"
  if grep -qx "  THING_REVIVED_IMAGE_ID: \"${id}\"" "${WORK}/flags.yml" \
    && grep -qx "    THING_REVIVED_IMAGE_ID = \"${id}\"" "${WORK}/flags.yml"; then
    ok "${name} (${id})"
  else
    bad "${name}: id missing from kas file"
  fi
}
mkdir -p "${WORK}/plain"
check_id "image id nogit (plain dir)" "${WORK}/plain" nogit
if command -v git >/dev/null 2>&1; then
  mkdir -p "${WORK}/repo"
  git -C "${WORK}/repo" init -q
  git -C "${WORK}/repo" -c user.name=t -c user.email=t@example.invalid commit -q --allow-empty -m init
  check_id "image id git (temp repo)" "${WORK}/repo" git
  mkdir -p "${WORK}/repo/sub"
  check_id "image id nogit (subdir of a repo is not a checkout root)" "${WORK}/repo/sub" nogit
  git -C "${WORK}/plain" init -q
  check_id "image id nogit (repo without commits)" "${WORK}/plain" nogit
else
  bad "git not installed, cannot test the git case"
fi
a="$(compute_image_id "${WORK}/plain")"
if [ -n "${a}" ]; then ok "image id never empty"; else bad "image id empty"; fi
# The id must reach the real script's call site, not only the helper.
# shellcheck disable=SC2016
if grep -q 'write_flags_kas .*"\${IMAGE_ID}"' "${BUILD}" && grep -q '^export THING_REVIVED_IMAGE_ID=' "${BUILD}"; then ok "build-image.sh passes and exports the image id"; else bad "build-image.sh does not wire the image id"; fi

echo "${pass} passed, ${fail} failed"
[ "${fail}" -eq 0 ]
