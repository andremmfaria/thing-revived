# Replace the stock /etc/sudoers: members of group sudo get full sudo with their own password.
# No NOPASSWD and no !authenticate anywhere (build/tools/check_image_accounts.py greps for both).
FILESEXTRAPATHS:prepend := "${THISDIR}/files:"

SRC_URI += "file://sudoers"

do_install:append() {
    # The sudo-lib package owns /etc/sudoers (CONFFILES) and the whole of ${sysconfdir}.
    install -m 0440 ${UNPACKDIR}/sudoers ${D}${sysconfdir}/sudoers
    install -d -m 0750 ${D}${sysconfdir}/sudoers.d

    # visudo is a target binary here (there is no sudo-native), so check the file with a
    # static rule check instead. A failure stops the build.
    if grep -Eq 'NOPASSWD|![[:space:]]*authenticate' ${D}${sysconfdir}/sudoers; then
        bbfatal "sudoers must not contain NOPASSWD or !authenticate"
    fi
    if ! grep -Eq '^%sudo[[:space:]]+ALL=\(ALL:ALL\)[[:space:]]+ALL$' ${D}${sysconfdir}/sudoers; then
        bbfatal "sudoers lacks the '%sudo ALL=(ALL:ALL) ALL' rule"
    fi
}
