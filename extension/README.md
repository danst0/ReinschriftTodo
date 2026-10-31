# Reinschrift GNOME Shell extension

Your [Reinschrift](../README.md) "My Day" in the GNOME top bar: see the tasks planned for today, tick them off, open their details, postpone them (Tomorrow, Weekend, Next Week, Someday) or take them off My Day, and add new tasks — without opening the app. It reads and writes the same Markdown file as the app. Supports GNOME 48–51.

## Install

The extension is pending review on extensions.gnome.org. Until then, install it from this repository:

```bash
git clone https://github.com/danst0/ReinschriftTodo
ReinschriftTodo/extension/install.sh
```

`install.sh` builds the package, installs it for the current user and tries to enable it. GNOME Shell only picks up a new extension after a fresh login on Wayland: log out and back in, then run

```bash
gnome-extensions enable reinschrift@dumke.me
```

or switch it on in the *Extensions* app. To update, pull the repository and run `install.sh` again (and log in again). `install.sh --pack` only builds the ZIP (`extension/build/reinschrift@dumke.me.shell-extension.zip`).

Requirements: `gnome-extensions` and `gettext` (`msgfmt`); the desktop app itself is optional.

## Which file it uses

No setup is needed when the desktop app is installed. The extension picks the first of:

1. `TODOS_DB_PATH`, if set in GNOME Shell's environment,
2. the file chosen in the extension's preferences (*To-do File → Markdown File*; *Use the Reinschrift App’s File* goes back to automatic),
3. the file the desktop app is set to — the Flatpak's settings first, then a native install's,
4. the app's default file `~/.local/share/reinschrift_todo/todos.md`.

When the app syncs via WebDAV, the extension does not talk to the server; it reads the Nextcloud desktop client's copy at `~/Nextcloud/<path>` (the app's *Path (relative)*), so the menu stays instant and works offline. If your synced folder lives elsewhere, or you only use the web app, choose the file in the preferences.

## Development

```bash
gjs -m extension/tests/run.js     # parity tests of the line format against the Rust core
```

Translations live in `extension/po/` (gettext domain `reinschrift-shell`) and are compiled by `install.sh`.
