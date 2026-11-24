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

# Behavioural check of the /etc overlay mode logic, without mount or root: run only the chmod lines
# of the shipped script that touch $UPPER_BASE against a scratch tree whose upper dir starts at the
# old 0700 (a device that booted before the fix), and assert the resulting modes.
check_etc_modes() {
    local script="$target/usr/libexec/thing-revived/thing-revived-etc-mount.sh" base="$work/etc-modes"
    if [ ! -f "$script" ]; then
        echo "SKIP [selftest] etc overlay modes: thing-revived-etc-mount.sh not in the rootfs"
        return 0
    fi
    mkdir -p "$base/upper" "$base/work"
    chmod 0700 "$base" "$base/upper" "$base/work"
    local lines
    lines="$(grep -E '^[[:space:]]*chmod [0-7]+ .*UPPER_BASE' "$script")"
    # The lines only use $UPPER_BASE; run them as a throwaway sh script, twice to show it is idempotent.
    printf 'UPPER_BASE=%s\n%s\n%s\n' "$base" "$lines" "$lines" >"$work/etc-modes.sh"
    if ! sh "$work/etc-modes.sh" 2>"$work/etc-modes.err"; then
        echo "FAIL [selftest] etc overlay modes: $(cat "$work/etc-modes.err")"
        status=1
        return 0
    fi
    local got want="0755 0700 0700"
    got="$(stat -c %a "$base/upper" "$base" "$base/work" | sed 's/^/0/' | tr '\n' ' ')"
    if [ "${got% }" = "$want" ]; then
        echo "PASS [selftest] etc overlay modes (upper 0755, parent 0700, work 0700)"
    else
        echo "FAIL [selftest] etc overlay modes: upper/parent/work are ${got% }, expected $want"
        status=1
    fi
}
check_etc_modes

# Ordering check of the shipped units. The unit files are copied next to stub targets that model
# the real graph (sysinit.target and sockets.target before basic.target), because
# `systemd-analyze verify` exits 0 on an ordering cycle and only prints it. Cycle text is a failure.
verify_units() {
    local unitdir="" d root="$work/unitroot" sys u
    for d in usr/lib/systemd/system lib/systemd/system; do
        if [ -f "$target/$d/thing-revived-provision.service" ]; then
            unitdir="$target/$d"
            break
        fi
    done
    if [ -z "$unitdir" ]; then
        echo "SKIP [selftest] systemd-analyze verify: thing-revived units not in the rootfs"
        return 0
    fi
    sys="$root/usr/lib/systemd/system"
    mkdir -p "$sys" "$root/usr/bin" "$root/usr/libexec/thing-revived" "$root/bin"
    for u in "$unitdir"/thing-revived-*.service "$unitdir"/sshd.socket; do
        [ -f "$u" ] && cp "$u" "$sys/"
    done
    for u in usr/bin/controller-agent usr/libexec/thing-revived/thing-revived-etc-mount.sh usr/libexec/thing-revived/thing-revived-usb-bind bin/true; do
        printf '#!/bin/sh\n' >"$root/$u"
        chmod 0755 "$root/$u"
    done
    stub() { printf '[Unit]\nDescription=stub\n%s\n' "${2:-}" >"$sys/$1"; }
    stub basic.target $'Wants=sockets.target sysinit.target\nAfter=sockets.target sysinit.target'
    stub sockets.target 'Wants=sshd.socket'
    stub sysinit.target $'Wants=thing-revived-etc.service thing-revived-provision.service'
    stub getty.target
    stub shutdown.target
    stub nss-user-lookup.target
    stub var.mount $'[Mount]\nWhat=tmpfs\nWhere=/var'
    stub systemd-tmpfiles-setup.service $'[Service]\nExecStart=/bin/true'
    stub controller-agent.service $'[Service]\nExecStart=/bin/true'
    stub thing-revived-terminal-agent.service $'[Service]\nExecStart=/bin/true'
    stub sshd.service $'[Service]\nExecStart=/bin/true'
    stub superbird-usb-gadget.service $'[Service]\nExecStart=/bin/true'
    stub superbird-usb-role.service $'[Service]\nExecStart=/bin/true'
    [ -f "$sys/sshd.socket" ] || stub sshd.socket $'Before=sockets.target\n[Socket]\nListenStream=22'
    local out rc
    out="$(env -u SYSTEMD_UNIT_PATH systemd-analyze verify --root="$root" --man=no basic.target thing-revived-usb-bind.service 2>&1)"
    rc=$?
    if [ "$rc" -ne 0 ] || printf '%s\n' "$out" | grep -qi 'ordering cycle'; then
        echo "FAIL [selftest] systemd-analyze verify: $out"
        status=1
    else
        echo "PASS [selftest] systemd-analyze verify (no ordering cycle)"
    fi
}

if command -v systemd-analyze >/dev/null 2>&1; then
    verify_units
else
    echo "SKIP [selftest] systemd-analyze not installed"
fi

exit "$status"
