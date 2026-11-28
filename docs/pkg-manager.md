# Package manager: options evaluated, decision pending

The image ships a fixed tool set and gets new software only through a new image. This page records every route to an updatable package manager that was evaluated on 2026-11-27, what was tested on a live device, and why none was adopted yet. The decision is deliberately left open.

## Constraints

The root filesystem is ext4 and read-only, with A/B slots that an OTA replaces. A persistent writable `/var` partition exists and an OTA does not touch it. The CPU is aarch64 (Cortex-A53) with 448 MB of RAM, the image is glibc 2.43 with systemd 259 and usrmerge, and the kernel is mainline 7.0.2. The owner wants tools such as vim, htop, curl, tmux and Python to be installable and updatable, ideally with an Android-like model where the system image carries the base and updates are applied on top.

## Option 1: opkg with a writable /usr overlay and a self-hosted signed feed (designed, built, dropped)

Idea: ship opkg in the image with its database in `/usr/lib/opkg`, mount an overlayfs on `/usr` whose upper layer lives on `/var`, and serve a signed ipk feed from the build host. Base packages sit in the read-only lower layer and upgrades land in the upper layer, which is the Android system-app model.

What was built (host side only, never flashed): the overlay mount unit with health and trash units, a `thing-revived-pkg` wrapper, a hold list generator, signed feed wiring, a persisted Device settings toggle, checker invariants and about 250 host tests. All of it passed its host tests. It is preserved in a git bundle outside the repository, and the repository history was reset.

Why it was dropped: it needs a self-hosted build and feed (a signing key, a feed server, a rebuild for every update), and a writable `/usr` weakens the read-only root guarantee for root-level attackers. After an A/B OTA the upper layer can shadow newer base files, so the plan dropped the whole layer on every image change and replayed a user package list, which means installed packages are lost on every slot flip. An independent review found six required changes before the first build. No public repository exists for a custom Yocto distribution, so the feed could never be public.

## Option 2: micromamba with conda-forge (tested on the device)

Idea: install the static micromamba binary under `/var` and create environments from the public conda-forge linux-aarch64 channel. conda-forge is a general binary repository and not only for Python.

Live spike result: installed vim 9.2, htop 3.5.3, tmux, curl 8.23, jq, ncdu, strace 7.3, lsof, rsync 3.5.2 and nano 9.3 in about 80 seconds with 92 MB peak RAM for micromamba. The environment took 456 MB, plus a 251 MB package cache, because vim pulled in Python 3.14. The device reached conda-forge directly over HTTPS. tcpdump and iproute2 are not available for aarch64. Versions are the newest of all options. Integrity relies on HTTPS and repodata hashes without a signature check seen.

Why it is parked: the footprint is large for the space available, and the owner does not need the newest versions.

## Option 3: Entware (tested on the device)

Idea: Entware is a public opkg feed (bin.entware.net, target aarch64-k3.10) built for routers and small devices. Every binary has its loader at `/opt/lib/ld-linux-aarch64.so.1` and a RUNPATH of `/opt/lib`, and Entware ships its own glibc 2.27, so it coexists with the system glibc but requires `/opt` at exactly that path. It never touches `/usr`.

Live spike result: opkg ran from a prefix on `/var` without root, and 18 packages (vim, nano, htop, tmux, curl, jq, ncdu, rsync, strace, tcpdump, lsof, python3, pip, bash, procps, ip-full, gnupg) installed in 44 seconds with 4 MB peak RAM. Total size 87 MB. The binaries ran on kernel 7.0 through Entware's own loader. tmux complained about the locale only. Not tested: running with a real `/opt` mount, the `ip` binary from `ip-full`, pip, gnupg, updates, and an OTA slot flip.

Verified from the feed: 3006 packages, index updated 2026-11-22, one effective maintainer team of about three people, userland built on glibc 2.27 from 2018, and the feed is unsigned with HTTP as the default transport. HTTPS is available on request. Old package versions move to an archive directory.

Integration needs `/opt` to be a writable path. The planned approach was a bind mount of `/var/lib/entware` onto `/opt` with a bootstrap command run on demand, since the feed is unsigned and nothing should fetch and run its code unattended. A variant would seed `/opt` from the image with an overlay, which brings back the OTA reconciliation problem on a smaller scale. Entware tools would be placed in `PATH` after or before the system tools depending on whether the owner wants them to override the base copies.

Why it is parked: no decision yet. Open points are the trust model of an unsigned feed, whether `/opt` should be a bind mount or a seeded overlay, and where `/opt` should live.

## Other candidates, dismissed

A Debian or Alpine chroot works and is well signed but awkward to use. Nix has the broadest coverage but is heavy and needs `/nix`. OpenWrt feeds are musl and incompatible. Homebrew, Guix, pkgsrc, Flatpak, Snap and AppImage do not fit. meta-debian is stale. No public Yocto feed exists for a custom distribution. pip with PyPI and static binaries from GitHub releases complement any option for Python packages and small tools. The full comparison is in `docs/research/public-package-sources.md` and `docs/research/entware-verification.md`.

## What the image does instead for now

A baked tool set (see `docs/development.md`) updated by OTA only. The two public sources above stay available for ad hoc use, and neither is part of the image.

## Open decision

Pick one of: nothing (baked tools only), Entware on a `/opt` bind mount (smallest and closest to the opkg workflow), micromamba (newest versions, largest footprint), or revisit a self-hosted signed feed. Any choice that needs a path in the root image, such as `/opt`, should be made before the next partition change, since a geometry change forces a full flash that wipes `/var`.
