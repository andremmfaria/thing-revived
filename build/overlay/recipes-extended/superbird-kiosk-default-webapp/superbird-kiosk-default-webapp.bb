SUMMARY = "Default webapp for the kiosk example image: thing-revived terminal"
DESCRIPTION = "Replaces the stock placeholder with the thing-revived \
on-device terminal frontend (real xterm.js 5.3.0 vendored for proper \
ANSI/VT100 rendering; see docs/terminal.md upstream)."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"
SRC_URI = " \
    file://index.html \
    file://style.css \
    file://app.js \
    file://wifi-logic.js \
    file://vendor/xterm.js \
    file://vendor/xterm.css \
    file://vendor/xterm-addon-fit.js \
"
S = "${UNPACKDIR}"
inherit allarch
do_install() {
    install -d ${D}${datadir}/superbird-kiosk-default/vendor
    install -m 0644 ${S}/index.html ${D}${datadir}/superbird-kiosk-default/index.html
    install -m 0644 ${S}/style.css ${D}${datadir}/superbird-kiosk-default/style.css
    install -m 0644 ${S}/app.js ${D}${datadir}/superbird-kiosk-default/app.js
    install -m 0644 ${S}/wifi-logic.js ${D}${datadir}/superbird-kiosk-default/wifi-logic.js
    install -m 0644 ${S}/vendor/xterm.js ${D}${datadir}/superbird-kiosk-default/vendor/xterm.js
    install -m 0644 ${S}/vendor/xterm.css ${D}${datadir}/superbird-kiosk-default/vendor/xterm.css
    install -m 0644 ${S}/vendor/xterm-addon-fit.js ${D}${datadir}/superbird-kiosk-default/vendor/xterm-addon-fit.js
}
FILES:${PN} = "${datadir}/superbird-kiosk-default"
