SUMMARY = "Bind the USB gadget (CDC-NCM dev link) when adbd is off"
DESCRIPTION = "Upstream superbird-usb-gadget composes the gadget (CDC-NCM plus an FFS adb \
function) and defers the UDC bind to adbd. Without adbd running, nothing would bind the gadget \
and the USB dev link (NCM, SSH to the kiosk user) would not exist. This installs \
thing-revived-usb-bind.service, which drops the unused ffs.adb link and binds the UDC NCM-only. \
It is mutually exclusive with android-tools-adbd.service via Conflicts= on both sides (see the \
unit file and the android-tools-adbd.service.d drop-in in \
recipes-core/superbird-usb-gadget), so starting either stops the other. The unit ships DISABLED \
on every build (no wants symlink): thing-revived-device-apply.service starts adbd or this unit at \
boot from the persisted ADB setting, the controller starts them live (Device settings, USB mode), \
and the failsafe unit starts this one when the late apply failed in device mode. The [Install] \
section stays because it documents the target, but nothing enables it; the image's systemd \
preset is \"disable *\"."
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
}

FILES:${PN} = " \
    ${libexecdir}/thing-revived/thing-revived-usb-bind \
    ${systemd_system_unitdir}/thing-revived-usb-bind.service \
"
