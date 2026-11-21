# Hardware At a Glance

Full detail and evidence trail: [`research/hardware-verification.md`](research/hardware-verification.md) (live shell findings) and [`research.md`](research/research.md) §1/§8 (teardown-sourced background). This file is a quick-reference summary only: if a number here ever disagrees with those files, the research files are authoritative.

| Property | Value |
|---|---|
| SoC | Amlogic S905D2 (G12A family) |
| GPU | Mali-G31 MP2 (Bifrost), Mesa/Panfrost acceleration |
| RAM | ~488MB total (512MB nominal) |
| Display | 4" 800×480 touchscreen, I2C controller (`tlsc6x`) |
| Input | `/dev/input/event0` = `gpio-keys`, 7 keycodes: `KEY_ESC`=1, `KEY_1`..`KEY_4`=2..5, `KEY_ENTER`=28 (knob-push), `KEY_M`=50 (see [`research/button-knob-clock-design.md`](research/button-knob-clock-design.md)); `/dev/input/event1` = knob rotation (`rotary@0`, `REL_HWHEEL` deltas, not discrete keys); `/dev/input/event3` = touchscreen |
| OS / init | `yocto-superbird`, systemd-managed services |
| Userspace ABI | `aarch64-unknown-linux-gnu` |
| Device access | SSH over USB CDC-NCM |
| On-device browser | Chromium kiosk |

See [`architecture.md`](architecture.md) for how these constraints shaped the system design, and [`yocto-build-and-flash.md`](yocto-build-and-flash.md) for the build/flash workflow.
