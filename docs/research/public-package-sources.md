# Public package sources for the Car Thing (read-only rootfs, aarch64, glibc 2.43)

Research date 2026-11-27. Research only, nothing installed or run on the device. Search backend note: the default search provider hit its usage limit, so results came from fallback providers; all cited pages were seen as search snippets or extracts, not full manual reads.

## Summary

No public ipk, rpm or deb feed matches a custom Yocto distro, so the "just point opkg at a public Yocto feed" route does not exist. The best fit is conda-forge through micromamba (or pixi) installed into a prefix on /var: it has native linux-aarch64 builds of every tool on the owner's list that is a normal CLI package, needs only glibc 2.17 or newer (the image has 2.43), needs no root writes and no /nix style fixed path, has a real update command, and survives OTA slot flips because everything lives on /var. Entware (a public glibc opkg feed that ships its own libc under /opt) is a close second and the only candidate that keeps the opkg workflow, but it is a community feed aimed at routers with an old glibc 2.27 and a thin signing story. Nix works on this kernel (user namespaces are enabled) and has the biggest binary cache, but costs far more disk and complexity. Static-binary tools and pip are good complements, not replacements.

## Device facts checked in the repo

- Image glibc: oe-core at /home/andremmfaria/workspace/yocto-superbird-build-2/sources/openembedded-core has meta/recipes-core/glibc/glibc_2.43.bb, and meta/conf/layer.conf sets LAYERSERIES_CORENAMES = "wrynose". So the image glibc is 2.43 (verified from the recipe file; the actually deployed image was not inspected).
- Kernel features: the fragment meta-superbird/recipes-kernel/linux/linux-superbird/superbird.cfg sets CONFIG_FUSE_FS=y, CONFIG_OVERLAY_FS=y, CONFIG_SQUASHFS=y with LZ4 and ZSTD, but does not mention USER_NS. The recipe starts from the arm64 defconfig then merges fragments. The arm64 defconfig in the kernel source tree under /home/andremmfaria/workspace/yocto-superbird-build/build/tmp/work-shared/superbird/kernel-source/arch/arm64/configs/defconfig line 28 sets CONFIG_USER_NS=y. The generated .config in that tree (dated Nov 22, from the older build directory, not build-2) shows CONFIG_NAMESPACES, UTS_NS, IPC_NS, USER_NS, PID_NS, NET_NS, SECCOMP, SECCOMP_FILTER, FUSE_FS, OVERLAY_FS and SQUASHFS (zlib, lz4, zstd) all =y. Caveat: confirm on the device with `zcat /proc/config.gz` or `unshare --user --pid echo YES`, and note sysctl user.max_user_namespaces could still be 0.
- Root is read-only, /var is writable and not touched by OTA, so any approach that installs into a prefix under /var and only needs a PATH entry survives slot flips. Anything that needs a new top-level directory (/nix, /opt, /gnu, /home/linuxbrew) needs that mount point to exist in the image or a bind mount from a systemd unit.

## Candidates

### 1. Yocto public feeds (ipk, rpm, deb)

- Viable: no.
- Evidence: Michael Opdenacker's ELC 2024 talk states "The Yocto project doesn't ship binary feeds yet" and that package feeds are produced locally under tmp/deploy and need `bitbake package-index` (https://rootcommit.com/pub/conferences/2024/elce/yocto-binary-distro/yocto-binary-distro.pdf). The Yocto autobuilder publishes build artifacts, not a stable feed for arbitrary distros, and a custom distro changes ABI-affecting config (DISTRO_FEATURES, glibc options, versions) so a feed from another distro cannot be trusted to match even if one existed. Vendor feeds (Toradex, Variscite, Phytec, balena) were not individually fetched; they are per-vendor distro feeds for their own images and cannot be assumed compatible. Marked unverified at vendor level.
- Yoe Distro (Khem Raj) was announced as an attempt at a binary Yocto distro but "doesn't ship binary images and packages yet" per the same talk (2024); current status unverified.
- meta-debian (https://github.com/meta-debian/meta-debian): builds Debian source packages with Yocto, does not consume a Debian binary feed. Default branch is warrior (Debian 10, Yocto 2.7), layer index says last commit 5 years 3 months ago on warrior (https://layers.openembedded.org/layerindex/branch/warrior/layer/meta-debian/). Dead for wrynose. Dismissed.
- PACKAGE_CLASSES = package_deb only changes the format Yocto itself produces (dpkg and apt tooling on target); it implies no public repo, you would still self-host.

### 2. Nix (cache.nixos.org)

- Viable: maybe (works technically, heavy).
- Why: needs /nix. Workarounds: nix-user-chroot (user namespaces plus bind mount, https://github.com/nix-community/nix-user-chroot), nix-portable with bwrap or its built-in nix runtime or proot fallback (https://github.com/davhau/nix-portable, aarch64-linux supported, tested on aarch64 Debian and NixOS), or Nix native `--store` relocation that uses user namespaces (https://wiki.nixos.org/wiki/Nix_Installation_Guide). Relocating the store path itself loses the official cache because store paths are hashed with the /nix prefix (https://discourse.nixos.org/t/nix-portable-nix-static-permissionless-install-free-pre-configured/11719), so a mount namespace is required. Simplest on this device: create an empty /nix mount point in the image, bind mount /var/nix on it with a systemd unit, then run the normal single-user install (https://nixos.org/manual/nix/stable/installation/single-user.html). That needs one image change.
- Kernel: user namespaces are on (see above), so nix-user-chroot and bwrap paths are possible, proot not needed.
- Glibc independence: Nix packages carry their own glibc (nixpkgs 26.05 ships glibc 2.42, https://nixos.org/manual/nixpkgs/stable/release-notes), so the host glibc does not matter. Aarch64 glibc 2.42 on a 7.0 kernel is fine in principle.
- Footprint: Nix closure per tool is large because each pulls its own glibc and deps (about 30 to 50 MB for the base set before tools; estimate, unverified). Store grows with every update until `nix-collect-garbage`. RAM: nix evaluation of nixpkgs can use several hundred MB (commonly reported, not measured here); on 448 MB RAM with zram this is risky. Unverified, must be tested. Using `nix profile install nixpkgs#htop` evaluates nixpkgs, which is the worst case; a pre-built profile fetched by store path avoids evaluation.
- Update story: `nix profile upgrade` or channel update, from the binary cache; survives OTA since /var holds the store.
- Tools: vim, nano, htop, tmux, curl, jq, ncdu, rsync, strace, tcpdump, lsof, python3 all exist in nixpkgs (not individually fetched; high prior, unverified per package).
- Trust: cache.nixos.org signs narinfo with the cache.nixos.org-1 key, verified by Nix client (signing mechanism documented at https://wiki.nixos.org/wiki/Signing_store_paths). Maintained by the NixOS Foundation; nixpkgs 26.05 released 2026-05-30. Nix 2.34.9 manual current.
- Verdict: best coverage and strongest signing story, worst footprint and RAM risk. Could replace the self-hosted feed but only with a mount point change and a disk budget check.

### 3. conda-forge via micromamba or pixi

- Viable: yes. Recommended.
- Why: conda-forge ships linux-aarch64 builds with a `__glibc >=2.17` minimum virtual package requirement (htop 3.5.2 linux-64 build lists `__glibc >=2.17,<3.0.a0`: https://prefix.dev/channels/conda-forge/packages/htop; micromamba package depends on `__glibc >=2.17,<3.0.a0`: https://anaconda.org/conda-forge/micromamba/files/modal/info/698c8e8204abee7b895a834b). The image has 2.43, so it is far above the floor. Binaries link libs from the prefix (libgcc, ncurses, libnl) and use the system glibc; no chroot, no namespaces, no fixed path. Prefix is relocatable (packages have has_prefix and are rewritten at install).
- Evidence of aarch64 tools: htop 3.5.3 linux-aarch64 (206 KB, 2026-08-17: https://anaconda.org/channels/conda-forge/packages/htop/files), tmux 3.8 linux-aarch64 (747 KB, 2026-11-07: https://prefix.dev/channels/conda-forge/packages/tmux), vim 9.2.1252 linux-aarch64 (11.35 MB, 24 days before fetch: https://anaconda.org/conda-forge/vim/files). curl, jq, ncdu, rsync, nano, python are very likely present (not individually fetched, unverified). strace, tcpdump and lsof availability on conda-forge: I did not confirm; these are kernel-adjacent and some may be missing or Linux-only. Treat as unverified.
- Installer: micromamba is a fully static executable (https://mamba.readthedocs.io/en/stable/installation/micromamba-installation.html) available for linux-aarch64 from micro.mamba.pm, with `micromamba self-update`. Current version 2.10.0 (updated 2026-10-26). Mamba 2.x is actively developed. pixi 0.84.2 released 2026-11-10 with `pixi global install` and `pixi self-update` (https://github.com/prefix-dev/pixi/releases). Pixi release notes mention a memory regression from 0.82.1 fixed in 0.83.0 and 0.84.1, a reminder to pin versions and test RAM.
- Footprint: tool packages are small (htop 0.2 MB, tmux 0.75 MB) but shared deps (ncurses, libgcc, libstdcxx, openssl, python) add up; python adds roughly 100 MB or more (estimate, unverified). Package cache can be cleaned after install. Plan about 300 to 600 MB for a modest toolset (estimate).
- RAM: micromamba uses libsolv; solving a few small packages against conda-forge linux-aarch64 repodata is the risk, because repodata for linux-aarch64 is large (tens of MB JSON; 35,273 packages synced across platforms per prefix.dev mirror page). A 2.0 regression about libsolv overhead exists (https://github.com/mamba-org/mamba/issues/3393). Could not find a measured number for 448 MB; must be tested on the device. Mitigations: solve on another machine and install from an explicit lock file or explicit URL list (`micromamba create -f spec.txt` with explicit specs skips solving), enable zram swap (already in the kernel fragment: CONFIG_ZRAM with LZ4).
- Update story: `micromamba update -n tools --all`, or re-solve remotely and ship an explicit lock. Survives OTA (prefix under /var, PATH via profile.d in the image).
- Trust: conda-forge packages are built by CI from feedstocks, served via HTTPS from anaconda.org and prefix.dev mirror. prefix.dev lists "Not cryptographically verified" provenance for these builds. Conda has optional package signature verification, but the default conda-forge channel does not enforce signed repodata for micromamba (not verified here). Trust is HTTPS plus the sha256 in repodata. Medium trust.
- Fit with project: could replace the opkg overlay for owner tools. Overlay on /usr not needed. Caveats: second copy of libs, ncurses terminfo path inside the prefix, no systemd units.

### 4. Debian or Ubuntu arm64 via chroot, mmdebstrap, debootstrap

- Viable: maybe.
- Facts: Debian 13 trixie (glibc 2.41, https://www.debian.org/News/2025/20250809; 13.8 released 2026-11-21, https://www.debian.org/releases/trixie/) supports arm64, 69,830 packages. Sizes measured by a third party for trixie amd64: debootstrap minbase 214 MB (183 MB after apt-get clean), mmdebstrap minbase 128 MB, default 310 MB (https://hvr.biz/articles/topic.php?slug=debootstrap-minimal-debian-chroot-bootable-image-traps, 2026-11-04). arm64 numbers should be similar (not measured).
- Mechanism: run as root with a plain chroot, which works on any kernel (device has bind mounts and proc) and does not require user namespaces (mmdebstrap sudo/root mode; unshare mode uses user namespaces and /etc/subuid). The chroot runs Debian's own glibc 2.41 inside, so no host glibc dependency. systemd and device access need bind mounts of /dev, /proc, /sys; the user must enter the chroot to use tools (they are not on the host PATH unless wrapped).
- Bootstrap needs apt, gpgv and dpkg tools on the device or must be run on another machine and the tarball shipped; mmdebstrap can build to a tarball or squashfs (squashfs support is in the kernel).
- Update story: `apt update && apt upgrade` inside the chroot against deb.debian.org with archive signing (Debian archive keys, apt verifies Release.gpg). This is the strongest trust story of the generic options. Survives OTA.
- Tools: all tools on the owner's list exist in Debian (vim, nano, htop, tmux, curl, jq, ncdu, rsync, strace, tcpdump, lsof, python3), not individually fetched but certain from Debian's size.
- Cost: 150 to 400 MB plus apt lists; RAM modest (apt). Inconvenience: separate filesystem view, hostname and /etc, tools run against chroot paths so `htop` sees the host processes only if /proc is bind mounted (it is).
- Verdict: very robust and well-signed, but a chroot is a different user experience (no host PATH integration) and heavy. Good for a "debug shell" rather than seamless tools.
- Alpine apk: apk-tools-static is 2 MB, Alpine 3.24.4 released 2026-11-05 (https://alpinelinux.org/posts/alpine-3.21.10-3.22.8-3.23.8-3.24.4-released/, https://dl-cdn.alpinelinux.org/alpine/latest-stable/main/aarch64/). Uses musl, so it works only inside a chroot (https://wiki.alpinelinux.org/w/index.php?title=Installing_Alpine_Linux_in_a_chroot), a minimal base is under about 10 MB in practice (not verified here), signed with Alpine keys. Smaller than Debian and fine for strace, tcpdump, lsof, htop, tmux, vim, jq, rsync, python3. Same chroot UX caveat. Better footprint than Debian, musl differences rarely matter for CLI tools. Maybe.

### 5. Homebrew on Linux, Guix, pkgsrc, Gentoo Prefix

- Homebrew: aarch64 Linux is Tier 1 since Nov 2025 (https://github.com/Homebrew/brew/pull/20974) but only with Ubuntu on a supported window and system glibc >= 2.39 and default prefix /home/linuxbrew/.linuxbrew (https://docs.brew.sh/Support-Tiers). The image glibc 2.43 satisfies the glibc rule but the distro rule does not (Yocto is not Ubuntu), and a custom prefix must be no longer than 26 bytes ("/var/brew" fits) but is Tier 3 unless in a "compatible custom prefix". Homebrew also expects a working system C compiler and standard dev tools even for bottles (https://docs.brew.sh/Homebrew-on-Linux), plus Ruby (portable Ruby for arm64 exists) and git. The device has no compiler. Verdict: no (heavy, wrong support tier, needs git and gcc).
- Guix: needs /gnu/store and /var/guix, a guix-daemon, build users; substitutes for aarch64 exist but coverage was shown as incomplete in a 2022 report (40.5 percent of queued builds on aarch64 at the time; https://lists.libreplanet.org/archive/html/guix-devel/2022-11/msg00026.html), so state is stale. Needs root writes at /gnu. No for this device (same /gnu mount point problem as Nix with a smaller cache and daemon cost).
- pkgsrc and Gentoo Prefix: source-based by default; Gentoo offers an optional binary repository since Dec 2023 for selected architectures (https://wiki.archlinux.org/title/Arch_compared_to_other_distributions), but Prefix on aarch64 with a binary repo was not confirmed. pkgsrc binary packages for aarch64 Linux not confirmed. Both would compile on a 448 MB device. No.

### 6. Flatpak, Snap, AppImage

- Viable: no for CLI tools.
- Flatpak needs bubblewrap, portals and OSTree runtimes (hundreds of MB for the shared runtime), aimed at GUI apps (https://itplaybook.org/courses/linux-deep-dive/chapter007/universal-packages.html). Snap needs the snapd daemon, systemd integration, AppArmor and the closed Snap Store backend (same source, and https://www.pistack.xyz/posts/2026-11-07-flatpak-vs-snap-vs-appimage-linux-packaging/); needs root and mounts squashfs under /snap, and does not fit a non-Ubuntu custom distro. AppImage mounts a squashfs via FUSE (https://docs.appimage.org/reference/architecture.html; the kernel has FUSE_FS=y but libfuse and fusermount must be present on the image) and has no update mechanism beyond AppImageUpdate; few CLI tools publish aarch64 AppImages.
- Dismiss all three.

### 7. PyPI (pip)

- Viable: yes for Python packages (complement, not a replacement).
- Python 3.14.4 is on the image. Pure-Python packages install anywhere. Binary wheels: manylinux aarch64 wheels exist for psutil, including cp314 (psutil publishes aarch64 Linux wheels, such as manylinux2014 aarch64 for 3.13t/3.14t; https://psutil.io/blog/2025/wheels-for-free-threaded-python/ and https://psutil.io/platform/). psutil 8.0.0 adds ppc64le and s390x wheels; musl wheels dropped since 7.2.0, not relevant here (glibc). glances is pure Python on top of psutil, so `pip install --user glances` should work (not verified in a fetch). A manylinux_2_17/2_28 wheel needs glibc >= 2.17 or 2.28; image has 2.43.
- Install location: a venv or `pip install --target` under /var, or `pip --user` with HOME on /var. Since root is read-only, the system python site-packages is not writable; use `python3 -m venv /var/venv` (needs ensurepip and venv in the image, unverified) or `pipx`/`uv`.
- Update: `pip install --upgrade`; survives OTA, but an interpreter minor-version change (image python 3.14 to 3.15) would break compiled wheels in the venv, so rebuild after OTA that changes Python.
- Trust: PyPI TLS plus hash pinning optional; Sigstore attestations for some projects (not verified here). Supply-chain risk is real (typosquatting); pin hashes.
- Tools: pip also carries some "binary" CLI tools as wheels (ruff, uv, httpie in Python), but not vim, htop, tmux, strace.

### 8. Static binaries from GitHub releases or other channels

- Viable: yes (complement).
- Mechanisms: mise (curl installer, ~/.local/bin; its glibc builds need glibc >= 2.18 and checksums come from GitHub; supports `github:user/repo` backend with checksum and GitHub attestation verification; https://mise.jdx.dev/installing-mise.html, https://mise.jdx.dev/dev-tools/backends/github.html), aqua (checksum verification and registry, https://aquaproj.github.io/docs/reference/security/checksum), eget (downloads GitHub release assets, `eget update --all`, `--sha256`; https://dev.to/inhere/eget-install-github-releases-without-waiting-for-a-central-index-751). The tools I did not individually check for linux-arm64 builds: ripgrep, fd, bat, jq (jq publishes static linux-arm64), busybox static, ncdu (2.x Zig static builds), btop (static aarch64 builds), micro editor, tmux (static builds exist via third-party projects only).
- Not available as upstream static aarch64 releases in my checks: vim (needs runtime files), tcpdump, strace, lsof, rsync; these have unofficial static builds in some repos but trust and freshness vary. Unverified.
- Update: re-run the tool; survives OTA; checksum gives integrity but not authenticity beyond GitHub account trust.
- Python-free, tiny, zero deps, ideal for Go and Rust tools. Coverage of the owner's list is partial.

### 9. OpenWrt, Entware, other generic ipk feeds

- OpenWrt aarch64_cortex-a53 feeds (https://downloads.openwrt.org/releases/packages-23.05/aarch64_cortex-a53/base, https://dev.gateworks.com/openwrt/24.01/bin/packages/aarch64_cortex-a53/packages): OpenWrt defaults to musl and packages built against musl do not run on glibc systems (https://openwrt-openwrt-35.mintlify.app/toolchain/musl-libc, https://www.musl-libc.org/faq.html). Index listings include musl-fts, a musl compatibility shim package, confirming the libc. A glibc build of OpenWrt exists but the public feeds are musl. Incompatible with the glibc Yocto image: the dynamic loader path differs (/lib/ld-musl-aarch64.so.1), and dependencies like libc are provided by the ipk set itself. They would only run inside a musl chroot. Also the opkg dialect differs and OpenWrt installs into /. No.
- Entware (https://bin.entware.net/aarch64-k3.10/): glibc 2.27 ABI per the installer script table for aarch64-k3.10 (https://raw.githubusercontent.com/Entware/installer.sh/master/generic.sh), ships its own ld.so and libc in an /opt prefix (installer directory lists ld-2.27.so, libc-2.27.so, libgcc_s.so.1, and an opkg binary at https://bin.entware.net/aarch64-k3.10/installer/). Feed rebuilt as recently as 2026-11-22. Entware uses its own userspace in /opt so it does not collide with the image libc; it needs /opt writable (create an empty /opt mount point in the image and bind mount /var/opt, or use `/opt` symlink from the image to /var/opt). This is the only public opkg feed that is architecture and libc compatible by construction, and the same package manager the project already uses. Package list includes many CLI tools (index shows packages such as acl, adb, apache; per-tool presence for htop, tmux, vim, jq, ncdu, rsync, strace, tcpdump, lsof not fetched, but Entware is known for these; unverified). Python3 present in Entware (unverified). Trust: opkg signature checking in Entware is not documented in the sources I saw; assume HTTPS only and community-run; unverified. Maintained for router firmware (Keenetic, Asus), kernel baseline 3.10, glibc 2.27 is old (2018) so security fixes in glibc itself are not tracked as fast, but it is private to /opt. Verdict: maybe, strongest "keep opkg" fit; the signing and older glibc are the caveats.

## Ranked recommendation

1. micromamba (or pixi) with conda-forge into /var/conda: seamless on PATH, current versions, glibc floor satisfied, no root-fs changes. Test RAM of the solve on the device and use explicit lock files if it fails.
2. Entware opkg feed under /opt (bind from /var/opt): keeps the opkg workflow; older glibc 2.27 userland and unclear signing.
3. Static binaries via mise or eget for Go and Rust tools, plus pip in a venv for Python packages: complements, not replacements.
4. Nix with a /nix mount: broadest and best-signed, but heavy and RAM risky.
5. Debian or Alpine chroot: strongest trust and full coverage, worst ergonomics; useful as a debug shell.
Dismissed: Yocto public feeds (do not exist), meta-debian (source only, dead), OpenWrt feeds (musl), Homebrew, Guix, pkgsrc, Gentoo Prefix, Flatpak, Snap, AppImage.

## Interaction with the existing opkg overlay and self-hosted feed

- Replace: conda-forge via micromamba could replace the overlay for owner-installed tools entirely; Nix could too; Entware replaces the feed server but not the overlay need (it still wants /opt).
- Complement: pip (Python packages), static binaries (Go and Rust single binaries), and a Debian or Alpine chroot (rare tools such as tcpdump or strace if conda-forge lacks them).
- Keep opkg for: anything the image itself must own (system services, kernel modules, the kiosk app), because those must match the image ABI and be rolled back with the A/B slot. Public feeds are for owner tools only.
- Caveats: a public feed gives updates without a build server but adds supply-chain exposure and network dependence on a device that is a kiosk; pin versions and consider an `exclude-newer` style delay (pixi supports `exclude-newer`).

## Unverified or risky

- RAM use of micromamba or pixi solving on 448 MB was not measured and no published number found; test first.
- Presence of strace, tcpdump, lsof, ncdu, rsync, jq, nano on conda-forge linux-aarch64 was not fetched individually.
- Whether the deployed image really has USER_NS enabled (checked a stale build tree .config, not the build-2 tree or the device) and whether sysctl allows it.
- Entware signing, per-tool presence and its /opt requirements on a Yocto image; Entware's glibc 2.27 userland is old.
- Nix disk and RAM numbers are estimates.
- Vendor feeds (Toradex, Variscite, Phytec, balena) not fetched; Yoe Distro current status not verified.
- Static aarch64 builds for tcpdump, strace, lsof, rsync not confirmed from upstream.

## Sources

- https://rootcommit.com/pub/conferences/2024/elce/yocto-binary-distro/yocto-binary-distro.pdf : Yocto ships no binary feeds
- https://www.siliconbladeconsultants.com/training/yocto/package-feeds-and-runtime-package-management/ : feed mechanics
- https://github.com/meta-debian/meta-debian and https://layers.openembedded.org/layerindex/branch/warrior/layer/meta-debian/ : meta-debian status
- https://anaconda.org/conda-forge/micromamba/ and https://prefix.dev/channels/conda-forge/packages/htop and /tmux : conda-forge aarch64 builds and glibc floor
- https://anaconda.org/conda-forge/vim/files : vim aarch64
- https://github.com/prefix-dev/pixi/releases : pixi status and memory regression note
- https://mamba.readthedocs.io/en/stable/installation/micromamba-installation.html and https://github.com/mamba-org/mamba/issues/3393 : micromamba install and libsolv overhead
- https://wiki.nixos.org/wiki/Nix_Installation_Guide, https://github.com/nix-community/nix-user-chroot, https://github.com/davhau/nix-portable, https://discourse.nixos.org/t/nix-portable-nix-static-permissionless-install-free-pre-configured/11719, https://nixos.org/manual/nix/stable/installation/single-user.html, https://nixos.org/manual/nixpkgs/stable/release-notes : Nix
- https://docs.brew.sh/Support-Tiers, https://docs.brew.sh/Homebrew-on-Linux, https://github.com/Homebrew/brew/pull/20974 : Homebrew
- https://www.debian.org/releases/trixie/, https://www.debian.org/News/2025/20250809, https://hvr.biz/articles/topic.php?slug=debootstrap-minimal-debian-chroot-bootable-image-traps, https://manpages.debian.org/trixie/mmdebstrap/mmdebstrap.1.en.html : Debian
- https://dl-cdn.alpinelinux.org/alpine/latest-stable/main/aarch64/ and https://alpinelinux.org/posts/alpine-3.21.10-3.22.8-3.23.8-3.24.4-released/ : Alpine
- https://openwrt-openwrt-35.mintlify.app/toolchain/musl-libc, https://downloads.openwrt.org/releases/packages-23.05/aarch64_cortex-a53/base, https://dev.gateworks.com/openwrt/24.01/bin/packages/aarch64_cortex-a53/packages : OpenWrt musl
- https://bin.entware.net/aarch64-k3.10/ and https://raw.githubusercontent.com/Entware/installer.sh/master/generic.sh : Entware
- https://psutil.io/platform/ and https://psutil.io/blog/2025/wheels-for-free-threaded-python/ : PyPI wheels
- https://mise.jdx.dev/installing-mise.html, https://mise.jdx.dev/dev-tools/backends/github.html, https://aquaproj.github.io/docs/reference/security/checksum, https://dev.to/inhere/eget-install-github-releases-without-waiting-for-a-central-index-751 : static binary tooling
- https://itplaybook.org/courses/linux-deep-dive/chapter007/universal-packages.html, https://docs.appimage.org/reference/architecture.html, https://www.pistack.xyz/posts/2026-11-07-flatpak-vs-snap-vs-appimage-linux-packaging/ : Flatpak, Snap, AppImage
- https://lists.libreplanet.org/archive/html/guix-devel/2022-11/msg00026.html and https://wiki.archlinux.org/title/Guix : Guix
