SUMMARY = "Account lockdown: /etc overlay, key-only sshd with a password-only exception for kiosk, first-boot provisioning units"
DESCRIPTION = "Installs the writable overlay on /etc (thing-revived-etc.service, upper layer on /var), \
thing-revived-provision.service (runs 'controller-agent provision' before sshd and the controller), \
the sshd and sshd.socket drop-ins (key-only by default, no root login, password or key for the kiosk \
user only), and tmpfiles for the account state. No SSH key is baked into the image. The sudoers file \
is installed by the sudo bbappend."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

SRC_URI = " \
    file://10-thing-revived-hardening.conf \
    file://thing-revived-sshd-socket.conf \
    file://thing-revived-accounts.tmpfiles \
    file://thing-revived-etc.service \
    file://thing-revived-provision.service \
    file://thing-revived-etc-mount.sh \
"
S = "${UNPACKDIR}"

inherit allarch systemd

RDEPENDS:${PN} = "bash libpam openssh-sshd shadow sudo systemd"

# Dev flags from build/build-image.sh (kas env map and literal local.conf values). Only the
# value 1 enables a flag. The image bbappend (T-P1.2) reads the same variables.
THING_REVIVED_ENABLE_ADB ?= "0"
THING_REVIVED_ENABLE_UART ?= "0"
do_install[vardeps] += "THING_REVIVED_ENABLE_ADB THING_REVIVED_ENABLE_UART"

ACCOUNTS_SHAREDIR = "${datadir}/thing-revived/accounts"

SYSTEMD_SERVICE:${PN} = "thing-revived-etc.service thing-revived-provision.service"
SYSTEMD_AUTO_ENABLE:${PN} = "enable"

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${sysconfdir}/ssh/sshd_config.d
    install -m 0644 ${S}/10-thing-revived-hardening.conf \
        ${D}${sysconfdir}/ssh/sshd_config.d/10-thing-revived-hardening.conf

    install -d ${D}${systemd_unitdir}/system/sshd.socket.d
    install -m 0644 ${S}/thing-revived-sshd-socket.conf \
        ${D}${systemd_unitdir}/system/sshd.socket.d/10-thing-revived-limits.conf

    install -d ${D}${systemd_unitdir}/system
    install -m 0644 ${S}/thing-revived-etc.service ${D}${systemd_unitdir}/system/thing-revived-etc.service
    install -m 0644 ${S}/thing-revived-provision.service ${D}${systemd_unitdir}/system/thing-revived-provision.service

    install -d ${D}${libexecdir}/thing-revived
    install -m 0755 ${S}/thing-revived-etc-mount.sh ${D}${libexecdir}/thing-revived/thing-revived-etc-mount.sh

    install -d ${D}${nonarch_libdir}/tmpfiles.d
    install -m 0644 ${S}/thing-revived-accounts.tmpfiles ${D}${nonarch_libdir}/tmpfiles.d/thing-revived-accounts.conf

    install -d -m 0755 ${D}${datadir}/thing-revived
    install -d -m 0755 ${D}${ACCOUNTS_SHAREDIR}

    # Effective dev flags, read by build/tools/check_image_accounts.py.
    adb=0
    uart=0
    [ "${THING_REVIVED_ENABLE_ADB}" = "1" ] && adb=1
    [ "${THING_REVIVED_ENABLE_UART}" = "1" ] && uart=1
    printf 'THING_REVIVED_ENABLE_ADB=%s\nTHING_REVIVED_ENABLE_UART=%s\n' "$adb" "$uart" \
        > ${D}${ACCOUNTS_SHAREDIR}/build-flags
    chmod 0644 ${D}${ACCOUNTS_SHAREDIR}/build-flags
}

FILES:${PN} = " \
    ${sysconfdir}/ssh/sshd_config.d/10-thing-revived-hardening.conf \
    ${systemd_unitdir}/system/sshd.socket.d/10-thing-revived-limits.conf \
    ${systemd_unitdir}/system/thing-revived-etc.service \
    ${systemd_unitdir}/system/thing-revived-provision.service \
    ${libexecdir}/thing-revived/thing-revived-etc-mount.sh \
    ${nonarch_libdir}/tmpfiles.d/thing-revived-accounts.conf \
    ${datadir}/thing-revived \
"
