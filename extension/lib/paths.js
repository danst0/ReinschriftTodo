// Where the to-do file lives. Shared by the extension and its preferences
// window, which run in different processes; only Gio/GLib here.

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

Gio._promisify(Gio.File.prototype, 'load_contents_async');
Gio._promisify(Gio.File.prototype, 'query_info_async');

export function isCancelled(e) {
    return e instanceof GLib.Error &&
        e.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED);
}

export async function readText(path, cancellable) {
    const [bytes] = await Gio.File.new_for_path(path).load_contents_async(cancellable);
    return new TextDecoder().decode(bytes);
}

async function exists(path, cancellable) {
    try {
        await Gio.File.new_for_path(path).query_info_async('standard::type',
            Gio.FileQueryInfoFlags.NONE, GLib.PRIORITY_DEFAULT, cancellable);
        return true;
    } catch (e) {
        if (isCancelled(e))
            throw e;
        return false;
    }
}

function prefsFiles() {
    const config = GLib.getenv('XDG_CONFIG_HOME') ||
        GLib.build_filenamev([GLib.get_home_dir(), '.config']);
    // Native install and Flatpak install each keep their own config.
    return [
        GLib.build_filenamev([config, 'reinschrift_todo', 'preferences.json']),
        GLib.build_filenamev([GLib.get_home_dir(), '.var', 'app',
            'me.dumke.Reinschrift', 'config', 'reinschrift_todo', 'preferences.json']),
    ];
}

/**
 * The app's preference files merged (last file wins per key) and the to-do
 * files they point to.
 */
export async function readAppPrefs(cancellable) {
    // The Flatpak config is the one the app actually uses when both exist.
    let merged = {};
    const candidates = [];
    for (const path of prefsFiles()) {
        let prefs;
        try {
            prefs = JSON.parse(await readText(path, cancellable));
        } catch (e) {
            if (isCancelled(e))
                throw e;
            continue;
        }
        merged = {...merged, ...prefs};
        if (prefs.db_path)
            candidates.push(prefs.db_path);
        if (prefs.use_webdav && prefs.webdav_path) {
            // WebDAV backend: use the Nextcloud sync mirror of the
            // remote file so the menu stays instant and offline-capable.
            candidates.push(GLib.build_filenamev(
                [GLib.get_home_dir(), 'Nextcloud', prefs.webdav_path]));
        }
    }
    return [merged, candidates];
}

/**
 * The to-do file to use: TODOS_DB_PATH, then the file chosen in the
 * extension's preferences, then the first existing file the app points to,
 * then the app's default location.
 */
export async function resolveDbPath(chosen, candidates, cancellable) {
    const env = GLib.getenv('TODOS_DB_PATH');
    if (env)
        return env;
    if (chosen)
        return chosen;

    candidates = [...candidates, GLib.build_filenamev(
        [GLib.get_user_data_dir(), 'reinschrift_todo', 'todos.md'])];
    for (const candidate of candidates) {
        if (await exists(candidate, cancellable))
            return candidate;
    }
    return candidates[candidates.length - 1];
}

/** A path with the home directory shortened to ~. */
export function displayPath(path) {
    const home = GLib.get_home_dir();
    path ??= '';
    return path.startsWith(`${home}/`) ? `~${path.slice(home.length)}` : path;
}
