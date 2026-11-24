# config/

One TOML file per component. Each file is fully commented and its values equal the defaults compiled into the component, so deleting a key (or the whole file) never changes behaviour.

| File | Consumed by | Where it ends up on the device | Env override |
|---|---|---|---|
| `controller.toml` | controller-agent (`controller/`) | `/etc/thing-revived/controller.toml` | `CONTROLLER_CONFIG` |
| `terminal.toml` | terminal-agent (`daemon/`), including the `[[session]]` tables that define the terminal tabs | `/etc/thing-revived/terminal.toml` | `TERMINAL_AGENT_CONFIG` |
| `frontend.toml` | the kiosk web app (`frontend/`), via the generator below | `/usr/share/superbird-kiosk-default/config.generated.js` | none |

## protocol/

`config/protocol/` holds the golden JSON wire fixtures (one file per message variant) shared by the controller and daemon Rust tests and by the frontend node and e2e tests. It is not read at runtime and is not installed on the device. See `config/protocol/README.md`.

## How each file is consumed

Agents (controller, terminal): the agent reads the path in its env var, falling back to `/etc/thing-revived/<name>.toml`. If the file is missing it runs on compiled-in defaults (the terminal agent logs a warning). If the file exists but is invalid it exits with status 78 (EX_CONFIG), so systemd shows a failed unit instead of silently running on the wrong values. The systemd units set the env vars explicitly (`Environment=CONTROLLER_CONFIG=/etc/thing-revived/controller.toml`, `Environment=TERMINAL_AGENT_CONFIG=/etc/thing-revived/terminal.toml`).

Frontend: the kiosk browser (Chromium 147 on `file://`, no build step) cannot read TOML: a `file://` page has no TOML parser and cannot fetch local files (see the [Chromium 147 frontend audit](../docs/research/chromium-147-frontend-audit.md)). At image build time `build/tools/toml_to_js.py` converts `frontend.toml` into a classic script `window.THING_CONFIG = {...};` (sorted keys, ASCII only, deterministic) that `index.html` loads before its other scripts. The committed `frontend/config.generated.js` is a stub that only creates an empty `window.THING_CONFIG` so the app also works when served straight from the repo; the build overwrites only the copy staged for the image and never the committed stub. Never edit a generated file; edit `frontend.toml`.

## Build flow

`build/build-image.sh` validates all three files (fail early, before cargo), copies `controller.toml` and `terminal.toml` into the recipes' `files/` dirs, and generates `config.generated.js` into the webapp recipe's `files/webapp/`. To check config alone, without cloning or building anything: `CONFIG_ONLY=1 build/build-image.sh` (see `build/README.md`). Validate by hand with `python3 build/tools/toml_to_js.py --check config/*.toml`.

## Clock settings

The device has no RTC. The system clock is synchronized over NTP by `systemd-timesyncd`, installed by the `thing-revived-ntp-config` recipe, so `terminal.toml` has no host, SSH or probe keys. Its `[clock]` section only controls status reporting: the terminal agent polls `timedatectl show -p NTPSynchronized` and sends `clock_sync` messages to the frontend, which always carry an offset of 0. `enabled` (default `true`) turns the poll thread off, `interval_secs` (default 60) is the poll period while synchronized, and `backoff_secs` (default `[10, 20, 40]`, last value repeats) are the poll delays while unsynchronized or when the status command fails.

## controller.toml `[users]`

The first-boot password screen (`docs/research/user-management-plan-delta-password-auth.md`, sections 4 and 5). Every default below equals the compiled-in value in `controller/src/utils/config.rs`, and a test pins `controller.toml` to those defaults.

| Key | Default | Range | Meaning |
|---|---|---|---|
| `ack_min_gap_ms` | `1500` | 500 to 5000 | Milliseconds between the password reaching the screen (sent to a frontend that asked with `user_bootstrap_get`) and the moment the second press of the M button is accepted as the acknowledgement. It is an input debounce against contact bounce and a double tap that would dismiss the password before it was read. It is not a display timeout: the password stays on screen, with no timer of any kind, until the second press. A press inside the gap is consumed and ignored. |
| `state_dir` | `/var/lib/thing-revived/accounts` | non-empty path | Directory holding the account `state.json` and the `kiosk.locked` flag file that keeps the terminal agent stopped while the `kiosk` account is locked. |
| `session_ttl_secs` | `1800` | 60 to 3600 | Fixed lifetime in seconds of an unlocked admin session (Device settings, including Users and Groups), counted from the moment of unlocking. Activity does not extend it and leaving the Users screens does not end it. It replaces the old idle-based `session_idle_secs`, which controller.toml no longer accepts. A session belongs to one WebSocket connection and ends with it. |
| `unlock_free_attempts` | `3` | 1 to 10 | Wrong unlock passwords per connection that cost no waiting. After that the waits are 5 s, 10 s, 20 s and so on, doubling up to `unlock_backoff_max_secs`. A correct password resets the count. |
| `unlock_backoff_max_secs` | `300` | 5 to 3600 | Longest wait between two unlock attempts on one connection. |
| `unlock_global_cap` | `20` | 1 to 1000 | Wrong unlock passwords across all connections inside `unlock_global_window_secs`. Past the cap every further attempt needs a fresh physical knob push, so a local process can stall other connections but never the person at the device. |
| `unlock_global_window_secs` | `600` | 10 to 86400 | The sliding window of that cap. |
| `mutations_per_minute` | `10` | 1 to 600 | Mutating commands (add, remove, lock, reset, groups, keys) allowed per connection per minute. |
| `max_users` | `16` | 1 to 1000 | Most managed users (uid 1000 to 59999) the menu will create. |
| `max_groups` | `32` | 1 to 1000 | Most owner-created groups. |
| `max_keys_per_user` | `20` | 1 to 100 | Most authorized keys per user. |
| `sha_rounds` | `65536` | 1000 to 999999999 | SHA-512 crypt rounds for `chpasswd`. Must equal the `rounds=` setting of the PAM `common-password` file, or a menu password and a `passwd` password would hash differently. |
| `home_base` | `/var/home` | non-empty path | Parent directory of the home directories the menu creates (`/home` is read-only). A user whose home or key file already exists cannot be re-created (`leftover_state`). |
| `keys_dir` | `/var/lib/thing-revived-ssh/authorized_keys` | non-empty path | Directory of the menu-managed `authorized_keys` files, one per user, the path sshd's `AuthorizedKeysFile` points at. Each file is written atomically with mode 0600 and owned by its user. |
| `assignable_system_groups` | `["sudo", "adm"]` | valid group names | System groups, besides groups with gid 1000 to 59999, that the menu may assign. `sudo` always needs a `confirm`; any group other than `adm` and `sudo` that you add here does too. `root`, `shadow`, `disk`, `kmem`, `tty`, `utmp`, `staff`, `sshd`, `messagebus` and `systemd-*` are never assignable whatever is listed. |
| `tool_timeout_secs` | `10` | 1 to 120 | Seconds before a `useradd`, `userdel`, `chpasswd`, `gpasswd`, `ssh-keygen` or `unix_chkpwd` call is killed. |

Admin unlock verifies the password with `/sbin/unix_chkpwd`, which does not go through PAM, so `pam_faillock` (five wrong passwords lock `kiosk` for ten minutes on SSH, `sudo` and `su`) neither counts nor blocks menu unlock attempts. The limits above are the only brake on guesses made through the menu: a faillock lock does not stop the menu, and the menu limits do not feed faillock.

`state_dir` is read by the controller service. `controller-agent provision` still uses the same path as a compiled-in constant, so changing it in the file without a matching change there would make the two disagree: leave it at the default unless you are testing. The password itself is never written to any file: it lives in controller memory from the M press until the acknowledgement, and the only persisted artefact is the SHA-512 hash in the `/etc` overlay's `shadow`.

## controller.toml `[device_settings]`

The persisted SSH, ADB, UART and USB mode settings (Device settings, see `docs/hardware.md#device-settings` and `docs/users.md`). It replaces the former `[usb_role]` and `[debug_access]` sections, which controller.toml no longer accepts (an unknown key makes the agent exit 78, so a device that still carries an old file must be updated together with the new binary). Every default below equals the compiled-in value in `controller/src/utils/config.rs`. The boot apply subcommands (`controller-agent apply-device-settings early|late|failsafe`) never read this file: they use the same values compiled in, and a test pins the two together, so a stale or broken `controller.toml` cannot stop the boot apply.

| Key | Default | Meaning |
|---|---|---|
| `systemctl_command` | `systemctl` | Program run as `<it> is-active\|start\|stop <unit>` (looked up on PATH). |
| `role_command` | `superbird-usb-role` | Program that reports and switches the USB-C port role (looked up on PATH). |
| `adb_unit` | `android-tools-adbd.service` | Starting this unit turns adb on. |
| `adb_disabled_unit` | `thing-revived-usb-bind.service` | Starting this unit turns adb off; its `Conflicts=` stops `adb_unit` first. |
| `uart_unit` | `thing-revived-uart-root@ttyAML0.service` | Starting this unit turns the UART root console on. |
| `uart_disabled_unit` | `serial-getty@ttyAML0.service` | Starting this unit after stopping `uart_unit` hands the tty back to the stock login prompt. |
| `ssh_socket_unit` | `sshd.socket` | The ssh listener started and stopped by the ssh setting. |
| `ssh_instance_glob` | `sshd@*.service` | Per connection ssh instances stopped when ssh is turned off. |
| `ssh_off_flag_path` | `/run/thing-revived/ssh-off` | Exists exactly while persisted ssh is off. The `sshd.socket` gate drop-in checks it. |
| `settings_path` | `/var/lib/thing-revived/device-settings.json` | The persisted settings file (`mode`, `ssh`, `adb`, `uart`, `image_id`). |
| `seed_path` | `/usr/share/thing-revived/device-seed.json` | Image baked seed for the first boot and for the reseed on image change. |
| `boot_role_path` | `/var/lib/superbird-usb-role/boot-role` | Legacy role file the stock role unit reads, mirrored from the persisted mode. |
| `command_timeout_secs` | `10` | Seconds before a tool call is killed and reported as `apply_failed` (1 to 120). |
| `mutations_per_minute` | `6` | Mutating commands allowed per connection per minute (1 to 600). Over it the answer is `rate_limited`. |

Live state is never cached: every `device_settings_get` and every reply re-reads `is-active` for the units and `superbird-usb-role status` for the role. The `device_settings` event carries the live values and the persisted ones side by side, so a failed apply shows as a difference. `THING_REVIVED_ENABLE_ADB` and `THING_REVIVED_ENABLE_UART` (build time, see `build/README.md`) are only the first-boot seed baked into the seed file. They do not change this section.

## Changing a value

In the repo (normal path): edit the TOML, rebuild and reflash the image.

On a running device: `/etc` is part of the read-only squashfs rootfs, so the installed files are read-only at runtime. Options:

1. Remount, edit, restart: `mount -o remount,rw /`, edit `/etc/thing-revived/<name>.toml`, then `systemctl restart controller-agent` or `systemctl restart thing-revived-terminal-agent`. Whether the edit survives a reboot depends on the rootfs layout and is unverified; it is gone after a reflash.
2. Point the agent at another file with the env var, using a runtime systemd drop-in (`systemctl edit --runtime controller-agent`, setting `Environment=CONTROLLER_CONFIG=/var/lib/thing-revived/controller.toml`). `/var` is the writable data partition, so the TOML itself persists, but a `--runtime` drop-in lives in `/run` and is gone after reboot. A persistent drop-in needs a writable `/etc/systemd/system`, i.e. the remount in option 1 or a change to the image.
3. Reflash an image built from the edited TOML.

The frontend config is baked into the image; changing it on the device means editing `config.generated.js` under `/usr/share/superbird-kiosk-default/` (same read-only caveat) or reflashing.

## Recommendation (follow-up, not implemented)

To make persistent on-device overrides possible without a remount, the agents' `utils/config.rs` could look for `/var/lib/thing-revived/<name>.toml` first and use it if present, falling back to the `/etc/thing-revived/<name>.toml` path and then to compiled-in defaults (an explicit env var still wins over both). The units need no change for that: `ExecStart` stays as is, and `/var/lib/thing-revived` already exists (tmpfiles.d, 0700, created by the wifi-config recipe). Whole-file replacement is simplest; per-key merging is optional.
