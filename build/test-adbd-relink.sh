#!/usr/bin/env bash
# Regression test for the adbd drop-in's ExecStartPre relink (the BusyBox ln -sf dereference bug:
# `ln -sf TARGET LINK` on a LINK that is already a symlink to a directory dereferences it and
# tries to create the new link INSIDE that directory, which configfs refuses with
# "Operation not permitted"; see zz-thing-revived-toggle.conf for the full device evidence).
#
# This extracts the real ExecStartPre command from the shipped drop-in, rewrites only the CFG
# path literal to point at a fake configfs tree under a temp dir, and runs it for the three
# states the device can boot into: link absent + UDC bound (the state thing-revived-usb-bind's
# `start` leaves), link present (UDC bound or unbound, the no-op case), and link absent + UDC
# already unbound. Uses the real BusyBox `ln` when available on the host (matches the device's
# BusyBox v1.37, where the bug was observed); otherwise falls back to the host's `ln` with a note,
# since the non-dereferencing property under test (presence-checked, never `ln -sf` on an
# existing link) does not depend on which `ln` implementation runs it.
#
# Usage: build/test-adbd-relink.sh
set -u

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dropin="$here/overlay/recipes-core/superbird-usb-gadget/superbird-usb-gadget/zz-thing-revived-toggle.conf"

status=0

if [ ! -f "$dropin" ]; then
    echo "FAIL [adbd-relink] drop-in not found: $dropin"
    exit 2
fi

real_cfg="/sys/kernel/config/usb_gadget/superbird-kiosk"
cmd_line="$(grep -E '^ExecStartPre=' "$dropin")" || {
    echo "FAIL [adbd-relink] no ExecStartPre= line in $dropin"
    exit 2
}
# Strip the ExecStartPre= key and the /bin/sh -c wrapper, leaving the quoted inner script.
script="${cmd_line#ExecStartPre=}"
case "$script" in
    "/bin/sh -c '"*"'")
        script="${script#/bin/sh -c \'}"
        script="${script%\'}"
        ;;
    *)
        echo "FAIL [adbd-relink] ExecStartPre is not a /bin/sh -c '...' form: $cmd_line"
        exit 2
        ;;
esac

work="$(mktemp -d "${TMPDIR:-/tmp}/adbd-relink.XXXXXX")" || exit 2
trap 'rm -rf "$work"' EXIT
fake_cfg="$work/cfg"

# Point the extracted script at the fake configfs tree instead of the real sysfs path.
test_script="${script//$real_cfg/$fake_cfg}"
if [ "$test_script" = "$script" ]; then
    echo "FAIL [adbd-relink] CFG path $real_cfg not found in ExecStartPre; extraction or the drop-in's path literal changed"
    exit 2
fi

bin_dir="$work/bin"
mkdir -p "$bin_dir"
if command -v busybox >/dev/null 2>&1; then
    printf '#!/bin/sh\nexec busybox ln "$@"\n' >"$bin_dir/ln"
    chmod 0755 "$bin_dir/ln"
    ln_note="real BusyBox ln (via busybox applet, matches the device's BusyBox v1.37)"
else
    ln_note="host ln (GNU coreutils semantics; busybox not found on this host, not device-exact for the -f case, but this test never calls ln -sf, only the presence-guarded ln -s, which both implementations create identically)"
fi

reset_fixture() {  # reset_fixture LINK_STATE(present|absent) UDC_STATE(bound|unbound)
    rm -rf "$fake_cfg"
    mkdir -p "$fake_cfg/configs/c.1" "$fake_cfg/functions/ffs.adb"
    if [ "$1" = present ]; then
        ln -s "$fake_cfg/functions/ffs.adb" "$fake_cfg/configs/c.1/ffs.adb"
    fi
    if [ "$2" = bound ]; then
        printf 'ff400000.usb' >"$fake_cfg/UDC"
    else
        : >"$fake_cfg/UDC"
    fi
}

run_case() {  # run_case LABEL LINK_STATE UDC_STATE
    local label="$1" link_state="$2" udc_state="$3" out rc link_path target_path
    reset_fixture "$link_state" "$udc_state"
    link_path="$fake_cfg/configs/c.1/ffs.adb"
    target_path="$fake_cfg/functions/ffs.adb"
    out="$(PATH="$bin_dir:$PATH" sh -c "$test_script" 2>&1)"
    rc=$?
    if [ "$rc" -ne 0 ]; then
        echo "FAIL [adbd-relink] $label: ExecStartPre exited $rc: $out"
        status=1
        return
    fi
    if [ ! -L "$link_path" ]; then
        echo "FAIL [adbd-relink] $label: $link_path is not a symlink after the run"
        status=1
        return
    fi
    if [ -e "$link_path/ffs.adb" ]; then
        echo "FAIL [adbd-relink] $label: nested $link_path/ffs.adb was created (the dereference bug)"
        status=1
        return
    fi
    resolved="$(readlink "$link_path")"
    if [ "$resolved" != "$target_path" ]; then
        echo "FAIL [adbd-relink] $label: $link_path -> $resolved, expected -> $target_path"
        status=1
        return
    fi
    echo "PASS [adbd-relink] $label"
}

echo "INFO [adbd-relink] using $ln_note"

# State 1: link absent, UDC bound -- thing-revived-usb-bind's `start` leaves exactly this.
run_case "link absent, UDC bound" absent bound

# State 2: link already present, UDC bound -- must be a pure no-op, never touch UDC or ln.
reset_fixture present bound
PATH="$bin_dir:$PATH" sh -c "$test_script" >"$work/out" 2>&1
rc=$?
udc_after="$(cat "$fake_cfg/UDC")"
if [ "$rc" -ne 0 ] || [ "$udc_after" != "ff400000.usb" ]; then
    echo "FAIL [adbd-relink] link present, UDC bound: expected a no-op leaving UDC bound, rc=$rc udc=$udc_after"
    status=1
elif [ ! -L "$fake_cfg/configs/c.1/ffs.adb" ] || [ -e "$fake_cfg/configs/c.1/ffs.adb/ffs.adb" ]; then
    echo "FAIL [adbd-relink] link present, UDC bound: link damaged or nested entry created"
    status=1
else
    echo "PASS [adbd-relink] link present, UDC bound (no-op, UDC untouched)"
fi

# State 3: link present, UDC unbound -- also a no-op.
run_case "link present, UDC unbound" present unbound

# State 4: link absent, UDC already unbound -- the echo "" must be tolerated, link still created.
run_case "link absent, UDC unbound" absent unbound

exit "$status"
