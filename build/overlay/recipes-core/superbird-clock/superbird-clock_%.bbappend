# Upstream masks systemd-timesyncd here (it assumed no NTP path). thing-revived syncs over NTP
# with timesyncd (thing-revived-ntp-config), so drop the /dev/null mask. superbird-clock itself
# stays: it still bumps the clock forward to the saved/build floor very early at boot.
do_install:append() {
    rm -f ${D}${sysconfdir}/systemd/system/systemd-timesyncd.service
}

FILES:${PN}:remove = "${sysconfdir}/systemd/system/systemd-timesyncd.service"
