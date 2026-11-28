# Entware verification and comparison with micromamba and conda-forge

Research date 2026-11-27. Research only: nothing installed on the device, no device or network service of ours touched. Entware feed files and the wiki git repo were fetched read-only to a scratch dir, one Entware ipk (htop) was downloaded and unpacked on the host only to read its ELF headers. conda-forge linux-aarch64 repodata.json.zst (29 MB, 231 MB uncompressed, 384,593 files, 6,845 package names) was fetched and parsed on 2026-11-27.

## Summary

Entware is better than the earlier report assumed on mechanics and worse on trust and freshness. Verified: its binaries use an absolute interpreter /opt/lib/ld-linux-aarch64.so.1 with RUNPATH /opt/lib, so it ships and uses its own glibc 2.27 and coexists with the system glibc 2.43; the feed is rebuilt weekly, is alive (commits on 2026-11-22), carries every tool the owner listed with current-ish versions, and costs about 29 MB download and 77 MB on disk for the whole toolset including python3 and pip. Not good: the feed is not signed (no Packages.sig, opkg.conf has no check_signature), the default feed URL is plain HTTP (HTTPS needs wget-ssl, a documented opt-in), vim default package is a tiny build, glibc 2.27 is from 2018, maintenance is effectively three people with one doing most work, and nothing in the project targets systemd or a Yocto root. conda-forge via micromamba is stronger on freshness, tool depth and ecosystem, but lacks tcpdump (verified absent) and is 2 to 5 times larger. Recommendation at the end: micromamba primary, Entware as a justified second, or Entware first if the owner values the opkg UX and small footprint over signing and freshness.

## Verified fact table, Entware

Status key: V verified from a primary source fetched today, U unverified or inferred.

| # | Fact | Status | Source |
|---|------|--------|--------|
| 1 | Targets: aarch64-k3.10 (supported), mipssf-k3.4, mipselsf-k3.4, x64-k3.2, armv7sf-k3.2 supported; armv5sf-k3.2, armv7sf-k2.6, x86-k2.6 withdrawn. Only one aarch64 target, no static aarch64 variant, no musl target | V | https://github.com/Entware/Entware/wiki and https://bin.entware.net/ (directory listing) |
| 2 | aarch64-k3.10 libc is glibc 2.27 (package libc 2.27-12, toolchain GCC 8.4.0 per Readme), minimum kernel 3.10 per target name and wiki table; running on kernel 7.0 not tested | V (libc and target), U (kernel 7.0 runtime) | https://bin.entware.net/aarch64-k3.10/installer/generic.sh, https://bin.entware.net/Readme.txt, Packages index |
| 3 | Self-contained libc: htop ELF has INTERP /opt/lib/ld-linux-aarch64.so.1, RUNPATH /opt/lib, NEEDED libc.so.6 libm libdl libgcc_s, max symbol GLIBC_2.17. Installer fetches ld-2.27.so, libc-2.27.so, libgcc_s.so.1, libpthread-2.27.so into /opt/lib. opkg itself is a static aarch64 executable (856 KB) | V (ELF headers read on host) | https://bin.entware.net/aarch64-k3.10/htop_3.5.3-1_aarch64-3.10.ipk, https://bin.entware.net/aarch64-k3.10/installer/ |
| 4 | Consequence: /opt must exist with that exact path for any Entware binary to start; the system loader is never used by Entware binaries and system binaries are not affected | V (ELF) / U (live test) | same |
| 5 | Installer (generic.sh in aarch64-k3.10/installer, TYPE=generic): needs root, mkdir /opt (warns if present), creates /opt/{bin,etc,lib/opkg,tmp,var/lock}, wget opkg and opkg.conf over http://bin.entware.net, runs opkg update and opkg install entware-opt, chmod 777 /opt/tmp, symlinks /opt/etc/{passwd,group,shells,shadow,gshadow} to /etc if they exist, symlinks /etc/localtime. It does not write to /etc, /usr or /lib. Note the file in the GitHub repo Entware/installer.sh master is the x86 template (ARCH=x86-k2.6), use the file served from the feed | V | https://bin.entware.net/aarch64-k3.10/installer/generic.sh, https://raw.githubusercontent.com/Entware/installer.sh/master/generic.sh |
| 6 | Only writable / requirement is creating the /opt path. The wiki officially recommends bind mount or symlink of /opt to persistent storage (Synology: mount -o bind, or ln -s as fallback; reMarkable (Yocto, read-only style root): ln -s ~/.entware /opt with manual install). No chroot is required | V | https://github.com/Entware/Entware/wiki/Install-on-Synology-NAS, https://github.com/Entware/Entware/wiki/Install-on-Remarkable (wiki git clone) |
| 7 | opkg.conf: src/gz entware http://bin.entware.net/aarch64-k3.10, dest root /, lists in /opt/var/opkg-lists, tmp /opt/tmp, arch all 100, arch aarch64-3.10 160. No check_signature line | V | https://bin.entware.net/aarch64-k3.10/installer/opkg.conf |
| 8 | Package count in aarch64-k3.10 Packages index: 3006 (wiki says over 2,500); 25 are arch all | V | https://bin.entware.net/aarch64-k3.10/Packages.gz |
| 9 | Feed trust: no Packages.sig, Packages.asc, Packages.gz.sig or Packages.usign (all 404). opkg binary contains usign verify code and a check_signature option but the shipped config does not enable it and no key or signature is published. Integrity is only the SHA256sum field inside the Packages index, delivered over the same channel | V (404s, opkg.conf, strings of opkg binary) | https://bin.entware.net/aarch64-k3.10/ |
| 10 | Transport: both http:// and https:// on bin.entware.net return 200 (nginx 1.22.1). Default opkg.conf uses http. HTTPS is a documented opt-in: install wget-ssl (1.25.0-5) and ca-certificates, edit opkg.conf; opkg shells out to wget, so PATH order matters | V | https://github.com/Entware/Entware/wiki/Using-HTTPS-with-opkg (wiki git clone), HTTP header checks |
| 11 | Update flow: opkg update && opkg upgrade, standard. Feed rebuilt: directory timestamps 2026-11-19 to 11-22 for all supported archs, Packages dated 2026-11-22 18:00; unsupported archs frozen 2024. Old versions are moved to archive/ (dated back to 2018), so the main dir holds only latest versions. Pinning: not documented in the wiki, opkg has a hold flag (opkg flag hold) by upstream opkg design | V (rebuild dates, archive), U (hold in Entware build) | https://bin.entware.net/aarch64-k3.10/archive/ |
| 12 | GitHub Entware/Entware: pushed 2026-11-22, last commits 2026-11-22 (nftables, dropbear, mbedtls), open issues count 1 (a package request, 0 open aarch64 issues by search), 2,812 stars, 185 forks, not archived, no GitHub releases, 75 issues created since 2026-01-01. Contributors: The-BB 2031 commits, zyxmon 1164, ryzhovau 459, then single digits | V | https://github.com/Entware/Entware, https://api.github.com/repos/Entware/Entware |
| 13 | Funding: .github/FUNDING.yml has a single Russian tip link (cloudtips). Mirror list exists on entware.net (page mentions Mirrors, not enumerated) | V (FUNDING), U (mirror details) | https://github.com/Entware/Entware (.github/FUNDING.yml) |
| 14 | Licence: the build tree is OpenWrt derived, GPL-2.0-only (COPYING); each package carries its own upstream licence in the Packages License field (vim has License: Vim). Redistribution of binaries is the project's own business, our use is download only | V | https://github.com/Entware/Entware (COPYING), Packages |
| 15 | Design target is routers and NAS (kernel 3.x era), stated in the wiki home. Wiki pages exist for Android and reMarkable (Yocto derived, read-only style), none for systemd generic Linux | V | https://github.com/Entware/Entware/wiki |
| 16 | opkg RAM need: Packages is 1.69 MB, 3006 entries, opkg is static C; actual RSS on 448 MB not measured | U | none found |
| 17 | glibc 2.27 patch level: package revision -12 suggests patches but CVE coverage vs upstream 2.27 era fixes not checked | U | none |

## Package coverage (Entware aarch64-k3.10, from the Packages index)

Size is the ipk download size, installed is Installed-Size from the index, dates are the ipk mtime on the feed. All 2026-11-19 to 11-22 except where noted. Source: https://bin.entware.net/aarch64-k3.10/Packages.gz and the directory listing.

| Package | Version | Download | Installed | Notes |
|---|---|---|---|---|
| vim (default) | 9.2.1014-1 | 782 KB | 1.5 MB | Tiny build, minimal features |
| vim-full + vim-runtime | 9.2.1014-1 | 1.7 MB + 2.7 MB | 3.3 MB + 13.3 MB | Needed for a normal vim |
| nano | 9.3-3a | 165 KB | 338 KB | nano-full also exists |
| htop | 3.5.3-1 | 147 KB | 328 KB | |
| tmux | 3.8-1 | 525 KB | 1.2 MB | |
| curl | 8.23.0-1 | 85 KB | 195 KB | plus libcurl 355 KB |
| jq | 1.8.2-1 | 170 KB | 440 KB | |
| ncdu | 1.22-1 | 40 KB | 92 KB | C 1.x, upstream 2.x is Zig |
| rsync | 3.5.1-1 | 272 KB | 553 KB | |
| strace | 6.18-2 | 467 KB | 1.3 MB | |
| tcpdump | 4.99.7-1 | 456 KB | 1.1 MB | plus libpcap 139 KB |
| lsof | 4.99.7-1 | 91 KB | 184 KB | |
| python3 (meta) | 3.14.7-2 | 1 KB | 10 KB | pulls python3-light 3.5 MB, base, asyncio, ctypes, openssl and others, 81 python3-* packages total |
| python3-pip | 26.2.1-1 | 2.4 MB | 6.6 MB | python3-setuptools 84.0.0 exists; python3-venv and python3-ensurepip not in index |
| bash | 5.3-5 | 594 KB | 1.3 MB | |
| coreutils | 9.11-1 | 1 KB meta | 10 KB | real applets in sub-packages, not itemised |
| procps-ng | 4.0.6-1 | 64 KB | 225 KB | |
| ip-full (iproute2) | 4.4.0-11 | not recorded | not recorded | there is no package named iproute2, ip-full 4.4.0 is old |
| gnupg2 | 2.5.21-2 | 511 KB | 1.1 MB | closure 5.3 MB download |
| busybox | 1.38.0-5 | 336 KB | 584 KB | optional, not installed by generic.sh |
| opkg | 2025.11.05 | 421 KB | 860 KB | |
| libc | 2.27-12 | 1.4 MB | 2.9 MB | |

Footprint computed from Depends closure over the index: base (entware-opt closure, 18 packages) is 3.6 MB download and 8.9 MB installed. The full owner toolset (vim-full, nano, htop, tmux, curl, jq, ncdu, rsync, strace, tcpdump, lsof, python3, python3-pip, wget-ssl, ca-certificates) is 64 packages, 28.6 MB download and 76.8 MB installed including the base. Python alone is 39 to 40 packages, 14 to 16 MB download, 35 to 42 MB installed; vim-full adds 17 MB installed because of vim-runtime.

## Side by side with conda-forge linux-aarch64

conda-forge numbers are from repodata.json of linux-aarch64 fetched 2026-11-27 (https://conda.anaconda.org/conda-forge/linux-aarch64/repodata.json.zst), latest version by version then timestamp. Size is the .conda file only, not dependencies. pip is noarch, so it is not in the linux-aarch64 index, see note.

| Tool | Entware version, size | conda-forge version, size, date | Verdict |
|---|---|---|---|
| vim | 9.2.1014 (full needs runtime, 4.4 MB dl) | 9.2.1252, 11.9 MB, 2026-10-27 | conda newer, much larger |
| nano | 9.3, 165 KB | 9.3, 705 KB, 2026-10-21 | equal |
| htop | 3.5.3, 147 KB | 3.5.3, 211 KB, 2026-08-17 | equal |
| tmux | 3.8, 525 KB | 3.8, 765 KB, 2026-11-07 | equal |
| curl | 8.23.0, 85 KB | 8.23.0, 203 KB, 2026-11-26 | equal |
| jq | 1.8.2, 170 KB | 1.8.2, 350 KB, 2026-08-11 | equal |
| ncdu | 1.22, 40 KB | 2.9.2, 658 KB, 2026-09-05 | conda newer major version |
| rsync | 3.5.1, 272 KB | 3.5.2, 478 KB, 2026-11-25 | conda one patch newer |
| strace | 6.18, 467 KB | 7.3, 610 KB, 2026-10-23 | conda newer |
| tcpdump | 4.99.7, 456 KB | ABSENT (no package named tcpdump or iproute2 in the linux-aarch64 index; anaconda.org API returns none; libpcap 1.10.6 exists) | Entware only |
| lsof | 4.99.7, 91 KB | 4.99.7, 119 KB, 2026-06-16 | equal |
| python | 3.14.7, base 226 KB, light 3.5 MB | 3.15.2, 27.7 MB (also 3.15 free-threaded builds), 2026-11-27 | conda newer minor, larger |
| pip | 26.2.1, 2.4 MB | 26.2.2 noarch (api.anaconda.org/package/conda-forge/pip); linux-aarch64 pip is stale 20.0.2 | equal |
| bash | 5.3-5 | 5.3, 2.2 MB | equal |
| coreutils | 9.11 | 9.12, 3.9 MB | similar |
| procps-ng | 4.0.6 | 4.0.6, 640 KB | equal |
| gnupg | 2.5.21 | 2.5.24, 3.9 MB | conda slightly newer |
| iproute2 | ip-full 4.4.0 | none | Entware only, old |

Rough dependency closure sizes in conda (latest of each dep, ignoring pins, so an upper bound and not a real solve, compressed .conda size): vim 92 MB (pulls perl, python, lua, ruby style deps), curl 37 MB, python 68 MB, rsync 21 MB, nano 8 MB, htop 8 MB, tmux 13 MB, jq 7 MB, ncdu 9 MB, strace 7 MB, lsof 7 MB; the owner toolset together is about 144 MB download in this crude estimate. This is an estimate, a real micromamba solve would be smaller because shared deps and older vim variants are chosen. Entware same set: 28.6 MB.

## Axis comparison

| Axis | Entware | micromamba and conda-forge |
|---|---|---|
| libc coexistence | Own glibc 2.27 under /opt, hard-coded /opt/lib interpreter, V | Uses system glibc (floor mostly __glibc >=2.17 or 2.28, image has 2.43), no own loader |
| Fixed path | /opt must exist (symlink in image to /var/opt, or bind mount unit) | None, prefix anywhere (/var/conda) |
| Root fs change | One path: /opt entry in image | None except PATH in profile.d |
| Signature check | None (V, no sig files, check_signature off) | repodata sha256 over HTTPS, conda signature verification not default (as in earlier report, not re-verified) |
| Transport | HTTP default, HTTPS opt-in | HTTPS |
| Freshness | Within one to three patch levels, vim 240 patches behind, strace 6.18 vs 7.3, ncdu 1.x | Current, updated daily |
| Tools missing | none of the owner's list; iproute2 old | tcpdump absent |
| Footprint | 77 MB installed for everything | 144 MB download estimate, bigger installed |
| Solver RAM | opkg simple, not measured | libsolv, risk on 448 MB, earlier report suggested lock files |
| Maintainers | 3 core people, one tip jar | Large community, anaconda.org and prefix.dev mirrors |
| Update UX | opkg update && opkg upgrade | micromamba update --all or pinned lockfile |
| Old glibc security | Risk: glibc 2.27 inside /opt only affects Entware tools | n/a, uses system glibc 2.43 |

## Known problems and fit

The wiki documents PATH ordering issues: because opkg calls wget, the shell may pick the firmware wget and fail with "wget returned 1, not a HTTP or FTP URI" so Entware paths must come first or wget-ssl be used explicitly (https://github.com/Entware/Entware/wiki/Using-HTTPS-with-opkg). The installer unsets LD_LIBRARY_PATH and LD_PRELOAD, so a system LD_LIBRARY_PATH would break Entware binaries only if they relied on it, which they do not since RUNPATH is set (V from ELF; RUNPATH is overridden by LD_LIBRARY_PATH in glibc, so keep it unset). The wiki warns that the Entware busybox package can interfere with firmware applets; avoid putting /opt/bin before /usr/bin globally, or keep the generic (non-alternative) install without busybox. Generic install links Entware's passwd and group to /etc, which is fine on a read-only root. No wiki page covers systemd or generic glibc Linux, and I found no report of Entware on a modern mainline 7.x kernel; this combination is therefore untested by the project. Entware services use /opt/etc/init.d/rc.unslung which is irrelevant for CLI use.

## Recommendation

Both, with micromamba primary and Entware as a deliberate second source, or Entware first if the owner values opkg UX and size.

- Decisive for micromamba: current versions (vim 9.2.1252, strace 7.3, ncdu 2.9, python 3.15), no /opt path or fixed loader, runs on the system glibc 2.43 so no second libc, HTTPS by default, large maintainer base.
- Decisive for Entware: it has tcpdump (and old iproute2), whole toolset is 29 MB download and 77 MB installed against roughly 144 MB or more for conda, opkg is a static 856 KB binary with no solver so RAM risk is lower than libsolv, same opkg workflow already used by the project, and the owner can pin by hand to a version snapshot in /var/opt.
- Decisive against Entware as sole source: no signed feed, HTTP default, one real maintainer, glibc 2.27 userland frozen on 2018 code, vim default is the tiny build, router-designed with no systemd or modern-kernel guidance.
- If tcpdump is the only gap, a single static tcpdump binary or an Entware install for just that one tool is cheaper than adding a whole second package system; but the owner asked for opkg style UX, so Entware alone is acceptable for a hobby device if HTTPS is enabled via wget-ssl and the bootstrap files are fetched with curl over HTTPS.

## Risks

- No signature: a compromised feed server or MITM (with HTTP default) means root-level code execution on the device as root. Mitigate with HTTPS, and ideally mirroring Packages by checksum.
- Single point: one nginx host bin.entware.net run by about three people; no officially listed mirror verified.
- glibc 2.27 inside /opt means any glibc CVE of the last eight years applies to Entware tools only (still networked tools like curl, rsync, tcpdump).
- Kernel 7.0 with glibc 2.27 binaries unproven on this device; test before standardising (cheap: unpack the installer files and run opkg --version).
- /opt needs an image change (empty directory or symlink to /var/opt) because root is read-only; A/B OTA does not touch /var, so /opt content survives.
- opkg RAM use and disk cleanup (opkg lists in /opt/var/opkg-lists, tmp in /opt/tmp) unmeasured.
- conda-forge side: tcpdump absent, closure size large, libsolv RAM unmeasured (a live spike is running by the orchestrator, not touched here).

## Unverified

- Real RAM use of opkg update and upgrade on 448 MB.
- Whether Entware glibc 2.27 binaries start on kernel 7.0 mainline with this device's seccomp or syscall set (expected yes, not tested).
- opkg hold or flag support in the Entware opkg build.
- Mirror list contents and uptime of entware.net mirrors.
- Whether conda-forge repodata signing is available by default for micromamba.
- Exact installed size of conda environments (only compressed package sizes and crude closures computed).

## Sources

- https://github.com/Entware/Entware and https://github.com/Entware/Entware/wiki (Home, Install-on-Synology-NAS, Install-on-Remarkable, Install-on-Android, Alternative-install-vs-standard, Using-HTTPS-with-opkg read from the wiki git repo clone)
- https://api.github.com/repos/Entware/Entware (stats, commits, contributors, issues)
- https://bin.entware.net/ , https://bin.entware.net/Readme.txt , https://bin.entware.net/aarch64-k3.10/ , Packages.gz, installer/generic.sh, installer/opkg.conf, installer/opkg, archive/
- https://bin.entware.net/aarch64-k3.10/htop_3.5.3-1_aarch64-3.10.ipk (ELF headers)
- https://conda.anaconda.org/conda-forge/linux-aarch64/repodata.json.zst and https://api.anaconda.org/package/conda-forge/pip , /tcpdump , /iproute2
- https://github.com/mamba-org/mamba/releases (micromamba 2.10.0, 2026-10-23)
- Prior report: /home/andremmfaria/projects/thing-revived/docs/research/public-package-sources.md
