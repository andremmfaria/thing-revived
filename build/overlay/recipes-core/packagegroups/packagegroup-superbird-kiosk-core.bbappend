# The placeholder superbird-kiosk-daemon (a hello-world heartbeat writer) needs the bandaid
# partition mounted at /var/lib/bandaid for its writable /opt overlay. The image has no bandaid
# partition (docs/research/bandaid-removal.md, row 14), so the daemon is dropped from the kiosk
# core group. Only this packagegroup references the daemon in the prod image.
RDEPENDS:${PN}:remove = "superbird-kiosk-daemon"
