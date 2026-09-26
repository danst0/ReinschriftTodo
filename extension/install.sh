#!/usr/bin/env bash
# Build the Reinschrift GNOME Shell extension package and install it for the
# current user. The same zip (build/reinschrift@dumke.me.shell-extension.zip)
# is what gets uploaded to extensions.gnome.org.
#
#   ./install.sh           build + install + enable
#   ./install.sh --pack    build only
set -euo pipefail

UUID="reinschrift@dumke.me"
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="$SRC/build"
ZIP="$OUT/$UUID.shell-extension.zip"

if ! command -v gnome-extensions >/dev/null 2>&1; then
    echo "gnome-extensions not found — this applet only runs on GNOME." >&2
    exit 1
fi

mkdir -p "$OUT"
# pack compiles po/*.po into locale/ and leaves out install.sh and tests/.
gnome-extensions pack "$SRC" \
    --force \
    --out-dir="$OUT" \
    --podir=po \
    --extra-source=lib \
    --extra-source=icons
echo "Built $ZIP"

if [[ "${1:-}" == "--pack" ]]; then
    exit 0
fi

gnome-extensions install --force "$ZIP"
echo "Installed $UUID"

gnome-extensions enable "$UUID" 2>/dev/null && echo "Extension enabled." || {
    echo "Enable it after your next login with:"
    echo "  gnome-extensions enable $UUID"
}
echo
echo "Note: GNOME Shell loads extension code at login — log out and back in"
echo "(Wayland) to run the new version."
echo
echo "Run parity tests with: gjs -m $SRC/tests/run.js"
