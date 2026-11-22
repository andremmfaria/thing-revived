SUMMARY = "udev rule that ejects Realtek driver-CD mode USB Wi-Fi dongles"
DESCRIPTION = "Installs a udev rule that sends a SCSI eject to Realtek 0bda:1a2b \
(driver CD mode, e.g. D-Link DWA-171 rev C1) so the dongle re-enumerates as its \
Wi-Fi function. Uses usb_modeswitch (without its tcl dispatcher) for the eject."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

SRC_URI = "file://80-thing-revived-wifi-modeswitch.rules"
S = "${UNPACKDIR}"

RDEPENDS:${PN} = "udev usb-modeswitch"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${nonarch_base_libdir}/udev/rules.d
    install -m 0644 ${S}/80-thing-revived-wifi-modeswitch.rules \
        ${D}${nonarch_base_libdir}/udev/rules.d/80-thing-revived-wifi-modeswitch.rules
}

FILES:${PN} = "${nonarch_base_libdir}/udev/rules.d/80-thing-revived-wifi-modeswitch.rules"
