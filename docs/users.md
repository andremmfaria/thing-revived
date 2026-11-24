# Users, Passwords and Remote Access

How accounts work on the device: the first-boot password, signing in over SSH, the on-device Users menu, and what the design does and does not protect. Status: implemented in the repo and passing its host tests, but not yet built into an image or run on a device, see [Not verified](#not-verified). The owner spec is [`research/user-management-plan-delta-password-auth.md`](research/user-management-plan-delta-password-auth.md), which amends [`research/user-management-plan.md`](research/user-management-plan.md). What is running is tracked in [`development.md`](development.md).

## Accounts at a glance

- `root` is locked. No password login, no SSH login (`PermitRootLogin no`), no empty passwords anywhere.
- `kiosk` is the one administrator. It is in the `sudo` and `adm` groups, and the terminal agent runs the on-device shells as `kiosk`. Root work goes through `sudo`.
- No SSH key is baked into the image and the build needs none on the host. The image ships `kiosk` locked, with no usable password.
- Account changes persist through an overlay on `/etc` whose upper layer lives on `/var`. Everything else under `/etc` stays read-only.
- Terminal shells run as `kiosk` and so carry its `sudo` and `adm` groups. systemd applies a user's groups from `/etc/group` whenever `User=` is set, and `SupplementaryGroups=` cannot remove them. Anything typed into the terminal can use `sudo` once the password is entered.

## First boot: setting the kiosk password

The first password is made on the device, with the physical M button. Nothing is shown over the network.

1. If `kiosk` already has a usable password hash, nothing happens and nothing is shown. This also covers a hash set by hand or a restored `/var`: the controller adopts it as acknowledged and never generates over it.
2. Otherwise the screen shows a full-screen message: the kiosk user password is unset, with an arrow pointing at the M button.
3. Press M once. The controller generates a 16 character password (a 56 symbol alphabet without `l`, `I`, `0` and `O`), sets it, and the screen replaces the message with the password as a single unbroken string. It is not a toast and there is no timeout of any kind.
4. Write it down. Press M a second time to confirm. The second press is the acknowledgement and is final: the password is never shown again. A second press within 1.5 seconds of the password appearing is ignored (`ack_min_gap_ms`, a debounce against contact bounce, not a display timeout).
5. If the device reboots before the second press, the password was never acknowledged. It is locked again, the unset message returns and the next M press generates a new password.

Details worth knowing:

- While the screen is up, M and the knob push do nothing else (no `m` typed into the terminal, no long-press restart), and ESC, buttons 1 to 4, the knob and touch are ignored. Only the M press moves the screen on. M behaves normally again once the screen is gone, or if no frontend is connected to show it.
- The password exists in controller memory from the first press until the second, and then only as a SHA-512 hash in the overlay's `/etc/shadow`. It is not written to a file, the journal or the frontend state.
- If account storage is not available (the overlay failed), the screen says so, nothing is generated and M behaves as normal. See [Degraded mode](#degraded-mode).
- Generation or acknowledgement can fail (`generate_failed`, `ack_failed`). The screen shows the message and M retries.

## Signing in over SSH

`kiosk` logs in with its password or with an SSH key from the key store (below). Every other user is key-only, and root is refused.

```sh
ssh kiosk@<device>
sudo -i
```

The port boots in device mode by default (see [`hardware.md`](hardware.md)), so the CDC-NCM link is up at boot even though adb is off (`thing-revived-usb-bind.service` binds the gadget), and you sign in over it as `kiosk` with the password. Host mode for USB Wi-Fi, Ethernet or storage adapters is opt-in with `superbird-usb-role persist host` and takes effect from the next boot on production builds (builds whose boot default is `THING_REVIVED_ENABLE_ADB=1` force device mode at boot, so the dev link always exists); while it is persisted the NCM link is down, so use the dock network, or run `superbird-usb-role persist device` and reboot. adb is not needed for SSH over this link: adb itself is a separate, runtime-toggleable surface available on every build from Settings, Debug access, gated by the kiosk password, see the Security notes below and [`hardware.md`](hardware.md#debug-access-adb-and-uart-toggle). The sshd settings live in `/etc/ssh/sshd_config.d/10-thing-revived-hardening.conf`: global `PasswordAuthentication no` and `AuthenticationMethods publickey`, then one `Match User kiosk` block that allows `password publickey` (alternatives, so either works). `MaxAuthTries` is 3 and sshd runs per connection under a socket with `MaxConnections=16` and `MaxConnectionsPerSource=3`.

SSH keys are read from `/var/lib/thing-revived-ssh/authorized_keys/<user>`, one file per user, managed by the Users menu. `kiosk` has no key file until one is added, so it signs in by password until then.

### Lockout (`pam_faillock`)

Five wrong passwords within 900 seconds lock the account for 600 seconds (`deny=5 unlock_time=600 fail_interval=900`) for SSH, `sudo` and `su`. The counter lives in tmpfs, so a reboot resets it. Because the count is per account, someone on the network can lock `kiosk` out of SSH and `sudo` for ten minutes by guessing wrongly. That is a bounded denial of service, accepted by the owner. Key logins are not counted and not blocked.

The Users menu verifies passwords with `unix_chkpwd`, which does not go through PAM. So faillock neither counts nor blocks unlock attempts made on the device, and a faillock lock does not stop the menu. The menu's own limiter is the only brake there (see `[users]` in [`../config/README.md`](../config/README.md)).

## The Users menu

Settings, Users. It asks for an administrator password (the same as the SSH password) and unlocks an admin session that ends after idle time (`session_idle_secs`, default 300), on Lock admin session, or when the connection closes. Wrong passwords back off per connection and are capped across connections, and past the cap another attempt needs a fresh physical knob push.

- **Users**: list with tags, then per user: lock or unlock the account, set a password (typed, or leave it empty to generate one, shown once on a one-time screen), groups, SSH keys, remove. `Add user...` creates a user, again with a typed or generated password. Setting the password of `kiosk` also changes its SSH password.
- **Groups**: create and remove groups and set who belongs to them. The menu can assign the system groups `sudo` and `adm`, and groups with gid 1000 to 59999. `sudo` always needs a confirmation.
- **SSH keys**: add a public key for a user (typed with a keyboard), see its entry, remove it. Keys for `kiosk` work next to its password. Keys for other users are their only way in.
- **Hold to confirm**: destructive actions (lock, remove, remove key and similar) need the knob push held, not tapped. An early release, an ESC tap or a stale timer sends nothing.

Guard rails stop menu mistakes only: the last administrator cannot be removed or locked, and system accounts and protected groups are refused. Anyone with `sudo` can still edit `/etc/shadow` directly. Limits (`max_users`, `max_groups`, `max_keys_per_user`, rate limits) are in `config/README.md`.

## Security notes and accepted limits

- **Single factor remote root.** The `kiosk` password plus `sudo` is root over SSH. The generated password is about 93 bits, but a human chosen password set later in the menu (minimum 12 characters) can be much weaker. This is accepted by the owner.
- **Debug access is a password-gated root side door, on every build.** Settings, Debug access (see [`hardware.md`](hardware.md#debug-access-adb-and-uart-toggle)) lets anyone who knows the kiosk password, and is physically at the device, turn on an unauthenticated root shell over the USB cable (adb has no password option of its own) or a passwordless root shell over the serial console (UART autologin as root), with no rebuild and no reflash. This replaces the older guarantee that a production image built with `THING_REVIVED_ENABLE_ADB=0` and `THING_REVIVED_ENABLE_UART=0` contained no such path at all: both flags now only set the state each surface starts in at boot, and the live toggle works on every build, prod included, independent of either flag's value. The controls are the existing Users admin unlock, a hold-to-confirm gesture, a persistent `DEBUG` badge on every screen except the first-boot password screen while either surface is on, automatic reversion to the build default when the admin session that enabled it ends (idle timeout, explicit lock or disconnect), and volatility, meaning neither toggle is written anywhere so a reboot always returns to the build default. Recovery from an unwanted toggle is a lock of the admin session or a reboot, and a factory reset or reflash if the kiosk password itself is compromised; the same physical access caveat as the first-boot password applies, since the attack needs the kiosk password and a hand on the device.
- **A lost password is a lockout.** After the acknowledgement there is no recovery by software if the only administrator password is lost. Recovery is the armed factory reset or a reflash, below.
- **Degraded mode.** If the `/etc` overlay fails, no password is set, `kiosk` stays locked, remote login is impossible and `sudo` cannot succeed. The screen still runs, with the storage message. There is no remote access in this state and no developer escape key. Recovery is a factory reset or a reflash.
- **Chromium runs as root with `--no-sandbox` and a loopback debugging port** (`127.0.0.1:9223`). Any local process can read the page, including the first-boot password while it is on screen. The on-screen password is not protected from local processes. See [`hardware.md`](hardware.md).
- **Local WebSocket.** While the password is showing, any local process that connects to the controller WebSocket and asks for the first-boot state is handed it. The Origin check stops browsers on other origins, not local programs.
- **Physical access.** Anyone who can reboot the device before the second M press sees a fresh password on the screen.
- **Journal.** Failed attempts name the target user, never the password. Someone typing a password at a username prompt leaks it to the journal, which the `adm` group can read.
- **CPU.** Password guesses cost CPU that Weston and Chromium share.

## The /etc overlay

Account changes need a writable `/etc`, and the root filesystem is read-only. The image therefore mounts an overlayfs on `/etc` early in boot, from `thing-revived-etc.service` running `/usr/libexec/thing-revived/thing-revived-etc-mount.sh`. The evaluated alternatives are summarised at the end of this section.

### Why an overlay

The root is read-only and A/B. shadow-utils (`useradd`, `chpasswd`, `passwd` and friends) does not edit `/etc/shadow` in place: it creates a lock file and a temporary file next to the database and renames the temporary file over it. That needs a writable directory, not a writable file. Per-file bind mounts of `passwd` and `shadow` and symlinks from `/etc/passwd` into `/var` both fail for that reason (the lock and temp files are still created in `/etc`, and a bind-mounted file cannot be renamed over). This was verified by test on the device.

### Layout

- Lower: a read-only bind of the root `/etc` at `/run/thing-revived/etc-lower`.
- Upper: `/var/lib/thing-revived/etc/upper`.
- Work: `/var/lib/thing-revived/etc/work`.
- Files that land in the upper today: `passwd`, `shadow`, `shadow-`, `.pwd.lock`, `.updated`, `ld.so.cache` and `machine-id`. After Phase 2 (the Users menu writing accounts) `group`, `gshadow` and `sudoers.d` land there too.
- `/etc/machine-id` is a tmpfs file mount made by `superbird-preinit`. The overlay would hide it, so the script keeps a handle on it at `/run/thing-revived/machine-id` and bind-mounts it back over the overlay.

### Mode rules

- The upper directory must be `0755`. The root of an overlay takes its mode from the upper directory, so a `0700` upper makes `/etc` itself `0700` and every non-root service fails with Permission denied (dbus as `messagebus`, the terminal agent as `kiosk`).
- The parent `/var/lib/thing-revived`, `/var/lib/thing-revived/etc` and the work directory stay `0700`.
- Files inside the upper keep their own modes, so `shadow` stays `0400`.
- The script runs the `chmod` lines on every start, not only on creation, so an upper created earlier with the wrong mode is repaired at the next boot.
- Regression guards: `build/tools/check_image_accounts.py` fails an image whose installed script does not set these modes, and `build/tools/rootfs_selftest.sh` runs the script's `chmod` lines on a scratch tree and asserts the result.

### Boot ordering and degraded mode

`thing-revived-etc.service` has `DefaultDependencies=no`, runs after `var.mount` and before `systemd-tmpfiles-setup.service`, `nss-user-lookup.target`, `thing-revived-provision.service`, `sshd.socket` and `sysinit.target`. Between the overlay mount and the `machine-id` re-bind, `/etc/machine-id` reads empty, which is why every consumer is ordered after this unit.

If the mount fails, each step already done is undone and `/etc` stays the read-only seed. This is the degraded mode described under [Security notes](#security-notes-and-accepted-limits): no password can be set, `kiosk` stays locked, there is no remote access and `sudo` cannot succeed. The screen still runs with the storage message. Recovery is a factory reset or a reflash.

### Persistence

- The upper lives on `/var`, the data partition. It survives slot flips, OTA and a reflash that leaves `/var` alone, so the `kiosk` hash and the persisted `boot-role` survive a flash too.
- A factory reset wipes the whole data partition, upper included, and the next boot provisions from scratch.
- First boot state (`acknowledged` and friends) is kept in `/var/lib/thing-revived/accounts/state.json`, also on `/var`.

### Inspecting it on a device

- `adb shell mount | grep ' /etc '` shows the overlay with its `lowerdir`, `upperdir` and `workdir`.
- `adb shell ls -la /var/lib/thing-revived/etc/upper` lists what has been written, and `adb shell ls -ld /etc /var/lib/thing-revived/etc/upper` shows the modes (both should be `drwxr-xr-x`).
- Without adb, run the same commands over SSH as `kiosk`, using `sudo` for the `/var/lib/thing-revived` paths, which are `0700`.

### Alternatives evaluated

[`research/etc-persistence-options.md`](research/etc-persistence-options.md) compares oe-core `overlayfs-etc` (an early init-time overlay, no empty machine-id window, but no fsck before the mount and a custom init template needed) with systemd userdb (no writable `/etc` needed, but nothing in the image can write the records, so the account adapter would be rewritten). Verdict: keep the custom late overlay. userdb is the named fallback if overlay boot tests fail.

## Recovery: factory reset or reflash

Reflashing the image in burn mode always works and wipes the device. The armed factory reset is the lighter option: upstream's `superbird-factory-reset.service` wipes the data partition at boot when the u-boot environment variable `factory_reset` is `1`. Arm it from a `kiosk` login with `sudo` when the password is known, or from a PC through burn mode otherwise. The next boot provisions from scratch: a new `kiosk` password through the M screen and an empty key store. It also wipes Bluetooth pairings and saved Wi-Fi networks. This procedure is taken from the plan and has not been run on the device.

## Not verified

The code and its host tests are done, but nothing here has been through a real bitbake build or run on the device. Until then these are unverified:

- Recipe parse and QA, including the package name `pam-plugin-faillock`, the `sudo` bbappend, the image post-process `thing_revived_pam`, extrausers and flag passthrough into bitbake.
- The `common-auth` ordering of the faillock lines against the existing `pam_unix` line, and faillock behaviour on SSH, `sudo` and `su`.
- The `/etc` overlay boot ordering, power cut during an account change, and `/var` persistence across a flash.
- The M press interception on the real button, the arrow position against the physical M button, and legibility of the password screen at 800x480.
- Password and key login as `kiosk`, root and other-user password refusal, and the armed factory reset.
- The runtime debug access toggle (Settings, Debug access, see [`hardware.md`](hardware.md#debug-access-adb-and-uart-toggle)): the Phase 0 spike exercised the underlying systemd and configfs mechanics on a dev device (`research/debug-toggles-spikes.md`), but the feature itself has not been through a bitbake build or a flash, and the on-device checklist in `research/debug-toggles-plan.md` has not been run.
