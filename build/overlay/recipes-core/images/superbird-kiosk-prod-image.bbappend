IMAGE_OVERHEAD_FACTOR = "1.0"
IMAGE_ROOTFS_EXTRA_SPACE = "4096"

IMAGE_INSTALL:append = " \
    superbird-weston-init-kiosk \
    bash \
    thing-revived-terminal-agent \
    controller-agent \
    thing-revived-ntp-config \
    thing-revived-usb-bind \
    thing-revived-uart-root \
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
# thing_revived_pam writes into common-password.
THING_REVIVED_SHA_ROUNDS ?= "65536"

# pam_faillock (owner spec S8, delta plan section 7.2): inline options, no faillock.conf. The
# counter lives in tmpfs /var/run/faillock, so a reboot resets it. The plugin package is
# pam-plugin-faillock (IMAGE_INSTALL above).
THING_REVIVED_FAILLOCK_OPTS ?= "deny=5 unlock_time=600 fail_interval=900"

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
ROOTFS_POSTPROCESS_COMMAND += "thing_revived_pam "
ROOTFS_POSTPROCESS_COMMAND += "thing_revived_assert_accounts "

# UART root console, runtime-toggleable on every build (docs/research/debug-toggles-plan.md
# section 4.4). thing-revived-uart-root@.service (build/overlay/recipes-extended/
# thing-revived-uart-root) always ships; only its ENABLEMENT for ttyAML0 (symlinked into
# getty.target.wants, exactly like the adb/usb-bind pair below) is decided by
# THING_REVIVED_ENABLE_UART. The unit's ExecStart ships as a @@THING_REVIVED_STOCK_GETTY_ARGS@@
# placeholder and is filled in here, unconditionally, by copying the stock serial-getty@.service's
# own ExecStart line verbatim (plus --autologin root) from this SAME rootfs, so baud/TERM always
# match. This replaces the old approach of sed-ing --autologin into the shared serial-getty@.
# service template directly: that template is shared by every possible getty instance and must
# stay untouched and autologin-free in EVERY build (a live edit to it would not be this plan's to
# make, and a static sed only on flag=1 builds cannot support a runtime toggle that must also work
# when the boot default is flag=0).
#
# /etc/securetty (owned by shadow-securetty, left as shipped, listing ttyAML0) is asserted but no
# longer conditionally truncated here: root stays locked (agetty --autologin does not need a
# hash), but login(1), which --autologin still execs, refuses root on a tty absent from
# securetty regardless of which gadget side booted. Truncating it on flag=0 builds (the old
# behaviour) would silently break the runtime toggle on exactly the builds where the owner's spec
# says it must still work (debug-toggles-plan.md section 0: "on EVERY build ... offers a live
# toggle"). This is a deviation from the plan's section 4.4, which did not call this out; flagged
# in the task return.
thing_revived_console_gate () {
    tty="ttyAML0"
    etc="${IMAGE_ROOTFS}${sysconfdir}"
    unitdir="${IMAGE_ROOTFS}${systemd_system_unitdir}"
    serial_unit="$unitdir/serial-getty@.service"
    uart_unit="$unitdir/thing-revived-uart-root@.service"
    wants="$etc/systemd/system/getty.target.wants"

    [ -f "$serial_unit" ] || bbfatal "serial-getty@.service is missing from the rootfs"
    [ -f "$uart_unit" ] || bbfatal "thing-revived-uart-root@.service is missing from the rootfs (is thing-revived-uart-root in IMAGE_INSTALL?)"
    if grep -Eq '^[[:space:]]*ExecStart=.*--autologin' "$serial_unit"; then
        bbfatal "serial-getty@.service has an autologin ExecStart; the shared template must stay untouched"
    fi

    stock_line=$(sed -nE 's/^[[:space:]]*ExecStart=//p' "$serial_unit" | head -n1)
    [ -n "$stock_line" ] || bbfatal "could not read ExecStart from serial-getty@.service"
    autologin_line=$(printf '%s' "$stock_line" | sed 's/getty /&--autologin root /')
    # Escape sed's own metacharacters (and the | delimiter used below) in the replacement text,
    # since $autologin_line carries a real path (e.g. /usr/sbin/agetty) verbatim.
    escaped_line=$(printf '%s' "$autologin_line" | sed -e 's/[&|\\]/\\&/g')
    sed -i "s|@@THING_REVIVED_STOCK_GETTY_ARGS@@|$escaped_line|" "$uart_unit"
    grep -q '@@THING_REVIVED_STOCK_GETTY_ARGS@@' "$uart_unit" \
        && bbfatal "thing-revived-uart-root@.service still has the unsubstituted ExecStart placeholder"

    grep -qx "$tty" "$etc/securetty" 2>/dev/null \
        || bbfatal "/etc/securetty does not list $tty; the UART runtime toggle needs root login permitted there on every build"

    mkdir -p "$wants"
    rm -f "$wants/serial-getty@${tty}.service" "$wants/thing-revived-uart-root@${tty}.service"
    if [ "${THING_REVIVED_ENABLE_UART}" = "1" ]; then
        bbwarn "THING_REVIVED_ENABLE_UART=1: the serial console logs in as root without a password"
        ln -sf "${systemd_system_unitdir}/thing-revived-uart-root@.service" \
            "$wants/thing-revived-uart-root@${tty}.service"
    else
        ln -sf "${systemd_system_unitdir}/serial-getty@.service" \
            "$wants/serial-getty@${tty}.service"
    fi
}

# adb/usb-bind pair, runtime-toggleable on every build (docs/research/debug-toggles-plan.md
# section 2 and 4.1). Both units always ship (android-tools-adbd.service comes from meta-oe via
# superbird-usb-gadget's RDEPENDS; thing-revived-usb-bind from this repo); only which one is
# ENABLED at boot (symlinked into its own [Install] WantedBy target: android-tools-adbd.service
# declares WantedBy=basic.target upstream, thing-revived-usb-bind.service declares
# WantedBy=multi-user.target) is decided by THING_REVIVED_ENABLE_ADB. Conflicts= on both sides
# (the zz-thing-revived-toggle.conf drop-in on adbd, the unit file itself on usb-bind) makes
# either side runtime-startable regardless of which was enabled at boot, which is what lets the
# Settings menu toggle work on every build.
#
# /etc/usb-debugging-enabled keeps meaning ONLY "this is an ADB build" for the usb-role drop-in's
# boot-time forced-device-role check (build/overlay/recipes-bsp/superbird-usb-role); nothing here
# or at runtime reads it to decide whether adbd runs, that is systemd's own unit-enablement state,
# re-queried live by the controller (never cached). It is removed on ADB=0 builds only because the
# usb-role drop-in's own check must see "this build never forces device role", a build-time-only
# concern unrelated to the runtime toggle.
thing_revived_adb_gate () {
    etc="${IMAGE_ROOTFS}${sysconfdir}"
    unitdir="${IMAGE_ROOTFS}${systemd_system_unitdir}"
    adb_wants="$etc/systemd/system/basic.target.wants"
    bind_wants="$etc/systemd/system/multi-user.target.wants"
    adb_dropin="$unitdir/android-tools-adbd.service.d/zz-thing-revived-toggle.conf"
    adb_unit="$unitdir/android-tools-adbd.service"
    bind_unit="$unitdir/thing-revived-usb-bind.service"

    [ -f "$adb_unit" ] || bbfatal "android-tools-adbd.service is missing from the rootfs"
    [ -f "$bind_unit" ] || bbfatal "thing-revived-usb-bind.service is missing from the rootfs"
    [ -f "$adb_dropin" ] || bbfatal "android-tools-adbd.service.d/zz-thing-revived-toggle.conf is missing from the rootfs"
    grep -Eq '^Conflicts=[[:space:]]*thing-revived-usb-bind\.service[[:space:]]*$' "$adb_dropin" \
        || bbfatal "$adb_dropin has no Conflicts=thing-revived-usb-bind.service"
    grep -Eq '^Conflicts=[[:space:]]*android-tools-adbd\.service[[:space:]]*$' "$bind_unit" \
        || bbfatal "thing-revived-usb-bind.service has no Conflicts=android-tools-adbd.service"

    mkdir -p "$adb_wants" "$bind_wants"
    rm -f "$adb_wants/android-tools-adbd.service" "$bind_wants/thing-revived-usb-bind.service"
    if [ "${THING_REVIVED_ENABLE_ADB}" = "1" ]; then
        bbwarn "THING_REVIVED_ENABLE_ADB=1: adbd gives an unauthenticated root shell over USB"
        ln -sf "$adb_unit" "$adb_wants/android-tools-adbd.service"
    else
        rm -f "$etc/usb-debugging-enabled"
        ln -sf "$bind_unit" "$bind_wants/thing-revived-usb-bind.service"
    fi

    adb_enabled=0
    bind_enabled=0
    [ -L "$adb_wants/android-tools-adbd.service" ] && adb_enabled=1
    [ -L "$bind_wants/thing-revived-usb-bind.service" ] && bind_enabled=1
    if [ "$adb_enabled" = "1" ] && [ "$bind_enabled" = "1" ]; then
        bbfatal "both android-tools-adbd.service and thing-revived-usb-bind.service are enabled"
    fi
    if [ "$adb_enabled" = "0" ] && [ "$bind_enabled" = "0" ]; then
        bbfatal "neither android-tools-adbd.service nor thing-revived-usb-bind.service is enabled"
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

# PAM hardening, done on the image rootfs and not in a libpam bbappend: a libpam do_install change
# alters libpam's signature, which cascades through libcap (PACKAGECONFIG pam) into systemd,
# gtk+3 and chromium and forces a full chromium rebuild. Must run before
# thing_revived_assert_accounts. Idempotent. The pam.d files are packaged in libpam-runtime.
#   - nullok / nullok_secure removed everywhere, so an empty hash can never authenticate
#     (oe-core's ssh_allow_empty_password may have rewritten nullok_secure to nullok).
#   - rounds= added to the sha512 pam_unix line of common-password.
#   - pam_faillock preauth before pam_unix, authfail and authsucc after it, and the account line.
#   - no pam_wheel restriction on su: pam_wheel is not shipped, and su to root cannot succeed
#     with a locked root anyway.
thing_revived_pam () {
    pamd="${IMAGE_ROOTFS}${sysconfdir}/pam.d"
    for f in common-auth common-account common-password; do
        [ -f "$pamd/$f" ] || bbfatal "$pamd/$f is missing from the rootfs"
    done

    for f in "$pamd"/*; do
        [ -f "$f" ] || continue
        sed -i -E 's/[[:space:]]+nullok(_secure)?([[:space:]]|$)/\2/g' "$f"
    done

    if ! grep -q 'rounds=' "$pamd/common-password"; then
        sed -i -E '/^password[[:space:]].*pam_unix\.so/ s/[[:space:]]sha512/ sha512 rounds=${THING_REVIVED_SHA_ROUNDS}/' \
            "$pamd/common-password"
    fi

    if ! grep -q 'pam_faillock' "$pamd/common-auth"; then
        grep -Eq '^auth[[:space:]].*pam_unix\.so' "$pamd/common-auth" \
            || bbfatal "common-auth has no pam_unix.so auth line to place pam_faillock around"
        sed -i -E \
            -e '/^auth[[:space:]].*pam_unix\.so/ i auth\trequired\t\t\tpam_faillock.so preauth ${THING_REVIVED_FAILLOCK_OPTS}' \
            -e '/^auth[[:space:]].*pam_unix\.so/ a auth\t[default=die]\t\t\tpam_faillock.so authfail ${THING_REVIVED_FAILLOCK_OPTS}\nauth\tsufficient\t\t\tpam_faillock.so authsucc ${THING_REVIVED_FAILLOCK_OPTS}' \
            "$pamd/common-auth"
    fi
    if ! grep -q 'pam_faillock' "$pamd/common-account"; then
        printf 'account\trequired\t\t\tpam_faillock.so\n' >> "$pamd/common-account"
    fi

    if grep -rEq '^[^#]*nullok' "$pamd"; then
        bbfatal "nullok is still present in $pamd"
    fi
    if ! grep -Eq '^password[[:space:]].*pam_unix\.so.*rounds=${THING_REVIVED_SHA_ROUNDS}' "$pamd/common-password"; then
        bbfatal "common-password does not carry rounds=${THING_REVIVED_SHA_ROUNDS}"
    fi
    for kw in preauth authfail authsucc; do
        if ! grep -Eq "^auth[[:space:]].*pam_faillock\.so $kw .*deny=5 .*unlock_time=600" "$pamd/common-auth"; then
            bbfatal "common-auth is missing the pam_faillock $kw line with deny=5 unlock_time=600"
        fi
    done
    if ! grep -Eq '^account[[:space:]]+required[[:space:]]+pam_faillock\.so' "$pamd/common-account"; then
        bbfatal "common-account is missing pam_faillock.so"
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

    # UART pair: the shared template must NEVER carry an autologin (regardless of the flag, see
    # thing_revived_console_gate); the templated unit must always ship; and exactly one of the two
    # getty.target.wants symlinks for ttyAML0 must exist, matching the flag. Checked here again,
    # after thing_revived_console_gate already ran, as the same kind of safety net
    # check_image_accounts.py (build/tools/) applies independently from outside the build.
    serial_unit="${IMAGE_ROOTFS}${systemd_system_unitdir}/serial-getty@.service"
    uart_unit="${IMAGE_ROOTFS}${systemd_system_unitdir}/thing-revived-uart-root@.service"
    getty_wants="$etc/systemd/system/getty.target.wants"
    if [ -f "$serial_unit" ] && grep -Eq '^[[:space:]]*ExecStart=.*--autologin' "$serial_unit"; then
        bbfatal "serial-getty@.service has an autologin ExecStart; the shared template must stay untouched in every build"
    fi
    [ -f "$uart_unit" ] || bbfatal "thing-revived-uart-root@.service is missing from the rootfs; it must ship on every build"
    if grep -q '@@THING_REVIVED_STOCK_GETTY_ARGS@@' "$uart_unit" 2>/dev/null; then
        bbfatal "thing-revived-uart-root@.service still has the unsubstituted ExecStart placeholder"
    fi
    uart_enabled=0
    serial_enabled=0
    [ -L "$getty_wants/thing-revived-uart-root@ttyAML0.service" ] && uart_enabled=1
    [ -L "$getty_wants/serial-getty@ttyAML0.service" ] && serial_enabled=1
    if [ "${THING_REVIVED_ENABLE_UART}" = "1" ]; then
        [ "$uart_enabled" = "1" ] || bbfatal "THING_REVIVED_ENABLE_UART=1 but thing-revived-uart-root@ttyAML0.service is not enabled"
        [ "$serial_enabled" = "0" ] || bbfatal "THING_REVIVED_ENABLE_UART=1 but serial-getty@ttyAML0.service is also enabled"
    else
        [ "$serial_enabled" = "1" ] || bbfatal "THING_REVIVED_ENABLE_UART is not 1 but serial-getty@ttyAML0.service is not enabled"
        [ "$uart_enabled" = "0" ] || bbfatal "THING_REVIVED_ENABLE_UART is not 1 but thing-revived-uart-root@ttyAML0.service is enabled"
    fi
    grep -qx "ttyAML0" "$etc/securetty" 2>/dev/null \
        || bbfatal "/etc/securetty does not list ttyAML0; the UART runtime toggle needs root login permitted there on every build"

    # adb/usb-bind pair: both units must ship, each must carry its half of the Conflicts= pair
    # (missing Conflicts= drop-ins fail the build per the owner's decision, debug-toggles-plan.md
    # section 12), and exactly one of the two WantedBy symlinks must exist, matching the flag.
    adb_unit="${IMAGE_ROOTFS}${systemd_system_unitdir}/android-tools-adbd.service"
    adb_dropin="${IMAGE_ROOTFS}${systemd_system_unitdir}/android-tools-adbd.service.d/zz-thing-revived-toggle.conf"
    bind_unit="${IMAGE_ROOTFS}${systemd_system_unitdir}/thing-revived-usb-bind.service"
    basic_wants="$etc/systemd/system/basic.target.wants"
    multiuser_wants="$etc/systemd/system/multi-user.target.wants"
    [ -f "$adb_unit" ] || bbfatal "android-tools-adbd.service is missing from the rootfs; it must ship on every build"
    [ -f "$bind_unit" ] || bbfatal "thing-revived-usb-bind.service is missing from the rootfs; it must ship on every build"
    [ -f "$adb_dropin" ] || bbfatal "android-tools-adbd.service.d/zz-thing-revived-toggle.conf is missing from the rootfs"
    grep -Eq '^Conflicts=[[:space:]]*thing-revived-usb-bind\.service[[:space:]]*$' "$adb_dropin" \
        || bbfatal "$adb_dropin has no Conflicts=thing-revived-usb-bind.service"
    grep -Eq '^Conflicts=[[:space:]]*android-tools-adbd\.service[[:space:]]*$' "$bind_unit" \
        || bbfatal "thing-revived-usb-bind.service has no Conflicts=android-tools-adbd.service"
    adb_enabled=0
    bind_enabled=0
    [ -L "$basic_wants/android-tools-adbd.service" ] && adb_enabled=1
    [ -L "$multiuser_wants/thing-revived-usb-bind.service" ] && bind_enabled=1
    if [ "${THING_REVIVED_ENABLE_ADB}" = "1" ]; then
        [ "$adb_enabled" = "1" ] || bbfatal "THING_REVIVED_ENABLE_ADB=1 but android-tools-adbd.service is not enabled"
        [ "$bind_enabled" = "0" ] || bbfatal "THING_REVIVED_ENABLE_ADB=1 but thing-revived-usb-bind.service is also enabled"
    else
        [ "$bind_enabled" = "1" ] || bbfatal "THING_REVIVED_ENABLE_ADB is not 1 but thing-revived-usb-bind.service is not enabled"
        [ "$adb_enabled" = "0" ] || bbfatal "THING_REVIVED_ENABLE_ADB is not 1 but android-tools-adbd.service is enabled"
    fi

    if [ -e "$etc/usb-debugging-enabled" ] && [ "${THING_REVIVED_ENABLE_ADB}" != "1" ]; then
        bbfatal "/etc/usb-debugging-enabled exists but THING_REVIVED_ENABLE_ADB is not 1"
    fi
    if [ ! -e "$etc/usb-debugging-enabled" ] && [ "${THING_REVIVED_ENABLE_ADB}" = "1" ]; then
        bbwarn "THING_REVIVED_ENABLE_ADB=1 but /etc/usb-debugging-enabled is not in the image"
    fi
}
