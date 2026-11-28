#!/bin/sh
# Wipe a stale bandaid filesystem found at the start of the data partition.
#
# A device flashed with the old layout has an ext4 labelled "bandaid" at the byte offset where
# the new data partition starts. x-systemd.makefs only formats a device that carries no
# filesystem signature, so /var would mount that stale 192 MiB filesystem. Clearing its
# signature here lets makefs format the partition fresh. A data partition with any other label
# (including none) is never touched. This script never blocks boot: every path exits 0.
#
# BusyBox blkid takes no -s/-o flags, so the label is parsed from the plain
# `blkid DEVICE` output (`DEVICE: LABEL="x" UUID=... TYPE=...`), which util-linux blkid prints
# in the same shape. The path assignments below are build-time placeholders filled in by
# do_install; there are no runtime overrides.

DEV="@@DATA_DEV@@"
BLKID="@@BLKID@@"
WIPEFS="@@WIPEFS@@"
UDEVADM="@@UDEVADM@@"
MOUNTS="@@MOUNTS@@"

log() {
    echo "thing-revived-data-guard: $*"
}

if [ ! -e "$DEV" ]; then
    "$UDEVADM" settle --timeout=20 >/dev/null 2>&1
fi
if [ ! -e "$DEV" ]; then
    log "$DEV absent; nothing to guard"
    exit 0
fi

real=$(readlink -f "$DEV" 2>/dev/null)
if [ -n "$real" ] && grep -q "^$real " "$MOUNTS" 2>/dev/null; then
    log "$real is mounted; leaving it alone"
    exit 0
fi

probe=$("$BLKID" "$DEV" 2>/dev/null)
if [ -z "$probe" ]; then
    log "$DEV carries no readable filesystem signature; makefs will format it"
    exit 0
fi

label=$(printf '%s\n' "$probe" | sed -n 's/.*[[:space:]]LABEL="\([^"]*\)".*/\1/p' | head -n 1)
if [ "$label" != "bandaid" ]; then
    log "$DEV label is '${label}'; not stale, leaving it alone"
    exit 0
fi

log "$DEV carries the stale bandaid filesystem; wiping its signature so makefs formats it fresh"
if "$WIPEFS" -a "$DEV" >/dev/null 2>&1; then
    log "wiped $DEV"
else
    log "wipefs failed on $DEV; /var may mount the stale filesystem"
fi
exit 0
