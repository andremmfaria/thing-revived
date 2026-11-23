# Remove nullok from the shipped PAM stack so an empty password hash can never
# authenticate (docs/research/user-management-plan.md, section 4).
#
# The recipe is libpam; the pam.d files are packaged in libpam-runtime. The
# upstream common-auth ships nullok_secure; oe-core's ssh_allow_empty_password
# rewrote it to nullok only while the allow-empty-password image feature was
# on, and the prod image bbappend removes that feature. Both spellings are
# stripped here as well, so the result does not depend on the feature list.
#
# No pam_wheel restriction on su: pam_wheel is not shipped, and su to root
# cannot succeed with a locked root anyway.

# pam_faillock (owner spec S8, delta plan section 7.2): deny=5 unlock_time=600
# fail_interval=900, inline options (no faillock.conf). The counter lives in tmpfs
# /var/run/faillock, so a reboot resets it. The plugin package is pam-plugin-faillock
# (added to IMAGE_INSTALL in the prod image bbappend). Line ordering against the shipped
# pam_unix line is validated on the first build and on the device.
THING_REVIVED_FAILLOCK_OPTS ?= "deny=5 unlock_time=600 fail_interval=900"

# Must equal [users] sha_rounds in config/controller.toml and
# THING_REVIVED_SHA_ROUNDS in the prod image bbappend.
THING_REVIVED_SHA_ROUNDS ?= "65536"

do_install:append() {
    for f in ${D}${sysconfdir}/pam.d/*; do
        [ -f "$f" ] || continue
        sed -i -E 's/[[:space:]]+nullok(_secure)?([[:space:]]|$)/\2/g' "$f"
    done

    # Idempotent: only add rounds= to the pam_unix line that has sha512.
    if ! grep -q 'rounds=' ${D}${sysconfdir}/pam.d/common-password; then
        sed -i -E '/^password[[:space:]].*pam_unix\.so/ s/[[:space:]]sha512/ sha512 rounds=${THING_REVIVED_SHA_ROUNDS}/' \
            ${D}${sysconfdir}/pam.d/common-password
    fi

    # Idempotent: skip when the lines are already there.
    if ! grep -q 'pam_faillock' ${D}${sysconfdir}/pam.d/common-auth; then
        sed -i -E \
            -e '/^auth[[:space:]].*pam_unix\.so/ i auth\trequired\t\t\tpam_faillock.so preauth ${THING_REVIVED_FAILLOCK_OPTS}' \
            -e '/^auth[[:space:]].*pam_unix\.so/ a auth\t[default=die]\t\t\tpam_faillock.so authfail ${THING_REVIVED_FAILLOCK_OPTS}\nauth\tsufficient\t\t\tpam_faillock.so authsucc ${THING_REVIVED_FAILLOCK_OPTS}' \
            ${D}${sysconfdir}/pam.d/common-auth
    fi
    if ! grep -q 'pam_faillock' ${D}${sysconfdir}/pam.d/common-account; then
        printf 'account\trequired\t\t\tpam_faillock.so\n' >> ${D}${sysconfdir}/pam.d/common-account
    fi

    if grep -rEq '^[^#]*nullok' ${D}${sysconfdir}/pam.d; then
        bbfatal "nullok is still present in ${D}${sysconfdir}/pam.d"
    fi
    if ! grep -Eq '^password[[:space:]].*pam_unix\.so.*rounds=${THING_REVIVED_SHA_ROUNDS}' ${D}${sysconfdir}/pam.d/common-password; then
        bbfatal "common-password does not carry rounds=${THING_REVIVED_SHA_ROUNDS}"
    fi
    for kw in preauth authfail authsucc; do
        if ! grep -Eq "^auth[[:space:]].*pam_faillock\.so $kw .*deny=5 .*unlock_time=600" ${D}${sysconfdir}/pam.d/common-auth; then
            bbfatal "common-auth is missing the pam_faillock $kw line with deny=5 unlock_time=600"
        fi
    done
    if ! grep -Eq '^account[[:space:]]+required[[:space:]]+pam_faillock\.so' ${D}${sysconfdir}/pam.d/common-account; then
        bbfatal "common-account is missing pam_faillock.so"
    fi
}
