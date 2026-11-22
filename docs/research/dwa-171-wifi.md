# D-Link DWA-171 Wi-Fi on Superbird (kernel 7.0.2-superbird)

Research date 2026-11-22. Read-only checks on build host `devel`, checkout `~/workspace/yocto-superbird-build`. Nothing was edited, built or flashed.

## Summary

The DWA-171 is three different chips depending on hardware revision, so "which driver" depends on the label under the dongle. All three are covered by in-tree mainline drivers in 7.0.2, with firmware in linux-firmware 20260410. The current image config builds none of it: `meta-superbird/recipes-kernel/linux/linux-superbird/superbird-disable.cfg` explicitly disables `CONFIG_WLAN`, `CONFIG_CFG80211` and `CONFIG_MAC80211`, and the distro removes the `wifi` DISTRO_FEATURE. The task premise was slightly off: B1 is not an RTL8821CU. It is a MediaTek MT7610U. The RTL8821CU is rev C1.

| Rev | Chip | USB ID (Wi-Fi mode) | In-tree driver (module) | Kconfig symbol | Firmware file | linux-firmware Yocto package |
|---|---|---|---|---|---|---|
| A1 | Realtek RTL8811AU (8821A family) | 2001:3314 | rtw88_8821au | `CONFIG_RTW88_8821AU` | `rtw88/rtw8821a_fw.bin` | `linux-firmware-rtl8821` |
| B1 | MediaTek MT7610U | 2001:3d02 | mt76x0u | `CONFIG_MT76x0U` | `mediatek/mt7610u.bin` | `linux-firmware-mediatek` |
| C1 | Realtek RTL8821CU | 2001:331d (after modeswitch) | rtw88_8821cu | `CONFIG_RTW88_8821CU` | `rtw88/rtw8821c_fw.bin` | `linux-firmware-rtl8821` |

The user's guess of 2001:331e is wrong for this product. In the 7.0.2 tree 2001:331e is `rtw8822bu` labelled DWA-181 (`rtw88/rtw8822bu.c:40`), a different chip (RTL8822BU).

## Revision to chip to ID evidence

### A1: RTL8811AU, 2001:3314

- WikiDevi rev A1 page: Realtek RTL8811AU, ID 2001:3314, FCC ID KA2WA171A1, Cameo-made, FCC date 2013-04-30. https://wikidevi.wi-cat.ru/D-Link_DWA-171_rev_A1
- Kernel: `drivers/net/wireless/realtek/rtw88/rtw8821au.c:40` has `USB_DEVICE_AND_INTERFACE_INFO(0x2001, 0x3314, ...) /* D-Link */` bound to `rtw8821a_hw_spec`. (Same file also lists 0x3318.)
- Kconfig `RTW88_8821AU` is titled "Realtek 8821AU/8811AU USB wireless network adapter" (`rtw88/Kconfig:206`).
- Caveat: a deviwiki search snippet for the C1 page also shows "ID: 2001:3314 (1 addl. devices)". I could not fetch that page (fetch error). If a C1 unit ever enumerates as 3314 it would wrongly bind rtw88_8821au. Unresolved, check `lsusb` on the real unit.

### B1: MediaTek MT7610U, 2001:3d02

- Kernel: `drivers/net/wireless/mediatek/mt76/mt76x0/usb.c:26` has `{ USB_DEVICE(0x2001, 0x3d02) }, /* D-LINK DWA-171 rev B1 */`. This is the strongest primary evidence in the whole report, it names the model and revision.
- WikiDevi rev B1 listing: chip brand MediaTek, ID 2001:3d02 (search snippet and the A1 page's revision table).
- Firmware: `MT7610U_FIRMWARE "mediatek/mt7610u.bin"` (`mt76x0/mt76x0.h:26`, `usb.c:344`). Exists upstream: WHENCE "Driver: mt76x0" lists `mediatek/mt7610u.bin` (https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain/WHENCE, line 6623).

### C1: RTL8821CU, 2001:331d

- Fedora forum log of a user with a C1 unit: `idVendor=2001, idProduct=331d`, Product "DWA-171", after a USB disconnect/re-enumeration from mass-storage mode. https://discussion.fedoraproject.org/t/using-a-dwa-171-wifi-adapter-with-fedora/64807
- GitHub repo coolshou/rtl8821cu is described as the driver for "D-Link DWA-171 rev C1". https://github.com/coolshou/rtl8821cu
- usb-modeswitch-data 20251207, file `usb_modeswitch.d/0bda:1a2b`: comment "D-Link DWA-171 Wifi Dongle", `TargetVendor=0x2001`, `TargetProduct=0x331d`, `StandardEject=1`. Tarball downloaded from draisberghof.de and read directly.
- Kernel: `rtw88/rtw8821cu.c:34` has `USB_DEVICE_AND_INTERFACE_INFO(0x2001, 0x331d, ...) /* D-Link */` bound to `rtw8821c_hw_spec`. The kernel comment does not name the model, the model link comes from the three sources above.
- D-Link's own support page confirms multiple hardware revisions with per-revision downloads (A, B, C listed; page body is truncated), and the product is discontinued, support ended 2025-01-31. https://support.dlink.com/productinfo.aspx?m=DWA-171 and https://support.dlink.com.au/DWA-171

## usb_modeswitch (relevant for C1 only)

C1 first enumerates as a Realtek mass-storage "DISK" device, `0bda:1a2b`, containing the Windows driver installer. It must be ejected to switch to 2001:331d.

- Evidence: usb_modeswitch forum thread shows `0bda:1a2b` before and `2001:331d D-Link Corp.` after `usb_modeswitch -KW -v 0bda -p 1a2b` (StandardEject), device descriptor Manufacturer "D-Link Corporation", Product "DISK". https://www.draisberghof.de/usb_modeswitch/bb/viewtopic.php?t=2831
- usb-modeswitch-data udev rule line 417 of `40-usb_modeswitch.rules`: `ATTR{idVendor}=="0bda", ATTR{idProduct}=="1a2b", RUN+=usb_modeswitch`.
- Kernel already has a quirk so `usb-storage` ignores this mode: `drivers/usb/storage/unusual_devs.h:1509` `UNUSUAL_DEV(0x0bda, 0x1a2b, ... "Realtek", "DISK", ... US_FL_IGNORE_DEVICE)` with comment "Ignore driver CD mode, otherwise usb_modeswitch may fail to switch the device into Wi-Fi mode". So no `CONFIG_USB_STORAGE` or SCSI changes are needed for the switch. (usb-storage is `=m` anyway via `superbird-usb-host.cfg`.)
- Yocto: `usb-modeswitch_2.6.2.bb` and `usb-modeswitch-data_20251207.bb` exist in `sources/meta-openembedded/meta-oe/recipes-support/usb-modeswitch/`. The binary package has `RDEPENDS = "tcl"` and `RRECOMMENDS = "usb-modeswitch-data"`, and ships a systemd unit `usb_modeswitch@.service` plus a udev helper script under `${nonarch_base_libdir}/udev`. This pulls in tcl and needs udev on the image (not checked in this pass), which is a real size cost on a 512 MiB-RAM device with a size-capped boot partition. Cheaper alternative: a one-line udev rule or script that sends the SCSI eject to the 0bda:1a2b device (the "standard eject" is START STOP UNIT eject), avoiding tcl. Not tested.
- A1 and B1 are not documented as presenting a CD mode. Evidence for A1 is weak (an Ask Ubuntu search snippet says rev A is "a straightforward wifi dongle"; page itself failed to fetch). No modeswitch rule exists for 3314 or 3d02 in usb-modeswitch-data (grep of `usb_modeswitch.d` returned nothing).

## Current image state (what is and is not built)

Kernel recipe: `meta-superbird/recipes-kernel/linux/linux-superbird_7.0.2.bb`. Fragment merge order in `do_configure`: arm64 `defconfig`, then `superbird.cfg`, `superbird-disable.cfg`, `superbird-usb-host.cfg`, then any other `*.cfg` in UNPACKDIR alphabetically (so a new extra fragment that is also in SRC_URI wins over the disable fragment), then `olddefconfig`.

- `superbird-disable.cfg` section "Wireless networking - no radios": `# CONFIG_WLAN is not set`, `# CONFIG_CFG80211 is not set`, `# CONFIG_MAC80211 is not set`, `# CONFIG_RFKILL_GPIO is not set`, `# CONFIG_RFKILL_INPUT is not set`.
- Built `.config` on devel confirms: `# CONFIG_CFG80211 is not set`, `# CONFIG_WLAN is not set`. Therefore `RTW88`, `RTL8XXXU`, `MT76*` are all invisible and not built. (A stray `CONFIG_MAC80211_STA_HASH_MAX_SIZE=0` is just a dangling numeric default.)
- Already fine: `CONFIG_RFKILL=y`, `CONFIG_FW_LOADER=y` (userspace helper, no `EXTRA_FIRMWARE`), `CONFIG_USB_XHCI_HCD=y`, `CONFIG_USB_DWC3=y` with `CONFIG_USB_DWC3_DUAL_ROLE=y`, `CONFIG_MODULES=y`, `CONFIG_USB_STORAGE=m`, `CONFIG_SCSI=m`. `CONFIG_CRYPTO` for mac80211 is pulled by select.
- `meta-superbird/conf/distro/superbird.conf:13`: `DISTRO_FEATURES:remove = "... 3g nfc wifi ..."`. `wifi` removed. Not a hard blocker for installing packages by name, but it influences recipe defaults (for example packagegroups and some `wpa-supplicant`/`iw` PACKAGECONFIG or RRECOMMENDS logic). Flag it.
- `meta-superbird/recipes-core/images/superbird-bsp-image.bb:24`: `BAD_RECOMMENDATIONS += "kernel-modules udev-hwdb wpa-supplicant wireless-regdb wireless-regdb-static weston-init"`. This only blocks RECOMMENDED pulls. An explicit `IMAGE_INSTALL` or `RDEPENDS` still installs these. Note `kernel-modules` is also bad-recommended, so the `.ko` files will not arrive automatically, they must be named explicitly.
- `IMAGE_INSTALL` is just `packagegroup-core-boot packagegroup-superbird-runtime` (`superbird-bsp-image.bb:19`). The runtime packagegroup has no wifi, firmware (other than `superbird-firmware`, BCM20703A2 only), `iw` or `wpa-supplicant` entries.
- Kernel binary: kernel is monolithic Image with a 64 MiB boot_a per comment in `superbird.cfg`, drivers should be `=m` as `.ko` on the squashfs rootfs to avoid growing the Image.

## Minimal change list

### Kernel config fragment (new fragment or edit `superbird-disable.cfg`)

The three `# CONFIG_... is not set` lines for WLAN, CFG80211 and MAC80211 in `superbird-disable.cfg` must be removed (or overridden by a fragment that sorts later in the merge loop and is in SRC_URI). Then add:

```
CONFIG_WLAN=y
CONFIG_CFG80211=m
CONFIG_MAC80211=m
CONFIG_WLAN_VENDOR_REALTEK=y
CONFIG_RTW88=m
# rev C1 (RTL8821CU, 2001:331d)
CONFIG_RTW88_8821CU=m
# rev A1 (RTL8811AU, 2001:3314)
CONFIG_RTW88_8821AU=m
CONFIG_WLAN_VENDOR_MEDIATEK=y
# rev B1 (MT7610U, 2001:3d02)
CONFIG_MT76x0U=m
```

Use only the lines for the revision you hold, or all three (small modules, rtw88 core is shared between the two Realtek ones). `RTW88_CORE`, `RTW88_USB`, `RTW88_8821C`, `RTW88_8821A`, `RTW88_88XXA`, `MT76_CORE`, `MT76_USB`, `MT76x02_USB`, `MT76x0_COMMON` are selected automatically. `WLAN_VENDOR_*` default to y but pinning them is harmless. `RTW88_8821CU` and `RTW88_8821AU` both `depends on USB` (met). Do not use `RTL8XXXU`: it does not list any of these IDs for 8821/8811 (its 2001 entries are 0x3319 and 0x3312, other D-Link models, `rtl8xxxu/core.c:7955,8214`) and has no 8821A/8821C support. Verified Kconfig at `rtw88/Kconfig:195,206`, `mt76x0/Kconfig`.

Optional: `CONFIG_CFG80211_CERTIFICATION_ONUS` is not needed. With default `CFG80211_REQUIRE_SIGNED_REGDB=y` and `CFG80211_USE_KERNEL_REGDB_KEYS=y` (`net/wireless/Kconfig:89,99`), cfg80211 wants `regulatory.db` and `regulatory.db.p7s` from `/lib/firmware`, which is the `wireless-regdb` package. Without it cfg80211 falls back to the world regulatory domain (5 GHz channels restricted to passive/no-IR), so Wi-Fi works but 5 GHz AP mode and some channels are limited. Hard requirement only if you want proper country channels.

### IMAGE_INSTALL / packagegroup additions (names verified in the layers)

| Need | Package | Recipe evidence | Applies to |
|---|---|---|---|
| Kernel modules | `kernel-module-rtw88-8821au`, `kernel-module-rtw88-8821cu`, `kernel-module-mt76x0u` plus the core chain pulled by kmod deps: `kernel-module-rtw88-core`, `kernel-module-rtw88-usb`, `kernel-module-mac80211`, `kernel-module-cfg80211` (and mt76 core modules for B1: `kernel-module-mt76`, `kernel-module-mt76-usb`, `kernel-module-mt76x02-lib`, `kernel-module-mt76x02-usb`, `kernel-module-mt76x0-common`) | module names derived from Makefiles by convention, not individually checked in the build output since nothing is built yet | per rev |
| Firmware A1, C1 | `linux-firmware-rtl8821` (FILES include `rtw88/rtw8821*.bin*`, covers both `rtw8821a_fw.bin` and `rtw8821c_fw.bin`; `RDEPENDS` on `linux-firmware-rtl-license`) | `sources/openembedded-core/meta/recipes-kernel/linux-firmware/linux-firmware_20260410.bb:1671-1675,1721` | A1, C1 |
| Firmware B1 | `linux-firmware-mediatek` (FILES `firmware/mediatek/*`; includes `mt7610u.bin`; RDEPENDS `linux-firmware-mediatek-license`). Note the narrower `linux-firmware-mt7601u`, `-mt7650`, `-mt76x2` packages do NOT include `mt7610u.bin` | same recipe, lines 1181-1192 and 1148-1178 | B1 |
| Regdb (optional) | `wireless-regdb` (recipe `wireless-regdb_2026.02.04.bb`, `meta/recipes-kernel/wireless-regdb`). Must be installed explicitly because of BAD_RECOMMENDATIONS | exists | all |
| wpa-supplicant | `wpa-supplicant` (2.11, `meta/recipes-connectivity/wpa-supplicant`), explicit install needed | exists | all |
| iw | `iw` (6.17, `meta/recipes-connectivity/iw`) | exists | all (diagnostics) |
| Mode switch | `usb-modeswitch` and `usb-modeswitch-data` (meta-oe, pulls `tcl`) | exists | C1 only |

Notes: `kernel-module-*` package names assume the kernel recipe emits per-module packages as usual for `inherit kernel`. The firmware recipe in this checkout is fetched but not yet built (`build/tmp/work/all-poky-linux/linux-firmware/20260410` contains only `temp/`), so I verified package contents from the recipe `FILES`, not from built package contents. Rtw88 firmware files for 8821a and 8821c are listed in upstream WHENCE (lines 3666 and 3673).

Do NOT add the umbrella `linux-firmware` or `kernel-modules`: they pull every firmware blob and would explode the squashfs.

### Network bring-up (out of scope, noted)

No network manager in the packagegroup. `wpa_supplicant` plus a systemd-networkd or init script would be needed. The USB-C port is device-mode by default (`superbird-usb-role` package exists), so a USB host/role switch is needed for the dongle to enumerate at all. Not investigated here.

## Confidence

- High: B1 = MT7610U = 2001:3d02 = `mt76x0u` (kernel source comment names the model and revision). Firmware file name and package (`mt7610u.bin` in `linux-firmware-mediatek`) verified in recipe and WHENCE.
- High: A1 = RTL8811AU = 2001:3314, covered by `rtw88_8821au` in 7.0.2 (kernel source and WikiDevi agree).
- High: C1 = RTL8821CU, Wi-Fi mode 2001:331d, covered by `rtw88_8821cu`; modeswitch rule 0bda:1a2b to 2001:331d verified in usb-modeswitch-data and forum log; kernel storage quirk present. The kernel ID table does not name the model, but three independent sources do.
- High: current config does not build any of it (`.config` and disable fragment checked directly).
- Medium: whether the `kernel-module-*` package names exactly match (nothing built). Whether A1 never shows a CD mode (weak evidence).
- Medium-low: the deviwiki C1 snippet listing 2001:3314 for C1; page not fetched, conflicts with the other C1 evidence.
- Not verified: actual performance or stability of rtw88 USB drivers on this SoC, DWC3 host-mode wiring, whether udev is present for the usb-modeswitch rule, exact built module names, hardware label of the user's unit. The D-Link product page was not fetched (only the support pages).
- Not tested on hardware.

## Sources

- Kernel tree on devel: `~/workspace/yocto-superbird-build/build/tmp/work-shared/superbird/kernel-source` (Makefile: 7.0.2). Files: `drivers/net/wireless/realtek/rtw88/{rtw8821au.c,rtw8821cu.c,rtw8822bu.c,rtw8812au.c,Kconfig}`, `drivers/net/wireless/mediatek/mt76/mt76x0/{usb.c,Kconfig,mt76x0.h}`, `drivers/net/wireless/realtek/rtl8xxxu/{core.c,Kconfig}`, `drivers/usb/storage/unusual_devs.h`, `net/wireless/Kconfig`, `net/mac80211/Kconfig`.
- Layer files: `meta-superbird/recipes-kernel/linux/linux-superbird/{superbird.cfg,superbird-disable.cfg,superbird-usb-host.cfg}`, `linux-superbird_7.0.2.bb`, `recipes-core/images/superbird-bsp-image.bb`, `conf/distro/superbird.conf`, `recipes-bsp/superbird-firmware/superbird-firmware.bb`, `sources/openembedded-core/meta/recipes-kernel/linux-firmware/linux-firmware_20260410.bb`, `sources/meta-openembedded/meta-oe/recipes-support/usb-modeswitch/*.bb`.
- https://wikidevi.wi-cat.ru/D-Link_DWA-171_rev_A1 (A1 chip and ID)
- https://wikidevi.wi-cat.ru/D-Link_DWA-171_rev_B1 (search snippet only: MediaTek, 2001:3d02)
- https://deviwiki.com/wiki/D-Link_DWA-171_rev_C1 (search snippet only, fetch failed)
- https://discussion.fedoraproject.org/t/using-a-dwa-171-wifi-adapter-with-fedora/64807 (C1 enumerates as 2001:331d after CD mode)
- https://www.draisberghof.de/usb_modeswitch/bb/viewtopic.php?t=2831 (0bda:1a2b to 2001:331d)
- https://www.draisberghof.de/usb_modeswitch/usb-modeswitch-data-20251207.tar.bz2 (rule file `0bda:1a2b`, udev rule)
- https://github.com/coolshou/rtl8821cu (search snippet: driver for DWA-171 rev C1)
- https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain/WHENCE (firmware file list)
- https://support.dlink.com/productinfo.aspx?m=DWA-171 and https://support.dlink.com.au/DWA-171 (revisions, discontinued)
- Not fetched: linux-hardware.org, morrownr/8821au and 8821cu repos, linux-usb.org usb.ids entry for 2001:3314/331d/3d02 (the usb.ids full text was fetched but my grep of the 2001 block returned no matches because of formatting, so usb.ids is not used as evidence).
