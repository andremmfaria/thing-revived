IMAGE_OVERHEAD_FACTOR = "1.0"
IMAGE_ROOTFS_EXTRA_SPACE = "4096"

IMAGE_INSTALL:append = " \
    superbird-weston-init-kiosk \
    bash \
    thing-revived-terminal-agent \
    controller-agent \
    thing-revived-ntp-config \
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
