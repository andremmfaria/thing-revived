#!/usr/bin/env python3
"""Convert a TOML file to a classic browser script: window.THING_CONFIG = {...};

The kiosk browser (Chromium 69, file://, no build step) cannot read TOML, so
build-image.sh runs this on config/frontend.toml and installs the result as
config.generated.js next to the web app. Output is deterministic (sorted keys,
fixed indentation, ASCII only) so repeated builds are byte-identical.

Usage:
  toml_to_js.py INPUT.toml [-o OUTPUT.js]   write the script (stdout if no -o)
  toml_to_js.py --check INPUT.toml [...]    only validate that files parse

Exit codes: 0 ok, 1 invalid input (parse error, non-finite number, I/O error),
2 usage error.
"""

import argparse
import datetime
import json
import math
import sys
import tomllib

HEADER = (
    "// GENERATED from config/frontend.toml by build/tools/toml_to_js.py.\n"
    "// DO NOT EDIT: change the TOML file and rebuild the image.\n"
)


class ConfigError(Exception):
    pass


def _normalise(value, path="$"):
    """Make a tomllib result JSON-safe; reject non-finite floats."""
    if isinstance(value, dict):
        return {k: _normalise(v, f"{path}.{k}") for k, v in value.items()}
    if isinstance(value, list):
        return [_normalise(v, f"{path}[{i}]") for i, v in enumerate(value)]
    if isinstance(value, float) and not math.isfinite(value):
        raise ConfigError(f"{path}: non-finite number ({value}) cannot be represented in JSON")
    if isinstance(value, (datetime.datetime, datetime.date, datetime.time)):
        return value.isoformat()
    return value


def load_toml(path):
    try:
        with open(path, "rb") as fh:
            data = tomllib.load(fh)
    except OSError as exc:
        raise ConfigError(f"{path}: cannot read: {exc.strerror or exc}") from exc
    except tomllib.TOMLDecodeError as exc:
        raise ConfigError(f"{path}: invalid TOML: {exc}") from exc
    return _normalise(data)


def render(data):
    body = json.dumps(
        data, sort_keys=True, indent=2, ensure_ascii=True, allow_nan=False, separators=(",", ": ")
    )
    # Keep the payload inert if it is ever inlined into HTML.
    body = body.replace("<", "\\u003c").replace(">", "\\u003e").replace("&", "\\u0026")
    return f"{HEADER}window.THING_CONFIG = {body};\n"


def main(argv=None):
    parser = argparse.ArgumentParser(description=(__doc__ or "").split("\n\n")[0])
    parser.add_argument("inputs", nargs="+", metavar="INPUT.toml")
    parser.add_argument("-o", "--output", help="output file (default: stdout)")
    parser.add_argument("--check", action="store_true", help="only validate that the inputs parse")
    args = parser.parse_args(argv)

    if not args.check and len(args.inputs) != 1:
        parser.error("exactly one input is required unless --check is used")

    try:
        if args.check:
            for path in args.inputs:
                load_toml(path)
            return 0
        text = render(load_toml(args.inputs[0]))
    except ConfigError as exc:
        print(f"toml_to_js: error: {exc}", file=sys.stderr)
        return 1

    if args.output:
        try:
            with open(args.output, "w", encoding="ascii", newline="\n") as fh:
                fh.write(text)
        except OSError as exc:
            print(f"toml_to_js: error: {args.output}: {exc.strerror or exc}", file=sys.stderr)
            return 1
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
