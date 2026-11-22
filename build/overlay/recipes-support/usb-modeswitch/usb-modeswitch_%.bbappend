# The usb_modeswitch binary only needs libusb1. The tcl dependency exists solely for the
# udev dispatcher script (/lib/udev/usb_modeswitch), which thing-revived-wifi-udev does not use
# (its rule calls the binary directly). Dropping tcl keeps a large interpreter off the squashfs rootfs.
RDEPENDS:${PN}:remove = "tcl"
# usb-modeswitch-data ships 40-usb_modeswitch.rules, which hands every known device to the tcl
# dispatcher. Without tcl that would only log failures, and the 0bda:1a2b case is handled by
# thing-revived-wifi-udev instead.
RRECOMMENDS:${PN}:remove = "usb-modeswitch-data"
# The dispatcher script (usb_modeswitch_dispatcher) still ships and needs tclsh, which QA flags as
# file-rdeps. Nothing on this image calls it, so skip the check instead of pulling in tcl.
INSANE_SKIP:${PN} += "file-rdeps"
