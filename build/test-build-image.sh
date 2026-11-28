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
# The fake HOME hides the real mise config, so mise would refuse it as untrusted.
# Trust the real config dir and the repo through the env var (never `mise trust`).
REAL_HOME="${HOME}"
export MISE_TRUSTED_CONFIG_PATHS="${REAL_HOME}/.config/mise:$(cd "${SCRIPT_DIR}/.." && pwd)${MISE_TRUSTED_CONFIG_PATHS:+:${MISE_TRUSTED_CONFIG_PATHS}}"
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

# Layout variables: each one must be emitted exactly once, in local_conf_header
# (indented 4 spaces), and never as an env passthrough entry.
write_flags_kas "${WORK}/flags.yml" 0 0 "20260101T000000Z-nogit"
check_var() {
  local var="$1" want="$2" n
  n="$(grep -cx "    ${var} = \"${want}\"" "${WORK}/flags.yml" || true)"
  if [ "${n}" = "1" ]; then ok "kas emits ${var} = ${want}"; else bad "kas ${var} = ${want} expected once, found ${n}"; fi
  if grep -q "^  ${var}:" "${WORK}/flags.yml"; then bad "kas ${var} must not be in the env map"; else ok "kas ${var} not in env map"; fi
}
check_var SUPERBIRD_ROOT_PART_SIZE 650
check_var SUPERBIRD_BANDAID_PART_SIZE 0
# Both are inside the local_conf_header block, after its key.
if awk '/^local_conf_header:/{f=1;next} f&&/SUPERBIRD_ROOT_PART_SIZE = "650"/{a=1} f&&/SUPERBIRD_BANDAID_PART_SIZE = "0"/{b=1} END{exit !(a&&b)}' "${WORK}/flags.yml"; then ok "layout vars are under local_conf_header"; else bad "layout vars not under local_conf_header"; fi
# The layout vars do not depend on the dev flags.
write_flags_kas "${WORK}/flags.yml" 1 1 "20260101T000000Z-nogit"
if grep -qx '    SUPERBIRD_ROOT_PART_SIZE = "650"' "${WORK}/flags.yml" && grep -qx '    SUPERBIRD_BANDAID_PART_SIZE = "0"' "${WORK}/flags.yml"; then ok "layout vars independent of adb/uart flags"; else bad "layout vars changed with adb/uart flags"; fi
# The OTA headroom arithmetic the comment in build-image.sh documents.
req=$((64 + 650 + 64)); fixed=$((8 + 2 * 64 + 2 * 650)); floor=$((3600 - fixed))
if [ "${req}" -eq 778 ] && [ "${floor}" -eq 2164 ] && [ "${floor}" -ge "${req}" ]; then ok "OTA headroom: required ${req} <= data floor ${floor} MiB"; else bad "OTA headroom arithmetic (${req}/${floor})"; fi
if grep -Eq '=[ ]+778 MiB' "${BUILD}" && grep -Eq '=[ ]+2164 MiB' "${BUILD}"; then ok "build-image.sh documents the headroom numbers"; else bad "headroom numbers missing from build-image.sh comment"; fi

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
