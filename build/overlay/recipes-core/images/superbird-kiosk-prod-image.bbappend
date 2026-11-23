IMAGE_OVERHEAD_FACTOR = "1.0"
IMAGE_ROOTFS_EXTRA_SPACE = "4096"

IMAGE_INSTALL:append = " \
    superbird-weston-init-kiosk \
    bash \
    thing-revived-terminal-agent \
    controller-agent \
    thing-revived-ntp-config \
    thing-revived-accounts \
    sudo \
    pam-plugin-faillock \
"

# Host mode via a USB-C dock (superbird-usb-role to-host): USB Ethernet and
# mass-storage drivers. The image otherwise ships no /lib/modules, so these
# modules (built by the superbird-usb-host.cfg kernel fragment) must be
# installed explicitly to be loadable.
IMAGE_INSTALL:append = " \
    kernel-module-usbnet \
    kernel-module-mii \
    kernel-module-r8152 \
    kernel-module-ax88179-178a \
    kernel-module-asix \
    kernel-module-cdc-ether \
    kernel-module-cdc-ncm \
    kernel-module-rndis-host \
    kernel-module-usb-storage \
    kernel-module-uas \
    kernel-module-sd-mod \
"

# USB Wi-Fi adapters (host mode via a USB-C dock, superbird-usb-role to-host). Drivers are built as
# modules by thing-revived-wifi.cfg. Only the driver leaf modules are named: kernel-module-split
# derives each package's RDEPENDS from modules.dep, so rtw88_core/rtw88_usb/mt76*/rt2x00*,
# mac80211, cfg80211 and libarc4 follow automatically (cfg80211 and mac80211 are also named for clarity).
# The Wi-Fi runtime config (wpa_supplicant unit, networkd file, tmpfiles) is thing-revived-wifi-config;
# networks are added from the on-device settings menu (docs/wifi.md).
IMAGE_INSTALL:append = " \
    kernel-module-cfg80211 \
    kernel-module-mac80211 \
    kernel-module-rtw88-8821au \
    kernel-module-rtw88-8821cu \
    kernel-module-rtw88-8812au \
    kernel-module-rtw88-8814au \
    kernel-module-rtw88-8822bu \
    kernel-module-rtw88-8822cu \
    kernel-module-rtw88-8723du \
    kernel-module-mt76x0u \
    kernel-module-mt76x2u \
    kernel-module-mt7601u \
    kernel-module-rt2800usb \
    kernel-module-rtl8xxxu \
    linux-firmware-rtl8821 \
    linux-firmware-rtl8812 \
    linux-firmware-rtl8814 \
    linux-firmware-rtl8822 \
    linux-firmware-rtl8723 \
    linux-firmware-rtl8188 \
    linux-firmware-rtl8192cu \
    linux-firmware-rtl8192 \
    linux-firmware-rtl8710 \
    linux-firmware-mt7610u \
    linux-firmware-mt76x2 \
    linux-firmware-mt7601u \
    linux-firmware-ralink \
    wireless-regdb-static \
    wpa-supplicant \
    wpa-supplicant-cli \
    wpa-supplicant-passphrase \
    iw \
    thing-revived-wifi-udev \
    thing-revived-wifi-config \
"

# ---------------------------------------------------------------------------
# Account lockdown (docs/research/user-management-plan.md, section 4).
# ---------------------------------------------------------------------------

# kas/base.yml puts these four into EXTRA_IMAGE_FEATURES, which bitbake.conf
# appends to IMAGE_FEATURES. Removing them makes oe-core stop writing
# PermitRootLogin yes / PermitEmptyPasswords yes (and nullok_secure -> nullok),
# stop the --autologin root edit of serial-getty@.service, and run
# zap_empty_root_password (root:: becomes root:*:). ssh-server-openssh stays.
IMAGE_FEATURES:remove = "allow-empty-password allow-root-login empty-root-password serial-autologin-root"

# Dev flags, passed in by build-image.sh through the generated kas file. Only
# the value 1 enables; the post-commands below treat anything else as off.
THING_REVIVED_ENABLE_ADB ?= "0"
THING_REVIVED_ENABLE_UART ?= "0"

# Must equal [users] sha_rounds in config/controller.toml and the rounds= that
# libpam_%.bbappend writes into common-password.
THING_REVIVED_SHA_ROUNDS ?= "65536"

# The kiosk administrator. No -p: useradd without a password writes "!" (a
# shell-active '!' is fragile through set_user_group). No -m: /var/home/kiosk
# is created at boot by tmpfiles because /var is not part of the rootfs. The
# password is set on first boot by the controller. root is locked on top of
# the "*" that zap_empty_root_password writes, so a missed zap still fails the
# assertion below instead of shipping an open root.
inherit extrausers
EXTRA_USERS_PARAMS = " \
    useradd -u 1000 -U -d /var/home/kiosk -s /usr/bin/bash -G sudo,adm kiosk; \
    usermod -L root; \
"

# The order matters: extrausers' set_user_group is already queued by the
# inherit above, so these run after the kiosk account exists.
ROOTFS_POSTPROCESS_COMMAND += "thing_revived_console_gate "
ROOTFS_POSTPROCESS_COMMAND += "thing_revived_adb_gate "
ROOTFS_POSTPROCESS_COMMAND += "thing_revived_login_defs "
ROOTFS_POSTPROCESS_COMMAND += "thing_revived_assert_accounts "

# UART root autologin only with THING_REVIVED_ENABLE_UART=1. Without it the
# console needs a password and /etc/securetty is empty. securetty is truncated
# here and not replaced by a recipe because shadow-securetty owns the file and
# a second owner collides under package_ipk. With the flag on, the securetty
# file is left as shipped and the sed below replicates oe-core's
# serial_autologin_root (which is gated on empty-root-password, removed above).
# Root stays locked either way: agetty --autologin does not need a hash.
thing_revived_console_gate () {
    securetty="${IMAGE_ROOTFS}${sysconfdir}/securetty"
    serial_unit="${IMAGE_ROOTFS}${systemd_system_unitdir}/serial-getty@.service"
    if [ "${THING_REVIVED_ENABLE_UART}" = "1" ]; then
        bbwarn "THING_REVIVED_ENABLE_UART=1: the serial console logs in as root without a password"
        if [ -e "$serial_unit" ]; then
            sed -i '/^\s*ExecStart\b/ s/getty /&--autologin root /' "$serial_unit"
        fi
    else
        mkdir -p "${IMAGE_ROOTFS}${sysconfdir}"
        : > "$securetty"
    fi
}

# adbd only starts when /etc/usb-debugging-enabled exists (the unit has
# ConditionPathExists on it); the file comes from superbird-usb-gadget.
thing_revived_adb_gate () {
    if [ "${THING_REVIVED_ENABLE_ADB}" = "1" ]; then
        bbwarn "THING_REVIVED_ENABLE_ADB=1: adbd gives an unauthenticated root shell over USB"
    else
        rm -f "${IMAGE_ROOTFS}${sysconfdir}/usb-debugging-enabled"
    fi
}

# Hash settings used by passwd and by anything that reads login.defs. chpasswd
# in the controller passes the method and rounds explicitly; this keeps the
# stock tools consistent with it. The kiosk shell must also be a listed shell.
thing_revived_login_defs () {
    defs="${IMAGE_ROOTFS}${sysconfdir}/login.defs"
    if [ ! -f "$defs" ]; then
        bbfatal "login.defs is missing from the rootfs"
    fi
    sed -i -E '/^[[:space:]]*(ENCRYPT_METHOD|SHA_CRYPT_MIN_ROUNDS|SHA_CRYPT_MAX_ROUNDS)[[:space:]]/d' "$defs"
    printf 'ENCRYPT_METHOD SHA512\nSHA_CRYPT_MIN_ROUNDS ${THING_REVIVED_SHA_ROUNDS}\nSHA_CRYPT_MAX_ROUNDS ${THING_REVIVED_SHA_ROUNDS}\n' >> "$defs"

    shells="${IMAGE_ROOTFS}${sysconfdir}/shells"
    if [ -f "$shells" ] && ! grep -qx '/usr/bin/bash' "$shells"; then
        echo '/usr/bin/bash' >> "$shells"
    fi
}

# Safety net, same rules as the image check script (gate G1c) so a bad rootfs
# stops the build instead of producing a flashable image.
thing_revived_assert_accounts () {
    etc="${IMAGE_ROOTFS}${sysconfdir}"
    shadow="$etc/shadow"
    [ -f "$shadow" ] || bbfatal "/etc/shadow is missing from the rootfs"

    root_hash=$(awk -F: '$1 == "root" { print $2 }' "$shadow")
    case "$root_hash" in
        '*'*|'!'*) ;;
        *) bbfatal "root is not locked in /etc/shadow (hash field '$root_hash')" ;;
    esac

    kiosk_hash=$(awk -F: '$1 == "kiosk" { print $2 }' "$shadow")
    [ "$kiosk_hash" = '!' ] || bbfatal "kiosk must have the locked hash '!' in /etc/shadow (got '$kiosk_hash')"

    empty=$(awk -F: 'NF >= 2 && $2 == "" { print $1 }' "$shadow")
    [ -z "$empty" ] || bbfatal "accounts with an empty password hash in /etc/shadow: $empty"

    if [ -d "$etc/pam.d" ] && grep -rEq '^[^#]*nullok' "$etc/pam.d"; then
        bbfatal "nullok is still present under /etc/pam.d"
    fi
    if ! grep -Eq '^auth[[:space:]].*pam_faillock\.so' "$etc/pam.d/common-auth" 2>/dev/null; then
        bbfatal "/etc/pam.d/common-auth does not carry pam_faillock.so"
    fi

    # No SSH key is baked into the image, and the sshd drop-in keeps key-only as the global default
    # with a password exception for kiosk only.
    if [ -d "${IMAGE_ROOTFS}${datadir}/thing-revived" ] && \
        [ -n "$(find "${IMAGE_ROOTFS}${datadir}/thing-revived" -name '*.authorized_keys' 2>/dev/null)" ]; then
        bbfatal "a *.authorized_keys file is baked under ${datadir}/thing-revived"
    fi
    hardening="$etc/ssh/sshd_config.d/10-thing-revived-hardening.conf"
    [ -f "$hardening" ] || bbfatal "$hardening is missing from the rootfs"
    grep -Eq '^Match User kiosk[[:space:]]*$' "$hardening" || bbfatal "$hardening has no 'Match User kiosk' block"
    pre_match=$(sed '/^Match /,$d' "$hardening")
    printf '%s\n' "$pre_match" | grep -Eq '^PasswordAuthentication[[:space:]]+no[[:space:]]*$' \
        || bbfatal "$hardening must set a global 'PasswordAuthentication no' before the Match block"

    for cfg in "$etc/ssh/sshd_config" "$etc/ssh/sshd_config_readonly"; do
        if [ -f "$cfg" ] && grep -Eqi '^[[:space:]]*(PermitRootLogin|PermitEmptyPasswords)[[:space:]]+yes' "$cfg"; then
            bbfatal "$cfg allows root login or empty passwords"
        fi
    done

    serial_unit="${IMAGE_ROOTFS}${systemd_system_unitdir}/serial-getty@.service"
    if [ -f "$serial_unit" ] && grep -Eq '^[[:space:]]*ExecStart.*--autologin' "$serial_unit"; then
        [ "${THING_REVIVED_ENABLE_UART}" = "1" ] || bbfatal "serial-getty@.service has an autologin but THING_REVIVED_ENABLE_UART is not 1"
    elif [ "${THING_REVIVED_ENABLE_UART}" = "1" ] && [ -f "$serial_unit" ]; then
        bbfatal "THING_REVIVED_ENABLE_UART=1 but serial-getty@.service has no autologin"
    fi
    if [ "${THING_REVIVED_ENABLE_UART}" != "1" ] && [ -s "$etc/securetty" ]; then
        bbfatal "/etc/securetty is not empty and THING_REVIVED_ENABLE_UART is not 1"
    fi

    if [ -e "$etc/usb-debugging-enabled" ] && [ "${THING_REVIVED_ENABLE_ADB}" != "1" ]; then
        bbfatal "/etc/usb-debugging-enabled exists but THING_REVIVED_ENABLE_ADB is not 1"
    fi
    if [ ! -e "$etc/usb-debugging-enabled" ] && [ "${THING_REVIVED_ENABLE_ADB}" = "1" ]; then
        bbwarn "THING_REVIVED_ENABLE_ADB=1 but /etc/usb-debugging-enabled is not in the image"
    fi
}
