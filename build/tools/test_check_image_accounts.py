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
    # Fixture directory kept from the old "securetty has stray entries" case (fixtures/bad/** are
    # not renamed here since removing one is not an option, see the T-D1.3 return notes); its
    # content and expected message now match the current rule: ttyAML0 must always be listed.
    "securetty_entries": ["/etc/securetty does not list ttyAML0"],
    "uart_autologin": [
        "autologin in", "the shared serial-getty template must stay untouched in every build"
    ],
    "adb_gate": ["/etc/usb-debugging-enabled exists", "must not ship in any build"],
    "flags_missing": ["build-flags is missing"],
    "unit_missing": ["thing-revived-provision.service is not installed"],
    "unit_not_enabled": ["thing-revived-etc.service is not enabled"],
    "unit_no_install": ["thing-revived-etc.service has no [Install] WantedBy=sysinit.target"],
    "unit_requisite": ["uses Requisite=", "lacks Requires=thing-revived-etc.service"],
    "usb_bind_missing": ["thing-revived-usb-bind.service is not installed"],
    "usb_bind_enabled": [
        "thing-revived-usb-bind.service is enabled (/etc/systemd/system/multi-user.target.wants/"
        "thing-revived-usb-bind.service); it must ship disabled"
    ],
    "usb_bind_no_conflicts": ["thing-revived-usb-bind.service has no Conflicts=android-tools-adbd.service"],
    "usb_bind_no_condition": [
        "thing-revived-usb-bind.service has a ConditionPathExists on /etc/usb-debugging-enabled"
    ],
    "usb_bind_no_role_order": ["thing-revived-usb-bind.service lacks After=superbird-usb-role.service"],
    "adb_dropin_no_conflicts": [
        "android-tools-adbd.service.d/zz-thing-revived-toggle.conf has no Conflicts="
        "thing-revived-usb-bind.service"
    ],
    "adb_dropin_condition_present": [
        "android-tools-adbd.service.d/zz-thing-revived-toggle.conf has a ConditionPathExists on "
        "/etc/usb-debugging-enabled"
    ],
    "adb_unit_enabled": [
        "android-tools-adbd.service is enabled (/etc/systemd/system/basic.target.wants/"
        "android-tools-adbd.service); it must ship disabled"
    ],
    "adb_unit_missing": ["android-tools-adbd.service is not installed"],
    "adb_dropin_no_reset": [
        "android-tools-adbd.service.d/zz-thing-revived-toggle.conf does not reset ConditionPathExists="
    ],
    "uart_root_no_conflicts": [
        "thing-revived-uart-root@.service has no Conflicts=serial-getty@%i.service"
    ],
    "uart_root_placeholder": [
        "thing-revived-uart-root@.service still has an unresolved @@..@@ placeholder in ExecStart"
    ],
    "uart_root_enabled": [
        "a thing-revived-uart-root instance is enabled (/etc/systemd/system/getty.target.wants/"
        "thing-revived-uart-root@ttyAML0.service); it must ship disabled"
    ],
    "uart_root_missing": ["thing-revived-uart-root@.service is not installed"],
    "usb_role_dropin_missing": ["10-thing-revived-default-device.conf drop-in is not installed"],
    "usb_role_default_host": ["does not default to device unless boot-role is host"],
    "usb_role_marker_branch": ["still reads /etc/usb-debugging-enabled; it must read only boot-role"],
    # Rule 4: unit A.
    "device_a_missing": ["thing-revived-device-settings.service is not installed"],
    "device_a_not_enabled": ["thing-revived-device-settings.service is not enabled (no sysinit.target.wants symlink)"],
    "device_a_no_install": [
        "thing-revived-device-settings.service has no [Install] WantedBy=sysinit.target"
    ],
    "device_a_default_deps": ["thing-revived-device-settings.service lacks DefaultDependencies=no"],
    "device_a_no_before_role": ["thing-revived-device-settings.service lacks Before=superbird-usb-role.service"],
    "device_a_before_sshd": ["thing-revived-device-settings.service has Before=sshd.socket"],
    "device_a_exec_wrong": [
        "thing-revived-device-settings.service ExecStart is not exactly "
        "'/usr/bin/controller-agent apply-device-settings early'"
    ],
    "device_a_systemctl": ["thing-revived-device-settings.service runs systemctl from an Exec line"],
    # Rule 5: units B and F.
    "device_b_missing": ["thing-revived-device-apply.service is not installed"],
    "device_b_not_enabled": ["thing-revived-device-apply.service is not enabled (no multi-user.target.wants symlink)"],
    "device_b_no_onfailure": [
        "thing-revived-device-apply.service lacks OnFailure=thing-revived-device-failsafe.service"
    ],
    "device_b_before_adbd": ["thing-revived-device-apply.service has Before=android-tools-adbd.service"],
    "device_b_before_multi_user": ["thing-revived-device-apply.service has Before=multi-user.target"],
    "device_b_before_getty": ["thing-revived-device-apply.service has Before=serial-getty@ttyAML0.service"],
    "device_b_onfailure_raw": [
        "thing-revived-device-apply.service OnFailure= names the raw unit thing-revived-usb-bind.service"
    ],
    "device_f_missing": ["thing-revived-device-failsafe.service is not installed"],
    "device_f_before": ["thing-revived-device-failsafe.service has a Before= line"],
    "device_f_install": ["thing-revived-device-failsafe.service has an [Install] section"],
    "device_f_exec_wrong": [
        "thing-revived-device-failsafe.service ExecStart is not exactly "
        "'/usr/bin/controller-agent apply-device-settings failsafe late'"
    ],
    "device_f_onfailure_raw": [
        "thing-revived-device-failsafe.service OnFailure= names the raw unit serial-getty@ttyAML0.service"
    ],
    # Rule 6: sshd.socket and the ssh gate.
    "ssh_socket_missing": ["sshd.socket is not installed"],
    "ssh_socket_not_enabled": ["sshd.socket is not enabled"],
    "ssh_gate_missing": ["sshd.socket.d/20-thing-revived-ssh-gate.conf is not installed"],
    "ssh_gate_extra_key": ["sshd.socket.d/20-thing-revived-ssh-gate.conf must contain exactly After="],
    "ssh_gate_wrong_condition": ["sshd.socket.d/20-thing-revived-ssh-gate.conf must contain exactly After="],
    "ssh_service_enabled": [
        "sshd.service is enabled (/etc/systemd/system/multi-user.target.wants/sshd.service); "
        "a second listener would bypass ssh off"
    ],
    # Rule 7: the seed file.
    "seed_missing": ["/usr/share/thing-revived/device-seed.json is missing"],
    "seed_invalid_json": ["/usr/share/thing-revived/device-seed.json is not valid JSON"],
    "seed_not_object": ["/usr/share/thing-revived/device-seed.json is not a JSON object"],
    "seed_bad_version": ["device-seed.json version is 2, expected 1"],
    "seed_empty_id": ["device-seed.json image_id '' is not <yyyymmddThhmmssZ>-<8 hex or nogit>"],
    "seed_bad_id": ["device-seed.json image_id 'diag4' is not <yyyymmddThhmmssZ>-<8 hex or nogit>"],
    "seed_adb_mismatch": ["device-seed.json adb is true, expected false from --adb-default 0"],
    "seed_uart_mismatch": ["device-seed.json uart is true, expected false from --uart-default 0"],
    "seed_adb_not_bool": ["device-seed.json adb is 'false', expected a boolean"],
    # Rule 9: the persisted file must not be baked.
    "device_settings_baked": [
        "/var/lib/thing-revived/device-settings.json is baked into the image; it is created at first boot"
    ],
    "etc_upper_mode_0700": ["sets the upper dir to 0700, expected 0755"],
    "terminal_root": ["thing-revived-terminal-agent.service does not run with User=kiosk"],
}


# ok case -> run_checks kwargs (adb_default/uart_default), covering the four build-flag
# combinations as seed file expectations (no unit is enabled in any of them); cases not listed here
# use the all-defaults (0/0) combination, same as good/.
OK_CASES = {
    "adb_enabled": {"adb_default": True},
    "uart_enabled": {"uart_default": True},
    "adb_and_uart_enabled": {"adb_default": True, "uart_default": True},
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
                if os.path.islink(src):
                    # Enablement symlinks in an overlay point at absolute in-image paths (e.g.
                    # /usr/lib/systemd/system/...) that do not exist on the host, so they must be
                    # relinked rather than followed (shutil.copy2 dereferences by default).
                    if os.path.lexists(target):
                        os.remove(target)
                    os.symlink(os.readlink(src), target)
                else:
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
                self.assertEqual(self.failures(tree, **OK_CASES.get(case, {})), [])

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

    def test_device_settings_fixtures_fail_exactly_once(self):
        # Each fixture of the device settings rules (plan 7.2) breaks exactly one rule: a second
        # failure line would mean two rules overlap on it.
        prefixes = (
            "device_", "seed_", "ssh_", "adb_unit_", "adb_dropin_no_reset", "usb_bind_enabled",
            "uart_root_enabled", "uart_root_missing", "usb_role_marker_branch",
        )
        for case in sorted(c for c in BAD_CASES if c.startswith(prefixes)):
            with self.subTest(case=case):
                shutil.rmtree(os.path.join(self.tmp.name, "rootfs"), ignore_errors=True)
                lines = self.failures(self.tree(os.path.join(FIXTURES, "bad", case)))
                self.assertEqual(len(lines), 1, lines)

    def test_seed_expectations_come_from_the_options(self):
        # The seed file is the only place the flags show up: good/ seeds 0/0, ok/ cases seed 1.
        lines = self.failures(self.tree(), adb_default=True)
        self.assertEqual(len(lines), 1, lines)
        self.assertIn("adb is false, expected true from --adb-default 1", lines[0])
        shutil.rmtree(os.path.join(self.tmp.name, "rootfs"))
        lines = self.failures(self.tree(), uart_default=True)
        self.assertEqual(len(lines), 1, lines)
        self.assertIn("uart is false, expected true from --uart-default 1", lines[0])
        shutil.rmtree(os.path.join(self.tmp.name, "rootfs"))
        tree = self.tree(os.path.join(FIXTURES, "ok", "adb_enabled"))
        lines = self.failures(tree)
        self.assertEqual(len(lines), 1, lines)
        self.assertIn("adb is true, expected false from --adb-default 0", lines[0])

    def test_marker_fails_in_every_build_flavour(self):
        # Rule 8: "exists iff flag is 1" is gone, the marker fails with the flag on as well.
        shutil.rmtree(os.path.join(self.tmp.name, "rootfs"), ignore_errors=True)
        tree = self.tree(os.path.join(FIXTURES, "ok", "adb_enabled"), extra={"etc/usb-debugging-enabled": b""})
        lines = self.failures(tree, adb_default=True)
        self.assertEqual(len(lines), 1, lines)
        self.assertIn("/etc/usb-debugging-enabled exists", lines[0])

    def test_any_wants_directory_counts_as_enabled(self):
        # Rule 1: "anywhere under /etc/systemd/system/*.wants", also a vendor wants dir.
        for rel in (
            "etc/systemd/system/graphical.target.wants/android-tools-adbd.service",
            "usr/lib/systemd/system/multi-user.target.wants/thing-revived-usb-bind.service",
            "etc/systemd/system/getty.target.requires/thing-revived-uart-root@ttyAML0.service",
        ):
            with self.subTest(link=rel):
                shutil.rmtree(os.path.join(self.tmp.name, "rootfs"), ignore_errors=True)
                tree = self.tree()
                link = os.path.join(tree.root, rel)
                os.makedirs(os.path.dirname(link), exist_ok=True)
                os.symlink("/usr/lib/systemd/system/" + os.path.basename(rel).split("@")[0] + ".service", link)
                lines = self.failures(tree)
                self.assertEqual(len(lines), 1, lines)
                self.assertIn("must ship disabled", lines[0])

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


class BbappendEnableRemoval(Base):
    """thing_revived_device_seed must strip adbd, usb-bind and uart-root enable links from every
    *.wants / *.requires directory of every systemd unit tree, and thing_revived_assert_accounts
    must catch one that is left. The two shell functions are run for real (bitbake variables
    become exported shell variables) against a scratch copy of the good fixture."""

    BBAPPEND = os.path.join(
        HERE, os.pardir, "overlay", "recipes-core", "images", "superbird-kiosk-prod-image.bbappend"
    )
    LINKS = [
        "etc/systemd/system/basic.target.wants/android-tools-adbd.service",
        "etc/systemd/system/multi-user.target.wants/android-tools-adbd.service",
        "etc/systemd/system/sysinit.target.wants/thing-revived-usb-bind.service",
        "etc/systemd/system/getty.target.wants/thing-revived-uart-root@ttyAML0.service",
        "usr/lib/systemd/system/basic.target.wants/android-tools-adbd.service",
        "usr/lib/systemd/system/local-fs.target.requires/thing-revived-usb-bind.service",
        "lib/systemd/system/basic.target.wants/android-tools-adbd.service",
    ]

    def function_text(self, name):
        with open(self.BBAPPEND) as handle:
            lines = handle.read().splitlines()
        start = lines.index(name + " () {")
        end = lines.index("}", start)
        return "\n".join(lines[start : end + 1])

    def run_functions(self, root, *names):
        script = "\n".join(
            [
                "bbfatal() { echo \"bbfatal: $*\" >&2; exit 1; }",
                "bbwarn() { :; }",
                "export IMAGE_ROOTFS=%s sysconfdir=/etc systemd_system_unitdir=/usr/lib/systemd/system" % root,
                "export datadir=/usr/share THING_REVIVED_ENABLE_ADB=0 THING_REVIVED_ENABLE_UART=0",
                "export THING_REVIVED_IMAGE_ID=20260101T000000Z-deadbeef",
            ]
            + [self.function_text(n) for n in names]
            + list(names)
        )
        return subprocess.run(["bash", "-c", script], capture_output=True, text=True)

    def scratch(self, links):
        root = os.path.join(self.tmp.name, "bbappend-rootfs")
        shutil.rmtree(root, ignore_errors=True)
        shutil.copytree(GOOD, root, symlinks=True)
        for link in links:
            path = os.path.join(root, link)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            os.symlink("/usr/lib/systemd/system/" + os.path.basename(link), path)
        return root

    def test_functions_are_valid_shell(self):
        for name in ("thing_revived_device_seed", "thing_revived_assert_accounts"):
            res = subprocess.run(["bash", "-n"], input=self.function_text(name), capture_output=True, text=True)
            self.assertEqual(res.returncode, 0, res.stderr)

    def test_removes_links_from_every_wants_dir(self):
        root = self.scratch(self.LINKS)
        res = self.run_functions(root, "thing_revived_device_seed")
        self.assertEqual(res.returncode, 0, res.stderr)
        for link in self.LINKS:
            self.assertFalse(os.path.lexists(os.path.join(root, link)), link)
        self.assertEqual(self.failures(cia.DirTree(root)), [])
        # enable links that must stay are untouched
        self.assertTrue(os.path.lexists(os.path.join(root, "etc/systemd/system/sockets.target.wants/sshd.socket")))

    def test_assertion_catches_each_leftover_link(self):
        for link in self.LINKS:
            with self.subTest(link=link):
                root = self.scratch([])
                self.assertEqual(self.run_functions(root, "thing_revived_device_seed").returncode, 0)
                path = os.path.join(root, link)
                os.makedirs(os.path.dirname(path), exist_ok=True)
                os.symlink("/x", path)
                res = self.run_functions(root, "thing_revived_assert_accounts")
                self.assertEqual(res.returncode, 1, res.stdout + res.stderr)
                self.assertIn("must ship disabled are enabled", res.stderr)


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

    def test_seed_options_are_checked_against_the_seed_file(self):
        res = self.run_cli(GOOD, "--adb-default", "1")
        self.assertEqual(res.returncode, 1, res.stdout)
        self.assertIn("adb is false, expected true from --adb-default 1", res.stdout)
        res = self.run_cli(GOOD, "--adb-default", "0", "--uart-default", "0")
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)

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
