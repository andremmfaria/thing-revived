FILESEXTRAPATHS:prepend := "${THISDIR}/${PN}:"

# thing-revived-wifi.cfg re-enables WLAN/CFG80211/MAC80211 after superbird-disable.cfg turns them
# off. linux-superbird_7.0.2.bb merges its three named fragments first and then every other *.cfg in
# UNPACKDIR, so both of these are merged after the disable fragment and win. The merge was verified with
# scripts/kconfig/merge_config.sh + olddefconfig against the real fragment set (see docs/yocto-build-and-flash.md).
SRC_URI += "file://thing-revived-uhid.cfg file://thing-revived-wifi.cfg"
