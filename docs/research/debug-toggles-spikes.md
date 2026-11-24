# Debug toggles: Phase 0 spike report

Run on 2026-11-24 against the dev device (ADB build, systemd 259.5) by the orchestrator, as required by `debug-toggles-plan.md` section 8. Gate G0 is passed: every mechanism the plan depends on is verified, with the caveats below.

## Results

| Item | Question | Result | Evidence |
|---|---|---|---|
| (a) | Does stopping adbd then starting the usb-bind script release and rebind the UDC cleanly, without a reboot? | Verified | `systemctl stop android-tools-adbd` returned 0 and the upstream `ExecStop` left `UDC` empty. `thing-revived-usb-bind start` then bound `ff400000.usb` with only `ncm.usb0` in `configs/c.1`. The run was driven by a transient unit that survived the link drop. |
| (b) | Can the `ffs.adb` link be recreated without recomposing the gadget? | Verified | With the UDC unbound, `ln -s $CFG/functions/ffs.adb $CFG/configs/c.1/ffs.adb` returned 0, `systemctl start android-tools-adbd` came up active, its `ExecStartPost` bound the UDC again, the FFS mount survived (`functionfs` still mounted) and `adb devices` listed the device afterwards. The `functions/ffs.adb` directory is never removed by usb-bind, only the config link. |
| (c) | Does `Conflicts=` between the adbd and usb-bind units behave, and does `systemd-analyze verify` show a cycle? | Verified with throwaway units on a user manager | Starting either unit stopped the other. `Conflicts=` plus `After=` on the starting side is enough for the stop to finish before the start (the plan's `ExecStop` hand-back relies on this). `systemd-analyze verify` printed nothing; its exit code is not trusted, the text output is checked. `systemd-analyze` is not installed on the device, so verification runs on the build host. |
| (d) | Does `Conflicts=serial-getty@%i.service` on a templated unit stop only the matching instance? | Verified with throwaway units | Starting `uart-root@ttyA` stopped `getty@ttyA` and left `getty@ttyB` running. Stopping the UART unit does not restart the getty, so a disable must start the stock getty explicitly. The real tty behaviour (autologin prompt) is part of the device checklist, not the spike. |
| (e) | What does the host see during a round trip? | Verified | The NCM interface stays and the ping outage lasts about 6 to 8 seconds per transition (16:41:29 fail, 16:41:37 ok on disable; 16:41:55 fail, 16:42:01 ok on enable). The link recovers by itself with no host action and no kernel USB reset. An ssh session over the link would drop and need reconnecting. |

## Consequences for the design

1. The runtime ADB enable sequence is: unbind is implicit in `Conflicts=` stopping usb-bind (its `stop` clears the UDC), then relink `ffs.adb`, then adbd starts and binds. The relink must be an `ExecStartPre` on the adbd side, and it must be idempotent (`ln -sf`).
2. The runtime ADB disable sequence is: start usb-bind, which stops adbd through `Conflicts=` (the upstream `ExecStop` unbinds), then the script removes the link and rebinds with NCM only.
3. The `ConditionPathExists=/etc/usb-debugging-enabled` gate on adbd and the negated gate on usb-bind must be cleared by a drop-in layered after the upstream one, because a condition that skips a manual start would make the runtime toggle a silent no-op. Boot-time state comes from which unit is enabled, decided by the build flag.
4. A transition costs a link drop of up to about 10 seconds. The UI says so before the hold completes, and the controller does not retry.
5. The UART disable must start `serial-getty@<tty>.service` explicitly after stopping the root console unit.
