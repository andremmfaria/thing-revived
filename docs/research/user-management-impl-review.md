# User management implementation review (pre-build)

Independent read-only review of HEAD before the single bitbake build. The reviewer ran out of iterations before writing this file, so the orchestrator transcribed the findings. Checked: delta spec, build overlay recipes, checker, PAM files against the BSP checkout on devel, controller services, first-boot overlay, menu-chart and users-logic. Not checked: sudo and openssh unit installs in a real image rootfs, the flag path in `build-image.sh` beyond a grep, `ssh_keygen.rs`.

## Blocker

1. `thing-revived-provision.service` lacks `DefaultDependencies=no` (`build/overlay/recipes-extended/thing-revived-accounts/files/thing-revived-provision.service:1-9`). It is `Before=sshd.socket`, and the default `After=basic.target` creates an ordering cycle through `sockets.target`. Reproduced with `systemd-analyze verify`; adding `DefaultDependencies=no` removes the cycle.

## Should

1. `mark_delivered` runs even when `emit_to` dropped the secret (`controller/src/services/users.rs:216-223`, `app/hub.rs`). The second M press could acknowledge a password the screen never received. Make `emit_to` report delivery and mark delivered only on success.
2. A state read error at the first `user_bootstrap_get` leaves no prompt until reconnect (`users.rs:150-159`). Retry on the next get and show a retry state in the frontend.
3. A corrupt `state.json` is treated as missing (`adapters/accounts_shadow.rs` near 479). With our own unacknowledged hash live this picks Adopt, which is the lockout the invariant prevents. Treat an unparsable file with a usable hash as unacknowledged.
4. `IPK_REQUIRED` in `build/tools/check_image_accounts.py:132` lacks `pam-plugin-faillock`. The name is confirmed in libpam 1.7.2 pkgdata on devel.
5. `rootfs_selftest.sh` does not run `systemd-analyze verify` on the shipped units, which is how the cycle got through.

## Nits

- Key files are user-owned 0600 (`adapters/authorized_keys_fs.rs:73`); root-owned 0644 would be tighter but sshd StrictModes accepts the current mode.
- `user_result` op names are inconsistent (`add` versus `user_group_add`), so toasts read oddly.
- `UnlockLimiter::press` is a stale one-slot credit, and a reconnect resets the per-connection backoff; only the global cap of 20 per 600 s limits guessing then.
- `check_can_unlock` returns `no_password` for a passwordless admin, which leaks that state.
- `build-flags` is written by both the accounts recipe and the image bbappend.
- The `enter-hold` timer is not blocked while the first-boot overlay is up (`menu-chart.js:419-428`).
- If `provision` fails, `sshd.socket` still starts; document the degraded mode.
- `user_secret` for menu-created users sits in reducer state readable over CDP; document it.

## Verified

- The `libpam_%.bbappend` sed produces correct faillock lines on the real OE `common-auth` and `common-account`, and the strings match the checker.
- `pam-plugin-faillock` exists; key logins are not blocked by faillock account phase.
- sshd `Match User kiosk` drop-in and key store directory modes suit StrictModes.
- First-boot state is written unacknowledged before `chpasswd`; relock on reboot works; provision precedes sshd and the controller.
- M gate has no stuck keys or orphan releases and arms only after a frontend get.
- Frontend input gating covers keyboard, `sendInput`, chart and document swallow.
- Secrets stay out of argv and logs; log policy is type-only for the new events.
- Guards for last admin, kiosk and system accounts are present and tested.
