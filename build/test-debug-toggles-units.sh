#!/usr/bin/env bash
# Ordering-cycle check for the device-settings unit set that is SHIPPED DISABLED: the adbd and
# usb-bind pair, the uart-root console unit and the stock serial getty, together with the boot
# apply units (docs/research/device-settings-plan.md sections 4.4 and 7.2). `systemd-analyze
# verify` exits 0 even when it prints an ordering cycle, so its TEXT output is what gets checked,
# the same pattern build/tools/rootfs_selftest.sh and build/tools/check_unit_graph.sh use. The
# sshd.socket side of the graph is covered by check_unit_graph.sh and is not repeated here.
#
# This script owns its own stub root: it copies the real shipped unit files from build/overlay/
# and lays down minimal stubs for everything they reference (the base android-tools-adbd.service,
# the stock serial-getty@.service, superbird-usb-gadget.service, superbird-usb-role.service, the
# basic/multi-user/getty/sockets/sysinit targets, dev-ttyAML0.device). Nothing of the pair or the
# console is enabled, exactly as on the image (only units A and B carry enablement symlinks),
# and the verify run names every shipped unit explicitly, so Conflicts= and After= between them
# are part of the transaction.
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
apply="$overlay/recipes-extended/thing-revived-device-apply/files"
mkdir -p "$sys" "$sys/android-tools-adbd.service.d" "$root/usr/libexec/thing-revived" "$root/usr/bin" "$root/bin"

stub() { printf '[Unit]\nDescription=stub\n%s\n' "${2:-}" >"$sys/$1"; }

# --- real shipped files under test ----------------------------------------
cp "$overlay/recipes-extended/thing-revived-usb-bind/files/thing-revived-usb-bind.service" \
    "$sys/thing-revived-usb-bind.service"
cp "$overlay/recipes-extended/thing-revived-uart-root/files/thing-revived-uart-root@.service" \
    "$sys/thing-revived-uart-root@.service"
cp "$overlay/recipes-core/superbird-usb-gadget/superbird-usb-gadget/zz-thing-revived-toggle.conf" \
    "$sys/android-tools-adbd.service.d/zz-thing-revived-toggle.conf"
cp "$apply/thing-revived-device-settings.service" "$apply/thing-revived-device-apply.service" \
    "$apply/thing-revived-device-failsafe.service" "$sys/"
# The @@...@@ ExecStart placeholder is only ever substituted at image build time
# (thing_revived_console_gate); fill it with a throwaway command here so the unit parses.
sed -i 's|@@THING_REVIVED_STOCK_GETTY_ARGS@@|/bin/true|' "$sys/thing-revived-uart-root@.service"

printf '#!/bin/sh\n' >"$root/usr/libexec/thing-revived/thing-revived-usb-bind"
chmod 0755 "$root/usr/libexec/thing-revived/thing-revived-usb-bind"
printf '#!/bin/sh\n' >"$root/usr/bin/controller-agent"
chmod 0755 "$root/usr/bin/controller-agent"
for bin in true ln sh; do
    printf '#!/bin/sh\n' >"$root/bin/$bin"
    chmod 0755 "$root/bin/$bin"
done

# --- stubs for everything the real units reference -------------------------
# android-tools-adbd.service: meta-oe's base unit, WantedBy=basic.target upstream, shipped disabled.
stub android-tools-adbd.service $'Requires=superbird-usb-gadget.service\nAfter=superbird-usb-gadget.service\n[Service]\nExecStartPre=-/bin/true\nExecStart=/bin/true\nExecStop=/bin/true\n[Install]\nWantedBy=basic.target'
stub superbird-usb-gadget.service $'[Service]\nExecStart=/bin/true'
stub superbird-usb-role.service $'[Service]\nExecStart=/bin/true'
# serial-getty@.service: the SHARED template stays untouched and autologin-free; this stub stands
# in for it unmodified, exactly as every build ships it.
stub serial-getty@.service $'ConditionPathExists=/dev/%I\n[Service]\nExecStart=-/sbin/agetty --noreset --noclear --keep-baud 115200,38400,9600 %I $TERM\n[Install]\nWantedBy=getty.target'
stub dev-ttyAML0.device ''
stub var.mount $'[Mount]\nWhat=tmpfs\nWhere=/var\nType=tmpfs'
stub systemd-tmpfiles-setup.service $'DefaultDependencies=no\nBefore=sysinit.target\n[Service]\nExecStart=/bin/true'
stub basic.target $'Wants=sockets.target sysinit.target\nAfter=sockets.target sysinit.target'
stub sockets.target ''
stub sysinit.target ''
stub multi-user.target $'Wants=basic.target\nAfter=basic.target'
stub getty.target ''
stub shutdown.target ''

# Only the boot apply units are enabled; the pair and the console ship disabled.
mkdir -p "$root/etc/systemd/system/sysinit.target.wants" "$root/etc/systemd/system/multi-user.target.wants"
ln -sf "$sys/thing-revived-device-settings.service" "$root/etc/systemd/system/sysinit.target.wants/thing-revived-device-settings.service"
ln -sf "$sys/thing-revived-device-apply.service" "$root/etc/systemd/system/multi-user.target.wants/thing-revived-device-apply.service"

run_verify() {  # run_verify LABEL UNIT...
    local label="$1" out rc
    shift
    out="$(timeout 60 env -u SYSTEMD_UNIT_PATH systemd-analyze verify --root="$root" --man=no "$@" 2>&1)"
    rc=$?
    if [ "$rc" -ne 0 ] || printf '%s\n' "$out" | grep -Eqi 'ordering cycle|Found dependency'; then
        echo "FAIL [debug-toggles-units] $label: $out"
        status=1
    else
        echo "PASS [debug-toggles-units] $label (no ordering cycle)"
    fi
}

run_verify "adb/usb-bind pair (both disabled)" basic.target android-tools-adbd.service thing-revived-usb-bind.service
run_verify "UART pair (uart-root and stock getty, both disabled)" getty.target thing-revived-uart-root@ttyAML0.service serial-getty@ttyAML0.service
run_verify "boot apply units with the pair and the console" multi-user.target sysinit.target \
    thing-revived-device-settings.service thing-revived-device-apply.service thing-revived-device-failsafe.service \
    android-tools-adbd.service thing-revived-usb-bind.service \
    thing-revived-uart-root@ttyAML0.service serial-getty@ttyAML0.service

exit "$status"
