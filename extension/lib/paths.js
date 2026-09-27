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

/** The app's preference files, the one it uses when both exist first. */
function prefsFiles() {
    const config = GLib.getenv('XDG_CONFIG_HOME') ||
        GLib.build_filenamev([GLib.get_home_dir(), '.config']);
    // Flatpak and native install each keep their own config; the Flatpak
    // one is what the published app reads.
    return [
        GLib.build_filenamev([GLib.get_home_dir(), '.var', 'app',
            'me.dumke.Reinschrift', 'config', 'reinschrift_todo', 'preferences.json']),
        GLib.build_filenamev([config, 'reinschrift_todo', 'preferences.json']),
    ];
}

/** The to-do file one app config points to, or null. */
function configuredFile(prefs) {
    if (prefs.use_webdav) {
        // The app then ignores db_path and talks to the server; read the
        // Nextcloud sync mirror of that file so the menu stays instant
        // and offline-capable.
        return prefs.webdav_path
            ? GLib.build_filenamev([GLib.get_home_dir(), 'Nextcloud', prefs.webdav_path])
            : null;
    }
    return prefs.db_path || null;
}

/**
 * The app's preferences (the preferred config wins per key) and the to-do
 * files they point to, in order of preference.
 */
export async function readAppPrefs(cancellable) {
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
        merged = {...prefs, ...merged};
        const file = configuredFile(prefs);
        if (file)
            candidates.push(file);
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
