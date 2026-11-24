#!/bin/sh
# Ordering-cycle check for the device-settings boot units (docs/research/device-settings-plan.md
# sections 4.4, 7.2 item 10 and P0.5). `systemd-analyze verify` exits 0 even when it prints an
# ordering cycle, so the TEXT output is what gets checked: the script fails on any line
# containing 'ordering cycle' (which covers 'Found ordering cycle') or 'Found dependency', and
# on any line reporting a unit that could not be found or loaded (a missing unit that is not
# stubbed means the check proved nothing).
#
# Usage:
#   build/tools/check_unit_graph.sh              good set, then the deliberately broken draft
#   build/tools/check_unit_graph.sh --rootfs DIR verify the units already installed in an
#                                                extracted image rootfs (real sshd.socket and
#                                                drop-ins), no stubs
#   build/tools/check_unit_graph.sh --bad-only   only the broken draft (exits nonzero)
#
# Exit 0 only when the good set is clean AND the broken draft is caught. In --rootfs mode, exit 0
# only when the real image unit set is clean. systemd-analyze is not installed on the device, so
# this runs on the build host only. Missing systemd-analyze is a failure (exit 2): a skipped
# check must not look like a pass.
#
# Environment: SSHD_UNIT_DIR overrides the directory holding the upstream openssh sshd.socket
# and sshd@.service text (default: the openssh recipe in the yocto build tree, if present, else
# the embedded copy below, which is the upstream text with the @VAR@ paths substituted).
set -u

here="$(cd "$(dirname "$0")" && pwd)"
overlay="$here/../overlay/recipes-extended"
apply="$overlay/thing-revived-device-apply/files"
accounts="$overlay/thing-revived-accounts/files"

mode=all
rootfs=
case "${1:-}" in
    --rootfs)
        mode=rootfs
        rootfs="${2:-}"
        [ -d "$rootfs" ] || { echo "FAIL [unit-graph] --rootfs needs a directory" >&2; exit 2; }
        ;;
    --bad-only) mode=bad ;;
    "") ;;
    *) echo "usage: $0 [--rootfs DIR | --bad-only]" >&2; exit 2 ;;
esac

if ! command -v systemd-analyze >/dev/null 2>&1; then
    echo "FAIL [unit-graph] systemd-analyze not installed; run on the build host" >&2
    exit 2
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/unit-graph.XXXXXX")" || exit 2
trap 'rm -rf "$work"' EXIT

# Lines in the verify TEXT that make the check fail.
bad_pattern='ordering cycle|Found dependency|not found|Failed to (load|create|prepare|open)|No such file'

# run_verify ROOT LABEL UNIT... : prints PASS or FAIL, returns 1 on a bad line.
run_verify() {
    rv_root="$1"
    rv_label="$2"
    shift 2
    rv_out="$(timeout 60 env -u SYSTEMD_UNIT_PATH systemd-analyze verify --root="$rv_root" --man=no "$@" 2>&1)"
    if printf '%s\n' "$rv_out" | grep -Eqi "$bad_pattern"; then
        [ "${rv_quiet:-0}" = 1 ] || echo "FAIL [unit-graph] $rv_label"
        printf '%s\n' "$rv_out" | sed 's/^/    /'
        return 1
    fi
    echo "PASS [unit-graph] $rv_label (no ordering cycle, no missing unit)"
    return 0
}

if [ "$mode" = rootfs ]; then
    run_verify "$rootfs" "real image unit set" \
        sysinit.target multi-user.target sockets.target sshd.socket \
        thing-revived-device-settings.service thing-revived-device-apply.service \
        thing-revived-device-failsafe.service
    exit $?
fi

# --- build the stub root -------------------------------------------------------------------
build_root() {  # build_root ROOT : lays down stubs, the real shipped files and the upstream sshd text
    br="$1"
    sys="$br/usr/lib/systemd/system"
    mkdir -p "$sys/sshd.socket.d" "$br/usr/bin" "$br/usr/libexec/thing-revived" "$br/bin" \
        "$br/etc/systemd/system/sysinit.target.wants" "$br/etc/systemd/system/multi-user.target.wants" \
        "$br/etc/systemd/system/sockets.target.wants"

    # Executables referenced by ExecStart= (verify checks they exist under --root).
    for f in usr/bin/controller-agent usr/libexec/thing-revived/thing-revived-etc-mount.sh \
        bin/mkdir bin/true usr/bin/mkdir usr/sbin/sshd; do
        mkdir -p "$br/$(dirname "$f")"
        printf '#!/bin/sh\n' >"$br/$f"
        chmod 0755 "$br/$f"
    done

    stub() {  # stub NAME [BODY] : minimal unit with Description plus BODY
        printf '[Unit]\nDescription=stub\n%s\n' "${2:-}" >"$sys/$1"
    }

    # Targets, close to the real systemd definitions (ordering edges only).
    stub shutdown.target ''
    stub local-fs.target ''
    stub swap.target ''
    stub var.mount '[Mount]
What=tmpfs
Where=/var
Type=tmpfs'
    stub nss-user-lookup.target ''
    stub sysinit.target 'Wants=local-fs.target swap.target
After=local-fs.target swap.target'
    stub sockets.target 'After=sysinit.target'
    stub basic.target 'Requires=sysinit.target
Wants=sockets.target
After=sysinit.target sockets.target'
    stub multi-user.target 'Requires=basic.target
After=basic.target'
    # Boot-time services the new units order against.
    stub systemd-tmpfiles-setup.service 'DefaultDependencies=no
After=local-fs.target
Before=sysinit.target
[Service]
Type=oneshot
ExecStart=/bin/true'
    stub superbird-usb-gadget.service '[Service]
Type=oneshot
ExecStart=/bin/true'
    stub superbird-usb-role.service 'After=superbird-usb-gadget.service
[Service]
Type=oneshot
ExecStart=/bin/true'
    stub sshdgenkeys.service '[Service]
Type=oneshot
ExecStart=/bin/true'

    # Real shipped units that already interact with sshd.socket (cycle coverage).
    cp "$accounts/thing-revived-etc.service" "$sys/thing-revived-etc.service"
    cp "$accounts/thing-revived-provision.service" "$sys/thing-revived-provision.service"
    cp "$accounts/thing-revived-sshd-socket.conf" "$sys/sshd.socket.d/10-thing-revived-limits.conf"
    stub controller-agent.service 'After=thing-revived-provision.service
[Service]
ExecStart=/usr/bin/controller-agent'
    stub thing-revived-terminal-agent.service '[Service]
ExecStart=/bin/true'
    stub getty.target ''

    # The upstream openssh sshd.socket and sshd@.service text (openembedded-core openssh recipe,
    # @BASE_BINDIR@ and @SBINDIR@ substituted). Prefer the real recipe files when present.
    up="${SSHD_UNIT_DIR:-/home/andremmfaria/workspace/yocto-superbird-build-2/sources/openembedded-core/meta/recipes-connectivity/openssh/openssh}"
    if [ -f "$up/sshd.socket" ] && [ -f "$up/sshd@.service" ]; then
        sed -e 's|@BASE_BINDIR@|/bin|g' -e 's|@SBINDIR@|/usr/sbin|g' "$up/sshd.socket" >"$sys/sshd.socket"
        sed -e 's|@BASE_BINDIR@|/bin|g' -e 's|@SBINDIR@|/usr/sbin|g' "$up/sshd@.service" >"$sys/sshd@.service"
    else
        cat >"$sys/sshd.socket" <<'UNIT'
[Unit]
Conflicts=sshd.service
Wants=sshdgenkeys.service
After=nss-user-lookup.target

[Socket]
ExecStartPre=/bin/mkdir -p /var/run/sshd
ListenStream=22
Accept=yes

[Install]
WantedBy=sockets.target
UNIT
        cat >"$sys/sshd@.service" <<'UNIT'
[Unit]
Description=OpenSSH Per-Connection Daemon
After=sshdgenkeys.service

[Service]
Environment="SSHD_OPTS="
EnvironmentFile=-/etc/default/ssh
ExecStart=-/usr/sbin/sshd -i $SSHD_OPTS
StandardInput=socket
KillMode=process
UNIT
    fi

    # Enablement symlinks, as the image would have them (preset is 'disable *').
    ln -sf "$sys/sshd.socket" "$br/etc/systemd/system/sockets.target.wants/sshd.socket"
    ln -sf "$sys/thing-revived-etc.service" "$br/etc/systemd/system/sysinit.target.wants/thing-revived-etc.service"
    ln -sf "$sys/thing-revived-provision.service" "$br/etc/systemd/system/sysinit.target.wants/thing-revived-provision.service"
}

units_all="sysinit.target sockets.target multi-user.target sshd.socket thing-revived-etc.service thing-revived-provision.service"

status=0

# --- good set: the real shipped units and drop-in ------------------------------------------
if [ "$mode" != bad ]; then
    good="$work/good"
    build_root "$good"
    gs="$good/usr/lib/systemd/system"
    cp "$apply/thing-revived-device-settings.service" "$gs/"
    cp "$apply/thing-revived-device-apply.service" "$gs/"
    cp "$apply/thing-revived-device-failsafe.service" "$gs/"
    cp "$apply/thing-revived-ssh-gate.conf" "$gs/sshd.socket.d/20-thing-revived-ssh-gate.conf"
    ln -sf "$gs/thing-revived-device-settings.service" "$good/etc/systemd/system/sysinit.target.wants/thing-revived-device-settings.service"
    ln -sf "$gs/thing-revived-device-apply.service" "$good/etc/systemd/system/multi-user.target.wants/thing-revived-device-apply.service"

    # shellcheck disable=SC2086
    run_verify "$good" "good set (units A, B, F, ssh gate, real sshd.socket)" \
        $units_all thing-revived-device-settings.service thing-revived-device-apply.service \
        thing-revived-device-failsafe.service || status=1

    # Sanity: the gate drop-in must really be merged into sshd.socket, else the run proves nothing.
    if ! grep -q 'ConditionPathExists=!/run/thing-revived/ssh-off' "$gs/sshd.socket.d/20-thing-revived-ssh-gate.conf"; then
        echo "FAIL [unit-graph] gate drop-in lacks the ssh-off condition"
        status=1
    fi
fi

# --- deliberately broken draft: early unit with Before=sshd.socket and default dependencies --
if [ "$mode" != rootfs ]; then
    bad="$work/bad"
    build_root "$bad"
    bs="$bad/usr/lib/systemd/system"
    cat >"$bs/thing-revived-device-settings.service" <<'UNIT'
[Unit]
Description=BROKEN DRAFT: early unit with default dependencies and Before=sshd.socket
After=var.mount systemd-tmpfiles-setup.service
Before=superbird-usb-role.service sshd.socket

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/bin/controller-agent apply-device-settings early

[Install]
WantedBy=sysinit.target
UNIT
    ln -sf "$bs/thing-revived-device-settings.service" "$bad/etc/systemd/system/sysinit.target.wants/thing-revived-device-settings.service"

    rv_quiet=1
    # shellcheck disable=SC2086
    if run_verify "$bad" "broken draft (must be caught)" \
        $units_all thing-revived-device-settings.service >"$work/bad.out" 2>&1; then
        echo "FAIL [unit-graph] broken draft was NOT caught: the checker would miss a real cycle"
        status=1
    else
        echo "PASS [unit-graph] broken draft caught"
        sed 's/^/    /' "$work/bad.out"
        if [ "$mode" = bad ]; then
            status=1
        fi
    fi
fi

exit "$status"
