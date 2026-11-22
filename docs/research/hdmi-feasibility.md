# HDMI output feasibility on the Car Thing (superbird, S905D2)

Researched 2026-11-22. Kernel 7.0.2-superbird, meta-superbird from yocto-superbird. Tags: V = page or file read this session, S = search snippet only, U = unverified or inference. Everything was read-only: ssh devel for the kernel tree and dts, ssh root@192.168.20.202 for dmesg and sysfs reads.

## Summary

Native HDMI or DisplayPort alt mode out of the Car Thing is almost certainly not possible, and nothing found suggests otherwise. The S905D2 has an HDMI TX block and mainline describes it, but no source shows it wired to any pad, connector or the USB-C port. The board has one USB-C receptacle, no Type-C/PD controller in any device tree or on the live device, and a USB2-only data path. Flipping `hdmi_tx` to okay in the dts is possible without hardware, but it would only create a DRM HDMI connector that never reports a sink. The only realistic route to an external picture is a USB display adapter (DisplayLink class), which needs out-of-tree or proprietary pieces, is unproven on this board, and the dock's HDMI port is not shown to be USB-based at all. Recommended conclusion: infeasible natively, feasible only via USB display with low confidence and high effort.

## Q1. Is HDMI TX or DP alt physically exposed?

- No source found that shows HDMI TX routed anywhere on the board. Absence of evidence, not proof of absence (U).
- The EDN teardown (2023-04-18) lists the main-board parts it identified: an Amlogic S905-family SoC, an Etron EM6HE16EWAKG-10 DDR3L, a Toshiba THGBMNG5D1LBAIL 4 GB eMMC and a Cypress CYW20706 Bluetooth SoC. It mentions a USB-C power input and no HDMI connector, PD chip or DP mux. The article is text I read, but I did not view the photos, so unlabelled chips are not ruled out (V for the text, U for the board). URL: https://www.edn.com/spotifys-car-thing-why-does-it-exist-is-the-crux-of-the-questioning.
- Stock Spotify device tree (spsgsb/kernel-common, branch buildroot-openlinux-201904-g12a, `superbird_evt_512.dts`, 1489 lines) contains no Type-C, TCPC, PD, FUSB or DP node. Its enabled I2C buses carry only the tlsc6x touch controller (i2c0), a LIS2DH12 accelerometer and ALS/proximity (i2c2) and the MFi coprocessor (i2c3). Grep for `hdmi`, `typec`, `tcpc` in that board dts found no hdmi node there (V). The vendor SoC dtsi `mesong12a.dtsi` does define an `amhdmitx` node with status okay (V), but that is the generic SoC file shared by Amlogic boards. It shows the block exists, not that the pins are connected. Whether the board dtsi overrides it was not checked (U).
- Stock USB: `dwc3` okay, `usb2_phy_v2` okay with portnum 2, `usb3_phy_v2` status disabled with `gpio-vbus-power = GPIOH_6`, `dwc2_a` okay with `controller-type = <3>` (V). The USB3 PHY is disabled in the vendor tree, so the SuperSpeed pairs that DP alt mode would need are not in use there. Whether the PCB even wires them to the connector is unverified (U).
- The FCC filing for the device (2AP3D-YX5H6679, Spotify USA, Bluetooth accessory, grant 2021-01-14) lists internal photo exhibits, block diagram and schematics, but the schematics, block diagram and parts list are marked metadata only (confidential, not downloadable). I did not open the internal photo PDFs (V for the listing, U for contents). https://fccid.io/2AP3D-YX5H6679
- frederic/superbird-bulkcmd (V) documents UART pads and U-Boot access, and says Qt EGLFS is used by the stock UI. It mentions no video output. bishopdynamics/superbird-tool (V) and alexcaoys/notes-superbird (V, support matrix lists MIPI Display, USB device and host, no HDMI row) likewise say nothing about HDMI.
- Not found: any teardown, forum, Hackaday, Thing Labs wiki or Discord statement that HDMI or DP is available. Discord is not searchable from here (U). Reddit posts (r/carthinghax) could not be fetched (S only: snippet says a USB-C dongle can give power, data and aux in, which is not a video claim).

## Q2. Can the USB-C port do DP alt mode?

- No Type-C controller is visible in any source. The live device has no `/sys/class/typec` (V, ssh root@192.168.20.202). Our kernel config has `# CONFIG_TYPEC is not set` and `CONFIG_USB_ROLE_SWITCH=y` (V, build .config). The role switch comes from the meson/dwc2 glue, selecting host or device in software, not from a CC controller.
- Our dts comment says the port is also the board's power feed, with VBUS taken from the upstream host (V, meson-g12a-superbird.dts). That fits a passive receptacle with no PD negotiation. Confirmed by the owner's earlier observation that the unit is powered through the dock, with the PD wattage not characterised (V, docs/hardware.md).
- The disable cfg comment says "no USB-C connector on this board" (V, superbird-disable.cfg). That wording is imprecise: there is a USB-C receptacle, but no Type-C controller. Consider rewording it.
- DP alt mode needs a source with a DP transmitter, SuperSpeed lanes on the connector and a CC/mux controller to negotiate it. Plugable KB states both host port and controller must support Alt Mode (S). The S905D2 HDMI TX outputs TMDS, so even with lanes you would need a TMDS-to-DP converter chip. No such chip appears in the teardown text (V text, U board). An adapter cannot add DP alt mode to a port that lacks it (S, ktcplay). Conclusion: DP alt mode is not available, confidence high on "no controller", medium on "no routed lanes".

## Q3. Video via a USB display path

- Mainline `udl` (DisplayLink) exists in the tree but `# CONFIG_DRM_UDL is not set` in our config (V). Its USB id table matches vendor 0x17e9, class 0xff, subclass 0, protocol 0, and the in-source comment says this is compatible with "USB 2.0 era graphics chips" (V, drivers/gpu/drm/udl/udl_drv.c). Newer DisplayLink chips (USB 3 docks) use a different protocol and are out of scope of mainline udl (U, inference from that comment, not tested).
- Newer DisplayLink docks need DisplayLink's `evdi` out-of-tree DRM module plus the proprietary DisplayLink userspace driver from Synaptics. The evdi README says the module is GPL v2 and the library LGPL v2.1 (V, https://github.com/DisplayLink/evdi). Synaptics distributes the userspace driver for Ubuntu releases (V, https://www.synaptics.com/products/displaylink-graphics/downloads/ubuntu, release 6.4). I could not confirm an aarch64 build or Yocto packaging, and the binary is closed (U).
- `CONFIG_DRM_GUD` is not set (V). GUD is a host-side driver for displays that run a GUD gadget. It does not help drive a dock's HDMI port.
- The dock's HDMI path is unverified. Existing research says the LMP USB-C mini dock chipsets could not be identified, and retailer pages disagree on HDMI 1.4 at 4K30 versus 4K60 (V, docs/research/lmp-mini-dock-chipsets.md). A DP-alt-mode HDMI converter is typical for such docks (U), which would not appear as a USB device at all. Part number note: that doc uses P/N 14368 and cites 15954 only via a snippet. The owner's number 15954 may be a different SKU, so check with lsusb on the real dock (U).
- Practical limits (U, inference): the port is USB 2.0 high speed only (dwc2, 480 Mbps), RAM is 512 MiB, and host mode is a runtime opt-in behind a powered hub (V, dts comment). A DisplayLink-class path would need: `CONFIG_DRM_UDL` or evdi out-of-tree for the kernel, the DisplayLink userspace daemon cross-built or repackaged, and a compositor that can drive a second DRM device. The stock UI runs Qt EGLFS (V, superbird-bulkcmd README), which targets one KMS device.

## Q4. What "HDMI transceiver device is disabled" means

- Source (V, kernel tree on devel, drivers/gpu/drm/meson/meson_encoder_hdmi.c): `meson_encoder_hdmi_probe()` calls `of_graph_get_remote_node(priv->dev->of_node, 1, 0)`. If it returns NULL it logs `dev_err(... "HDMI transceiver device is disabled")` and returns 0, so the probe continues without an HDMI encoder. It is an error-level log but a benign, intended path.
- Why it returns NULL (V): `of_graph_get_remote_node()` in drivers/of/property.c returns NULL when the remote node fails `of_device_is_available()`. The vpu `port@1` endpoint `hdmi_tx_out` points at `hdmi_tx_in` on the `hdmi_tx` node, and `meson-g12-common.dtsi` sets `status = "disabled"` on `hdmi_tx` (V, line in the hdmi-tx@0 node). Our board dts does not override it (V, grep found no hdmi_tx reference in meson-g12a-superbird.dts).
- Layer: the SoC common dtsi sets disabled; the board dts is the layer that would enable it. Nothing in the image config prevents it: `CONFIG_DRM_MESON_DW_HDMI=y` is already set (V, superbird.cfg line 37).
- What enabling needs, modelled on `meson-g12a-sei510.dts` (V): `&hdmi_tx { status = "okay"; pinctrl-0 = <&hdmitx_hpd_pins>, <&hdmitx_ddc_pins>; pinctrl-names = "default"; hdmi-supply = <&regulator>; };`, an `hdmi-connector` node (compatible "hdmi-connector", type "a") with a port linked to `hdmi_tx_tmds_out`, and an `&hdmi_tx_tmds_port` endpoint. The Radxa Zero patch shows the same shape (S, lkml). The driver takes an optional "hdmi" regulator (V, meson_dw_hdmi.c line 699), and HPD and DDC pins are defined in the common dtsi (V, hdmitx_ddc_pins, hdmitx_hpd_pins).
- Could it be flipped without hardware? Yes, the dtb will accept it, and a DRM HDMI connector would appear. It would report disconnected because no sink exists, and nothing else changes (U, expected not tested). Risks (U): the stock clock notes in our dts say the DSI path already uses the HDMI-PLL-sourced `vid_pll` (V), so a dw-hdmi probe that touches HDMI PLL, resets or `CLKID_HDMI` could disturb the working DSI panel. The VPU register window is also shrunk to 256 KiB for GE2D (V, dts), and the HDMI encoder path may use ranges outside it (U). That makes a blind flip a regression risk for a feature with no output.
- The panfrost line is unrelated to HDMI: the dts deliberately leaves `mali-supply` unset because the board has no separately controllable GPU rail, and says panfrost still binds (V, dts comment). dmesg on the device shows panfrost initialised (V, ssh). It is benign.
- Live device dmesg confirms: `meson-drm ff900000.vpu: HDMI transceiver device is disabled` (twice, probe runs twice), `CVBS Output connector not available`, DSI-1 as the only DRM connector (`/sys/class/drm/card1-DSI-1`, V).

## Q5. Conclusion and what to tell the owner

| Route | Verdict | Effort | Risk |
|---|---|---|---|
| Native HDMI TX from the SoC | Infeasible. No evidence of routing to any connector, and no hardware access path. | Not applicable | Soldering to unidentified pads would risk the board. Not recommended. |
| DP alt mode over USB-C | Infeasible. No Type-C/PD controller, USB2-only port, no TMDS to DP converter seen. | Not applicable | None for us, nothing to build. |
| Enable `hdmi_tx` in dts | Possible but pointless: gives a never-connected connector. | About 1 hour for the dts, plus a boot test | May disturb the DSI clock tree. Not advised without a reason. |
| USB display (DisplayLink class) | Feasible only if the owner has a real DisplayLink adapter or a dock whose HDMI is DisplayLink. Unproven here. | Days to weeks: kernel modules, evdi or proprietary userspace on aarch64, compositor and UI work | High: closed binary, 512 MiB RAM, USB2 bandwidth, a second DRM device in a single-KMS UI stack. |

What to tell the owner: the Car Thing's own screen is the only video output the board supports, and the HDMI port on the dock cannot be used because the Car Thing does not output video over USB-C. The boot messages are harmless: the HDMI one just means the HDMI block is turned off in the device tree, and the Mali one means the GPU has no voltage regulator to control. If they still want external video, the first step is to run `lsusb -v` on the dock (see the existing dock research) and check for vendor 17e9 (DisplayLink). If it is absent, the answer is final.

## Confidence

- High: the meaning of the message and the dts layers (read from source on devel); no Type-C node or driver on the board or live device; udl and GUD not built.
- Medium: no HDMI/DP routing, based on the stock dts, the teardown text and project docs all being silent. This is not a schematic-level confirmation.
- Could not verify: PCB photos or schematics (FCC schematics are confidential, I did not open internal photos); whether USB3 lanes are wired to the connector; Discord or Reddit discussion; the dock's HDMI chip; aarch64 availability of DisplayLink userspace; actual effect of enabling `hdmi_tx` on the DSI clocks (not tested, device unchanged).

## Sources

- https://www.edn.com/spotifys-car-thing-why-does-it-exist-is-the-crux-of-the-questioning : teardown chip list (V).
- https://fccid.io/2AP3D-YX5H6679 : FCC exhibits list (V).
- https://github.com/frederic/superbird-bulkcmd : UART and boot notes (V).
- https://github.com/bishopdynamics/superbird-tool : boot modes (V).
- https://github.com/alexcaoys/notes-superbird : support matrix (V), raw BUILDING.md read.
- https://github.com/spsgsb/kernel-common/blob/buildroot-openlinux-201904-g12a/arch/arm64/boot/dts/amlogic/superbird_evt_512.dts : stock board dts (V).
- https://github.com/spsgsb/kernel-common/blob/buildroot-openlinux-201904-g12a/arch/arm64/boot/dts/amlogic/mesong12a.dtsi : vendor SoC dtsi with amhdmitx (V).
- https://github.com/DisplayLink/evdi and https://www.synaptics.com/products/displaylink-graphics/downloads/ubuntu : evdi licensing and Ubuntu driver (V).
- https://kb.plugable.com/understanding-usb-c-alt-mode and https://us.ktcplay.com/blogs/technology-hub/what-is-displayport-alternate-mode-usb-c : alt mode requirements (S).
- https://lkml.iu.edu/hypermail/linux/kernel/2109.1/01293.html : Radxa Zero hdmi_tx enable pattern (S).
- Local, read-only: ~/workspace/yocto-superbird-build on devel (meson-g12a-superbird.dts, superbird.cfg, superbird-disable.cfg, kernel-source meson-g12-common.dtsi, meson-g12a-sei510.dts, meson_encoder_hdmi.c, meson_dw_hdmi.c, of/property.c, udl_drv.c, build .config); device 192.168.20.202 dmesg and /sys/class/drm, /sys/class/typec; docs/hardware.md; docs/research/lmp-mini-dock-chipsets.md.
