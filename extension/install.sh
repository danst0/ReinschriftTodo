#!/usr/bin/env bash
# Install the Reinschrift GNOME Shell menu applet for the current user.
set -euo pipefail

UUID="reinschrift@dumke.me"
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/$UUID"

if ! command -v gnome-shell >/dev/null 2>&1; then
    echo "gnome-shell not found — this applet only runs on GNOME." >&2
    exit 1
fi

mkdir -p "$DEST"
rsync -a --delete \
    --exclude 'install.sh' \
    --exclude 'tests/' \
    --exclude '.git/' \
    "$SRC/" "$DEST/"

echo "Installed to $DEST"

if command -v gnome-extensions >/dev/null 2>&1; then
    gnome-extensions enable "$UUID" 2>/dev/null && echo "Extension enabled." || {
        echo "Could not enable automatically. Enable it manually:"
        echo "  gnome-extensions enable $UUID"
    }
    echo
    echo "Note: on Wayland, log out and back in (or restart GNOME Shell on X11)"
    echo "if the applet does not appear in the top bar right away."
else
    echo "Enable it after your next login with:"
    echo "  gnome-extensions enable $UUID"
fi

if command -v gjs >/dev/null 2>&1; then
    echo
    echo "Run parity tests with: gjs -m $SRC/tests/run.js"
fi
