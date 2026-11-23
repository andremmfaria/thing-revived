#!/usr/bin/env bash
# Self-test of a built rootfs (plan section 14). Runs check_image_accounts.py on the input and,
# for a rootfs directory, validates a COPY of the account files with host tools only
# (sshd -T, visudo -c, pwck -r, grpck -r). Nothing from the image is ever executed.
#
# Usage: rootfs_selftest.sh [ROOTFS_DIR|EXT4|WIC] [check_image_accounts.py options]
# With no argument it runs against the good fixture, which proves the wrapper itself.
# A missing host tool is reported as SKIP, not as a failure.
set -u

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
target="${1:-$here/fixtures/good}"
[ $# -gt 0 ] && shift
extra=("$@")

status=0
python3 "$here/check_image_accounts.py" "$target" "${extra[@]}" || status=1

if [ ! -d "$target" ]; then
    echo "SKIP host tool checks: input is not a rootfs directory"
    exit "$status"
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/rootfs-selftest.XXXXXX")" || exit 2
trap 'rm -rf "$work"' EXIT

# Copy without following symlinks, then keep only what the host tools need.
mkdir -p "$work/etc"
cp -a "$target/etc/." "$work/etc/" 2>/dev/null || {
    echo "FAIL [selftest] cannot copy $target/etc"
    exit 1
}

step() {  # step NAME CMD...
    local name="$1"
    shift
    if out="$("$@" 2>&1)"; then
        echo "PASS [selftest] $name"
    else
        echo "FAIL [selftest] $name: $out"
        status=1
    fi
}

if command -v visudo >/dev/null 2>&1 && visudo --version 2>&1 | grep -qi 'visudo-rs'; then
    echo "SKIP [selftest] visudo is sudo-rs, which rejects sudoers settings of the image's sudo"
elif command -v visudo >/dev/null 2>&1; then
    # Drop include directives so visudo does not read the build host's sudoers.d.
    grep -vE '^[[:space:]]*[@#]include' "$work/etc/sudoers" >"$work/sudoers.check" || true
    chmod 0440 "$work/sudoers.check"
    step "visudo -c" visudo -c -f "$work/sudoers.check"
else
    echo "SKIP [selftest] visudo not installed"
fi

if command -v pwck >/dev/null 2>&1 && [ -f "$work/etc/passwd" ] && [ -f "$work/etc/shadow" ]; then
    # Homes live on /var and do not exist in the image; pwck only needs to parse the files.
    step "pwck -r (parse)" bash -c 'pwck -r -s "$1" "$2" </dev/null; rc=$?; [ $rc -lt 2 ]' _ \
        "$work/etc/passwd" "$work/etc/shadow"
else
    echo "SKIP [selftest] pwck not installed"
fi

if command -v grpck >/dev/null 2>&1 && [ -f "$work/etc/group" ]; then
    gshadow=()
    [ -f "$work/etc/gshadow" ] && gshadow=("$work/etc/gshadow")
    # grpck resolves members against the HOST passwd, so "no user" lines are expected noise.
    step "grpck -r" bash -c '
        out="$(grpck -r "$@" </dev/null 2>&1)"; rc=$?
        bad="$(echo "$out" | grep -vE "^(group .*: no user |delete member |grpck: no changes)" || true)"
        [ -z "$bad" ] || { echo "$bad"; exit 1; }
        [ $rc -le 2 ]' _ "$work/etc/group" "${gshadow[@]}"
else
    echo "SKIP [selftest] grpck not installed"
fi

sshd_bin="$(command -v sshd || ls /usr/sbin/sshd 2>/dev/null || true)"
if [ -n "$sshd_bin" ] && command -v ssh-keygen >/dev/null 2>&1; then
    # Point Include at the copy, add a throwaway host key, then ask sshd for the effective config.
    ssh-keygen -q -t ed25519 -N '' -f "$work/hostkey" >/dev/null 2>&1
    for cfg in sshd_config sshd_config_readonly; do
        [ -f "$work/etc/ssh/$cfg" ] || continue
        sed "s#^\([[:space:]]*Include[[:space:]]\+\)/etc/#\1$work/etc/#" \
            "$work/etc/ssh/$cfg" >"$work/$cfg.check"
        step "sshd -T $cfg" bash -c '
            run() { "$1" -T -h "$2" -f "$3" -C "user=$4,host=localhost,addr=127.0.0.1" 2>&1; }
            kiosk="$(run "$1" "$2" "$3" kiosk)" || { echo "$kiosk"; exit 1; }
            root="$(run "$1" "$2" "$3" root)" || { echo "$root"; exit 1; }
            # kiosk: password or key. Everyone else, root included: key only.
            echo "$kiosk" | grep -qx "permitrootlogin no" &&
            echo "$kiosk" | grep -qx "passwordauthentication yes" &&
            echo "$kiosk" | grep -qx "authenticationmethods password publickey" &&
            echo "$kiosk" | grep -qx "permitemptypasswords no" &&
            echo "$kiosk" | grep -qx "kbdinteractiveauthentication no" &&
            echo "$root" | grep -qx "permitrootlogin no" &&
            echo "$root" | grep -qx "passwordauthentication no" &&
            echo "$root" | grep -qx "authenticationmethods publickey" &&
            echo "$root" | grep -qx "permitemptypasswords no" &&
            echo "$root" | grep -qx "kbdinteractiveauthentication no" ||
                { echo "unexpected effective settings"; exit 1; }' _ \
            "$sshd_bin" "$work/hostkey" "$work/$cfg.check"
    done
else
    echo "SKIP [selftest] sshd or ssh-keygen not installed"
fi

exit "$status"
