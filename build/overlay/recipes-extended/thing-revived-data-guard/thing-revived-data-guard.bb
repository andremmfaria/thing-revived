SUMMARY = "Wipe a stale bandaid filesystem left at the start of the data partition"
DESCRIPTION = "A device flashed with the old layout has an ext4 labelled bandaid at the byte \
offset where the bandaid-less layout carves the data partition. x-systemd.makefs formats only a \
device without a filesystem signature, so /var would mount the stale 192 MiB filesystem. \
thing-revived-data-guard.service (oneshot, ordered before the fsck, makefs and var.mount of data) \
wipes the signature when blkid reports the label bandaid on PARTLABEL=data, so makefs formats it \
fresh. It never blocks boot and exits 0 on every path (docs/research/bandaid-removal.md, risk \
table)."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

SRC_URI = " \
    file://thing-revived-data-guard.sh \
    file://thing-revived-data-guard.service \
"
S = "${UNPACKDIR}"

inherit systemd

# Enabled by the 98-<pkg>.preset that SYSTEMD_AUTO_ENABLE generates (systemd_handle_machine_id runs
# preset-all with --preset-mode=enable-only, so nothing is disabled behind it; the same pattern
# meta-superbird's superbird-fsck uses). No 00- preset needed.
SYSTEMD_SERVICE:${PN} = "thing-revived-data-guard.service"
SYSTEMD_AUTO_ENABLE = "enable"

# blkid and wipefs from util-linux (BusyBox blkid, if built, has no flags this script relies on;
# it only uses the plain `blkid DEVICE` form so either works). udevadm comes with systemd.
RDEPENDS:${PN} = "util-linux-blkid util-linux-wipefs systemd"

THING_REVIVED_DATA_GUARD_DEV ?= "/dev/disk/by-partlabel/data"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${libexecdir}/thing-revived
    install -m 0755 ${S}/thing-revived-data-guard.sh ${D}${libexecdir}/thing-revived/thing-revived-data-guard
    sed -i \
        -e "s|@@DATA_DEV@@|${THING_REVIVED_DATA_GUARD_DEV}|g" \
        -e "s|@@BLKIDS@@|blkid ${base_sbindir}/blkid ${sbindir}/blkid.util-linux|g" \
        -e "s|@@WIPEFS@@|${sbindir}/wipefs|g" \
        -e "s|@@UDEVADM@@|${bindir}/udevadm|g" \
        -e "s|@@MOUNTS@@|/proc/self/mounts|g" \
        ${D}${libexecdir}/thing-revived/thing-revived-data-guard
    if grep -q '@@' ${D}${libexecdir}/thing-revived/thing-revived-data-guard; then
        bbfatal "thing-revived-data-guard still has an unsubstituted @@ token"
    fi

    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${S}/thing-revived-data-guard.service ${D}${systemd_system_unitdir}/thing-revived-data-guard.service
}

do_install[vardeps] += "THING_REVIVED_DATA_GUARD_DEV"

FILES:${PN} = " \
    ${libexecdir}/thing-revived/thing-revived-data-guard \
    ${systemd_system_unitdir}/thing-revived-data-guard.service \
"
