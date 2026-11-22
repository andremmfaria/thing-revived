SUMMARY = "Automatic NTP time sync with systemd-timesyncd"
DESCRIPTION = "The device has no RTC. This enables systemd-timesyncd (built into the \
systemd recipe via PACKAGECONFIG timesyncd) and installs a timesyncd.conf drop-in with \
pool and fallback servers. timesyncd follows systemd-networkd, which handles both Ethernet \
(80-wired.network) and Wi-Fi (80-thing-revived-wifi.network), so the clock syncs as soon as \
any network is up. It keeps its last sync time in /var/lib/systemd/timesync/clock on the \
data partition and uses it as a floor at the next boot. No credentials or user names."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

SRC_URI = "file://10-thing-revived-ntp.conf"
S = "${UNPACKDIR}"

inherit allarch

RDEPENDS:${PN} = "systemd"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${sysconfdir}/systemd/timesyncd.conf.d
    install -m 0644 ${S}/10-thing-revived-ntp.conf ${D}${sysconfdir}/systemd/timesyncd.conf.d/10-thing-revived-ntp.conf

    # The image's systemd preset is "disable *", so enable the unit explicitly (same
    # symlink `systemctl enable` creates from the unit's WantedBy=sysinit.target).
    install -d ${D}${sysconfdir}/systemd/system/sysinit.target.wants
    ln -sf ${systemd_system_unitdir}/systemd-timesyncd.service \
        ${D}${sysconfdir}/systemd/system/sysinit.target.wants/systemd-timesyncd.service
}

FILES:${PN} = " \
    ${sysconfdir}/systemd/timesyncd.conf.d/10-thing-revived-ntp.conf \
    ${sysconfdir}/systemd/system/sysinit.target.wants/systemd-timesyncd.service \
"
