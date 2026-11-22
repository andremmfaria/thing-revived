SUMMARY = "Runtime Wi-Fi configuration: wpa_supplicant unit, networkd file, tmpfiles"
DESCRIPTION = "Installs the per-interface thing-revived-wpa@.service (wpa_supplicant \
with its config on the data partition at /var/lib/thing-revived/wpa_supplicant.conf), \
a udev rule that starts it when a wireless interface appears, a systemd-networkd \
.network file giving wireless interfaces DHCP, and a tmpfiles.d entry creating the \
root-only state directory and the initial config. No credentials are shipped.\
See docs/wifi.md upstream."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

SRC_URI = " \
    file://thing-revived-wpa@.service \
    file://90-thing-revived-wifi-wpa.rules \
    file://80-thing-revived-wifi.network \
    file://thing-revived-wifi.conf \
"
S = "${UNPACKDIR}"

inherit allarch

RDEPENDS:${PN} = "wpa-supplicant udev"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${systemd_unitdir}/system
    install -m 0644 ${S}/thing-revived-wpa@.service ${D}${systemd_unitdir}/system/thing-revived-wpa@.service

    install -d ${D}${nonarch_base_libdir}/udev/rules.d
    install -m 0644 ${S}/90-thing-revived-wifi-wpa.rules \
        ${D}${nonarch_base_libdir}/udev/rules.d/90-thing-revived-wifi-wpa.rules

    install -d ${D}${systemd_unitdir}/network
    install -m 0644 ${S}/80-thing-revived-wifi.network ${D}${systemd_unitdir}/network/80-thing-revived-wifi.network

    install -d ${D}${nonarch_libdir}/tmpfiles.d
    install -m 0644 ${S}/thing-revived-wifi.conf ${D}${nonarch_libdir}/tmpfiles.d/thing-revived-wifi.conf
}

FILES:${PN} = " \
    ${systemd_unitdir}/system/thing-revived-wpa@.service \
    ${nonarch_base_libdir}/udev/rules.d/90-thing-revived-wifi-wpa.rules \
    ${systemd_unitdir}/network/80-thing-revived-wifi.network \
    ${nonarch_libdir}/tmpfiles.d/thing-revived-wifi.conf \
"
