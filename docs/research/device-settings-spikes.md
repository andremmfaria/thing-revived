# Device settings: Phase 0 spike report

Run on 2026-11-24 on the dev device (ADB build, image diag4, systemd 259.5, BusyBox shell without `timeout`) by the orchestrator. Scope is section 11 of `device-settings-plan.md`. Each item is tagged verified or unverified. Items that need the new units or binary are deferred to wave 5 and are listed at the end.

## Results

| Item | Result | Evidence |
|---|---|---|
| P0.1 live ssh toggle | Verified | `sshd.socket` is the listener (enabled, `Accept=yes`, `ListenStream=22`, `Conflicts=sshd.service`). `sshd@.service` is an indirect template and no plain `sshd.service` is enabled. `sshd-unix-local.socket` is a generated AF_UNIX socket and is not a network listener. From vermelho, with the socket running a connection to 10.42.1.26 port 22 gets `Permission denied (publickey,password)`, after `systemctl stop sshd.socket 'sshd@*.service'` it gets `Connection refused`, and after `systemctl start sshd.socket` it is accepted again. A drop-in with `ConditionPathExists=!/run/thing-revived/ssh-off` makes `systemctl start sshd.socket` a silent no-op while the flag exists (journal: `sshd.socket skipped, unmet condition check`), and it starts normally once the flag is removed. |
| P0.2 live host switch | Verified | `superbird-usb-role to-host --no-revert` prints `host role set; no automatic revert` and `status` then shows `role: host` and `revert: none`. The role switch alone leaves adbd active and the UDC file unchanged. The USB link vanished from the host for about 19 s while the role was host (ping fail 20:24:46 to 20:25:13, lsusb 0 then 1). |
| P0.3 live host to device | Verified | `superbird-usb-role to-device` restores the gadget by itself when the UDC stayed bound: `lsusb` shows the gadget at once and ping recovers about 8 s later. No restart of adbd or usb-bind was needed in that case. |
| P0.4 usb-bind and adbd in host role | Partly verified | With role host, `systemctl stop android-tools-adbd` empties the UDC (upstream ExecStop), and `systemctl start android-tools-adbd` succeeds (rc 0) and binds the UDC again, with no hang or error. `systemctl start thing-revived-usb-bind` returned 0 but did nothing on this image because the old image still carries its `ConditionPathExists=!/etc/usb-debugging-enabled`. Whether the new usb-bind unit behaves in host role stays unverified until wave 5. The plan's rule that the failsafe never starts usb-bind in host role stays as the conservative choice. |
| P0.9 console and getty generator | Verified | `/proc/cmdline` has `console=ttyAML0,115200n8`. `/run/systemd/generator/getty.target.wants/serial-getty@ttyAML0.service` exists, so the getty generator starts a stock serial getty on the console tty whenever nothing else is arranged. A Conflicts= from `thing-revived-uart-root@ttyAML0` therefore does the right thing when it starts later, and the stock getty must never be masked (a masked unit would break the disable path). With UART off, the console shows the normal login prompt, which needs a password, so root stays locked. |
| P0.10 data partition and factory reset | Verified | `/var` is `/dev/mmcblk0p7` (ext4, `x-systemd.makefs`). `state.json` kept its 01:45 timestamp and `boot-role` its older value across the burn-mode flashes of this week, so a burn-mode flash does not wipe `/var`. `superbird-factory-reset.service` runs `/usr/libexec/superbird-factory-reset`, which reads `fw_printenv factory_reset`, clears it, and runs `wipefs -a` on the partition labelled `data`. That wipes `device-settings.json` with everything else on `/var`. `factory_reset` is currently unset. |

## Deferred, with reasons

| Item | Why | When |
|---|---|---|
| P0.5 `systemd-analyze verify` on the real unit set | Needs the units from T3 and a built rootfs. | T3 gate and wave 5. |
| P0.6 boot apply ordering with stubs | Needs the T3 unit drafts. It needs a reboot of the device. | After wave 1, before wave 3. |
| P0.7 persisted-file edge cases | Rust unit tests in T5, real binary in wave 5. | Wave 2 and 5. |
| P0.8 live UART through `login` | The uart-root unit is not on this image. | Wave 5. |

## Consequences for the plan

1. Host and device switching are verified as plain `to-host --no-revert` and `to-device`, and the link returns by itself. The host side sees about 19 s of outage plus about 8 s of NCM recovery, matching the plan's warning text.
2. The sshd gate design is verified as written: the unit is `sshd.socket`, the flag is under `/run/thing-revived`, and a failed condition is silent.
3. P0.9 confirms the plan's decision not to mask the stock getty: the generator starts it by default, and uart-root takes over through Conflicts=.
4. Nothing found requires a change to the plan.
