SUMMARY = "Persistent device settings boot apply: early unit, late unit, failsafe unit, sshd.socket gate"
DESCRIPTION = "Ships the boot-time half of the Device settings feature (docs/research/\
device-settings-plan.md section 4.4). thing-revived-device-settings.service (unit A, early, \
DefaultDependencies=no, file writes only) loads or seeds /var/lib/thing-revived/device-settings.json, \
mirrors Mode to boot-role and writes or removes /run/thing-revived/ssh-off. \
thing-revived-device-apply.service (unit B, late) starts adbd or usb-bind, the UART console and ssh \
to match the persisted values; its OnFailure= is thing-revived-device-failsafe.service (unit F), \
which only restores a USB link and a console. The sshd.socket.d/20-thing-revived-ssh-gate.conf \
drop-in keeps sshd.socket from starting while the ssh-off flag exists. The image preset is \
'disable *', so the enablement symlinks for A (sysinit.target.wants) and B (multi-user.target.wants) \
are created in do_install, the same way thing-revived-accounts does it. All three units run \
'controller-agent apply-device-settings early|late|failsafe', shipped by the controller-agent \
recipe; they never read controller.toml."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

SRC_URI = " \
    file://thing-revived-device-settings.service \
    file://thing-revived-device-apply.service \
    file://thing-revived-device-failsafe.service \
    file://thing-revived-ssh-gate.conf \
    file://00-thing-revived.preset \
"
S = "${UNPACKDIR}"

inherit allarch

RDEPENDS:${PN} = "controller-agent systemd"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${systemd_system_unitdir}
    install -m 0644 ${S}/thing-revived-device-settings.service ${D}${systemd_system_unitdir}/thing-revived-device-settings.service
    install -m 0644 ${S}/thing-revived-device-apply.service ${D}${systemd_system_unitdir}/thing-revived-device-apply.service
    install -m 0644 ${S}/thing-revived-device-failsafe.service ${D}${systemd_system_unitdir}/thing-revived-device-failsafe.service

    install -d ${D}${systemd_system_unitdir}/sshd.socket.d
    install -m 0644 ${S}/thing-revived-ssh-gate.conf \
        ${D}${systemd_system_unitdir}/sshd.socket.d/20-thing-revived-ssh-gate.conf

    # Early-sorting preset: `systemctl preset-all` (systemd_handle_machine_id) runs after every
    # thing_revived_* rootfs post-process function and would otherwise enable adbd from its
    # 98-android-tools-adbd.preset (WantedBy=basic.target). First match wins, names sort in order.
    install -d ${D}${systemd_unitdir}/system-preset
    install -m 0644 ${S}/00-thing-revived.preset ${D}${systemd_unitdir}/system-preset/00-thing-revived.preset

    # Same symlinks `systemctl enable` creates from WantedBy=. Unit F has no [Install] on purpose.
    install -d ${D}${sysconfdir}/systemd/system/sysinit.target.wants
    ln -sf ${systemd_system_unitdir}/thing-revived-device-settings.service \
        ${D}${sysconfdir}/systemd/system/sysinit.target.wants/thing-revived-device-settings.service
    install -d ${D}${sysconfdir}/systemd/system/multi-user.target.wants
    ln -sf ${systemd_system_unitdir}/thing-revived-device-apply.service \
        ${D}${sysconfdir}/systemd/system/multi-user.target.wants/thing-revived-device-apply.service
}

FILES:${PN} = " \
    ${systemd_system_unitdir}/thing-revived-device-settings.service \
    ${systemd_system_unitdir}/thing-revived-device-apply.service \
    ${systemd_system_unitdir}/thing-revived-device-failsafe.service \
    ${systemd_system_unitdir}/sshd.socket.d/20-thing-revived-ssh-gate.conf \
    ${systemd_unitdir}/system-preset/00-thing-revived.preset \
    ${sysconfdir}/systemd/system/sysinit.target.wants/thing-revived-device-settings.service \
    ${sysconfdir}/systemd/system/multi-user.target.wants/thing-revived-device-apply.service \
"
