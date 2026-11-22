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

    if grep -rEq '^[^#]*nullok' ${D}${sysconfdir}/pam.d; then
        bbfatal "nullok is still present in ${D}${sysconfdir}/pam.d"
    fi
    if ! grep -Eq '^password[[:space:]].*pam_unix\.so.*rounds=${THING_REVIVED_SHA_ROUNDS}' ${D}${sysconfdir}/pam.d/common-password; then
        bbfatal "common-password does not carry rounds=${THING_REVIVED_SHA_ROUNDS}"
    fi
}
