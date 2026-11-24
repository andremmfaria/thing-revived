SUMMARY = "Templated root-autologin console unit, runtime-toggleable per instance"
DESCRIPTION = "Ships thing-revived-uart-root@.service unconditionally (docs/research/\
debug-toggles-plan.md section 4.4): the unit always exists on the rootfs so the Settings menu's \
UART toggle works on every build, but whether it is ENABLED for a given tty (symlinked into \
getty.target.wants) is decided at image build time by THING_REVIVED_ENABLE_UART, in \
thing_revived_console_gate (superbird-kiosk-prod-image.bbappend), the same enablement-not-\
presence split used for the adb/usb-bind pair. Conflicts=serial-getty@%i.service stops only the \
matching stock getty instance when this unit starts for that instance; stopping this unit does \
not restart the stock getty automatically (systemd does not revive the Conflicts= loser), so \
disabling it needs an explicit start of serial-getty@<tty>.service, done by the gate at build \
time and by the controller at runtime. The @@THING_REVIVED_STOCK_GETTY_ARGS@@ placeholder in the \
shipped unit is filled in by thing_revived_console_gate, copying the stock serial-getty@.service \
ExecStart line from the SAME rootfs verbatim (plus --autologin root), so baud rate and $TERM \
always match the stock console; the shared serial-getty@.service template itself is never \
edited, unlike before this change."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

SRC_URI = " \
    file://thing-revived-uart-root@.service \
"
S = "${UNPACKDIR}"

inherit allarch

RDEPENDS:${PN} = "systemd util-linux-agetty"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${S}/thing-revived-uart-root@.service \
        ${D}${systemd_system_unitdir}/thing-revived-uart-root@.service
}

FILES:${PN} = " \
    ${systemd_system_unitdir}/thing-revived-uart-root@.service \
"
