# Bash Availability for the On-Device Terminal

## REVISION (2026-11-16): bash has since been added to the shipped image

This file's "Overall verdict" below recommended adding `bash` via `IMAGE_INSTALL:append = " bash"` once `yocto-superbird` became the base. That reflash happened the next night: see [`yocto-build-and-flash.md`](../yocto-build-and-flash.md#c-adding-missing-base-packages-and-the-two-new-daemons-to-the-image), whose §5c shows the real `IMAGE_INSTALL:append` line that now ships `bash` (plus `openssh-ssh`/`openssh-keygen`) on `superbird-kiosk-prod-image`. The research and reasoning below are otherwise unchanged and still accurate as background.

Research date: 2026-11-15. Scope: does the terminal feature's PTY backend get a real GNU bash, or is it stuck with BusyBox ash, on stock firmware vs. the community firmware options. Confidence levels and sources below; nothing here is device-specific-confirmed unless stated.

## 1. BusyBox ash (stock firmware, v1.36.0) interactive feature completeness

Line-editing, history (up-arrow), and tab-completion in BusyBox ash are each separately gated by `CONFIG_FEATURE_EDITING`, `CONFIG_FEATURE_EDITING_HISTORY`, `CONFIG_FEATURE_TAB_COMPLETION`, none forced on. Size-constrained embedded Buildroot builds routinely strip these to save a few KB. Even with editing enabled, ash's completion is file/command-only, not bash's programmable completion. No superbird-specific BusyBox `.config` was found to confirm this exact build's state.

**Confidence: medium** (general ash behavior well documented; this build's actual config not located).
Sources: https://en.wikipedia.org/wiki/BusyBox , https://github.com/mirror/busybox/blob/master/shell/Config.src , https://busybox.net/FAQ.html

## 2. Is real bash present anywhere today?

- **Stock firmware:** no evidence of bash; consistent with the project's existing finding that the userland is BusyBox-only. Confidence: medium-high.
- **yocto-superbird:** README explicitly lists `superbird-bsp-image`'s contents as "kernel, busybox, sshd, USB-CDC gadget" — bash not named, and no other variant names it either. Confidence: high (explicit, absent).
- **nixos-superbird:** no explicit mention either way. NixOS has a documented habit of not providing `/bin/bash` by default (confirmed via a NixOS Discourse thread on exactly this pain point); the project's `modules/default.nix` shows no shell-selection option. Confidence: medium, leans "not present as login shell."
- **superbird-debian-kiosk ("Wall Thing"):** Debian 11 Bullseye rootfs; Debian's `bash` package is Priority:required, installed by `debootstrap` on every stock Debian image. Strong inference, not an explicit README statement for the on-device rootfs specifically (the README's own host-side scripts use bash, confirming bash on the host, not necessarily the device image). Confidence: medium-high.

## 3. Adding bash to a yocto-superbird image

`bash` is a mainline OE-core recipe (`meta/recipes-extended/bash/`), not a bolt-on community layer — same tier as the `openssh` recipe this project already plans to use for an SSH client. Expected to be a one-line `IMAGE_INSTALL:append = " bash"`, identical pattern to the project's existing kernel-module-append syntax.

On the GLIBC-mismatch question this project already hit once (the Rust bridge daemon, built outside the Yocto toolchain, linked dynamically against a newer host glibc than the device's runtime): a `bash` built via the standard in-tree OE recipe shares the same sysroot/toolchain/glibc as every other package in that image, so it cannot drift to a newer glibc ABI the way an externally cross-compiled binary can. This is sound toolchain reasoning, not a sourced explicit statement that says so for this BSP.

**Confidence: high** on recipe availability; **medium** (inferred, not sourced) on the static-link non-issue.
Sources: https://github.com/openembedded/openembedded-core/blob/master/meta/recipes-extended/bash/bash.inc , https://github.com/JoeyEamigh/yocto-superbird (README, IMAGE_INSTALL:append pattern)

## 4. Cost

BusyBox ash is ~60KB added to an already-resident multi-call binary. Real bash + libreadline + a trimmed terminfo set is roughly 1.5-2MB total rootfs footprint, with sub-MB per-process RAM. Negligible against the device's confirmed ~323MB free RAM, especially next to an already-running Chromium kiosk. No superbird-specific storage-partition numbers found to sanity-check against, but the order of magnitude is not in question.

**Confidence: medium** (reasoned estimate, not profiled on this exact build).

## Overall verdict

Real bash is not shipped today on any of the three firmware options out of the box, except likely-by-default on `superbird-debian-kiosk` (inferred from "it's Debian," not confirmed explicitly). The clean path for this project, since `yocto-superbird` is already the recommended base for everything else (BT, kernel, the terminal feature itself): add `bash` via the standard `IMAGE_INSTALL:append = " bash"` OE-core recipe, same mechanism already planned for adding an SSH client. Cost is negligible, and the GLIBC-mismatch class of problem that bit this project once should not recur because it's built in-tree against the same sysroot as the rest of the image. Stock firmware has no path to bash without replacing the OS; `nixos-superbird` is the one option where it's genuinely uncertain and may need deliberate wiring against NixOS's own habit of omitting `/bin/bash`.

## Implication for the terminal feature

The PTY backend's configured shell command should default to `bash` once `yocto-superbird` ships it (better interactive experience: real history, completion, line editing), with `ash`/`sh` as the fallback if running on stock firmware or an image that hasn't added the `bash` package yet. Keep this as a config value, not a hardcoded path, consistent with the same config-not-hardcode approach already used for the shell-vs-ssh-target choice.
