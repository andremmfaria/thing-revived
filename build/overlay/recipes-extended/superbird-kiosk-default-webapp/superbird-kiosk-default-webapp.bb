SUMMARY = "Default webapp for the kiosk example image: thing-revived terminal"
DESCRIPTION = "Replaces the stock placeholder with the thing-revived \
on-device terminal frontend (real xterm.js 5.3.0 vendored for proper \
ANSI/VT100 rendering; see docs/terminal.md upstream). build/build-image.sh \
rsyncs frontend/ (minus tests/ and node_modules/) into files/webapp and \
writes files/webapp/config.generated.js from config/frontend.toml; this \
recipe installs that whole tree recursively."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

# A directory in SRC_URI is fetched recursively into ${UNPACKDIR}/webapp.
SRC_URI = "file://webapp"
S = "${UNPACKDIR}"

inherit allarch

do_compile[noexec] = "1"
do_configure[noexec] = "1"

do_install() {
    install -d ${D}${datadir}/superbird-kiosk-default
    cp -r --no-preserve=ownership ${S}/webapp/. ${D}${datadir}/superbird-kiosk-default/
    # Fail the build rather than ship a kiosk without its generated config.
    test -f ${D}${datadir}/superbird-kiosk-default/config.generated.js
    find ${D}${datadir}/superbird-kiosk-default -type d -exec chmod 0755 {} +
    find ${D}${datadir}/superbird-kiosk-default -type f -exec chmod 0644 {} +
}

FILES:${PN} = "${datadir}/superbird-kiosk-default"
