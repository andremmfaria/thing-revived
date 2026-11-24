FILESEXTRAPATHS:prepend := "${THISDIR}/${PN}:"

# Runtime adb/UART toggles (docs/research/debug-toggles-plan.md section 2 and 4.1). The upstream
# recipe (meta-superbird/recipes-core/superbird-usb-gadget/superbird-usb-gadget_0.1.bb) already
# installs android-tools-adbd.service.d/superbird.conf, which gates adbd on
# /etc/usb-debugging-enabled with ConditionPathExists and binds the UDC itself on start. This
# drop-in is named to sort AFTER that file (systemd applies .d/ drop-ins in filename order, so
# "zz-" wins) and:
#   - clears the inherited ConditionPathExists, so a manual or runtime `systemctl start
#     android-tools-adbd.service` always runs instead of silently no-op-ing when the build
#     default is off (docs/research/debug-toggles-spikes.md consequence 3).
#   - adds Conflicts=/After= against thing-revived-usb-bind.service, the mutually exclusive
#     counterpart (build/overlay/recipes-extended/thing-revived-usb-bind), so starting either side
#     stops the other and releases the UDC before the new side binds it.
#   - re-links ffs.adb as an idempotent ExecStartPre, because thing-revived-usb-bind's own script
#     removes that configfs link every time it binds the gadget NCM-only; adbd's upstream
#     ExecStartPost (same drop-in family) only binds the UDC, it does not restore this link.
SRC_URI += "file://zz-thing-revived-toggle.conf"

do_install:append() {
    install -d ${D}${systemd_system_unitdir}/android-tools-adbd.service.d
    install -m 0644 ${UNPACKDIR}/zz-thing-revived-toggle.conf \
        ${D}${systemd_system_unitdir}/android-tools-adbd.service.d/zz-thing-revived-toggle.conf
}

FILES:${PN} += "${systemd_system_unitdir}/android-tools-adbd.service.d/zz-thing-revived-toggle.conf"
