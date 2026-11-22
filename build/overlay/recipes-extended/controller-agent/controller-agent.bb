SUMMARY = "thing-revived controller agent (prebuilt Rust binary)"
DESCRIPTION = "Exclusively grabs the gpio-keys-polled (buttons/ESC) and rotary \
(knob) evdev devices, replays KEY_M/KEY_ENTER through a uinput passthrough \
device so they still reach the browser normally, and broadcasts button/knob \
WebSocket events to the terminal frontend on port 8765. Cross-compiled \
upstream for aarch64-unknown-linux-gnu with +crt-static (static-pie); this \
recipe only installs the prebuilt artifact, it does not compile anything."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

SRC_URI = " \
    file://controller-agent \
    file://controller.toml \
    file://controller-agent.service \
"
S = "${UNPACKDIR}"

inherit systemd

INSANE_SKIP:${PN} += "already-stripped"
do_compile[noexec] = "1"
do_configure[noexec] = "1"

SYSTEMD_SERVICE:${PN} = "controller-agent.service"
SYSTEMD_AUTO_ENABLE:${PN} = "enable"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${S}/controller-agent ${D}${bindir}/controller-agent

    # Read-only on the device (squashfs rootfs); see config/README.md.
    install -d ${D}${sysconfdir}/thing-revived
    install -m 0644 ${S}/controller.toml ${D}${sysconfdir}/thing-revived/controller.toml

    install -d ${D}${systemd_unitdir}/system
    install -m 0644 ${S}/controller-agent.service ${D}${systemd_unitdir}/system/controller-agent.service
}

FILES:${PN} += " \
    ${bindir}/controller-agent \
    ${sysconfdir}/thing-revived/controller.toml \
    ${systemd_unitdir}/system/controller-agent.service \
"

# Keep local edits to the config across package updates.
CONFFILES:${PN} += "${sysconfdir}/thing-revived/controller.toml"
