#!/usr/bin/env bash
# Host check for thing-revived-data-guard (docs/research/bandaid-removal.md, risk "Old bandaid
# superblock at the new data start"). Renders the shipped script the way the recipe's do_install
# does, with every path pointed at a scratch dir of fakes, and checks: a stale bandaid label is
# wiped, an ext4 data label is untouched, unreadable and absent devices and a mounted device exit
# 0 without a wipe, a failing wipefs still exits 0. Also checks the unit text and, when
# systemd-analyze is available, that the unit forms no ordering cycle against stubs shaped like
# the real boot ordering (verify exits 0 on a cycle, so its output is grepped). No bitbake.
#
# Usage: timeout 60 build/test-data-guard.sh
set -u

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dir="$here/overlay/recipes-extended/thing-revived-data-guard"
src="$dir/files/thing-revived-data-guard.sh"
unit="$dir/files/thing-revived-data-guard.service"
recipe="$dir/thing-revived-data-guard.bb"

status=0
pass() { echo "PASS [data-guard] $1"; }
fail() { echo "FAIL [data-guard] $1"; status=1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/data-guard.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# --- recipe wiring -----------------------------------------------------------------------
for tok in DATA_DEV BLKID WIPEFS UDEVADM MOUNTS; do
    if grep -q "@@${tok}@@" "$src"; then pass "script has the @@${tok}@@ placeholder"; else fail "script lacks @@${tok}@@"; fi
    if grep -Fq -- "s|@@${tok}@@|" "$recipe"; then pass "recipe substitutes @@${tok}@@"; else fail "recipe does not substitute @@${tok}@@"; fi
done
if grep -Eq '^inherit systemd$' "$recipe" && grep -Eq '^SYSTEMD_SERVICE:\$\{PN\} = "thing-revived-data-guard\.service"$' "$recipe" \
    && grep -Eq '^SYSTEMD_AUTO_ENABLE = "enable"$' "$recipe"; then pass "recipe enables the unit through the systemd class"; else fail "recipe does not enable the unit via the systemd class"; fi
if grep -Eq '^RDEPENDS:\$\{PN\} = ".*util-linux-blkid.*util-linux-wipefs' "$recipe"; then pass "recipe depends on util-linux-blkid and util-linux-wipefs"; else fail "recipe lacks the blkid/wipefs RDEPENDS"; fi
if grep -Eq 'getenv|\$\{?(DEV|BLKID|WIPEFS|MOUNTS|UDEVADM)[A-Z_]*:-' "$src" || grep -Eq '^[[:space:]]*(DEV|BLKID|WIPEFS|MOUNTS|UDEVADM)=.*\$\{?[A-Za-z_]+:-' "$src"; then fail "script has a runtime env-var hook"; else pass "script has no runtime env-var hooks"; fi
if [ -x "$src" ]; then pass "script is executable in the tree"; else fail "script is not executable"; fi

# --- render the script against fakes ------------------------------------------------------
# One scratch tree per case so fakes and logs never leak between cases.
render() { # case dir
    local c="$1"
    mkdir -p "$c/bin" "$c/dev"
    sed -e "s|@@DATA_DEV@@|$c/dev/data|g" \
        -e "s|@@BLKID@@|$c/bin/blkid|g" \
        -e "s|@@WIPEFS@@|$c/bin/wipefs|g" \
        -e "s|@@UDEVADM@@|$c/bin/udevadm|g" \
        -e "s|@@MOUNTS@@|$c/mounts|g" "$src" > "$c/guard.sh"
    chmod +x "$c/guard.sh"
    : > "$c/mounts"
    : > "$c/wipefs.log"
    : > "$c/udevadm.log"
    printf '#!/bin/sh\necho "$@" >> "%s/udevadm.log"\nexit 0\n' "$c" > "$c/bin/udevadm"
    printf '#!/bin/sh\necho "$@" >> "%s/wipefs.log"\nexit 0\n' "$c" > "$c/bin/wipefs"
    chmod +x "$c/bin/udevadm" "$c/bin/wipefs"
    : > "$c/real"                      # stand-in for the partition node
    ln -s "$c/real" "$c/dev/data"      # like /dev/disk/by-partlabel/data
}
set_blkid() { # case dir, exit code, output text
    printf '#!/bin/sh\ncat <<'"'"'EOF'"'"'\n%s\nEOF\nexit %s\n' "$3" "$2" > "$1/bin/blkid"
    chmod +x "$1/bin/blkid"
}
run_case() { # case dir; sets rc and out
    out="$(timeout 20 "$1/guard.sh" 2>&1)"; rc=$?
}

# 1. stale bandaid label: wiped, exit 0, journal line.
c="$work/stale"; render "$c"
set_blkid "$c" 0 "$c/dev/data: LABEL=\"bandaid\" UUID=\"11111111-2222-3333-4444-555555555555\" TYPE=\"ext4\" PARTLABEL=\"data\" PARTUUID=\"aaaa\""
run_case "$c"
if [ "$rc" -eq 0 ] && grep -qx -- "-a $c/dev/data" "$c/wipefs.log"; then pass "stale bandaid label is wiped with wipefs -a"; else fail "stale bandaid not wiped (rc=$rc log='$(cat "$c/wipefs.log")')"; fi
case "$out" in *"stale bandaid"*) pass "wipe is logged";; *) fail "wipe not logged: $out";; esac

# 2. ext4 data label: untouched.
c="$work/data"; render "$c"
set_blkid "$c" 0 "$c/dev/data: LABEL=\"data\" UUID=\"1\" TYPE=\"ext4\" PARTLABEL=\"data\""
run_case "$c"
if [ "$rc" -eq 0 ] && [ ! -s "$c/wipefs.log" ]; then pass "ext4 data label is untouched"; else fail "data label wiped or failed (rc=$rc)"; fi

# 3. No label at all, only PARTLABEL=bandaid: PARTLABEL must not match the LABEL test.
c="$work/partlabel"; render "$c"
set_blkid "$c" 0 "$c/dev/data: UUID=\"1\" TYPE=\"ext4\" PARTLABEL=\"bandaid\""
run_case "$c"
if [ "$rc" -eq 0 ] && [ ! -s "$c/wipefs.log" ]; then pass "PARTLABEL=bandaid alone is not mistaken for the filesystem label"; else fail "PARTLABEL matched as a label (rc=$rc)"; fi

# 4. A label that merely contains bandaid: not an exact match.
c="$work/lookalike"; render "$c"
set_blkid "$c" 0 "$c/dev/data: LABEL=\"bandaid2\" TYPE=\"ext4\""
run_case "$c"
if [ "$rc" -eq 0 ] && [ ! -s "$c/wipefs.log" ]; then pass "a lookalike label (bandaid2) is untouched"; else fail "lookalike label wiped (rc=$rc)"; fi

# 5. Unreadable device: blkid finds nothing (exit 2, no output): exit 0, no wipe.
c="$work/unreadable"; render "$c"
set_blkid "$c" 2 ""
run_case "$c"
if [ "$rc" -eq 0 ] && [ ! -s "$c/wipefs.log" ]; then pass "unreadable device exits 0 without a wipe"; else fail "unreadable device mishandled (rc=$rc)"; fi

# 6. blkid itself missing or failing hard: exit 0.
c="$work/noblkid"; render "$c"
rm -f "$c/bin/blkid"
run_case "$c"
if [ "$rc" -eq 0 ] && [ ! -s "$c/wipefs.log" ]; then pass "missing blkid exits 0"; else fail "missing blkid mishandled (rc=$rc)"; fi

# 7. Device absent: settle is tried, then exit 0.
c="$work/absent"; render "$c"; rm -f "$c/dev/data"
set_blkid "$c" 0 "unused"
run_case "$c"
if [ "$rc" -eq 0 ] && [ ! -s "$c/wipefs.log" ] && grep -q settle "$c/udevadm.log"; then pass "absent device settles udev and exits 0"; else fail "absent device mishandled (rc=$rc)"; fi

# 8. Mounted device: never wiped even with the stale label.
c="$work/mounted"; render "$c"
set_blkid "$c" 0 "$c/dev/data: LABEL=\"bandaid\" TYPE=\"ext4\""
echo "$c/real /var ext4 rw 0 0" > "$c/mounts"
run_case "$c"
if [ "$rc" -eq 0 ] && [ ! -s "$c/wipefs.log" ]; then pass "a mounted device is left alone"; else fail "mounted device wiped (rc=$rc)"; fi

# 9. wipefs fails: still exit 0.
c="$work/wipefail"; render "$c"
set_blkid "$c" 0 "$c/dev/data: LABEL=\"bandaid\" TYPE=\"ext4\""
printf '#!/bin/sh\necho "$@" >> "%s/wipefs.log"\nexit 1\n' "$c" > "$c/bin/wipefs"
run_case "$c"
if [ "$rc" -eq 0 ] && [ -s "$c/wipefs.log" ]; then pass "a failing wipefs still exits 0"; else fail "failing wipefs mishandled (rc=$rc)"; fi

# 10. BusyBox style blkid output (no PARTLABEL, label first): same result.
c="$work/busybox"; render "$c"
set_blkid "$c" 0 "$c/dev/data: LABEL=\"bandaid\" UUID=\"abcd\" TYPE=\"ext4\""
run_case "$c"
if [ "$rc" -eq 0 ] && grep -qx -- "-a $c/dev/data" "$c/wipefs.log"; then pass "BusyBox-shaped blkid output is handled"; else fail "BusyBox-shaped output mishandled (rc=$rc)"; fi

# --- unit text -----------------------------------------------------------------------------
uget() { sed -nE "s/^$1=//p" "$unit"; }
if grep -qx 'DefaultDependencies=no' "$unit"; then pass "unit has DefaultDependencies=no"; else fail "unit lacks DefaultDependencies=no"; fi
before="$(uget Before | tr '\n' ' ')"
for t in var.mount local-fs.target shutdown.target; do
    case " $before" in *" $t "*|*" $t") pass "unit is Before=$t";; *) fail "unit is not Before=$t";; esac
done
if uget Conflicts | grep -qx 'shutdown.target'; then pass "unit has Conflicts=shutdown.target"; else fail "unit lacks Conflicts=shutdown.target"; fi
if grep -Eq '^(Requires|Wants|Requisite|BindsTo|PartOf|OnFailure)=' "$unit"; then fail "unit pulls in or depends on another unit"; else pass "unit has no Requires/Wants/BindsTo/OnFailure"; fi
if grep -Eq '^Type=oneshot$' "$unit" && grep -Eq '^ExecStart=-/usr/libexec/thing-revived/thing-revived-data-guard$' "$unit"; then pass "unit is a oneshot whose failure is ignored"; else fail "unit ExecStart or Type wrong"; fi
if grep -Eq '^WantedBy=sysinit\.target$' "$unit"; then pass "unit installs into sysinit.target"; else fail "unit lacks WantedBy=sysinit.target"; fi
if grep -Fq 'ExecStart=-/usr/libexec/thing-revived/thing-revived-data-guard' "$unit" && grep -Fq '${libexecdir}/thing-revived/thing-revived-data-guard' "$recipe"; then pass "unit ExecStart matches the recipe install path"; else fail "unit ExecStart does not match the recipe install path"; fi

# --- ordering cycle check against stubs ----------------------------------------------------
if command -v systemd-analyze >/dev/null 2>&1; then
    u="$work/units"; mkdir -p "$u"
    cp "$unit" "$u/"
    # Ordering shaped like the real image: provision and fsck are copied from the upstream
    # meta-superbird units (Before=local-fs-pre.target); makefs and var.mount follow the
    # fstab generator's shape for PARTLABEL=data with x-systemd.makefs.
    printf '[Unit]\nDescription=sysinit\nWants=local-fs.target\nAfter=local-fs.target swap.target\n' > "$u/sysinit.target"
    printf '[Unit]\nDescription=basic\nRequires=sysinit.target\nAfter=sysinit.target\n' > "$u/basic.target"
    printf '[Unit]\nDescription=local-fs\nAfter=local-fs-pre.target\n' > "$u/local-fs.target"
    for t in local-fs-pre swap shutdown sockets; do printf '[Unit]\nDescription=%s\n' "$t" > "$u/$t.target"; done
    cat > "$u/superbird-provision.service" <<'EOF'
[Unit]
Description=provision
DefaultDependencies=no
After=systemd-udevd.service
Before=local-fs-pre.target shutdown.target
Conflicts=shutdown.target
[Service]
Type=oneshot
ExecStart=/bin/true
[Install]
WantedBy=sysinit.target
EOF
    cat > "$u/superbird-fsck@.service" <<'EOF'
[Unit]
Description=fsck %i
DefaultDependencies=no
After=systemd-udevd.service superbird-provision.service
Before=local-fs-pre.target shutdown.target
Conflicts=shutdown.target
[Service]
Type=oneshot
ExecStart=/bin/true
EOF
    cat > "$u/systemd-makefs@.service" <<'EOF'
[Unit]
Description=makefs %f
DefaultDependencies=no
After=%i.device
Before=shutdown.target
Conflicts=shutdown.target
[Service]
Type=oneshot
ExecStart=/bin/true
EOF
    cat > "$u/var.mount" <<'EOF'
[Unit]
Description=var
After=local-fs-pre.target systemd-makefs@dev-disk-by\x2dpartlabel-data.service superbird-fsck@data.service
Before=local-fs.target
[Mount]
What=/dev/disk/by-partlabel/data
Where=/var
EOF
    for n in systemd-udevd systemd-udev-trigger; do
        printf '[Unit]\nDescription=%s\nDefaultDependencies=no\nBefore=sysinit.target\n[Service]\nExecStart=/bin/true\n' "$n" > "$u/$n.service"
    done
    vout="$(SYSTEMD_UNIT_PATH="$u" timeout 30 systemd-analyze verify --man=no "$u/thing-revived-data-guard.service" "$u/var.mount" "$u/sysinit.target" "$u/superbird-provision.service" "$u/superbird-fsck@data.service" 2>&1)"
    if printf '%s\n' "$vout" | grep -qi 'ordering cycle'; then fail "ordering cycle in the shipped unit: $(printf '%s\n' "$vout" | grep -i 'ordering cycle' | head -n 2)"; else pass "no ordering cycle against the stubbed boot ordering"; fi

    # Negative control: without DefaultDependencies=no the stubs must show a cycle, proving the
    # check can see one.
    sed '/^DefaultDependencies=no$/d' "$unit" > "$u/neg-guard.service"
    nout="$(SYSTEMD_UNIT_PATH="$u" timeout 30 systemd-analyze verify --man=no "$u/neg-guard.service" "$u/var.mount" "$u/sysinit.target" 2>&1)"
    if printf '%s\n' "$nout" | grep -qi 'ordering cycle'; then pass "negative control: the cycle check flags a unit with default dependencies"; else fail "negative control: cycle check did not flag the default-dependency variant: $nout"; fi
else
    echo "SKIP [data-guard] systemd-analyze not available; ordering cycle check not run"
fi

exit "$status"
