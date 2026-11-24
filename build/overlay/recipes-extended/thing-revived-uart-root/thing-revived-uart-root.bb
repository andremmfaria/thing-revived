SUMMARY = "Templated root-autologin console unit, runtime-toggleable per instance"
DESCRIPTION = "Ships thing-revived-uart-root@.service unconditionally (docs/research/\
device-settings-plan.md sections 4.4 and 7.1). The unit always exists on the rootfs and is DISABLED \
on every build (no getty.target.wants symlink): thing-revived-device-apply.service starts it for \
ttyAML0 at boot when the persisted UART setting is on, and the controller starts and stops it live. \
Conflicts=serial-getty@%i.service stops only the matching stock getty instance when this unit \
starts for that instance; stopping this unit does not restart the stock getty automatically \
(systemd does not revive the Conflicts= loser), so disabling it needs an explicit start of \
serial-getty@<tty>.service, done by the boot apply and by the controller. The stock getty is never \
masked: the getty generator starts it on the console tty by default, which is the UART off state. \
The @@THING_REVIVED_STOCK_GETTY_ARGS@@ placeholder in the shipped unit is filled in by \
thing_revived_console_gate (superbird-kiosk-prod-image.bbappend), copying the stock \
serial-getty@.service ExecStart line from the SAME rootfs verbatim (plus --autologin root), so \
baud rate and $TERM always match the stock console; the shared serial-getty@.service template \
itself is never edited."
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
