# Removing the bandaid partition from the thing-revived image

Research only, read-only inspection, nothing built or flashed. Date 2026-11-27. BSP root: `/home/andremmfaria/workspace/yocto-superbird-build-2` (called UP below). Project root: `/home/andremmfaria/projects/thing-revived` (called TR below).

## Summary

Removal is feasible with project-side overrides only, no upstream edits: a project wks without the bandaid line, three variable overrides in the prod image bbappend, one packagegroup bbappend that drops the placeholder daemon, one fstab line removed in a rootfs post-process, plus checker and test updates. Nothing in the bootloader, the OTA sw-description files, the flash script or the extlinux boot line depends on bandaid. The data partition grows by 192 MiB (1468 to 1660 MiB on the 3600 MiB floor, +13%). The one real risk is not at build time: a re-flashed device that previously had the 900 MiB layout will have the old bandaid ext4 superblock sitting exactly at the start of the new data partition, which `x-systemd.makefs` will not reformat. A guard is needed (see risks). Shrinking to 32 MiB instead gains 160 MiB and needs only `SUPERBIRD_BANDAID_PART_SIZE = "32"` in the kas file, but keeps the whole bandaid machinery alive for nothing.

## Findings: every bandaid reference

Evidence form is file:line. "Must change" means required for a clean removal build or boot; "stay" means harmless to leave; "irrelevant" means no effect on this image.

| # | Reference | Verdict | Notes |
|---|---|---|---|
| 1 | `UP/meta-superbird/files/wic/superbird-mainline.wks.in:8` the `part bandaid ... --fixed-size ${SUPERBIRD_BANDAID_PART_SIZE}M` line | must change | The only thing that creates the partition. Needs a project wks (see recipe). Setting the size var to 0 does not remove the line; a 0M fixed-size part is untested and should not be relied on (unverified). |
| 2 | `UP/meta-superbird/conf/distro/superbird.conf:50` `SUPERBIRD_BANDAID_PART_SIZE ?= "192"` | can stay | Weak default. Still read by headroom check (row 4), `bandaid-image.bbclass:24` and `check_image_accounts.py`. With a custom wks it no longer shapes the disk. |
| 3 | `UP/meta-superbird/classes/bandaid-image.bbclass` (`BANDAID_IMG_BYTES` line 24, truncate line 46, mkfs line 47) | irrelevant | Only runs when `superbird-kiosk-bandaid` is built. A size of 0 would break `truncate`/`mkfs.ext4`, so never build the recipe with 0. |
| 4 | `UP/meta-superbird/classes/superbird-headroom-check.bbclass:22,25` | can stay | `mib()` does `int(raw.strip())`, so "0" is accepted (lines 9-16). Formula `wic_fixed = env + 2*boot + 2*root + band` (line 25). With 192 left in: 8+128+1800+192 = 2128, data_floor 1472, required 1028 (64+900+64), passes. Setting it to "0" in the kas file makes the check accurate (floor 1664) and is accepted. It only gates when `SUPERBIRD_REQUIRE_OTA_HEADROOM` is 1 (line 6). |
| 5 | `UP/meta-superbird/classes/mainline-flashthing.bbclass:21-29,46-54,69-73,83-95,115-116` | can stay, set flag | Everything is gated on `MAINLINE_FLASHTHING_WITH_BANDAID == "1"`. With "0": no `bandaid.ext4` input, no GPT name lookup (so the fatal at line 72 does not fire), no extra `writeUserArea` step, no extra zip member. Only the description string at line 101 still says "+ bandaid" (cosmetic). |
| 6 | `UP/examples/meta-superbird-kiosk-example/recipes-core/images/superbird-kiosk-image-base.inc:7-8` `MAINLINE_FLASHTHING_WITH_BANDAID = "1"` and recipe name | must change | Plain `=` in the recipe wins over a plain `=` in local.conf, so override in the prod bbappend (a kas local_conf_header line would need `:forcevariable`, unverified here). |
| 7 | same inc `:26` `WKS_FILE = "superbird-mainline.wks.in"` | must change | Same precedence rule. Override in the prod bbappend. |
| 8 | same inc `:35` `EXTRA_IMAGEDEPENDS += "superbird-uboot superbird-kiosk-bandaid"` | must change | Drop the bandaid recipe: `EXTRA_IMAGEDEPENDS:remove = "superbird-kiosk-bandaid"` in the bbappend. Leaving it only wastes build time; it still builds with size 192. |
| 9 | `UP/examples/.../recipes-core/superbird-kiosk-bandaid/superbird-kiosk-bandaid.bb` | irrelevant | Never built once row 8 is done. Also referenced by `BANDAID_PACKAGES` (daemon + default webapp). |
| 10 | `UP/meta-superbird/recipes-core/base-files/base-files/fstab:2` `PARTLABEL=bandaid /var/lib/bandaid ext4 rw,noatime,sync,nofail 0 0`, wired by `base-files_%.bbappend` | must change | See boot behaviour below. Remove the line in a rootfs post-process. |
| 11 | `UP/meta-superbird/recipes-core/opt-overlay/` (`opt-overlay.bb`, `opt-overlay@.service:4-5` After/Wants `var-lib-bandaid.mount`, `opt-overlay-bind:6,10-30`) | can stay | Template unit, `SYSTEMD_AUTO_ENABLE = "disable"` (opt-overlay.bb:14). Only instantiated by `superbird-kiosk-daemon.service` (`Requires=opt-overlay@superbird-kiosk.service`). With the daemon gone nothing instantiates it. Even if instantiated, `Wants=` is soft and `opt-overlay-bind:10-17` falls back to a read-only factory bind when `/var/lib/bandaid` is not a mountpoint. It is pulled in by `packagegroup-superbird-runtime.bb:28` (upstream) so it stays installed; dead weight only. |
| 12 | `UP/meta-superbird/recipes-core/superbird-fsck/superbird-fsck.bb:14` `SYSTEMD_SERVICE = "superbird-fsck@bandaid.service superbird-fsck@data.service"`; `superbird-fsck.sh:10` policy; `superbird-fsck@.service` | can stay (optional cleanup) | With no partition, `superbird-fsck.sh:15-22` runs `udevadm settle` then exits 0 "absent; nothing to check". Costs one settle at sysinit. Optional: delete `sysinit.target.wants/superbird-fsck@bandaid.service` in the same post-process. Whether `preset-all` re-enables it was not checked (unverified). |
| 13 | `UP/meta-superbird/recipes-core/packagegroups/packagegroup-superbird-runtime.bb:24,28` (superbird-fsck, opt-overlay) | can stay | Needed by the data fsck anyway (fsck) and harmless (opt-overlay). |
| 14 | `UP/examples/.../packagegroups/packagegroup-superbird-kiosk-core.bb:13` RDEPENDS `superbird-kiosk-daemon` | must change if daemon is dropped | Only the kiosk-core packagegroup and `BANDAID_PACKAGES` reference the daemon in recipes (grep over UP and TR/build/overlay). Drop via a project bbappend: `RDEPENDS:${PN}:remove = "superbird-kiosk-daemon"`. |
| 15 | `UP/examples/.../superbird-kiosk-daemon/` (`.bb:17` RDEPENDS opt-overlay, service `Requires=opt-overlay@superbird-kiosk`, `ExecStart=/opt/superbird-kiosk/daemon/kioskd.current`) | can stay out of the image | Self contained hello-world heartbeat writer (`kioskd.c:24`). No other unit orders against it. |
| 16 | `UP/examples/.../chromium-kiosk/chromium-kiosk_%.bbappend:5` `KIOSK_ENV_OVERRIDE_FILE=/opt/superbird-kiosk/kiosk-env` | can stay | The launcher reads it only if set and readable (`UP/meta-superbird/recipes-graphics/chromium-kiosk/files/chromium-kiosk-launch:196-198`). Missing file is skipped. `/opt/superbird-kiosk` also would not exist, no error. |
| 17 | `UP/meta-superbird/recipes-core/superbird-provision/files/superbird-provision.sh` | can stay | No bandaid mention. Carves data with `sgdisk -e -n 0:0:0 -c 0:data -t 0:8300`, i.e. the whole free tail after the last allocated partition. Early exit if `sgdisk -p` already lists `data` (line 12). Its headroom warning computes free space from the real last allocated sector, so it adapts. |
| 18 | `superbird-init`, `superbird-preinit`, `superbird-factory-reset`, `superbird-slot-ok`, extlinux, uenv, `superbird-bsp-update` sw-descriptions | irrelevant | No bandaid reference (grep). They address `env`, `boot_a/b`, `root_a/b`, `data` by partlabel only (`extlinux.conf:8` `root=PARTLABEL=root_${slot}`, sw-description `by-partlabel/boot_*`/`root_*`, factory-reset `by-partlabel/data`). |
| 19 | `UP/meta-bridgething/**`, `UP/scripts/superbird-flash-fast`, `UP/scripts/test-*` | irrelevant | Not part of this build (bridgething product and dev helper scripts). `superbird-flash-fast` is an upstream helper, not used by `TR/build/flash-image.sh`. |
| 20 | u-boot (`ThingLabsOSS/superbird-uboot`, checked at pinned SRCREV `89aaad01...` from `meta-superbird/recipes-bsp/superbird-uboot/*.bb` and at master) | irrelevant | `git grep -i bandaid` hits only `CLAUDE.md:47` (a prose note listing the GPT layout). Code finds partitions by name: env via `CONFIG_ENV_FAT_DEVICE_AND_PART="0#env"` (`configs/spotify_carthing_defconfig:13`), boot logo by `part_get_info_by_name(desc, "boot_<slot>")` (`board/amlogic/spotify-carthing/spotify-carthing.c`), slot state in env vars `slot_active`/`slot_<x>_tries`. No fixed LBA or index for bandaid. |
| 21 | Flash meta: `mainline-flashthing.bbclass:90-95` | irrelevant when flag 0 | Steps are bulkcmd, two `writeBootPartition`, `writeUserArea lba 0` (wic, sparse). The bandaid write is a separate optional step at the GPT start LBA of `bandaid`. No other LBA is hard coded. |
| 22 | `TR/build/flash-image.sh` | irrelevant | No bandaid, LBA or partition handling; it passes the newest zip to flashthing-cli (lines 72-80, 155-158). `TR/build/build-image.sh` `stage_artifact` is also partition agnostic (picks `*flashthing*.zip`, drops env-only). |
| 23 | `TR/build/tools/check_image_accounts.py:203,206,1444,1869-1870,1891` and `test_check_image_accounts.py:594-595,962` | must change | See checker section. |
| 24 | `TR/build/tools/fixtures/partitions/*.sfdisk` (nine files carry a bandaid row; `bad_missing_bandaid.sfdisk` has none) | must change | Fixtures encode the bandaid at sector 3973120 (1940 MiB), size 393216 (192 MiB). |
| 25 | `TR/docs/research/user-management-plan.md:83`, `user-management-spikes.md:71` | docs only | Describe `/var/lib/bandaid` as separate and the wic table; update after the change. |
| 26 | `TR/build/overlay/**` | irrelevant | No bandaid, opt-overlay or daemon reference (grep). `TR/build/test-pkg-hold.sh:39` lists `superbird-kiosk-daemon` only in a fake HELD fixture string, no behaviour tied to the real package. `TR/build/test-build-image.sh:137` asserts exact kas lines; any new kas line is not asserted by existing checks, so adding lines there does not break it, but add a `want_line` if you add one. |

## Minimal override recipe (no upstream edits)

1. New file `TR/build/overlay/files/wic/superbird-mainline-nobandaid.wks.in`: copy of the upstream wks with the `part bandaid` line (and the stale comment) removed, same `${SUPERBIRD_*}` variables, same `bootloader --ptable gpt`. Keep the `.in` suffix, wic expands it in `image_types_wic.bbclass` (python block around line 166-185).
2. How bitbake finds it: `build-image.sh` step 5 runs `cp -a build/overlay/. examples/meta-superbird-kiosk-example/`, so the file lands at `<example layer>/files/wic/`. The example `conf/layer.conf:1` does `BBPATH .= ":${LAYERDIR}"`. `image_types_wic.bbclass:49` sets `WKS_SEARCH_PATH ?= "${THISDIR}:<each BBPATH>/files/wic"`, and `wks_search` (line 52-60) returns the first hit via `bb.utils.which`. Use a different file name from upstream: meta-superbird is listed before the example layer in `UP/kas/example-kiosk.yml`, so an identically named file in the example layer would lose to upstream (inferred from layer order, not run).
3. In `TR/build/overlay/recipes-core/images/superbird-kiosk-prod-image.bbappend` (already exists, runs after the inc) add: `WKS_FILE = "superbird-mainline-nobandaid.wks.in"`, `MAINLINE_FLASHTHING_WITH_BANDAID = "0"`, `EXTRA_IMAGEDEPENDS:remove = "superbird-kiosk-bandaid"`.
4. New `TR/build/overlay/recipes-core/packagegroups/packagegroup-superbird-kiosk-core.bbappend`: `RDEPENDS:${PN}:remove = "superbird-kiosk-daemon"`. Optional, since the daemon without bandaid still runs from the read-only factory bind (opt-overlay-bind:10-17) but its heartbeat write to `/opt/superbird-kiosk/.heartbeat` would hit a read-only squashfs/ext4 bind and fail, so drop it.
5. In the same prod bbappend add a `ROOTFS_POSTPROCESS_COMMAND` function that `sed -i '/PARTLABEL=bandaid/d' ${IMAGE_ROOTFS}${sysconfdir}/fstab`, then `bbfatal` if `grep -q bandaid` still matches (same style as the existing `bbfatal` assertions). An alternative of shipping an own fstab through a `base-files` bbappend with `FILESEXTRAPATHS:prepend` would also work if the higher-priority layer's prepend wins, but that precedence was not run (unverified); the post-process is the safer choice.
6. In the generated kas `local_conf_header` (`write_flags_kas`, build-image.sh) add `SUPERBIRD_BANDAID_PART_SIZE = "0"` so the headroom check counts the real layout. Accepted by `mib()`; it reaches nothing else once the bandaid recipe is not built. Add a matching `want_line` to `TR/build/test-build-image.sh`.
7. Optional: remove `sysinit.target.wants/superbird-fsck@bandaid.service` in the post-process, with an assertion.

## Boot behaviour if the fstab entry or opt-overlay units remain without the partition

- The fstab entry has `nofail` (fstab:2), so the generated `var-lib-bandaid.mount` is a `Wants=` of local-fs.target, not a `Requires=`. A missing device does not fail boot. Whether `nofail` also drops the `Before=local-fs.target` ordering, and so how long the 90 s default device timeout can delay the basic boot, depends on the systemd version's fstab-generator and was not verified (systemd source is not in the tree). Add `x-systemd.device-timeout=` or simply delete the line; deleting is the recommendation.
- `opt-overlay@.service` uses `Wants=var-lib-bandaid.mount` plus `After=` (lines 4-5): soft dependency, a failed mount does not fail the unit, and the script then binds the factory copy read-only (opt-overlay-bind:10-17). It is only instantiated by the daemon unit.
- `superbird-fsck@bandaid` exits 0 when the device is absent (superbird-fsck.sh:19-22).

## First-boot data carve and size change

Numbers use the project layout (env 8, boot 64 x2, root 900 x2, 4 MiB alignment from `--align 4096`) as in `TR/build/tools/fixtures/partitions/good.sfdisk`: bandaid starts at sector 3973120 = 1940 MiB, ends 2132 MiB. The eMMC floor is `SUPERBIRD_MIN_EMMC_SIZE_MIB` 3600 (superbird.conf); the real chip size is unverified (the docs observe a data partition of about 2.3 GiB on the old 516 MiB layout).

| Layout | Data start | Data size on 3600 MiB floor | Gain |
|---|---|---|---|
| Current, bandaid 192 | 2132 MiB | 1468 MiB | none |
| Bandaid 32 | 1972 MiB | 1628 MiB | +160 MiB (+10.9%) |
| Bandaid removed | 1940 MiB | 1660 MiB | +192 MiB (+13.1%) |

The data carve is `superbird-provision.sh:31` (`sgdisk -e -n 0:0:0`) which takes the whole free tail after the last allocated partition, then `partx -a`, so a layout ending at root_b works without change. The OTA need is 1028 MiB (64+900+64); removal leaves 1660, shrinking leaves 1628, both pass. The flashthing zip also loses the 201 MB `bandaid.ext4` (deploy dir shows 201326592 bytes) though it compresses heavily.

## Checker and tests to change

- `check_image_accounts.py:203` `PARTITION_LAYOUT` drops the bandaid tuple (or makes it optional); `:206` default `bandaid_mib` and `:1869-1870`, `:1891` the `--bandaid-size-mib` option go or accept 0 meaning absent; docstring at `:1444` ("Six named partitions") becomes five. The headroom math at `:1476-1487` uses the real end of the last partition, so it adapts by itself.
- Fixtures: delete the bandaid row from `good.sfdisk` and every `bad_*.sfdisk` and fix start sectors where nothing follows; `bad_missing_bandaid.sfdisk` becomes the new good shape and `bad_bandaid_size.sfdisk` is deleted. `bad_headroom.sfdisk` must be recomputed (its expected 868 MiB headroom relies on the bandaid end at 2732 MiB).
- `test_check_image_accounts.py:594-595` (remove the two bandaid cases), `:599` (new headroom number), `:962` (expected name list drops "bandaid"). Add a case that a leftover `bandaid` partition is reported as unexpected if you want the checker to enforce removal.
- `TR/build/test-build-image.sh`: add `want_line` for the new kas variable. No existing test asserts the daemon or opt-overlay.
- Docs to update: the two research docs in row 25, and `TR/docs/packages.md:159-165` ("data partition starts at a different sector, `/var` effectively wiped").

## Risk table

| Stage | Risk | Rating | Detail and mitigation |
|---|---|---|---|
| Build | Wrong WKS_FILE precedence (kas local_conf loses to recipe) | low | Use the bbappend, not kas, for `WKS_FILE` and the flashthing flag. |
| Build | wks not found | low | Distinct file name, delivered by the existing `cp -a` overlay copy, found through BBPATH `files/wic`. Build fails loudly (`image_types_wic.bbclass:85`) if not. |
| Build | Leaving `superbird-kiosk-bandaid` in EXTRA_IMAGEDEPENDS with size 0 | medium if done | `truncate -s 0` plus `mkfs.ext4` on an empty file fails. Remove the dep, or never set 0 together with it. |
| Build | Checker or tests failing | low | Mechanical updates listed above; run `check_image_accounts` tests after. |
| Flash | Old bandaid superblock at the new data start | medium to high on re-flashed devices | A device flashed with the current 900 layout has ext4 "bandaid" at 1940 MiB. The new GPT has no partition there, but the carve creates `data` starting exactly at 1940 MiB, and `x-systemd.makefs` (fstab:1) formats only when no filesystem is found. The stale filesystem (192 MiB, label bandaid) would mount as `/var`, leaving the user with 192 MiB, not 1660. This is the same unknown noted at `user-management-spikes.md:71` (does the flasher clear the area). Whether flashthing's sparse write zeroes the tail is unverified. Mitigation: a project unit ordered after `superbird-provision.service` and before `var.mount` that runs `mkfs.ext4 -L data` when `blkid` reports label `bandaid` on the data partition (or zero the first MiB at the new data LBA from the project's flash step). Verify on a device before trusting. |
| Flash | Fresh or blank device | low | Nothing stale, normal carve. |
| First boot | fstab line left in with no partition | low to medium | Soft dependency; possible boot delay of up to the device timeout (unverified). Removed by the post-process. |
| First boot | Kiosk loses `/opt/superbird-kiosk` | low | Only the daemon and the optional `kiosk-env` override use it; the launcher tolerates a missing file. |
| First boot | OTA after removal | low | sw-description files address only boot/root slots by partlabel; OTA does not touch bandaid or data. |
| Rollback | Going back to bandaid later | medium | Would need a reflash and another data move; data layout changes again. |

## Go or no-go

Go on removal, given the guard against the stale filesystem. It is the cleaner end state (+192 MiB, no dead mount, no daemon) and the override surface is small and entirely project-owned. Shrinking to 32 MiB is the lower-effort fallback: a one-line kas change (`SUPERBIRD_BANDAID_PART_SIZE = "32"`), no wks, no fstab, no daemon changes, +160 MiB, and the checker needs only its default 32. Two caveats on shrinking: the stale-superblock problem still applies (old bandaid ext4 would sit at 1940 MiB where the new bandaid begins, but bandaid.ext4 is rewritten by the zip so that case is clean) and the upstream placeholder daemon plus its 4 MiB journal-formatted partition stay for nothing. Real difference between the options: 32 MiB more data and no bandaid machinery at the cost of the override set and the guard. If the owner wants the least risk this release, shrink; if the goal is a clean layout, remove.

## Confidence

- High: wks content, bandaid-flag gating in the flash class, headroom formula and its acceptance of 0, provision carving the tail, absence of bandaid in u-boot code and flash/build scripts, daemon dependents (grep over UP and TR), data size arithmetic on the 3600 MiB floor.
- Medium: wic template and WKS_SEARCH_PATH behaviour (read in `image_types_wic.bbclass`, not executed), bbappend precedence over the recipe `=` assignment (standard bitbake, not run).
- Unverified: nofail ordering and exact boot delay with a missing partition (no systemd source read); flashthing sparse-write behaviour on the area after root_b; real eMMC size; `WKS_FILE:forcevariable` from kas; whether `preset-all` re-enables a removed fsck symlink; wic behaviour with a 0M fixed-size part; layer order effect on same-name wks; the u-boot grep covered the pinned SRCREV and the master and debug-console heads only.

## Sources (local, read-only)

- `UP/meta-superbird/files/wic/superbird-mainline.wks.in`, `conf/distro/superbird.conf`, `classes/{bandaid-image,mainline-flashthing,superbird-headroom-check,superbird-image}.bbclass`, `recipes-core/{opt-overlay,superbird-fsck,superbird-provision,base-files,packagegroups,images}/**`
- `UP/examples/meta-superbird-kiosk-example/**`, `UP/kas/{base,superbird,example-kiosk}.yml`
- `UP/sources/openembedded-core/meta/classes-recipe/image_types_wic.bbclass`
- `UP/build/downloads/git2/github.com.ThingLabsOSS.superbird-uboot.git` (git grep on 89aaad01, master, debug-console)
- `TR/build/{build-image.sh,flash-image.sh,test-build-image.sh,test-pkg-hold.sh}`, `TR/build/overlay/**`, `TR/build/tools/{check_image_accounts.py,test_check_image_accounts.py,fixtures/partitions/*}`
- `UP/build/tmp/deploy/images/superbird/` (bandaid.ext4 size, one prior wic table)
