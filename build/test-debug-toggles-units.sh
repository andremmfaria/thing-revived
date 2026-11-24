#!/usr/bin/env bash
# Ordering-cycle check for the debug-toggles unit changes (T-D1.1/T-D1.2,
# docs/research/debug-toggles-plan.md section 8, item c of the spike). `systemd-analyze verify`
# exits 0 even when it prints an ordering cycle, so its TEXT output is what gets checked, the same
# pattern build/tools/rootfs_selftest.sh already uses for the other thing-revived units.
#
# This script owns its own stub root (it does not touch build/tools/rootfs_selftest.sh, which is
# owned by a different task): it copies the real shipped unit files from build/overlay/ and lays
# down minimal stubs for everything they reference (the base android-tools-adbd.service, the
# stock serial-getty@.service, superbird-usb-gadget.service, superbird-usb-role.service, basic/
# multi-user/getty/sockets/sysinit targets, dev-ttyAML0.device), then runs `systemd-analyze
# verify` against both pair heads (android-tools-adbd.service / thing-revived-usb-bind.service)
# and the UART instance (thing-revived-uart-root@ttyAML0.service / serial-getty@ttyAML0.service).
#
# Usage: build/test-debug-toggles-units.sh
set -u

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
overlay="$here/overlay"

status=0

if ! command -v systemd-analyze >/dev/null 2>&1; then
    echo "SKIP [debug-toggles-units] systemd-analyze not installed"
    exit 0
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/debug-toggles-units.XXXXXX")" || exit 2
trap 'rm -rf "$work"' EXIT

root="$work/unitroot"
sys="$root/usr/lib/systemd/system"
mkdir -p "$sys" "$sys/android-tools-adbd.service.d" "$root/usr/libexec/thing-revived" "$root/bin"

stub() { printf '[Unit]\nDescription=stub\n%s\n' "${2:-}" >"$sys/$1"; }

# --- real shipped files under test ----------------------------------------
cp "$overlay/recipes-extended/thing-revived-usb-bind/files/thing-revived-usb-bind.service" \
    "$sys/thing-revived-usb-bind.service"
cp "$overlay/recipes-extended/thing-revived-uart-root/files/thing-revived-uart-root@.service" \
    "$sys/thing-revived-uart-root@.service"
cp "$overlay/recipes-core/superbird-usb-gadget/superbird-usb-gadget/zz-thing-revived-toggle.conf" \
    "$sys/android-tools-adbd.service.d/zz-thing-revived-toggle.conf"
# The @@...@@ ExecStart placeholder is only ever substituted at image build time
# (thing_revived_console_gate); fill it with a throwaway command here so the unit parses.
sed -i 's|@@THING_REVIVED_STOCK_GETTY_ARGS@@|/bin/true|' "$sys/thing-revived-uart-root@.service"

printf '#!/bin/sh\n' >"$root/usr/libexec/thing-revived/thing-revived-usb-bind"
chmod 0755 "$root/usr/libexec/thing-revived/thing-revived-usb-bind"
for bin in true ln; do
    printf '#!/bin/sh\n' >"$root/bin/$bin"
    chmod 0755 "$root/bin/$bin"
done

# --- stubs for everything the real units reference -------------------------
# android-tools-adbd.service: meta-oe's base unit (F1 in debug-toggles-plan.md), WantedBy=basic.target.
stub android-tools-adbd.service $'Requires=superbird-usb-gadget.service\nAfter=superbird-usb-gadget.service\n[Service]\nExecStartPre=-/bin/true\nExecStart=/bin/true\nExecStop=/bin/true\n[Install]\nWantedBy=basic.target'
stub superbird-usb-gadget.service $'[Service]\nExecStart=/bin/true'
stub superbird-usb-role.service $'[Service]\nExecStart=/bin/true'
# serial-getty@.service: the SHARED template must stay untouched/autologin-free (plan 4.4); this
# stub stands in for it unmodified, exactly as every build ships it.
stub serial-getty@.service $'ConditionPathExists=/dev/%I\n[Service]\nExecStart=-/sbin/agetty --noreset --noclear --keep-baud 115200,38400,9600 %I $TERM\n[Install]\nWantedBy=getty.target'
stub dev-ttyAML0.device ''
stub basic.target $'Wants=sockets.target sysinit.target\nAfter=sockets.target sysinit.target'
stub sockets.target ''
stub sysinit.target ''
stub multi-user.target $'Wants=basic.target\nAfter=basic.target'
stub getty.target ''
stub shutdown.target ''

# android-tools-adbd.service is WantedBy=basic.target; thing-revived-usb-bind.service is
# WantedBy=multi-user.target (see thing_revived_adb_gate). Enable one side at a time, the same
# "exactly one enabled" invariant thing_revived_assert_accounts checks on a real image, so the
# verify run matches a real boot's transaction instead of an impossible both-disabled graph.
mkdir -p "$root/etc/systemd/system/basic.target.wants" "$root/etc/systemd/system/multi-user.target.wants" \
    "$root/etc/systemd/system/getty.target.wants"
ln -sf "$sys/android-tools-adbd.service" "$root/etc/systemd/system/basic.target.wants/android-tools-adbd.service"
ln -sf "$sys/serial-getty@.service" "$root/etc/systemd/system/getty.target.wants/serial-getty@ttyAML0.service"

run_verify() {  # run_verify LABEL UNIT...
    local label="$1" out rc
    shift
    out="$(timeout 60 env -u SYSTEMD_UNIT_PATH systemd-analyze verify --root="$root" --man=no "$@" 2>&1)"
    rc=$?
    if [ "$rc" -ne 0 ] || printf '%s\n' "$out" | grep -qi 'ordering cycle'; then
        echo "FAIL [debug-toggles-units] $label: $out"
        status=1
    else
        echo "PASS [debug-toggles-units] $label (no ordering cycle)"
    fi
}

run_verify "adb/usb-bind pair (ADB enabled)" basic.target android-tools-adbd.service thing-revived-usb-bind.service
run_verify "UART pair (serial-getty enabled)" getty.target thing-revived-uart-root@ttyAML0.service serial-getty@ttyAML0.service

# Flip to the other boot default (THING_REVIVED_ENABLE_ADB=1 / THING_REVIVED_ENABLE_UART=1) and
# verify again, so both halves of thing_revived_adb_gate's and thing_revived_console_gate's
# enablement switch are exercised, not just the flag=0 default.
rm -f "$root/etc/systemd/system/basic.target.wants/android-tools-adbd.service" \
    "$root/etc/systemd/system/getty.target.wants/serial-getty@ttyAML0.service"
ln -sf "$sys/thing-revived-usb-bind.service" "$root/etc/systemd/system/multi-user.target.wants/thing-revived-usb-bind.service"
ln -sf "$sys/thing-revived-uart-root@.service" "$root/etc/systemd/system/getty.target.wants/thing-revived-uart-root@ttyAML0.service"
run_verify "adb/usb-bind pair (usb-bind enabled)" multi-user.target android-tools-adbd.service thing-revived-usb-bind.service
run_verify "UART pair (uart-root enabled)" getty.target thing-revived-uart-root@ttyAML0.service serial-getty@ttyAML0.service

exit "$status"
