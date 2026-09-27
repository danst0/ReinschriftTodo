import Adw from 'gi://Adw';
import Gio from 'gi://Gio';
import Gtk from 'gi://Gtk';

import {ExtensionPreferences, gettext as _} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

import {displayPath, readAppPrefs, resolveDbPath} from './lib/paths.js';

Gio._promisify(Gtk.FileDialog.prototype, 'open', 'open_finish');

export default class ReinschriftPreferences extends ExtensionPreferences {
    fillPreferencesWindow(window) {
        const settings = this.getSettings();
        // Held by the window so the settings object outlives this function.
        window._settings = settings;

        const page = new Adw.PreferencesPage();
        const group = new Adw.PreferencesGroup({
            title: _('To-do File'),
            description: _('Without a choice, the extension uses the file set in the Reinschrift app.'),
        });
        page.add(group);

        const row = new Adw.ActionRow({
            title: _('Markdown File'),
            subtitle_selectable: true,
        });
        group.add(row);

        const reset = new Gtk.Button({
            icon_name: 'edit-clear-symbolic',
            tooltip_text: _('Use the Reinschrift App’s File'),
            valign: Gtk.Align.CENTER,
            css_classes: ['flat'],
        });
        reset.connect('clicked', () => settings.reset('todo-file'));
        row.add_suffix(reset);

        const choose = new Gtk.Button({
            label: _('Choose…'),
            valign: Gtk.Align.CENTER,
        });
        choose.connect('clicked', () => this._choose(window, settings));
        row.add_suffix(choose);

        let generation = 0;
        const update = async () => {
            const chosen = settings.get_string('todo-file');
            reset.visible = chosen !== '';
            if (chosen) {
                row.subtitle = displayPath(chosen);
                return;
            }
            // Show which file "automatic" currently means.
            const current = ++generation;
            row.subtitle = _('Automatic');
            try {
                const [, candidates] = await readAppPrefs(null);
                const path = await resolveDbPath('', candidates, null);
                if (current === generation) {
                    // Translators: {path} is the file the Reinschrift app points to.
                    row.subtitle = _('Automatic: {path}').replace('{path}', displayPath(path));
                }
            } catch (e) {
                logError(e, 'Reinschrift: resolving the to-do file failed');
            }
        };
        settings.connect('changed::todo-file', () => update());
        update();

        window.add(page);
    }

    async _choose(window, settings) {
        const markdown = new Gtk.FileFilter({name: _('Markdown Files')});
        markdown.add_suffix('md');
        markdown.add_mime_type('text/markdown');
        const all = new Gtk.FileFilter({name: _('All Files')});
        all.add_pattern('*');
        const filters = new Gio.ListStore({item_type: Gtk.FileFilter});
        filters.append(markdown);
        filters.append(all);

        const dialog = new Gtk.FileDialog({
            title: _('Choose To-do File'),
            modal: true,
            filters,
            default_filter: markdown,
        });
        const current = settings.get_string('todo-file');
        if (current)
            dialog.initial_file = Gio.File.new_for_path(current);

        let file;
        try {
            file = await dialog.open(window, null);
        } catch (e) {
            if (!e.matches?.(Gtk.DialogError, Gtk.DialogError.DISMISSED))
                logError(e, 'Reinschrift: choosing the to-do file failed');
            return;
        }
        const path = file?.get_path();
        if (path)
            settings.set_string('todo-file', path);
    }
}
