IMAGE_OVERHEAD_FACTOR = "1.0"
IMAGE_ROOTFS_EXTRA_SPACE = "4096"

IMAGE_INSTALL:append = " \
    superbird-weston-init-kiosk \
    bash \
    openssh-ssh \
    openssh-keygen \
    thing-revived-terminal-agent \
    controller-agent \
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
