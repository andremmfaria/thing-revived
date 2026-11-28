# Drop the two large font packages that the upstream chromium-kiosk recipe pulls
# in through RDEPENDS. noto-sans-cjk is about 31 MiB and noto-color-emoji about
# 5 MiB, so roughly 36 MiB comes off the rootfs. The frontend only uses Latin
# glyphs plus a few symbols (section sign, e-acute, bullet, arrow, middle dot),
# which liberation-fonts still covers. Side effect: Wi-Fi SSIDs, Bluetooth names
# and terminal output containing CJK or emoji characters render as tofu.
# The example layer ships its own chromium-kiosk_%.bbappend; both apply.
RDEPENDS:${PN}:remove = "noto-color-emoji noto-sans-cjk"
