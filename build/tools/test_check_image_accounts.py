"""Tests for check_image_accounts.py.

Fixture layout under fixtures/: good/ is a complete synthetic rootfs, bad/<case>/ and ok/<case>/
hold only the files that differ from good/ (an optional DELETE file lists paths to remove). A
case is materialised by copying good/, applying the overlay, then running the checker. All
content is synthetic. The private key marker case is built at run time so no key-like text is
committed.
"""

import os
import shutil
import shutil as _shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import check_image_accounts as cia  # noqa: E402

FIXTURES = os.path.join(HERE, "fixtures")
GOOD = os.path.join(FIXTURES, "good")
TOOL = os.path.join(HERE, "check_image_accounts.py")

# bad case -> substrings that must each appear in at least one failure line
BAD_CASES = {
    "sshd_root_login_yes": ["PermitRootLogin is 'yes'"],
    "sshd_password_auth_early": ["PasswordAuthentication is 'yes'"],
    "sshd_readonly_empty_passwords": ["sshd_config_readonly: PermitEmptyPasswords is 'yes'"],
    "sshd_kbdinteractive": ["KbdInteractiveAuthentication is 'yes'"],
    "sshd_authkeys_path": ["AuthorizedKeysFile is '%h/.ssh/authorized_keys"],
    "key_baked_present": ["baked key file /usr/share/thing-revived/accounts/kiosk.authorized_keys"],
    "key_other_baked": ["baked key file /usr/share/thing-revived/other.authorized_keys"],
    "password_global": ["PasswordAuthentication is 'yes'"],
    "match_other_user": ["a Match block", "'Match User admin'", "no 'Match User kiosk' block"],
    "match_extra_key": ["extra or repeated key allowtcpforwarding"],
    "match_root_override": ["overrides PermitRootLogin"],
    "match_missing": ["no 'Match User kiosk' block"],
    "match_duplicate": ["2 'Match User kiosk' blocks, expected exactly one"],
    "kiosk_authmethods_wrong": ["AuthenticationMethods is 'publickey password', expected 'password publickey'"],
    "maxauthtries_high": ["MaxAuthTries is '6'"],
    "maxauthtries_missing": ["MaxAuthTries is not set"],
    "faillock_missing": [
        "pam_faillock.so preauth deny=5", "pam_faillock.so authfail deny=5",
        "pam_faillock.so authsucc deny=5",
    ],
    "faillock_args_wrong": ["pam_faillock.so preauth deny=5 unlock_time=600 fail_interval=900"],
    "faillock_account_missing": ["common-account lacks"],
    "faillock_so_missing": ["pam_faillock.so is not installed"],
    "other_has_hash": ["account 'daemon' ships a password hash"],
    "kiosk_hash_star": ["kiosk hash is '*', expected '!'"],
    "sshd_no_dropin": ["PermitRootLogin is not set", "AuthorizedKeysFile is not set"],
    "sshd_match_block": ["a Match block", "'Match Address 10.0.0.0/8'"],
    "root_empty_hash": ["account 'root' has an empty password hash", "root is not locked"],
    "root_unlocked": ["root is not locked"],
    "other_empty_hash": ["account 'daemon' has an empty password hash"],
    "second_uid0": ["account 'toor' has uid 0"],
    "kiosk_missing": ["account 'kiosk' is missing"],
    "kiosk_not_adm": ["kiosk is not in group 'adm'"],
    "kiosk_not_sudo": ["kiosk is not in group 'sudo'"],
    "kiosk_has_hash": ["kiosk ships a password hash"],
    "kiosk_home": ["kiosk home is '/home/kiosk'"],
    "sudoers_nopasswd": ["NOPASSWD", "no '%sudo ALL=(ALL:ALL) ALL' rule"],
    "sudoers_d_nopasswd": ["/etc/sudoers.d/kiosk: NOPASSWD"],
    "sudoers_noauth": ["!authenticate"],
    "sudoers_no_rule": ["no '%sudo ALL=(ALL:ALL) ALL' rule"],
    "sudo_missing": ["no sudo binary"],
    "pam_nullok": ["nullok in /etc/pam.d/common-auth"],
    "securetty_entries": ["/etc/securetty is not empty"],
    "uart_autologin": ["autologin in", "THING_REVIVED_ENABLE_UART is not set"],
    "adb_gate": ["/etc/usb-debugging-enabled exists", "THING_REVIVED_ENABLE_ADB is not set"],
    "flags_missing": ["build-flags is missing"],
    "unit_missing": ["thing-revived-provision.service is not installed"],
    "unit_not_enabled": ["thing-revived-etc.service is not enabled"],
    "unit_no_install": ["thing-revived-etc.service has no [Install] WantedBy=sysinit.target"],
    "unit_requisite": ["uses Requisite=", "lacks Requires=thing-revived-etc.service"],
    "terminal_root": ["thing-revived-terminal-agent.service does not run with User=kiosk"],
}


def materialise(dest, case_dir=None, extra=None):
    """Copy good/ to dest, apply an overlay directory and extra {relpath: bytes}."""
    shutil.copytree(GOOD, dest, symlinks=True)
    if case_dir:
        delete = os.path.join(case_dir, "DELETE")
        if os.path.exists(delete):
            with open(delete, encoding="utf-8") as fh:
                for rel in fh.read().split():
                    os.remove(os.path.join(dest, rel))
        for root, _dirs, files in os.walk(case_dir):
            for name in files:
                src = os.path.join(root, name)
                rel = os.path.relpath(src, case_dir)
                if rel == "DELETE":
                    continue
                target = os.path.join(dest, rel)
                os.makedirs(os.path.dirname(target), exist_ok=True)
                shutil.copy2(src, target)
    for rel, data in (extra or {}).items():
        target = os.path.join(dest, rel)
        os.makedirs(os.path.dirname(target), exist_ok=True)
        with open(target, "wb") as fh:
            fh.write(data)


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def tree(self, case_dir=None, extra=None):
        dest = os.path.join(self.tmp.name, "rootfs")
        materialise(dest, case_dir, extra)
        return cia.DirTree(dest)

    def failures(self, tree, **kwargs):
        return cia.run_checks(tree, **kwargs).failures


class DirectoryFixtures(Base):
    def test_good_passes(self):
        report = cia.run_checks(self.tree())
        self.assertEqual(report.failures, [])

    def test_ok_variants_pass(self):
        for case in sorted(os.listdir(os.path.join(FIXTURES, "ok"))):
            with self.subTest(case=case):
                shutil.rmtree(os.path.join(self.tmp.name, "rootfs"), ignore_errors=True)
                tree = self.tree(os.path.join(FIXTURES, "ok", case))
                self.assertEqual(self.failures(tree), [])

    def test_every_bad_fixture_is_covered(self):
        on_disk = set(os.listdir(os.path.join(FIXTURES, "bad")))
        self.assertEqual(on_disk, set(BAD_CASES))

    def test_each_bad_fixture_fails_with_expected_message(self):
        for case, expected in sorted(BAD_CASES.items()):
            with self.subTest(case=case):
                shutil.rmtree(os.path.join(self.tmp.name, "rootfs"), ignore_errors=True)
                lines = self.failures(self.tree(os.path.join(FIXTURES, "bad", case)))
                self.assertTrue(lines, "no failure reported")
                for needle in expected:
                    self.assertTrue(
                        any(needle in line for line in lines),
                        "%r not in %r" % (needle, lines),
                    )

    def test_bad_fixture_fails_only_for_its_own_reason_area(self):
        # A single-cause fixture must not break unrelated checks.
        lines = self.failures(self.tree(os.path.join(FIXTURES, "bad", "pam_nullok")))
        self.assertEqual(len(lines), 1, lines)

    def test_private_key_marker_fails(self):
        marker = b"-----BEGIN OPENSSH " + b"PRIVATE KEY-----\nfake\n"
        extra = {"var/home/kiosk/.ssh/id_fixture": marker}
        lines = self.failures(self.tree(extra=extra))
        self.assertEqual(len(lines), 1, lines)
        self.assertIn("private key marker in /var/home/kiosk/.ssh/id_fixture", lines[0])

    def test_marker_outside_scan_dirs_is_ignored(self):
        marker = b"-----BEGIN RSA " + b"PRIVATE KEY-----\n"
        self.assertEqual(self.failures(self.tree(extra={"usr/lib/testdata.pem": marker})), [])

    def test_include_order_first_value_wins(self):
        # Later contradictory values after the hardening drop-in must not matter.
        extra = {"etc/ssh/sshd_config.d/30-late.conf": b"PermitRootLogin yes\n"}
        self.assertEqual(self.failures(self.tree(extra=extra)), [])

    def test_symlink_outside_root_is_not_followed(self):
        tree = self.tree()
        os.symlink("/etc/passwd", os.path.join(tree.root, "etc", "escape"))
        self.assertEqual(tree.read("/etc/escape"), tree.read("/etc/passwd"))
        outside = os.path.join(self.tmp.name, "outside.txt")
        with open(outside, "w", encoding="utf-8") as fh:
            fh.write("secret")
        os.symlink(outside, os.path.join(tree.root, "etc", "abs"))
        self.assertIsNone(tree.read("/etc/abs"))

    def test_ipk_dir(self):
        ipks = os.path.join(self.tmp.name, "ipk")
        os.makedirs(ipks)
        lines = self.failures(self.tree(), ipk_dir=ipks)
        self.assertEqual(len(lines), 3, lines)
        for name in ("sudo_1.9_arm64.ipk", "thing-revived-accounts_0.1_all.ipk",
                     "pam-plugin-faillock_1.7.2_arm64.ipk"):
            open(os.path.join(ipks, name), "w").close()
        self.assertEqual(self.failures(self.tree_again(), ipk_dir=ipks), [])

    def tree_again(self):
        shutil.rmtree(os.path.join(self.tmp.name, "rootfs"))
        return self.tree()


class CommandLine(Base):
    def run_cli(self, *args):
        return subprocess.run([sys.executable, TOOL, *args], capture_output=True, text=True)

    def test_good_exit_zero(self):
        res = self.run_cli(GOOD)
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)
        self.assertIn("OK:", res.stdout)

    def test_bad_exit_one_one_line_per_failure(self):
        dest = os.path.join(self.tmp.name, "rootfs")
        materialise(dest, os.path.join(FIXTURES, "bad", "pam_nullok"))
        res = self.run_cli(dest)
        self.assertEqual(res.returncode, 1)
        fails = [l for l in res.stdout.splitlines() if l.startswith("FAIL ")]
        self.assertEqual(len(fails), 1, res.stdout)

    def test_usage_error_exit_two(self):
        res = self.run_cli(os.path.join(self.tmp.name, "nope"))
        self.assertEqual(res.returncode, 2)
        self.assertIn("ERROR:", res.stderr)

    def test_build_key_option_is_rejected(self):
        res = self.run_cli(GOOD, "--build-key", os.path.join(self.tmp.name, "k.pub"))
        self.assertEqual(res.returncode, 2)
        self.assertIn("unrecognized arguments", res.stderr)


E2FS = all(_shutil.which(t) for t in ("mke2fs", "debugfs", "dumpe2fs", "sfdisk"))


@unittest.skipUnless(E2FS, "e2fsprogs and sfdisk are needed for image inputs")
class ImageInputs(Base):
    def make_ext4(self, case_dir=None, size_mib=16):
        rootfs = os.path.join(self.tmp.name, "rootfs")
        materialise(rootfs, case_dir)
        image = os.path.join(self.tmp.name, "root.ext4")
        subprocess.run(
            ["mke2fs", "-q", "-t", "ext4", "-b", "4096", "-d", rootfs, image, "%dM" % size_mib],
            check=True, capture_output=True,
        )
        return image

    def make_wic(self, ext4, name="root_a"):
        wic = os.path.join(self.tmp.name, "disk.wic")
        mib = 1 << 20
        root_size = os.path.getsize(ext4)
        with open(wic, "wb") as fh:
            fh.truncate(4 * mib + 2 * root_size + mib)
        script = (
            "label: gpt\n"
            "start=2048, size=%d, name=boot\n"
            "start=%d, size=%d, name=%s\n"
            "start=%d, size=%d, name=root_b\n"
            % (
                (4 * mib - 2048 * 512) // 512,
                4 * mib // 512, root_size // 512, name,
                (4 * mib + root_size) // 512, root_size // 512,
            )
        )
        subprocess.run(["sfdisk", "-q", wic], input=script, text=True, check=True,
                       capture_output=True)
        with open(wic, "r+b") as fh:
            fh.seek(4 * mib)
            with open(ext4, "rb") as src:
                fh.write(src.read())
        return wic

    def test_ext4_good(self):
        image = self.make_ext4()
        res = cia.run_checks(cia.Ext4Tree(image), min_free_mib=1)
        self.assertEqual(res.failures, [])

    def test_ext4_bad_fixture_matches_directory_result(self):
        case = os.path.join(FIXTURES, "bad", "unit_not_enabled")
        image = self.make_ext4(case)
        lines = self.failures(cia.Ext4Tree(image), min_free_mib=1)
        self.assertEqual(len(lines), 1, lines)
        self.assertIn("thing-revived-etc.service is not enabled", lines[0])

    def test_ext4_resolves_symlinks_inside_image(self):
        tree = cia.Ext4Tree(self.make_ext4())
        self.assertEqual(
            tree.resolve("/etc/systemd/system/sysinit.target.wants/thing-revived-etc.service"),
            "/usr/lib/systemd/system/thing-revived-etc.service",
        )
        self.assertIn(b"Requires=thing-revived-etc.service", tree.read(
            "/etc/systemd/system/sysinit.target.wants/thing-revived-provision.service"))

    def test_free_space_gate(self):
        image = self.make_ext4()
        tree = cia.Ext4Tree(image)
        self.assertEqual(self.failures(tree, min_free_mib=1), [])
        lines = self.failures(tree, min_free_mib=100000)
        self.assertEqual(len(lines), 1, lines)
        self.assertIn("root_a has", lines[0])
        self.assertIn("need at least 100000 MiB", lines[0])

    def test_wic_carves_root_a(self):
        wic = self.make_wic(self.make_ext4())
        res = subprocess.run(
            [sys.executable, TOOL, wic, "--min-free-mib", "1"],
            capture_output=True, text=True,
        )
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)

    def test_wic_without_root_a_is_usage_error(self):
        wic = self.make_wic(self.make_ext4(), name="other")
        res = subprocess.run([sys.executable, TOOL, wic], capture_output=True, text=True)
        self.assertEqual(res.returncode, 2)
        self.assertIn("no partition named 'root_a'", res.stderr)

    def test_wic_partition_index_fallback(self):
        wic = self.make_wic(self.make_ext4(), name="other")
        res = subprocess.run(
            [sys.executable, TOOL, wic, "--root-partition", "2", "--min-free-mib", "1"],
            capture_output=True, text=True,
        )
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)


if __name__ == "__main__":
    unittest.main()
