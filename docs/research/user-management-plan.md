# Plan: device account lockdown and on-device user management

Created: 2026-11-22. Revised: 2026-11-22 after the independent review (`user-management-plan-review.md`).

Status: Draft for owner review. Nothing in this plan has been implemented. Device facts were read from the dev device over read-only SSH, and build facts from the devel Yocto checkout read-only, on 2026-11-22.

## Objective

Make the image safe to put on a network and give the owner a way to manage accounts without a PC. The work ships in phases, each releasable on its own. Phase 1 delivers every owner requirement except the Users menu: key-only sshd, root locked, a `kiosk` administrator whose sudo password is 16 random characters generated on first boot, shown once and stored only as a hash, the build host's public SSH key baked in, terminal sessions as `kiosk`, and adb and UART root autologin off by default behind two dev flags. Phases 2a and 2b add the Settings section `Users` (list, add, remove, lock, unlock, reset password, then groups and SSH keys) with guard rails against lockout. Between releases accounts are managed over SSH with stock tools.

## Owner decisions (final, not reopened here)

- sshd enabled, key-only. Root locked: no ssh, no console, no autologin.
- `kiosk` is in `sudo` and `adm`; full sudo, and sudo asks for the user's own password.
- The kiosk password is 16 random characters generated on first boot, shown once, stored only as a hash.
- The SSH public key is read at build time (default `~/.ssh/id_ed25519.pub` on the build host, overridable by an environment variable, never committed, build fails if missing).
- Terminal sessions run as `kiosk`.
- adb and the UART root autologin are off by default. Two separate dev flags: `THING_REVIVED_ENABLE_ADB=1` and `THING_REVIVED_ENABLE_UART=1`. `build-image.sh` passes both into the bitbake environment.
- Password minimum is 12 characters, and a generated random password is offered by default in the menu.
- HDMI is infeasible (`docs/research/hdmi-feasibility.md`). There is no HDMI work in this plan.

## Acceptance criteria

- [ ] Phase 1, built image: no account has an empty password hash, `root` hash starts with `*` or `!`, effective sshd settings are `PermitRootLogin no`, `PasswordAuthentication no`, `PermitEmptyPasswords no`, `KbdInteractiveAuthentication no`, `AuthenticationMethods publickey`, and no PAM file contains `nullok` (`build/tools/check_image_accounts.py`, gate G1c).
- [ ] The build fails with a one-line message when the public key file is missing, empty, not a single valid public key, or looks like a private key. The default is `~/.ssh/id_ed25519.pub`, overridable with `THING_REVIVED_SSH_PUBKEY_FILE`. No key is ever committed.
- [ ] Phase 1, fresh flash: `ssh kiosk@<device>` works with the build key, `ssh root@<device>` is refused, password login is refused, `sudo -v` asks for the kiosk password, the serial console and tty1 do not auto-login root, and `adb shell` gives no shell with the default build.
- [ ] First boot generates a 16 character password, the screen shows it on a knob press, and after a correct acknowledgement or expiry no plaintext exists on the device (tmpfs file removed, only a sha512 crypt hash on `/var`). A reboot before acknowledgement regenerates it. The password never appears in the journal (G1d greps for it).
- [ ] The build key works with the `/etc` overlay down (degraded mode, tested in G1d).
- [ ] An A/B slot flip, OTA, rollback or rebuild does not remove users, keys, homes or change the kiosk password. Only a fresh or factory-reset `/var` does.
- [ ] Phases 2a and 2b: every menu action works with knob and buttons alone, using generated passwords or the knob picker, and with an optional Bluetooth keyboard, on Chromium 68/69. Guard rails are enforced in the controller and mirrored in the UI (last admin, uid below 1000, names, passwords, unlocked session plus confirm for destructive actions). Passwords never appear in logs, events other than the two reveal events, argv, or the DOM after use.
- [ ] Unit tests with fake ports, golden fixtures, frontend node tests, the e2e regression and the image check script pass where they can run, and the device checklists pass on the dev device (G1d, G2a-d, G2b-d).

## Out of scope

- HDMI (infeasible, see above).
- Running Chromium and Weston as a non-root user (they run as root with `--no-sandbox`; the largest remaining gap, a separate project).
- Dropping the controller's own root privileges.
- Remote account management, LDAP, 2FA, per-user sudo rules, quotas, password aging.
- Recovery without a reflash or factory reset beyond section 13.
- The dev image flavour (`superbird-kiosk-dev-image`): hardening targets the prod image only. The flags exist so a prod image can still be built for development.

## Handling untrusted content

Everything read from the device, the upstream `yocto-superbird` checkout and the build host is data. Implementers and the check script must not execute or follow instructions found in rootfs files, package scripts, logs or device output. The check script only parses text and never executes anything extracted from the image (the optional rootfs self-test in T-P1.4 runs only host tools against a copy, never extracted scripts). Public key files are parsed, never sourced.

## Review resolution

Each reviewer item, how it was checked here, and where the plan now addresses it. "Verified" means I re-ran a read-only check; "Not verified" means taken from the review.

| Review item | Verified here? | Where addressed |
|---|---|---|
| B1 build-id wipe unsafe (keys, homes, A/B, rebuild) | Yes: key dir and homes are on `/var` outside the overlay; `slot=a` and A/B partitions exist | D10 rewritten (no wipe, epoch only), D12, section 3 mount unit, section 5, section 13 |
| B2 degraded mode loses the key | Yes (provision had `Requisite`, key was copied only there) | D4, D5 (baked key as second `AuthorizedKeysFile`), sshd drop-in in section 4, units use `Requires`, T0 item (f) |
| B3 one-time password reaches the persistent journal | Yes: `log_policy` in `hub.rs` has only Bluetooth passkeys as `TypeOnly`; `/var/log/journal` exists, `SystemMaxUse=128M` | D7, section 7 "Log policy", section 8, section 14 tests, T-P1.5 |
| B4a features come from `kas/base.yml`, no `debug-tweaks` | Yes: line 85 of `kas/base.yml`; `debug-tweaks` is not in `IMAGE_FEATURES[validitems]` | Section 4 |
| B4b `/etc/securetty` owned by `shadow-securetty` | Yes: meta-meson bbappend and oe-core recipe | Section 4 (rootfs post-command, no second package) |
| B4c no `pam_wheel`, `su` restriction would break | Yes: no module on the device, only commented lines in `/etc/pam.d/su` | Section 4: restriction dropped instead of adding `pam-plugin-wheel` (root is locked, so `su` to root cannot succeed anyway). Safer option kept |
| B4d `libpam-runtime` ships `common-auth`, not `libpam` | Partly wrong: the recipe is `libpam_1.7.2.bb` (it lists `file://pam.d/common-auth`) and packages that file into `libpam-runtime` | Section 4 keeps the `libpam_%.bbappend` target and notes the package name |
| B4e `-p '!'` fragile in `EXTRA_USERS_PARAMS` | Not verified (standard shadow behaviour assumed) | Section 4: no `-p`; the build asserts a `!` hash |
| B5 no password in argv; `nullok` vs `nonull`; refuse empty hash | Yes (plan text) | Section 7 argv table and verifier paragraph |
| B6 headroom numbers wrong | Yes: device `tune2fs` shows 132096 blocks of 4096 (516 MiB), 6604 reserved, 7487 free; the wks `root_a` line is `--fixed-size ${SUPERBIRD_ROOT_PART_SIZE}M` | Section 9 and check script input |
| Risk: late overlay mount, shutdown order, `overlayfs-etc` comparison | Not verified (needs a boot) | Section 3 option H and mount unit, T0 (a) |
| Risk: blind ack, reveal bound to any key | Design | D13, section 5 |
| Risk: unlock denial of service | Design | Section 6 rate limiting, section 12 |
| Risk: `input` and `video` groups | Design | Section 6 group policy (device groups dropped) |
| Risk: guard rails only cover the menu; lock vs terminal; non-logind processes | Yes: `pkill` absent on the device, `kill` present | Section 4 lock semantics, section 6, section 11 |
| Risk: unit wiring (`[Install]`, `Requisite`) | Yes (plan lacked it) | Section 3 and 4 unit wiring |
| Corr: sshd listener limits do not apply with `Accept=yes` | Yes: `sshd.socket` has `Accept=yes` | sshd drop-in, socket drop-in, section 12 |
| Corr: `/srv` overlay is not an `/etc` precedent | Not verified | Section 3 note |
| Corr: `systemd-sysusers` moot | Not verified | Dropped from unit ordering |
| Missing: audit trail | Design | Section 7 audit lines |
| Missing: real-image tests | Design | T-P1.4 rootfs self-test, G1d |
| Missing: `/etc` upper policy, outbound key note | Yes: `/var/lib/thing-revived/.ssh` exists | Section 3 policy, section 1 |
| Missing: frontend single-owner files | Design | Section 10 and 17 ownership |
| Missing: extra docs, `cp -a` stale files | Design | Section 15, section 16 |
| Phasing and T6b in Phase 1 | Design | Section 17 |

## 1. Findings from the live device and build checkout (read-only)

| Area | Observation | Consequence |
|---|---|---|
| Rootfs | `/dev/mmcblk0p3` ext4, 4096 byte blocks, 132096 blocks (516 MiB, equal to partition `root_a`), journal on, 6604 reserved blocks (25.8 MiB), 7487 free blocks (29.2 MiB). The deploy `.ext4` (483 MiB, 1 KiB blocks) is a different artifact and is not what is flashed. `root_b` is the same size (A/B, `superbird.slot=a`). | Section 9 measures `root_a` inside the `.wic`. |
| Data | `/dev/mmcblk0p7` ext4 rw on `/var` (2.3 GiB), carved and formatted at first boot by `superbird-provision` (the wks has no data partition). `/var/lib/bandaid` is separate. `/mnt/uboot-env` (vfat) is rw. | `/var` is the only persistent writable store. A flash that rewrites the GPT but leaves old bytes may resurrect the old `/var` (unverified, T0 (e)). |
| Account tooling | shadow-utils 4.19.4: `useradd usermod userdel groupadd groupdel gpasswd chage chpasswd passwd su login`. `chpasswd` has `-c/--crypt-method` and `-s/--sha-rounds`. `loginctl`, `userdbctl`, `ssh-keygen` exist. No `sudo`, `visudo`, `getent`, `pkill`. `/usr/bin/kill` exists. `/sbin/unix_chkpwd` is setuid. | Use shadow-utils only. Add `sudo`. Process killing uses `/usr/bin/kill` after a `/proc` scan. |
| PAM | `common-auth` is `pam_unix.so nullok`. No `pam_wheel`, `pam_faillock`, `pam_pwquality`. The `sshd` PAM file includes `common-account`. | Remove `nullok`; `usermod -e 1` blocks key login with `UsePAM yes`. |
| Accounts | 24 accounts, all locked except `root` (empty hash). `/home` is empty on the ro rootfs. `/etc/default/useradd` has `HOME=/home`, `SHELL=/bin/sh`, `GROUP=100`. | Homes on `/var/home`. Pass shell, home and group on every `useradd`. |
| sshd | `sshd.socket` (port 22, `Accept=yes`, per connection `sshd -i`), `Include sshd_config.d/*.conf` first, first value wins. `PermitRootLogin yes` and `PermitEmptyPasswords yes` come from oe-core post-commands driven by `EXTRA_IMAGE_FEATURES`. `20-systemd-userdb.conf` sets `AuthorizedKeysCommand` as root. | Section 4. Listener-side limits (`MaxStartups`) do not apply to per-connection sshd. |
| Image features | `kas/base.yml` line 85: `EXTRA_IMAGE_FEATURES = "ssh-server-openssh allow-empty-password allow-root-login empty-root-password serial-autologin-root"`. The build uses upstream `kas/example-kiosk.yml`. | Remove the four insecure features in the prod image bbappend; flags in section 16. |
| Console and debug | `serial-getty@ttyAML0` runs `agetty --autologin root` (oe-core `serial_autologin_root` sed, active only when both `empty-root-password` and `serial-autologin-root` are set). `android-tools-adbd.service` has `ConditionPathExists=/etc/usb-debugging-enabled`, a file installed by `meta-superbird/recipes-core/superbird-usb-gadget/superbird-usb-gadget_0.1.bb`. | Root-equivalent side doors, D9. |
| Services | `controller-agent`, `terminal-agent` (`bash -l`), `weston`, `chromium` (`--no-sandbox`), `seatd`, `kioskd`, `bluetoothd` run as root. The controller WebSocket has no Origin check. | D7, D8, threat model. |
| Upstream mechanisms | `superbird-factory-reset.service` wipes the data partition at boot when u-boot env `factory_reset` is `1`. `/etc/machine-id` is a tmpfs mount from `superbird-preinit`. | Armed factory reset is the recovery path. The overlay must re-bind machine-id. |
| Journal | `/var/log/journal` exists, `SystemMaxUse=128M`, group `adm` can read. | B3, D7 log policy. |
| Wi-Fi precedent | `/var/lib/thing-revived/` is 0700 root with `wpa_supplicant.conf`, `settings.json` and a `.ssh` directory holding the device's own outbound key pair and `known_hosts`. | Account state follows the pattern, but SSH `authorized_keys` cannot live there. The outbound key is not a user key and the keys screen never lists it. The check script scans the image tree, not a used device. |

## 2. Decisions

| # | Decision | Why, and what was rejected |
|---|---|---|
| D1 | Administrator means member of group `sudo`. Sudoers grants `%sudo ALL=(ALL:ALL) ALL` with the caller's own password. `wheel` and `adm` carry no sudo rights. Nobody joins group `root`. | One authority group gives the last-admin guard a single definition. The group exists from `base-passwd` (gid 27); nothing in the PAM files references it. |
| D2 | `kiosk` is uid 1000, private group `kiosk`, home `/var/home/kiosk`, shell `/usr/bin/bash`, groups `sudo` and `adm`. No device groups. | `adm` is an owner requirement and gives journal reading for diagnostics. It also means the kiosk shell can read the persistent journal, so D7 keeps secrets out of the journal. `input`, `video`, `render`, `dialout`, `tty`, `seat` are unnecessary: the controller, Weston and Chromium stay root and PTYs come from devpts. |
| D3 | Persistence: overlayfs on `/etc`, upper and work on `/var`, mounted by a systemd unit after `/var`. Fallback: systemd-userdb. | Section 3, gated by T0. |
| D4 | `kiosk` is in the rootfs seed (`extrausers`, no `-p`, so `!`). The build public key is baked into the read-only rootfs and is a second `AuthorizedKeysFile` path. First boot only sets the kiosk password. | With `/var` or the overlay broken, root stays locked, `kiosk` exists and key login works (degraded, not locked out of SSH, not open). Needs T0 (f): key login for a `!` hash with `UsePAM yes`. Rejected: key copy at first boot (skipped when the overlay fails), creating kiosk at first boot. |
| D5 | SSH keys: `AuthorizedKeysFile /usr/share/thing-revived/accounts/%u.authorized_keys /var/lib/thing-revived-ssh/authorized_keys/%u`. The first is the baked build key (kiosk only). The second is the menu-managed store (dir 0755 root, files 0644 root, outside the overlay). The menu cannot remove the baked key: it is part of the image, so a reflash replaces it and the owner never loses the way in (open question 4). | sshd reads keys as the target user and `StrictModes` checks every path component, so neither can sit under the 0700 `/var/lib/thing-revived`. A baked key means a new image's key is effective immediately with no wipe or copy. Rejected: copying the seed into `/var` (stale after a rebuild, review B1). |
| D6 | The controller (already root) performs account changes by spawning absolute-path shadow-utils binaries with argument vectors, a cleared environment, fixed `PATH`, passwords on stdin only. One mutex serialises mutations. No helper binary now. | A helper only reduces risk if the controller drops root, which is out of scope. Rejected: reimplementing hashing and file locking in Rust. |
| D7 | The loopback WebSocket is not trusted. Every mutating command requires an unlocked admin session bound to that connection. Destructive commands also carry `confirm`. All `user_*`, `group_*` and `ssh_key_*` replies are targeted to the requesting connection (never broadcast) and **logged type only** (section 7). | Any local process can connect to `127.0.0.1:8765`. The hub broadcasts and logs full JSON today, so targeted replies and the log policy are new work in Phase 1 (T-P1.5). |
| D8 | Terminal sessions run as `kiosk` (`User=kiosk`). Root work uses `sudo` with the kiosk password. | A root shell on the touchscreen defeats sudo-with-password. |
| D9 | adb and UART autologin are off by default and re-enabled only by `THING_REVIVED_ENABLE_ADB=1` and `THING_REVIVED_ENABLE_UART=1`, independent of each other. `/etc/securetty` is emptied unless the UART flag is on. | adb and UART autologin are passwordless root shells. |
| D10 | **Account epoch, no wipe.** Account state (overlay upper, key store, homes, `state.json`) lives on `/var` and persists across builds. There is no build-id and no wipe code. State exists only if `/var` was provisioned: a fresh `/var` (new data partition, or a factory reset, which wipes the whole data partition) starts with none. `state.json` carries a random 128 bit `epoch` written when provisioning completes, plus `kiosk_password_set` and `acknowledged`. Slot flips, OTA, rollback and dev rebuilds change nothing. Each boot `provision` reconciles: (a) any system account or group (uid or gid below 1000) present in the lower `/etc` files and missing from the upper copy is appended to the upper copy, so a newer image can add system users without a wipe; (b) the build key is effective automatically (D5); (c) key files in the key store for users that no longer exist are deleted. A fresh flash that leaves `/var` intact does **not** reset accounts (T0 (e) measures whether that happens); the owner uses the armed factory reset (section 13) for a full reset. | The review showed a per-build id wipes accounts on every OTA, rollback and dev rebuild, leaves keys and homes behind, and lets a re-created name inherit an old key and home. Rejected: build-id nonce (above), wiping keys and homes together with the overlay (destroys data on every rebuild). Steelman against: an image that changes a system account incompatibly would need a wipe; reconcile only appends, and a mismatch is logged type only and shown as a banner, which is acceptable for this device. |
| D11 | First-boot logic is the controller subcommand `controller-agent provision`. The overlay mount unit stays a tiny fixed-string shell script. | Shares generator, validator and tool wrappers with the live service. |
| D12 | Homes at `/var/home/<user>`. `user_add` refuses (`leftover_state`) when `/var/home/<name>` or a key file for that name already exists. `user_remove` deletes the key file, and the home unless the owner picks "keep home"; a kept home blocks re-creating that name until the owner removes it by hand. | `/home` is read-only. Stops a reused name or uid inheriting old state. |
| D13 | Reveal flow: ack is by proof. `user_bootstrap_ack` carries the last four characters of the password; a mismatch keeps the secret and returns `bad_ack`. Reveal needs a **knob push** (not any key) seen by the evdev adapter within 10 seconds, consumed by one reveal. | Stops blind acknowledgement locking the owner out (review risk 2) and stops any key press from releasing the secret to a polling local process. |

## 3. Persistence of account files on a read-only rootfs

Writable needs: `/etc/passwd`, `/etc/shadow`, `/etc/group`, `/etc/gshadow` (and `-` backups), `/etc/sudoers.d/`. shadow-utils creates siblings in `/etc` and renames over the target, so the directory must be writable and the targets real files.

| Option | How | Fails because / costs | Verdict |
|---|---|---|---|
| A. Symlinks into `/var` | `/etc/passwd -> /var/...` | Needs sibling files in read-only `/etc`; system accounts vanish before `/var` is mounted. | Rejected |
| B. Bind mount each file | `mount --bind` per file | `rename()` onto a bind mount gives EBUSY. | Rejected |
| C. Overlayfs on `/etc` | lower = rootfs `/etc`, upper and work on `/var/lib/thing-revived/etc/` | Late mount ordering, machine-id re-bind, copy-up hides later lower changes (D10 reconcile covers system accounts only), one more mount that can fail. | Chosen, gated by T0 |
| D. systemd-userdb | JSON records under `/etc/userdb` symlinked into `/var` | We would write the record format, hashing and locking ourselves; `pam_unix` reaching hashes through `nss-systemd` is unverified; static glibc cannot use NSS so the controller parses files anyway. | Fallback if T0 fails |
| E. `nss-altfiles` or `libnss-extrausers` | second NSS source | Not in the pinned layers, shadow-utils cannot write it. | Rejected |
| F. `systemd-sysusers` | declarative | Needs a writable `/etc`; creation, not persistence. The service is inactive on the device. | Rejected as mechanism |
| G. Remount rootfs rw | `mount -o remount,rw /` | Breaks the read-only guarantee, flash wear. | Rejected |
| H. oe-core `overlayfs-etc` | mounts the overlay in a preinit before systemd | Needs the data partition at that point, but `superbird-provision` creates and formats `/var` after udev on first boot, so the backing store does not exist yet. Not tried on a boot (T0 (a) compares it and records any workaround). | Rejected unless T0 finds a workaround |

Note on precedent: per the review, `/srv` is a loopback copy of a directory that is already on the rootfs (`mount-copybind` at local-fs time). It is not a precedent for a late overlay over a directory systemd has already read (not verified by me).

Steelman against C: mounting over `/etc` under a running systemd is unusual (units, journald, udevd, networkd and timesyncd are already running, some in private mount namespaces), shutdown ordering vs `/var` is unproven, and copy-up hides later lower changes. Answer: only the account files and `sudoers.d` are written, D10 reconciles system accounts, and T0 tests propagation, `systemd-analyze verify`, shutdown order and a power cut before anything depends on it. If any of that cannot be solved cleanly, switch to option D with the same ports and services; only the adapter in section 7 and the image unit change.

Policy for the upper layer: only the four account files, their `-` backups and `sudoers.d` are expected. `provision` logs the names (not contents) of any other file found in the upper layer as a warning, and the Users menu shows a banner. Root processes can still drop files there, which outlive the boot and hide lower updates; this is documented, not blocked.

Mount unit `thing-revived-etc.service` (fixed-string shell script, no inputs):

1. `[Unit] DefaultDependencies=no`, `RequiresMountsFor=/var`, `After=var.mount`, `Before=systemd-tmpfiles-setup.service nss-user-lookup.target thing-revived-provision.service sshd.socket`, `Conflicts=shutdown.target`. `[Service] Type=oneshot`, `RemainAfterExit=yes`, `ExecStop` unmounts the overlay (best effort; T0 verifies it leaves `/var` unbusy at shutdown). `[Install] WantedBy=sysinit.target` (without it a `DefaultDependencies=no` unit never starts).
2. Create `/var/lib/thing-revived/etc/{upper,work}` mode 0700 if missing. No wipe logic (D10).
3. Bind the current `/etc` read-only to `/run/thing-revived/etc-lower`, bind the `/etc/machine-id` mount to `/run/thing-revived/machine-id`, mount the overlay on `/etc`, bind the saved machine-id back.
4. On any failure the script exits non-zero and leaves `/etc` as the read-only seed. This is degraded mode (D4): root locked, `kiosk` has the baked key and no password, sshd and the controller still start (they are ordered after, but never `Require`, the etc or provision units), and the Users menu shows "Account storage unavailable".

## 4. Image hardening (Phase 1)

Everything is under `build/overlay/` (synced by `build-image.sh`).

| Path | What |
|---|---|
| `recipes-extended/thing-revived-accounts/thing-revived-accounts.bb` | `inherit allarch systemd`. `RDEPENDS` on `sudo`, `shadow`, `libpam`, `openssh-sshd`. `SYSTEMD_SERVICE` lists `thing-revived-etc.service thing-revived-provision.service`; both units carry `[Install] WantedBy=sysinit.target`. |
| `.../files/10-thing-revived-hardening.conf` | sshd drop-in, installed to `/etc/ssh/sshd_config.d/`. |
| `.../files/thing-revived-sshd-socket.conf` | `sshd.socket.d` drop-in with `MaxConnections=16` and `MaxConnectionsPerSource=3` (listener limits that apply to `Accept=yes`). |
| `.../files/sudoers` | Installed as `/etc/sudoers` through the sudo bbappend (mode 0440). |
| `.../files/thing-revived-etc.service`, `thing-revived-provision.service`, `thing-revived-etc-mount.sh` | Units and script (sections 3 and 5). Provision: `Requires=thing-revived-etc.service`, `After=thing-revived-etc.service systemd-tmpfiles-setup.service`, `Before=sshd.socket controller-agent.service thing-revived-terminal-agent.service getty.target`, `[Install] WantedBy=sysinit.target`. `Requires=` (not `Requisite=`) so an ordering race cannot cause a spurious skip. |
| `.../files/thing-revived-accounts.tmpfiles` | `/usr/lib/tmpfiles.d/thing-revived-accounts.conf`: `d /var/home 0755 root root`, `d /var/home/kiosk 0700 kiosk kiosk`, `d /var/lib/thing-revived/etc 0700 root root`, `d /var/lib/thing-revived/accounts 0700 root root`, `d /var/lib/thing-revived-ssh 0755 root root`, `d /var/lib/thing-revived-ssh/authorized_keys 0755 root root`, `d /run/thing-revived 0700 root root`. |
| `.../files/kiosk.authorized_keys` | Staged by `build-image.sh` per build, installed to `/usr/share/thing-revived/accounts/kiosk.authorized_keys` (0644 root, dirs 0755). Never in the repo. |
| `recipes-extended/sudo/sudo_%.bbappend` | Installs the sudoers, ensures `/etc/sudoers.d` exists, runs `visudo -c` at `do_install` if `sudo-native` provides it. |
| `recipes-extended/pam/libpam_%.bbappend` | `do_install:append` strips `nullok` (and `nullok_secure`) from the `pam.d` files, including `common-auth` and `common-password`, and sets `rounds=` in `common-password`. The recipe is `libpam`; the file is packaged in `libpam-runtime`. No `su` restriction (below). |
| `recipes-extended/thing-revived-terminal-agent/files/thing-revived-terminal-agent.service` | Add `User=kiosk`, `Group=kiosk`, `SupplementaryGroups=` (empty), `Environment=HOME=/var/home/kiosk`, `After=thing-revived-provision.service`, `ConditionPathExists=!/var/lib/thing-revived/accounts/kiosk.locked` (lets a kiosk lock stop the terminal). |
| `recipes-extended/controller-agent/files/controller-agent.service` | Add `After=thing-revived-provision.service thing-revived-etc.service`. Stays root. |
| `recipes-core/images/superbird-kiosk-prod-image.bbappend` | See below. |

Image bbappend content:

- `IMAGE_INSTALL:append = " sudo shadow thing-revived-accounts"`.
- `IMAGE_FEATURES:remove = "allow-empty-password allow-root-login empty-root-password serial-autologin-root"`. These come from `EXTRA_IMAGE_FEATURES` in `kas/base.yml`; `debug-tweaks` no longer exists in this release. Removing them makes oe-core stop writing `PermitRootLogin yes`, `PermitEmptyPasswords yes` and `--autologin root`, and makes `zap_empty_root_password` write `root:*:`. T-P1.2 confirms the effective value with `bitbake -e superbird-kiosk-prod-image | grep ^IMAGE_FEATURES=`. This replaces the earlier openssh `do_install:append` rewrite and the autologin drop-in; the assertions below stay as the safety net.
- `inherit extrausers` with `EXTRA_USERS_PARAMS = "useradd -u 1000 -U -d /var/home/kiosk -s /usr/bin/bash -G sudo,adm kiosk;"`. No `-p` (a shell-active `'!'` is fragile through `set_user_group`, and `useradd` without a password writes `!`). `-m` is absent (home comes from tmpfiles).
- `ROOTFS_POSTPROCESS_COMMAND` functions, in order: (1) UART: when `THING_REVIVED_ENABLE_UART` is `1`, replicate oe-core's `serial_autologin_root` sed on `serial-getty@.service` (root stays locked, autologin does not need a hash) and leave `/etc/securetty` alone; otherwise truncate `${IMAGE_ROOTFS}/etc/securetty` (a rootfs command, not a second package, because `shadow-securetty` owns that file and a second owner collides under `package_ipk`); (2) ADB: unless `THING_REVIVED_ENABLE_ADB` is `1`, `rm -f ${IMAGE_ROOTFS}/etc/usb-debugging-enabled`; (3) a final assertion that `root` has a hash starting `*` or `!`, `kiosk` has `!`, no hash field in `/etc/shadow` is empty, no `nullok` under `/etc/pam.d`, sshd config has no `PermitRootLogin yes`, `serial-getty@.service` has `--autologin root` only when the UART flag is on, and `usb-debugging-enabled` exists only when the ADB flag is on; `bbfatal` otherwise. The flag variables default with `?= "0"`, and the effective flags are written to `/usr/share/thing-revived/accounts/build-flags` for the check script.
- `IMAGE_ROOTFS_EXTRA_SPACE` is not used (it does not size the wic filesystem, section 9).

`su` restriction dropped: the review proposed adding `pam-plugin-wheel`. `pam_wheel.so` is absent on the device and `su` to root cannot succeed with a locked root, so the restriction adds a package and a new failure mode for no gain. The safer option is kept.

sshd drop-in `10-thing-revived-hardening.conf` (first value wins; `10-` sorts before upstream `20-systemd-userdb.conf`):

```
PermitRootLogin no
PasswordAuthentication no
PermitEmptyPasswords no
KbdInteractiveAuthentication no
PubkeyAuthentication yes
AuthenticationMethods publickey
UsePAM yes
AuthorizedKeysFile /usr/share/thing-revived/accounts/%u.authorized_keys /var/lib/thing-revived-ssh/authorized_keys/%u
AuthorizedKeysCommand none
MaxAuthTries 3
LoginGraceTime 30
X11Forwarding no
PermitUserEnvironment no
```

`UsePAM yes` stays because account locking relies on the PAM account check. `AllowTcpForwarding` stays default because `docs/architecture.md` documents `ssh -L`. `MaxStartups` and `PerSourcePenalties` are not listed: sshd runs per connection (`Accept=yes`, `sshd -i`), where listener-side limits do not apply; the socket drop-in above is the throttle (its behaviour is checked in T0 (i)). Open question 3 covers `AllowGroups`.

Lock semantics (used by Phase 2a, and by hand in Phase 1): `passwd -l` does not stop key login, so lock is `usermod --lock --expiredate 1 <n>` (the `sshd` PAM account stack rejects the expired account even for key login). Unlock is `usermod --unlock --expiredate "" <n>`; unlock of an account that has only `!` and no password is refused with `no_password`. After locking, the adapter ends sessions in three steps: `loginctl terminate-user <n>` (logind sessions only), then a `/proc` scan for processes with that real uid and `/usr/bin/kill -KILL <pids>` (covers terminal shells and `sudo` children that are not logind sessions), then a re-scan; if processes remain the result is `sessions_remain`. Locking `kiosk` additionally creates `/var/lib/thing-revived/accounts/kiosk.locked` and runs `systemctl stop thing-revived-terminal-agent.service`; the unit's `ConditionPathExists=!` keeps it from restarting, and unlock removes the flag and starts the unit. Without this the terminal agent would keep a kiosk shell running after a lock, because systemd `User=` does not consult account expiry.

Sudo `/etc/sudoers`:

```
Defaults env_reset, secure_path="/usr/sbin:/usr/bin:/sbin:/bin"
Defaults timestamp_timeout=5, timestamp_type=tty
Defaults passwd_tries=3, passwd_timeout=1
Defaults use_pty, lecture=never, !visiblepw
root ALL=(ALL:ALL) ALL
%sudo ALL=(ALL:ALL) ALL
@includedir /etc/sudoers.d
```

No `NOPASSWD`, no `!authenticate` (the check script greps for both). `sudoers.d` is empty in the image. The controller never edits sudoers. `sudo` asks for the caller's own password (`targetpw` is off by default).

PAM and login: remove `nullok` everywhere. sha512 rounds in `common-password` and `/etc/login.defs`: T0 (h) measures `chpasswd` and one `unix_chkpwd` check on the A113 and picks the largest of 65536, 32768, 16384 that stays under 0.5 s; the value is a `[users]` config key and must match the PAM file. `getty@tty1` stays; login for any user needs a password, and root cannot log in.

Limits to state in the docs: guard rails protect against menu mistakes only. Anyone with `sudo` can edit `/etc/shadow` directly. A lost kiosk password with no other admin means a reflash or factory reset.

## 5. First-boot flow (Phase 1)

`thing-revived-provision.service` runs `controller-agent provision` on every boot and is a fast no-op in steady state.

State file `/var/lib/thing-revived/accounts/state.json` (0600 root): `{epoch, kiosk_password_set, acknowledged}`. Plaintext holder `/run/thing-revived/first-boot-secret` (tmpfs 0600 root, the password and an expiry in monotonic seconds).

Algorithm, each step safe to repeat:

1. Reconcile (D10): append missing lower system accounts to the upper copies, delete key files of users that no longer exist, warn about unexpected upper files. The build key needs no step (D5).
2. If `acknowledged` is true: stop (steady state).
3. If `acknowledged` is false, or the state is missing: generate a 16 character password from the 56 symbol set `ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789` with `getrandom` and rejection sampling (about 93 bits). Write it to the tmpfs file first, then set it with `chpasswd` on stdin (name and password never in argv), then write `state.json` with `kiosk_password_set=true, acknowledged=false`. The tmpfs write comes first so a crash leaves nothing or a matching pair.
4. If `kiosk_password_set` is true, `acknowledged` is false and the tmpfs file is missing (reboot before ack): the password is unknown to everyone, so regenerate (step 3). SSH key login is unaffected because the key is baked.

Display and scrub, by the controller service while it runs:

- `user_bootstrap` advertises a pending secret (no password). The frontend shows a blocking screen "Admin password ready, press the knob to reveal".
- `user_bootstrap_reveal` is honoured only if a knob push was seen by the evdev adapter within 10 seconds; it consumes that press and replies only to the requesting connection. A local process polling the command gets the secret only if it wins a race against the owner's own knob push in that window; this is accepted and documented.
- The reveal can repeat until acknowledged or expired, so a Chromium crash does not lose it. `user_bootstrap_ack` with the correct last four characters sets `acknowledged=true`, zeroes and unlinks the tmpfs file and drops the in-memory copy. A wrong value returns `bad_ack` and changes nothing. The first successful `sudo` or `user_unlock` also counts as acknowledgement (proof of knowledge).
- Expiry is 15 minutes (`first_boot_ttl_secs`). On expiry the file is scrubbed and the screen says "Admin password was not collected. Reboot to generate a new one." A headless owner can reboot-loop indefinitely; SSH key login works throughout, and this is documented.
- Plaintext lives only in the tmpfs file and controller memory. Swap is zram. The reveal and secret events are logged type only (section 7).

A fresh flash, OTA, rollback and slot flip: see D10. If `/var` is recreated the same path runs as a clean first boot.

## 6. Account model and guard rails (Phases 2a and 2b)

Evaluated by pure functions in `domain/accounts/` on a snapshot parsed from `/etc/passwd`, `/etc/group` and a reduced `/etc/shadow`:

- Managed user: uid 1000 to 59999. System user: uid below 1000, listed read-only, never modifiable.
- Admin: user in group `sudo`, not locked (no leading `!`, not expired) and with a usable hash.
- Reserved account: `kiosk`. It cannot be removed or renamed. It can be locked or lose `sudo` only if another effective admin exists. Locking it also stops its terminal (section 4).
- Assignable groups: any group with gid 1000 to 59999, plus `[users] assignable_system_groups = ["sudo", "adm"]`. `adm` exposes the persistent journal; `sudo` grants admin and needs `confirm`. Device groups (`input`, `video`, `render`, `dialout`, `plugdev`, `audio`, `systemd-journal`) are dropped from the default list: `input` gives read access to `/dev/input`, including a Bluetooth keyboard used to type passwords, and `video` opens `/dev/dri`; nothing that runs as a user needs them (all device-facing services are root) and `adm` already covers log reading. Adding any of them is a config change plus `confirm`. Never assignable: `root`, `shadow`, `disk`, `kmem`, `tty`, `utmp`, `staff`, `sshd`, `messagebus`, `systemd-*`. System groups cannot be created or removed.

Rules (pure, typed `GuardError` mapped to a wire code):

| Rule | Code |
|---|---|
| Name matches `^[a-z_][a-z0-9_-]{0,30}$`, not reserved, not an existing user or group name | `invalid_name`, `exists` |
| `/var/home/<name>` or a key file already exists | `leftover_state` |
| New or reset password 12 to 128 printable ASCII, not equal to the name, not one repeated class. Generated (16 char) passwords pass. | `invalid_password` |
| Target uid below 1000 for any mutation | `system_account` |
| Remove, lock, demote, or blank the password of the last effective admin | `last_admin` |
| Remove or rename `kiosk` | `protected_account` |
| Group not assignable | `group_not_assignable` |
| More than 16 managed users, 32 groups, 20 keys per user | `too_many` |
| Command on a connection that is not unlocked or has expired | `not_unlocked` |
| Wrong admin password at unlock | `bad_password`, then `rate_limited` |
| Remove user or group or key, lock, reset another user's password: missing or mismatched `confirm` | `confirm_required` |
| Changing your own password without `current_password` | `bad_password` |
| Unlock of an account with no password; processes survive the kill; `userdel` fails on live processes | `no_password`, `sessions_remain`, `user_busy` |

Unlock rate limiting (time-injected state machine, no sleeping in tests): per connection, 3 free tries then 5 s, 10 s, 20 s up to 5 minutes, reset on success. A global cap of 20 failed unlocks per 10 minutes across all connections; past the cap, further unlocks require a fresh knob push on the unlock screen (the same press source as the reveal), so a local process can stall other connections but can never lock the physical owner out. Also a cap of 10 mutating commands per minute per connection, and no throttle at all on the physical-press path. The remaining denial of service (a local process burning the global cap) is in the threat model.

`/etc/shadow` is read only to derive lock state and admins. The adapter reduces each entry to `{locked, has_password}` at the boundary and never forwards hashes. The verifier refuses any entry with no hash.

## 7. Controller design (layered Pattern A)

Phase column: 1 = Phase 1, A = 2a, B = 2b.

| Layer | File | Content | Phase |
|---|---|---|---|
| domain | `controller/src/domain/accounts/mod.rs` | module list, one owner per phase | 1, A, B |
| domain | `.../password.rs`, `.../state.rs` | validator and generator (entropy closure), `state.json` model and the reconcile planner | 1 |
| domain | `.../names.rs`, `model.rs`, `guard.rs`, `ratelimit.rs`, `session.rs`, `events.rs` | name rules, passwd and group snapshot, guard rules, rate limiter, session, error mapping | A |
| domain | `.../groups.rs`, `keys.rs` | group policy, authorized-key line parser (rejects options, multiple lines, over 8 KiB, unknown types) | B |
| ports | `controller/src/ports.rs` | Phase 1: `OneTimeSecrets`, `Entropy`, `PressSource` (knob push instants), `AccountAdmin::set_password`. A: `AccountDb`, rest of `AccountAdmin` (`add_user`, `remove_user`, `set_locked`, `end_sessions`), `PasswordVerifier`. B: `AuthorizedKeysStore`, `KeyInspector`, group methods. | 1, A, B |
| adapters | `adapters/secret_tmpfs.rs`, `os_entropy.rs`, a chpasswd-only slice of `accounts_shadow.rs` | | 1 |
| adapters | `adapters/accounts_shadow.rs` (full), `unix_chkpwd.rs` | `std::process::Command` on absolute paths | A |
| adapters | `adapters/authorized_keys_fs.rs`, `ssh_keygen.rs` | atomic writes: temp file, fsync, rename, fsync the directory | B |
| services | `services/users.rs` | holds ports behind `Arc<dyn ...>`, one mutation mutex, limiter and sessions. Phase 1 contains only bootstrap handling. | 1, A, B |
| app | `app/{router,policy,wire_map,hub}.rs`, `lib.rs`, `main.rs` | new router arms; `policy::execution` marks every `user_*`, `group_*`, `ssh_key_*` command `Blocking`; `hub` gains `emit_to`; `main.rs` dispatches `provision`; `lib.rs` picks concrete adapters | 1, A, B |
| protocol, utils | `protocol/{command,event}.rs`, `utils/config.rs`, `utils/types.rs` | variants (section 8), `[users]` config | 1, A, B |

Targeted replies (Phase 1, T-P1.5): `Router::handle` receives a `ConnId`. `adapters/ws_server.rs` assigns one per accepted connection, and `EventHub` keeps a map from `ConnId` to that client's writer. `emit_to` writes to one client. A disconnect drops that connection's unlock session and any pending reveal. Existing broadcasts are unchanged. Add an Origin check to the controller handshake (accept no Origin, `null`, loopback `http`, as `net::origin_allowed` in the terminal agent). This only stops browser pages from foreign origins; other local processes omit or forge Origin, so it is defence in depth only.

Log policy (Phase 1, required): in `controller/src/app/hub.rs`, `log_policy` becomes default-deny for the new families: every `user_*`, `group_*` and `ssh_key_*` event returns `LogPolicy::TypeOnly`, written as explicit match arms so a new variant is a compile-time decision. `emit_to` calls the same `log_policy`. `user_bootstrap_secret` and `user_secret` are never logged beyond their type. Every new password field is `Secret` (redacted `Debug`), so `debug!("client command: {cmd:?}")` stays safe. A test iterates every user-related event, sends it through both `broadcast` and `emit_to` with a capturing logger, and asserts no field value (password, fingerprint, username) reaches the log. Because `adm` reads the persistent journal and `kiosk` is in `adm`, nothing in the journal may contain a secret; G1d greps for it on the device. Journald stays persistent (not changed).

Audit lines (Phase 2a): one log line per mutation with op, target name, result code and connection id; no secrets, no key bodies, no fingerprints.

Password verification (Phase 2a): the preferred adapter runs `/sbin/unix_chkpwd <user> nonull` with the password and a NUL on stdin and reads the exit status. It must be `nonull`, never `nullok` (which would accept an empty password for an empty hash), and the adapter independently refuses to verify any account whose reduced shadow entry has no hash or a locked hash. The calling convention is confirmed in T0 (b). If unreliable, the fallback is the `sha-crypt` crate behind the same `PasswordVerifier` port (reads the hash inside the adapter only). The controller is a static glibc binary and never uses `getpwnam`, PAM or NSS.

Argument vectors (built as `Vec<OsString>` from validated values; no `sh -c`; `env_clear()`, `PATH=/usr/sbin:/usr/bin:/sbin:/bin`, `LANG=C`, 10 second kill timeout, output captured and discarded except for exit status mapping):

| Operation | argv |
|---|---|
| add user | `/usr/sbin/useradd --create-home --home-dir /var/home/<n> --shell /usr/bin/bash --user-group --comment "" <n>` (no `--password`: the account is created locked) |
| set password | `/usr/sbin/chpasswd --crypt-method SHA512 --sha-rounds <rounds>`, stdin `<n>:<password>\n`. The password is never in argv or the environment. The validator forbids newlines. |
| lock | `/usr/sbin/usermod --lock --expiredate 1 <n>`, then end sessions (section 4) |
| unlock | `/usr/sbin/usermod --unlock --expiredate "" <n>` |
| remove user | end sessions, `/usr/sbin/userdel --remove <n>` (without `--remove` for "keep home"), then delete `/var/lib/thing-revived-ssh/authorized_keys/<n>` |
| add or remove group member | `/usr/bin/gpasswd --add <n> <g>`, `/usr/bin/gpasswd --delete <n> <g>` (B) |
| add or remove group | `/usr/sbin/groupadd <g>`, `/usr/sbin/groupdel <g>` (B) |
| end sessions | `/usr/bin/loginctl terminate-user <n>`, then `/usr/bin/kill -KILL <pid...>` for pids found by scanning `/proc/<pid>/status` for the real uid |

`useradd` runs before `chpasswd`, so a new account is locked until its password is set, and the service rolls back (`userdel --remove`) if setting the password fails. Startup reconciliation lists users with no hash as "no password" rather than hiding them. Flag spellings and the `usermod -e ""` and `usermod -U` edge cases are confirmed in T0 (c).

## 8. Wire protocol additions

Same socket and conventions as the existing bridge (JSON `type` tag, explicit `null`, field order as emitted). Password fields are `Secret`. All `user_*`, `group_*` and `ssh_key_*` replies are targeted (D7) and logged type only.

Phase 1 commands: `user_bootstrap_get` (reply `user_bootstrap`), `user_bootstrap_reveal` (needs a knob push within 10 s; reply `user_bootstrap_secret` or `user_error`), `user_bootstrap_ack` with `last4` (reply `user_bootstrap`, or `user_error` with `bad_ack`).

Phase 1 events: `user_bootstrap` (`pending`, `username`, `acknowledged`, `expires_in_secs` nullable, `storage_ok`), `user_bootstrap_secret` (`username`, `password`, `expires_in_secs`), `user_error` (`context`, `message`).

Phase 2a commands: `user_unlock` (`username`, `password`), `user_lock_session`, `user_list_get`, `user_add` (`name`, `password` empty means generate, `admin`), `user_remove` (`name`, `keep_home`, `confirm`), `user_set_locked` (`name`, `locked`, `confirm`), `user_set_password` (`name`, `password` empty means generate, `current_password` own account only, `confirm`).

Phase 2a events: `user_session` (`unlocked`, `expires_in_secs` nullable, `username` nullable), `user_list` (`users: [{name, uid, gid, home, shell, locked, admin, system, protected, groups}]`), `user_result` (`op`, `name`, `success`, `error` nullable, `message` nullable), `user_secret` (`name`, `password`, shown once).

Phase 2b commands: `group_list_get`, `user_group_add` and `user_group_remove` (`name`, `group`), `group_add` (`name`), `group_remove` (`name`, `confirm`), `ssh_key_list_get` (`name`), `ssh_key_add` (`name`, `key`), `ssh_key_remove` (`name`, `fingerprint`, `confirm`).

Phase 2b events: `group_list` (`groups: [{name, gid, system, assignable, members}]`), `ssh_key_list` (`name`, `keys: [{fingerprint, key_type, comment, baked}]`; the baked key has `baked: true` and is not removable), and `user_list` gains `key_count` (its fixtures are updated in that phase).

Error codes: `not_unlocked`, `bad_password`, `bad_ack`, `rate_limited`, `invalid_name`, `invalid_password`, `invalid_key`, `exists`, `leftover_state`, `not_found`, `system_account`, `protected_account`, `last_admin`, `group_not_assignable`, `too_many`, `confirm_required`, `no_password`, `sessions_remain`, `user_busy`, `storage_unavailable`, `internal`.

Golden fixtures under `config/protocol/controller/` (naming per its README, a third segment for a variant): Phase 1: `command.user_bootstrap_get`, `command.user_bootstrap_reveal`, `command.user_bootstrap_ack`, `event.user_bootstrap`, `event.user_bootstrap.none`, `event.user_bootstrap_secret`, `event.user_error`. Phase 2a: `command.user_unlock`, `command.user_lock_session`, `command.user_list_get`, `command.user_add`, `command.user_add.generate`, `command.user_remove`, `command.user_set_locked`, `command.user_set_password`, `command.user_set_password.generate`, `event.user_session`, `event.user_session.locked`, `event.user_list`, `event.user_result`, `event.user_result.failed`, `event.user_secret`. Phase 2b: the remaining group and key commands and events, and a changed `event.user_list`. All `.json`, values synthetic. `controller/tests/wire_golden.rs` gets entries for all of them (it fails on any unreferenced file). A test serialises every user-related event and asserts no password appears except in `user_secret` and `user_bootstrap_secret`. Update `config/protocol/controller/README.md` and `frontend/src/utils/types.js` (`types.test.js` fails otherwise).

## 9. Image size and headroom

The flashed rootfs is the `root_a` partition inside the `.wic`: a 4 KiB-block ext4 of exactly 516 MiB (132096 blocks) made by wic from `--source rootfs --fixed-size ${SUPERBIRD_ROOT_PART_SIZE}M` in `meta-superbird/files/wic/superbird-mainline.wks.in`. On the dev device (an earlier build): 7487 free blocks (29.2 MiB), of which 6604 (25.8 MiB) are reserved for root. `IMAGE_ROOTFS_EXTRA_SPACE` and `IMAGE_OVERHEAD_FACTOR` size the separate deploy `.ext4`, not this filesystem, so they are not tools here.

- Additions are `sudo` (estimate 1.5 to 3 MiB, measure), the small accounts package, drop-ins, and a larger `controller-agent`.
- Measurement: the check script finds the `root_a` partition with `sfdisk -d <wic>` (start and size), carves it with `dd` to a scratch file, and runs `dumpe2fs -h` (block count, free blocks, reserved) and `debugfs -R "cat ..."` for file checks. No root needed. T0 (g) records the baseline on the current `.wic` with the same commands.
- Gate: free space on the carved filesystem at least 4 MiB (hard fail) and a warning under 8 MiB. Whether the final image population honours the reserved blocks is not verified; the gate uses total free blocks and prints the reserved figure alongside.
- If short, shrink first (unused locale or doc content pulled in by sudo's dependencies). The partition size is fixed by the upstream wks.
- `/var` has about 2.2 GiB free, so state is negligible.

## 10. Frontend (Phases 1, 2a, 2b)

Screens are data in `frontend/src/menu/screens/users.js` (new, listed in `menu/screens/index.js` and `index.html`), using the existing row kinds. Pure logic in `frontend/src/users/users-logic.js` (validation mirroring the controller, key line sanity check, row builders), node tested. Input semantics unchanged: knob moves, knob push or ESC tap selects, ESC hold or button 1 goes back, buttons 2 to 4 ignored.

| Screen id | Purpose | Phase |
|---|---|---|
| `users-bootstrap` | blocking reveal: "Admin account kiosk created", `Press knob to reveal`, then the password as a single unbroken string (large monospace, `user-select:none`, not an input), a countdown, then `I have saved it`, which asks for the last four characters through the knob picker (`bad_ack` shows a message and keeps the secret). Held only in `ui.users.secret`, cleared on ack, back, timeout and disconnect. | 1 |
| root (existing) `Users` row, `users-unlock`, `users-home`, `users-list`, `users-user`, `users-add`, `users-pw`, `users-secret`, `users-remove-confirm`, `users-lock-confirm` | unlock gate (admin picker, masked password, `Show password`, `Pick characters with knob...`, `Unlock`; the on-screen keyboard from `weston-keyboard` is a hard dependency for the 16 character kiosk password, the knob picker is only the fallback), list, per-user actions, add (generate ON by default), reset, one-time display, destructive confirms with the name echoed as `confirm` | 2a |
| `users-groups`, `groups-list`, `group-actions`, `group-add`, `group-remove-confirm`, `keys-list`, `key-actions`, `key-add`, `key-remove-confirm` | membership toggles, group management, SSH keys (the baked key row is read-only; `key-add` needs a real keyboard because a key exceeds the picker's 63 character limit, and says so) | 2b |

Behaviour: entering `Users` while locked goes to `users-unlock`; `not_unlocked` or expiry returns there; leaving the section or a 5 minute idle sends `user_lock_session`. Guard errors show a toast plus the `message`. `users-add` and `users-pw` default to generated passwords because a 12 character knob entry means over a hundred detents.

Password hygiene in the browser: passwords live in `ui.users.*` only while their screen is open; `frontend/src/utils/logging.js` redaction already covers `password`, `secret`, `passphrase` and gets `current_password` and `last4`; the real input is `type=password`, `autocomplete=off`, `autocapitalize=off`; nothing enters `localStorage`.

Chromium 68/69 constraints (keep to 68): classic scripts, ES2018 ceiling enforced by `syntax-guard.test.js`, no optional chaining, no `??`, no `flex gap`, no `:focus-visible`, no `inset`, no `aspect-ratio`, no `navigator.clipboard`, no `Object.fromEntries`, no ES modules. Reuse the Wi-Fi `enter_guard_ms` double-delivery guard.

Single-owner frontend files, one task per phase owns each: `frontend/src/state/**`, `frontend/src/transport/**`, `frontend/src/utils/**` and `frontend/src/users/**` (logic task); `frontend/src/menu/**`, `frontend/src/wifi/picker-view.js`, `frontend/index.html`, CSS and `frontend/tests/e2e/**` (screens task). `menu-chart.js` belongs to the screens task and `reducers.js` to the logic task; if both need a change, the screens task asks the logic task owner for the reducer addition, and the orchestrator resolves conflicts. In Phase 1 one task (T-P1.8) owns all of these.

## 11. Terminal agent and the shell user (D8)

Today `terminal-agent` runs as root with no Origin check, so any local process gets a root shell. After D8 it runs as `kiosk` with no supplementary groups, so a local process gets a `kiosk` shell: lower impact, but it reaches the admin's home and keys. Because `SupplementaryGroups=` is empty, the agent's shells do not get `adm` and cannot read the journal; an SSH login as `kiosk` does. The sudo timestamp is per tty (`timestamp_type=tty`). Root-only tasks in the on-device terminal now need `sudo` and the kiosk password. The agent starts after `kiosk` exists (D4) and its `HOME` exists (tmpfiles). Locking `kiosk` stops the terminal through the flag-file condition (section 4); without that, systemd `User=` would not consult account expiry.

## 12. Threat model

| Actor | Can reach | Mitigation | Residual |
|---|---|---|---|
| Network attacker | sshd on port 22 | Key-only auth, no root login, `MaxAuthTries 3`, `LoginGraceTime 30`, socket `MaxConnections` and `MaxConnectionsPerSource`, no password to guess | Compromise of the owner's private key; a connection flood within the socket limits |
| Network attacker via USB | `adbd` when the port is in device role | Flag file not installed by default (D9) | Dev builds with `THING_REVIVED_ENABLE_ADB=1` expose an unauthenticated root shell. Document loudly. |
| Local process, any user | controller WS 8765 and terminal WS 8766 on loopback, unauthenticated | Unlocked admin session per connection (D7), targeted type-only events, Origin check, per-connection backoff with a global cap | The terminal WS still hands a `kiosk` shell to any local process (accepted). A local process can exhaust the global unlock cap, which forces a knob push for further unlocks (a bounded denial of service, not a bypass). A local process can race the owner for the reveal inside the 10 s press window. |
| Physical attacker | touchscreen, knob, USB-C, UART | Reveal needs a knob push and expires, no UART autologin, Users menu needs the admin password | Physical access defeats everything (burn mode, eMMC). Not defended. Shoulder-surfing the first-boot password is accepted. |
| Compromised Chromium | local kiosk page | Local content only | Chromium and Weston run as root with `--no-sandbox`; a renderer exploit is root. Out of scope, tracked as the largest gap. |
| Bad input from the UI | controller commands | Server-side validation, argv only, no shell | Crafted names excluded by the strict regex |
| Log or artifact leak | journal, fixtures, build output | `Secret` type, type-only logging for all account events, frontend log redaction, check script greps the image tree for private key markers | The build host key path is printed; the key itself is not. `adm` and the persistent journal are why this matters. |
| Admin with sudo | everything | Guard rails are menu-only by design | `sudo` can edit `/etc/shadow` directly; no narrow sudoers rule is added |

## 13. Recovery, lockout and rollback

- No on-device "reset accounts" action, by design (it would hand admin to anyone at the screen).
- Forgotten kiosk password, SSH key works: `sudo` needs the password, so the key alone cannot recover it. The ways back are below.
- Reflash with `build/flash-image.sh` in burn mode always works. Whether it resets accounts depends on whether `/var` survives (T0 (e)); with an intact `/var`, accounts, keys, homes and the kiosk password are kept (D10).
- Armed factory reset (documented procedure, no new code): upstream wipes the data partition at boot when u-boot env `factory_reset` is `1` (`superbird-factory-reset.service`). Arm it from a key login with sudo when the password is known, or from a PC through burn mode otherwise. The next boot re-provisions from scratch (new kiosk password, empty key store). Document that it also wipes Bluetooth pairings and Wi-Fi networks.
- If the `/etc` overlay fails (degraded mode, D4): root locked, key login for `kiosk` works through the baked key, `sudo` cannot succeed (no password), the Users menu shows the storage banner. Recover by reflashing or the factory reset.
- Rollback of this change in the image: remove the `thing-revived-accounts` recipe and bbappend lines. Reflashing the previous image is the real rollback; note that an older image still has an empty root password, so a rollback reopens the device until it is reflashed forward.

## 14. Tests and checks

| Level | What | Where |
|---|---|---|
| Domain unit | password and name rules, generator alphabet and rejection sampling (scripted entropy), state and reconcile planner, guard rules (last-admin with locked and expired admins), rate limiter (per connection and global cap) and session expiry with an injected clock, key parsing, snapshot parsing of sample files | `controller/src/domain/accounts/*` |
| Service with fake ports | Phase 1: provision steps (crash between tmpfs write and hash), reveal gated by a knob push (a non-knob key does not release it), ack with right and wrong `last4`, scrub, expiry, regenerate after reboot. A: unlock, lockout, add (manual and generated), rollback on password failure, remove deletes the key file, lock (both `usermod` flags, session end, `sessions_remain`, kiosk terminal flag), leftover state refusal. B: group and key flows | `controller/src/services/users.rs`, `controller/tests/router_flow.rs` |
| Adapter | argv for each op (recording spawner fake: no `sh`, no concatenated strings, `env_clear`, no password in argv), authorized-keys atomic writes in a temp dir | adapter tests |
| Source guard | extend `controller/tests/layering.rs` (`std::process::Command` only in `adapters/`; no `"sh"` or `-c` in account adapters); new `controller/tests/no_secret_logging.rs`; new `controller/tests/log_policy.rs` (every `user_*` event is `TypeOnly` through both `broadcast` and `emit_to`) | `controller/tests/` |
| Protocol golden | every fixture parses (commands) or matches byte for byte (events) | `wire_golden.rs`, `bridge-client.test.js`, `types.test.js` |
| Frontend unit and e2e | `users-logic.test.js`, `menu-chart.test.js`, reducers clearing secrets, syntax guard; e2e in `menu-regression.mjs` and `mock-server.mjs` including an assertion that no DOM text or console output contains the secret after leaving the screen | `frontend/tests/` |
| Image check script | `build/tools/check_image_accounts.py` with `test_check_image_accounts.py` and good and bad fixture trees. Inputs: a rootfs directory, a tar, or the `.wic` (carve `root_a`, section 9; the deploy `.ext4` is not accepted as the primary input), and the deploy ipk directory. Checks: no empty hash and root locked; `kiosk` uid 1000, in `sudo` and `adm`, home on `/var/home`, shell in `/etc/shells`; effective sshd config (first value wins across the base file, `sshd_config.d/*.conf` and the `_readonly` copy) matches section 4; no `nullok`; sudoers has no `NOPASSWD` or `!authenticate`; `/etc/securetty` empty unless the UART flag is in `build-flags`; serial getty has no autologin unless the UART flag; `/etc/usb-debugging-enabled` absent unless the ADB flag; baked key present and equal to the build key fingerprint, no private key markers in the tree; units enabled with `[Install]` and `Requires=` not `Requisite=`; terminal unit has `User=kiosk`; `sudo` and accounts ipks present; free space at least 4 MiB. One failure line per failure, non-zero exit. | `build/tools/` |
| Real-image self-test (optional, if qemu-user for aarch64 exists on the devel host) | against a copy of the built rootfs: `sshd -T -f`, `visudo -c`, `pwck -r`, `grpck -r` | `build/tools/rootfs_selftest.sh` |
| Build script | `KEY_ONLY=1` mode and a shell test with temp files; flag passthrough proof (section 16) | `build/` |
| Device | gate checklists G1d, G2a-d, G2b-d, run by a human after an owner-approved flash; includes `systemd-analyze verify`, `critical-chain`, `systemctl --failed` | docs |

CI in `.github/workflows/ci.yml`: new Rust and node tests as today; the Python check script unit tests run; the image check runs only on the build host.

## 15. Documentation to update

`docs/users.md` (new, modelled on `docs/wifi.md`), `docs/architecture.md` (security model, layers, terminal user, the `ssh -L ... root@<device>` example becomes `kiosk@<device>`), `docs/development.md`, `docs/yocto-build-and-flash.md` (build needs a public key; line 47 "accepts SSH as root" is wrong after this change; SSH as kiosk; first-boot password; the two flags; overlay; factory reset), `docs/hardware.md`, `docs/terminal.md` (shell as kiosk, sudo; line 80 "unauthenticated root shell"), `docs/dependency-surface.md` (sudo), `build/README.md` (`THING_REVIVED_SSH_PUBKEY_FILE`, `THING_REVIVED_ENABLE_ADB`, `THING_REVIVED_ENABLE_UART`, `KEY_ONLY`), `config/README.md` and `config/controller.toml` (`[users]`, every key equal to the compiled-in default, which config tests enforce), `config/protocol/controller/README.md`, `frontend/tests/README.md`, `docs/research/yocto-build-decisions.md` (short note).

`[users]` keys: `session_idle_secs = 300`, `first_boot_ttl_secs = 900`, `reveal_press_window_secs = 10`, `unlock_free_attempts = 3`, `unlock_backoff_max_secs = 300`, `unlock_global_cap = 20`, `unlock_global_window_secs = 600`, `mutations_per_minute = 10`, `max_users = 16`, `max_groups = 32`, `max_keys_per_user = 20`, `min_password_len = 12`, `sha_rounds` (value from T0 (h)), `home_base = "/var/home"`, `keys_dir = "/var/lib/thing-revived-ssh/authorized_keys"`, `baked_keys_dir = "/usr/share/thing-revived/accounts"`, `secret_dir = "/run/thing-revived"`, `state_dir = "/var/lib/thing-revived/accounts"`, `assignable_system_groups = ["sudo", "adm"]`, `tool_timeout_secs = 10`.

## 16. Build-time key and flag handling (T-P1.3 detail)

`build/build-image.sh` gets `resolve_ssh_pubkey` before the clone step: path from `THING_REVIVED_SSH_PUBKEY_FILE`, default `${HOME}/.ssh/id_ed25519.pub`. It fails with `[build-image] ERROR: SSH public key file not found: <path>. Create one with ssh-keygen -t ed25519 or set THING_REVIVED_SSH_PUBKEY_FILE.` for a missing, unreadable or empty file; rejects a file starting with `-----BEGIN`; requires exactly one non-comment line accepted by `ssh-keygen -l -f` with no leading options; and prints the fingerprint it will bake in. It writes the key into the checkout's `thing-revived-accounts/files/kiosk.authorized_keys` only (the checkout is scratch, never the repo). The overlay sync is `cp -a` without delete, so a stale key file can survive in a reused `YOCTO_CHECKOUT`: the script removes `kiosk.authorized_keys` explicitly before writing and again if the build is aborted. The key changes the sstate hash every run, which is expected. `.gitignore` gets a defensive entry for `kiosk.authorized_keys`. `CONFIG_ONLY=1` skips the key check so CI can still validate config; `KEY_ONLY=1` runs only this step.

Flags: `THING_REVIVED_ENABLE_ADB` and `THING_REVIVED_ENABLE_UART` (only the value `1` enables; anything else is `0`) must reach bitbake. The build uses upstream `kas/example-kiosk.yml` through `kas-container`, so the script writes an extra kas config into the scratch checkout (for example `kas/thing-revived-flags.yml`) with a top-level `env:` map naming both variables, so kas adds them to `BB_ENV_PASSTHROUGH_ADDITIONS`, and builds with `kas/example-kiosk.yml:kas/thing-revived-flags.yml`. Whether `kas-container` forwards the host variables into the container is not verified. The task proves it: with a flag set, `kas-container shell ... -c 'bitbake -e superbird-kiosk-prod-image | grep ^THING_REVIVED_ENABLE'` shows the value. If it does not, the fallback is to write the literal values into the generated kas file's `local_conf_header` (the script already owns that file). Either way the recipes read `THING_REVIVED_ENABLE_ADB ?= "0"` and `THING_REVIVED_ENABLE_UART ?= "0"`. The script prints a loud warning when either flag is on and records them in `build-flags`.

## 17. Phases and tasks

Sizes: S under half a day, M half to one and a half days, L one and a half to three days. Task ids are `T-P<phase>.<n>`. A task owns its files exclusively; if it needs a file owned by another it asks the orchestrator. Within a phase no two tasks share a file.

### Phase 0: spike (report only, 1 to 2 days)

**T-P0.1** (M) spike T0. Owns `docs/research/user-management-spikes.md` (new). Nothing else is written in the repo. Gate G0.

Safety classes, so the owner can see exactly what T0 does to the device:

- Class A, read-only (no writes anywhere on the device, safe any time): `tune2fs -l`, `ls`, `cat` of configs, `--help` of shadow-utils, `systemd-analyze verify` of existing units, `systemd-analyze critical-chain`, `systemctl --failed`, timing a crypt computation (CPU only).
- Class B, RAM only (writes only under a tmpfs scratch dir `/run/t0`, removed afterwards; nothing on flash; nothing mounted over `/etc` outside a private mount namespace that dies with its process): a scratch overlay under `/run/t0` to observe copy-up and `rename()` of `passwd`; shadow tools against a prefix directory (`useradd -P`) on `/run/t0`; `unix_chkpwd` against a scratch shadow bind-mounted over `/etc/shadow` inside `unshare -m` (the namespace is private, the real file is untouched). A reboot clears everything.
- Class C, destructive or persistent (flash, anything on `/var`, installing units, a boot with a test image, power cut): never performed by the agent. The agent writes the exact checklist; the owner runs it only when present and with a recovery plan (burn-mode reflash of the known-good image per `docs/yocto-build-and-flash.md`). Items: boot a test image with the overlay unit and run `systemd-analyze verify`, shutdown ordering (does `/var` unmount cleanly), a power cut during a `useradd`, and flashing twice (a different and an identical image) to see whether `/var` survives.
- Host work (devel host, read-only against the checkout, scratch copies under a temp dir): rehearse the overlay and machine-id re-bind under `unshare -m` with a built rootfs, `systemd-analyze --root=<rootfs> verify` on the new units, read the pinned linux-pam source for the `unix_chkpwd` convention.

T0 questions, each answered in the report with a tag V (verified) or U (unverified):

- (a) Overlay on `/etc` with the machine-id re-bind: propagation into units that use `ProtectSystem=` or a private mount namespace, ordering with `sshd.socket`, `systemd-tmpfiles-setup` and journald, shutdown order, and a power cut. Plus a written comparison with oe-core `overlayfs-etc` and whether any workaround exists given `/var` is created after udev. Output: overlay or userdb.
- (b) `unix_chkpwd` calling convention with `nonull` and the stdin protocol, and that it fails on an empty or locked hash.
- (c) shadow flags in 4.19.4: `chpasswd -c SHA512 -s`, `useradd` without `--password` writes `!`, `usermod -e ""`, `usermod -U` on a `!`-only hash, `useradd --comment ""`.
- (d) `ssh-keygen -l -f /dev/stdin` with the key on stdin.
- (e) Does a flash wipe `/var` (Class C, owner run).
- (f) Key login to an account whose hash is `!` with `UsePAM yes` and an unexpired account (the degraded-mode premise), and that `usermod -e 1` blocks key login.
- (g) Baseline free space of `root_a` in the current `.wic` by the section 9 commands.
- (h) sha512 rounds timing for `chpasswd` and one `unix_chkpwd` on the A113 for 65536, 32768 and 16384.
- (i) `kas-container` forwarding of the two flag variables (section 16), and whether `sshd.socket` `MaxConnections` and `MaxConnectionsPerSource` are honoured.
- (j) Which package ships `/etc/pam.d/su`, and that removing `nullok` leaves `login`, `su` and `sudo` working.
- (k) Whether `loginctl terminate-user` plus a `/proc` scan and `kill` clears a terminal-agent shell and a `sudo` child.

Gate G0: the report answers (a) to (k) with tags; Class C items are either run by the owner or listed as open. The owner picks overlay or userdb. T-P1.1, T-P1.2 and the Phase 1 controller tasks do not start before G0. T-P1.3, T-P1.4 and T-P1.6 do not depend on G0 and may start in parallel with T0.

### Phase 1: hardening, kiosk account, first boot, reveal (releasable alone)

Wave 1 (parallel after G0; tasks marked "no G0" may start earlier):

- **T-P1.1** (L) accounts recipe and units. Owns `build/overlay/recipes-extended/thing-revived-accounts/**` and `build/overlay/recipes-extended/sudo/**`. Section 3 and 4 content: recipe, mount unit and script, provision unit, tmpfiles, sshd drop-in, socket drop-in, sudoers, sudo bbappend, baked key install path.
- **T-P1.2** (M) image wiring. Owns `build/overlay/recipes-core/images/**`, `build/overlay/recipes-extended/pam/**`, `build/overlay/recipes-extended/thing-revived-terminal-agent/**`, `build/overlay/recipes-extended/controller-agent/**`. Image bbappend (features, extrausers, post-commands, assertions), libpam bbappend, terminal and controller unit edits. Contract with T-P1.1: recipe name `thing-revived-accounts`, units `thing-revived-etc.service` and `thing-revived-provision.service`, flag variables `THING_REVIVED_ENABLE_ADB` and `THING_REVIVED_ENABLE_UART`.
- **T-P1.3** (S) build script. Owns `build/build-image.sh`, `build/flash-image.sh` (message only), `build/README.md`, `.gitignore`, and the generated kas file logic. Section 16. No G0.
- **T-P1.4** (M) check script. Owns `build/tools/check_image_accounts.py`, `build/tools/test_check_image_accounts.py`, `build/tools/fixtures/**`, `build/tools/rootfs_selftest.sh`. Starts from section 14, finalises after T-P1.1 and T-P1.2. No G0.
- **T-P1.5** (M) targeted replies, log policy, Origin check. Owns `controller/src/app/hub.rs`, `controller/src/adapters/ws_server.rs`, the `ConnId` signature change in `controller/src/app/router.rs`, the hub parts of `controller/tests/router_flow.rs`, and the new `controller/tests/log_policy.rs`. Section 7. Lands before T-P1.7b because it changes the `Router::handle` signature.
- **T-P1.6** (M) protocol. Owns `controller/src/protocol/**`, `config/protocol/controller/**`, `controller/tests/wire_golden.rs`, `controller/src/utils/types.rs`. Phase 1 variants and fixtures (section 8). No G0.
- **T-P1.7a** (M) provision core. Owns `controller/src/domain/accounts/{mod,password,state}.rs`, `controller/src/domain/mod.rs`, `controller/src/ports.rs`, `controller/src/adapters/{secret_tmpfs,os_entropy,accounts_shadow}.rs`, `controller/src/adapters/mod.rs`, `controller/src/main.rs` (`provision` dispatch). Needs T0 (a) and (c).

Wave 2 (after wave 1 dependencies):

- **T-P1.7b** (L) bootstrap service and wiring. Owns `controller/src/services/{users,mod}.rs`, `controller/src/app/{router,policy,wire_map}.rs` (after T-P1.5 lands its signature change), `controller/src/lib.rs`, `controller/src/utils/config.rs`, `config/controller.toml`, `config/README.md`, `controller/tests/layering.rs`, `controller/tests/no_secret_logging.rs`. Depends on T-P1.5, T-P1.6, T-P1.7a.
- **T-P1.8** (M) frontend reveal screen. Owns `frontend/src/**`, `frontend/index.html`, CSS and `frontend/tests/**` (Phase 1 slice: bootstrap screen, ack by last four, redaction, types). Depends on T-P1.6 (fixtures).

Wave 3: **T-P1.9** (M) docs. Owns `docs/users.md`, `docs/terminal.md`, `docs/architecture.md`, `docs/development.md`, `docs/yocto-build-and-flash.md`, `docs/hardware.md`, `docs/dependency-surface.md`, `docs/research/yocto-build-decisions.md`. Depends on all above. (`config/protocol/controller/README.md` is T-P1.6's, `frontend/tests/README.md` is T-P1.8's, `build/README.md` is T-P1.3's.)

Wave 4: **T-P1.10** (M) integration, no file ownership, owner or orchestrator only: full image build, flash on the dev device (owner approval), checklist G1d.

Gates:

- G1a (T-P1.1 to T-P1.3): `CONFIG_ONLY=1 build/build-image.sh` still passes; `KEY_ONLY=1 build/build-image.sh` fails cleanly for a missing, empty and private-key file and succeeds with a valid one; the flag passthrough proof from section 16 passes; recipe parse and QA pass (`do_package_qa`, no file collision on `/etc/securetty`).
- G1b (controller): in `controller/`, `cargo test` and `cargo clippy -- -D warnings`; `layering`, `wire_golden`, `log_policy` and `no_secret_logging` tests pass.
- G1c (image): `check_image_accounts.py` passes on the built `.wic` and fails on each bad fixture; `bitbake -e` shows the four features removed; free space at least 4 MiB; the script's unit tests pass.
- G1d (device, after flash): SSH as `kiosk` with the key works; root and password SSH refused (`ssh -o PreferredAuthentications=password` fails); the serial console shows a login prompt without autologin; the first-boot screen shows the password once after a knob push and a wrong `last4` is refused; ack scrubs `/run/thing-revived`; `sudo -v` works with it and fails without; reboot before ack regenerates, reboot after ack keeps it; `journalctl -b | grep` for the password finds nothing; degraded mode (owner-run Class C: block the overlay mount) still gives key login and a locked root; `systemd-analyze verify`, `systemd-analyze critical-chain` and `systemctl --failed` are clean; the terminal runs as `kiosk`; `adb devices` offers no shell with the default build and the flag builds work independently; an A/B slot flip or OTA (if available) keeps accounts; identical-image and different-image reflash results are recorded; `su` and tty1 login still work.
- G1e (docs): reviewed against code (each documented default equals `controller.toml` and the compiled-in value).

### Phase 2a: menu core (unlock, list, add, remove, lock and unlock, reset password)

Depends on Phase 1 released and on T0 (b) and (h).

Wave 1 (parallel):

- **T-P2a.1** (M) domain. Owns `controller/src/domain/accounts/{names,model,guard,ratelimit,session,events,mod}.rs`.
- **T-P2a.2** (M) adapters. Owns `controller/src/adapters/{accounts_shadow,unix_chkpwd}.rs` (extends the Phase 1 slice), `controller/src/adapters/mod.rs`, `controller/src/ports.rs`.
- **T-P2a.3** (S) protocol. Owns `controller/src/protocol/**`, `config/protocol/controller/**`, `controller/tests/wire_golden.rs`, `controller/src/utils/types.rs`.
- **T-P2a.4** (M) frontend logic. Owns `frontend/src/users/**`, `frontend/src/state/**`, `frontend/src/transport/**`, `frontend/src/utils/**`, their unit tests. Depends on T-P2a.3.

Wave 2:

- **T-P2a.5** (L) service and wiring. Owns `controller/src/services/{users,mod}.rs`, `controller/src/app/{router,policy,wire_map}.rs`, `controller/src/lib.rs`, `controller/src/utils/config.rs`, `config/controller.toml`, `config/README.md`, `controller/tests/layering.rs`, `controller/tests/no_secret_logging.rs`. Depends on T-P2a.1, 2, 3.
- **T-P2a.6** (L) frontend screens and e2e. Owns `frontend/src/menu/**`, `frontend/src/wifi/picker-view.js`, `frontend/index.html`, CSS, `frontend/tests/e2e/**`, `frontend/tests/menu-chart.test.js`. Depends on T-P2a.3, T-P2a.4.

Wave 3: **T-P2a.7** (S) docs (`docs/users.md` and the section 15 files touched). Wave 4: **T-P2a.8** device checklist (owner or orchestrator).

Gates:

- G2a-a (controller): `cargo test`, `cargo clippy -- -D warnings`; layering, golden, log policy and secret-logging tests pass; adapter argv tests pass.
- G2a-b (frontend): `node --test frontend/tests/*.test.js`, `syntax-guard`, and `node frontend/tests/e2e/menu-regression.mjs` pass or print SKIP with the reason.
- G2a-c (docs): documented defaults equal the config.
- G2a-d (device): from the menu with knob only (and the on-screen keyboard for the unlock password): unlock, wrong-password backoff, list, add with a generated password (shown once, then gone), a new user can SSH after a key is added by hand, remove deletes the key file and home and a re-add of a name with leftover state is refused, lock blocks that user's key login and ends their shell, kiosk lock stops the terminal and unlock restores it, reset password, `last_admin` and `system_account` errors appear, a second connection cannot reuse the first's session, the global cap forces a knob push, audit lines appear in the journal with no secrets.

### Phase 2b: groups and SSH keys

Depends on Phase 2a released. Only items with an owner requirement are in scope; the count limits (`max_users` and similar) only if cheap.

Wave 1: **T-P2b.1** (S) domain (`domain/accounts/{groups,keys,mod}.rs`); **T-P2b.2** (M) adapters (`adapters/{authorized_keys_fs,ssh_keygen,accounts_shadow}.rs`, `ports.rs`, `adapters/mod.rs`); **T-P2b.3** (S) protocol and fixtures (same file set as T-P2a.3); **T-P2b.4** (S) frontend logic (same file set as T-P2a.4, after T-P2b.3).

Wave 2: **T-P2b.5** (M) service and wiring (same file set as T-P2a.5); **T-P2b.6** (M) frontend screens and e2e (same file set as T-P2a.6). Wave 3: **T-P2b.7** (S) docs. Wave 4: **T-P2b.8** device checklist.

Gates: G2b-a and G2b-b as G2a-a and G2a-b. G2b-c docs. G2b-d (device): group toggles with the default assignable list (`sudo` and `adm`) and `confirm` for `sudo`; `input` and `video` are refused; the keys list shows the baked key as read-only; add a key with a Bluetooth keyboard and log in with it; remove it; removing the baked key and removing `kiosk` are both refused; a second connection cannot add a key without an unlocked admin session.

Parallelism summary: Phase 0 first (T-P1.3, T-P1.4 and T-P1.6 may start alongside). Phase 1: T-P1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.7a together; then T-P1.7b and T-P1.8; then T-P1.9; then T-P1.10. Phases 2a and 2b each follow the same wave shape and do not overlap each other (they share files).

## 18. Risks (ranked)

1. Overlay on `/etc`: mount ordering, hidden machine-id mount, shutdown order, unit cycles. Mitigation: T0 including the Class C checks, degraded mode with a baked key, fallback to systemd-userdb.
2. A lost kiosk password with no other admin means a reflash or factory reset. Mitigation: ack by proof, first successful sudo or unlock counts as acknowledgement, the armed factory reset, clear docs.
3. Local abuse of the unauthenticated loopback WebSocket. Mitigation: unlock sessions, targeted type-only replies, Origin check, knob-push gating, per-connection backoff plus a global cap. Residual noted.
4. A secret in the journal readable through `adm`. Mitigation: type-only log policy with a test, G1d grep.
5. Side doors the brief did not name (adb, UART autologin, root Chromium). Mitigation: D9 and the threat model; Chromium privilege is out of scope.
6. Image headroom (about 29 MiB free, 25.8 MiB of it reserved). Mitigation: measured on the right artifact, 4 MiB gate, trim fallback.
7. Rolling back to an older image that still has an empty root password. Mitigation: documented in section 13.
8. `unix_chkpwd` is a PAM-internal helper. Mitigation: port with a `sha-crypt` fallback.
9. Removing `nullok` breaks an unexpected login path. Mitigation: G1d tests tty1, serial, `su`, `sudo`.
10. A controller crash mid-operation. Mitigation: accounts are created locked, rollback on failure, startup reconciliation.
11. shadow flag differences. Mitigation: T0 (c) and pinned argv tests.
12. Chromium 68/69 quirks with password input. Mitigation: reuse the Wi-Fi pattern, verify on device.
13. Rounds too slow on the A113. Mitigation: T0 (h) picks the value; the verify call is `Blocking`.
14. `kas-container` may not forward the flag variables. Mitigation: proof step and generated-config fallback (section 16).

## 19. Open questions

Settled by the owner and removed: sshd key-only, root locked, kiosk groups and sudo, generated password, build key, terminal as kiosk, the two flags, password minimum, no HDMI.

1. Does a flash wipe or resurrect `/var`? Answered by T0 (e), owner run. It decides whether a reflash alone resets accounts or the factory reset is needed.
2. Overlay on `/etc` or systemd-userdb? Decided by the owner after T0 on its evidence.
3. Should `kiosk` stay protected (not removable or renamable) and SSH stay open to every managed user with a key, or add `AllowGroups`? Default: protected, open.
4. Should the baked build key be removable from the menu? Default: no (D5). Allowing it adds a lockout path.
5. Is the admin unlock (password, 5 minute session) acceptable on the knob UI with the on-screen keyboard as the primary entry? Default: yes, because the WebSocket cannot authenticate its client.
6. Should the armed factory reset be a documented procedure only (default) or also a menu action? A menu action would give anyone at the screen a fresh admin password.
7. Is the 15 minute reveal window with "reboot to regenerate" acceptable? Default: yes.

Unresolved by me: T0 items (a), (b), (e), (f), (h), (i) and (k) cannot be verified read-only and stay as spike tasks, not facts.

## Planning notes for the caller

- Persistence alternatives (eight options, section 3), privileged execution (helper vs root controller, D6), key delivery (baked second path vs first-boot copy, D5) and reset semantics (epoch vs build-id, D10) were weighed. The overlay still has the least custom security code and the riskiest boot mechanics, so it is gated on T0 with a named fallback.
- The biggest behavioural change from the first draft: accounts now survive rebuilds, OTA and rollback, and a reflash resets them only if `/var` is wiped. This follows the review and is the safer default for rollback.
- Findings the brief did not list: UART autologin as root, active adbd, no Origin check, the persistent journal readable by `adm`, and the device's own outbound SSH key under `/var/lib/thing-revived/.ssh`.
