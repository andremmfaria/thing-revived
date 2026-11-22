import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(HERE, "toml_to_js.py")
PREFIX = "window.THING_CONFIG = "


def run(*args):
    return subprocess.run([sys.executable, TOOL, *args], capture_output=True, text=True)


def payload(stdout):
    """Parse the JSON object out of the generated script."""
    start = stdout.index(PREFIX) + len(PREFIX)
    assert stdout.endswith(";\n")
    return json.loads(stdout[start:-2])


class TomlToJsTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def write(self, text, name="in.toml"):
        path = os.path.join(self.tmp.name, name)
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text)
        return path

    def convert(self, text):
        res = run(self.write(text))
        self.assertEqual(res.returncode, 0, res.stderr)
        return res.stdout

    def test_header_and_shape(self):
        out = self.convert('a = 1\n')
        self.assertTrue(out.startswith("// GENERATED from config/frontend.toml"))
        self.assertIn("DO NOT EDIT", out.split(PREFIX)[0])
        self.assertTrue(out.endswith("};\n"))

    def test_nested_tables(self):
        out = self.convert('[ws]\nhost = "127.0.0.1"\n[ws.retry]\nmax = 5\n[[keys]]\nname = "a"\n[[keys]]\nname = "b"\n')
        self.assertEqual(
            payload(out),
            {"ws": {"host": "127.0.0.1", "retry": {"max": 5}}, "keys": [{"name": "a"}, {"name": "b"}]},
        )

    def test_arrays(self):
        out = self.convert('xs = [1, 2, 3]\nmixed = [["a"], ["b", "c"]]\nempty = []\n')
        self.assertEqual(payload(out), {"xs": [1, 2, 3], "mixed": [["a"], ["b", "c"]], "empty": []})

    def test_strings_quotes_unicode(self):
        out = self.convert('s = "say \\"hi\\" \\\\ \\n tab\\t"\nu = "caf\\u00e9 \\u2603 \\U0001F600"\nt = "</script><b>&"\n')
        data = payload(out)
        self.assertEqual(data["s"], 'say "hi" \\ \n tab\t')
        self.assertEqual(data["u"], "caf\u00e9 \u2603 \U0001F600")
        self.assertEqual(data["t"], "</script><b>&")
        # Output is pure ASCII and cannot close a script element.
        out.encode("ascii")
        self.assertNotIn("</script>", out)
        self.assertNotIn("<", out)

    def test_line_separators_escaped(self):
        out = self.convert('s = "a\\u2028b\\u2029c"\n')
        self.assertNotIn("\u2028", out)
        self.assertNotIn("\u2029", out)
        self.assertEqual(payload(out)["s"], "a\u2028b\u2029c")

    def test_numbers_and_booleans(self):
        out = self.convert('i = 42\nneg = -7\nf = 1.5\ne = 1e3\nyes = true\nno = false\n')
        data = payload(out)
        self.assertEqual(data, {"i": 42, "neg": -7, "f": 1.5, "e": 1000.0, "yes": True, "no": False})
        self.assertIs(data["yes"], True)
        self.assertIsInstance(data["i"], int)

    def test_non_finite_rejected(self):
        for bad in ("x = inf\n", "x = -inf\n", "x = nan\n", "x = [1.0, nan]\n"):
            res = run(self.write(bad))
            self.assertEqual(res.returncode, 1, bad)
            self.assertIn("non-finite", res.stderr)
            self.assertEqual(res.stdout, "")

    def test_datetime_becomes_string(self):
        data = payload(self.convert("d = 2024-01-02T03:04:05Z\n"))
        self.assertEqual(data["d"], "2024-01-02T03:04:05+00:00")

    def test_invalid_toml(self):
        path = self.write("this is = not = toml\n")
        res = run(path)
        self.assertNotEqual(res.returncode, 0)
        self.assertIn("invalid TOML", res.stderr)
        self.assertIn(path, res.stderr)
        self.assertEqual(res.stdout, "")

    def test_missing_file(self):
        res = run(os.path.join(self.tmp.name, "nope.toml"))
        self.assertEqual(res.returncode, 1)
        self.assertIn("cannot read", res.stderr)

    def test_no_output_file_on_error(self):
        out = os.path.join(self.tmp.name, "out.js")
        res = run(self.write("bad ="), "-o", out)
        self.assertEqual(res.returncode, 1)
        self.assertFalse(os.path.exists(out))

    def test_check_mode(self):
        good = self.write("a = 1\n", "good.toml")
        bad = self.write("a =\n", "bad.toml")
        self.assertEqual(run("--check", good).returncode, 0)
        res = run("--check", good, bad)
        self.assertEqual(res.returncode, 1)
        self.assertIn("bad.toml", res.stderr)

    def test_deterministic_and_sorted(self):
        src = self.write('z = 1\na = 2\n[m]\nq = 1\nb = 2\n')
        a = os.path.join(self.tmp.name, "a.js")
        b = os.path.join(self.tmp.name, "b.js")
        self.assertEqual(run(src, "-o", a).returncode, 0)
        self.assertEqual(run(src, "-o", b).returncode, 0)
        with open(a, "rb") as fa, open(b, "rb") as fb:
            ba, bb = fa.read(), fb.read()
        self.assertEqual(ba, bb)
        text = ba.decode("ascii")
        self.assertLess(text.index('"a"'), text.index('"m"'))
        self.assertLess(text.index('"m"'), text.index('"z"'))
        self.assertLess(text.index('"b"'), text.index('"q"'))

    def test_key_order_independent(self):
        one = self.convert("a = 1\nb = 2\n")
        two = self.convert("b = 2\na = 1\n")
        self.assertEqual(one, two)


if __name__ == "__main__":
    unittest.main()
