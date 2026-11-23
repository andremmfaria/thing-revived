SUMMARY = "Bind the USB gadget (CDC-NCM dev link) when adbd is disabled"
DESCRIPTION = "Upstream superbird-usb-gadget composes the gadget (CDC-NCM plus an FFS adb \
function) and defers the UDC bind to adbd. This image leaves adbd off unless built with \
THING_REVIVED_ENABLE_ADB=1, so nothing would bind the gadget and the USB dev link (NCM, SSH \
to the kiosk user) would not exist. This installs thing-revived-usb-bind.service, which \
drops the unused ffs.adb link and binds the UDC, only when /etc/usb-debugging-enabled is \
absent, so it never overlaps with the adbd path. The unit is enabled explicitly because the \
image's systemd preset is \"disable *\"."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

SRC_URI = " \
    file://thing-revived-usb-bind.service \
    file://thing-revived-usb-bind \
"
S = "${UNPACKDIR}"

inherit allarch

RDEPENDS:${PN} = "systemd"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${libexecdir}/thing-revived
    install -m 0755 ${S}/thing-revived-usb-bind ${D}${libexecdir}/thing-revived/thing-revived-usb-bind

    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${S}/thing-revived-usb-bind.service ${D}${systemd_system_unitdir}/thing-revived-usb-bind.service

    # Same symlink `systemctl enable` creates from WantedBy=multi-user.target.
    install -d ${D}${sysconfdir}/systemd/system/multi-user.target.wants
    ln -sf ${systemd_system_unitdir}/thing-revived-usb-bind.service \
        ${D}${sysconfdir}/systemd/system/multi-user.target.wants/thing-revived-usb-bind.service
}

FILES:${PN} = " \
    ${libexecdir}/thing-revived/thing-revived-usb-bind \
    ${systemd_system_unitdir}/thing-revived-usb-bind.service \
    ${sysconfdir}/systemd/system/multi-user.target.wants/thing-revived-usb-bind.service \
"
