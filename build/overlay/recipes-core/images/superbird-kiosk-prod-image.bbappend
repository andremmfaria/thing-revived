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
    thing-revived-device-apply \
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
# the value 1 enables; the post-commands below treat anything else as off. Since the
# device settings rework (docs/research/device-settings-plan.md section 4.3) they no longer
# touch any unit: they are only the first-boot and per-image seed of the persisted adb and
# uart settings (/usr/share/thing-revived/device-seed.json).
THING_REVIVED_ENABLE_ADB ?= "0"
THING_REVIVED_ENABLE_UART ?= "0"

# Identity of this image build, passed in by build-image.sh next to the two flags
# (<yyyymmddThhmmssZ>-<8 hex of the git revision, or nogit>). It is baked into the seed file;
# the device re-seeds adb and uart when it differs from the id stored in device-settings.json.
# Empty is a build error (thing_revived_device_seed), so a bare kas run without the variable
# cannot produce an image with a blind reseed rule.
THING_REVIVED_IMAGE_ID ?= ""

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
ROOTFS_POSTPROCESS_COMMAND += "thing_revived_device_seed "
ROOTFS_POSTPROCESS_COMMAND += "thing_revived_login_defs "
ROOTFS_POSTPROCESS_COMMAND += "thing_revived_pam "
ROOTFS_POSTPROCESS_COMMAND += "thing_revived_assert_accounts "

# UART root console (docs/research/device-settings-plan.md sections 4.4 and 7.1).
# thing-revived-uart-root@.service (build/overlay/recipes-extended/thing-revived-uart-root)
# always ships and is DISABLED: nothing is symlinked into getty.target.wants. Whether the root
# console runs is decided at boot by thing-revived-device-apply.service from the persisted UART
# setting, and live by the controller. The stock serial-getty@ttyAML0 is left exactly as
# upstream has it: the getty generator starts it on the kernel console tty by default (spike
# P0.9), which is the desired "UART off" state (normal login, root locked), and uart-root takes
# over through its Conflicts=serial-getty@%i.service. The stock getty must NEVER be masked or
# otherwise removed here, because unit B and the controller must be able to start it.
#
# The unit's ExecStart ships as a @@THING_REVIVED_STOCK_GETTY_ARGS@@ placeholder and is filled in
# here, unconditionally, by copying the stock serial-getty@.service's own ExecStart line verbatim
# (plus --autologin root) from this SAME rootfs, so baud/TERM always match. The shared
# serial-getty@.service template itself is never edited and stays autologin-free in every build.
#
# /etc/securetty (owned by shadow-securetty, left as shipped, listing ttyAML0) is asserted:
# login(1), which agetty --autologin still execs, refuses root on a tty absent from securetty
# regardless of the hash, so the UART setting could never work without it.
thing_revived_console_gate () {
    tty="ttyAML0"
    etc="${IMAGE_ROOTFS}${sysconfdir}"
    unitdir="${IMAGE_ROOTFS}${systemd_system_unitdir}"
    serial_unit="$unitdir/serial-getty@.service"
    uart_unit="$unitdir/thing-revived-uart-root@.service"

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
        || bbfatal "/etc/securetty does not list $tty; the UART setting needs root login permitted there on every build"
    if [ "${THING_REVIVED_ENABLE_UART}" = "1" ]; then
        bbwarn "THING_REVIVED_ENABLE_UART=1: seeds the root console on at first boot and on every image change, logging in as root without a password"
    fi
}

# Device settings seed (docs/research/device-settings-plan.md sections 4.3 and 7.1). Replaces the
# old adb/usb-bind enablement gate. Both adbd and usb-bind ship DISABLED on every build (no wants
# symlink anywhere; thing-revived-device-apply.service starts exactly one of them at boot from
# the persisted settings, and either side can be started live because Conflicts= is set on both
# sides: the zz-thing-revived-toggle.conf drop-in on adbd, the unit file itself on usb-bind).
# The flags only become the seed /usr/share/thing-revived/device-seed.json (read-only rootfs, so
# it cannot drift), together with THING_REVIVED_IMAGE_ID, which makes a reflash re-seed adb and
# uart once per image on a device whose /var survived.
#
# The legacy marker /etc/usb-debugging-enabled has no producer or reader left (installed by the
# upstream superbird-usb-gadget recipe, read by adbd-superbird.conf whose Condition the zz drop-in
# clears, and read by the old usb-role drop-in), so it is removed on EVERY build, not only on
# flag 0 builds.
thing_revived_device_seed () {
    etc="${IMAGE_ROOTFS}${sysconfdir}"
    unitdir="${IMAGE_ROOTFS}${systemd_system_unitdir}"
    adb_dropin="$unitdir/android-tools-adbd.service.d/zz-thing-revived-toggle.conf"
    adb_unit="$unitdir/android-tools-adbd.service"
    bind_unit="$unitdir/thing-revived-usb-bind.service"
    seed_dir="${IMAGE_ROOTFS}${datadir}/thing-revived"
    seed="$seed_dir/device-seed.json"

    [ -f "$adb_unit" ] || bbfatal "android-tools-adbd.service is missing from the rootfs"
    [ -f "$bind_unit" ] || bbfatal "thing-revived-usb-bind.service is missing from the rootfs"
    [ -f "$adb_dropin" ] || bbfatal "android-tools-adbd.service.d/zz-thing-revived-toggle.conf is missing from the rootfs"
    grep -Eq '^Conflicts=[[:space:]]*thing-revived-usb-bind\.service[[:space:]]*$' "$adb_dropin" \
        || bbfatal "$adb_dropin has no Conflicts=thing-revived-usb-bind.service"
    grep -Eq '^Conflicts=[[:space:]]*android-tools-adbd\.service[[:space:]]*$' "$bind_unit" \
        || bbfatal "thing-revived-usb-bind.service has no Conflicts=android-tools-adbd.service"

    if [ -z "${THING_REVIVED_IMAGE_ID}" ]; then
        bbfatal "THING_REVIVED_IMAGE_ID is empty; build with build/build-image.sh, which passes it, or set it in the kas flags file"
    fi
    printf '%s\n' "${THING_REVIVED_IMAGE_ID}" | grep -Eq '^[0-9]{8}T[0-9]{6}Z-([0-9a-f]{8}|nogit)$' \
        || bbfatal "THING_REVIVED_IMAGE_ID '${THING_REVIVED_IMAGE_ID}' does not match <yyyymmddThhmmssZ>-<8 hex or nogit>"

    # Disabled means no wants symlink anywhere: the generated wants dirs under /etc and the
    # vendor ones under the unit directory.
    find "$etc/systemd/system" "$unitdir" -type l \( -path '*.wants/*' -o -path '*.requires/*' \) \
        \( -name 'android-tools-adbd.service' -o -name 'thing-revived-usb-bind.service' \
           -o -name 'thing-revived-uart-root@*.service' \) -delete

    adb=false
    uart=false
    if [ "${THING_REVIVED_ENABLE_ADB}" = "1" ]; then
        bbwarn "THING_REVIVED_ENABLE_ADB=1: seeds adb on at first boot and on every image change; adbd gives an unauthenticated root shell over USB"
        adb=true
    fi
    [ "${THING_REVIVED_ENABLE_UART}" = "1" ] && uart=true

    mkdir -p "$seed_dir"
    printf '{"version":1,"image_id":"%s","adb":%s,"uart":%s}\n' "${THING_REVIVED_IMAGE_ID}" "$adb" "$uart" > "$seed"
    chmod 0644 "$seed"

    rm -f "$etc/usb-debugging-enabled"
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

    # Device settings (plan section 7.2, build side of check_image_accounts.py). Nothing here may
    # depend on the flags except the seed file content: no unit is enabled any more.
    unitdir="${IMAGE_ROOTFS}${systemd_system_unitdir}"
    serial_unit="$unitdir/serial-getty@.service"
    uart_unit="$unitdir/thing-revived-uart-root@.service"
    if [ -f "$serial_unit" ] && grep -Eq '^[[:space:]]*ExecStart=.*--autologin' "$serial_unit"; then
        bbfatal "serial-getty@.service has an autologin ExecStart; the shared template must stay untouched in every build"
    fi
    [ -f "$uart_unit" ] || bbfatal "thing-revived-uart-root@.service is missing from the rootfs; it must ship on every build"
    if grep -q '@@THING_REVIVED_STOCK_GETTY_ARGS@@' "$uart_unit" 2>/dev/null; then
        bbfatal "thing-revived-uart-root@.service still has the unsubstituted ExecStart placeholder"
    fi
    grep -Eq '^Conflicts=[[:space:]]*serial-getty@%i\.service[[:space:]]*$' "$uart_unit" \
        || bbfatal "thing-revived-uart-root@.service has no Conflicts=serial-getty@%i.service"
    grep -qx "ttyAML0" "$etc/securetty" 2>/dev/null \
        || bbfatal "/etc/securetty does not list ttyAML0; the UART setting needs root login permitted there on every build"

    # Both units of each pair ship, carry their Conflicts= halves, and none of them is enabled.
    # The stock serial-getty@ttyAML0 link is NOT in that list: upstream (meta-superbird) ships it
    # and it is left as is (P0.9).
    adb_unit="$unitdir/android-tools-adbd.service"
    adb_dropin="$unitdir/android-tools-adbd.service.d/zz-thing-revived-toggle.conf"
    bind_unit="$unitdir/thing-revived-usb-bind.service"
    [ -f "$adb_unit" ] || bbfatal "android-tools-adbd.service is missing from the rootfs; it must ship on every build"
    [ -f "$bind_unit" ] || bbfatal "thing-revived-usb-bind.service is missing from the rootfs; it must ship on every build"
    [ -f "$adb_dropin" ] || bbfatal "android-tools-adbd.service.d/zz-thing-revived-toggle.conf is missing from the rootfs"
    grep -Eq '^Conflicts=[[:space:]]*thing-revived-usb-bind\.service[[:space:]]*$' "$adb_dropin" \
        || bbfatal "$adb_dropin has no Conflicts=thing-revived-usb-bind.service"
    grep -Eq '^Conflicts=[[:space:]]*android-tools-adbd\.service[[:space:]]*$' "$bind_unit" \
        || bbfatal "thing-revived-usb-bind.service has no Conflicts=android-tools-adbd.service"
    enabled=$(find "$etc/systemd/system" "$unitdir" \( -path '*.wants/*' -o -path '*.requires/*' \) \
        \( -name 'android-tools-adbd.service' -o -name 'thing-revived-usb-bind.service' \
           -o -name 'thing-revived-uart-root@*.service' \
           -o -name 'sshd.service' \) 2>/dev/null)
    [ -z "$enabled" ] || bbfatal "units that must ship disabled are enabled: $enabled"

    # Seed file: exactly what thing_revived_device_seed writes, so adb and uart equal the flags.
    seed="${IMAGE_ROOTFS}${datadir}/thing-revived/device-seed.json"
    [ -f "$seed" ] || bbfatal "$seed is missing from the rootfs"
    expect_adb=false
    expect_uart=false
    [ "${THING_REVIVED_ENABLE_ADB}" = "1" ] && expect_adb=true
    [ "${THING_REVIVED_ENABLE_UART}" = "1" ] && expect_uart=true
    expected_seed=$(printf '{"version":1,"image_id":"%s","adb":%s,"uart":%s}' "${THING_REVIVED_IMAGE_ID}" "$expect_adb" "$expect_uart")
    [ "$(cat "$seed")" = "$expected_seed" ] || bbfatal "$seed is not the expected seed $expected_seed"
    [ ! -e "$etc/usb-debugging-enabled" ] || bbfatal "/etc/usb-debugging-enabled exists; the marker has no producer any more and must be removed on every build"
    [ ! -e "${IMAGE_ROOTFS}/var/lib/thing-revived/device-settings.json" ] \
        || bbfatal "device-settings.json is baked into the rootfs; the persisted file is created at first boot"

    # Boot apply units: A enabled on sysinit, B enabled on multi-user, F present, ssh gate in place,
    # sshd.socket enabled and no plain sshd.service enabled.
    for u in thing-revived-device-settings thing-revived-device-apply thing-revived-device-failsafe; do
        [ -f "$unitdir/$u.service" ] || bbfatal "$u.service is missing from the rootfs (is thing-revived-device-apply in IMAGE_INSTALL?)"
    done
    [ -L "$etc/systemd/system/sysinit.target.wants/thing-revived-device-settings.service" ] \
        || bbfatal "thing-revived-device-settings.service is not enabled in sysinit.target.wants"
    [ -L "$etc/systemd/system/multi-user.target.wants/thing-revived-device-apply.service" ] \
        || bbfatal "thing-revived-device-apply.service is not enabled in multi-user.target.wants"
    [ -f "$unitdir/sshd.socket.d/20-thing-revived-ssh-gate.conf" ] \
        || bbfatal "sshd.socket.d/20-thing-revived-ssh-gate.conf is missing from the rootfs"
    sshd_socket_wants=$(find "$etc/systemd/system" "$unitdir" -path '*.wants/sshd.socket' 2>/dev/null)
    [ -n "$sshd_socket_wants" ] || bbfatal "sshd.socket is not enabled; it must stay enabled (the ssh gate drop-in decides at boot)"
}
