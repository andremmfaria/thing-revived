#!/usr/bin/env bash
# Host check for the bandaid partition removal and the baked tool set
# (docs/research/bandaid-removal.md). Asserts the prod image bbappend carries the overrides, the
# project wks has no bandaid part line and keeps the other five parts, the fstab strip function
# works (extracted from the bbappend and run against fixtures, with a negative case), and the
# packagegroup bbappend drops the placeholder daemon. It does not run bitbake.
#
# Usage: timeout 60 build/test-bandaid-removal.sh
set -u

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
overlay="$here/overlay"
bbappend="$overlay/recipes-core/images/superbird-kiosk-prod-image.bbappend"
wks="$overlay/files/wic/superbird-mainline-nobandaid.wks.in"
pgroup="$overlay/recipes-core/packagegroups/packagegroup-superbird-kiosk-core.bbappend"
script="$here/build-image.sh"

status=0
pass() { echo "PASS [bandaid-removal] $1"; }
fail() { echo "FAIL [bandaid-removal] $1"; status=1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/bandaid-removal.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# Active (non-comment) lines of the bbappend, with backslash continuations joined, so a mention
# in a comment never satisfies an assertion.
active="$work/active.txt"
sed -e '/^[[:space:]]*#/d' "$bbappend" | sed -e ':a' -e '/\\$/N; s/\\\n[[:space:]]*/ /; ta' > "$active"

has_line() { grep -Eq -- "$1" "$active"; }

# --- bbappend variable overrides ---------------------------------------------------------
if has_line '^WKS_FILE = "superbird-mainline-nobandaid\.wks\.in"$'; then pass "WKS_FILE points at the project wks"; else fail "WKS_FILE is not superbird-mainline-nobandaid.wks.in"; fi
if has_line '^MAINLINE_FLASHTHING_WITH_BANDAID = "0"$'; then pass "MAINLINE_FLASHTHING_WITH_BANDAID = 0"; else fail "MAINLINE_FLASHTHING_WITH_BANDAID is not 0"; fi
if has_line '^EXTRA_IMAGEDEPENDS:remove = "superbird-kiosk-bandaid"$'; then pass "EXTRA_IMAGEDEPENDS:remove drops superbird-kiosk-bandaid"; else fail "EXTRA_IMAGEDEPENDS:remove = superbird-kiosk-bandaid missing"; fi
if has_line '^SUPERBIRD_BANDAID_PART_SIZE'; then fail "SUPERBIRD_BANDAID_PART_SIZE is assigned in the bbappend (the kas file owns it)"; else pass "SUPERBIRD_BANDAID_PART_SIZE left to the kas file"; fi
if has_line '^BAD_RECOMMENDATIONS \+= "vim-help vim-tutor"$'; then pass "BAD_RECOMMENDATIONS drops vim-help and vim-tutor"; else fail "BAD_RECOMMENDATIONS += vim-help vim-tutor missing"; fi
if has_line '^ROOTFS_POSTPROCESS_COMMAND \+= "thing_revived_strip_bandaid_fstab "$'; then pass "fstab strip is a ROOTFS_POSTPROCESS_COMMAND"; else fail "thing_revived_strip_bandaid_fstab not in ROOTFS_POSTPROCESS_COMMAND"; fi
if has_line '^IMAGE_PREPROCESS_COMMAND \+= "thing_revived_assert_data_guard "$'; then pass "data guard assertion is an IMAGE_PREPROCESS_COMMAND"; else fail "thing_revived_assert_data_guard not in IMAGE_PREPROCESS_COMMAND"; fi

# --- IMAGE_INSTALL additions -------------------------------------------------------------
tools="curl vim vim-common vim-vimrc vim-syntax nano htop tmux procps ncurses-tools file jq less ncdu rsync bash-completion iproute2 iproute2-ip iputils-ping ethtool usbutils strace lsof tcpdump python3-core python3-pip python3-setuptools python3-ensurepip evtest i2c-tools mmc-utils libgpiod-tools util-linux-lsblk util-linux-findmnt systemd-analyze ldd thing-revived-data-guard"
installed="$(grep -E '^IMAGE_INSTALL:append = ' "$active" | sed -E 's/^[^"]*"([^"]*)".*$/\1/' | tr -s '[:space:]' '\n')"
for pkg in $tools; do
    if printf '%s\n' "$installed" | grep -qx -- "$pkg"; then pass "IMAGE_INSTALL has $pkg"; else fail "IMAGE_INSTALL lacks $pkg"; fi
done
# Names that do not exist as packages upstream (checked against pkgdata and the recipes).
for bad in glibc-ldd gpiod-tools libgpiod-utils; do
    if printf '%s\n' "$installed" | grep -qx -- "$bad"; then fail "IMAGE_INSTALL names the non-existent package $bad"; else pass "IMAGE_INSTALL does not name $bad"; fi
done

# --- wks ---------------------------------------------------------------------------------
if [ -f "$wks" ]; then pass "wks exists at build/overlay/files/wic/"; else fail "missing $wks"; fi
wks_active="$(sed -e '/^[[:space:]]*#/d' "$wks" 2>/dev/null)"
if printf '%s\n' "$wks_active" | grep -qi bandaid; then fail "wks still has a bandaid line"; else pass "wks has no bandaid line"; fi
names="$(printf '%s\n' "$wks_active" | sed -nE 's/^part[[:space:]].*--part-name[[:space:]]+([A-Za-z_]+).*$/\1/p' | tr '\n' ' ')"
if [ "$names" = "env boot_a root_a boot_b root_b " ]; then pass "wks keeps exactly env boot_a root_a boot_b root_b"; else fail "wks parts are '$names'"; fi
if printf '%s\n' "$wks_active" | grep -Eq '^bootloader --ptable gpt$'; then pass "wks keeps bootloader --ptable gpt"; else fail "wks lost its bootloader gpt line"; fi
if printf '%s\n' "$wks_active" | grep -Fq -- '--fixed-size ${SUPERBIRD_ROOT_PART_SIZE}M'; then pass "wks keeps the SUPERBIRD_ROOT_PART_SIZE variable"; else fail "wks lost the root size variable"; fi
# Same name as upstream would lose to meta-superbird in BBPATH order.
if [ "$(basename "$wks")" != "superbird-mainline.wks.in" ]; then pass "wks name differs from upstream superbird-mainline.wks.in"; else fail "wks shadows the upstream name"; fi

# The wks reaches the example layer's files/wic through the existing overlay copy.
if grep -qE '^[[:space:]]*cp -a "\$\{REPO_ROOT\}/build/overlay/\."' "$script"; then pass "build-image.sh copies build/overlay/. into the example layer"; else fail "build-image.sh no longer copies build/overlay/."; fi

# --- packagegroup bbappend ---------------------------------------------------------------
pg_line="$(grep -E '^[[:space:]]*RDEPENDS:\$\{PN\}:remove[[:space:]]*=' "$pgroup" 2>/dev/null || true)"
if [ "$pg_line" = 'RDEPENDS:${PN}:remove = "superbird-kiosk-daemon"' ]; then pass "packagegroup bbappend removes superbird-kiosk-daemon"; else fail "packagegroup bbappend line is '$pg_line'"; fi
# The recipe is unversioned, so the bbappend must be too ("_%" would not match).
if [ "$(basename "$pgroup")" = "packagegroup-superbird-kiosk-core.bbappend" ]; then pass "packagegroup bbappend name matches the unversioned recipe"; else fail "wrong packagegroup bbappend name"; fi

# --- fstab strip function, extracted from the bbappend and run against fixtures ------------
fn="$work/strip.sh"
awk '/^thing_revived_strip_bandaid_fstab \(\) \{/{f=1} f{print} f&&/^}/{exit}' "$bbappend" > "$fn"
if [ -s "$fn" ]; then pass "extracted thing_revived_strip_bandaid_fstab"; else fail "could not extract thing_revived_strip_bandaid_fstab"; fi

run_strip() { # rootfs dir; prints bbfatal text on failure
    ( IMAGE_ROOTFS="$1"; sysconfdir=/etc
      bbfatal() { echo "bbfatal: $*"; exit 1; }
      # shellcheck disable=SC1090
      . "$fn"
      thing_revived_strip_bandaid_fstab )
}

good="$work/good"; mkdir -p "$good/etc"
cat > "$good/etc/fstab" <<'EOF'
PARTLABEL=data     /var            ext4  rw,noatime,nofail,x-systemd.makefs       0 0
PARTLABEL=bandaid  /var/lib/bandaid ext4 rw,noatime,sync,nofail                   0 0
PARTLABEL=env      /mnt/uboot-env  vfat  rw,noatime,sync,nofail                   0 2

proc            /proc       proc    defaults                    0 0
tmpfs           /tmp        tmpfs   defaults                    0 0
EOF
if out="$(run_strip "$good")" && ! grep -q bandaid "$good/etc/fstab" \
    && grep -q '^PARTLABEL=data ' "$good/etc/fstab" && grep -q '^PARTLABEL=env ' "$good/etc/fstab"; then
    pass "strip removes the PARTLABEL=bandaid line and keeps data and env"
else
    fail "strip did not clean the fixture fstab: $out"
fi
if run_strip "$good" >/dev/null; then pass "strip is idempotent"; else fail "strip failed on an already clean fstab"; fi

# Negative: a bandaid reference the sed does not match must make the function fail.
neg="$work/neg"; mkdir -p "$neg/etc"
cat > "$neg/etc/fstab" <<'EOF'
PARTLABEL=data     /var            ext4  rw,noatime,nofail,x-systemd.makefs       0 0
LABEL=bandaid      /var/lib/bandaid ext4 rw,noatime,sync,nofail                   0 0
EOF
if out="$(run_strip "$neg")"; then fail "strip accepted a leftover LABEL=bandaid line"; else
    case "$out" in *"still mentions bandaid"*) pass "negative: leftover LABEL=bandaid line is a bbfatal";; *) fail "negative failed for the wrong reason: $out";; esac
fi
# Negative: a fstab without the data line must fail too.
nodata="$work/nodata"; mkdir -p "$nodata/etc"
printf 'PARTLABEL=bandaid  /var/lib/bandaid ext4 rw 0 0\nproc /proc proc defaults 0 0\n' > "$nodata/etc/fstab"
if out="$(run_strip "$nodata")"; then fail "strip accepted a fstab without the data line"; else
    case "$out" in *"lost its PARTLABEL=data"*) pass "negative: missing data line is a bbfatal";; *) fail "negative (no data) failed for the wrong reason: $out";; esac
fi
# Negative: no fstab at all.
empty="$work/empty"; mkdir -p "$empty/etc"
if run_strip "$empty" >/dev/null; then fail "strip accepted a missing fstab"; else pass "negative: missing fstab is a bbfatal"; fi

# --- no em or en dashes in the files this task owns --------------------------------------
dash_files=("$bbappend" "$wks" "$pgroup" "$here/test-bandaid-removal.sh" "$here/test-data-guard.sh" \
    "$overlay/recipes-extended/thing-revived-data-guard/thing-revived-data-guard.bb" \
    "$overlay/recipes-extended/thing-revived-data-guard/files/thing-revived-data-guard.sh" \
    "$overlay/recipes-extended/thing-revived-data-guard/files/thing-revived-data-guard.service")
if grep -lP '[\x{2013}\x{2014}]' "${dash_files[@]}" 2>/dev/null; then fail "em or en dash found in an owned file"; else pass "no em or en dashes in owned files"; fi

exit "$status"
