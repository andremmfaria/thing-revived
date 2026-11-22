# D-Link DWA-171 rev B1 (MT7610U) needs only mediatek/mt7610u.bin (80 KB). The stock
# linux-firmware-mediatek package ships all of firmware/mediatek/* (about 62 MB installed), far too
# much for the squashfs rootfs. Carve out a one-file package; PACKAGES =+ puts it ahead of
# ${PN}-mediatek so it claims the file first.
PACKAGES =+ "${PN}-mt7610u"
LICENSE:${PN}-mt7610u = "Firmware-mediatek"
FILES:${PN}-mt7610u = "${nonarch_base_libdir}/firmware/mediatek/mt7610u.bin*"
RDEPENDS:${PN}-mt7610u += "${PN}-mediatek-license"
