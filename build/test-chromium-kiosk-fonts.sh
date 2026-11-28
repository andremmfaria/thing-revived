#!/usr/bin/env bash
# Host check for the chromium-kiosk font drop (docs/research/packages-and-usr-overlay-plan.md
# section 2.4). Asserts the bbappend sits where bitbake will parse it once build-image.sh has
# copied build/overlay/ into the example layer, and that it removes both font packages from
# RDEPENDS. It does not run bitbake.
#
# Usage: build/test-chromium-kiosk-fonts.sh
set -u

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
overlay="$here/overlay"
dir="$overlay/recipes-graphics/chromium-kiosk"
file="$dir/chromium-kiosk_%.bbappend"
script="$here/build-image.sh"

status=0
pass() { echo "PASS [chromium-kiosk-fonts] $1"; }
fail() { echo "FAIL [chromium-kiosk-fonts] $1"; status=1; }

# Same relative path as the upstream recipe and the example layer's own bbappend, so the
# BBFILES pattern recipes-*/*/*.bbappend of the example layer matches it.
if [ -f "$file" ]; then
    pass "bbappend exists at build/overlay/recipes-graphics/chromium-kiosk/"
else
    fail "missing $file"
fi

# Active (non-comment) line only, so a mention in a comment does not satisfy the test.
line="$(grep -E '^[[:space:]]*RDEPENDS:\$\{PN\}:remove[[:space:]]*=' "$file" 2>/dev/null || true)"
if [ -n "$line" ]; then
    pass "RDEPENDS:\${PN}:remove assignment present"
else
    fail "no RDEPENDS:\${PN}:remove assignment"
fi
for pkg in noto-color-emoji noto-sans-cjk; do
    value="$(printf '%s\n' "$line" | sed -E 's/^[^"]*"([^"]*)".*$/\1/')"
    if printf '%s\n' "$value" | tr -s '[:space:]' '\n' | grep -qx -- "$pkg"; then
        pass "remove line names $pkg"
    else
        fail "remove line does not name $pkg"
    fi
done

# build-image.sh must copy the whole overlay tree (so the new directory rides along) into the
# example layer, and that layer must be where chromium-kiosk bbappends are already parsed.
if grep -qE '^[[:space:]]*cp -a "\$\{REPO_ROOT\}/build/overlay/\." "\$\{CHECKOUT\}/\$\{EXAMPLE_LAYER\}/"' "$script"; then
    pass "build-image.sh copies build/overlay/. into the example layer"
else
    fail "build-image.sh no longer copies build/overlay/. into the example layer"
fi
if grep -qE '^EXAMPLE_LAYER="examples/meta-superbird-kiosk-example"' "$script"; then
    pass "EXAMPLE_LAYER is examples/meta-superbird-kiosk-example"
else
    fail "EXAMPLE_LAYER changed"
fi

exit "$status"
