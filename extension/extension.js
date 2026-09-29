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

/** Identifies a task across re-reads for the opened-details state. */
function rowId(item) {
    return item.key.marker ?? `line:${item.key.lineIndex}`;
}

function toDateTime(d) {
    return GLib.DateTime.new_local(d.getFullYear(), d.getMonth() + 1,
        d.getDate(), d.getHours(), d.getMinutes(), 0);
}

/**
 * Human due label, its urgency class and the highlighter flag, relative to
 * today — like the web app's due view: late is open and overdue (or the time
 * has passed), marked is open and due today or earlier.
 */
function describeDue(due, done = false) {
    if (isSomeday(due))
        return {label: _('Someday'), cls: '', marked: false};

    const now = new Date();
    const days = Math.round(
        (startOfDay(due).getTime() - startOfDay(now).getTime()) / 86400000);
    const hasTime = due.getHours() !== 0 || due.getMinutes() !== 0;
    const time = hasTime ? ` ${toDateTime(due).format('%H:%M')}` : '';

    let label;
    if (days < -1) {
        label = ngettext('{n} day overdue', '{n} days overdue', -days)
            .replace('{n}', String(-days));
    } else if (days === -1) {
        label = _('Yesterday') + time;
    } else if (days === 0) {
        label = _('Today') + time;
    } else if (days === 1) {
        label = _('Tomorrow') + time;
    } else {
        // Translators: short due date within the next weeks, a GLib.DateTime
        // format (like strftime); e.g. "%-d. %b" for German.
        // xgettext:no-javascript-format
        const fmt = days < 7 ? '%A' : _('%b %-d');
        label = toDateTime(due).format(fmt).trim() + time;
    }

    const late = !done && (days < 0 || (hasTime && due < now));
    return {
        label,
        cls: late ? 'rs-due-late' : (!done && days === 0 ? 'rs-due-today' : ''),
        marked: !done && days <= 0,
    };
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
    _init(item, expanded, onToggle, onExpand) {
        super._init({style_class: 'rs-row'});

        this._item = item;
        this._onToggle = onToggle;
        this._onExpand = onExpand;
        if (expanded)
            this.add_style_class_name('rs-row-expanded');

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

        const box = this._textBox = new St.BoxLayout({
            vertical: true,
            x_expand: true,
            y_align: Clutter.ActorAlign.CENTER,
            style_class: 'rs-text',
        });

        // The highlighter hugs the title like the web app's <mark>; the bin
        // keeps the label's natural width, long titles ellipsize.
        this._title = new St.Label({style_class: 'rs-title'});
        this._title.clutter_text.ellipsize = Pango.EllipsizeMode.END;
        this._titleBox = new St.Bin({
            style_class: 'rs-title-box',
            x_align: Clutter.ActorAlign.START,
        });
        this._titleBox.set_child(this._title);
        box.add_child(this._titleBox);

        // Meta tokens the way they read in the Markdown line: +project in
        // ochre, @place in blue, the due date emphasized, recurrence after.
        const meta = [];
        for (const project of item.projects ?? [])
            meta.push([`+${project}`, 'rs-tok rs-tok-project']);
        for (const context of item.contexts ?? [])
            meta.push([`@${context}`, 'rs-tok rs-tok-context']);
        if (item.due) {
            const due = describeDue(item.due, item.done);
            meta.push([due.label, `rs-tok rs-due${due.cls ? ` ${due.cls}` : ''}`.trim()]);
        }
        if (item.recurrence)
            meta.push([`↻ ${item.recurrence}`, 'rs-tok rs-tok-rec']);

        if (meta.length) {
            const line = new St.BoxLayout({style_class: 'rs-meta'});
            meta.forEach(([text, cls]) =>
                line.add_child(new St.Label({text, style_class: cls})));
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
            this._titleBox.remove_style_class_name('rs-marked');
        } else {
            this._checkbox.remove_style_class_name('rs-checkbox-done');
            this.remove_style_class_name('rs-row-done');
            if (this._item.due && describeDue(this._item.due).marked)
                this._titleBox.add_style_class_name('rs-marked');
        }
        this._title.clutter_text.set_markup(markUpTitle(this._item.title, done));
    }

    /**
     * The checkbox (and the margin left of the text) ticks the task off,
     * anywhere else opens its details. From the keyboard, Space ticks off
     * and Enter opens. The menu stays open either way.
     */
    activate(event) {
        if (this._hitsCheckbox(event)) {
            this._setDone(!this._item.done);
            this._onToggle(this);
        } else {
            this._onExpand(this, event?.type() === Clutter.EventType.KEY_PRESS);
        }
    }

    _hitsCheckbox(event) {
        if (!event)
            return false;
        if (event.type() === Clutter.EventType.KEY_PRESS)
            return event.get_key_symbol() === Clutter.KEY_space;
        const [x] = event.get_coords();
        const [textLeft] = this._textBox.get_transformed_position();
        return x < textLeft;
    }

    get item() {
        return this._item;
    }
});

/** Note and actions of the task opened below its row. */
const DetailItem = GObject.registerClass(
class DetailItem extends PopupMenu.PopupBaseMenuItem {
    _init(item, onPostpone, onRemove) {
        super._init({
            style_class: 'rs-detail',
            activate: false,
            hover: false,
            can_focus: false,
        });
        this.track_hover = false;

        const box = new St.BoxLayout({
            vertical: true,
            x_expand: true,
            style_class: 'rs-detail-box',
        });

        // The note reads like the web app's details: a quoted block.
        if (item.note) {
            const quoted = new St.BoxLayout({style_class: 'rs-detail-note-row'});
            quoted.add_child(new St.Widget({style_class: 'rs-note-rule'}));
            const note = new St.Label({
                text: item.note,
                style_class: 'rs-detail-note',
                x_expand: true,
            });
            note.clutter_text.line_wrap = true;
            note.clutter_text.line_wrap_mode = Pango.WrapMode.WORD_CHAR;
            quoted.add_child(note);
            box.add_child(quoted);
        }

        // Natural-width chips that wrap, like the web app's action row.
        const flow = new Clutter.FlowLayout({
            orientation: Clutter.Orientation.HORIZONTAL,
            column_spacing: 6,
            row_spacing: 6,
        });
        const actions = new St.Widget({layout_manager: flow, x_expand: true});
        const button = (label, onClicked, styleClass = 'rs-chip') => {
            const btn = new St.Button({
                label,
                style_class: styleClass,
                can_focus: true,
            });
            btn.connect('clicked', onClicked);
            return btn;
        };

        if (!item.done) {
            // Postponing also takes the task off today's plan, otherwise it
            // would stay right here.
            const targets = [
                ['tomorrow', _('Tomorrow')],
                ['weekend', _('Weekend')],
                ['nextweek', _('Next Week')],
                ['someday', _('Someday')],
            ];
            for (const [target, label] of targets)
                actions.add_child(button(label, () => onPostpone(target)));
        }
        actions.add_child(button(_('Remove from My Day'), onRemove, 'rs-chip rs-chip-quiet'));
        box.add_child(actions);

        this.add_child(box);
    }
});

/** The web app's header: today's date, the "My Day" tab and progress. */
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

        // The weekday big, the rest of the date beside it — the web app
        // renders its header the same way (GLib localizes the names).
        const now = GLib.DateTime.new_now_local();
        const dateRow = new St.BoxLayout({style_class: 'rs-header-date'});
        dateRow.add_child(new St.Label({
            text: now.format('%A'),
            style_class: 'rs-date-day',
            y_align: Clutter.ActorAlign.END,
        }));
        // Translators: the date next to the weekday in the menu header, a
        // GLib.DateTime format (like strftime); e.g. "%-d. %B" for German.
        // xgettext:no-javascript-format
        dateRow.add_child(new St.Label({
            text: now.format(_('%B %-d')),
            style_class: 'rs-date-rest',
            y_align: Clutter.ActorAlign.END,
        }));
        column.add_child(dateRow);

        // "My Day" reads like the active tab of the web app's view bar; the
        // progress count sits at its right like a section count.
        const tabs = new St.BoxLayout({style_class: 'rs-header-tabs', x_expand: true});
        const tab = new St.BoxLayout({vertical: true, style_class: 'rs-tab'});
        tab.add_child(new St.Label({text: _('My Day'), style_class: 'rs-tab-label'}));
        tab.add_child(new St.Widget({style_class: 'rs-tab-underline', x_expand: true}));
        tabs.add_child(tab);
        tabs.add_child(new St.Widget({x_expand: true}));
        if (total > 0) {
            // Translators: progress in the menu header, e.g. "4 of 13 done".
            const progress = _('{done} of {total} done')
                .replace('{done}', String(done))
                .replace('{total}', String(total));
            tabs.add_child(new St.Label({
                text: progress,
                style_class: 'rs-progress-text',
                y_align: Clutter.ActorAlign.CENTER,
            }));
        }
        column.add_child(tabs);

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

/** Entry for a new task; it lands in today's plan, like adding in the app's "My Day".
 *  Styled like the web app's add line: a bare input over a 2px ink underline,
 *  with the plus as a trailing submit button. */
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

        const box = new St.BoxLayout({
            vertical: true,
            x_expand: true,
            style_class: 'rs-add-box',
        });

        const row = new St.BoxLayout({style_class: 'rs-add-row', x_expand: true});
        this.entry = new St.Entry({
            style_class: 'rs-add-entry',
            hint_text: _('Add a task'),
            can_focus: true,
            x_expand: true,
        });
        this.entry.text = draft;
        this.entry.clutter_text.connect('activate', () => onSubmit(this.entry.text));
        this.entry.clutter_text.connect('text-changed', () => onChange(this.entry.text));
        row.add_child(this.entry);

        const submit = new St.Button({
            style_class: 'rs-add-submit',
            can_focus: true,
        });
        submit.set_child(new St.Icon({
            icon_name: 'list-add-symbolic',
            style_class: 'rs-add-submit-icon',
        }));
        submit.connect('clicked', () => onSubmit(this.entry.text));
        row.add_child(submit);

        box.add_child(row);
        box.add_child(new St.Widget({style_class: 'rs-add-underline'}));
        this.add_child(box);
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
        // Like the web app's My Day empty state: the icon sits in a
        // highlighter-yellow circle.
        const badge = new St.Bin({
            style_class: 'rs-state-badge',
            x_align: Clutter.ActorAlign.CENTER,
        });
        badge.set_child(new St.Icon({
            icon_name: iconName,
            style_class: 'rs-state-icon',
        }));
        box.add_child(badge);
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

/** Full-width footer button; like the web app's, primary is inverted ink,
 *  secondary is a bordered button. */
const OpenItem = GObject.registerClass(
class OpenItem extends PopupMenu.PopupBaseMenuItem {
    _init(text, onOpen, styleClass = 'rs-btn-primary') {
        super._init({style_class: `rs-open-row ${styleClass}`});
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

        this._openStateChangedId = this._menu.connect('open-state-changed', (_menu, open) => {
            if (open) {
                this._scheduleRebuild();
            } else if (this._showCompleted || this._expanded) {
                // Start collapsed again next time.
                this._showCompleted = false;
                this._expanded = null;
                this._scheduleRebuild();
            }
        });
        // Keep keyboard focus visible while arrowing through a long list.
        // Items also turn active on hover; only follow the keyboard.
        this._activeChangedId = this._menu.connect('active-changed', (_menu, item) => {
            if (item && this._scroll?.contains(item) && item.has_key_focus())
                ensureActorVisibleInScrollView(this._scroll, item);
        });

        this._cancellable = new Gio.Cancellable();
        this._monitor = null;
        this._monitorChangedId = 0;
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
        this._expanded = null;
        this._focusExpanded = false;
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
        if (this._rebuildId) {
            GLib.source_remove(this._rebuildId);
            this._rebuildId = 0;
        }
        if (this._settleId) {
            GLib.source_remove(this._settleId);
            this._settleId = 0;
        }
        this._cancellable?.cancel();
        this._cancellable = null;
        this._unwatch();
        if (this._menu) {
            this._menu.disconnect(this._openStateChangedId);
            this._menu.disconnect(this._activeChangedId);
            this._openStateChangedId = 0;
            this._activeChangedId = 0;
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
        this._addItem?.destroy();
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
        // (main.js _getStylesheet); 'default' therefore means dark. The
        // indicator carries the classes too, so the panel badge can follow
        // the same palette.
        const dark = this._stSettings.color_scheme !== St.SystemColorScheme.PREFER_LIGHT;
        this._indicator?.remove_style_class_name(dark ? 'rs-light' : 'rs-dark');
        this._indicator?.add_style_class_name(dark ? 'rs-dark' : 'rs-light');
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
        this._unwatch();
        this._monitoredPath = this._dbPath;
        try {
            this._monitor = Gio.File.new_for_path(this._dbPath)
                .monitor_file(Gio.FileMonitorFlags.NONE, null);
            this._monitorChangedId =
                this._monitor.connect('changed', () => this._scheduleRebuild());
        } catch {
            this._monitor = null;
        }
    }

    _unwatch() {
        if (!this._monitor)
            return;
        this._monitor.disconnect(this._monitorChangedId);
        this._monitorChangedId = 0;
        this._monitor.cancel();
        this._monitor = null;
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
            }, 'rs-btn-secondary'));
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
        let focusRow = null;
        const addRow = item => {
            const expanded = rowId(item) === this._expanded;
            const row = new TodoRow(item, expanded,
                r => this._toggle(r),
                (r, fromKeyboard) => this._expand(r.item, fromKeyboard));
            section.addMenuItem(row);
            if (expanded) {
                section.addMenuItem(new DetailItem(item,
                    target => this._postpone(item, target),
                    () => this._removeFromMyDay(item)));
                focusRow = row;
            }
        };

        if (active.length === 0)
            section.addMenuItem(new StateItem('object-select-symbolic', _('All done for today'), null));
        for (const item of active)
            addRow(item);

        if (done.length) {
            section.addMenuItem(new CompletedToggle(done.length, this._showCompleted, () => {
                this._showCompleted = !this._showCompleted;
                this._scheduleRebuild();
            }));
            if (this._showCompleted) {
                for (const item of done)
                    addRow(item);
            }
        }

        addOpenItem();

        // Opened from the keyboard: stay on the row so arrowing continues there.
        if (focusRow && this._focusExpanded && this._menu.isOpen)
            focusRow.grab_key_focus();
        this._focusExpanded = false;

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
        this._scroll = section.scroll;
        return section;
    }

    // ---------------------------------------------------------- actions

    /**
     * Read-modify-write against the current file content, like the app.
     * `edit` changes the lines in place and returns false to skip saving.
     * Returns whether the file was written.
     */
    async _modifyFile(edit) {
        const cancellable = this._cancellable;

        let content;
        try {
            content = await readText(this._dbPath, cancellable);
        } catch (e) {
            if (!isCancelled(e))
                Main.notifyError('Reinschrift', `${_('Could not read the to-do file')}: ${e.message}`);
            return false;
        }

        const hadTrailing = content.endsWith('\n');
        const lines = Format.splitLines(content);
        if (edit(lines) === false)
            return false;
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
            return false;
        }
        return !cancellable.is_cancelled();
    }

    async _toggle(row) {
        const item = row.item;
        const keys = [{lineIndex: item.key.lineIndex, marker: item.key.marker}];
        if (!await this._modifyFile(lines => Format.toggleInLines(lines, keys, !item.done)))
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

    /** Open or close the details below a task; one at a time. */
    _expand(item, fromKeyboard) {
        const id = rowId(item);
        this._expanded = this._expanded === id ? null : id;
        this._focusExpanded = fromKeyboard;
        this._scheduleRebuild();
    }

    /** Move a task to a later day, off today's plan. */
    async _postpone(item, target) {
        const due = Format.dueForTarget(target, item.due);
        await this._editTask(item,
            line => Format.removeMyday(Format.rewriteDue(line, due)));
    }

    async _removeFromMyDay(item) {
        await this._editTask(item, line => Format.removeMyday(line));
    }

    async _editTask(item, rewrite) {
        const saved = await this._modifyFile(lines =>
            Format.updateInLines(lines, item.key, rewrite));
        if (!saved)
            return;
        this._expanded = null;
        this._scheduleRebuild();
    }

    /** Append a new task planned for today. */
    async _add(text) {
        const title = text.trim();
        if (title === '' || this._adding)
            return;
        this._adding = true;
        try {
            if (!await this._modifyFile(lines => {
                Format.addToLines(lines, title);
            }))
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
