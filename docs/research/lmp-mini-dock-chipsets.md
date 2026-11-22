# LMP USB-C mini Dock (P/N 14368) chipset research

Researched 2026-11-22. Target kernel: 7.0.2-superbird (yocto-superbird, aarch64), host mode.

## Summary

The actual chips inside the LMP USB-C mini Dock could not be verified. No teardown, lsusb dump, macOS System Information listing, linux-hardware.org probe, FCC filing or LMP support document naming the Ethernet, hub, card reader or PD chips was found in about 12 searches and 8 page fetches. LMP only says the dock contains "a number of chipsets" and that it is "Plug & Play, no driver needed" (macOS/Windows). Everything below the verified facts is therefore a candidate mapping, not an identification. The practical approach is to plug the dock into the Car Thing and read `lsusb -v` (or `/sys/bus/usb/devices/*/{idVendor,idProduct,manufacturer,product}`), then pick the module from the table of candidates.

The built module set already covers every plausible Ethernet chip for this class of dock, so no rebuild should be needed whichever chip turns out to be present.

## Verified facts about the product

- 9 ports per LMP page: 1x USB-C 3.1 host cable (22 cm), 1x USB-C PD 3.0 (up to 100 W pass-through), 1x USB-C data (5 Gbps), 1x USB-A 3.0 BC1.2 (1.5 A), 2x USB-A 3.0, 1x Gigabit Ethernet, SD and microSD readers, 1x HDMI (LMP claims up to 4K@60 Hz; a reseller page says HDMI 1.4, 4K@30 Hz, so the video spec is inconsistent between sources). Source: https://lmp-adapter.com/product/lmp-usb-c-mini-dock (fetched, current), https://shop.dataworld.de/art/15954 (search snippet only).
- Chip vendors are not listed on the LMP page, FAQ or support page. Source: https://lmp-adapter.com/support (fetched, no chip info).
- The 8-port (older) and 9-port (current, extra USB-C data port) revisions share P/N 14368 on retailer pages, so the internals may differ between production runs. Chip identity from one unit may not hold for another. Confidence: medium, inferred from retailer listings (https://sozodistributing.com/lmp-usb-c-mini-dock-hdmi-3x-usb-3-0-ethernet-sd-microsd-usb-c-charging-aluminum-housing-silver/14368 says 8 ports, LMP says 9).
- Reviews checked (MacSources, Galaxus, Amazon.de listing) give no chipset details. Source: https://macsources.com/lmp-usb-c-mini-dock-review-a-space-gray-travel-companion-that-works

## Function to module map

Module and package names come from the task's list of built modules and the standard Yocto naming rule (`kernel-module-` plus module name with underscores turned into hyphens). The VID:PID values for candidates are from the task brief and a forum lsusb dump, not from this dock. Linking of VID:PID to driver is background kernel knowledge not re-fetched this session.

| Function | Chip | VID:PID | Kernel module | Yocto package | Confidence |
|---|---|---|---|---|---|
| Gigabit Ethernet | UNVERIFIED. Candidate A: Realtek RTL8153 | 0bda:8153 (candidate) | r8152 (needs usbnet, mii) | kernel-module-r8152, kernel-module-usbnet, kernel-module-mii | Chip: unverified. Mapping if A: high |
| Gigabit Ethernet | UNVERIFIED. Candidate B: ASIX AX88179 | 0b95:1790 (candidate) | ax88179_178a (needs usbnet, mii) | kernel-module-ax88179-178a, kernel-module-usbnet, kernel-module-mii | Chip: unverified. Mapping if B: high |
| Gigabit Ethernet | UNVERIFIED. Candidate C: ASIX AX88179A/B or other chip exposing CDC-NCM | unknown | cdc_ncm (needs cdc_ether, usbnet) | kernel-module-cdc-ncm, kernel-module-cdc-ether, kernel-module-usbnet | Unverified, low |
| USB-A 3.0 x3, USB-C data x2 | Hub chip(s) UNVERIFIED (Genesys GL35xx, VIA VL8xx, Realtek RTS54xx all plausible) | unknown | none, USB hub class support is core `usbcore`/hub driver | none (no kernel-module package) | Mapping: high. Chip: unverified |
| SD and microSD reader | UNVERIFIED (Genesys GL32xx, Realtek RTS5xxx, Alcor AU69xx all plausible). Almost all USB readers present as USB Mass Storage | unknown | usb-storage (or uas if the reader exposes UAS), plus sd_mod for the block device | kernel-module-usb-storage, kernel-module-uas, kernel-module-sd-mod | Mapping: medium. Chip: unverified |
| Card filesystem | n/a | n/a | exfat (SDXC cards ship exFAT) | kernel-module-exfat | Medium, depends on card |
| PD controller | UNVERIFIED | unknown | none needed on the Car Thing side for dock operation | none | Unverified |
| HDMI | UNVERIFIED video bridge (DP alt mode to HDMI converter is typical). Not a USB data function | unknown | none usable | none | Unverified. Car Thing host port is unlikely to negotiate DP alt mode, expect HDMI not to work |

Notes on the Ethernet candidates:

- For Realtek, `r8152` binds by VID:PID table. For ASIX AX88179, `ax88179_178a` binds by VID:PID, and newer AX88179A/B parts can instead enumerate with a CDC-NCM configuration, in which case `cdc_ncm` binds. The `asix` module in the list covers the older AX88172/AX88772 (10/100) family and is not expected for a gigabit dock.
- A Realtek chip is the statistically likeliest choice, since most docks in this class use RTL8153 (examples in https://dancharblog.wordpress.com/2021/07/07/another-usb-c-dock-teardown and https://forums.rockylinux.org/t/ethernet-via-usb-c-docking-station/10303). That is base-rate reasoning about other docks, not evidence about this one. Treat as a guess.
- If the dock shows up only partly (hub but no NIC), check that `r8152` and `ax88179_178a` modules are actually loaded and that `usbnet` and `mii` are present, since both depend on them.

## Hub chips, firmware

- USB hub support is core Linux (`CONFIG_USB`, hub class driver in usbcore), so no separate hub module and no `kernel-module-*` package is needed whichever hub chip is used. This is standard kernel behaviour, not specific to this dock.
- No `linux-firmware` package should be needed for the hubs, card reader or Realtek/ASIX gigabit NICs in normal use. Hub chips that need firmware (for example some VIA VL8xx or Genesys parts) load it from an on-board SPI flash, not from the host. Caveat: this is general knowledge and was not confirmed for the specific parts here, and r8152 can optionally load `rtl_nic/*.fw` PHY patch firmware (works without it). If the Ethernet chip turns out to be Realtek, adding `linux-firmware-rtl-nic` is optional.

## Not found (searched, no result)

- Teardowns or PCB photos naming chips for LMP 14368 (also searched ChargerLAB, Dan Charlton style teardowns): none.
- lsusb or macOS System Information dumps for this dock: none.
- linux-hardware.org probe containing an LMP mini dock: none surfaced.
- FCC ID for LMP 14368: none found (LMP is Swiss, product may use CE only).
- LMP support docs naming chips: none. Manual at manuals.plus (https://manuals.plus/gl/asin/B01MZFK9S1) failed to fetch.

## Suggested next step

Plug the dock into the Car Thing (or any Linux host) and run:

```
lsusb -t
lsusb -v 2>/dev/null | grep -E 'idVendor|idProduct|iManufacturer|iProduct'
dmesg | tail -50
```

Match the Ethernet VID:PID against the table above. Then update this file with the real values.

## Sources

- https://lmp-adapter.com/product/lmp-usb-c-mini-dock : port list and specs, no chip data.
- https://lmp-adapter.com/support : FAQ, no chip data.
- https://sozodistributing.com/lmp-usb-c-mini-dock-hdmi-3x-usb-3-0-ethernet-sd-microsd-usb-c-charging-aluminum-housing-silver/14368 : reseller page, 8-port variant.
- https://www.galaxus.at/en/s1/producttype/toplist/brand/docking-stations-usb-hubs-303/lmp-16236 : retailer description, no chip data.
- https://macsources.com/lmp-usb-c-mini-dock-review-a-space-gray-travel-companion-that-works : review, no chip data.
- https://dancharblog.wordpress.com/2021/07/07/another-usb-c-dock-teardown : other-dock teardown, base rate only.
- https://forums.rockylinux.org/t/ethernet-via-usb-c-docking-station/10303 : other-dock lsusb showing 0bda:8153, base rate only.
- https://forum.gl-inet.com/t/axt1800-usb-port-works-with-my-2-5gbit-and-5gbit-ethernet-adaptors/21817 : lsusb strings for 0bda:8153 and 0b95:1790.
