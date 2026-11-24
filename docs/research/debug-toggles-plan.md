# Plan: runtime-toggleable ADB and UART root access

Created: 2026-11-24. Status: Draft for owner review. Nothing in this plan has been implemented.

**Superseded 2026-11-24** by [`device-settings-plan.md`](device-settings-plan.md). Section 0 (the owner spec restated as a volatile, auto-disable contract), section 5.4 (the volatile guarantee made structural), the section 7 recommendation and the section 12 decisions 1 and 6 are replaced by the persisted design: adb, UART, SSH and the USB-C mode now live behind a fixed 30 minute admin unlock and survive a reboot in `/var/lib/thing-revived/device-settings.json`, with reseed on image change and a factory reset as the recovery path instead of volatility. The rest of this plan, including the Phase 0 spike findings (items a to e, recorded in `debug-toggles-spikes.md`) and the verified mechanics they established, stands as history and is reused by the new plan.

Device facts below were read on the attached dev Car Thing over adb, read-only, on 2026-11-24. Build facts were read from the local checkout at `/home/andremmfaria/projects/thing-revived`, read-only, on 2026-11-24. This plan amends nothing in `docs/research/user-management-plan.md`; it adds a new capability on top of the account lockdown that plan describes (D9's two build flags), and it follows that plan's layering, naming, and task-breakdown conventions so the two documents read as one body of work.

## 0. Owner spec, restated as a contract

- `THING_REVIVED_ENABLE_ADB` and `THING_REVIVED_ENABLE_UART` stop meaning "this build has the feature." They become the boot-time state: `1` starts that debug surface enabled at every boot, `0` (default) starts it disabled.
- On every build, including prod, the Settings menu offers a live toggle for each of the two surfaces, independent of the other.
- The toggle is volatile. A reboot always returns to the build default. Nothing about the runtime toggle is written to `/var` or to the `/etc` overlay's upper layer.
- The toggle requires the existing Users admin unlock (the kiosk password) plus a hold-to-confirm row, enforced server-side in the controller, not only in the frontend.
- While either surface is enabled, a persistent on-screen indicator is visible on every screen, not a timed toast.
- Account and session lockdown is otherwise unchanged: root stays locked, adb has no authentication of its own, UART autologin is root with no password.

## 1. Verified device and repo facts

Each row names the command run and the key output line, read-only, no writes.

| # | Area | Command | Key output | Consequence for this plan |
|---|---|---|---|---|
| F1 | adbd unit | `systemctl cat android-tools-adbd.service` | Base unit (meta-oe): `ConditionPathExists=/etc/usb-debugging-enabled`, `ExecStartPre=-/usr/bin/android-gadget-setup adb`, `ExecStart=/usr/bin/adbd`, `[Install] WantedBy=basic.target`. Repo drop-in `android-tools-adbd.service.d/superbird.conf`: clears and re-sets the same Condition, adds `After=superbird-usb-gadget.service`, `Requires=superbird-usb-gadget.service`, and `ExecStartPost` that retries binding the UDC for up to 10 s once adbd has written the FFS descriptors. | The upstream superbird drop-in already has an `ExecStop` that writes an empty string to the gadget's `UDC` file, which releases the bind (confirmed on the device, drop-in line 48; this drop-in ships from `meta-superbird`, not this repo). What is still unverified is whether the `ffs.adb` link can be rebuilt after `thing-revived-usb-bind` removed it, and whether the host sees a clean NCM reconnect. |
| F2 | adbd state today | `systemctl is-active android-tools-adbd.service` | `active` | This device is the ADB dev build; `/etc/usb-debugging-enabled` exists (`ls -la` shows it, 133 bytes, dated 2011, i.e. baked at build time, not written at runtime). |
| F3 | usb-bind unit | `systemctl cat thing-revived-usb-bind.service` | `ConditionPathExists=!/etc/usb-debugging-enabled`, `Requires=superbird-usb-gadget.service`, `After=superbird-usb-gadget.service superbird-usb-role.service`, `ExecStart=.../thing-revived-usb-bind start`, `ExecStop=.../thing-revived-usb-bind stop`, `[Install] WantedBy=multi-user.target` | On this ADB build the unit is `inactive` (its Condition is false). Its own script (`build/overlay/recipes-extended/thing-revived-usb-bind/files/thing-revived-usb-bind`) on `start` removes the `ffs.adb` config-fs link, then binds the UDC retrying for up to 10 s; on `stop` it unbinds the UDC (`printf '' > $CFG/UDC`). It never re-adds `ffs.adb`. |
| F4 | gadget composition | `systemctl cat superbird-usb-gadget.service` | `DefaultDependencies=no`, `ConditionPathExistsGlob=/sys/class/udc/*`, `ExecStart=/usr/libexec/superbird-usb-gadget`, `WantedBy=multi-user.target`. Runs once at boot, composes `ncm.usb0` and `ffs.adb` into `configs/c.1`, does not itself bind the UDC. | `ffs.adb`'s configfs link is created exactly once, at this unit's single boot-time run. Nothing in the image today recreates it later. A live switch back to adb after `thing-revived-usb-bind` has removed it needs a way to restore that link without re-running the whole gadget composition (open question, section 8). |
| F5 | USB role drop-in | `cat .../superbird-usb-role/10-thing-revived-default-device.conf` (repo) and `superbird-usb-role status` (device) | Drop-in `ExecStart`: `if [ -e /etc/usb-debugging-enabled ]; then exec superbird-usb-role to-device; fi; role=$(cat /var/lib/superbird-usb-role/boot-role || echo device); ...`. Device: `role: device`, `boot role: host` (persisted host is ignored because the flag file exists). | Confirms `hardware.md`: the drop-in reads the SAME file the adbd/usb-bind Condition pair reads. Today that file encodes "this is an ADB build." After this change the file must keep encoding only the build default (never the runtime toggle), or an ADB dev build's persisted host role silently stops being overridden the way the owner expects. This is Q2 and is addressed in section 4.3. |
| F6 | UART today | `systemctl status serial-getty@ttyAML0.service`, `cat /etc/securetty` | Unit `active (running)`, `ExecStart=-/usr/sbin/agetty --noreset --noclear ... - ${TERM}` (no `--autologin`), `/etc/securetty` is empty. `cat /proc/cmdline` shows `console=ttyAML0,115200n8`. | This build has `THING_REVIVED_ENABLE_UART=0` even though it is an ADB build, confirming the two flags are independent today as documented. The autologin edit (`build/overlay/.../superbird-kiosk-prod-image.bbappend` lines 129-141, `thing_revived_console_gate`) is a `sed` on the shared template `serial-getty@.service`, which is why it currently cannot be instance-scoped; this matters for the runtime design (section 4.4). |
| F7 | Accounts | `cat /etc/shadow \| grep -E '^root\|^kiosk'` | `root:!:...` (locked), `kiosk:$6$...` (real hash set). | Root lockdown holds on this build; nothing in this plan proposes giving root a password. |
| F8 | Controller layering precedent | `controller/src/{domain,adapters,services}/usb_role.rs`, `controller/src/adapters/usb_role_cli.rs` | `UsbRoleCli` runs fixed argv via `std::process::Command`, no shell; `UsbRoleService` holds the port and an `EventSink`; the frontend screen is data in `frontend/src/menu/screens/usb-role.js`; `adb_forced()` on the port is a plain file-exists check read fresh every call, never cached. | This is the template this plan's controller work follows (section 5). |
| F9 | Admin unlock precedent | `controller/src/services/accounts.rs`, `frontend/src/menu/screens/users-kit.js` | One `Sessions` store keyed by `ConnId`, `require(conn, now)` re-checked on every mutating call, idle timeout restarts on activity (`touch` in `users-kit.js`), destructive rows carry `confirm` that must echo a target string, verified server-side in `require_echo`. | This plan reuses the same session, not a second one (section 5.1), so "the Users admin unlock" stays singular, matching the owner's spec sentence. |

## 2. Design in 15 lines

1. Two new systemd units, each with a sibling unit it is mutually exclusive with via `Conflicts=`, replace the single-file Condition gate for live switching: `android-tools-adbd.service` conflicts with `thing-revived-usb-bind.service`; a new `thing-revived-uart-root@ttyAML0.service` conflicts with the stock `serial-getty@ttyAML0.service`.
2. Build time only decides which side of each pair is enabled (symlinked) by default; nothing in `/var` or the `/etc` overlay records the runtime state.
3. The runtime toggle is `systemctl start` on the currently-inactive side of a pair; `Conflicts=` makes systemd stop the other side automatically, and an explicit fallback start restores it on disable.
4. `/etc/usb-debugging-enabled` keeps meaning "ADB build default" only, read once by the USB-role drop-in and once by image assertions; it is never written or read at runtime by the toggle.
5. A new controller layer (`domain/debug_access.rs`, `ports::DebugAccessCtl`, `adapters/debug_access_cli.rs`, `services/debug_access.rs`) mirrors `usb_role.rs` exactly: fixed argv, no shell, state read fresh with `systemctl is-active` on every query.
6. The admin gate is the existing `AccountsService` session, exposed through a narrow `AdminSessionGate` port, not a second unlock mechanism.
7. Every mutating wire command needs an unlocked session on that connection, then a `confirm` string equal to `"adb"` or `"uart"`, checked server-side before any systemctl call.
8. Enabling adb is refused with a typed error when the live USB role is host; the UI explains that device mode is needed and offers the existing USB-mode screen.
9. Every status reply queries the live unit state and the live USB role; nothing is cached across requests.
10. A persistent status-bar badge, owned by the menu chrome (not by the Debug access screen), renders whenever either surface reports enabled; it is a one-line addition to a shared file, kept in its own task.
11. On any toggle failure the service leaves (or returns) the surface disabled and reports `toggle_failed`; the fail-safe direction is always "off."
12. Recommended (owner to confirm, section 7): an admin idle-lock or explicit lock-session also disables both surfaces, closing the lingering-session risk.
13. The riskiest mechanics, live UDC release and `ffs.adb` relink without a reboot, are unverified; they are a Phase 0 spike before any implementation task starts, exactly like T0 in the account-lockdown plan.
14. Image assertions change from "absent unless flag=1" to "unit shipped, and enabled only when flag=1," checked for both the adbd/usb-bind pair and the new UART unit.
15. Nothing here changes account lockdown: root stays locked, adb stays unauthenticated by design, UART autologin stays root-with-no-password; this plan only makes the exposure window operator-controlled and visible.

## 3. Alternatives considered and why they lost

- **Keep the single Condition file as the runtime gate, write it at runtime.** Rejected: the file must live on `/etc`, which is only writable through the account overlay's upper layer on `/var`; writing it at runtime would either persist across reboot (violating "volatile") or need a second, parallel writable store invented just for this, which is more moving parts than two `Conflicts=` pairs. Steelman: a tmpfs copy of the file (`/run/...`) with Conditions repointed at it avoids persistence, but then boot-time enablement and runtime enablement are two different mechanisms layered on the same file, and the UART case still needs a second unit regardless because two gettys cannot share a tty. Decided it is simpler to have one mechanism (`Conflicts=` plus enablement) for both surfaces than two.
- **Edit `serial-getty@.service` (the shared template) at runtime, as the build-time `sed` already does.** Rejected: the template is shared by every possible serial-getty instance; a live edit would need a reload and risks editing instance state that is not ours to own. A second, instance-scoped unit that conflicts with the first is the standard systemd pattern for "two ways to serve the same device" and ships cleanly as a drop-in-adjacent file.
- **Put the debug-access toggle commands inside `AccountsService`.** Rejected on file-ownership grounds: `controller/src/services/accounts.rs` is already the single largest file in the controller (over 1800 lines with its tests) and a single-owner file for the account-lockdown plan's later phases; adding an unrelated feature there would conflict with that plan's task boundaries. A separate service that depends on a narrow slice of `AccountsService` (the session gate only) keeps both files independently ownable.
- **Give the debug-access toggle its own admin password or its own session.** Rejected: the owner's spec says "the existing Users admin unlock," singular; a second session would let someone unlock debug access without ever proving they know the kiosk password through the same screen, and would double the idle-timer and rate-limiter code for no stated benefit.
- **Trust a controller-held boolean for "is adb enabled" instead of querying `systemctl is-active` every time.** Rejected explicitly by the task brief; also a correctness risk, because the live state can change under the controller (a crash-looping unit, a manual `systemctl` from a terminal session) and a cached bool would then lie to the badge and to the guard that refuses to enable adb in host mode.

Where this might be wrong: the `Conflicts=` based switch for adb assumes stopping `android-tools-adbd.service` plus starting `thing-revived-usb-bind.service` is enough to hand the UDC back to NCM-only cleanly and that the reverse direction can re-create the `ffs.adb` configfs link without re-running the full gadget composition. Neither has been exercised on a device. If the UDC comes back in a half-bound state, the only recovery today is a reboot, which contradicts the goal of an on-the-fly toggle. This is flagged as risk 1 and gated behind a spike (section 8) rather than assumed to work.

## 4. Findings and answers to the brief's seven questions

### 4.1 What starts adbd today, and what the flag file gates

`android-tools-adbd.service` is the meta-oe stock unit plus the repo's `superbird.conf` drop-in (F1). It starts today only because `/etc/usb-debugging-enabled` exists on this ADB build (F2); the file is baked into the rootfs at image build time by `thing_revived_adb_gate` in the image bbappend, never written on the device. `thing-revived-usb-bind.service` is the mutually exclusive counterpart, gated on the negation of the same file, and its own script owns the "remove `ffs.adb`, bind UDC for NCM only" sequence (F3).

To enable adb cleanly at runtime: the UDC must be released from whichever side holds it, the `ffs.adb` configfs link must exist again if it was removed, and then adbd must be started and allowed to re-run its own `ExecStartPost` bind retry loop. To disable it cleanly: adbd must be stopped, the UDC it holds is released by the existing upstream `ExecStop` (F1), and `thing-revived-usb-bind.service` must be started to remove `ffs.adb` again and rebind the UDC for NCM only. Whether the NCM link survives this round trip without a reboot is unverified; see risk 1.

### 4.2 Can the gadget stay up for NCM while adbd stops, and what does the host see

The gadget device (`configs/c.1`) itself is composed once at boot by `superbird-usb-gadget.service` and is not torn down by either adbd or `thing-revived-usb-bind`; only the UDC bind (which exposes the composed gadget to the host) and the `ffs.adb` link move. In principle NCM can stay up the whole time: `thing-revived-usb-bind`'s own script already does exactly this today at boot (remove `ffs.adb`, bind UDC, NCM-only). What is unverified is the live round trip in the other direction, from an already-bound-for-adb UDC back to NCM-only and back again, without a reboot. The host's view during a toggle is also unverified: a live UDC unbind and rebind typically causes the whole composite USB device to disappear and reappear from the host's perspective (so an open `ssh` session over the NCM link would drop), which is acceptable for an explicit admin action but should be stated plainly in the frontend warning text, the same way the existing USB-mode screen warns about losing the link on a role change (`frontend/src/menu/screens/usb-role.js`).

### 4.3 The `/etc/usb-debugging-enabled` file and the usb-role drop-in, after the flag means boot-time state

The drop-in's guard (`10-thing-revived-default-device.conf`, F5) must keep reading the file exactly as it does today: it already only encodes the build flag, baked at image time, never touched at runtime under this plan. No change is needed to the drop-in itself. What must not happen is any attempt to make the runtime adb toggle write or remove that file, because the drop-in's `superbird-usb-role` call only runs once, early at boot (`Before=android-tools-adbd.service`), so a later write would have no effect on the running role anyway, and would also violate the volatility requirement if it landed on the `/etc` overlay. The two concerns (boot-time forced device role, runtime adb toggle) are already cleanly separated by virtue of the drop-in running once before either new unit pair starts.

If the running or persisted role is host, adb is impossible: there is no device-mode gadget for adbd to bind to. The runtime toggle must check the live role (`UsbRoleCtl::status().role`, the port already used by `usb_role.rs`) before attempting to enable adb, and refuse with a typed error (`usb_role_host`) rather than attempting the systemctl calls and failing confusingly. The frontend shows this as a row note pointing at the existing USB-mode screen ("Switch USB mode to Device, then reboot, to use ADB"), not as a toast, consistent with the existing `usb-role.js` note pattern for the `adb_forced` case.

### 4.4 UART: today's mechanism, and the runtime-safe alternative

Today's autologin is a `sed` on the shared template `serial-getty@.service`'s `ExecStart` line, applied at image build time only when `THING_REVIVED_ENABLE_UART=1` (`thing_revived_console_gate`, F6). This cannot be the runtime mechanism for two reasons: the template is shared by every potential serial-getty instance (editing it live risks instances this plan has no business touching), and a systemd drop-in cannot be written to the read-only rootfs at runtime, as the brief notes.

The runtime-safe alternative, preferred over writing a drop-in into the `/etc` overlay's writable upper: ship a second, instance-scoped unit, `thing-revived-uart-root@ttyAML0.service` (or non-templated `thing-revived-uart-root@.service` instantiated for `ttyAML0`), carrying the `--autologin root` `ExecStart` permanently, `BindsTo=dev-%i.device`, and `Conflicts=serial-getty@%i.service`. This unit is always present on the rootfs starting with this change, regardless of the flag. Whether it is enabled (symlinked into `getty.target.wants`, so it starts at boot instead of the stock getty) is the one thing the build flag controls, exactly mirroring the adb/usb-bind pair in section 4.1. The toggle itself is `systemctl start thing-revived-uart-root@ttyAML0.service` to enable (the `Conflicts=` stops the stock getty automatically) and `systemctl stop thing-revived-uart-root@ttyAML0.service; systemctl start serial-getty@ttyAML0.service` to disable (the stock getty is not auto-restarted by systemd just because the conflicting unit stopped, so the second call is required). No file is written anywhere at runtime for this toggle; justification: it is the one option in section 3 of the "persistence of account files" list category (the account-lockdown plan's own section 3) that this change can reuse without new writable storage, and it keeps the volatility guarantee trivially true by construction rather than by a reset-on-boot script that has to remember to clean up.

Build-time assertions change from "serial-getty has no autologin unless the flag is 1" to: the autologin unit `thing-revived-uart-root@.service` exists on the rootfs unconditionally, is enabled (present in `getty.target.wants`) only when `THING_REVIVED_ENABLE_UART=1`, and the stock `serial-getty@ttyAML0.service` symlink is the complement (present when the flag is `0`, since both cannot be enabled at once without the boot racing the `Conflicts=` transaction). `check_image_accounts.py` and the bbappend's `thing_revived_assert_accounts` gain the symmetric check for both possible build values.

## 5. Controller design

### 5.1 Layering, following `usb_role.rs` exactly

| Layer | File | Content |
|---|---|---|
| domain | `controller/src/domain/debug_access.rs` (new) | `DebugAccessKind { Adb, Uart }`, `DebugAccessStatus { enabled: bool, build_default: bool }`, parsing of `systemctl is-active` output (`active` -> true, anything else -> false, matching the existing `parse_status` style), a pure `validate_kind` and `validate_confirm` pair returning `AppError`. |
| ports | `controller/src/ports.rs` (append) | `trait DebugAccessCtl: Send + Sync { fn status(&self, kind: DebugAccessKind) -> Result<DebugAccessStatus, AppError>; fn enable(&self, kind: DebugAccessKind) -> Result<(), AppError>; fn disable(&self, kind: DebugAccessKind) -> Result<(), AppError>; }` plus a second, narrow port `trait AdminSessionGate: Send + Sync { fn require(&self, conn: ConnId) -> Result<String, AppError>; }`. |
| adapters | `controller/src/adapters/debug_access_cli.rs` (new) | Runs `systemctl is-active <unit>`, `systemctl start <unit>`, `systemctl stop <unit>` with fixed argv built from the `DebugAccessKind`, never from client input, no shell, same `Command::output` pattern as `usb_role_cli.rs`. Unit names come from `[debug_access]` in `controller.toml` (`adb_unit`, `adb_disabled_unit`, `uart_unit`, `uart_disabled_unit`), the same way `[usb_role]` carries its program names, so a renamed unit is a config change, not a recompile. |
| adapters (bridge) | `controller/src/services/accounts.rs` (small append) | `AccountsService` implements `AdminSessionGate` by delegating to its existing `Sessions::require`, exposed as `pub fn admin_gate(self: &Arc<Self>) -> Arc<dyn AdminSessionGate>`. This is the only change to that file; it adds no new state. |
| services | `controller/src/services/debug_access.rs` (new) | `DebugAccessService` holds `Arc<dyn DebugAccessCtl>`, `Arc<dyn UsbRoleCtl>` (reused, read-only, to check the live role before enabling adb), `Arc<dyn AdminSessionGate>`, `Arc<dyn TargetedSink>`. One mutation mutex, mirroring `AccountsService`'s `mutate` lock, so two toggle requests on different connections cannot race the same unit pair. |
| app | `controller/src/app/{router,policy,wire_map}.rs` | New router arms for `debug_access_get` and `debug_access_set`; `policy::execution` marks both `Blocking` (they spawn processes); `wire_map` adds the new command and event variants. |
| protocol | `controller/src/protocol/{command,event}.rs`, `config/protocol/controller/**` | New variants and golden fixtures (section 5.3). |

### 5.2 Server-side gate, exactly once, before any systemctl call

`debug_access_set` handler sequence, all server-side, none of it delegated to the frontend's own checks:

1. `AdminSessionGate::require(conn)` -> `not_unlocked` on failure, same wire code and same meaning as every Users mutation.
2. `confirm` must equal the kind's canonical string (`"adb"` or `"uart"`), checked with the same `require_echo`-style exact-match helper already in `accounts.rs`; otherwise `confirm_required`.
3. For `kind == Adb` and `enabled == true` only: `UsbRoleCtl::status()` is queried live; if the running role is not `Device`, reply `usb_role_host` and do nothing else.
4. The mutation mutex is taken, `DebugAccessCtl::enable` or `::disable` runs, and the result is re-read with a fresh `status()` call rather than assumed from the write's own exit code, matching F8's "never trust a cached bool."
5. Any adapter error (spawn failure, non-zero exit, a status mismatch after the attempted change) is reported as `toggle_failed` and the service leaves the surface in whatever state `status()` now actually reports; no attempt is made to guess and no optimistic event is sent. If disabling failed outright, the badge-driving `debug_access` event still reflects the true (possibly still-enabled) state, so the UI never under-reports an active debug surface.

### 5.3 Wire protocol additions

Same conventions as `config/protocol/controller/README.md`: JSON `type` tag, explicit `null`, field order as emitted, golden fixtures named with a third segment for variants.

Commands: `debug_access_get` (no fields), `debug_access_set` (`kind`: `"adb"` or `"uart"`, `enabled`: bool, `confirm`: string).

Events: `debug_access` (`adb: { enabled, build_default }`, `uart: { enabled, build_default }`), `debug_access_error` (`context`, `code`, `message`).

Error codes: `not_unlocked`, `confirm_required`, `invalid_kind`, `usb_role_host`, `toggle_failed`, `rate_limited` (reuses the existing mutation limiter per connection, same as `AccountsService::mutation_gate`).

Golden fixtures: `command.debug_access_get`, `command.debug_access_set`, `command.debug_access_set.disable`, `event.debug_access`, `event.debug_access.both_off`, `event.debug_access_error`, registered in `controller/tests/wire_golden.rs` the same way the usb_role and users fixtures are.

### 5.4 Volatile guarantee, made structural, not procedural

Nothing in `DebugAccessCtl`, `DebugAccessService`, or the new units ever opens a file under `/var` or the `/etc` overlay's upper layer. The guarantee is not "remember to not persist it"; it is architectural, because the only state that exists is systemd's in-memory unit activation state and the configfs gadget state in `/sys`, both of which are reset by a reboot by construction. `controller/tests/layering.rs` gets an assertion that `debug_access_cli.rs` opens no path under `/var` (the same style check that already restricts `std::process::Command` usage to `adapters/`).

### 5.5 Fail-safe behaviour

Startup: the controller never assumes a surface is off; `debug_access_get`'s first reply after boot already comes from a live `systemctl is-active`, so whatever the build-time unit enablement actually produced is what is reported, with no separate "trust the flag" code path. A controller crash mid-toggle leaves the units in whatever state the last successful systemctl call left them (not necessarily "off"), which is why `status()` is always re-queried rather than remembered; the next client to ask gets the truth, and the badge (driven by the same event) will show "enabled" if it genuinely still is. This is intentionally different from "default to disabled on error": disabled-on-error applies to the toggle action's own failure handling (step 5 of 5.2), not to a query, because a query that lies and says "disabled" while adb is actually reachable would be worse than an accurate badge.

## 6. Frontend design

New screen `frontend/src/menu/screens/debug-access.js`, listed under Settings, behind the Users unlock gate exactly like the Users section: entering it while locked shows the same `users-kit.js` `lockedRows()` redirect to `users-unlock` (reusing `screen()` from `users-kit.js` with `open: false`), because the spec names the Users admin unlock specifically, not a separate password.

Rows, 800x480, Chromium 68/69 subset, classic script, no flexbox `gap`, no `??`, no optional chaining (same constraints as every other screen, enforced by `frontend/tests/syntax-guard.test.js`):

- A label row per surface showing `ADB: ON (build default: OFF)` or the equivalent, driven by `debug_access_get` on `onEnter`.
- One hold-to-confirm toggle row per surface (`hold: true`, like `usb-role.js`'s `Reboot now` row), sending `debug_access_set` with the matching `confirm` string baked into the row definition, never typed by the user (the hold gesture plus the already-unlocked session is the confirmation the owner asked for; the `confirm` field exists for the server-side check in 5.2, not as a second typed prompt).
- A note row, shown only when `kind == adb` and the live role is host, pointing at the USB-mode screen, mirroring the `adb_forced` note already in `usb-role.js`.
- A warning row stating plainly that the toggle does not survive a reboot and that enabling either surface removes an authentication layer (no password for adb, no password for the UART root shell), matching the tone of the existing USB-mode screen's warning rows.

Persistent indicator: a status-bar badge, not a row on this screen and not a toast, so it is visible from any screen including the terminal and the clock. This lives in the shared menu chrome (the header or status area already rendering clock and connectivity state, not yet named here because it has not been located in this pass; the implementing task must find it, likely under `frontend/src/menu/**` or `frontend/index.html`'s top-level chrome). The chrome reads `domain.debugAccess.adb.enabled || domain.debugAccess.uart.enabled` from the shared `ctx.domain` the same way every other screen does, and renders one small persistent label such as `DEBUG` when true. Because this touches a file every other screen's rendering also depends on, it is kept as its own task with no other task touching the same file in the same wave (section 9).

## 7. Security review

What this changes in the trust model: today, enabling adb or UART root autologin requires rebuilding and reflashing the image, an action that needs the build host and, on a managed fleet, whatever approval gate sits in front of a flash. After this change, anyone who can read the kiosk password off the owner (or who has it because they are the owner) and is physically at the device can enable an unauthenticated root shell over USB or a passwordless root shell over the serial port, with no rebuild. This is a deliberate owner trade-off (debugging convenience without a reflash loop) and is accepted, but it should be written down exactly this plainly in `docs/users.md`, next to the existing "Single factor remote root" and "Physical access" notes in that file's Security notes section, because it is a new physical-access escalation path that the account-lockdown plan's threat model (section 12 of `user-management-plan.md`) did not anticipate when it listed adb and UART autologin only as build-time dev flags.

Lingering-session risk: the task brief asks directly whether adb should auto-disable when the admin session locks. Recommendation: yes. The admin unlock is what gates the toggle; once that session ends (idle timeout, explicit lock, or the connection closing), a debug surface left enabled is no longer protected by anything except "someone has to notice." Auto-disable on session end is a small addition to `AccountsService`'s existing `lock_session` and idle-expiry paths (the same place `drop_sessions_if_not_admin` already runs side effects when an admin stops being effective) and keeps the badge and the actual exposure in sync with the one thing the owner can see is active, the unlock state. This is listed as a ranked open question in section 8 rather than assumed, because it is a behaviour change the owner has not explicitly signed off on, but it is called out here as a plain recommendation.

What the old build flags previously guaranteed and is now lost: a prod image previously could not have adb or UART root autologin turned on without a new build and flash, full stop, a hard physical-world gate (someone has to have build tooling and flash access). After this change, a prod image with the default flags still ships with both off, but the ceiling on "how hard is it to turn on" drops to "know the kiosk password and hold a knob for one second," on every build, including prod. The owner should see this stated exactly once, plainly, before approving: this plan trades a build-time gate for a password-plus-physical-presence gate, on purpose, because the alternative (no live toggle at all) is the status quo the owner is asking to change.

Everything else in the account lockdown's threat model is unchanged: the loopback WebSocket is still untrusted by default and still needs the admin session for any mutating command; root is still locked; adb still has no authentication of its own when it is on, which is why the toggle, not adb itself, is what carries the new risk.

## 8. Test plan

| Level | What | Where |
|---|---|---|
| Domain unit | `DebugAccessKind` parsing, `DebugAccessStatus` round trip through `systemctl is-active` text variants (`active`, `inactive`, `failed`, `activating`), confirm-string validation | `controller/src/domain/debug_access.rs` |
| Adapter | argv for `status`, `enable`, `disable` against a recording fake tool, exactly like `usb_role_cli.rs`'s `rig()` helper; a non-zero exit is a `toggle_failed`-mapped error, not a panic | `controller/src/adapters/debug_access_cli.rs` |
| Service with fakes | not-unlocked refusal, confirm-string refusal, `usb_role_host` refusal when the fake `UsbRoleCtl` reports host, a successful enable and disable round trip against a fake `DebugAccessCtl`, a failing adapter call leaves the reported status as whatever the fake's post-call `status()` says (not optimistically "disabled"), concurrent toggles serialise through the mutation mutex | `controller/src/services/debug_access.rs` |
| Source guard | extend `controller/tests/layering.rs`: `debug_access_cli.rs` is the only file calling `systemctl` for this feature, opens no path under `/var`; extend `controller/tests/no_secret_logging.rs` only if a future change adds any secret-shaped field (none are planned here) | `controller/tests/` |
| Protocol golden | every new fixture parses or matches byte for byte | `controller/tests/wire_golden.rs`, `frontend` bridge-client and types tests |
| Frontend unit | row builders for locked and unlocked states, the host-mode note only appearing when the fake domain says host, the badge's shared-chrome logic in isolation | `frontend/tests/*.test.js` |
| Frontend e2e | enabling adb (fake server), the badge appearing, confirming the hold gesture is required (a tap alone does nothing) | `frontend/tests/e2e/menu-regression.mjs` |
| Image assertions | `build/tools/check_image_accounts.py` and the bbappend's `thing_revived_assert_accounts`: the new UART unit pair's enablement matches the flag, the adb/usb-bind pair's enablement matches the flag, both units always exist on the rootfs regardless of flag value | `build/tools/` |
| Device, human or orchestrator only | see the Phase 0 spike below; this is what G1-equivalent device checklists in this plan depend on | docs, run by a human with recovery (burn-mode reflash) available |

### Phase 0 spike (mandatory before any implementation task below starts)

This mirrors T0 in `user-management-plan.md` exactly: a report-only phase, Class A (read-only), Class B (tmpfs-only, reversible), and Class C (destructive, owner-run only) safety classes, answering:

- (a) Does `systemctl stop android-tools-adbd.service` followed by `systemctl start thing-revived-usb-bind.service` cleanly release and rebind the UDC on this hardware, with the host seeing the NCM link come back, without a kernel-level USB reset or a reboot? (Class B/C: requires toggling adbd on the real device; recovery is a reboot, which is always safe here.)
- (b) Can the `ffs.adb` configfs link be recreated after `thing-revived-usb-bind` has removed it, without re-running `superbird-usb-gadget`'s full composition, so that re-enabling adb after a disable does not need to tear down `ncm.usb0` too? (Class B: inspect configfs structure, attempt a manual `mkdir`/symlink recreation in a private namespace first if possible, else Class C on the real gadget.)
- (c) (Verify by reading the TEXT output of `systemd-analyze verify` for an ordering cycle line; its exit code is 0 even when a cycle is printed, as this repo already hit once.) Does `Conflicts=` between `android-tools-adbd.service` and `thing-revived-usb-bind.service`, with both left enabled at their stock `[Install]` targets, cause a boot-time race, or does systemd's transaction resolution reliably pick the one whose Condition (today's file-based gate, kept for boot time only) is true? (Class A: `systemd-analyze verify`, dry-run with `systemctl --dry-run` if available on this systemd version.)
- (d) Does `Conflicts=serial-getty@%i.service` on a templated `thing-revived-uart-root@.service` correctly stop the specific `serial-getty@ttyAML0.service` instance, or does systemd's template/instance Conflicts resolution behave differently than assumed? (Class B: enable the new unit in a private mount namespace test against a copy of the unit files, or Class C on the real tty if Class B cannot exercise it.)
- (e) What does an interactive `ssh` session over the NCM link actually observe during an adb enable/disable round trip: a clean disconnect and reconnect, or a longer outage? (Class C, owner-run, with a second access path, such as the touchscreen terminal or a Bluetooth keyboard, available before starting.)

Gate: the spike report (`docs/research/debug-toggles-spikes.md`, new, owned entirely by the spike task) answers (a) through (e) with a verified or unverified tag. No controller or build task below starts before this gate, the same dependency shape as T0 gating Phase 1 in the account-lockdown plan.

## 9. Task breakdown

Sizes: S under half a day, M half to one and a half days, L one and a half to three days. A task owns its files exclusively; within a wave no two tasks share a file.

### Wave 0: spike (orchestrator-run, no delegated children)

- **T-D0.1** (M) the Phase 0 spike above. Owns `docs/research/debug-toggles-spikes.md` only. Classes B and C items need the real device and are run by the orchestrator, not a delegated craftsman, because they involve `systemctl start/stop` of live units and configfs writes, which are outside this plan's own read-only device-access rule. Gate G0.

### Wave 1: build-side units (depends on G0 for items that touch live unit behaviour; the pure file-authoring parts do not)

- **T-D1.1** (M) adb/usb-bind unit pair. Owns `build/overlay/recipes-extended/thing-revived-usb-bind/**` (the `Conflicts=` drop-in addition) and a new `build/overlay/recipes-bsp/android-tools/android-tools-adbd/**` or equivalent bbappend path for the `Conflicts=` drop-in on the adbd side (the `ExecStop` already exists upstream, do not add one) (the exact recipe path for overriding this unit's drop-in needs locating during this task; it is not yet in the repo, since today's `superbird.conf` drop-in ships from `meta-superbird`, not this repo's overlay, and this plan needs its own drop-in layered after it). No G0 dependency for authoring the files; G0 (a) and (c) must pass before this task's units are considered done, not before they are written.
- **T-D1.2** (M) UART unit pair. Owns a new `build/overlay/recipes-extended/thing-revived-uart-root/**` recipe (unit file, any helper script) and the `thing_revived_console_gate` function in `build/overlay/recipes-core/images/superbird-kiosk-prod-image.bbappend` (rewritten to enable one of the two units by flag instead of sed-ing the shared template). Depends on G0 (d).
- **T-D1.3** (M) image assertions. Owns `build/tools/check_image_accounts.py`, `build/tools/test_check_image_accounts.py`, `build/tools/fixtures/**`, and the `thing_revived_assert_accounts` function in the same bbappend file as T-D1.2 touches; coordinates with T-D1.2 on that one file through the orchestrator rather than both editing it in the same wave. Starts after T-D1.1 and T-D1.2 land.

### Wave 2: controller (depends on G0; may start alongside wave 1)

- **T-D2.1** (M) domain and ports. Owns `controller/src/domain/debug_access.rs` (new), the `DebugAccessCtl` and `AdminSessionGate` additions to `controller/src/ports.rs`.
- **T-D2.2** (M) adapter. Owns `controller/src/adapters/debug_access_cli.rs` (new), the matching entry in `controller/src/adapters/mod.rs`, the `[debug_access]` section of `controller/src/utils/config.rs` and `config/controller.toml`. Depends on T-D2.1 for the port signatures.
- **T-D2.3** (S) protocol. Owns `controller/src/protocol/**` additions, `config/protocol/controller/**` new fixtures, `controller/tests/wire_golden.rs` entries, `controller/src/utils/types.rs`. No dependency on T-D2.1 or T-D2.2; may start immediately after G0.
- **T-D2.4** (S) the one-line bridge in `accounts.rs`. Owns only the `AdminSessionGate` impl block and the `admin_gate()` accessor added to `controller/src/services/accounts.rs`; nothing else in that file changes. Depends on T-D2.1 for the trait signature.

### Wave 3: controller wiring (depends on wave 2)

- **T-D3.1** (L) service and app wiring. Owns `controller/src/services/debug_access.rs` (new), `controller/src/app/{router,policy,wire_map}.rs`, `controller/src/lib.rs`, `controller/tests/layering.rs` additions. Depends on T-D2.1 through T-D2.4.

### Wave 4: frontend (depends on T-D2.3 for fixtures; may start alongside wave 3)

- **T-D4.1** (M) screen and logic. Owns `frontend/src/menu/screens/debug-access.js` (new), any pure logic it needs under `frontend/src/users/**` or a new `frontend/src/debug-access/**`, and its unit tests. Depends on T-D2.3.
- **T-D4.2** (S) persistent indicator. Owns exactly the shared chrome file(s) the badge needs (to be located during this task; likely one file under `frontend/src/menu/**` or `frontend/index.html`), kept deliberately tiny and reviewed carefully since it is a shared-file touch. Depends on T-D2.3 for the event shape only; does not depend on T-D4.1.

### Wave 5: docs (depends on all above)

- **T-D5.1** (M) docs. Owns `docs/users.md` (Security notes addition, section 7 above), `docs/hardware.md` (USB-C port section, the adb/usb-bind pair description), `docs/development.md` (flag description update), `build/README.md` (flag semantics update), `config/README.md` and `config/controller.toml` (`[debug_access]` defaults), `config/protocol/controller/README.md`.

### Wave 6: integration (orchestrator or owner only, no file ownership)

- **T-D6.1** full image build with both flag values, flash on the dev device, run the device checklist below.

Tasks that cannot be run by delegated craftsmen at any point: T-D0.1's Class B/C items, and T-D6.1 in full, because both need `systemctl start/stop` of live units, configfs writes, or a flash, all outside the read-only device-access rule this plan itself was written under.

### Device checklist (G-Dv, run after T-D6.1's flash, by a human or the orchestrator)

- With `THING_REVIVED_ENABLE_ADB=0`: `adb devices` offers no shell; Settings, Debug access, unlock, hold-confirm "Enable ADB"; `adb devices` now offers a shell; the badge appears on every screen including the terminal; reboot; `adb devices` offers no shell again (volatility); the kiosk password did not change and no file under `/var/lib/thing-revived` changed (`find /var/lib/thing-revived -newer <marker>` before and after).
- With `THING_REVIVED_ENABLE_UART=0`: the serial console shows a normal login prompt; enable UART from the menu; the serial console now drops straight to a root shell with no password; disable; the normal prompt returns; reboot; the normal prompt is back (volatility).
- Enabling adb while the USB role is host is refused with the stated note, and the USB-mode screen can switch back to device and reboot to make adb usable again.
- Locking the admin session (idle or explicit) while a surface is enabled: confirm the owner's decision from section 7's open question and that the implemented behaviour matches it.
- A toggle attempted while storage or the systemctl call is deliberately broken (simulate by renaming a unit file) reports `toggle_failed` and the badge matches the true, re-queried state, not an optimistic guess.

## 10. Ranked open questions for the owner

1. Should enabling adb or UART also auto-disable when the Users admin session locks (idle timeout, explicit lock, or disconnect)? Recommendation in section 7 is yes. This changes behaviour beyond what the brief stated outright and should be confirmed before T-D3.1 implements it either way.
2. Does the Phase 0 spike's item (a) and (b) (live UDC release and `ffs.adb` relink without a reboot) need to succeed for this feature to ship at all, or is a documented "the NCM link may drop for a few seconds during an adb toggle" an acceptable outcome? This decides whether risk 1 is a blocker or a documented limitation.
3. Should the persistent badge distinguish adb from UART (two indicators, or one label naming which), or is a single generic "DEBUG" badge sufficient? Affects T-D4.2's scope only.
4. Is a reboot allowed to interrupt a toggle request mid-flight (for example, the USB-mode screen's own "Reboot now" row, which this plan does not change), or should the controller refuse a reboot while a toggle is in progress? Given the mutation mutex already serialises toggle calls, this is likely a non-issue, but the owner should see it named once.
5. Should the image assertions (`check_image_accounts.py`) fail a build where both the adb-pair and the UART-pair units are missing their `Conflicts=` drop-in, or only warn? The account-lockdown plan's precedent (`bbfatal` for every account-related guarantee) suggests fail, but this plan's units are new enough that the owner may prefer a warning during the first build cycle.
6. Does the owner want the badge visible during the first-boot password screen (`docs/users.md`'s M-button flow), which today blocks all other input? Default assumption here is no, since that screen is deliberately exclusive of everything else, but it should be named rather than assumed silently.
7. Should the `confirm` string sent by the hold gesture be visible anywhere in the UI (for audit, matching the Users menu's typed `confirm` for destructive rows), or is the hold gesture itself sufficient proof of intent, as proposed in section 6? The Users menu requires the admin to type the exact name for destructive actions; this plan's hold-only gesture is a lighter bar, which may or may not be what the owner wants for a security-relevant toggle.

## 11. Acceptance criteria

- [ ] On a prod image built with both flags at `0`, the Settings menu shows a Debug access screen behind the Users unlock, with both surfaces reporting disabled and matching the live `systemctl is-active` state on a fresh boot.
- [ ] Enabling adb from the menu makes `adb devices` (from a host over the existing USB link) offer a shell within the spike's confirmed timing, with no change to `/etc/usb-debugging-enabled` or any file under `/var`.
- [ ] Enabling UART from the menu makes the serial console autologin as root with no password, with no change to any file under `/var` or the `/etc` overlay's upper layer.
- [ ] A reboot after either toggle was enabled returns both surfaces to the build's `0` default, verified by `systemctl is-active` on both unit pairs immediately after boot.
- [ ] Every `debug_access_set` call is refused with `not_unlocked` on a connection without an unlocked Users admin session, refused with `confirm_required` without the correct confirm string, and refused with `usb_role_host` when attempting to enable adb while the USB role is host, all enforced in the controller, independent of what the frontend sends.
- [ ] The persistent badge is visible on at least three different screens (a menu screen, the terminal, and the clock or idle screen) whenever either surface is enabled, and disappears within one status query interval of both being disabled.
- [ ] `cargo test`, `cargo clippy -- -D warnings`, `node --test frontend/tests/*.test.js`, and the image check script's unit tests all pass with the new files in place.
- [ ] The Phase 0 spike report exists and tags every one of its five questions verified or unverified; no controller or build task claims a mechanism the spike left unverified without saying so in its own file's comments.
- [ ] `docs/users.md` states the new trust-model change from section 7 in its Security notes section, in the same plain style as the existing entries there.

## 12. Owner decisions (2026-11-24)

The owner accepted the recommendation on all seven open questions in section 10. Auto-disable both surfaces when the admin session ends: yes. A few seconds of USB link drop during a toggle: acceptable, so the spike result is not a ship blocker. One generic DEBUG badge. The controller refuses a reboot while a toggle is in flight. Missing `Conflicts=` drop-ins fail the image build. No badge on the first-boot password screen. Hold-to-confirm alone is sufficient, no typed confirm string. The review in `debug-toggles-plan-review.md` set two required corrections (the `ExecStop` claim and the `systemd-analyze verify` text check), both applied above.
