SUMMARY = "thing-revived on-device terminal agent (prebuilt Rust binary)"
DESCRIPTION = "PTY-to-WebSocket relay serving the on-device terminal UI. \
Cross-compiled upstream for aarch64-unknown-linux-gnu with +crt-static \
(static-pie); this recipe only installs the prebuilt artifact, it does not \
compile anything."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

SRC_URI = " \
    file://terminal-agent \
    file://terminal-agent.toml \
    file://thing-revived-terminal-agent.service \
"
S = "${UNPACKDIR}"

inherit systemd

INSANE_SKIP:${PN} += "already-stripped"
do_compile[noexec] = "1"
do_configure[noexec] = "1"

SYSTEMD_SERVICE:${PN} = "thing-revived-terminal-agent.service"
SYSTEMD_AUTO_ENABLE:${PN} = "enable"

do_install() {
    install -d ${D}${bindir}
    install -m 0755 ${S}/terminal-agent ${D}${bindir}/terminal-agent

    install -d ${D}${sysconfdir}/thing-revived
    install -m 0644 ${S}/terminal-agent.toml ${D}${sysconfdir}/thing-revived/terminal-agent.toml

    install -d ${D}${systemd_unitdir}/system
    install -m 0644 ${S}/thing-revived-terminal-agent.service ${D}${systemd_unitdir}/system/thing-revived-terminal-agent.service
}

FILES:${PN} += " \
    ${bindir}/terminal-agent \
    ${sysconfdir}/thing-revived/terminal-agent.toml \
    ${systemd_unitdir}/system/thing-revived-terminal-agent.service \
"

# Keep local edits to the config across package updates.
CONFFILES:${PN} += "${sysconfdir}/thing-revived/terminal-agent.toml"
