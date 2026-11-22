#!/bin/sh
# Mount a writable overlay on /etc so shadow-utils can rename passwd, shadow, group and gshadow.
# Lower layer: the read-only rootfs /etc. Upper and work: /var/lib/thing-revived/etc.
# Fixed strings only, no inputs. On any failure /etc stays the read-only seed (degraded mode).
set -eu

UPPER_BASE=/var/lib/thing-revived/etc
RUN_DIR=/run/thing-revived
LOWER=$RUN_DIR/etc-lower
SAVED_ID=$RUN_DIR/machine-id

log() { echo "thing-revived-etc: $*" >&2; }

etc_is_overlay() {
    grep -Eq '^[0-9]+ [0-9]+ [0-9:]+ [^ ]+ /etc [^-]* - overlay ' /proc/self/mountinfo
}

start() {
    if etc_is_overlay; then
        log "overlay already mounted on /etc"
        return 0
    fi

    mkdir -p /var/lib/thing-revived
    chmod 0700 /var/lib/thing-revived
    mkdir -p "$UPPER_BASE/upper" "$UPPER_BASE/work"
    chmod 0700 "$UPPER_BASE" "$UPPER_BASE/upper" "$UPPER_BASE/work"
    mkdir -p "$RUN_DIR"
    chmod 0700 "$RUN_DIR"
    mkdir -p "$LOWER"
    : >"$SAVED_ID"

    # Every step after the first bind is undone on failure, so /etc is never left half set up.
    mount --bind /etc "$LOWER"
    if ! mount -o remount,bind,ro "$LOWER"; then
        umount "$LOWER" || true
        return 1
    fi
    # /etc/machine-id is a tmpfs file mount from superbird-preinit. Keep a handle on it, because the
    # overlay hides it and the lower file is an empty placeholder.
    if ! mount --bind /etc/machine-id "$SAVED_ID"; then
        umount "$LOWER" || true
        return 1
    fi
    if ! mount -t overlay overlay \
        -o "lowerdir=$LOWER,upperdir=$UPPER_BASE/upper,workdir=$UPPER_BASE/work" /etc; then
        umount "$SAVED_ID" || true
        umount "$LOWER" || true
        return 1
    fi
    if ! mount --bind "$SAVED_ID" /etc/machine-id; then
        umount /etc || true
        umount "$SAVED_ID" || true
        umount "$LOWER" || true
        return 1
    fi
    log "overlay mounted on /etc"
}

stop() {
    # Reverse order of start. Best effort: a busy mount must not block shutdown.
    if etc_is_overlay; then
        umount /etc/machine-id || true
        umount /etc || true
    fi
    umount "$SAVED_ID" 2>/dev/null || true
    umount "$LOWER" 2>/dev/null || true
}

case "${1:-}" in
    start) start ;;
    stop) stop ;;
    *)
        log "usage: $0 start|stop"
        exit 2
        ;;
esac
