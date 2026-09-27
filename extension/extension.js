import GObject from 'gi://GObject';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Shell from 'gi://Shell';
import St from 'gi://St';
import Clutter from 'gi://Clutter';
import Pango from 'gi://Pango';

import {Extension, gettext as _, ngettext} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {ensureActorVisibleInScrollView} from 'resource:///org/gnome/shell/misc/animationUtils.js';

import * as Format from './lib/format.js';
import {displayPath, isCancelled, readAppPrefs, readText, resolveDbPath} from './lib/paths.js';

const APP_ID = 'me.dumke.Reinschrift.desktop';

/** How long a freshly ticked row stays in place before the list re-sorts. */
const SETTLE_MS = 450;

Gio._promisify(Gio.File.prototype, 'replace_contents_bytes_async', 'replace_contents_finish');

// ---------------------------------------------------------------- helpers

function ymd(d) {
    const p = n => String(n).padStart(2, '0');
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

function startOfDay(d) {
    return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}

function isSomeday(d) {
    return d && d.getFullYear() === 9999;
}

function isMyDay(item) {
    return item.myday !== null && ymd(item.myday) === ymd(new Date());
}

function toDateTime(d) {
    return GLib.DateTime.new_local(d.getFullYear(), d.getMonth() + 1,
        d.getDate(), d.getHours(), d.getMinutes(), 0);
}

/** Human due label plus its urgency class, relative to today. */
function describeDue(due) {
    if (isSomeday(due))
        return [_('Someday'), 'rs-due-later'];

    const days = Math.round(
        (startOfDay(due).getTime() - startOfDay(new Date()).getTime()) / 86400000);
    const hasTime = due.getHours() !== 0 || due.getMinutes() !== 0;
    const time = hasTime ? ` ${toDateTime(due).format('%H:%M')}` : '';

    if (days < -1) {
        const text = ngettext('{n} day overdue', '{n} days overdue', -days)
            .replace('{n}', String(-days));
        return [text, 'rs-due-overdue'];
    }
    if (days === -1)
        return [_('Yesterday') + time, 'rs-due-overdue'];
    if (days === 0)
        return [_('Today') + time, 'rs-due-today'];
    if (days === 1)
        return [_('Tomorrow') + time, 'rs-due-later'];
    // Translators: short due date within the next weeks, a GLib.DateTime
    // format (like strftime); e.g. "%-d. %b" for German.
    // xgettext:no-javascript-format
    const fmt = days < 7 ? '%A' : _('%b %-d');
    return [toDateTime(due).format(fmt).trim() + time, 'rs-due-later'];
}

function sortKey(item, mode) {
    if (mode === 'topic')
        return (item.projects[0] ?? '￿').toLowerCase();
    if (mode === 'location')
        return (item.contexts[0] ?? '￿').toLowerCase();
    return item.due ? Format.formatDue(item.due) : '￿';
}

function markUpTitle(text, done) {
    const escaped = GLib.markup_escape_text(text, -1);
    return done ? `<s>${escaped}</s>` : escaped;
}

// ---------------------------------------------------------------- widgets

const TodoRow = GObject.registerClass(
class TodoRow extends PopupMenu.PopupBaseMenuItem {
    _init(item, onToggle) {
        super._init({style_class: 'rs-row'});

        this._item = item;
        this._onToggle = onToggle;

        this._checkbox = new St.Bin({
            style_class: 'rs-checkbox',
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._check = new St.Icon({
            icon_name: 'object-select-symbolic',
            style_class: 'rs-check',
        });
        this._checkbox.set_child(this._check);
        this.add_child(this._checkbox);

        const box = new St.BoxLayout({
            vertical: true,
            x_expand: true,
            y_align: Clutter.ActorAlign.CENTER,
            style_class: 'rs-text',
        });

        this._title = new St.Label({style_class: 'rs-title', x_expand: true});
        this._title.clutter_text.ellipsize = Pango.EllipsizeMode.END;
        box.add_child(this._title);

        const meta = [];
        for (const project of item.projects ?? [])
            meta.push([`+${project}`, 'rs-meta-tag']);
        for (const context of item.contexts ?? [])
            meta.push([`@${context}`, 'rs-meta-tag']);
        if (item.due && !item.done)
            meta.push(describeDue(item.due));
        if (item.recurrence)
            meta.push([`↻ ${item.recurrence}`, 'rs-meta-tag']);

        if (meta.length) {
            const line = new St.BoxLayout({style_class: 'rs-meta'});
            meta.forEach(([text, cls], i) => {
                if (i > 0)
                    line.add_child(new St.Label({text: '·', style_class: 'rs-sep'}));
                line.add_child(new St.Label({text, style_class: cls}));
            });
            box.add_child(line);
        }

        this.add_child(box);
        this._setDone(item.done);
    }

    _setDone(done) {
        this._check.visible = done;
        if (done) {
            this._checkbox.add_style_class_name('rs-checkbox-done');
            this.add_style_class_name('rs-row-done');
        } else {
            this._checkbox.remove_style_class_name('rs-checkbox-done');
            this.remove_style_class_name('rs-row-done');
        }
        this._title.clutter_text.set_markup(markUpTitle(this._item.title, done));
    }

    // Ticking off keeps the menu open so several tasks can be done in a row.
    activate(_event) {
        this._setDone(!this._item.done);
        this._onToggle(this);
    }

    get item() {
        return this._item;
    }
});

/** Title, date and progress. Not activatable. */
const HeaderItem = GObject.registerClass(
class HeaderItem extends PopupMenu.PopupBaseMenuItem {
    _init(done, total) {
        super._init({
            style_class: 'rs-header',
            // Non-reactive items get the shell's dimmed insensitive color;
            // stay reactive but inert instead.
            activate: false,
            hover: false,
            can_focus: false,
        });
        this.track_hover = false;

        const column = new St.BoxLayout({vertical: true, x_expand: true});

        const top = new St.BoxLayout({style_class: 'rs-header-top'});
        const text = new St.BoxLayout({vertical: true, x_expand: true});
        text.add_child(new St.Label({text: _('My Day'), style_class: 'rs-header-title'}));
        // Translators: today's date in the menu header, a GLib.DateTime
        // format (like strftime); e.g. "%A, %-d. %B" for German.
        // xgettext:no-javascript-format
        const date = GLib.DateTime.new_now_local().format(_('%A, %B %-d'));
        // Translators: progress in the menu header, e.g. "4 of 13 done".
        const progress = _('{done} of {total} done')
            .replace('{done}', String(done))
            .replace('{total}', String(total));
        const subtitle = total > 0 ? `${date} · ${progress}` : date;
        text.add_child(new St.Label({text: subtitle, style_class: 'rs-header-subtitle'}));
        top.add_child(text);
        column.add_child(top);

        if (total > 0) {
            const track = new St.BoxLayout({
                style_class: 'rs-progress',
                x_expand: true,
            });
            const fill = new St.Widget({
                style_class: 'rs-progress-fill',
                y_expand: true,
            });
            track.add_child(fill);
            track.connect('notify::width', () => {
                fill.width = Math.round(track.width * done / total);
            });
            column.add_child(track);
        }

        this.add_child(column);
    }
});

/** Entry for a new task; it lands in today's plan, like adding in the app's "My Day". */
const AddItem = GObject.registerClass(
class AddItem extends PopupMenu.PopupBaseMenuItem {
    _init(draft, onSubmit, onChange) {
        super._init({
            style_class: 'rs-add',
            activate: false,
            hover: false,
            can_focus: false,
        });
        this.track_hover = false;

        this.entry = new St.Entry({
            style_class: 'rs-add-entry',
            hint_text: _('Add a task'),
            primary_icon: new St.Icon({
                icon_name: 'list-add-symbolic',
                style_class: 'rs-add-icon',
            }),
            can_focus: true,
            x_expand: true,
        });
        this.entry.text = draft;
        this.entry.clutter_text.connect('activate', () => onSubmit(this.entry.text));
        this.entry.clutter_text.connect('text-changed', () => onChange(this.entry.text));
        this.add_child(this.entry);
    }
});

/** Expander for the completed tasks; toggles without closing the menu. */
const CompletedToggle = GObject.registerClass(
class CompletedToggle extends PopupMenu.PopupBaseMenuItem {
    _init(count, expanded, onToggle) {
        super._init({style_class: 'rs-completed-toggle'});
        this._onToggle = onToggle;

        this.add_child(new St.Icon({
            icon_name: expanded ? 'pan-down-symbolic' : 'pan-end-symbolic',
            style_class: 'rs-expander',
        }));
        this.add_child(new St.Label({
            text: _('Completed'),
            style_class: 'rs-completed-label',
            y_align: Clutter.ActorAlign.CENTER,
        }));
        this.add_child(new St.Label({
            text: String(count),
            style_class: 'rs-count',
            y_align: Clutter.ActorAlign.CENTER,
        }));
    }

    activate(_event) {
        this._onToggle();
    }
});

/** Centered icon + message for empty and error states. */
const StateItem = GObject.registerClass(
class StateItem extends PopupMenu.PopupBaseMenuItem {
    _init(iconName, title, hint) {
        super._init({
            style_class: 'rs-state',
            activate: false,
            hover: false,
            can_focus: false,
        });
        this.track_hover = false;
        const box = new St.BoxLayout({
            vertical: true,
            x_expand: true,
            x_align: Clutter.ActorAlign.CENTER,
            style_class: 'rs-state-box',
        });
        box.add_child(new St.Icon({
            icon_name: iconName,
            style_class: 'rs-state-icon',
            x_align: Clutter.ActorAlign.CENTER,
        }));
        box.add_child(new St.Label({
            text: title,
            style_class: 'rs-state-title',
            x_align: Clutter.ActorAlign.CENTER,
        }));
        if (hint) {
            const label = new St.Label({
                text: hint,
                style_class: 'rs-state-hint',
                x_align: Clutter.ActorAlign.CENTER,
            });
            // Hints can be long (translations, file paths); wrap instead of cutting off.
            label.clutter_text.line_wrap = true;
            label.clutter_text.line_wrap_mode = Pango.WrapMode.WORD_CHAR;
            label.clutter_text.line_alignment = Pango.Alignment.CENTER;
            box.add_child(label);
        }
        this.add_child(box);
    }
});

const OpenItem = GObject.registerClass(
class OpenItem extends PopupMenu.PopupBaseMenuItem {
    _init(text, onOpen) {
        super._init({style_class: 'rs-open-row'});
        this.add_child(new St.Label({
            text,
            x_expand: true,
            x_align: Clutter.ActorAlign.CENTER,
        }));
        this.connect('activate', () => onOpen());
    }
});

/**
 * Menu section living inside a scroll view. It is added to the menu the
 * regular way so its items are wired up (keyboard navigation, closing), then
 * mount() moves its box into the scroll view — the shell's max-height on the
 * menu only helps if some of the content can scroll.
 */
class ScrollSection extends PopupMenu.PopupMenuSection {
    constructor() {
        super();
        this.scroll = new St.ScrollView({
            style_class: 'rs-scroll',
            hscrollbar_policy: St.PolicyType.NEVER,
            vscrollbar_policy: St.PolicyType.AUTOMATIC,
            // A real gutter, so row hover highlights stop short of the bar.
            overlay_scrollbars: false,
            x_expand: true,
            y_expand: true,
        });
        // Lets the menu's removeAll()/isEmpty() find the section through
        // its new parent.
        this.scroll._delegate = this;
    }

    mount(menuBox) {
        menuBox.replace_child(this.actor, this.scroll);
        this.scroll.set_child(this.actor);
    }

    destroy() {
        super.destroy();
        // Only after the box is gone: destroying the scroll view from the
        // box's own destroy handler trips a Clutter assertion and aborts
        // the shell.
        this.scroll.destroy();
    }
}

// --------------------------------------------------------------- indicator

const Indicator = GObject.registerClass(
class Indicator extends PanelMenu.Button {
    _init(iconPath) {
        super._init(0.5, 'Reinschrift', false);

        const box = new St.BoxLayout({style_class: 'panel-status-menu-box rs-indicator'});

        box.add_child(new St.Icon({
            gicon: Gio.FileIcon.new(Gio.File.new_for_path(iconPath)),
            style_class: 'system-status-icon',
        }));

        this._badge = new St.Label({
            text: '0',
            style_class: 'rs-badge',
            visible: false,
            // Without this the label fills the panel height and renders as a disc.
            y_align: Clutter.ActorAlign.CENTER,
        });
        box.add_child(this._badge);

        this.add_child(box);
    }

    setBadge(count) {
        this._badge.visible = count > 0;
        this._badge.text = String(count);
    }
});

// ---------------------------------------------------------------- extension

export default class ReinschriftExtension extends Extension {
    enable() {
        const iconPath = this.dir.get_child('icons')
            .get_child('reinschrift-symbolic.svg').get_path();

        this._indicator = new Indicator(iconPath);
        this._menu = this._indicator.menu;
        this._menu.actor.add_style_class_name('rs-menu');

        // Same source the shell uses to pick gnome-shell-light/-dark.css.
        this._stSettings = St.Settings.get();
        this._colorSchemeChangedId =
            this._stSettings.connect('notify::color-scheme', () => this._applyTheme());
        this._applyTheme();

        // The extension also works without the app (web app, Nextcloud,
        // Obsidian); everything pointing to the app follows its install state.
        this._appSystem = Shell.AppSystem.get_default();
        this._installedChangedId =
            this._appSystem.connect('installed-changed', () => this._scheduleRebuild());

        this._settings = this.getSettings();
        this._settingsChangedId =
            this._settings.connect('changed::todo-file', () => this._scheduleRebuild());

        this._menu.connect('open-state-changed', (_menu, open) => {
            if (open) {
                this._scheduleRebuild();
            } else if (this._showCompleted) {
                // Start collapsed again next time.
                this._showCompleted = false;
                this._scheduleRebuild();
            }
        });

        this._cancellable = new Gio.Cancellable();
        this._monitor = null;
        this._monitoredPath = null;
        this._dbPath = null;
        this._prefs = {};
        this._items = undefined;
        this._loading = null;
        this._reloadQueued = false;
        this._rebuildId = 0;
        this._settleId = 0;
        this._scroll = null;
        this._showCompleted = false;
        this._addItem = null;
        this._draft = '';
        this._adding = false;
        // PopupMenu.open() refuses to open an empty menu, so it has to be
        // populated up front (a loading state) — rendering only on open
        // would never fire.
        this._render();
        this._scheduleRebuild();

        Main.panel.addToStatusArea(this.uuid, this._indicator);
    }

    disable() {
        for (const id of [this._rebuildId, this._settleId]) {
            if (id)
                GLib.source_remove(id);
        }
        this._rebuildId = 0;
        this._settleId = 0;
        this._cancellable?.cancel();
        this._cancellable = null;
        if (this._monitor) {
            this._monitor.cancel();
            this._monitor = null;
        }
        if (this._stSettings && this._colorSchemeChangedId) {
            this._stSettings.disconnect(this._colorSchemeChangedId);
            this._colorSchemeChangedId = 0;
        }
        this._stSettings = null;
        if (this._appSystem && this._installedChangedId) {
            this._appSystem.disconnect(this._installedChangedId);
            this._installedChangedId = 0;
        }
        this._appSystem = null;
        if (this._settings && this._settingsChangedId) {
            this._settings.disconnect(this._settingsChangedId);
            this._settingsChangedId = 0;
        }
        this._settings = null;
        this._scroll = null;
        this._addItem = null;
        this._menu = null;
        this._indicator?.destroy();
        this._indicator = null;
        this._items = null;
    }

    // ------------------------------------------------------------ theming

    _applyTheme() {
        const actor = this._menu?.actor;
        if (!actor)
            return;
        // The shell is dark unless the system explicitly prefers light
        // (main.js _getStylesheet); 'default' therefore means dark.
        const dark = this._stSettings.color_scheme !== St.SystemColorScheme.PREFER_LIGHT;
        actor.remove_style_class_name(dark ? 'rs-light' : 'rs-dark');
        actor.add_style_class_name(dark ? 'rs-dark' : 'rs-light');
    }

    // ------------------------------------------------------------ data

    /**
     * Re-read preferences and the to-do file; null items when unreadable.
     * State is only touched at the end, so a disable() in between (which
     * cancels) leaves nothing half-applied.
     */
    async _load() {
        const cancellable = this._cancellable;
        const [prefs, candidates] = await readAppPrefs(cancellable);
        const dbPath = await resolveDbPath(
            this._settings.get_string('todo-file'), candidates, cancellable);

        let items = null;
        try {
            const content = await readText(dbPath, cancellable);
            items = Format.splitLines(content)
                .map((line, i) => Format.parseLine(line, i))
                .filter(Boolean);
        } catch (e) {
            if (isCancelled(e))
                throw e;
        }
        cancellable.set_error_if_cancelled();

        this._prefs = prefs;
        this._dbPath = dbPath;
        this._watch();
        this._items = items;
        this._indicator.setBadge(
            items ? items.filter(item => !item.done && isMyDay(item)).length : 0);
    }

    /** (Re)attach the file monitor when the resolved path changed. */
    _watch() {
        if (this._monitoredPath === this._dbPath)
            return;
        this._monitor?.cancel();
        this._monitor = null;
        this._monitoredPath = this._dbPath;
        try {
            this._monitor = Gio.File.new_for_path(this._dbPath)
                .monitor_file(Gio.FileMonitorFlags.NONE, null);
            this._monitor.connect('changed', () => this._scheduleRebuild());
        } catch {
            this._monitor = null;
        }
    }

    // ------------------------------------------------------------ menu

    /** Rebuild outside the current signal emission (row activation, file monitor). */
    _scheduleRebuild() {
        if (this._rebuildId || this._settleId)
            return;
        this._rebuildId = GLib.idle_add(GLib.PRIORITY_DEFAULT_IDLE, () => {
            this._rebuildId = 0;
            this._reload();
            return GLib.SOURCE_REMOVE;
        });
    }

    /** Load in the background, then render; coalesces overlapping requests. */
    async _reload() {
        if (this._loading) {
            this._reloadQueued = true;
            return;
        }
        do {
            this._reloadQueued = false;
            this._loading = this._load();
            try {
                await this._loading;
            } catch (e) {
                if (isCancelled(e))
                    return;
                logError(e, 'Reinschrift: loading failed');
            } finally {
                this._loading = null;
            }
            if (!this._menu)
                return;
            this._render();
        } while (this._reloadQueued);
    }

    _render() {
        const scrollPos = this._scroll?.vadjustment.value ?? 0;
        // Rebuilds come from the file monitor too, often while typing or right
        // after adding; the entry gets its focus back so tasks can be added in a row.
        const entryFocused = this._addItem !== null &&
            global.stage.key_focus === this._addItem.entry.clutter_text;
        this._menu.removeAll();
        this._scroll = null;
        this._addItem = null;

        const app = this._app();
        const addOpenItem = () => {
            if (app)
                this._menu.addMenuItem(new OpenItem(_('Open Reinschrift'), () => this._openApp()));
        };

        if (this._items === undefined) {
            // First load still running.
            this._menu.addMenuItem(new HeaderItem(0, 0));
            addOpenItem();
            return;
        }

        if (this._items === null) {
            this._menu.addMenuItem(new HeaderItem(0, 0));
            const hint = app
                ? _('Create one in the Reinschrift app.')
                // Translators: {path} is where the to-do file was looked for.
                : _('Expected at {path}').replace('{path}', displayPath(this._dbPath));
            this._menu.addMenuItem(new StateItem('dialog-warning-symbolic',
                _('No to-do file found'), hint));
            this._menu.addMenuItem(new OpenItem(_('Choose To-do File…'), () => {
                this._menu.close();
                this.openPreferences();
            }));
            addOpenItem();
            return;
        }

        // Mirrors the app's "My Day" view (gui/src/ui.rs populate_myday_view):
        // today's planned tasks, open ones first, completed below.
        const mode = this._prefs.sort_mode ?? 'topic';
        const byMode = (a, b) =>
            sortKey(a, mode).localeCompare(sortKey(b, mode)) ||
            a.key.lineIndex - b.key.lineIndex;
        const planned = this._items.filter(isMyDay);
        const active = planned.filter(item => !item.done).sort(byMode);
        const done = planned.filter(item => item.done).sort(byMode);

        this._menu.addMenuItem(new HeaderItem(done.length, planned.length));
        this._addItem = new AddItem(this._draft,
            text => this._add(text),
            text => {
                this._draft = text;
            });
        this._menu.addMenuItem(this._addItem);
        if (entryFocused && this._menu.isOpen)
            this._addItem.entry.grab_key_focus();

        if (planned.length === 0) {
            this._menu.addMenuItem(new StateItem('weather-clear-symbolic',
                _('Nothing planned for today'),
                app ? _('Add a task above or plan some in Reinschrift.') : _('Add a task above.')));
            addOpenItem();
            return;
        }

        const section = this._addScrollSection();
        if (active.length === 0)
            section.addMenuItem(new StateItem('object-select-symbolic', _('All done for today'), null));
        for (const item of active)
            section.addMenuItem(new TodoRow(item, row => this._toggle(row)));

        if (done.length) {
            section.addMenuItem(new CompletedToggle(done.length, this._showCompleted, () => {
                this._showCompleted = !this._showCompleted;
                this._scheduleRebuild();
            }));
            if (this._showCompleted) {
                for (const item of done)
                    section.addMenuItem(new TodoRow(item, row => this._toggle(row)));
            }
        }

        addOpenItem();

        if (scrollPos > 0 && this._menu.isOpen) {
            // Restore once the new content has been allocated.
            const adj = this._scroll.vadjustment;
            const id = adj.connect('changed', () => {
                adj.disconnect(id);
                adj.value = Math.min(scrollPos, adj.upper - adj.page_size);
            });
        }
    }

    /** Scrollable part of the menu; see ScrollSection. */
    _addScrollSection() {
        const section = new ScrollSection();
        this._menu.addMenuItem(section);
        section.mount(this._menu.box);
        const scroll = section.scroll;

        // Keep keyboard focus visible while arrowing through a long list.
        section.actor.connect('child-added', (_box, child) => {
            child.connect('key-focus-in', () =>
                ensureActorVisibleInScrollView(scroll, child));
        });

        this._scroll = scroll;
        return section;
    }

    // ---------------------------------------------------------- actions

    async _toggle(row) {
        const item = row.item;
        const keys = [{lineIndex: item.key.lineIndex, marker: item.key.marker}];
        const file = Gio.File.new_for_path(this._dbPath);
        const cancellable = this._cancellable;

        // Read-modify-write against the current file content, like the app.
        let content;
        try {
            content = await readText(this._dbPath, cancellable);
        } catch (e) {
            if (!isCancelled(e))
                Main.notifyError('Reinschrift', `${_('Could not read the to-do file')}: ${e.message}`);
            return;
        }

        const hadTrailing = content.endsWith('\n');
        const lines = Format.splitLines(content);
        Format.toggleInLines(lines, keys, !item.done);
        const output = Format.joinLines(lines, hadTrailing);

        try {
            await file.replace_contents_bytes_async(
                new GLib.Bytes(new TextEncoder().encode(output)),
                null,
                false,
                Gio.FileCreateFlags.REPLACE_DESTINATION,
                cancellable);
        } catch (e) {
            if (!isCancelled(e))
                Main.notifyError('Reinschrift', `${_('Could not save')}: ${e.message}`);
            return;
        }
        if (cancellable.is_cancelled())
            return;

        // Let the tick register visually before the row moves to "Completed";
        // file-monitor rebuilds are held back until then.
        if (this._settleId)
            GLib.source_remove(this._settleId);
        this._settleId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, SETTLE_MS, () => {
            this._settleId = 0;
            this._scheduleRebuild();
            return GLib.SOURCE_REMOVE;
        });
    }

    /** Append a new task planned for today; read-modify-write like _toggle. */
    async _add(text) {
        const title = text.trim();
        if (title === '' || this._adding)
            return;
        this._adding = true;
        const cancellable = this._cancellable;

        try {
            let content;
            try {
                content = await readText(this._dbPath, cancellable);
            } catch (e) {
                if (!isCancelled(e))
                    Main.notifyError('Reinschrift', `${_('Could not read the to-do file')}: ${e.message}`);
                return;
            }

            const hadTrailing = content.endsWith('\n');
            const lines = Format.splitLines(content);
            Format.addToLines(lines, title);
            const output = Format.joinLines(lines, hadTrailing);

            try {
                await Gio.File.new_for_path(this._dbPath).replace_contents_bytes_async(
                    new GLib.Bytes(new TextEncoder().encode(output)),
                    null,
                    false,
                    Gio.FileCreateFlags.REPLACE_DESTINATION,
                    cancellable);
            } catch (e) {
                if (!isCancelled(e))
                    Main.notifyError('Reinschrift', `${_('Could not save')}: ${e.message}`);
                return;
            }
            if (cancellable.is_cancelled())
                return;

            this._draft = '';
            this._addItem?.entry.set_text('');
            this._scheduleRebuild();
        } finally {
            this._adding = false;
        }
    }

    /** The installed Reinschrift app (native or Flatpak), or null. */
    _app() {
        return this._appSystem?.lookup_app(APP_ID) ?? null;
    }

    _openApp() {
        this._menu.close();
        this._app()?.activate();
    }
}
