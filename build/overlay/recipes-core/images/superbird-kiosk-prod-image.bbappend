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
