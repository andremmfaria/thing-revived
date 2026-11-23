FILESEXTRAPATHS:prepend := "${THISDIR}/${PN}:"

# Default the USB port to device role at boot via a systemd drop-in (see the .conf for why a
# persisted boot-role file cannot be shipped in the image).
SRC_URI += "file://10-thing-revived-default-device.conf"

do_install:append() {
    install -d ${D}${systemd_system_unitdir}/superbird-usb-role.service.d
    install -m 0644 ${UNPACKDIR}/10-thing-revived-default-device.conf \
        ${D}${systemd_system_unitdir}/superbird-usb-role.service.d/10-thing-revived-default-device.conf
}

FILES:${PN} += "${systemd_system_unitdir}/superbird-usb-role.service.d/10-thing-revived-default-device.conf"
