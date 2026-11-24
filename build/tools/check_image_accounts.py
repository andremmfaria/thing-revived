#!/usr/bin/env python3
"""Check the account lockdown of a built Thing Revived image (plan section 14, gate G1c).

Input is one of: a rootfs directory, an ext4 image, or a .wic (the root_a partition is carved
out with sfdisk data and read with debugfs, no root needed). Nothing found in the image is ever
executed: files are only read and parsed as text. Symlinks are resolved inside the image, never
on the host.

One failure line per failure, exit status 1 if there is any failure, 2 for a usage or input
error, 0 otherwise. Standard library only; sfdisk, dumpe2fs and debugfs are needed for images.
"""

import argparse
import fnmatch
import os
import re
import shutil
import subprocess
import sys
import tempfile

# ---------------------------------------------------------------------------------------------
# Expected paths and values. Everything that depends on the image recipes (T-P1.1, T-P1.2) is
# in this table so it is easy to adjust when those recipes land.
# ---------------------------------------------------------------------------------------------
SYSTEMD_UNIT_DIRS = ["/etc/systemd/system", "/usr/lib/systemd/system", "/lib/systemd/system"]

PATHS = {
    "passwd": "/etc/passwd",
    "shadow": "/etc/shadow",
    "group": "/etc/group",
    "shells": "/etc/shells",
    "securetty": "/etc/securetty",
    "sudoers": "/etc/sudoers",
    "sudoers_d": "/etc/sudoers.d",
    "pam_dir": "/etc/pam.d",
    "usb_debugging_gate": "/etc/usb-debugging-enabled",
    "build_flags": "/usr/share/thing-revived/accounts/build-flags",
    "share_dir": "/usr/share/thing-revived",
    "pam_common_auth": "/etc/pam.d/common-auth",
    "pam_common_account": "/etc/pam.d/common-account",
}
# sshd reads these; every one that exists must satisfy the effective settings below.
SSHD_CONFIGS = ["/etc/ssh/sshd_config", "/etc/ssh/sshd_config_readonly"]
SSHD_REQUIRED_CONFIG = "/etc/ssh/sshd_config"
SSHD_BASE_DIR = "/etc/ssh"

# Effective values (lower-case compare), first value wins. Section 4 drop-in.
SSHD_EXPECTED = {
    "permitrootlogin": ["no"],
    "permitemptypasswords": ["no"],
    "passwordauthentication": ["no"],
    "kbdinteractiveauthentication": ["no"],
    "pubkeyauthentication": ["yes"],
    "authenticationmethods": ["publickey"],
    "usepam": ["yes"],
    "authorizedkeysfile": ["/var/lib/thing-revived-ssh/authorized_keys/%u"],
    "authorizedkeyscommand": ["none"],
}
SSHD_MAX_AUTH_TRIES = 3
# The one Match block accepted: `Match User kiosk` with exactly these settings (delta plan 7.1).
KIOSK_MATCH_SETTINGS = {
    "passwordauthentication": ["yes"],
    "authenticationmethods": ["password", "publickey"],
}

# pam_faillock lines (delta plan 7.2). Fields are tab separated, options are one field.
FAILLOCK_OPTS = ["deny=5", "unlock_time=600", "fail_interval=900"]
FAILLOCK_AUTH_LINES = [
    ("auth", "required", "pam_faillock.so", "preauth"),
    ("auth", "[default=die]", "pam_faillock.so", "authfail"),
    ("auth", "sufficient", "pam_faillock.so", "authsucc"),
]
FAILLOCK_ACCOUNT_LINE = ("account", "required", "pam_faillock.so", None)
PAM_MODULE_DIRS = [
    "/lib/security", "/usr/lib/security", "/lib64/security", "/usr/lib64/security",
    "/lib/aarch64-linux-gnu/security", "/usr/lib/aarch64-linux-gnu/security",
]
SSHD_NAMES = {
    "permitrootlogin": "PermitRootLogin",
    "permitemptypasswords": "PermitEmptyPasswords",
    "passwordauthentication": "PasswordAuthentication",
    "kbdinteractiveauthentication": "KbdInteractiveAuthentication",
    "pubkeyauthentication": "PubkeyAuthentication",
    "authenticationmethods": "AuthenticationMethods",
    "usepam": "UsePAM",
    "authorizedkeysfile": "AuthorizedKeysFile",
    "authorizedkeyscommand": "AuthorizedKeysCommand",
}

KIOSK_USER = "kiosk"
KIOSK_UID = 1000
KIOSK_HOME = "/var/home/kiosk"
KIOSK_GROUPS = ["sudo", "adm"]
SUDO_GROUP = "sudo"
SUDO_BINARIES = ["/usr/bin/sudo", "/usr/bin/sudo.sudo"]

UART_FLAG = "THING_REVIVED_ENABLE_UART"
ADB_FLAG = "THING_REVIVED_ENABLE_ADB"
SERIAL_GETTY_TEMPLATES = ["serial-getty@.service"]

# adb/usb-bind and UART unit pairs (debug-toggles-plan.md section 4, T-D1.1/T-D1.2). Both units
# of each pair ship on every build; only which one is systemd-enabled depends on the boot flag,
# matched here against --adb-default/--uart-default rather than against the baked build-flags
# file, since the point of these checks is to catch the gate functions disagreeing with what the
# build was actually asked for.
ADB_UNIT = "android-tools-adbd.service"
ADB_DROPIN_NAME = "zz-thing-revived-toggle.conf"
BIND_UNIT = "thing-revived-usb-bind.service"
ADB_BASIC_WANTS = "basic.target.wants"
BIND_MULTIUSER_WANTS = "multi-user.target.wants"

UART_UNIT_TEMPLATE = "thing-revived-uart-root@.service"
UART_TTY = "ttyAML0"
UART_GETTY_WANTS = "getty.target.wants"

# unit name -> required settings. wanted_by: [Install] WantedBy must list it, and an enable
# symlink <target>.wants/<unit> must exist in one of ENABLE_DIRS. requires/after: values that
# must appear in [Unit] Requires= / After=. user: [Service] User=. condition: exact
# ConditionPathExists= lines that must be present.
REQUIRED_UNITS = {
    "thing-revived-etc.service": {"wanted_by": "sysinit.target"},
    "thing-revived-provision.service": {
        "wanted_by": "sysinit.target",
        "requires": ["thing-revived-etc.service"],
        "after": ["thing-revived-etc.service"],
    },
    "thing-revived-terminal-agent.service": {
        "user": KIOSK_USER,
        "after": ["thing-revived-provision.service"],
    },
    "controller-agent.service": {
        "after": ["thing-revived-provision.service", "thing-revived-etc.service"],
    },
    # thing-revived-usb-bind.service's own structural checks (Requires/After/Conflicts) and its
    # enablement (which is flag-dependent, not fixed) live in check_adb_pair instead, alongside
    # the android-tools-adbd.service side of the same pair.
}
USB_ROLE_UNIT = "superbird-usb-role.service"
USB_ROLE_DROPIN = "10-thing-revived-default-device.conf"
ETC_MOUNT_SCRIPT = "/usr/libexec/thing-revived/thing-revived-etc-mount.sh"
ETC_UPPER_BASE = "/var/lib/thing-revived/etc"
# (label, path as written in the script, required octal mode). The overlay root takes its mode
# from the upper dir, so 0700 there makes /etc 0700 and locks out non-root services.
ETC_DIR_MODES = [
    ("upper dir", "upper", "0755"),
    ("parent dir", "", "0700"),
    ("work dir", "work", "0700"),
]
ENABLE_DIRS = ["/etc/systemd/system", "/usr/lib/systemd/system", "/lib/systemd/system"]

# Directories scanned for private key markers (kept narrow: binaries elsewhere legitimately
# contain such strings in test data).
SECRET_SCAN_DIRS = ["/etc", "/usr/share/thing-revived", "/var", "/home", "/root"]
SECRET_SCAN_MAX_BYTES = 1 << 20
PRIVATE_KEY_RE = re.compile(rb"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----")

ROOT_PARTITION_NAME = "root_a"
MIN_FREE_MIB = 4
WARN_FREE_MIB = 8

IPK_REQUIRED = ["sudo", "thing-revived-accounts", "pam-plugin-faillock"]


class ImageError(Exception):
    """The input could not be read; reported as a usage error (exit 2)."""


# ---------------------------------------------------------------------------------------------
# Image access. Trees resolve symlinks inside the image by themselves.
# ---------------------------------------------------------------------------------------------
class Tree:
    """Read-only view of a rootfs. Subclasses give lstat, readlink, raw read and listing."""

    free_space = None  # (free_bytes, reserved_bytes) when known

    def _lstat(self, path):
        raise NotImplementedError

    def _readlink(self, path):
        raise NotImplementedError

    def _read_raw(self, path):
        raise NotImplementedError

    def _listdir_raw(self, path):
        raise NotImplementedError

    def resolve(self, path, follow_last=True):
        """Return a normalised absolute path with symlinks resolved inside the image."""
        pending = [p for p in path.split("/") if p]
        done = []
        hops = 0
        while pending:
            part = pending.pop(0)
            if part == ".":
                continue
            if part == "..":
                if done:
                    done.pop()
                continue
            candidate = "/" + "/".join(done + [part])
            if (pending or follow_last) and self._lstat(candidate) == "link":
                hops += 1
                if hops > 40:
                    raise ImageError("too many symlinks resolving " + path)
                target = self._readlink(candidate)
                if target is None:
                    raise ImageError("cannot read symlink " + candidate)
                if target.startswith("/"):
                    done = []
                pending = [p for p in target.split("/") if p] + pending
                continue
            done.append(part)
        return "/" + "/".join(done)

    def kind(self, path, follow=True):
        """'file', 'dir', 'link' or None."""
        try:
            return self._lstat(self.resolve(path, follow_last=follow))
        except ImageError:
            return None

    def exists(self, path):
        return self.kind(path) is not None

    def lexists(self, path):
        return self.kind(path, follow=False) is not None

    def read(self, path):
        """File bytes, or None when the path is missing or not a regular file."""
        try:
            real = self.resolve(path)
        except ImageError:
            return None
        if self._lstat(real) != "file":
            return None
        return self._read_raw(real)

    def text(self, path):
        data = self.read(path)
        return None if data is None else data.decode("utf-8", "replace")

    def listdir(self, path):
        try:
            real = self.resolve(path)
        except ImageError:
            return []
        if self._lstat(real) != "dir":
            return []
        return sorted(self._listdir_raw(real))

    def walk_files(self, path):
        """Yield absolute paths of regular files below path without following symlinks."""
        for name in self.listdir(path):
            child = path.rstrip("/") + "/" + name
            kind = self.kind(child, follow=False)
            if kind == "dir":
                yield from self.walk_files(child)
            elif kind == "file":
                yield child


class DirTree(Tree):
    def __init__(self, root):
        self.root = os.path.realpath(root)

    def _host(self, path):
        return os.path.join(self.root, path.lstrip("/"))

    def _lstat(self, path):
        host = self._host(path)
        if os.path.islink(host):
            return "link"
        if os.path.isdir(host):
            return "dir"
        if os.path.isfile(host):
            return "file"
        return None

    def _readlink(self, path):
        return os.readlink(self._host(path))

    def _read_raw(self, path):
        with open(self._host(path), "rb") as fh:
            return fh.read()

    def _listdir_raw(self, path):
        return os.listdir(self._host(path))


class Ext4Tree(Tree):
    """Read an ext4 image with debugfs (read-only, no root)."""

    def __init__(self, image):
        self.image = image
        self._stat_cache = {}
        self.free_space = self._free_space()

    def _debugfs(self, request):
        proc = subprocess.run(
            ["debugfs", "-R", request, self.image], capture_output=True, check=False
        )
        return proc.stdout, proc.stderr.decode("utf-8", "replace")

    def _free_space(self):
        proc = subprocess.run(["dumpe2fs", "-h", self.image], capture_output=True, text=True)
        fields = {}
        for line in proc.stdout.splitlines():
            key, _, value = line.partition(":")
            fields[key.strip()] = value.strip()
        try:
            block = int(fields["Block size"])
            return int(fields["Free blocks"]) * block, int(fields["Reserved block count"]) * block
        except (KeyError, ValueError):
            raise ImageError("dumpe2fs could not read block counts of " + self.image)

    def _stat(self, path):
        if path in self._stat_cache:
            return self._stat_cache[path]
        out, err = self._debugfs("stat " + _debugfs_quote(path))
        text = out.decode("utf-8", "replace")
        info = None
        match = re.search(r"Type:\s+(\w+)", text)
        if match and "not found" not in err:
            kind = {"regular": "file", "directory": "dir", "symlink": "link"}.get(match.group(1))
            link = re.search(r'Fast link dest: "(.*)"', text)
            info = (kind, link.group(1) if link else None)
        self._stat_cache[path] = info
        return info

    def _lstat(self, path):
        if path == "/":
            return "dir"
        info = self._stat(path)
        return info[0] if info else None

    def _readlink(self, path):
        info = self._stat(path)
        return info[1] if info else None

    def _read_raw(self, path):
        out, _ = self._debugfs("cat " + _debugfs_quote(path))
        return out

    def _listdir_raw(self, path):
        out, _ = self._debugfs("ls -p " + _debugfs_quote(path))
        names = []
        for line in out.decode("utf-8", "replace").splitlines():
            parts = line.split("/")
            if len(parts) >= 7 and parts[5] not in (".", ".."):
                names.append(parts[5])
        return names


def _debugfs_quote(path):
    if re.search(r"[\s\"']", path):
        raise ImageError("refusing unusual path " + repr(path))
    return path


def carve_root_partition(wic, workdir, name=ROOT_PARTITION_NAME, index=None):
    """Copy the root partition out of a disk image into workdir and return its path."""
    proc = subprocess.run(["sfdisk", "-d", wic], capture_output=True, text=True)
    if proc.returncode != 0:
        raise ImageError("sfdisk -d failed on %s: %s" % (wic, proc.stderr.strip()))
    sector = 512
    parts = []
    for line in proc.stdout.splitlines():
        match = re.match(r"sector-size:\s*(\d+)", line)
        if match:
            sector = int(match.group(1))
        match = re.match(r"\S+\s*:\s*(.*)", line)
        if not match or "start=" not in line:
            continue
        fields = dict(re.findall(r'(\w+)=\s*("[^"]*"|[^,\s]+)', match.group(1)))
        parts.append(
            (int(fields["start"]), int(fields["size"]), fields.get("name", "").strip('"'))
        )
    chosen = None
    if index is not None:
        if 1 <= index <= len(parts):
            chosen = parts[index - 1]
    else:
        for part in parts:
            if part[2] == name:
                chosen = part
    if chosen is None:
        raise ImageError(
            "no partition named %r in %s (use --root-partition N); found %s"
            % (name, wic, [p[2] for p in parts])
        )
    start, size, _ = chosen
    out = os.path.join(workdir, "root_a.ext4")
    remaining = size * sector
    with open(wic, "rb") as src, open(out, "wb") as dst:
        src.seek(start * sector)
        while remaining:
            chunk = src.read(min(remaining, 1 << 20))
            if not chunk:
                raise ImageError("wic is shorter than its partition table says")
            dst.write(chunk)
            remaining -= len(chunk)
    return out


def is_ext4(path):
    with open(path, "rb") as fh:
        fh.seek(1080)
        return fh.read(2) == b"\x53\xef"


# ---------------------------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------------------------
class Report:
    def __init__(self):
        self.failures = []
        self.warnings = []

    def fail(self, check, message):
        self.failures.append("FAIL [%s] %s" % (check, message))

    def warn(self, check, message):
        self.warnings.append("WARN [%s] %s" % (check, message))


def active_lines(text):
    """Non-empty, non-comment lines with backslash continuations joined."""
    joined = re.sub(r"\\\r?\n", " ", text)
    for raw in joined.splitlines():
        line = raw.strip()
        if line and not line.startswith("#"):
            yield line


# ---------------------------------------------------------------------------------------------
# Checks
# ---------------------------------------------------------------------------------------------
def parse_sshd(tree, path, depth=0):
    """Return (settings, matches). settings: key -> (args, file), first value wins, global scope.

    matches is a list of (file, criteria_args, [(key, args)]) for every Match block found.
    """
    settings = {}
    matches = []
    text = tree.text(path)
    if text is None or depth > 4:
        return settings, matches
    current = None
    for line in active_lines(text):
        parts = _split_args(line)
        key = parts[0].lower()
        args = parts[1:]
        if key == "match":
            current = (path, args, [])
            matches.append(current)
            continue
        if key == "include" and current is None:
            for pattern in args:
                for inc in _expand_include(tree, pattern):
                    sub, sub_matches = parse_sshd(tree, inc, depth + 1)
                    for k, v in sub.items():
                        settings.setdefault(k, v)
                    matches.extend(sub_matches)
            continue
        if current is not None:
            current[2].append((key, args))
            continue
        settings.setdefault(key, (args, path))
    return settings, matches


def _split_args(line):
    line = re.sub(r"^([A-Za-z]+)\s*=\s*", r"\1 ", line)
    return [a.strip('"') for a in re.findall(r'"[^"]*"|\S+', line)]


def _expand_include(tree, pattern):
    if not pattern.startswith("/"):
        pattern = SSHD_BASE_DIR + "/" + pattern
    directory, _, base = pattern.rpartition("/")
    return [
        directory + "/" + name
        for name in tree.listdir(directory or "/")
        if fnmatch.fnmatchcase(name, base)
    ]


def is_kiosk_match(criteria):
    return [c.lower() for c in criteria[:1]] == ["user"] and criteria[1:] == [KIOSK_USER]


def check_match_blocks(config, matches, report):
    kiosk_blocks = 0
    for path, criteria, body in matches:
        label = "Match " + " ".join(criteria)
        if not is_kiosk_match(criteria):
            report.fail("sshd", "%s: a Match block in %s is not allowed: '%s'" % (config, path, label))
            continue
        kiosk_blocks += 1
        seen = set()
        for key, args in body:
            name = SSHD_NAMES.get(key, key)
            if key in ("permitrootlogin", "permitemptypasswords"):
                report.fail("sshd", "%s: '%s' in %s overrides %s" % (config, label, path, name))
            elif key not in KIOSK_MATCH_SETTINGS or key in seen:
                report.fail(
                    "sshd",
                    "%s: '%s' in %s has an extra or repeated key %s" % (config, label, path, name),
                )
            else:
                expected = KIOSK_MATCH_SETTINGS[key]
                if [a.lower() for a in args] != expected:
                    report.fail(
                        "sshd",
                        "%s: '%s' %s is '%s', expected '%s'"
                        % (config, label, name, " ".join(args), " ".join(expected)),
                    )
            seen.add(key)
        for key in KIOSK_MATCH_SETTINGS:
            if key not in seen:
                report.fail("sshd", "%s: 'Match User kiosk' does not set %s" % (config, SSHD_NAMES[key]))
    if kiosk_blocks == 0:
        report.fail("sshd", "%s: no 'Match User kiosk' block" % config)
    elif kiosk_blocks > 1:
        report.fail("sshd", "%s: %d 'Match User kiosk' blocks, expected exactly one" % (config, kiosk_blocks))


def check_sshd(tree, report):
    if not tree.exists(SSHD_REQUIRED_CONFIG):
        report.fail("sshd", "%s is missing" % SSHD_REQUIRED_CONFIG)
        return
    for config in SSHD_CONFIGS:
        if not tree.exists(config):
            continue
        settings, matches = parse_sshd(tree, config)
        check_match_blocks(config, matches, report)
        for key, expected in SSHD_EXPECTED.items():
            name = SSHD_NAMES[key]
            if key not in settings:
                report.fail(
                    "sshd", "%s: %s is not set, expected '%s'" % (config, name, " ".join(expected))
                )
                continue
            args, source = settings[key]
            got = [a if key == "authorizedkeysfile" else a.lower() for a in args]
            if got != expected:
                report.fail(
                    "sshd",
                    "%s: %s is '%s' (from %s), expected '%s'"
                    % (config, name, " ".join(args), source, " ".join(expected)),
                )
        if "maxauthtries" not in settings:
            report.fail("sshd", "%s: MaxAuthTries is not set, expected <= %d" % (config, SSHD_MAX_AUTH_TRIES))
        else:
            args, source = settings["maxauthtries"]
            if len(args) != 1 or not args[0].isdigit() or not 1 <= int(args[0]) <= SSHD_MAX_AUTH_TRIES:
                report.fail(
                    "sshd",
                    "%s: MaxAuthTries is '%s' (from %s), expected 1 to %d"
                    % (config, " ".join(args), source, SSHD_MAX_AUTH_TRIES),
                )


def parse_colon_file(text):
    rows = []
    for line in (text or "").splitlines():
        if line.strip() and not line.startswith("#"):
            rows.append(line.split(":"))
    return rows


def check_accounts(tree, report):
    shadow_text = tree.text(PATHS["shadow"])
    passwd_text = tree.text(PATHS["passwd"])
    group_text = tree.text(PATHS["group"])
    if shadow_text is None:
        report.fail("shadow", "%s is missing" % PATHS["shadow"])
        return
    if passwd_text is None:
        report.fail("passwd", "%s is missing" % PATHS["passwd"])
        return
    shadow = {}
    for row in parse_colon_file(shadow_text):
        if len(row) < 2:
            report.fail("shadow", "malformed line in %s" % PATHS["shadow"])
            continue
        shadow[row[0]] = row[1]
        if row[1] == "":
            report.fail("shadow", "account '%s' has an empty password hash" % row[0])
        elif row[1][:1] not in ("*", "!") and row[0] not in ("root", KIOSK_USER):
            report.fail("shadow", "account '%s' ships a password hash" % row[0])
    if "root" not in shadow:
        report.fail("shadow", "root has no entry in %s" % PATHS["shadow"])
    elif shadow["root"][:1] not in ("*", "!"):
        report.fail("shadow", "root is not locked (hash must start with '*' or '!')")

    passwd = {row[0]: row for row in parse_colon_file(passwd_text) if len(row) >= 7}
    for name, row in passwd.items():
        if row[2] == "0" and name != "root":
            report.fail("passwd", "account '%s' has uid 0" % name)

    kiosk = passwd.get(KIOSK_USER)
    if kiosk is None:
        report.fail("kiosk", "account '%s' is missing from %s" % (KIOSK_USER, PATHS["passwd"]))
        return
    if kiosk[2] != str(KIOSK_UID):
        report.fail("kiosk", "%s has uid %s, expected %d" % (KIOSK_USER, kiosk[2], KIOSK_UID))
    if kiosk[5] != KIOSK_HOME:
        report.fail("kiosk", "%s home is '%s', expected '%s'" % (KIOSK_USER, kiosk[5], KIOSK_HOME))
    shells = [l for l in active_lines(tree.text(PATHS["shells"]) or "")]
    if kiosk[6] not in shells:
        report.fail("kiosk", "%s shell '%s' is not in %s" % (KIOSK_USER, kiosk[6], PATHS["shells"]))
    hash_ = shadow.get(KIOSK_USER)
    if hash_ is None:
        report.fail("kiosk", "%s has no entry in %s" % (KIOSK_USER, PATHS["shadow"]))
    elif hash_[:1] not in ("*", "!", ""):
        report.fail("kiosk", "%s ships a password hash, expected '!'" % KIOSK_USER)
    elif hash_ != "!":
        report.fail("kiosk", "%s hash is '%s', expected '!'" % (KIOSK_USER, hash_))
    members = {}
    for row in parse_colon_file(group_text):
        if len(row) >= 4:
            members[row[0]] = set(filter(None, row[3].split(",")))
    for group in KIOSK_GROUPS:
        if KIOSK_USER not in members.get(group, set()):
            report.fail("kiosk", "%s is not in group '%s'" % (KIOSK_USER, group))


def check_no_baked_keys(tree, report):
    share = PATHS["share_dir"]
    kiosk_key = share + "/accounts/kiosk.authorized_keys"
    if tree.lexists(kiosk_key):
        report.fail("key", "baked key file %s must not exist" % kiosk_key)
    for file_path in tree.walk_files(share):
        if file_path.endswith(".authorized_keys") and file_path != kiosk_key:
            report.fail("key", "baked key file %s must not exist" % file_path)


def check_no_private_keys(tree, report):
    for base in SECRET_SCAN_DIRS:
        for file_path in tree.walk_files(base):
            data = tree.read(file_path)
            if data is not None and len(data) <= SECRET_SCAN_MAX_BYTES and PRIVATE_KEY_RE.search(data):
                report.fail("secrets", "private key marker in %s" % file_path)


def check_sudo(tree, report):
    if not any(tree.exists(p) for p in SUDO_BINARIES):
        report.fail("sudo", "no sudo binary (%s)" % " or ".join(SUDO_BINARIES))
    files = [PATHS["sudoers"]]
    for name in tree.listdir(PATHS["sudoers_d"]):
        if "." not in name and not name.endswith("~"):  # sudo skips these names too
            files.append(PATHS["sudoers_d"] + "/" + name)
    rule = re.compile(r"^%" + SUDO_GROUP + r"\s+ALL\s*=\s*\(ALL(:ALL)?\)\s+ALL$")
    found_rule = False
    for path in files:
        text = tree.text(path)
        if text is None:
            if path == PATHS["sudoers"]:
                report.fail("sudoers", "%s is missing" % path)
            continue
        for line in active_lines(text):
            if line.startswith("@includedir") or line.startswith("#include"):
                continue
            if "NOPASSWD" in line:
                report.fail("sudoers", "%s: NOPASSWD in '%s'" % (path, line))
            if re.search(r"!\s*authenticate", line):
                report.fail("sudoers", "%s: !authenticate in '%s'" % (path, line))
            if re.search(r"\b(targetpw|rootpw|runaspw)\b", line) and "!" not in line:
                report.fail("sudoers", "%s: asks for another user's password in '%s'" % (path, line))
            if rule.match(line):
                found_rule = True
    if not found_rule:
        report.fail("sudoers", "no '%%%s ALL=(ALL:ALL) ALL' rule" % SUDO_GROUP)


def _faillock_lines(text):
    """Yield (type, control, module, args) for each tab separated faillock line."""
    for line in active_lines(text or ""):
        fields = re.split(r"\t+", line, maxsplit=2)
        if len(fields) != 3:
            continue
        module = fields[2].split()
        if module and module[0] == "pam_faillock.so":
            yield fields[0], fields[1], module[0], module[1:]


def check_faillock(tree, report):
    auth_path = PATHS["pam_common_auth"]
    auth = list(_faillock_lines(tree.text(auth_path)))
    for typ, control, module, keyword in FAILLOCK_AUTH_LINES:
        want = [keyword] + FAILLOCK_OPTS
        if not any((t, c, m) == (typ, control, module) and a == want for t, c, m, a in auth):
            report.fail(
                "pam",
                "%s lacks tab separated '%s %s %s %s'"
                % (auth_path, typ, control, module, " ".join(want)),
            )
    account_path = PATHS["pam_common_account"]
    typ, control, module, _ = FAILLOCK_ACCOUNT_LINE
    if not any((t, c, m, a) == (typ, control, module, []) for t, c, m, a in
               _faillock_lines(tree.text(account_path))):
        report.fail("pam", "%s lacks tab separated '%s %s %s'" % (account_path, typ, control, module))
    if not any(tree.exists(d + "/pam_faillock.so") for d in PAM_MODULE_DIRS):
        report.fail("pam", "pam_faillock.so is not installed (looked in %s)" % ", ".join(PAM_MODULE_DIRS))


def check_pam(tree, report):
    names = tree.listdir(PATHS["pam_dir"])
    if not names:
        report.fail("pam", "%s is missing or empty" % PATHS["pam_dir"])
    for name in names:
        path = PATHS["pam_dir"] + "/" + name
        text = tree.text(path)
        for line in active_lines(text or ""):
            if re.search(r"\bnullok(_secure)?\b", line):
                report.fail("pam", "nullok in %s: '%s'" % (path, line))
    check_faillock(tree, report)


def read_flags(tree, report):
    text = tree.text(PATHS["build_flags"])
    if text is None:
        report.fail("flags", "%s is missing" % PATHS["build_flags"])
        return {UART_FLAG: False, ADB_FLAG: False}
    values = {}
    for line in active_lines(text):
        key, sep, value = line.partition("=")
        if sep:
            values[key.strip()] = value.strip().strip("\"'")
    return {flag: values.get(flag) == "1" for flag in (UART_FLAG, ADB_FLAG)}


def serial_getty_files(tree):
    found = []
    for directory in SYSTEMD_UNIT_DIRS:
        for name in tree.listdir(directory):
            base = name[:-2] if name.endswith(".d") else name
            if not fnmatch.fnmatchcase(base, "serial-getty@*.service"):
                continue
            path = directory + "/" + name
            if tree.kind(path) == "dir":
                found.extend(path + "/" + n for n in tree.listdir(path) if n.endswith(".conf"))
            else:
                found.append(path)
    return found


def check_side_doors(tree, report, flags):
    # Unrelated to the adb/UART unit pairs below: this is the boot-time-only marker the
    # usb-role drop-in reads (check_usb_role_default), kept in sync with the baked build-flags
    # file rather than with --adb-default.
    if not flags[ADB_FLAG] and tree.lexists(PATHS["usb_debugging_gate"]):
        report.fail(
            "adb", "%s exists but %s is not set" % (PATHS["usb_debugging_gate"], ADB_FLAG)
        )


def find_dropin(tree, unit_name, dropin_name):
    for directory in SYSTEMD_UNIT_DIRS:
        path = "%s/%s.d/%s" % (directory, unit_name, dropin_name)
        if tree.exists(path):
            return path
    return None


def unit_enabled(tree, wants_dir, unit_name):
    return any(tree.lexists("%s/%s/%s" % (d, wants_dir, unit_name)) for d in ENABLE_DIRS)


def check_adb_pair(tree, report, adb_default):
    """android-tools-adbd.service / thing-revived-usb-bind.service (debug-toggles-plan.md 4.1).

    Both units must always ship; the adbd drop-in and the bind unit must each carry the other's
    name in Conflicts= and neither may gate on /etc/usb-debugging-enabled (a Condition there
    would make a runtime `systemctl start` a silent no-op, defeating the live toggle); and
    exactly one of the two must be enabled, matching --adb-default.
    """
    adb_path = find_unit(tree, ADB_UNIT)
    bind_path = find_unit(tree, BIND_UNIT)
    if adb_path is None:
        report.fail("adb-pair", "%s is not installed" % ADB_UNIT)
    if bind_path is None:
        report.fail("adb-pair", "%s is not installed" % BIND_UNIT)
    if adb_path is None or bind_path is None:
        return

    bind_unit = parse_unit(tree.text(bind_path) or "")
    if "superbird-usb-gadget.service" not in unit_words(bind_unit, "Unit", "Requires"):
        report.fail("adb-pair", "%s lacks Requires=superbird-usb-gadget.service" % BIND_UNIT)
    for ordering in ("superbird-usb-gadget.service", "superbird-usb-role.service"):
        if ordering not in unit_words(bind_unit, "Unit", "After"):
            report.fail("adb-pair", "%s lacks After=%s" % (BIND_UNIT, ordering))
    if ADB_UNIT not in unit_words(bind_unit, "Unit", "Conflicts"):
        report.fail("adb-pair", "%s has no Conflicts=%s" % (BIND_UNIT, ADB_UNIT))
    if any(PATHS["usb_debugging_gate"] in c for c in bind_unit.get("Unit", {}).get("ConditionPathExists", [])):
        report.fail(
            "adb-pair", "%s has a ConditionPathExists on %s" % (BIND_UNIT, PATHS["usb_debugging_gate"])
        )

    dropin_label = "%s.d/%s" % (ADB_UNIT, ADB_DROPIN_NAME)
    dropin_path = find_dropin(tree, ADB_UNIT, ADB_DROPIN_NAME)
    if dropin_path is None:
        report.fail("adb-pair", "%s is not installed" % dropin_label)
    else:
        dropin = parse_unit(tree.text(dropin_path) or "")
        if BIND_UNIT not in unit_words(dropin, "Unit", "Conflicts"):
            report.fail("adb-pair", "%s has no Conflicts=%s" % (dropin_label, BIND_UNIT))
        if any(PATHS["usb_debugging_gate"] in c for c in dropin.get("Unit", {}).get("ConditionPathExists", [])):
            report.fail(
                "adb-pair", "%s has a ConditionPathExists on %s" % (dropin_label, PATHS["usb_debugging_gate"])
            )

    adb_enabled = unit_enabled(tree, ADB_BASIC_WANTS, ADB_UNIT)
    bind_enabled = unit_enabled(tree, BIND_MULTIUSER_WANTS, BIND_UNIT)
    if adb_default:
        if not adb_enabled:
            report.fail("adb-pair", "%s is not enabled, expected --adb-default 1" % ADB_UNIT)
        if bind_enabled:
            report.fail(
                "adb-pair",
                "%s is enabled, expected --adb-default 1 (only %s should be enabled)" % (BIND_UNIT, ADB_UNIT),
            )
    else:
        if not bind_enabled:
            report.fail("adb-pair", "%s is not enabled, expected --adb-default 0" % BIND_UNIT)
        if adb_enabled:
            report.fail(
                "adb-pair",
                "%s is enabled, expected --adb-default 0 (only %s should be enabled)" % (ADB_UNIT, BIND_UNIT),
            )


def check_uart_pair(tree, report, uart_default):
    """serial-getty@.service / thing-revived-uart-root@.service (debug-toggles-plan.md 4.4).

    The shared serial-getty@ template (and any per-instance override) must never carry an
    autologin, in every build; the templated root console unit must always ship, fully
    resolved (no leftover @@..@@ placeholder) and Conflicts=serial-getty@%i.service; exactly one
    of the two must be enabled for ttyAML0, matching --uart-default; and /etc/securetty must
    list ttyAML0 on every build, since the runtime toggle needs root login permitted there
    regardless of the boot default.
    """
    for path in serial_getty_files(tree):
        for line in active_lines(tree.text(path) or ""):
            if line.startswith("ExecStart") and re.search(r"(--autologin|\s-a)\s*\S", line):
                report.fail(
                    "uart-pair",
                    "autologin in %s; the shared serial-getty template must stay untouched in every build" % path,
                )
    if not any(tree.exists(d + "/" + t) for d in SYSTEMD_UNIT_DIRS for t in SERIAL_GETTY_TEMPLATES):
        report.warn("uart-pair", "no serial-getty@.service template found")

    template_path = find_unit(tree, UART_UNIT_TEMPLATE)
    if template_path is None:
        report.fail("uart-pair", "%s is not installed" % UART_UNIT_TEMPLATE)
    else:
        uart_unit = parse_unit(tree.text(template_path) or "")
        exec_starts = uart_unit.get("Service", {}).get("ExecStart", [])
        if any("@@" in v for v in exec_starts):
            report.fail(
                "uart-pair", "%s still has an unresolved @@..@@ placeholder in ExecStart" % UART_UNIT_TEMPLATE
            )
        if not any(re.search(r"--autologin\s+root\b", v) for v in exec_starts):
            report.fail("uart-pair", "%s ExecStart lacks --autologin root" % UART_UNIT_TEMPLATE)
        if "serial-getty@%i.service" not in unit_words(uart_unit, "Unit", "Conflicts"):
            report.fail("uart-pair", "%s has no Conflicts=serial-getty@%%i.service" % UART_UNIT_TEMPLATE)

    uart_instance = "thing-revived-uart-root@%s.service" % UART_TTY
    serial_instance = "serial-getty@%s.service" % UART_TTY
    uart_enabled = unit_enabled(tree, UART_GETTY_WANTS, uart_instance)
    serial_enabled = unit_enabled(tree, UART_GETTY_WANTS, serial_instance)
    if uart_default:
        if not uart_enabled:
            report.fail("uart-pair", "%s is not enabled, expected --uart-default 1" % uart_instance)
        if serial_enabled:
            report.fail(
                "uart-pair",
                "%s is enabled, expected --uart-default 1 (only %s should be enabled)" % (serial_instance, uart_instance),
            )
    else:
        if not serial_enabled:
            report.fail("uart-pair", "%s is not enabled, expected --uart-default 0" % serial_instance)
        if uart_enabled:
            report.fail(
                "uart-pair",
                "%s is enabled, expected --uart-default 0 (only %s should be enabled)" % (uart_instance, serial_instance),
            )

    securetty = tree.text(PATHS["securetty"])
    if securetty is None:
        report.fail("securetty", "%s is missing, expected to list %s" % (PATHS["securetty"], UART_TTY))
    elif UART_TTY not in list(active_lines(securetty)):
        report.fail("securetty", "%s does not list %s" % (PATHS["securetty"], UART_TTY))


def parse_unit(text):
    """Return {section: {key: [values]}}."""
    unit = {}
    section = None
    for line in active_lines(text):
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1]
            unit.setdefault(section, {})
        elif section is not None and "=" in line:
            key, _, value = line.partition("=")
            unit[section].setdefault(key.strip(), []).append(value.strip())
    return unit


def unit_words(unit, section, key):
    return [w for v in unit.get(section, {}).get(key, []) for w in v.split()]


def find_unit(tree, name):
    for directory in SYSTEMD_UNIT_DIRS:
        path = directory + "/" + name
        if tree.exists(path):
            return path
    return None


def check_units(tree, report):
    for name, want in REQUIRED_UNITS.items():
        path = find_unit(tree, name)
        if path is None:
            report.fail("units", "%s is not installed" % name)
            continue
        unit = parse_unit(tree.text(path) or "")
        target = want.get("wanted_by")
        if target:
            if target not in unit_words(unit, "Install", "WantedBy"):
                report.fail("units", "%s has no [Install] WantedBy=%s" % (name, target))
            if not any(tree.lexists("%s/%s.wants/%s" % (d, target, name)) for d in ENABLE_DIRS):
                report.fail("units", "%s is not enabled (no %s.wants symlink)" % (name, target))
        for requirement in want.get("requires", []):
            if requirement not in unit_words(unit, "Unit", "Requires"):
                report.fail("units", "%s lacks Requires=%s" % (name, requirement))
        for ordering in want.get("after", []):
            if ordering not in unit_words(unit, "Unit", "After"):
                report.fail("units", "%s lacks After=%s" % (name, ordering))
        for condition in want.get("condition", []):
            if condition not in unit.get("Unit", {}).get("ConditionPathExists", []):
                report.fail("units", "%s lacks ConditionPathExists=%s" % (name, condition))
        if "user" in want and unit.get("Service", {}).get("User", [None])[0] != want["user"]:
            report.fail("units", "%s does not run with User=%s" % (name, want["user"]))
    for directory in SYSTEMD_UNIT_DIRS:
        for name in tree.listdir(directory):
            if name.startswith("thing-revived-") and name.endswith(".service"):
                unit = parse_unit(tree.text(directory + "/" + name) or "")
                if "Requisite" in unit.get("Unit", {}):
                    report.fail("units", "%s uses Requisite= (use Requires=)" % name)


def _script_chmod_modes(text):
    """Map each chmod target word of a shell script to the list of modes it is given."""
    modes = {}
    for line in active_lines(text):
        for command in re.split(r"[;&|]+|&&|\|\|", line):
            words = command.split()
            if len(words) >= 3 and words[0] == "chmod" and re.fullmatch(r"0?[0-7]{3,4}", words[1]):
                mode = words[1].zfill(4)
                for target in words[2:]:
                    modes.setdefault(target.strip("\"'"), []).append(mode)
    return modes


def check_etc_overlay_modes(tree, report):
    """The /etc overlay upper dir must be 0755, its parent and work dir 0700 (mode of /etc)."""
    text = tree.text(ETC_MOUNT_SCRIPT)
    if text is None:
        report.fail("etc-overlay", "%s is missing" % ETC_MOUNT_SCRIPT)
        return
    modes = _script_chmod_modes(text)
    for label, sub, want in ETC_DIR_MODES:
        names = ["$UPPER_BASE/" + sub, "${UPPER_BASE}/" + sub] if sub else ["$UPPER_BASE", "${UPPER_BASE}"]
        if not sub:
            names.append("/var/lib/thing-revived")
        else:
            names.append(ETC_UPPER_BASE + "/" + sub)
        got = [m for n in names for m in modes.get(n, [])]
        if not got:
            report.fail("etc-overlay", "%s does not chmod the %s to %s" % (ETC_MOUNT_SCRIPT, label, want))
        elif any(m != want for m in got):
            report.fail(
                "etc-overlay",
                "%s sets the %s to %s, expected %s"
                % (ETC_MOUNT_SCRIPT, label, " and ".join(sorted(set(got))), want),
            )


def check_usb_role_default(tree, report):
    """The USB port boots device unless boot-role says host; adb dev builds always boot device."""
    for directory in SYSTEMD_UNIT_DIRS:
        path = "%s/%s/%s" % (directory, USB_ROLE_UNIT + ".d", USB_ROLE_DROPIN)
        if tree.exists(path):
            break
    else:
        report.fail("usb-role", "%s drop-in is not installed" % USB_ROLE_DROPIN)
        return
    unit = parse_unit(tree.text(path) or "")
    # An empty ExecStart= resets the stock one; the last non-empty value is the effective command.
    commands = [v for v in unit.get("Service", {}).get("ExecStart", []) if v]
    command = commands[-1] if commands else ""
    if "to-device" not in command or "to-host --no-revert" not in command:
        report.fail("usb-role", "%s ExecStart lacks to-device and to-host --no-revert" % USB_ROLE_DROPIN)
    elif "|| echo device" not in command or '= "host"' not in command:
        report.fail("usb-role", "%s does not default to device unless boot-role is host" % USB_ROLE_DROPIN)
    elif command.find("-e /etc/usb-debugging-enabled") < 0 or command.find("-e /etc/usb-debugging-enabled") > command.find("boot-role"):
        report.fail("usb-role", "%s does not force device when /etc/usb-debugging-enabled exists" % USB_ROLE_DROPIN)
    if "ConditionPathExists" not in unit.get("Unit", {}) or any(
        unit["Unit"]["ConditionPathExists"]
    ):
        report.fail("usb-role", "%s does not reset ConditionPathExists=" % USB_ROLE_DROPIN)


def check_free_space(tree, report, min_mib):
    if tree.free_space is None:
        report.warn("space", "free space not checked (directory input)")
        return
    free, reserved = tree.free_space
    mib = free / (1 << 20)
    note = "%.1f MiB free, %.1f MiB of it reserved" % (mib, reserved / (1 << 20))
    if mib < min_mib:
        report.fail("space", "root_a has %s, need at least %s MiB" % (note, min_mib))
    elif mib < WARN_FREE_MIB:
        report.warn("space", "root_a has %s, under %d MiB" % (note, WARN_FREE_MIB))


def check_ipks(ipk_dir, report):
    names = os.listdir(ipk_dir) if os.path.isdir(ipk_dir) else []
    for package in IPK_REQUIRED:
        if not any(re.match(re.escape(package) + r"_[^/]*\.ipk$", n) for n in names):
            report.fail("ipk", "no %s_*.ipk in %s" % (package, ipk_dir))


def run_checks(tree, ipk_dir=None, min_free_mib=MIN_FREE_MIB, adb_default=False, uart_default=False):
    report = Report()
    flags = read_flags(tree, report)
    check_sshd(tree, report)
    check_accounts(tree, report)
    check_no_baked_keys(tree, report)
    check_no_private_keys(tree, report)
    check_sudo(tree, report)
    check_pam(tree, report)
    check_side_doors(tree, report, flags)
    check_units(tree, report)
    check_adb_pair(tree, report, adb_default)
    check_uart_pair(tree, report, uart_default)
    check_usb_role_default(tree, report)
    check_etc_overlay_modes(tree, report)
    check_free_space(tree, report, min_free_mib)
    if ipk_dir:
        check_ipks(ipk_dir, report)
    return report


def open_tree(path, workdir, root_partition):
    if os.path.isdir(path):
        return DirTree(path)
    if not os.path.isfile(path):
        raise ImageError("no such file or directory: " + path)
    if is_ext4(path):
        return Ext4Tree(path)
    return Ext4Tree(carve_root_partition(path, workdir, index=root_partition))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("image", help="rootfs directory, ext4 image or .wic")
    parser.add_argument("--ipk-dir", help="deploy ipk directory (sudo and accounts ipks)")
    parser.add_argument("--min-free-mib", type=int, default=MIN_FREE_MIB)
    parser.add_argument("--root-partition", type=int, help="1-based partition index, if unnamed")
    parser.add_argument(
        "--adb-default", type=int, choices=[0, 1], default=0,
        help="expected boot-time THING_REVIVED_ENABLE_ADB (which of the adb pair is enabled)",
    )
    parser.add_argument(
        "--uart-default", type=int, choices=[0, 1], default=0,
        help="expected boot-time THING_REVIVED_ENABLE_UART (which of the UART pair is enabled)",
    )
    args = parser.parse_args(argv)
    with tempfile.TemporaryDirectory(prefix="check-image-accounts-") as workdir:
        try:
            tree = open_tree(args.image, workdir, args.root_partition)
            report = run_checks(
                tree, args.ipk_dir, args.min_free_mib,
                bool(args.adb_default), bool(args.uart_default),
            )
        except ImageError as exc:
            print("ERROR: %s" % exc, file=sys.stderr)
            return 2
    for line in report.warnings:
        print(line)
    for line in report.failures:
        print(line)
    if report.failures:
        print("%d check failure(s)" % len(report.failures))
        return 1
    print("OK: image account checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
