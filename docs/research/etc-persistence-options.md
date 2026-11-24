# Persisting account changes on a read-only root: oe-core overlayfs-etc, systemd userdb and the other option classes

Status: research report, no builds and no device access. Written 2026-11-23 against the pinned yocto-superbird checkout at `/home/andremmfaria/workspace/yocto-superbird-build-2` (read only). It checks and extends the earlier decisions in `docs/research/user-management-plan.md` (section 3 options C, D, H) and `docs/research/user-management-spikes.md` (spike a), it does not redo them.

Claim tags used throughout: **[V-src]** verified by reading source in the checkout or upstream source at the named tag, **[V-doc]** stated in upstream documentation or man pages I fetched, **[U]** unverified (inference, memory, or needs a boot). Fetched web content was treated as data only. Upstream source files were read from raw.githubusercontent.com at the tags named in the Sources section.

## Verdict in ten lines

1. Keep the current late `/etc` overlay (thing-revived-etc.service plus script) as the Phase 2 baseline. Neither alternative is a drop-in win, and the late overlay is the only option that keeps stock shadow-utils working with no rewrite.
2. oe-core `overlayfs-etc` exists in this checkout (oe-core branch `wrynose`, Yocto 6.0, `meta/classes-recipe/overlayfs-etc.bbclass`) and is technically usable, but not as shipped. It cannot replace our script without a custom init template (the stock template drops the `--machine-id=` argument that `superbird-preinit` passes), `OVERLAYFS_ETC_CREATE_MOUNT_DIRS = "0"` (the default remounts the read-only root rw), and a fix for the first boot, when the data partition does not exist yet. [V-src]
3. What overlayfs-etc would buy: the overlay exists before systemd starts, so the empty machine-id window, the lost `ProtectSystem=` protection, the unit ordering list and the cycle risk all disappear. What it costs: no fsck before the mount (our `superbird-fsck` refuses to check a mounted filesystem), device resolution before udev, no retry on failure, and one extra reboot after first boot unless provisioning moves into the preinit. [V-src for the script behaviour, U for how it plays on the device]
4. systemd userdb (JSON records in `/etc/userdb` as a symlink into `/var`) works for login, sshd, sudo and PAM password auth with only nss-systemd and pam_unix, and the hash is read from `privileged.hashedPassword` through the NSS `shadow` database. [V-src]
5. userdb has no writer for regular users in our image: `userdbctl` is read only, `homectl` is not built (PACKAGECONFIG `homed` is off), and `useradd`, `usermod`, `chpasswd`, `groupadd` and `gpasswd` only edit `/etc/passwd`, `/etc/shadow`, `/etc/group` and `/etc/gshadow`. Phase 2 would have to write the JSON records, the uid symlinks, the `.membership` files and the hashes itself. [V-doc, V-src]
6. userdb removes the writable-`/etc` requirement and every boot-order risk, at the price of the whole `accounts_shadow.rs` adapter being rewritten and of owning record validation, uid allocation, locking and crash safety. Rough effort 2 to 3 times the overlay path.
7. Option 3 (tmpfs or overlay on a handful of paths) cannot work: shadow-utils creates `passwd.lock`, `passwd+` and `passwd.<pid>` in the directory of the database file, so the directory must be writable. [V-src]
8. Option 4 (symlinked or relocated `/etc/passwd`) fails the same way: shadow-utils follows a symlink only at the final rename, and still creates its lock and temp files next to the symlink. pam_unix hardcodes `/etc/passwd` and `/etc/shadow` for writes and glibc `files` has no alternate path. Only `nss_db` (`/var/db`) offers a relocated read path and it needs a custom writer too. [V-src, U for glibc]
9. Single-factor security is unchanged by either alternative: the hash stays a root-only file on `/var`, PAM and `unix_chkpwd` do the checking, faillock still applies. userdb adds one new class of failure, a malformed record silently makes the account disappear.
10. If Class C boot tests (shutdown order, power cut, machine-id window) fail on the late overlay, the cheapest fix is overlayfs-etc with the changes listed in section 2.9 plus a one-time reboot after provisioning. userdb stays the named fallback if both overlay variants are rejected.

## 0. What any option has to satisfy

- Root is ext4 read-only (`root=PARTLABEL=root_${slot} ro`, kernel command line in `meta-superbird/recipes-bsp/superbird-extlinux/files/extlinux.conf:8`), booted with `init=/sbin/superbird-preinit`. [V-src]
- `/var` is a separate ext4 on `PARTLABEL=data`, mounted by systemd from `fstab` with `rw,noatime,nofail,x-systemd.makefs` (`meta-superbird/recipes-core/base-files/base-files/fstab:1`). [V-src]
- The data partition does not exist on the first boot. `superbird-provision.sh` carves it with `sgdisk -n 0:0:0 -c 0:data` (line 32), runs `partx -a` (line 41) and `udevadm settle` (line 43). The unit runs `After=systemd-udevd.service Before=local-fs-pre.target` (`superbird-provision.service:3-4`). A factory reset wipes the data partition before `systemd-makefs` (`superbird-factory-reset.service:6`). `superbird-fsck.service` runs `After=systemd-udevd.service superbird-provision.service Before=local-fs-pre.target` and its script refuses to check a mounted device (`superbird-fsck.sh`, "is mounted; refusing to check a live filesystem"). [V-src]
- Shadow-utils tools need a writable directory containing the database (the device test in the task brief, and section 3.2 below for the source reason). [V-src]
- First-boot password setup (docs/users.md, lines 26 and 27) needs account storage on the very first boot, not after a second boot.
- Degraded mode must keep `kiosk` locked if account storage fails (docs/users.md line 64).

## 1. Release and layer facts (checked in the checkout)

- oe-core is pinned to branch `wrynose`, commit `42fa856a00ac16b2a7a83d7ecfa60a5be192b16c` (`kas/base.yml:21-24`), `LAYERSERIES_CORENAMES = "wrynose"` (`sources/openembedded-core/meta/conf/layer.conf:10`), and `git describe --tags` in `sources/openembedded-core` prints `yocto-6.0`. systemd is 259.5 (`meta/recipes-core/systemd/systemd_259.5.bb`), libpam 1.7.2 (`meta/recipes-extended/pam/libpam_1.7.2.bb`), shadow 4.19.4, openssh 10.3p1, glibc 2.43. [V-src]
- `INIT_MANAGER = "systemd"` (`meta-superbird/conf/distro/superbird.conf:9`), `DISTRO_FEATURES:append = " pam"` (`kas/base.yml:86`). `overlayfs` is not in `DISTRO_FEATURES` anywhere in `kas/` or `meta-superbird/conf` (grep for `overlayfs` and `OVERLAYFS` in `meta-superbird`, `kas` and `conf` found nothing). [V-src]
- Kernel has `CONFIG_OVERLAY_FS=y` (`meta-superbird/recipes-kernel/linux/linux-superbird/superbird.cfg:229`). [V-src]

## 2. Option 1: oe-core `overlayfs-etc`

### 2.1 Mechanism

- Enabled by the image feature `overlayfs-etc` (`IMAGE_FEATURES[validitems]` in `meta/classes-recipe/image.bbclass:43`; the class is in `IMGCLASSES` at `image.bbclass:24`; listed in `core-image.bbclass:44`). It is an image class, not a recipe class, and the docs say it must not be inherited directly. [V-src, V-doc]
- It adds `create_overlayfs_etc_preinit` to `ROOTFS_POSTPROCESS_COMMAND` (`overlayfs-etc.bbclass:34`). That function reads the template, formats it with the variables and writes a shell script, mode 0755 (`bbclass:48-90`). There is no systemd unit and no generator. [V-src]
- Two placement modes. Default `OVERLAYFS_ETC_USE_ORIG_INIT_NAME = "1"`: `/sbin/init` is renamed to `/sbin/init.orig` and the generated script is written as `/sbin/init` (`bbclass:80-85`). Setting it to `0` writes `/sbin/preinit` and you edit the boot command line yourself (`bbclass:63-65, 87-89`). Default mode sets `IMAGE_FEATURES_CONFLICTS_overlayfs-etc = package-management` (`bbclass:35`, evaluated at `image.bbclass:179`). [V-src]
- `/sbin/init` in this image is systemd's own link (`systemd_259.5.bb:675` lists `${base_sbindir}/init` in the systemd package). [V-src]

### 2.2 Variables

| Variable | Default | Effect | Source |
|---|---|---|---|
| `OVERLAYFS_ETC_MOUNT_POINT` | empty, fatal if unset | where the data device is mounted pre-init; the class creates it if `CREATE_MOUNT_DIRS` is 1 | `bbclass:37, 53-54` |
| `OVERLAYFS_ETC_DEVICE` | empty, fatal if unset | positional device argument to `mount` | `bbclass:39, 55-56` |
| `OVERLAYFS_ETC_FSTYPE` | empty, fatal if unset | `mount -t` | `bbclass:38, 57-58` |
| `OVERLAYFS_ETC_MOUNT_OPTIONS` | `defaults` | `mount -o` for the data device only | `bbclass:41` |
| `OVERLAYFS_ETC_INIT_TEMPLATE` | `${COREBASE}/meta/files/overlayfs-etc-preinit.sh.in` | template, formatted with Python `str.format`, so literal braces must be doubled | `bbclass:42, 60-61, 88` |
| `OVERLAYFS_ETC_EXPOSE_LOWER` | `0` | if 1, bind the original `/etc` read-only at `<mount>/overlay-etc/lower` | `bbclass:43`, template 33-38 |
| `OVERLAYFS_ETC_USE_ORIG_INIT_NAME` | `1` | see 2.1 | `bbclass:40` |
| `OVERLAYFS_ETC_CREATE_MOUNT_DIRS` | `1` | if 1, the script remounts `/` rw and `mkdir -p` the mount points | `bbclass:44`, template 6-13 |

The template file is the other source of truth, line numbers below are for `meta/files/overlayfs-etc-preinit.sh.in`. [V-src]

### 2.3 What the generated script does

1. If `CREATE_MOUNT_DIRS`: `mount -o remount,rw /`, then `mkdir -p /proc /sys /run /var/run <mount point>` (template 6-13).
2. Mounts `proc` and `sysfs` (15-16), sets `CONSOLE` default (18).
3. Layout: `BASE=<mount point>/overlay-etc`, `upper=$BASE/upper`, `work=$BASE/work`, `lower=$BASE/lower` (20-23).
4. `mount -n -t <FSTYPE> -o <OPTIONS> <DEVICE> <MOUNT_POINT>` (26-28). On success: `mkdir -p upper work` (30-31), optional `mount -o bind,ro /etc $LOWER_DIR` (33-38), then `mount -n -t overlay -o upperdir=...,lowerdir=/etc,workdir=...,index=off,xino=off,redirect_dir=off,metacopy=off ... /etc` (40-45). The lower is `/etc` itself, mounted over itself.
5. On overlay failure it prints `PREINIT: Mounting etc-overlay failed!` and goes on (46). On data mount failure it prints `PREINIT: Mounting </data> failed!` and goes on (48).
6. Unmounts `/sys` and `/proc` (51-52) and `exec {SBIN_INIT_NAME}` (55). **No `"$@"`**: the arguments given to the script are discarded.

The upper directory is created with the init process's umask, no chmod is done, and the overlay root takes its mode from the upper directory (the bug we already hit). With the default flow the directory is created fresh, so it is 0755 only if init's umask is 022 [U], and a pre-existing 0700 upper (our provisioning today does `chmod 0700` on the base and `0755` on the upper, `thing-revived-etc-mount.sh:30-31`) must stay 0755 [V-src for our script, U for the kernel behaviour, which I did not re-verify here].

### 2.4 Interaction with read-only root, custom init, systemd

- Read-only root: the default `CREATE_MOUNT_DIRS=1` remounts `/` read-write (template 7) and the script never remounts it read-only. Whether systemd's `systemd-remount-fs` restores `ro` for `/` when `fstab` has no `/` entry is not something I verified [U]. Our `fstab` has no `/` line (`base-files/fstab`). If it does not, a degraded boot would leave the root writable. Set `OVERLAYFS_ETC_CREATE_MOUNT_DIRS = "0"` and make `/proc`, `/sys`, `/run`, `/var/run` and the mount point exist in the rootfs. `/var/run` and `/run` come from base-files, the mount point must be shipped (`/var` already exists). [V-src for the variable, U for systemd behaviour]
- Custom init: the boot command line runs `/sbin/superbird-preinit` (`extlinux.conf:8`), which runs `exec /sbin/init --machine-id="$machine_id" "$@"` (`meta-superbird/recipes-core/superbird-machine-id/files/superbird-preinit.sh:12`). With the default placement, `/sbin/init` is now the generated script, which ends in `exec /sbin/init.orig` with no arguments, so **the `--machine-id=` argument is lost** and systemd falls back to its own machine-id handling. With `USE_ORIG_INIT_NAME=0` the generated `/sbin/preinit` also ignores its arguments. Either way a custom template is needed that ends in `exec <init> "$@"`, or the two scripts must be merged into one (our script computes the id from the efuse before the overlay, the overlay script would run after it). [V-src]
- systemd as init: the overlay exists before PID 1 reads anything, so units, `fstab`, `sshd_config.d`, `machine-id`, `nsswitch.conf` and `passwd` are all read through the overlay from the start. No unit is needed, no ordering list (`Before=systemd-tmpfiles-setup.service nss-user-lookup.target ... sshd.socket`), no cycle risk, no namespace propagation question, and no `ProtectSystem=` stacking (spikes (a), bullets on propagation and on the lost read-only protection). The upstream rationale is the same (class header `bbclass:9-11`, Bootlin write-up and the systemd-devel thread it cites). [V-src, V-doc]
- machine-id with `--machine-id=` forwarded: systemd's `machine_id_setup` writes the given id straight into `/etc/machine-id` if the file can be opened for writing and the caller did not ask for a transient id (`src/shared/machine-id-setup.c:218-` at v259, and the comment at line 225 for the transient case). On an overlay the file would be copied up and persist in the upper. So the id is stored, not bind-mounted, and the re-bind step in our script (`thing-revived-etc-mount.sh:43-60`) disappears. Whether the device currently ends up with a tmpfs bind or an upper file is covered by the brief (machine-id is in the upper today), so I did not test the new flow. [V-src for the setup code, U for the device]

### 2.5 The data device timing problem

- The script runs before udev and before `systemd-makefs`. The device argument must therefore resolve without udev. `/dev/disk/by-partlabel/data` does not exist yet. `mount PARTLABEL=data` is resolved by libblkid in util-linux, but I did not confirm which `mount` the image has (util-linux or busybox) nor that busybox supports `PARTLABEL=` [U]. A fixed node such as `/dev/mmcblk0pN` also needs `/dev` to be populated, which needs devtmpfs; I did not find `CONFIG_DEVTMPFS` in `superbird.cfg` (grep found no line, it may come from the base config) [U]. The data partition number is computed at run time by `sgdisk` (`superbird-provision.sh:35`), it is stable for a given image layout but I did not read the wks to name it [U].
- First boot and factory reset: the partition is absent (or blank) when the preinit runs. The mount fails, the script prints the message and boots with the plain read-only `/etc` (template 48). Then `superbird-provision`, `superbird-fsck` and `systemd-makefs` create and format `/var` as today, but `/etc` stays read-only until the next boot. First-boot password setup then reports account storage unavailable, which contradicts docs/users.md. [V-src]
- Workarounds, none free: (a) reboot once after provisioning (guarded by a marker on `/var`; adds one boot to first boot and to every factory reset, and a reboot loop risk if the marker write fails); (b) move partition carving and `mkfs` into the preinit (large change: it pulls `sgdisk`, `partx`, mkfs, the u-boot env reset check at `/mnt/uboot-env` and the headroom logic out of systemd units into an init script with no journal); (c) hybrid: late unit on first boot, early overlay afterwards (two mechanisms for one job, and the spikes already judged it as gaining little). [V-src for the facts, judgement is mine]
- The preinit mounts the data partition itself. If the mount point is `/var`, systemd later finds `/var` already mounted. That the `x-systemd.makefs` and `superbird-fsck` units behave sanely on an already mounted device is unverified [U]; `superbird-fsck.sh` explicitly skips checking a mounted device, so **the repair policy for `data` (e2fsck then `UNREPAIRABLE=leave`) is bypassed whenever the early mount succeeded**. ext4 still replays its journal on mount, but latent errors are no longer repaired at boot. If the early mount fails because of corruption, `superbird-fsck` then repairs `/var` later in the same boot, but the overlay is not retried. [V-src]
- Using a different mount point such as `/data` and bind-mounting `/data` over `/var` later would avoid the double ownership of `/var` but adds another mount to manage [U].

### 2.6 Failure modes and degraded mode

- There is no degraded mode beyond "log and continue" (template 46, 48). No retry, no marker file, no early journal entry (stdout goes to the console only, the journal is not up). Our controller can detect the state by reading `/proc/self/mountinfo` for an `overlay` mount on `/etc`, as `etc_is_overlay` does today (`thing-revived-etc-mount.sh:14-16`). [V-src]
- If the data device mounts but is corrupt in a way that mount succeeds, the overlay mounts and account files can be read wrong. If the overlay mount fails after the data mount succeeded, the data mount stays in place. Neither path is cleaned up. [V-src]
- Rootfs stays writable in degraded state if `CREATE_MOUNT_DIRS` is left at 1 (see 2.4).
- Power cut: same ext4 plus overlay behaviour as the late overlay (kernel overlayfs fsyncs the upper file before completing a data copy-up, kernel docs "Overlay Filesystem", [V-doc]), shadow-utils' own temp-file-and-rename is unchanged. Not tested on the device [U].

### 2.7 /etc modes

- Not handled by the class. The upper root is whatever `mkdir -p` produced. The overlay's root inode takes its mode, owner and times from the upper (the bug in the brief). With overlayfs-etc nothing re-asserts 0755. Our current script's `chmod 0755 .../upper` (line 31) would have to move into the custom template. [V-src for our script, U for the kernel rule beyond the brief]
- File modes inside the overlay are preserved on copy-up (`shadow` stays 0400 as noted in the script comment, lines 27-30). [U for the exact rule, matches the spike results]

### 2.8 Upstream known issues and limits

- Documented in the class header: updates and migration of `/etc` content under the overlay are out of scope (`bbclass:31-32`), and `package-management` conflicts in the default mode (`bbclass:21-25, 35`). [V-src]
- Template drops script arguments, uses `mount -n`, has no fsck, no retry, no `/dev` setup, uses fixed overlay options (`index=off,xino=off,redirect_dir=off,metacopy=off`, template 44). [V-src]
- machine-id and late-service problems with an `/etc` overlay mounted after PID 1 are the reason the class exists (Bootlin article and the systemd-devel thread). [V-doc]
- A Toradex forum thread and a Unix StackExchange answer report people moving to this class after failing with late mounts, and that an empty `/etc/machine-id` after a late overlay breaks networkd. That matches our spike (empty-id window). Single third-party sources, treat as less certain. [V-doc]
- The selftests (`meta/lib/oeqa/selftest/cases/overlayfs.py:271-530`, using `meta-selftest/files/wic/overlayfs_etc.wks.in`) cover a qemu wic with the data partition present at boot. They test the happy path, `EXPOSE_LOWER`, and the package-management conflict, not a missing or corrupt device. [V-src]
- I did not search the Yocto bugzilla or the openembedded-core mailing list archives for filed bugs against this class, so absence of other known issues is not evidence of none [U].

### 2.9 What the repo would change to adopt it

1. Image (`build/overlay/recipes-core/images/superbird-kiosk-prod-image.bbappend`): add `overlayfs-etc` to `IMAGE_FEATURES`; set in the image or the distro: `OVERLAYFS_ETC_MOUNT_POINT`, `OVERLAYFS_ETC_FSTYPE = "ext4"`, `OVERLAYFS_ETC_DEVICE` (a device string that resolves pre-udev), `OVERLAYFS_ETC_MOUNT_OPTIONS = "rw,noatime"`, `OVERLAYFS_ETC_CREATE_MOUNT_DIRS = "0"`, `OVERLAYFS_ETC_EXPOSE_LOWER = "1"` (replaces `/run/thing-revived/etc-lower`), `OVERLAYFS_ETC_USE_ORIG_INIT_NAME = "0"` and a custom `OVERLAYFS_ETC_INIT_TEMPLATE` in our layer.
2. Custom template: copy of the stock one that (i) ends with `exec /sbin/init "$@"`, (ii) does `chmod 0755` on the upper and keeps base dirs 0700, (iii) logs to `/dev/kmsg` instead of only stdout, (iv) leaves a marker (for example a file under `/run`) that the controller reads for degraded state, (v) does not touch `/` read-write. Braces must be doubled.
3. Boot chain: either merge the template into `superbird-preinit.sh` (preferred, one `init=`) or make `superbird-preinit` `exec /sbin/preinit --machine-id=... "$@"`. `superbird_preinit_check` in `meta-superbird/classes/superbird-image.bbclass:62-68` would need to check the new file too.
4. First boot: add the reboot-once unit (2.5a) or accept account storage only from the second boot, and update docs/users.md and the first-boot screen logic.
5. Delete `thing-revived-etc.service` and the mount script, the machine-id re-bind, the ordering lists and T0 (a) from the plan; keep the degraded detection. `thing-revived-provision.service` loses `Requisite=thing-revived-etc.service`.
6. Tests: G1d needs a boot with a blank data partition, a boot with a corrupt data partition, and a boot after the first-boot reboot.

Effort: 3 to 5 days of repo work plus device boots. Risk list: first-boot gap, fsck bypass, pre-udev device resolution, the single-init-chain merge, silent failure, and no way to test the pre-systemd script off the device except under `unshare` and a loop device.

## 3. Option 2: systemd userdb (and homed, sysusers, tmpfiles)

### 3.1 What is built into our image (checked in the recipes)

- `systemd_259.5.bb` default `PACKAGECONFIG` includes `nss`, `userdb`, `sysusers`, `pam` (via DISTRO_FEATURES), `logind`, `gshadow`, `wheel-group`, `utmp` for glibc (lines 55-82 of the recipe). `homed` is **not** in the default list (PACKAGECONFIG[homed] is defined at line 131 and checked at 852-853: homed also requires `userdb openssl cryptsetup`). No other layer in the checkout appends it (grep of `meta-superbird` and `build/overlay` for PACKAGECONFIG found nothing for systemd). So `systemd-homed`, `homectl` and `pam_systemd_home` are not in the image. [V-src]
- `userdbctl` is packaged (`systemd_259.5.bb:712`), `20-systemd-userdb.conf` for sshd is packaged (`:695`). `nss` pulls the runtime package `libnss-systemd` (`PACKAGECONFIG[nss] = "-Dnss-systemd=true,...,libnss-systemd"`, line 161). The recipe postinst adds `systemd` to the `passwd:`, `group:` and `shadow:` lines of `/etc/nsswitch.conf` (`systemd_259.5.bb:889-896`). It does not touch `gshadow:`. Base `nsswitch.conf` is `passwd: compat`, `group: compat`, `shadow: compat`, `gshadow: files` (`meta/recipes-core/base-files/base-files/nsswitch.conf`), so the effective lines are `compat systemd`. [V-src]
- `systemd-userdbd` is built (`ENABLE_USERDB`, `src/userdb/meson.build` at v259) with units `systemd-userdbd.service` and `.socket`. Whether those are enabled in our image is unverified [U]. The brief says `userdbctl` exists on the device, consistent with the above.
- PAM: `pam-plugin-unix` is in the systemd PAM_PLUGINS (recipe line 41). libpam is 1.7.2 with PACKAGECONFIG `audit selinux userdb` only (`libpam_1.7.2.bb:39-42`; its `userdb` flag is the unrelated Berkeley-DB pam_userdb), and the `pwaccess` option is not wired in the recipe, so pam_unix uses plain NSS (below). [V-src]

### 3.2 Where userdb reads records

- Drop-in directories, in lookup order: `/etc/userdb`, `/run/userdb`, `/run/host/userdb`, `/usr/local/lib/userdb`, `/usr/lib/userdb` (`USERDB_DROPIN_DIR_NULSTR` in `src/shared/userdb.h` at v259; the man page lists four, without `/usr/local/lib`). Files: `<name>.user`, `<name>.user-privileged` (root only, 0600), `<uid>.user` symlinks, `<name>.group`, `.group-privileged`, and `<user>:<group>.membership` (contents ignored, existence is the membership). [V-src, V-doc]
- The static records are read by the client library itself (`src/shared/userdb-dropin.c`, `src/shared/userdb.c:1069-1075`), so nss-systemd works without `systemd-userdbd` running. nss-systemd calls `userdb_by_name` with `USERDB_EXCLUDE_NSS` so it does not recurse into itself (`src/nss-systemd/userdb-glue.c:16-24`). The man page says the same ("also works without the service running"). [V-src, V-doc]
- `/etc/userdb` as a symlink to `/var/lib/thing-revived/userdb`: lookups open `<dir>/<name>.user` by path with `search_and_fopen_nulstr` (`userdb-dropin.c:128, 158`), which follows directory symlinks. I did not find any code that rejects a symlinked directory, and enumeration uses `conf_files_list_nulstr` with `CONF_FILES_REGULAR` (`userdb.c:1069-1075`), which I did not read to the end [U for enumeration through a symlinked directory]. A dangling symlink (no `/var`) is a missing directory, the next directory in the list is used, so a baked `/usr/lib/userdb/kiosk.user` would be the degraded fallback. First match wins by the name of the function and the man page ("static user/group records generally do not override conflicting records in /etc/passwd"), the exact precedence between directories I did not test [U].
- Important: static drop-ins never override `/etc/passwd`. `kiosk` is created by `extrausers` in the rootfs (`superbird-kiosk-prod-image.bbappend:108-109`), so its locked entry in `/etc/passwd` and `/etc/shadow` wins under `compat systemd`. The userdb option therefore requires moving `kiosk` out of the rootfs databases and into a baked `/usr/lib/userdb` record, and the same for the `sudo` and `adm` memberships. [V-doc, V-src for the extrausers part]

### 3.3 Does pam_unix, sshd, sudo, login work with userdb-only users

- pam_unix reads the account with `getpwnam` through NSS (`modules/pam_unix/passverify.c:203`, `#define pam_modutil_getpwnam(h,n) getpwnam(n)` at line 44 for the helper build). If `pw_passwd` is the shadow marker `x` it takes the shadow data from `getspnam` (line 232) or runs the setuid helper `unix_chkpwd`, which does the same `getspnam` (`HELPER_COMPILE` branch, lines 209-240). `PAM_UNIX_TRY_GETSPNAM` is a meson option (`meson.build:56`), when off, the module always runs the helper. [V-src]
- nss-systemd returns `pw_passwd = x` for all its users (`userdb-glue.c:63-64`, `PASSWORD_SEE_SHADOW`) and answers `getspnam` from the record: the hash is `hashedPassword[0]` of the record's `privileged` section, or `!*` (locked and invalid) if none (`userdb-glue.c:152-176`). `sp_expire` is 1 for `locked: true` (`userdb-glue.c:167`). If the caller cannot read the `.user-privileged` file (not root), the record is marked incomplete and `getspnam` returns not found (`userdb-glue.c:204-205`, `userdb-dropin.c:43-102`), so the hash is only visible to root, the same posture as `/etc/shadow` 0400. [V-src]
- Therefore sshd (PAM `account` and `auth` through `pam_unix`, run as root in the monitor), `login`, `su` and `sudo` (PAM service `sudo` includes `common-auth` and `common-account`, `meta/recipes-extended/sudo/files/sudo.pam`) authenticate userdb users with **pam_unix and nss-systemd only**. Neither sshd nor sudo links against systemd's code, both call `getpwnam`, `getgrnam` and `getgrouplist` and rely on NSS. The `shadow:` line must contain `systemd`, which the recipe already adds. `pam_systemd_home` is only for homed users and is not needed. [V-src for pam_unix, V-doc for the NSS flow, U for sudo and sshd internals which I did not read]
- Group membership of `sudo` for a userdb user: the `.membership` file `kiosk:sudo.membership` plus a `sudo.group` record or an existing rootfs `sudo` group. With the rootfs group in `/etc/group` (`compat`) and nss-systemd after it, whether the two are merged depends on glibc's merge support for `compat`, which I did not verify [U]. The safer layout is to define `sudo` and `adm` fully in userdb.
- `pam_unix` password changes (`passwd` as a user) write `/etc/passwd` and `/etc/shadow` directly (`passverify.c:859, 920, 979, 1060`), so they fail for userdb users. Phase 2 does not use that path (it sets passwords from the controller), but any user-driven `passwd` in a shell as `kiosk` will fail. [V-src]
- SSH authorized keys can also live in the record (`userdbctl ssh-authorized-keys`, wired by `20-systemd-userdb.conf` as `AuthorizedKeysCommand`, [V-doc]). The plan keeps a file store, so no change needed. The existing plan note that `10-` drop-in beats `20-systemd-userdb.conf` stays valid.

### 3.4 Tools

| Operation | shadow-utils (current) | userdb |
|---|---|---|
| create or remove user | `useradd`, `userdel` edit `/etc/passwd`, `/etc/shadow` (`PASSWD_FILE "/etc/passwd"`, shadow `lib/defines.h:139-140`) | no tool. Write `<name>.user`, `<uid>.user` symlink, optional privileged file |
| set password | `chpasswd -c SHA512` | write `privileged.hashedPassword` ourselves (crypt(3) or a crate) |
| lock or expire | `usermod -L`, `-e` | `locked`, `notAfterUSec` fields |
| groups | `groupadd`, `gpasswd`, `usermod -aG` | `.group` files and `<user>:<group>.membership` files |
| inspect | `getent` is not in the image | `userdbctl user|group|users-in-group|groups-of-user` (read only, [V-doc]) |

`userdbctl` has `--from-file` to validate a record offline (since 258), but no add, modify or remove verb. `homectl` could create regular users (`homectl create --storage=directory`) but is not built, needs openssl and cryptsetup, manages `/home` images and signs records, which is much heavier than our need. `systemd-sysusers` writes `/etc/passwd` and `/etc/group` (needs a writable `/etc`, cannot target userdb), and `systemd-tmpfiles` can create directories and files but not records. `systemd-userdb-load-credentials.service` can install records from system credentials into `/etc/userdb` at boot (`userdb.user.*`), which is a seed path, not a runtime writer. [V-doc]

Existing shadow tools against userdb-only users: they read the database files, not NSS, so `usermod` and `chpasswd` on a userdb user fail with "does not exist"; `useradd` and `groupadd` check for collisions through `getpwnam` and `getgrnam` so they see userdb entries for uid and name conflicts. This follows from shadow's `pwio.c` using `PASSWD_FILE` and `commonio`, I did not run it [U].

### 3.5 Work for Phase 2

- Rewrite or replace `controller/src/adapters/accounts_shadow.rs` with a record store: schema (userName, uid, gid, realName, homeDirectory, shell, disposition `regular`, memberOf, `privileged.hashedPassword`, `locked`, `lastPasswordChangeUSec`), atomic writes (temp file in the same directory plus rename, fsync of the directory), a lock, uid and gid allocation (we own it, shadow's `useradd` allocation is gone), group membership files, removal of stale symlinks, and offline validation with `userdbctl --from-file` in tests. Password hashing in-process (sha-crypt, the plan's SHA-512 rounds) replaces `chpasswd -c/-s`.
- Rootfs: `kiosk`, `sudo`, `adm` move to baked `/usr/lib/userdb` records, extrausers and the `sudoers` group assumptions in `check_image_accounts.py` change, and `/etc/userdb` becomes a build-time symlink into `/var` (created by the recipe or tmpfiles).
- Boot order: nothing mounts over `/etc`. The only dependency is that `/var` is mounted before the first lookup of a userdb user (`sshd.socket`, the controller, the terminal agent need `RequiresMountsFor=/var`). The empty-machine-id window, namespace propagation, unit cycles and the `Before=` list in section 3 of the plan disappear. [V-src for the lookup path]
- sudoers: files in `/etc/sudoers.d` still need a writable location if Phase 2 adds per-user sudoers entries. A symlink `/etc/sudoers.d` to a `/var` directory works for plain file writes in that directory (the write happens in the target directory, unlike the shadow lock files), but sudo's checks on ownership and mode of included files and directory I did not read [U]. Group membership of `sudo` can avoid per-user sudoers files completely.
- Tests: the host tests that fake `/etc` via `useradd -P` (spikes class B) change to record fixtures. The G1d device checklist gets `userdbctl user kiosk`, `getent` is absent so use `userdbctl`.
- Effort: roughly 8 to 15 days including the replaced tests and the image seed change, against 3 to 5 for adopting overlayfs-etc and zero for the status quo.

### 3.6 Failure modes and risks

- A malformed record, a wrong mode on `.user-privileged`, or a missing uid symlink makes the user vanish or the hash unreadable; the PAM stack then returns an authentication error, not a crash. Conflicting uids between records (userdb has no collision handling, `USER_GROUP_API` says clients do not merge or detect conflicts) cause lookup errors. [V-doc]
- `/var` missing or corrupt: the baked `/usr/lib/userdb` records and the rootfs `/etc` still resolve, no mount to fail, no hang. This is a cleaner degraded mode than either overlay. [V-src for the search list, U for the exact fallback behaviour]
- nss-systemd talks to `systemd-userdbd` over Varlink if present, with timeouts; the service is sandboxed with `ProtectSystem=strict` (read-only is fine for reads, `units/systemd-userdbd.service.in` at v259). A hung userdbd is an NSS delay for every lookup, same class of risk as any NSS module [V-src for the unit, U for delay behaviour].
- Static glibc binaries (the controller is described as statically linked in the plan, row D) cannot use NSS, so the controller must parse the records itself, which it would do anyway as the writer.
- Every program that edits `/etc/passwd` directly (busybox `adduser`, `passwd`, `chsh`, package postinstalls) stops affecting userdb users, and they are unaffected by `/var` content. This is an expectation gap rather than a break.
- The `shadow:` NSS lookup for a user in `/etc/passwd` but not in `/etc/shadow` (a half-migrated `kiosk`) returns not found from files and then goes to nss-systemd, a source of confusing states during migration [U].

### 3.7 Security effect (single-factor kiosk password)

Unchanged in kind: one hash, verified by pam_unix or `unix_chkpwd`, SHA-512 as set by our writer, faillock unchanged because it is PAM. The hash file is `.user-privileged`, 0600 root (`nss-systemd(8)`), same exposure as `/etc/shadow` 0400. New exposure: our controller becomes the only code that creates records and hashes, so a bug in it is a direct account bypass (a user record with an empty or wrong `hashedPassword` or without `locked`); with shadow-utils the tool did that. Validation with `userdbctl --from-file` in the controller tests and a post-write read back through `getpwnam` and `getspnam` reduce it.

## 4. Option 3: tmpfs or overlay only on the handful of paths

Confirmed not workable. shadow-utils creates `<db>.<pid>`, `<db>.lock` and `<db>+` next to the database (`lib/commonio.c:369` for the pid file and lock, `:489` for the lock, `:942` for `passwd+`), so the directory containing the file, `/etc`, has to be writable. Bind-mounting writable files over `passwd` and `shadow` fails exactly as the device test showed (cannot lock `/etc/passwd`). An overlay or tmpfs can only be mounted on a directory, and the only directory containing these files is `/etc`. A tmpfs would also lose the data at reboot. Sub-directory overlays (`/etc/sudoers.d`, `/etc/ssh/sshd_config.d`, `/etc/pam.d`) are possible but only for non-database files. [V-src, device test from the brief]

## 5. Option 4: relocating or symlinking the databases

| Variant | Why it fails or what it needs | Tag |
|---|---|---|
| 4a. `/etc/passwd` and friends as symlinks into `/var` | shadow `lrename` resolves the symlink target at rename time (`lib/commonio.c:76-95`), but the lock, the `.pid` file and the `passwd+` temp file are built from the unresolved name, so they are created in `/etc` (lines 369, 489, 942). With `/etc` read-only the lock fails. With `/etc` writable (overlay) the temp file is on the root or overlay filesystem and the rename target is on `/var`, a cross-device rename returns `EXDEV` | V-src |
| 4b. pam_unix options | pam_unix has no option for an alternate passwd or shadow path. Reads go through NSS and writes use literal `/etc/passwd`, `/etc/shadow`, `/etc/passwd+`-style paths (`passverify.c:857-860, 920, 979, 1060`; `.pwdXXXXXX` temp under `/etc` at line 545) | V-src |
| 4c. rebuild shadow-utils with `-DPASSWD_FILE=...` | The paths are `#ifndef` macros (`lib/defines.h:139-144`), so a build-time override is possible and the lock and temp files would then follow the new directory. But every other consumer of `/etc/passwd` (glibc NSS files, `login`, `su`, pam_unix writes, sshd, sudo) would still read `/etc` unless also rebuilt or reconfigured, and glibc's `nss_files` has no run-time path option that I know of [U for glibc]. Reject: a fork of the account stack | V-src for macros, U for glibc |
| 4d. NSS `db` (`libnss_db`, `/var/db/*.db`) or `libnss-extrausers` | glibc ships `libnss-db` as a package (`glibc-package.inc:36`, files `/var/db/Makefile` and `makedbs.sh`), so `passwd: files db` could read a database under `/var`. Needs a writer that rebuilds the `.db` files after every change (`makedb`, whether it is in our image is not verified [U]) and cannot be driven by shadow-utils. No `libnss-extrausers` recipe exists in the checkout (`find` for `*extrausers*` finds only `meta/classes/extrausers.bbclass`, the build-time class). Same cost class as userdb with a worse record format and no upstream-supported drop-in directory | V-src for what exists, U for runtime |
| 4e. `shadow-utils -R/-P` | `useradd -P <prefix>` makes the tools operate on `<prefix>/etc/passwd` (`lib/prefix_flag.c:115-118`), which is how the host tests run them. The system still reads `/etc/passwd`, so the prefix tree is not seen by login unless it is the real `/etc` | V-src |

Only 4a through 4e exist as option classes besides overlay and userdb. None is viable on its own.

## 6. Comparison

| Criterion | Current: late overlay | 1. overlayfs-etc (custom template) | 2. userdb | 3. tmpfs or overlay on paths | 4. relocated or symlinked databases |
|---|---|---|---|---|---|
| Mechanism | systemd unit after `/var`, overlay on live `/etc` | pre-systemd init script mounts data and overlay | JSON records in `/etc/userdb` (symlink to `/var`), nss-systemd and pam_unix | cannot hold account databases | symlinks, build-time paths, or `nss_db` |
| Phase 2 code | unchanged (shadow-utils tools) | unchanged | `accounts_shadow.rs` rewritten, record writer, hashing, locking | n/a | n/a |
| Image and repo change | already done | new feature and variables, custom template, init chain merge, remove unit and script | seed `kiosk`, `sudo`, `adm` as baked records, `/etc/userdb` symlink, `extrausers` change | n/a | n/a |
| Boot order | late, ordering list, machine-id window, namespace stacking | before PID 1, none of those | no `/etc` mount, only "after `/var`" | n/a | n/a |
| First boot | works (`/var` exists after provision) | `/var` absent, no overlay until next boot | works | n/a | n/a |
| Corrupt or missing `/var` | degraded by unit failure, kiosk locked | degraded by log line, no retry, fsck bypassed | falls back to baked records, no mount to fail | n/a | n/a |
| Security effect | same hash file, root 0400 | same | same posture, new bug surface in our writer | n/a | n/a |
| Effort | done, plus Class C tests | 3 to 5 days plus device boots | 8 to 15 days | n/a | n/a |
| Top risks | Class C unknowns (shutdown, power cut), `ProtectSystem=` stacking, machine-id window | first-boot gap, fsck policy bypass, pre-udev device resolution, dropped `--machine-id` arg, rootfs left rw if misconfigured | malformed record locks out, merge semantics of `sudo` group, uid allocation bugs | n/a | n/a |
| Verdict | baseline | upgrade path if Class C fails | named fallback | rejected | rejected |

## 7. Recommendation

- Keep the late overlay as the Phase 2 design. It is the only option that leaves the Phase 2 adapters on stock shadow-utils with no new record format and no rewrite, and its known risks are boot-time facts that the Class C checklist in the spikes report already targets.
- Adopt `overlayfs-etc` instead only if the Class C tests show an unfixable problem with the late overlay (machine-id window in a service we cannot reorder, shutdown busy `/var`, `ProtectSystem=` stacking mattering for a unit we care about). The package is: custom template with `"$@"`, `CREATE_MOUNT_DIRS=0`, `EXPOSE_LOWER=1`, a pre-udev device string, merge with `superbird-preinit`, and a reboot-once step after provisioning or a factory reset. Plan the fsck policy change explicitly, because the early mount makes `superbird-fsck` a no-op on a healthy mount.
- Keep userdb as the fallback already named in the plan. Do not start it speculatively, because its cost is the whole writer.
- Reject options 3 and 4. The directory-write requirement is in shadow-utils source, not in our configuration.

## 8. Claims I could not verify

- Which `mount` (util-linux or busybox) is in the image and whether it resolves `PARTLABEL=data` before udev; whether `/dev` is populated at preinit time (no `CONFIG_DEVTMPFS` line found in `superbird.cfg`); the data partition number.
- Whether `systemd-remount-fs` restores `ro` on `/` after the preinit's `mount -o remount,rw /` when `fstab` has no `/` entry.
- Behaviour of `x-systemd.makefs`, `var.mount` and `superbird-fsck` when `/var` is already mounted by the preinit.
- The init process umask (assumed 022) and the exact kernel overlay rule for the root inode mode beyond what the brief states.
- Whether `bind,ro` in the template yields a read-only lower with the image's `mount`.
- Whether `systemd-userdbd.service` and `.socket` are enabled in the image; nss-systemd client behaviour when the service is down but the drop-ins exist (the man page says it works).
- Directory precedence between `/etc/userdb` and `/usr/lib/userdb` for the same user name, enumeration through a symlinked `/etc/userdb`, and merging of `/etc/group` with userdb groups under `compat`.
- sudo's checks on a symlinked `/etc/sudoers.d` and on included file ownership; whether sshd or sudo have any further direct dependency on nss-systemd beyond NSS lookups.
- Runtime behaviour of shadow `usermod` and `chpasswd` against userdb-only users (derived from source, not executed), and of `useradd` collision checks.
- Whether glibc 2.43 `nss_files` or `compat` accepts any alternate database path; whether `makedb` is in the image.
- Upstream bug reports for `overlayfs-etc` (bugzilla and mailing list were not searched). The "available since Yocto 4.0" statement comes from a single StackExchange snippet.
- On-device timing of a first-boot reboot and its effect on the first-boot UX.
- Power cut and shutdown behaviour of every option (all need Class C boots).

## 9. Sources

Checkout (read only), all under `/home/andremmfaria/workspace/yocto-superbird-build-2`:

- `sources/openembedded-core/meta/classes-recipe/overlayfs-etc.bbclass` (lines 34-90 as cited), `meta/files/overlayfs-etc-preinit.sh.in` (lines 6-55), `meta/classes-recipe/overlayfs.bbclass:51-52`, `meta/files/overlayfs-unit.mount.in`, `meta/files/overlayfs-all-overlays.service.in`, `meta/lib/oe/overlayfs.py`, `meta/classes-recipe/image.bbclass:24,43,179`, `meta/classes-recipe/core-image.bbclass:44`, `meta/lib/oeqa/selftest/cases/overlayfs.py:271-530`, `meta-selftest/files/wic/overlayfs_etc.wks.in`
- `sources/openembedded-core/meta/recipes-core/systemd/systemd_259.5.bb` (55-82, 131, 161, 195-198, 695, 712, 852-853, 889-896), `meta/recipes-extended/pam/libpam_1.7.2.bb:39-42`, `meta/recipes-core/base-files/base-files/nsswitch.conf`, `meta/recipes-extended/sudo/files/sudo.pam`, `meta/recipes-core/glibc/glibc-package.inc:36`
- `meta-superbird/recipes-bsp/superbird-extlinux/files/extlinux.conf:8`, `meta-superbird/recipes-core/superbird-machine-id/files/superbird-preinit.sh`, `meta-superbird/recipes-core/superbird-provision/files/superbird-provision.{sh,service}`, `meta-superbird/recipes-core/superbird-fsck/files/superbird-fsck.{sh,service}`, `meta-superbird/recipes-core/superbird-factory-reset/files/superbird-factory-reset.service`, `meta-superbird/recipes-core/base-files/base-files/fstab`, `meta-superbird/classes/superbird-image.bbclass:62-68`, `meta-superbird/conf/distro/superbird.conf`, `kas/base.yml`
- Our repo: `/home/andremmfaria/projects/thing-revived/build/overlay/recipes-extended/thing-revived-accounts/files/thing-revived-etc-mount.sh` and `thing-revived-etc.service`, `docs/users.md`, `docs/research/user-management-plan.md` (rows C, D, H and section 3), `docs/research/user-management-spikes.md` (spike a)

Upstream source (read at tag, raw GitHub):

- systemd v259: `src/nss-systemd/nss-systemd.c`, `src/nss-systemd/userdb-glue.c`, `src/shared/userdb.c`, `src/shared/userdb.h`, `src/shared/userdb-dropin.c`, `src/shared/machine-id-setup.c`, `units/systemd-userdbd.service.in`, `src/userdb/meson.build`
- linux-pam v1.7.2: `modules/pam_unix/passverify.c`, `modules/pam_unix/support.c`, `meson.build`
- shadow 4.19.4: `lib/commonio.c`, `lib/defines.h`, `lib/prefix_flag.c`, `lib/pwio.c`

Docs and man pages (fetched 2026-11-23):

- https://www.freedesktop.org/software/systemd/man/latest/nss-systemd.html
- https://www.freedesktop.org/software/systemd/man/latest/systemd-userdbd.service.html
- https://www.freedesktop.org/software/systemd/man/latest/userdbctl.html
- https://systemd.io/USER_RECORD/ and https://systemd.io/USER_GROUP_API/
- https://docs.yoctoproject.org/ref-manual/classes.html#overlayfs-etc (6.1-tip docs, last updated 2026-11-20)
- https://bootlin.com/blog/systemd-read-only-rootfs-and-overlay-file-system-over-etc (undated article, less certain, used for the machine-id and ordering rationale)
- https://docs.kernel.org/filesystems/overlayfs.html (via search snippet only)
- Search snippets only, not opened: unix.stackexchange.com/questions/772629, community.toradex.com/t/automount-overlay-for-etc/15529
