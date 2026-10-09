use std::cell::{Cell, RefCell};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Mutex;
use std::time::Duration as StdDuration;

use adw::prelude::*;
use adw::{self, Application};
use anyhow::{anyhow, Result};
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use glib::{clone, BoxedAnyObject};
use gtk::gdk;
use gtk::gio;
use gtk::AlertDialog;
use gtk::gio::prelude::*;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;
use serde::Deserialize;
use tokio::runtime::Runtime;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use std::collections::{HashMap, HashSet, VecDeque};
use reinschrift_core::embeddings;
use reinschrift_core::parser::parse_line;
use reinschrift_core::util::{canonical_casing_map, canonicalize_token};
use reinschrift_core::{data, TodoItem, SortMode, sort_items, t, tc, Preferences, load_preferences, write_preferences};
use reinschrift_core::{DueRange, TodoFilter, NO_TAG};

enum VoiceMsg {
    Error(String),
    Transcription(String),
    Transcribing,
    Finished,
}

#[derive(Clone)]
#[derive(PartialEq)]
enum ListEntry {
    Header(String),
    Item(TodoItem),
    /// Kandidat im "Mein Tag"-Planungs-Picker (noch nicht für heute geplant).
    PickerItem(TodoItem),
}

/// Was `AppState::apply_entries` am Store getan hat.
#[derive(Clone, Copy, PartialEq)]
enum ListUpdate {
    Unchanged,
    /// Nur geänderte Abschnitte ersetzt; die übrigen Zeilen bleiben.
    /// `refocus_y`: Der Fokus lag in der Liste und wurde vorher abgenommen;
    /// er gehört danach der Zeile an dieser y-Position.
    Partial { refocus_y: Option<f32> },
    /// Alles ersetzt.
    Full,
}

/// Datenschlüssel am Zeilen-Stack für das gebundene `BoxedAnyObject`.
const ROW_ENTRY_KEY: &str = "row-entry";

/// Zustand außerhalb der `ListEntry`, den das Binden einer Zeile ausliest.
/// `apply_entries` vergleicht ihn mit dem letzten Aufbau, um zu wissen,
/// welche Zeilen trotz gleichem Eintrag neu gebunden werden müssen.
#[derive(Clone, PartialEq)]
struct RowContext {
    highlight: Option<String>,
    selection_mode: bool,
    selected: HashSet<String>,
    compact: bool,
    myday: bool,
}

/// Ein Auftrag der Hintergrund-Schreibwarteschlange (siehe `AppState::submit`).
#[derive(Clone)]
enum WriteJob {
    /// `id` ist der Journal-Eintrag; fehlt nur, wenn das Journal nicht
    /// geschrieben werden konnte.
    Op {
        id: Option<u64>,
        op: data::PendingOp,
        mode: data::ApplyMode,
    },
    Undo,
    /// „Überschreiben" nach einem Konflikt: genau diesen Inhalt schreiben.
    Overwrite(String),
}

enum WriteResult {
    Op(Result<Option<String>>),
    Undo(Result<Option<String>>),
    Overwrite(Result<()>),
}

impl WriteJob {
    /// Läuft im Hintergrund-Thread.
    fn run(&self) -> WriteResult {
        match self {
            WriteJob::Op { op, mode, .. } => WriteResult::Op(op.apply(*mode)),
            WriteJob::Undo => WriteResult::Undo(data::undo()),
            WriteJob::Overwrite(content) => {
                WriteResult::Overwrite(data::force_write_content(content.clone()))
            }
        }
    }

    fn failed(&self, err: anyhow::Error) -> WriteResult {
        match self {
            WriteJob::Op { .. } => WriteResult::Op(Err(err)),
            WriteJob::Undo => WriteResult::Undo(Err(err)),
            WriteJob::Overwrite(_) => WriteResult::Overwrite(Err(err)),
        }
    }
}

thread_local! {
    /// Ein Journal pro Prozess. Öffnet man direkt nach dem Schließen wieder
    /// ein Fenster, während das alte noch speichert, landet es im selben
    /// Prozess (GApplication ist einzelinstanzig) — beide teilen sich dann die
    /// Datei, statt sich gegenseitig ihre Einträge zu überschreiben.
    static JOURNAL: Rc<RefCell<data::Journal>> = Rc::new(RefCell::new(data::Journal::open({
        let mut path = glib::user_data_dir();
        path.push("reinschrift_todo");
        path.push("pending-writes.json");
        path
    })));
    /// Übrig gebliebene Einträge nur einmal pro Prozess nachholen.
    static JOURNAL_REPLAYED: Cell<bool> = const { Cell::new(false) };
}

/// Eine noch nicht gespeicherte Änderung auf die angezeigte Liste anwenden.
///
/// Das ist eine Vorschau, keine zweite Implementierung: sobald alles
/// gespeichert ist, ersetzt der echte Dateistand sie (samt Details wie der
/// nächsten Instanz einer wiederkehrenden Aufgabe).
fn apply_optimistic(items: &mut Vec<TodoItem>, op: &data::PendingOp) {
    use data::PendingOp as Op;
    fn hits(item: &TodoItem, keys: &[data::TodoKey]) -> bool {
        keys.iter().any(|key| match key.marker.as_deref() {
            Some(marker) if !marker.is_empty() => item.key.marker.as_deref() == Some(marker),
            _ => item.key.line_index == key.line_index,
        })
    }
    fn add_missing(tags: &mut Vec<String>, new: &[String]) {
        for tag in new {
            let tag = tag.trim_start_matches(['+', '@']).trim();
            if !tag.is_empty() && !tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                tags.push(tag.to_string());
            }
        }
    }
    match op {
        Op::SetDone { keys, done } => items
            .iter_mut()
            .filter(|item| hits(item, keys))
            .for_each(|item| item.done = *done),
        Op::SetDue { keys, target } => items
            .iter_mut()
            .filter(|item| hits(item, keys))
            .for_each(|item| {
                let due = data::due_for_target(*target, item.due);
                if data::leaves_myday(due) {
                    item.myday = None;
                }
                item.due = Some(due);
            }),
        Op::SetMyday { key, on } => {
            let today = Local::now().date_naive();
            items
                .iter_mut()
                .filter(|item| hits(item, std::slice::from_ref(key)))
                .for_each(|item| item.myday = on.then_some(today));
        }
        Op::Update { item: updated } => {
            for item in items.iter_mut().filter(|item| hits(item, std::slice::from_ref(&updated.key))) {
                let line_index = item.key.line_index;
                *item = updated.clone();
                item.key.line_index = line_index;
            }
        }
        Op::Delete { keys } => items.retain(|item| !hits(item, keys)),
        Op::Add { line, marker } => {
            let exists = items.iter().any(|i| i.key.marker.as_deref() == Some(marker.as_str()));
            if !exists && let Some(item) = parse_line(line, usize::MAX) {
                items.push(item);
            }
        }
        Op::Assign {
            keys,
            projects,
            contexts,
        } => {
            for item in items.iter_mut().filter(|item| hits(item, keys)) {
                add_missing(&mut item.projects, projects);
                add_missing(&mut item.contexts, contexts);
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct AiParseResult {
    title: Option<String>,
    due: Option<String>,
    context: Option<String>,
    project: Option<String>,
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AiChatMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct AiChatResponse {
    message: AiChatMessage,
}

const DEFAULT_DUE_TIME: NaiveTime = NaiveTime::from_hms_opt(0, 0, 0).expect("midnight available");

/// File name used when a WebDAV account is connected without an explicit path.
const DEFAULT_WEBDAV_PATH: &str = "todos.md";

/// Create (if missing) and return the fallback todos file inside XDG data home.
/// Returns None if the directory cannot be prepared or the file cannot be created.
fn ensure_default_database() -> Option<PathBuf> {
    let mut path = glib::user_data_dir();
    path.push("reinschrift_todo");
    if let Err(err) = fs::create_dir_all(&path) {
        eprintln!("failed to create data dir {}: {}", path.display(), err);
        return None;
    }
    path.push("todos.md");
    if !path.exists()
        && let Err(err) = fs::write(&path, "") {
            eprintln!("failed to create default todos file {}: {}", path.display(), err);
            return None;
        }
    Some(path)
}

/// Accessible-Label setzen, damit Orca Icon-only-Buttons und Checkboxen
/// vorlesen kann (Tooltips allein sind für Screenreader nicht verlässlich).
fn set_a11y_label(widget: &impl IsA<gtk::Accessible>, label: &str) {
    widget.update_property(&[gtk::accessible::Property::Label(label)]);
}

/// Übersetzte Strings für das Einbetten in Builder-UI-XML escapen.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn schedule_poll(state: Rc<AppState>, interval: u32) {
    let weak = Rc::downgrade(&state);
    glib::timeout_add_seconds_local_once(interval, move || {
        let Some(state) = weak.upgrade() else {
            return;
        };
        // Die Abfrage läuft im Hintergrund: ein langsamer oder hängender
        // Server lässt so nicht mehr alle zehn Sekunden das Fenster stocken.
        glib::spawn_future_local(async move {
            let next_interval = match state.check_for_updates().await {
                Ok(changed) => {
                    if changed {
                        state.warm_semantic_index();
                    }
                    10
                }
                Err(e) => {
                    eprintln!("{}", t("Auto-reload failed: {}").replace("{}", &e.to_string()));
                    std::cmp::min(interval * 2, 300)
                }
            };
            schedule_poll(state, next_interval);
        });
    });
}

/// Creates an Entry with a dropdown button for suggestions.
/// Returns (entry, horizontal_box) where horizontal_box contains entry + button.
fn create_suggestion_entry(
    initial_text: &str,
    suggestions: &[String],
    prefix: &str,
) -> (gtk::Entry, gtk::Box) {
    let entry = gtk::Entry::new();
    entry.set_text(initial_text);
    entry.set_activates_default(true);
    entry.set_hexpand(true);

    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    hbox.append(&entry);

    // Only show button if there are suggestions
    if !suggestions.is_empty() {
        let popover = gtk::Popover::new();
        let listbox = gtk::ListBox::new();
        listbox.set_selection_mode(gtk::SelectionMode::None);

        // Take top 5 suggestions
        for suggestion in suggestions.iter().take(5) {
            let label_text = format!("{}{}", prefix, suggestion);
            let label = gtk::Label::new(Some(&label_text));
            label.set_xalign(0.0);
            label.set_margin_start(8);
            label.set_margin_end(8);
            label.set_margin_top(4);
            label.set_margin_bottom(4);
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&label));
            listbox.append(&row);
        }

        popover.set_child(Some(&listbox));

        let menu_button = gtk::MenuButton::new();
        menu_button.set_icon_name("view-more-symbolic");
        menu_button.set_popover(Some(&popover));
        menu_button.set_tooltip_text(Some(&t("Show suggestions")));
        menu_button.add_css_class("flat");
        set_a11y_label(&menu_button, &t("Show suggestions"));

        // Connect row activation to append suggestion to entry
        let entry_clone = entry.clone();
        let prefix_owned = prefix.to_string();
        let suggestions_owned: Vec<String> = suggestions.iter().take(5).cloned().collect();
        let popover_clone = popover.clone();
        listbox.connect_row_activated(move |_, row| {
            let index = row.index() as usize;
            if let Some(suggestion) = suggestions_owned.get(index) {
                let suggestion_with_prefix = format!("{}{}", prefix_owned, suggestion);
                let current = entry_clone.text();

                // Check if already present (tag-aware: names may contain spaces)
                let prefix_char = prefix_owned.chars().next().unwrap_or('+');
                let already_present = data::split_tag_input(&current, prefix_char)
                    .iter()
                    .any(|s| s == suggestion);

                if !already_present {
                    let new_text = if current.is_empty() {
                        suggestion_with_prefix
                    } else {
                        format!("{} {}", current.trim(), suggestion_with_prefix)
                    };
                    entry_clone.set_text(&new_text);
                }
            }
            popover_clone.popdown();
        });

        hbox.append(&menu_button);
    }

    (entry, hbox)
}

/// Attach a live title-autocomplete popover to an existing entry.
///
/// The popover opens once the trimmed entry text reaches 2 characters and
/// shows up to 8 case-insensitive matches from `title_provider`. Down/Up
/// navigate, Enter selects, Escape closes (without consuming the
/// surrounding container's Escape handler when the popover is hidden).
/// Extract the suggestion title from an autocomplete row (the label is
/// either the row child itself or the first child of its hbox).
fn autocomplete_row_title(row: &gtk::ListBoxRow) -> Option<String> {
    let child = row.child()?;
    if let Ok(label) = child.clone().downcast::<gtk::Label>() {
        return Some(label.text().to_string());
    }
    let hbox = child.downcast::<gtk::Box>().ok()?;
    hbox.first_child()?
        .downcast::<gtk::Label>()
        .ok()
        .map(|label| label.text().to_string())
}

fn attach_title_autocomplete(
    entry: &gtk::Entry,
    title_provider: Rc<dyn Fn() -> Vec<String>>,
) {
    attach_title_autocomplete_with_duplicate(entry, title_provider, None, None);
}

/// Quote a tag for inline +project/@context syntax if it contains spaces.
fn quote_tag(tag: &str) -> String {
    if tag.contains(char::is_whitespace) {
        format!("\"{}\"", tag)
    } else {
        tag.to_string()
    }
}

/// The typed text plus the semantic hit's tags (auto-tagging): selecting a
/// semantically similar task keeps the new title and appends its
/// +projects/@contexts.
fn semantic_apply_text(typed: &str, item: &TodoItem) -> String {
    let mut out = typed.trim().to_string();
    for project in &item.projects {
        out.push_str(&format!(" +{}", quote_tag(project)));
    }
    for context in &item.contexts {
        out.push_str(&format!(" @{}", quote_tag(context)));
    }
    out
}

/// Like `attach_title_autocomplete`, but with an optional duplicate action:
/// when `on_duplicate` is set, each suggestion row gets a copy button that
/// duplicates the matched task (issue #6). When `semantic_state` is set
/// (and the preference is enabled), a debounced, labeled "similar tasks"
/// section with embedding-based hits appears below the substring matches.
fn attach_title_autocomplete_with_duplicate(
    entry: &gtk::Entry,
    title_provider: Rc<dyn Fn() -> Vec<String>>,
    on_duplicate: Option<Rc<dyn Fn(String)>>,
    semantic_state: Option<std::rc::Weak<AppState>>,
) {
    let popover = gtk::Popover::new();
    popover.set_parent(entry);
    popover.set_autohide(false);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_has_arrow(false);

    let listbox = gtk::ListBox::new();
    listbox.set_selection_mode(gtk::SelectionMode::Single);

    let scroller = gtk::ScrolledWindow::builder()
        .child(&listbox)
        .min_content_height(40)
        .max_content_height(280)
        .propagate_natural_height(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    popover.set_child(Some(&scroller));

    let popover = Rc::new(popover);
    let listbox = Rc::new(listbox);

    let rebuild: Rc<dyn Fn()> = {
        let listbox = Rc::clone(&listbox);
        let popover = Rc::clone(&popover);
        let provider = Rc::clone(&title_provider);
        let entry_weak = entry.downgrade();
        let on_duplicate = on_duplicate.clone();
        Rc::new(move || {
            let Some(entry) = entry_weak.upgrade() else { return; };
            let text = entry.text().to_string();
            let trimmed = text.trim();

            while let Some(child) = listbox.first_child() {
                listbox.remove(&child);
            }

            if trimmed.chars().count() < 2 {
                popover.popdown();
                return;
            }

            let needle = trimmed.to_lowercase();
            let titles = provider();
            let mut count = 0;
            for title in &titles {
                if title == trimmed {
                    continue;
                }
                if !title.to_lowercase().contains(&needle) {
                    continue;
                }
                let label = gtk::Label::builder()
                    .label(title)
                    .xalign(0.0)
                    .ellipsize(pango::EllipsizeMode::End)
                    .hexpand(true)
                    .build();
                label.set_margin_start(8);
                label.set_margin_end(8);
                label.set_margin_top(4);
                label.set_margin_bottom(4);

                let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                hbox.append(&label);

                if let Some(on_duplicate) = on_duplicate.as_ref() {
                    let copy_btn = gtk::Button::builder()
                        .icon_name("edit-copy-symbolic")
                        .tooltip_text(t("Duplicate task"))
                        .build();
                    copy_btn.add_css_class("flat");
                    copy_btn.set_valign(gtk::Align::Center);
                    set_a11y_label(&copy_btn, &t("Duplicate task"));
                    let on_duplicate = Rc::clone(on_duplicate);
                    let title_for_copy = title.clone();
                    let popover_for_copy = Rc::clone(&popover);
                    let entry_for_copy = entry.clone();
                    copy_btn.connect_clicked(move |_| {
                        popover_for_copy.popdown();
                        entry_for_copy.set_text("");
                        on_duplicate(title_for_copy.clone());
                    });
                    hbox.append(&copy_btn);
                }

                let row = gtk::ListBoxRow::new();
                row.set_child(Some(&hbox));
                listbox.append(&row);
                count += 1;
                if count >= 8 {
                    break;
                }
            }

            if count == 0 {
                popover.popdown();
            } else {
                // Popover auf Entry-Breite bringen — sonst kollabiert das
                // ScrolledWindow auf Minimalbreite und alle Titel
                // ellipsieren zu „…".
                popover.set_size_request(entry.width(), -1);
                popover.popup();
            }
        })
    };

    let rebuild_for_change = Rc::clone(&rebuild);
    entry.connect_changed(move |_| {
        rebuild_for_change();
    });

    // Debounced semantische Sektion „Ähnliche Aufgaben" unterhalb der
    // Substring-Treffer. Der Rebuild oben leert die Listbox bei jeder
    // Eingabe, daher können veraltete Semantik-Zeilen nicht stehenbleiben.
    if let Some(state_weak) = semantic_state {
        let semantic_timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
        let listbox_sem = Rc::clone(&listbox);
        let popover_sem = Rc::clone(&popover);
        entry.connect_changed(move |entry_now| {
            if let Some(id) = semantic_timer.borrow_mut().take() {
                id.remove();
            }
            let Some(state) = state_weak.upgrade() else {
                return;
            };
            if !state.semantic_enabled() {
                return;
            }
            let typed = entry_now.text().trim().to_string();
            if typed.chars().count() < 4 {
                return;
            }
            let listbox = Rc::clone(&listbox_sem);
            let popover = Rc::clone(&popover_sem);
            let entry_weak = entry_now.downgrade();
            let timer_slot = Rc::clone(&semantic_timer);
            let timer_done = Rc::clone(&semantic_timer);
            let id = glib::timeout_add_local_once(StdDuration::from_millis(300), move || {
                *timer_done.borrow_mut() = None;
                glib::spawn_future_local(async move {
                    let hits = state
                        .semantic_query(
                            typed.clone(),
                            embeddings::TAG_SUGGEST_TOP_K,
                            embeddings::TAG_SUGGEST_THRESHOLD,
                            false,
                        )
                        .await;
                    if hits.is_empty() {
                        return;
                    }
                    let Some(entry) = entry_weak.upgrade() else {
                        return;
                    };
                    // Veraltet oder Fokus inzwischen weg: nichts anzeigen.
                    if entry.text().trim() != typed {
                        return;
                    }
                    if !entry.state_flags().contains(gtk::StateFlags::FOCUS_WITHIN) {
                        return;
                    }
                    // Substring-Treffer sind bereits sichtbar — auslassen.
                    let needle = typed.to_lowercase();
                    let fresh: Vec<TodoItem> = hits
                        .into_iter()
                        .map(|(item, _)| item)
                        .filter(|item| !item.title.to_lowercase().contains(&needle))
                        .collect();
                    if fresh.is_empty() {
                        return;
                    }

                    let header_label = gtk::Label::builder()
                        .label(t("Similar tasks"))
                        .xalign(0.0)
                        .build();
                    header_label.add_css_class("dim-label");
                    header_label.set_margin_start(8);
                    header_label.set_margin_end(8);
                    header_label.set_margin_top(6);
                    header_label.set_margin_bottom(2);
                    let header_row = gtk::ListBoxRow::new();
                    header_row.set_child(Some(&header_label));
                    header_row.set_selectable(false);
                    header_row.set_activatable(false);
                    listbox.append(&header_row);

                    for item in fresh {
                        // Erstes (unsichtbares) Label trägt den Übernahme-
                        // Text: getippter Titel + Tags des Treffers —
                        // `autocomplete_row_title` liest das erste Label.
                        let apply = semantic_apply_text(&typed, &item);
                        let hidden = gtk::Label::new(Some(&apply));
                        hidden.set_visible(false);

                        let mut display = item.title.clone();
                        for project in &item.projects {
                            display.push_str(&format!(" +{}", quote_tag(project)));
                        }
                        for context in &item.contexts {
                            display.push_str(&format!(" @{}", quote_tag(context)));
                        }
                        let label = gtk::Label::builder()
                            .label(&display)
                            .xalign(0.0)
                            .ellipsize(pango::EllipsizeMode::End)
                            .hexpand(true)
                            .build();
                        label.set_margin_start(8);
                        label.set_margin_end(8);
                        label.set_margin_top(4);
                        label.set_margin_bottom(4);

                        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                        hbox.append(&hidden);
                        hbox.append(&label);

                        let row = gtk::ListBoxRow::new();
                        row.set_child(Some(&hbox));
                        listbox.append(&row);
                    }
                    popover.set_size_request(entry.width(), -1);
                    popover.popup();
                });
            });
            *timer_slot.borrow_mut() = Some(id);
        });
    }

    let popover_for_row = Rc::clone(&popover);
    let entry_for_row = entry.clone();
    listbox.connect_row_activated(move |_, row| {
        if let Some(title) = autocomplete_row_title(row) {
            entry_for_row.set_text(&title);
            entry_for_row.set_position(-1);
        }
        popover_for_row.popdown();
    });

    let key_ctl = gtk::EventControllerKey::new();
    key_ctl.set_propagation_phase(gtk::PropagationPhase::Capture);
    let listbox_for_key = Rc::clone(&listbox);
    let popover_for_key = Rc::clone(&popover);
    let entry_for_key = entry.clone();
    key_ctl.connect_key_pressed(move |_, key, _, _| {
        if !popover_for_key.is_visible() {
            return glib::Propagation::Proceed;
        }
        match key {
            gdk::Key::Down => {
                // Nicht-selektierbare Zeilen (Semantik-Header) überspringen.
                let mut idx = match listbox_for_key.selected_row() {
                    Some(row) => row.index() + 1,
                    None => 0,
                };
                let next = loop {
                    match listbox_for_key.row_at_index(idx) {
                        Some(row) if row.is_selectable() => break Some(row),
                        Some(_) => idx += 1,
                        None => break listbox_for_key
                            .row_at_index(0)
                            .filter(|row| row.is_selectable()),
                    }
                };
                if let Some(row) = next {
                    listbox_for_key.select_row(Some(&row));
                }
                glib::Propagation::Stop
            }
            gdk::Key::Up => {
                let mut idx = match listbox_for_key.selected_row() {
                    Some(row) => row.index() - 1,
                    None => -1,
                };
                let prev = loop {
                    if idx < 0 {
                        break None;
                    }
                    match listbox_for_key.row_at_index(idx) {
                        Some(row) if row.is_selectable() => break Some(row),
                        _ => idx -= 1,
                    }
                };
                if let Some(row) = prev {
                    listbox_for_key.select_row(Some(&row));
                }
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                if let Some(row) = listbox_for_key.selected_row() {
                    if let Some(title) = autocomplete_row_title(&row) {
                        entry_for_key.set_text(&title);
                        entry_for_key.set_position(-1);
                    }
                    popover_for_key.popdown();
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
            gdk::Key::Escape => {
                popover_for_key.popdown();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    entry.add_controller(key_ctl);

    // Pop down when the entry is hidden so the popover doesn't linger above
    // a closed dialog.
    let popover_for_unmap = Rc::clone(&popover);
    entry.connect_unmap(move |_| {
        popover_for_unmap.popdown();
    });
}

pub fn build_ui(app: &Application, debug_mode: bool) -> Result<()> {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        "@keyframes pulse {
            0% { opacity: 1.0; }
            50% { opacity: 0.3; }
            100% { opacity: 1.0; }
        }
        .pulse {
            animation: pulse 1s infinite;
        }
        .selected-row {
            background-color: alpha(@accent_bg_color, 0.15);
            border-radius: 6px;
        }
        .compact-touch {
            min-width: 44px;
            min-height: 44px;
        }",
    );
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("Could not connect to a display."),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    // Gespeicherte Fenstergröße wiederherstellen. Position und Arbeitsfläche
    // bestimmt unter Wayland ausschließlich der Compositor — GTK4 bietet
    // dafür bewusst keine API mehr.
    let saved_prefs = load_preferences();
    let mut start_width = saved_prefs.window_width.filter(|w| *w > 0).unwrap_or(560);
    let mut start_height = saved_prefs.window_height.filter(|h| *h > 0).unwrap_or(780);
    // Auf kleinen Bildschirmen (Linux-Smartphones, Issue #12) passt die
    // gespeicherte Desktop-Größe nicht aufs Display: begrenzen und
    // gleich maximiert starten.
    let mut small_screen = false;
    if let Some(display) = gdk::Display::default()
        && let Some(monitor) = display.monitors().item(0).and_downcast::<gdk::Monitor>()
    {
        let geometry = monitor.geometry();
        if geometry.width() > 0 {
            start_width = start_width.min(geometry.width());
            small_screen = geometry.width() < 560;
        }
        if geometry.height() > 0 {
            start_height = start_height.min(geometry.height());
        }
    }
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(t("Reinschrift"))
        .default_width(start_width)
        .default_height(start_height)
        .build();
    if saved_prefs.window_maximized || small_screen {
        window.maximize();
    }

    // Titel darf schrumpfen: auf schmalen Fenstern (Issue #12) würde ein
    // nicht kürzbarer Titel die Kopfzeile über die Fensterbreite drücken.
    let title_label = gtk::Label::builder()
        .label(t("Reinschrift"))
        .ellipsize(pango::EllipsizeMode::End)
        .build();
    let header = adw::HeaderBar::builder()
        .title_widget(&title_label)
        .build();

    let search_entry = gtk::SearchEntry::builder()
        .placeholder_text(t("Search…"))
        .hexpand(true)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(6)
        .build();

    let search_revealer = gtk::Revealer::builder()
        .child(&search_entry)
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .build();

    // Hauptmenü (Hamburger): Einstellungen und Tastenkürzel-Fenster;
    // primary=true öffnet es zusätzlich per F10.
    let menu_model = gio::Menu::new();
    menu_model.append(Some(&t("Settings")), Some("app.open-settings"));
    menu_model.append(Some(&t("Keyboard Shortcuts")), Some("app.shortcuts"));

    let menu_btn = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text(t("Main menu"))
        .menu_model(&menu_model)
        .primary(true)
        .build();
    menu_btn.add_css_class("flat");
    set_a11y_label(&menu_btn, &t("Main menu"));
    header.pack_start(&menu_btn);

    let add_task_btn = gtk::ToggleButton::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text(t("Add"))
        .build();
    add_task_btn.add_css_class("flat");
    set_a11y_label(&add_task_btn, &t("Add"));
    header.pack_end(&add_task_btn);

    let search_btn = gtk::ToggleButton::builder()
        .icon_name("system-search-symbolic")
        .tooltip_text(t("Search…"))
        .build();
    search_btn.add_css_class("flat");
    set_a11y_label(&search_btn, &t("Search…"));
    header.pack_end(&search_btn);

    let refresh_btn = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text(t("Reload (Ctrl+R)"))
        .build();
    refresh_btn.add_css_class("flat");
    set_a11y_label(&refresh_btn, &t("Reload (Ctrl+R)"));
    header.pack_end(&refresh_btn);

    let select_btn = gtk::ToggleButton::builder()
        .icon_name("object-select-symbolic")
        .tooltip_text(t("Select"))
        .build();
    select_btn.add_css_class("flat");
    set_a11y_label(&select_btn, &t("Select"));
    header.pack_end(&select_btn);

    let overlay = adw::ToastOverlay::new();
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);
    let store = gio::ListStore::new::<BoxedAnyObject>();
    let state = Rc::new(AppState::new(&window, &overlay, &store, debug_mode));

    // Fenstergröße und Maximiert-Zustand beim Schließen speichern.
    // default-width/-height verfolgen in GTK4 die aktuelle (unmaximierte)
    // Größe, daher genügt das Auslesen im close_request.
    let state_for_close = Rc::clone(&state);
    window.connect_close_request(move |win| {
        let mut prefs = state_for_close.preferences.borrow_mut();
        prefs.window_width = Some(win.default_width());
        prefs.window_height = Some(win.default_height());
        prefs.window_maximized = win.is_maximized();
        let _ = write_preferences(&prefs);
        glib::Propagation::Proceed
    });

    // Neue To-do Eingabezeile unter den Filtereinstellungen
    let new_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    new_row.set_margin_start(12);
    new_row.set_margin_end(12);
    new_row.set_margin_top(6);
    new_row.set_margin_bottom(6);

    let new_entry = gtk::Entry::new();
    new_entry.set_placeholder_text(Some(&t("New To-do…")));
    new_entry.set_hexpand(true);
    new_row.append(&new_entry);

    let search_btn_for_stop = search_btn.clone();
    let state_for_stop = Rc::clone(&state);
    search_entry.connect_stop_search(move |entry| {
        entry.set_text("");
        *state_for_stop.search_term.borrow_mut() = String::new();
        state_for_stop.repopulate_store();
        search_btn_for_stop.set_active(false);
    });

    let state_for_search = Rc::clone(&state);
    search_entry.connect_search_changed(move |entry| {
        *state_for_search.search_term.borrow_mut() = entry.text().to_string();
        state_for_search.repopulate_store();
    });

    // Enter in der Suche lädt zusätzlich bedeutungsähnliche Treffer nach
    // (nicht bei jedem Tastendruck — Embeddings kosten eine HTTP-Runde).
    let state_for_semantic_search = Rc::clone(&state);
    search_entry.connect_activate(move |entry| {
        let query = entry.text().to_string();
        if !query.trim().is_empty() && state_for_semantic_search.semantic_enabled() {
            state_for_semantic_search.run_semantic_search(query);
        }
    });

    let search_revealer_clone = search_revealer.clone();
    let search_entry_focus = search_entry.clone();
    let add_task_btn_clone = add_task_btn.clone();
    let state_for_search_toggle = Rc::clone(&state);
    search_btn.connect_toggled(move |btn| {
        let active = btn.is_active();
        search_revealer_clone.set_reveal_child(active);
        if active {
            search_entry_focus.grab_focus();
            add_task_btn_clone.set_active(false);
        } else {
            search_entry_focus.set_text("");
            *state_for_search_toggle.search_term.borrow_mut() = String::new();
            state_for_search_toggle.repopulate_store();
        }
    });

    let add_task_btn_for_esc = add_task_btn.clone();
    let new_entry_key_controller = gtk::EventControllerKey::new();
    new_entry_key_controller.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            add_task_btn_for_esc.set_active(false);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    new_entry.add_controller(new_entry_key_controller);

    if state.title_autocomplete_enabled() {
        let provider_state = Rc::clone(&state);
        let title_provider: Rc<dyn Fn() -> Vec<String>> =
            Rc::new(move || provider_state.collect_existing_titles());
        let duplicate_state = Rc::downgrade(&state);
        let on_duplicate: Rc<dyn Fn(String)> = Rc::new(move |title| {
            if let Some(state) = duplicate_state.upgrade() {
                state.duplicate_by_title(&title);
            }
        });
        attach_title_autocomplete_with_duplicate(
            &new_entry,
            title_provider,
            Some(on_duplicate),
            Some(Rc::downgrade(&state)),
        );
    }

    let voice_btn = gtk::Button::builder()
        .icon_name("audio-input-microphone-symbolic")
        .tooltip_text(t("Voice"))
        .css_classes(["flat"])
        .build();
    voice_btn.set_visible(state.use_whisper());
    set_a11y_label(&voice_btn, &t("Voice"));
    new_row.append(&voice_btn);

    let state_for_voice = Rc::clone(&state);
    let voice_btn_clone = voice_btn.clone();
    let new_entry_for_voice = new_entry.clone();
    voice_btn.connect_clicked(move |_| {
        state_for_voice.toggle_recording(&voice_btn_clone, &new_entry_for_voice);
    });

    let add_btn = gtk::Button::with_label(&t("Add"));
    add_btn.add_css_class("suggested-action");
    new_row.append(&add_btn);

    // FlowBox statt Box: auf schmalen Bildschirmen (Issue #12) rutschen die
    // Filter in die nächste Zeile, statt aus dem Fenster zu laufen.
    let controls = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .max_children_per_line(4)
        .min_children_per_line(1)
        .row_spacing(6)
        .column_spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(6)
        .build();

    let sort_label = gtk::Label::builder()
        .label(t("Sort by:"))
        .xalign(0.0)
        .build();
    controls.append(&sort_label);

    let sort_selector = gtk::DropDown::from_strings(&[&t("+ Topics"), &t("@ Locations"), &t("Date")]);
    sort_selector.set_selected(state.sort_mode().to_index());
    set_a11y_label(&sort_selector, &t("Sort by:"));
    controls.append(&sort_selector);

    let filter_button = gtk::MenuButton::builder()
        .label(t("Filter"))
        .tooltip_text(t("Filter"))
        .valign(gtk::Align::Center)
        .build();
    set_a11y_label(&filter_button, &t("Filter"));
    let filter_popover = gtk::Popover::new();
    filter_button.set_popover(Some(&filter_popover));
    // Neu aufbauen bei jedem Öffnen: die Projekt-/Ortsliste folgt der Datei.
    filter_popover.connect_show(clone!(#[weak] state, move |popover| {
        popover.set_child(Some(&build_filter_panel(&state)));
    }));
    controls.append(&filter_button);
    state.filter_button.replace(Some(filter_button));
    state.update_filter_button();

    let myday_filter = gtk::ToggleButton::builder()
        .label(t("My Day"))
        .tooltip_text(t("My Day"))
        .build();
    myday_filter.set_valign(gtk::Align::Center);
    myday_filter.set_active(state.myday_view());
    controls.append(&myday_filter);

    // Eingabezeile + Inline-Duplikatwarnung („Meinst du …?") untereinander;
    // die Warnung blockiert das Hinzufügen nie.
    let duplicate_warning_label = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    duplicate_warning_label.add_css_class("dim-label");
    duplicate_warning_label.set_margin_start(12);
    duplicate_warning_label.set_margin_end(12);
    duplicate_warning_label.set_margin_bottom(6);

    let add_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    add_column.append(&new_row);
    add_column.append(&duplicate_warning_label);

    // Debounced Duplikat-Check beim Tippen (nur offene Aufgaben,
    // konservativer Threshold); degradiert still ohne Ollama.
    {
        let dup_timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
        let state_weak = Rc::downgrade(&state);
        let warning = duplicate_warning_label.clone();
        new_entry.connect_changed(move |entry| {
            if let Some(id) = dup_timer.borrow_mut().take() {
                id.remove();
            }
            warning.set_visible(false);
            let Some(state) = state_weak.upgrade() else {
                return;
            };
            if !state.semantic_enabled() {
                return;
            }
            let typed = entry.text().trim().to_string();
            if typed.chars().count() < 4 {
                return;
            }
            let entry_weak = entry.downgrade();
            let warning = warning.clone();
            let timer_slot = Rc::clone(&dup_timer);
            let timer_done = Rc::clone(&dup_timer);
            let id = glib::timeout_add_local_once(StdDuration::from_millis(600), move || {
                *timer_done.borrow_mut() = None;
                glib::spawn_future_local(async move {
                    let hits = state
                        .semantic_query(typed.clone(), 1, embeddings::DUPLICATE_THRESHOLD, true)
                        .await;
                    let Some(entry) = entry_weak.upgrade() else {
                        return;
                    };
                    if entry.text().trim() != typed {
                        return; // veraltete Antwort
                    }
                    if let Some((item, _score)) = hits.into_iter().next() {
                        warning.set_text(&format!("⚠ {} {}", t("Did you mean …?"), item.title));
                        warning.set_visible(true);
                    }
                });
            });
            *timer_slot.borrow_mut() = Some(id);
        });
    }

    let add_revealer = gtk::Revealer::builder()
        .child(&add_column)
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .build();

    let add_revealer_clone = add_revealer.clone();
    let new_entry_clone = new_entry.clone();
    let search_btn_clone2 = search_btn.clone();
    add_task_btn.connect_toggled(move |btn| {
        let active = btn.is_active();
        add_revealer_clone.set_reveal_child(active);
        if active {
            new_entry_clone.grab_focus();
            search_btn_clone2.set_active(false);
        }
    });

    let state_for_add = Rc::clone(&state);
    let new_entry_for_add = new_entry.clone();
    add_btn.connect_clicked(move |_| {
        state_for_add.handle_add_submission(&new_entry_for_add);
    });

    // Enter im Textfeld soll ebenfalls das To-do anlegen
    let state_for_add2 = Rc::clone(&state);
    let new_entry_for_add2 = new_entry.clone();
    new_entry.connect_activate(move |_| {
        state_for_add2.handle_add_submission(&new_entry_for_add2);
    });

    // Aktionsleiste für den Mehrfachauswahl-Modus (Issue #8)
    let selection_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    selection_row.set_margin_start(12);
    selection_row.set_margin_end(12);
    selection_row.set_margin_top(6);
    selection_row.set_margin_bottom(6);

    let selection_count = gtk::Label::builder()
        .label(t("{} selected").replace("{}", "0"))
        .xalign(0.0)
        .build();
    selection_count.add_css_class("dim-label");
    selection_row.append(&selection_count);

    let selection_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    selection_spacer.set_hexpand(true);
    selection_row.append(&selection_spacer);

    let bulk_complete_btn = gtk::Button::with_label(&t("Complete"));
    selection_row.append(&bulk_complete_btn);

    let bulk_reopen_btn = gtk::Button::with_label(&t("Reopen"));
    selection_row.append(&bulk_reopen_btn);

    // Fälligkeits-Popover mit den vier bekannten Zielen
    let due_popover_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let bulk_due_today_btn = gtk::Button::with_label(&t("Set due date to today"));
    let bulk_due_tomorrow_btn = gtk::Button::with_label(&t("Postpone to tomorrow"));
    let bulk_due_weekend_btn = gtk::Button::with_label(&t("Postpone to weekend"));
    let bulk_due_sometime_btn = gtk::Button::with_label(&t("Postpone to 'sometimes'"));
    for btn in [
        &bulk_due_today_btn,
        &bulk_due_tomorrow_btn,
        &bulk_due_weekend_btn,
        &bulk_due_sometime_btn,
    ] {
        btn.add_css_class("flat");
        due_popover_box.append(btn);
    }
    let due_popover = gtk::Popover::builder().child(&due_popover_box).build();
    let bulk_due_btn = gtk::MenuButton::builder()
        .label(t("Due…"))
        .popover(&due_popover)
        .build();
    selection_row.append(&bulk_due_btn);

    let bulk_assign_btn = gtk::Button::with_label(&t("Assign"));
    selection_row.append(&bulk_assign_btn);

    let bulk_delete_btn = gtk::Button::with_label(&t("Delete"));
    bulk_delete_btn.add_css_class("destructive-action");
    selection_row.append(&bulk_delete_btn);

    let selection_revealer = gtk::Revealer::builder()
        .child(&selection_row)
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .build();

    *state.selection_bar.borrow_mut() = Some(selection_revealer.clone());
    *state.selection_count_label.borrow_mut() = Some(selection_count.clone());
    *state.selection_toggle.borrow_mut() = Some(select_btn.clone());

    select_btn.connect_toggled(clone!(#[weak] state, move |btn| {
        state.set_selection_mode(btn.is_active());
    }));

    bulk_complete_btn.connect_clicked(clone!(#[weak] state, move |_| {
        state.bulk_complete(true);
    }));
    bulk_reopen_btn.connect_clicked(clone!(#[weak] state, move |_| {
        state.bulk_complete(false);
    }));
    bulk_delete_btn.connect_clicked(clone!(#[weak] state, move |_| {
        state.bulk_delete();
    }));
    bulk_assign_btn.connect_clicked(clone!(#[weak] state, move |_| {
        state.bulk_assign();
    }));
    bulk_due_today_btn.connect_clicked(clone!(#[weak] state, #[weak] due_popover, move |_| {
        due_popover.popdown();
        state.bulk_set_due(data::DueTarget::Today);
    }));
    bulk_due_tomorrow_btn.connect_clicked(clone!(#[weak] state, #[weak] due_popover, move |_| {
        due_popover.popdown();
        state.bulk_set_due(data::DueTarget::Tomorrow);
    }));
    bulk_due_weekend_btn.connect_clicked(clone!(#[weak] state, #[weak] due_popover, move |_| {
        due_popover.popdown();
        state.bulk_set_due(data::DueTarget::Weekend);
    }));
    bulk_due_sometime_btn.connect_clicked(clone!(#[weak] state, #[weak] due_popover, move |_| {
        due_popover.popdown();
        state.bulk_set_due(data::DueTarget::Sometime);
    }));

    // Erzeuge das vertikale Content-Layout noch vor dem Einfügen der neuen Zeile
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&controls);
    content.append(&search_revealer);
    content.append(&add_revealer);
    content.append(&selection_revealer);
    content.append(&overlay);

    let list_view = create_list_view(&state);
    *state.list_view.borrow_mut() = Some(list_view.clone());

    // Add keyboard controller to skip headers when navigating
    let nav_controller = gtk::EventControllerKey::new();
    let nav_state = Rc::clone(&state);
    nav_controller.connect_key_pressed(move |_, keyval, _, _| {
        if keyval != gdk::Key::Up && keyval != gdk::Key::Down {
            return glib::Propagation::Proceed;
        }

        let Some(list_view) = nav_state.list_view.borrow().as_ref().cloned() else {
            return glib::Propagation::Proceed;
        };
        let Some(model) = list_view.model() else {
            return glib::Propagation::Proceed;
        };
        let Ok(selection) = model.downcast::<gtk::SingleSelection>() else {
            return glib::Propagation::Proceed;
        };

        let current = selection.selected();
        let n_items = nav_state.store.n_items();
        if n_items == 0 {
            return glib::Propagation::Proceed;
        }

        let direction: i32 = if keyval == gdk::Key::Up { -1 } else { 1 };

        // Start position: from current if valid, otherwise before first/after last
        let start_pos: i32 = if current == gtk::INVALID_LIST_POSITION {
            if direction > 0 { -1 } else { n_items as i32 }
        } else {
            current as i32
        };

        // Find next non-header item
        let mut pos = start_pos + direction;
        loop {
            // Check bounds
            if pos < 0 || pos >= n_items as i32 {
                return glib::Propagation::Stop;
            }

            // Check if this position is an item (not a header);
            // Picker-Zeilen („Mein Tag" planen) sind ebenfalls anwählbar.
            if let Some(obj) = nav_state.store.item(pos as u32)
                && let Ok(boxed) = obj.downcast::<BoxedAnyObject>() {
                    let entry = boxed.borrow::<ListEntry>();
                    if matches!(&*entry, ListEntry::Item(_) | ListEntry::PickerItem(_)) {
                        selection.set_selected(pos as u32);
                        list_view.scroll_to(pos as u32, gtk::ListScrollFlags::NONE, None);
                        return glib::Propagation::Stop;
                    }
                }

            // Move to next position
            pos += direction;
        }
    });
    list_view.add_controller(nav_controller);

    let scrolled = gtk::ScrolledWindow::builder()
        .child(&list_view)
        .vexpand(true)
        .hexpand(true)
        .build();
    *state.scrolled_window.borrow_mut() = Some(scrolled.clone());
    overlay.set_child(Some(&scrolled));

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&content));

    window.set_content(Some(&toolbar_view));

    // ESC-Taste zum Schließen der Revealer, ? für Hilfe, Ctrl+N/F für Aktionen
    let key_controller = gtk::EventControllerKey::new();
    let search_btn_esc = search_btn.clone();
    let add_task_btn_esc = add_task_btn.clone();
    let state_for_keys = Rc::clone(&state);
    key_controller.connect_key_pressed(move |_, key, _, modifiers| {
        let has_ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
        
        if key == gdk::Key::Escape {
            if state_for_keys.selection_mode.get() {
                state_for_keys.set_selection_mode(false);
                return glib::Propagation::Stop;
            }
            search_btn_esc.set_active(false);
            add_task_btn_esc.set_active(false);
            glib::Propagation::Stop
        } else if key == gdk::Key::question && !has_ctrl {
            state_for_keys.show_shortcuts_window();
            glib::Propagation::Stop
        } else if has_ctrl && (key == gdk::Key::n || key == gdk::Key::N) {
            add_task_btn_esc.set_active(!add_task_btn_esc.is_active());
            glib::Propagation::Stop
        } else if has_ctrl && (key == gdk::Key::f || key == gdk::Key::F) {
            search_btn_esc.set_active(!search_btn_esc.is_active());
            glib::Propagation::Stop
        } else if has_ctrl && (key == gdk::Key::z || key == gdk::Key::Z) {
            state_for_keys.enqueue(WriteJob::Undo);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(key_controller);

    // Setze Fokus direkt ins neue Eingabefeld beim Start
    // new_entry.grab_focus();

    // Wenn das Fenster den Fokus erhält, setze den Cursor in das Eingabefeld
    // let new_entry_for_focus = new_entry.clone();
    // window.connect_notify_local(Some("is-active"), move |window, _| {
    //     if window.is_active() {
    //         new_entry_for_focus.grab_focus();
    //     }
    // });

    let refresh_action = gio::SimpleAction::new("reload", None);
    refresh_action.connect_activate(clone!(#[weak] state, move |_, _| {
        state.refresh_in_background();
    }));
    app.add_action(&refresh_action);
    app.set_accels_for_action("app.reload", &["<Primary>r"]);

    let settings_action = gio::SimpleAction::new("open-settings", None);
    let state_for_settings_action = Rc::clone(&state);
    let voice_btn_for_settings = voice_btn.clone();
    settings_action.connect_activate(move |_, _| {
        state_for_settings_action.show_settings_dialog(Some(voice_btn_for_settings.clone()));
    });
    app.add_action(&settings_action);
    app.set_accels_for_action("app.open-settings", &["<Primary>comma"]);

    let shortcuts_action = gio::SimpleAction::new("shortcuts", None);
    let state_for_shortcuts_action = Rc::clone(&state);
    shortcuts_action.connect_activate(move |_, _| {
        state_for_shortcuts_action.show_shortcuts_window();
    });
    app.add_action(&shortcuts_action);
    app.set_accels_for_action("app.shortcuts", &["<Primary>question", "F1"]);

    let close_action = gio::SimpleAction::new("close-window", None);
    let window_for_close = window.clone();
    close_action.connect_activate(move |_, _| {
        window_for_close.close();
    });
    app.add_action(&close_action);
    app.set_accels_for_action("app.close-window", &["<Primary>w", "<Primary>q", "<Alt>F4"]);

    refresh_btn.connect_clicked(clone!(#[weak] app, move |_| {
        app.activate_action("app.reload", None);
    }));

    // Keep state alive for the window lifetime so weak references can upgrade.
    unsafe {
        window.set_data("app-state", state.clone());
    }

    // Kompaktmodus (Issue #12): Linux-Smartphones haben rund 360 px logische
    // Breite. Unter 480 px wechseln die To-do-Zeilen auf ein Überlaufmenü,
    // die Auswahlleiste auf Ikonen und die Filterleiste lässt das Sort-Label weg.
    let bulk_text_buttons: Vec<(gtk::Button, &'static str, String)> = vec![
        (bulk_complete_btn.clone(), "object-select-symbolic", t("Complete")),
        (bulk_reopen_btn.clone(), "edit-undo-symbolic", t("Reopen")),
        (bulk_assign_btn.clone(), "document-edit-symbolic", t("Assign")),
        (bulk_delete_btn.clone(), "user-trash-symbolic", t("Delete")),
    ];
    let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        480.0,
        adw::LengthUnit::Px,
    ));
    let state_apply = Rc::clone(&state);
    let sort_label_apply = sort_label.clone();
    let bulk_apply = bulk_text_buttons.clone();
    let due_apply = bulk_due_btn.clone();
    breakpoint.connect_apply(move |_| {
        state_apply.set_compact(true);
        sort_label_apply.set_visible(false);
        set_bulk_bar_compact(&bulk_apply, &due_apply, true);
    });
    let state_unapply = Rc::clone(&state);
    let sort_label_unapply = sort_label.clone();
    let bulk_unapply = bulk_text_buttons.clone();
    let due_unapply = bulk_due_btn.clone();
    breakpoint.connect_unapply(move |_| {
        state_unapply.set_compact(false);
        sort_label_unapply.set_visible(true);
        set_bulk_bar_compact(&bulk_unapply, &due_unapply, false);
    });
    window.add_breakpoint(breakpoint);

    window.present();

    if let Err(err) = state.reload() {
        let err_msg = err.to_string();
        let mut recovered = false;

        if err_msg == t("No database file configured. Please select or create one in the settings.")
            && let Some(fallback) = ensure_default_database() {
                data::set_todo_path(fallback.clone());
                {
                    let mut prefs = state.preferences.borrow_mut();
                    prefs.db_path = Some(fallback.to_string_lossy().into_owned());
                    let _ = write_preferences(&prefs);
                }
                if state.reload().is_ok() {
                    recovered = true;
                }
            }

        if !recovered {
            let msg = if err_msg == t("No database file configured. Please select or create one in the settings.") {
                err_msg
            } else {
                format!("{}\n{}", t("Could not load To-dos: {}").replace("{}", &err_msg), t("Please select a valid file in settings."))
            };
            state.show_error(&msg);
            state.show_settings_dialog(None);
        }
    }

    state.replay_journal();

    // Embedding-Index im Hintergrund vorwärmen, damit die ersten
    // semantischen Vorschläge nicht den vollen Index-Aufbau abwarten müssen.
    state.warm_semantic_index();

    sort_selector.connect_selected_notify(clone!(#[weak] state, move |dropdown| {
        let mode = SortMode::from_index(dropdown.selected());
        state.set_sort_mode(mode);
    }));

    myday_filter.connect_toggled(clone!(#[weak] state, move |btn| {
        state.set_myday_view(btn.is_active());
    }));

    if let Err(err) = state.install_monitor() {
        state.show_error(&t("File monitoring not available: {}").replace("{}", &err.to_string()));
    }

    schedule_poll(Rc::clone(&state), 10);
    state.schedule_reminder_check();

    Ok(())
}

/// Auswahlleiste im Kompaktmodus (Issue #12): Text-Buttons werden zu
/// ikonenbasierten Buttons mit Tooltip, damit die Leiste auf 360 px passt.
fn set_bulk_bar_compact(
    text_buttons: &[(gtk::Button, &'static str, String)],
    due_button: &gtk::MenuButton,
    compact: bool,
) {
    for (button, icon_name, label) in text_buttons {
        set_a11y_label(button, label);
        if compact {
            button.set_icon_name(icon_name);
            button.set_tooltip_text(Some(label.as_str()));
        } else {
            button.set_label(label);
            button.set_tooltip_text(None);
        }
    }
    let due_label = t("Due…");
    set_a11y_label(due_button, &due_label);
    if compact {
        due_button.set_icon_name("x-office-calendar-symbolic");
        due_button.set_tooltip_text(Some(&due_label));
    } else {
        due_button.set_label(&due_label);
        due_button.set_tooltip_text(None);
    }
}

/// Verdrahtet einen Zeilen-Button mit der Aufgabe der aktuell gebundenen
/// Zeile. Zeilen werden recycelt, deshalb wird das To-do erst beim Klick
/// aufgelöst.
fn wire_row_action<F>(
    button: &gtk::Button,
    list_item: &gtk::ListItem,
    state: &std::rc::Weak<AppState>,
    action: F,
) where
    F: Fn(&Rc<AppState>, &TodoItem) + 'static,
{
    let weak_item = list_item.downgrade();
    let weak_state = state.clone();
    button.connect_clicked(move |_| {
        let Some(list_item) = weak_item.upgrade() else {
            return;
        };
        let Some(obj) = list_item.item() else {
            return;
        };
        let Ok(todo_obj) = obj.downcast::<BoxedAnyObject>() else {
            return;
        };
        let entry = todo_obj.borrow::<ListEntry>();
        let todo = match &*entry {
            ListEntry::Item(todo) => todo.clone(),
            _ => return,
        };
        drop(entry);
        if let Some(state) = weak_state.upgrade() {
            action(&state, &todo);
        }
    });
}

fn create_list_view(state: &Rc<AppState>) -> gtk::ListView {
    let factory = gtk::SignalListItemFactory::new();
    let state_weak = Rc::downgrade(state);
    let factory_state = state_weak.clone();

    factory.connect_setup(move |_, list_item_obj| {
        let Some(list_item) = list_item_obj.downcast_ref::<gtk::ListItem>() else {
            return;
        };

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::None);
        stack.set_hexpand(true);

        // Header row
        let header_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header_box.set_margin_start(12);
        header_box.set_margin_end(12);
        header_box.set_margin_top(8);
        header_box.set_margin_bottom(4);
        let header_label = gtk::Label::builder()
            .xalign(0.0)
            .label("")
            .build();
        header_label.add_css_class("heading");
        header_label.add_css_class("dim-label");
        header_box.append(&header_label);
        stack.add_named(&header_box, Some("header"));

        // Todo row
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        container.set_homogeneous(false);
        container.set_margin_start(12);
        container.set_margin_end(12);
        container.set_margin_top(6);
        container.set_margin_bottom(6);

        // Auswahl-Checkbox für den Mehrfachauswahl-Modus (nur dort sichtbar)
        let select_check = gtk::CheckButton::new();
        select_check.set_valign(gtk::Align::Center);
        select_check.set_visible(false);
        select_check.add_css_class("selection-mode");
        set_a11y_label(&select_check, &t("Select task"));
        container.append(&select_check);

        let check = gtk::CheckButton::new();
        check.set_valign(gtk::Align::Center);
        set_a11y_label(&check, &t("Mark as done"));
        container.append(&check);

        let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let title = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::End)
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .build();
        title.add_css_class("title-4");
        column.append(&title);

        let meta = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .build();
        meta.add_css_class("dim-label");
        column.append(&meta);

        container.append(&column);

        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        container.append(&spacer);

        let myday_btn = gtk::Button::builder()
            .icon_name("starred-symbolic")
            .tooltip_text(t("My Day"))
            .build();
        myday_btn.set_valign(gtk::Align::Center);
        myday_btn.add_css_class("flat");
        set_a11y_label(&myday_btn, &t("My Day"));
        container.append(&myday_btn);

        let today_btn = gtk::Button::builder()
            .icon_name("x-office-calendar-symbolic")
            .tooltip_text(t("Set due date to today"))
            .build();
        today_btn.set_valign(gtk::Align::Center);
        today_btn.add_css_class("flat");
        set_a11y_label(&today_btn, &t("Set due date to today"));
        container.append(&today_btn);

        let tomorrow_btn = gtk::Button::builder()
            .icon_name("go-next-symbolic")
            .tooltip_text(t("Postpone to tomorrow"))
            .build();
        tomorrow_btn.set_valign(gtk::Align::Center);
        tomorrow_btn.add_css_class("flat");
        set_a11y_label(&tomorrow_btn, &t("Postpone to tomorrow"));
        container.append(&tomorrow_btn);

        let weekend_btn = gtk::Button::builder()
            .icon_name("weather-clear-symbolic")
            .tooltip_text(t("Postpone to weekend"))
            .build();
        weekend_btn.set_valign(gtk::Align::Center);
        weekend_btn.add_css_class("flat");
        set_a11y_label(&weekend_btn, &t("Postpone to weekend"));
        container.append(&weekend_btn);

        let sometimes_btn = gtk::Button::builder()
            .icon_name("alarm-symbolic")
            .tooltip_text(t("Postpone to 'sometimes'"))
            .build();
        sometimes_btn.set_valign(gtk::Align::Center);
        sometimes_btn.add_css_class("flat");
        set_a11y_label(&sometimes_btn, &t("Postpone to 'sometimes'"));
        container.append(&sometimes_btn);

        // Kompaktmodus (Issue #12): auf schmalen Fenstern ersetzt ein
        // Überlaufmenü die fünf Schnellaktionen.
        let menu_btn = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text(t("More actions"))
            .build();
        menu_btn.set_valign(gtk::Align::Center);
        menu_btn.add_css_class("flat");
        set_a11y_label(&menu_btn, &t("More actions"));

        let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        menu_box.set_margin_top(6);
        menu_box.set_margin_bottom(6);
        menu_box.set_margin_start(6);
        menu_box.set_margin_end(6);
        let menu_myday_btn = gtk::Button::with_label(&t("My Day"));
        let menu_today_btn = gtk::Button::with_label(&t("Set due date to today"));
        let menu_tomorrow_btn = gtk::Button::with_label(&t("Postpone to tomorrow"));
        let menu_weekend_btn = gtk::Button::with_label(&t("Postpone to weekend"));
        let menu_sometimes_btn = gtk::Button::with_label(&t("Postpone to 'sometimes'"));
        for btn in [
            &menu_myday_btn,
            &menu_today_btn,
            &menu_tomorrow_btn,
            &menu_weekend_btn,
            &menu_sometimes_btn,
        ] {
            btn.add_css_class("flat");
            btn.set_halign(gtk::Align::Fill);
            menu_box.append(btn);
        }
        let menu_popover = gtk::Popover::builder().child(&menu_box).build();
        menu_btn.set_popover(Some(&menu_popover));
        container.append(&menu_btn);

        stack.add_named(&container, Some("item"));

        // Picker row: "Mein Tag" planen — [+]-Button plus Titel/Metadaten
        let picker_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        picker_box.set_margin_start(12);
        picker_box.set_margin_end(12);
        picker_box.set_margin_top(6);
        picker_box.set_margin_bottom(6);

        let picker_add_btn = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text(t("Add to My Day"))
            .build();
        picker_add_btn.set_valign(gtk::Align::Center);
        picker_add_btn.add_css_class("flat");
        set_a11y_label(&picker_add_btn, &t("Add to My Day"));
        picker_box.append(&picker_add_btn);

        let picker_column = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let picker_title = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::End)
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .build();
        picker_column.append(&picker_title);

        let picker_meta = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .build();
        picker_meta.add_css_class("dim-label");
        picker_column.append(&picker_meta);
        picker_box.append(&picker_column);

        stack.add_named(&picker_box, Some("picker"));
        list_item.set_child(Some(&stack));

        // Keyboard shortcuts for list items
        let key_controller = gtk::EventControllerKey::new();
        let state_item_key = factory_state.clone();
        let weak_list_item = list_item.downgrade();
        
        key_controller.connect_key_pressed(move |_, keyval, _, _| {
            let Some(list_item) = weak_list_item.upgrade() else { return glib::Propagation::Proceed; };
            let Some(obj) = list_item.item() else { return glib::Propagation::Proceed; };
            let Ok(todo_obj) = obj.downcast::<BoxedAnyObject>() else { return glib::Propagation::Proceed; };
            let entry = todo_obj.borrow::<ListEntry>();
            let Some(state) = state_item_key.upgrade() else { return glib::Propagation::Proceed; };
            let todo = match &*entry {
                ListEntry::Item(todo) => todo.clone(),
                // Im Planungs-Picker übernimmt die Leertaste in „Mein Tag"
                // (wie das Plus); Enter öffnet den Bearbeiten-Dialog.
                ListEntry::PickerItem(todo) if keyval == gdk::Key::space => {
                    state.toggle_myday(todo);
                    return glib::Propagation::Stop;
                }
                _ => return glib::Propagation::Proceed,
            };

            // Im Auswahlmodus schaltet die Leertaste die Auswahl um;
            // andere Schnellaktionen sind dort deaktiviert.
            if state.selection_mode.get() {
                if keyval == gdk::Key::space {
                    state.toggle_selection_at(list_item.position());
                    return glib::Propagation::Stop;
                }
                return glib::Propagation::Proceed;
            }

            let unicode = keyval.to_unicode();
            match keyval {
                gdk::Key::space => {
                    state.toggle_item(&todo, !todo.done);
                    glib::Propagation::Stop
                }
                gdk::Key::Delete | gdk::Key::KP_Delete => {
                    state.request_delete(&todo);
                    glib::Propagation::Stop
                }
                _ if unicode == Some('h') || unicode == Some('H') => {
                    state.set_due_today(&todo);
                    glib::Propagation::Stop
                }
                _ if unicode == Some('m') || unicode == Some('M') => {
                    state.set_due_tomorrow(&todo);
                    glib::Propagation::Stop
                }
                _ if unicode == Some('w') || unicode == Some('W') => {
                    state.set_due_weekend(&todo);
                    glib::Propagation::Stop
                }
                _ if unicode == Some('l') || unicode == Some('L') => {
                    state.set_due_in_days(&todo, 7);
                    glib::Propagation::Stop
                }
                _ if unicode == Some('s') || unicode == Some('S') => {
                    state.set_due_sometimes(&todo);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        stack.add_controller(key_controller);

        unsafe {
            list_item.set_data("stack", stack.downgrade());
            list_item.set_data("header-label", header_label.downgrade());
            list_item.set_data("todo-check", check.downgrade());
            list_item.set_data("select-check", select_check.downgrade());
            list_item.set_data("todo-title", title.downgrade());
            list_item.set_data("todo-meta", meta.downgrade());
            list_item.set_data("todo-button", tomorrow_btn.downgrade());
            list_item.set_data("todo-menu-btn", menu_btn.downgrade());
            list_item.set_data("todo-myday-btn", myday_btn.downgrade());
            list_item.set_data("todo-today-btn", today_btn.downgrade());
            list_item.set_data("todo-weekend-btn", weekend_btn.downgrade());
            list_item.set_data("todo-sometimes-btn", sometimes_btn.downgrade());
            list_item.set_data("picker-title", picker_title.downgrade());
            list_item.set_data("picker-meta", picker_meta.downgrade());
        }

        // Auswahl-Checkbox: Marker in die Auswahl aufnehmen/entfernen
        let select_list = list_item.downgrade();
        let select_state = factory_state.clone();
        select_check.connect_toggled(move |btn| {
            let Some(list_item) = select_list.upgrade() else {
                return;
            };
            let Some(obj) = list_item.item() else {
                return;
            };
            let Ok(todo_obj) = obj.downcast::<BoxedAnyObject>() else {
                return;
            };
            let entry = todo_obj.borrow::<ListEntry>();
            let todo = match &*entry {
                ListEntry::Item(todo) => todo.clone(),
                _ => return,
            };
            drop(entry);

            // Zeilen-Hervorhebung direkt am Stack pflegen
            if let Some(stack) = btn
                .ancestor(gtk::Stack::static_type())
                .and_downcast::<gtk::Stack>()
            {
                if btn.is_active() {
                    stack.add_css_class("selected-row");
                } else {
                    stack.remove_css_class("selected-row");
                }
            }

            let Some(state) = select_state.upgrade() else {
                return;
            };
            if !state.selection_mode.get() {
                return;
            }
            let Some(marker) = todo.key.marker.as_deref() else {
                return;
            };
            let is_selected = state.selected_markers.borrow().contains(marker);
            if btn.is_active() == is_selected {
                return;
            }
            state.toggle_selection_marker(marker, btn.is_active());
        });

        let weak_list = list_item.downgrade();
        let state_for_handler = factory_state.clone();
        check.connect_toggled(move |btn| {
            let Some(list_item) = weak_list.upgrade() else {
                return;
            };
            let Some(obj) = list_item.item() else {
                return;
            };
            let Ok(todo_obj) = obj.downcast::<BoxedAnyObject>() else {
                return;
            };
            let entry = todo_obj.borrow::<ListEntry>();
            let todo = match &*entry {
                ListEntry::Item(todo) => todo.clone(),
                _ => return,
            };
            if btn.is_active() == todo.done {
                return;
            }

            if let Some(state) = state_for_handler.upgrade() {
                if state.selection_mode.get() {
                    return;
                }
                state.toggle_item(&todo, btn.is_active());
            }
        });

        let myday_list = list_item.downgrade();
        let myday_state = factory_state.clone();
        myday_btn.connect_clicked(move |_| {
            let Some(list_item) = myday_list.upgrade() else {
                return;
            };
            let Some(obj) = list_item.item() else {
                return;
            };
            let Ok(todo_obj) = obj.downcast::<BoxedAnyObject>() else {
                return;
            };
            let entry = todo_obj.borrow::<ListEntry>();
            let todo = match &*entry {
                ListEntry::Item(todo) => todo.clone(),
                _ => return,
            };

            if let Some(state) = myday_state.upgrade() {
                state.toggle_myday(&todo);
            }
        });

        let tomorrow_list = list_item.downgrade();
        let tomorrow_state = factory_state.clone();
        tomorrow_btn.connect_clicked(move |_| {
            let Some(list_item) = tomorrow_list.upgrade() else {
                return;
            };
            let Some(obj) = list_item.item() else {
                return;
            };
            let Ok(todo_obj) = obj.downcast::<BoxedAnyObject>() else {
                return;
            };
            let entry = todo_obj.borrow::<ListEntry>();
            let todo = match &*entry {
                ListEntry::Item(todo) => todo.clone(),
                _ => return,
            };

            if let Some(state) = tomorrow_state.upgrade() {
                state.set_due_tomorrow(&todo);
            }
        });

        let weekend_list = list_item.downgrade();
        let weekend_state = factory_state.clone();
        weekend_btn.connect_clicked(move |_| {
            let Some(list_item) = weekend_list.upgrade() else {
                return;
            };
            let Some(obj) = list_item.item() else {
                return;
            };
            let Ok(todo_obj) = obj.downcast::<BoxedAnyObject>() else {
                return;
            };
            let entry = todo_obj.borrow::<ListEntry>();
            let todo = match &*entry {
                ListEntry::Item(todo) => todo.clone(),
                _ => return,
            };

            if let Some(state) = weekend_state.upgrade() {
                state.set_due_weekend(&todo);
            }
        });

        let today_list = list_item.downgrade();
        let today_state = factory_state.clone();
        today_btn.connect_clicked(move |_| {
            let Some(list_item) = today_list.upgrade() else {
                return;
            };
            let Some(obj) = list_item.item() else {
                return;
            };
            let Ok(todo_obj) = obj.downcast::<BoxedAnyObject>() else {
                return;
            };
            let entry = todo_obj.borrow::<ListEntry>();
            let todo = match &*entry {
                ListEntry::Item(todo) => todo.clone(),
                _ => return,
            };

            if let Some(state) = today_state.upgrade() {
                state.set_due_today(&todo);
            }
        });

        let sometimes_list = list_item.downgrade();
        let sometimes_state = factory_state.clone();
        sometimes_btn.connect_clicked(move |_| {
            let Some(list_item) = sometimes_list.upgrade() else {
                return;
            };
            let Some(obj) = list_item.item() else {
                return;
            };
            let Ok(todo_obj) = obj.downcast::<BoxedAnyObject>() else {
                return;
            };
            let entry = todo_obj.borrow::<ListEntry>();
            let todo = match &*entry {
                ListEntry::Item(todo) => todo.clone(),
                _ => return,
            };

            if let Some(state) = sometimes_state.upgrade() {
                state.set_due_sometimes(&todo);
            }
        });

        wire_row_action(&menu_myday_btn, list_item, &factory_state, |state, todo| {
            state.toggle_myday(todo);
        });
        wire_row_action(&menu_today_btn, list_item, &factory_state, |state, todo| {
            state.set_due_today(todo);
        });
        wire_row_action(&menu_tomorrow_btn, list_item, &factory_state, |state, todo| {
            state.set_due_tomorrow(todo);
        });
        wire_row_action(&menu_weekend_btn, list_item, &factory_state, |state, todo| {
            state.set_due_weekend(todo);
        });
        wire_row_action(&menu_sometimes_btn, list_item, &factory_state, |state, todo| {
            state.set_due_sometimes(todo);
        });

        let picker_list = list_item.downgrade();
        let picker_state = factory_state.clone();
        picker_add_btn.connect_clicked(move |_| {
            let Some(list_item) = picker_list.upgrade() else {
                return;
            };
            let Some(obj) = list_item.item() else {
                return;
            };
            let Ok(todo_obj) = obj.downcast::<BoxedAnyObject>() else {
                return;
            };
            let entry = todo_obj.borrow::<ListEntry>();
            let todo = match &*entry {
                ListEntry::PickerItem(todo) => todo.clone(),
                _ => return,
            };

            if let Some(state) = picker_state.upgrade() {
                state.toggle_myday(&todo);
            }
        });

    });

    let highlight_state = state_weak.clone();
    factory.connect_bind(move |_, list_item_obj| {
        let Some(list_item) = list_item_obj.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(obj) = list_item.item() else {
            return;
        };
        let Ok(todo_obj) = obj.downcast::<BoxedAnyObject>() else {
            return;
        };
        let entry = todo_obj.borrow::<ListEntry>();
        // Überschriften weder hervorheben noch aktivieren; ListItems werden
        // wiederverwendet, daher bei jedem Bind neu setzen.
        let is_header = matches!(&*entry, ListEntry::Header(_));
        list_item.set_selectable(!is_header);
        list_item.set_activatable(!is_header);
        let Some(stack_ref_ptr) = (unsafe { list_item.data::<glib::WeakRef<gtk::Stack>>("stack") }) else {
            return;
        };
        let Some(stack) = unsafe { stack_ref_ptr.as_ref() }.upgrade() else {
            return;
        };

        // Für `visible_rows`: welcher Eintrag gerade in dieser Zeile steht.
        unsafe { stack.set_data(ROW_ENTRY_KEY, todo_obj.clone()) };

        let highlight_marker = highlight_state
            .upgrade()
            .and_then(|s| s.recently_updated.borrow().clone());

        match &*entry {
            ListEntry::Header(label) => {
                stack.set_visible_child_name("header");
                stack.remove_css_class("selected-row");
                if let Some(header_ref_ptr) = unsafe {
                    list_item.data::<glib::WeakRef<gtk::Label>>("header-label")
                }
                    && let Some(header_label) = unsafe { header_ref_ptr.as_ref() }.upgrade() {
                        header_label.set_text(label);
                    }
            }
            ListEntry::Item(todo) => {
                stack.set_visible_child_name("item");
                let is_highlighted = todo
                    .key
                    .marker
                    .as_ref()
                    .map(|m| highlight_marker.as_deref() == Some(m.as_str()))
                    .unwrap_or(false);

                if is_highlighted {
                    stack.add_css_class("pulse");
                } else {
                    stack.remove_css_class("pulse");
                }

                // Mehrfachauswahl: Sichtbarkeiten in BEIDEN Modi explizit
                // setzen, da Zeilen recycelt werden.
                let (selection_mode, is_selected) = highlight_state
                    .upgrade()
                    .map(|s| {
                        let mode = s.selection_mode.get();
                        let sel = mode
                            && todo
                                .key
                                .marker
                                .as_deref()
                                .map(|m| s.selected_markers.borrow().contains(m))
                                .unwrap_or(false);
                        (mode, sel)
                    })
                    .unwrap_or((false, false));

                let compact = highlight_state
                    .upgrade()
                    .map(|s| s.compact.get())
                    .unwrap_or(false);

                if is_selected {
                    stack.add_css_class("selected-row");
                } else {
                    stack.remove_css_class("selected-row");
                }

                if let Some(select_ref_ptr) = unsafe {
                    list_item.data::<glib::WeakRef<gtk::CheckButton>>("select-check")
                }
                    && let Some(select_widget) = unsafe { select_ref_ptr.as_ref() }.upgrade() {
                        select_widget.set_visible(selection_mode);
                        if select_widget.is_active() != is_selected {
                            select_widget.set_active(is_selected);
                        }
                    }

                for button_key in [
                    "todo-myday-btn",
                    "todo-today-btn",
                    "todo-button",
                    "todo-weekend-btn",
                    "todo-sometimes-btn",
                ] {
                    if let Some(btn_ref_ptr) = unsafe {
                        list_item.data::<glib::WeakRef<gtk::Button>>(button_key)
                    }
                        && let Some(btn) = unsafe { btn_ref_ptr.as_ref() }.upgrade() {
                            btn.set_visible(!selection_mode && !compact);
                        }
                }

                if let Some(menu_ref_ptr) = unsafe {
                    list_item.data::<glib::WeakRef<gtk::MenuButton>>("todo-menu-btn")
                }
                    && let Some(menu_widget) = unsafe { menu_ref_ptr.as_ref() }.upgrade() {
                        menu_widget.set_visible(!selection_mode && compact);
                        if compact {
                            menu_widget.add_css_class("compact-touch");
                        } else {
                            menu_widget.remove_css_class("compact-touch");
                        }
                    }

                if let Some(check_ref_ptr) = unsafe {
                    list_item.data::<glib::WeakRef<gtk::CheckButton>>("todo-check")
                }
                    && let Some(check_widget) = unsafe { check_ref_ptr.as_ref() }.upgrade() {
                        check_widget.set_visible(!selection_mode);
                        if compact {
                            check_widget.add_css_class("compact-touch");
                        } else {
                            check_widget.remove_css_class("compact-touch");
                        }
                        if check_widget.is_active() != todo.done {
                            check_widget.set_active(todo.done);
                        }
                    }
                if let Some(title_ref_ptr) = unsafe {
                    list_item.data::<glib::WeakRef<gtk::Label>>("todo-title")
                }
                    && let Some(title_widget) = unsafe { title_ref_ptr.as_ref() }.upgrade() {
                        title_widget.set_text(&todo.title);
                        if todo.done {
                            title_widget.add_css_class("dim-label");
                        } else {
                            title_widget.remove_css_class("dim-label");
                        }
                        // In "Mein Tag" bleiben erledigte Aufgaben (auch
                        // wiederkehrende, deren nächste Instanz sofort wieder
                        // aktiv auftaucht) durchgestrichen sichtbar.
                        let in_myday = highlight_state.upgrade().map(|s| s.myday_view()).unwrap_or(false);
                        if todo.done && in_myday {
                            let attrs = pango::AttrList::new();
                            attrs.insert(pango::AttrInt::new_strikethrough(true));
                            title_widget.set_attributes(Some(&attrs));
                        } else {
                            title_widget.set_attributes(None);
                        }
                    }
                if let Some(meta_ref_ptr) = unsafe {
                    list_item.data::<glib::WeakRef<gtk::Label>>("todo-meta")
                }
                    && let Some(meta_widget) = unsafe { meta_ref_ptr.as_ref() }.upgrade() {
                        meta_widget.set_text(&format_metadata(todo));
                    }
            }
            ListEntry::PickerItem(todo) => {
                stack.set_visible_child_name("picker");
                stack.remove_css_class("pulse");
                stack.remove_css_class("selected-row");

                if let Some(title_ref_ptr) = unsafe {
                    list_item.data::<glib::WeakRef<gtk::Label>>("picker-title")
                }
                    && let Some(title_widget) = unsafe { title_ref_ptr.as_ref() }.upgrade() {
                        title_widget.set_text(&todo.title);
                    }
                if let Some(meta_ref_ptr) = unsafe {
                    list_item.data::<glib::WeakRef<gtk::Label>>("picker-meta")
                }
                    && let Some(meta_widget) = unsafe { meta_ref_ptr.as_ref() }.upgrade() {
                        meta_widget.set_text(&format_metadata(todo));
                    }
            }
        }
    });

    let model = gtk::SingleSelection::new(Some(state.store()));
    model.set_autoselect(false);
    model.set_can_unselect(true);
    let list_view = gtk::ListView::new(Some(model.clone()), Some(factory));
    list_view.set_single_click_activate(true);
    // single-click-activate wählt die Zeile unter dem Zeiger aus; ohne das
    // bliebe die Hervorhebung stehen, nachdem die Maus die Liste verlassen hat.
    let motion = gtk::EventControllerMotion::new();
    motion.connect_leave(move |_| model.set_selected(gtk::INVALID_LIST_POSITION));
    list_view.add_controller(motion);
    let activate_state = state_weak.clone();
    list_view.connect_activate(move |_, position| {
        if let Some(state) = activate_state.upgrade() {
            state.open_entry_at(position);
        }
    });
    list_view
}

/// Die Zeile der ListView fokussieren, die die y-Position `y` überdeckt.
fn focus_row_at(list_view: &gtk::ListView, y: f32) {
    let mut child = list_view.first_child();
    while let Some(row) = child {
        child = row.next_sibling();
        if !row.is_child_visible() || !row.is_focusable() {
            continue;
        }
        let Some(top) = row.compute_point(list_view, &gtk::graphene::Point::new(0.0, 0.0)) else {
            continue;
        };
        if top.y() <= y && y < top.y() + row.height() as f32 {
            row.grab_focus();
            return;
        }
    }
}

/// Die gerade sichtbaren Zeilen von oben nach unten, mit ihrem Objekt aus
/// dem Store und ihrer y-Position relativ zur ListView.
fn visible_rows(list_view: &gtk::ListView) -> Vec<(BoxedAnyObject, f32)> {
    if !list_view.is_mapped() {
        return Vec::new();
    }
    let height = list_view.height() as f32;
    let mut rows = Vec::new();
    let mut child = list_view.first_child();
    while let Some(row) = child {
        child = row.next_sibling();
        if !row.is_child_visible() {
            continue;
        }
        let Some(stack) = row.first_child() else { continue };
        let Some(obj) = (unsafe { stack.data::<BoxedAnyObject>(ROW_ENTRY_KEY) })
            .map(|ptr| unsafe { ptr.as_ref() }.clone())
        else {
            continue;
        };
        let Some(top) = row.compute_point(list_view, &gtk::graphene::Point::new(0.0, 0.0)) else {
            continue;
        };
        let y = top.y();
        if y + row.height() as f32 > 0.0 && y < height {
            rows.push((obj, y));
        }
    }
    rows.sort_by(|a, b| a.1.total_cmp(&b.1));
    rows
}

struct AppState {
    store: gio::ListStore,
    overlay: adw::ToastOverlay,
    monitor: RefCell<Option<gio::FileMonitor>>,
    /// Was die Liste anzeigt: `confirmed_items` plus alle noch nicht
    /// gespeicherten Änderungen (siehe `rebuild_view`).
    cached_items: RefCell<Vec<TodoItem>>,
    /// Zuletzt gelesener Dateistand, fortgeschrieben um erfolgreich
    /// gespeicherte Änderungen bis zum nächsten Reload.
    confirmed_items: RefCell<Vec<TodoItem>>,
    last_fingerprint: RefCell<Option<String>>,
    /// Wartende Schreibaufträge, in Klick-Reihenfolge.
    write_queue: RefCell<VecDeque<WriteJob>>,
    /// Der Auftrag, der gerade im Hintergrund geschrieben wird.
    write_in_flight: RefCell<Option<WriteJob>>,
    /// Zählt fertige Schreibaufträge; ein Reload, der einen davon verpasst
    /// haben kann, wird verworfen.
    writes_completed: Cell<u64>,
    /// Hält die Anwendung am Leben, solange geschrieben wird.
    write_hold: RefCell<Option<gio::ApplicationHoldGuard>>,
    journal: Rc<RefCell<data::Journal>>,
    /// Marker neuer Aufgaben, die beim Speichern ausgetauscht werden mussten.
    marker_renames: RefCell<HashMap<String, String>>,
    sort_mode: RefCell<SortMode>,
    window: glib::WeakRef<adw::ApplicationWindow>,
    preferences: RefCell<Preferences>,
    search_term: RefCell<String>,
    list_view: RefCell<Option<gtk::ListView>>,
    scrolled_window: RefCell<Option<gtk::ScrolledWindow>>,
    is_recording: Arc<AtomicBool>,
    ai_runtime: Arc<Runtime>,
    recently_updated: RefCell<Option<String>>,
    notified_items: RefCell<HashSet<String>>,
    /// Mehrfachauswahl-Modus (Issue #8): Klicks selektieren statt zu bearbeiten.
    selection_mode: Cell<bool>,
    /// Kompaktmodus (Issue #12): schmale Fenster (Linux-Smartphones) zeigen
    /// die Schnellaktionen der Zeilen in einem Überlaufmenü.
    compact: Cell<bool>,
    /// Auswahl per Marker — line_index verschiebt sich bei jedem Reload.
    selected_markers: RefCell<HashSet<String>>,
    /// Stand von `RowContext` beim letzten `apply_entries`.
    row_context: RefCell<Option<RowContext>>,
    selection_bar: RefCell<Option<gtk::Revealer>>,
    selection_count_label: RefCell<Option<gtk::Label>>,
    selection_toggle: RefCell<Option<gtk::ToggleButton>>,
    /// Filter-Knopf in der Kopfzeile; sein Label zeigt die Zahl aktiver Filter.
    filter_button: RefCell<Option<gtk::MenuButton>>,
    /// Lazy geladener Embedding-Index (semantische Vorschläge); wird bei
    /// Fingerprint- oder Modellwechsel transparent neu aufgebaut.
    embedding_cache: RefCell<Option<Rc<embeddings::EmbeddingCache>>>,
    /// Wache gegen parallele Index-Builds: während ein Build läuft, liefern
    /// weitere Anfragen den (ggf. veralteten) Cache, statt pro Tastendruck
    /// einen weiteren vollen Rebuild gegen Ollama zu starten.
    embedding_index_building: Cell<bool>,
    /// Memoisierte Query-Vektoren (Modell + kanonischer Text → Vektor);
    /// spart die HTTP-Runde bei wiederholten/zurückgenommenen Eingaben.
    query_embed_cache: RefCell<HashMap<String, Vec<f32>>>,
    _debug_mode: bool,
}

impl AppState {
    fn new(window: &adw::ApplicationWindow, overlay: &adw::ToastOverlay, store: &gio::ListStore, debug_mode: bool) -> Self {
        let current_at_start = data::todo_path();
        let mut prefs = load_preferences();
        let sort_mode = prefs
            .sort_mode
            .as_deref()
            .map(SortMode::from_key)
            .unwrap_or(SortMode::Topic);
        prefs.sort_mode = Some(sort_mode.as_key().to_string());

        prefs.ai_timeout_secs = prefs.ai_timeout_secs.clamp(5, 120);

        let ai_runtime = Arc::new(Runtime::new().expect("failed to create tokio runtime"));

        if prefs.use_webdav {
             if let Some(url) = &prefs.webdav_url {
                 data::set_backend_config(data::BackendConfig::WebDav {
                     url: url.clone(),
                     path: prefs.webdav_path.clone(),
                     username: prefs.webdav_username.clone(),
                     password: prefs.webdav_password.clone(),
                 });
             }
        } else {
            let default_path = data::default_todo_path();
            
            if !current_at_start.as_os_str().is_empty() && current_at_start != default_path {
                // Command line argument was used
                prefs.db_path = Some(current_at_start.to_string_lossy().into_owned());
            } else if let Some(db_path) = prefs.db_path.clone() {
                // No command line argument, use saved preference
                data::set_todo_path(PathBuf::from(db_path));
            } else if !current_at_start.as_os_str().is_empty() {
                // No command line and no preference, use default
                prefs.db_path = Some(current_at_start.to_string_lossy().into_owned());
            }
        }

        if !prefs.use_whisper {
            let mut model_path = glib::user_cache_dir();
            model_path.push("reinschrift_todo");
            model_path.push("ggml-small.bin");
            if model_path.exists() {
                let _ = fs::remove_file(model_path);
            }
        }

        Self {
            store: store.clone(),
            overlay: overlay.clone(),
            monitor: RefCell::new(None),
            cached_items: RefCell::new(Vec::new()),
            sort_mode: RefCell::new(sort_mode),
            window: window.downgrade(),
            preferences: RefCell::new(prefs),
            search_term: RefCell::new(String::new()),
            list_view: RefCell::new(None),
            scrolled_window: RefCell::new(None),
            is_recording: Arc::new(AtomicBool::new(false)),
            ai_runtime: ai_runtime.clone(),
            _debug_mode: debug_mode,
            recently_updated: RefCell::new(None),
            notified_items: RefCell::new(HashSet::new()),
            selection_mode: Cell::new(false),
            compact: Cell::new(false),
            selected_markers: RefCell::new(HashSet::new()),
            row_context: RefCell::new(None),
            selection_bar: RefCell::new(None),
            selection_count_label: RefCell::new(None),
            selection_toggle: RefCell::new(None),
            filter_button: RefCell::new(None),
            embedding_cache: RefCell::new(None),
            embedding_index_building: Cell::new(false),
            query_embed_cache: RefCell::new(HashMap::new()),
            last_fingerprint: RefCell::new(None),
            confirmed_items: RefCell::new(Vec::new()),
            write_queue: RefCell::new(VecDeque::new()),
            write_in_flight: RefCell::new(None),
            writes_completed: Cell::new(0),
            write_hold: RefCell::new(None),
            journal: JOURNAL.with(Rc::clone),
            marker_renames: RefCell::new(HashMap::new()),
        }
    }

    fn collect_existing_titles(&self) -> Vec<String> {
        use std::collections::HashMap;
        let items = self.cached_items.borrow();
        let mut counts: HashMap<String, usize> = HashMap::new();
        for item in items.iter() {
            let t = item.title.trim();
            if !t.is_empty() {
                *counts.entry(t.to_string()).or_insert(0) += 1;
            }
        }
        let mut titles: Vec<(String, usize)> = counts.into_iter().collect();
        titles.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        titles.into_iter().map(|(t, _)| t).collect()
    }

    fn title_autocomplete_enabled(&self) -> bool {
        self.preferences.borrow().title_autocomplete_enabled
    }

    fn collect_existing_tags(&self) -> (Vec<String>, Vec<String>) {
        let (canon_projects, canon_contexts) = self.canonical_tag_maps();
        let items = self.cached_items.borrow();

        // Count project frequencies (case-insensitive: variants aggregate)
        let mut project_counts: HashMap<String, usize> = HashMap::new();
        for item in items.iter() {
            for project in &item.projects {
                *project_counts.entry(project.to_lowercase()).or_insert(0) += 1;
            }
        }

        // Count context frequencies (case-insensitive: variants aggregate)
        let mut context_counts: HashMap<String, usize> = HashMap::new();
        for item in items.iter() {
            for context in &item.contexts {
                *context_counts.entry(context.to_lowercase()).or_insert(0) += 1;
            }
        }

        // Sort by frequency (descending) and take top 10, shown in the most
        // frequently used casing
        let mut projects: Vec<_> = project_counts.into_iter().collect();
        projects.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let projects: Vec<String> = projects
            .into_iter()
            .take(10)
            .map(|(k, _)| canonicalize_token(&canon_projects, &k))
            .collect();

        let mut contexts: Vec<_> = context_counts.into_iter().collect();
        contexts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let contexts: Vec<String> = contexts
            .into_iter()
            .take(10)
            .map(|(k, _)| canonicalize_token(&canon_contexts, &k))
            .collect();

        (projects, contexts)
    }

    /// Collect the recurring project/context tags to offer the AI as the
    /// existing structure it should reuse. Unlike [`collect_existing_tags`]
    /// (used for manual autocomplete, where even one-off tags are helpful),
    /// this drops one-off tags (count < 2, mostly typos/ad-hoc noise) and caps
    /// at the top 25 by frequency — so genuinely reused but less frequent tags
    /// (e.g. +Einkaufen) are still offered, while noise is kept out. Returned
    /// in the most frequently used casing.
    fn collect_ai_tag_hints(&self) -> (Vec<String>, Vec<String>) {
        const MAX_TAGS: usize = 25;
        const MIN_TAG_COUNT: usize = 2;

        let (canon_projects, canon_contexts) = self.canonical_tag_maps();
        let items = self.cached_items.borrow();

        let mut project_counts: HashMap<String, usize> = HashMap::new();
        let mut context_counts: HashMap<String, usize> = HashMap::new();
        for item in items.iter() {
            for project in &item.projects {
                *project_counts.entry(project.to_lowercase()).or_insert(0) += 1;
            }
            for context in &item.contexts {
                *context_counts.entry(context.to_lowercase()).or_insert(0) += 1;
            }
        }

        let rank = |counts: HashMap<String, usize>, canon: &HashMap<String, String>| {
            let mut tags: Vec<_> = counts.into_iter().collect();
            tags.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            tags.into_iter()
                .filter(|(_, count)| *count >= MIN_TAG_COUNT)
                .take(MAX_TAGS)
                .map(|(k, _)| canonicalize_token(canon, &k))
                .collect::<Vec<String>>()
        };

        (
            rank(project_counts, &canon_projects),
            rank(context_counts, &canon_contexts),
        )
    }

    fn store(&self) -> gio::ListStore {
        self.store.clone()
    }

    fn sort_mode(&self) -> SortMode {
        *self.sort_mode.borrow()
    }

    fn show_completed(&self) -> bool {
        self.preferences.borrow().show_done
    }

    fn filter(&self) -> TodoFilter {
        self.preferences.borrow().effective_filter()
    }

    fn myday_view(&self) -> bool {
        self.preferences.borrow().myday_view
    }

    fn skip_delete_confirmation(&self) -> bool {
        self.preferences.borrow().skip_delete_confirmation
    }

    fn use_whisper(&self) -> bool {
        self.preferences.borrow().use_whisper
    }

    fn use_ai_on_new_topic(&self) -> bool {
        self.preferences.borrow().use_ai_on_new_topic
    }

    fn ai_timeout_secs(&self) -> u64 {
        self.preferences.borrow().ai_timeout_secs
    }

    fn ollama_url(&self) -> String {
        self.preferences
            .borrow()
            .ollama_url
            .clone()
            .unwrap_or_else(|| "http://localhost:11434".to_string())
    }

    fn ollama_model(&self) -> String {
        self.preferences
            .borrow()
            .ollama_model
            .clone()
            .unwrap_or_else(|| "llama3.1:8b".to_string())
    }

    fn semantic_enabled(&self) -> bool {
        self.preferences.borrow().semantic_enabled
    }

    fn embedding_model(&self) -> String {
        self.preferences
            .borrow()
            .embedding_model
            .clone()
            .unwrap_or_else(|| embeddings::DEFAULT_EMBEDDING_MODEL.to_string())
    }

    fn embedding_cache_path(&self) -> PathBuf {
        let mut dir = glib::user_cache_dir();
        dir.push("reinschrift_todo");
        dir.push("embeddings.json");
        dir
    }

    fn embedding_config(&self) -> embeddings::EmbeddingConfig {
        embeddings::EmbeddingConfig {
            ollama_url: self.ollama_url(),
            model: self.embedding_model(),
            timeout_secs: self.ai_timeout_secs(),
        }
    }

    /// Lazy aufgebauter Embedding-Index. Baut bei Fingerprint- oder
    /// Modellwechsel inkrementell neu (blocking HTTP/IO im Tokio-Pool);
    /// liefert `None` bei deaktiviertem Feature oder Backend-Fehler —
    /// alle semantischen Features degradieren dann still.
    async fn semantic_index(self: &Rc<Self>) -> Option<Rc<embeddings::EmbeddingCache>> {
        if !self.semantic_enabled() {
            return None;
        }
        let model = self.embedding_model();
        let fingerprint = self.last_fingerprint.borrow().clone().unwrap_or_default();
        if let Some(cache) = self.embedding_cache.borrow().as_ref()
            && cache.db_fingerprint == fingerprint && cache.model == model {
                return Some(Rc::clone(cache));
            }
        // Ein Build läuft bereits (früherer Tastendruck oder Vorwärmen):
        // veralteten Cache nutzen statt einen zweiten parallelen vollen
        // Rebuild gegen Ollama zu starten.
        if self.embedding_index_building.get() {
            return self.embedding_cache.borrow().as_ref().map(Rc::clone);
        }
        let items = self.cached_items.borrow().clone();
        let cfg = self.embedding_config();
        let path = self.embedding_cache_path();
        let fp = fingerprint.clone();
        self.embedding_index_building.set(true);
        let result = self
            .ai_runtime
            .spawn_blocking(move || embeddings::ensure_index(&path, &cfg, &items, &fp))
            .await;
        self.embedding_index_building.set(false);
        match result {
            Ok(Ok(cache)) => {
                let cache = Rc::new(cache);
                *self.embedding_cache.borrow_mut() = Some(Rc::clone(&cache));
                Some(cache)
            }
            Ok(Err(err)) => {
                eprintln!("semantic index unavailable: {err}");
                None
            }
            Err(_) => None,
        }
    }

    /// Wärmt den Embedding-Index im Hintergrund vor, damit die erste
    /// semantische Anfrage nur noch die Query-Embedding-Runde bezahlt und
    /// nicht den vollen Index-Aufbau (der bei großer Datenbank Sekunden
    /// dauern kann und die Vorschläge dann als veraltet verworfen würden).
    fn warm_semantic_index(self: &Rc<Self>) {
        if !self.semantic_enabled() {
            return;
        }
        let state = Rc::clone(self);
        glib::spawn_future_local(async move {
            let _ = state.semantic_index().await;
        });
    }

    /// Bedeutungsähnliche Aufgaben zum Suchtext (absteigend nach Score).
    /// Identische Titel werden ausgelassen; `open_only` filtert Erledigte.
    async fn semantic_query(
        self: &Rc<Self>,
        query: String,
        top_k: usize,
        threshold: f32,
        open_only: bool,
    ) -> Vec<(TodoItem, f32)> {
        let canonical = embeddings::canonical_text(&query);
        if canonical.is_empty() {
            return Vec::new();
        }
        let Some(cache) = self.semantic_index().await else {
            return Vec::new();
        };
        let cfg = self.embedding_config();
        // Query-Vektor memoisieren: gleiche Eingabe (z. B. nach Backspace)
        // kostet sonst jedes Mal eine volle Embedding-HTTP-Runde.
        let memo_key = format!("{}\u{1f}{}", cfg.model, canonical);
        let memoized = self.query_embed_cache.borrow().get(&memo_key).cloned();
        let query_vec = match memoized {
            Some(vec) => vec,
            None => {
                let inputs = vec![canonical.clone()];
                let embed_result = self
                    .ai_runtime
                    .spawn_blocking(move || embeddings::embed_texts(&cfg, &inputs))
                    .await;
                match embed_result {
                    Ok(Ok(mut vecs)) if !vecs.is_empty() => {
                        let vec = vecs.remove(0);
                        let mut memo = self.query_embed_cache.borrow_mut();
                        if memo.len() >= 256 {
                            memo.clear();
                        }
                        memo.insert(memo_key, vec.clone());
                        vec
                    }
                    _ => return Vec::new(),
                }
            }
        };
        // Alle Marker über dem Threshold scoren, dann auf Items abbilden
        // und erst nach dem Filtern auf top_k kürzen.
        let hits = cache.similar_markers(&query_vec, cache.items.len(), threshold);
        let items = self.cached_items.borrow();
        let by_marker: std::collections::HashMap<&str, &TodoItem> = items
            .iter()
            .filter_map(|i| i.key.marker.as_deref().map(|m| (m, i)))
            .collect();
        let mut results = Vec::new();
        for (marker, score) in hits {
            let Some(item) = by_marker.get(marker.as_str()) else {
                continue;
            };
            if open_only && item.done {
                continue;
            }
            if embeddings::canonical_text(&item.title) == canonical {
                continue;
            }
            results.push(((*item).clone(), score));
            if results.len() >= top_k {
                break;
            }
        }
        results
    }

    /// Hängt nach expliziter Suche (Enter) eine vierte Sektion
    /// „Bedeutungsähnliche To-dos" an die Suchergebnisse an, dedupliziert
    /// gegen die bereits angezeigten Substring-Treffer. Jede weitere
    /// Eingabe baut den Store neu auf und entfernt die Sektion wieder.
    fn run_semantic_search(self: &Rc<Self>, query: String) {
        let state = Rc::clone(self);
        glib::spawn_future_local(async move {
            let hits = state
                .semantic_query(
                    query.clone(),
                    embeddings::SEARCH_TOP_K,
                    embeddings::SEARCH_THRESHOLD,
                    false,
                )
                .await;
            if hits.is_empty() {
                return;
            }
            // Veraltete Antwort: Suchbegriff hat sich inzwischen geändert.
            if *state.search_term.borrow() != query {
                return;
            }
            let mut shown: HashSet<(usize, Option<String>)> = HashSet::new();
            for i in 0..state.store.n_items() {
                if let Some(obj) = state.store.item(i)
                    && let Ok(boxed) = obj.downcast::<BoxedAnyObject>()
                        && let ListEntry::Item(todo) = &*boxed.borrow::<ListEntry>() {
                            shown.insert((todo.key.line_index, todo.key.marker.clone()));
                        }
            }
            let fresh: Vec<TodoItem> = hits
                .into_iter()
                .map(|(item, _)| item)
                .filter(|item| !shown.contains(&(item.key.line_index, item.key.marker.clone())))
                .collect();
            if fresh.is_empty() {
                return;
            }
            state
                .store
                .append(&BoxedAnyObject::new(ListEntry::Header(t(
                    "Similar by meaning",
                ))));
            for item in fresh {
                state.store.append(&BoxedAnyObject::new(ListEntry::Item(item)));
            }
        });
    }

    fn mark_recently_updated(self: &Rc<Self>, marker: String) {
        {
            let mut slot = self.recently_updated.borrow_mut();
            *slot = if marker.is_empty() { None } else { Some(marker.clone()) };
        }

        self.repopulate_store();

        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(2, move || {
            if let Some(state) = weak.upgrade() {
                let should_clear = {
                    let slot = state.recently_updated.borrow();
                    slot.as_deref() == Some(marker.as_str())
                };
                if should_clear {
                    *state.recently_updated.borrow_mut() = None;
                    state.repopulate_store();
                }
            }

            glib::ControlFlow::Break
        });
    }

    fn whisper_language(&self) -> String {
        self.preferences.borrow().whisper_language.clone()
    }

    fn whisper_model_path(&self) -> PathBuf {
        let mut dir = glib::user_cache_dir();
        dir.push("reinschrift_todo");
        dir.push("ggml-small.bin");
        dir
    }

    /// Synchron neu laden — nur dort, wo ohnehin gewartet werden muss (Start,
    /// Wechsel der Datenbank). Alles andere nutzt `reload_async`.
    fn reload(&self) -> Result<()> {
        let (items, fingerprint) = data::load_todos_with_fingerprint()?;
        self.adopt_loaded(items, fingerprint);
        Ok(())
    }

    /// Frisch gelesenen Dateistand übernehmen. Noch nicht gespeicherte
    /// Änderungen bleiben darüber sichtbar.
    fn adopt_loaded(&self, items: Vec<TodoItem>, fingerprint: String) {
        *self.confirmed_items.borrow_mut() = items;
        *self.last_fingerprint.borrow_mut() = Some(fingerprint);
        self.rebuild_view();
    }

    /// Anzeige = bestätigter Dateistand + alle Änderungen, die noch in der
    /// Schreibwarteschlange stehen oder gerade gespeichert werden.
    fn rebuild_view(&self) {
        let mut items = self.confirmed_items.borrow().clone();
        let in_flight = self.write_in_flight.borrow();
        let queue = self.write_queue.borrow();
        for job in in_flight.iter().chain(queue.iter()) {
            if let WriteJob::Op { op, .. } = job {
                apply_optimistic(&mut items, op);
            }
        }
        *self.cached_items.borrow_mut() = items;
        drop(in_flight);
        drop(queue);
        self.repopulate_store();
    }

    /// Die Datei im Hintergrund lesen, ohne das Fenster zu blockieren.
    ///
    /// `Ok(false)`: Während des Lesens wurde ein eigener Schreibvorgang fertig,
    /// das Ergebnis kann ihn also noch nicht enthalten und wird verworfen —
    /// sonst flackerte die gerade gemachte Änderung kurz weg. Nach dem letzten
    /// Schreibvorgang wird ohnehin neu geladen.
    async fn reload_async(&self) -> Result<bool> {
        let completed = self.writes_completed.get();
        let (items, fingerprint) = gio::spawn_blocking(data::load_todos_with_fingerprint)
            .await
            .map_err(|_| anyhow!("reload thread panicked"))??;
        if self.writes_completed.get() != completed {
            return Ok(false);
        }
        self.adopt_loaded(items, fingerprint);
        Ok(true)
    }

    /// `reload_async` ohne Rückmeldung an den Aufrufer; Fehler als Toast.
    fn refresh_in_background(self: &Rc<Self>) {
        let state = Rc::clone(self);
        glib::spawn_future_local(async move {
            if let Err(err) = state.reload_async().await {
                state.show_error(&t("Could not reload To-dos: {}").replace("{}", &err.to_string()));
            }
        });
    }

    /// Liefert `true`, wenn sich die Datenbank geändert hat und neu
    /// geladen wurde (Aufrufer wärmt dann den Embedding-Index nach).
    ///
    /// Solange eigene Schreibvorgänge laufen, wird nicht nachgesehen: die
    /// ändern den Fingerprint ja gerade selbst, und danach lädt die
    /// Warteschlange ohnehin neu.
    async fn check_for_updates(&self) -> Result<bool> {
        if self.writes_pending() {
            return Ok(false);
        }
        let current_fp = gio::spawn_blocking(data::get_fingerprint)
            .await
            .map_err(|_| anyhow!("fingerprint thread panicked"))??;
        if self.last_fingerprint.borrow().as_deref() == Some(current_fp.as_str()) {
            return Ok(false);
        }
        self.reload_async().await
    }

    fn writes_pending(&self) -> bool {
        self.write_in_flight.borrow().is_some() || !self.write_queue.borrow().is_empty()
    }

    /// Änderung sofort anzeigen und im Hintergrund speichern.
    ///
    /// Vorher wurde jede Aktion im Hauptthread gespeichert und danach neu
    /// geladen — bei WebDAV drei bis vier Netzwerkrunden, in denen das Fenster
    /// stand. Jetzt landet die Änderung zuerst im Journal (damit sie Beenden,
    /// Absturz oder Abmelden übersteht), dann in der Anzeige, und wird danach
    /// der Reihe nach geschrieben.
    fn submit(self: &Rc<Self>, mut op: data::PendingOp) {
        for (from, to) in self.marker_renames.borrow().iter() {
            op.rename_marker(from, to);
        }
        let backend = data::backend_identity(&data::get_backend_config());
        let id = match self.journal.borrow_mut().push(&backend, op.clone()) {
            Ok(id) => Some(id),
            Err(err) => {
                // Speichern geht trotzdem; nur ein Absturz davor wäre nicht abgesichert.
                eprintln!("Could not record pending write: {err:#}");
                None
            }
        };
        self.enqueue(WriteJob::Op {
            id,
            op,
            mode: data::ApplyMode::Live,
        });
        self.rebuild_view();
    }

    fn enqueue(self: &Rc<Self>, job: WriteJob) {
        self.write_queue.borrow_mut().push_back(job);
        self.hold_app();
        self.run_next_write();
    }

    /// Die Anwendung am Leben halten, bis alles gespeichert ist — auch wenn
    /// das Fenster inzwischen geschlossen wurde. `run_next_write` gibt sie
    /// wieder frei, sobald die Warteschlange leer ist.
    fn hold_app(&self) {
        if self.write_hold.borrow().is_none()
            && let Some(app) = self.window.upgrade().and_then(|w| w.application())
        {
            *self.write_hold.borrow_mut() = Some(app.hold());
        }
    }

    /// Nicht gespeicherte Änderungen aus einer früheren Sitzung nachholen
    /// (App beendet, abgestürzt oder abgemeldet, bevor alles geschrieben war).
    /// Nur einmal pro Prozess: ein zweites Fenster teilt sich das Journal.
    fn replay_journal(self: &Rc<Self>) {
        if JOURNAL_REPLAYED.with(|done| done.replace(true)) {
            return;
        }
        let backend = data::backend_identity(&data::get_backend_config());
        let leftovers = self.journal.borrow().leftovers(&backend);
        if leftovers.is_empty() {
            return;
        }
        let mut dropped = 0;
        for entry in leftovers {
            match entry.op.for_replay() {
                Some(op) => self.write_queue.borrow_mut().push_back(WriteJob::Op {
                    id: Some(entry.id),
                    op,
                    mode: data::ApplyMode::Replay,
                }),
                None => {
                    dropped += 1;
                    self.forget_journal_entry(Some(entry.id));
                }
            }
        }
        let queued = self.write_queue.borrow().len();
        if queued > 0 {
            self.show_info(
                &t("Saving {} changes from the last session").replace("{}", &queued.to_string()),
            );
            self.hold_app();
            self.run_next_write();
            self.rebuild_view();
        }
        if dropped > 0 {
            self.show_error(
                &t("{} changes from the last session could not be restored")
                    .replace("{}", &dropped.to_string()),
            );
        }
    }

    /// Den nächsten Schreibvorgang starten. Immer nur einer zur Zeit, in der
    /// Reihenfolge der Klicks — jeder liest die Datei frisch und wendet seine
    /// Änderung darauf an.
    fn run_next_write(self: &Rc<Self>) {
        if self.write_in_flight.borrow().is_some() {
            return;
        }
        let Some(job) = self.write_queue.borrow_mut().pop_front() else {
            // Alles gespeichert: Anwendung freigeben (sie darf jetzt enden)
            // und den echten Dateistand holen, etwa die nächste Instanz einer
            // wiederkehrenden Aufgabe.
            self.write_hold.borrow_mut().take();
            if self.window.upgrade().is_some() {
                self.refresh_in_background();
            }
            return;
        };
        *self.write_in_flight.borrow_mut() = Some(job.clone());
        let state = Rc::clone(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || job.run()).await;
            let job = state
                .write_in_flight
                .borrow_mut()
                .take()
                .expect("write in flight");
            state.writes_completed.set(state.writes_completed.get() + 1);
            let result = result.unwrap_or_else(|_| job.failed(anyhow!("write thread panicked")));
            state.finish_write(job, result);
            state.run_next_write();
        });
    }

    fn finish_write(self: &Rc<Self>, job: WriteJob, result: WriteResult) {
        match (job, result) {
            (WriteJob::Op { id, op, .. }, WriteResult::Op(Ok(stored))) => {
                self.forget_journal_entry(id);
                // Bis zum nächsten Reload gilt die Änderung als Dateistand.
                apply_optimistic(&mut self.confirmed_items.borrow_mut(), &op);
                if let data::PendingOp::Add { marker, .. } = &op
                    && let Some(stored) = stored
                    && &stored != marker
                {
                    self.rename_marker(marker, &stored);
                }
            }
            (WriteJob::Op { id, .. }, WriteResult::Op(Err(err))) => {
                if self.window.upgrade().is_none() {
                    // Das Fenster ist zu, niemand sieht eine Meldung. Im
                    // Journal lassen: der nächste Start versucht es erneut
                    // und meldet dann, falls es wieder scheitert.
                    eprintln!("Could not save change: {err:#}");
                    return;
                }
                self.forget_journal_entry(id);
                // Ohne diese Änderung neu aufbauen, damit nichts angezeigt
                // bleibt, was nie gespeichert wurde.
                self.rebuild_view();
                if !self.handle_conflict(&err) {
                    self.show_error(&t("Could not update entry: {}").replace("{}", &err.to_string()));
                }
            }
            (WriteJob::Undo, WriteResult::Undo(result)) => match result {
                Ok(Some(desc)) => self.show_info(&t("Undone: {}").replace("{}", &desc)),
                Ok(None) => self.show_info(&t("Nothing to undo")),
                Err(err) => {
                    if !self.handle_conflict(&err) {
                        self.show_error(&err.to_string());
                    }
                }
            },
            (WriteJob::Overwrite(_), WriteResult::Overwrite(Err(err))) => {
                self.show_error(&t("Could not update entry: {}").replace("{}", &err.to_string()));
            }
            _ => {}
        }
    }

    fn forget_journal_entry(&self, id: Option<u64>) {
        if let Some(id) = id
            && let Err(err) = self.journal.borrow_mut().remove(id)
        {
            eprintln!("Could not update pending-write journal: {err:#}");
        }
    }

    /// Eine neue Aufgabe hat in der Datei einen anderen Marker bekommen, weil
    /// ein anderes Gerät ihren schon vergeben hatte. Alles, was danach an ihr
    /// geändert wird, muss dem neuen Marker folgen.
    fn rename_marker(&self, from: &str, to: &str) {
        self.marker_renames
            .borrow_mut()
            .insert(from.to_string(), to.to_string());
        let mut journal = self.journal.borrow_mut();
        for job in self.write_queue.borrow_mut().iter_mut() {
            if let WriteJob::Op { id, op, .. } = job {
                op.rename_marker(from, to);
                if let Some(id) = id
                    && let Err(err) = journal.replace(*id, op.clone())
                {
                    eprintln!("Could not update pending-write journal: {err:#}");
                }
            }
        }
        drop(journal);
        for item in self.confirmed_items.borrow_mut().iter_mut() {
            if item.key.marker.as_deref() == Some(from) {
                item.key.marker = Some(to.to_string());
            }
        }
        self.rebuild_view();
    }

    fn toggle_item(self: &Rc<Self>, todo: &TodoItem, done: bool) {
        // toggle_todos erledigt auch den Sonderfall wiederkehrender Aufgaben
        // (überfällige erst auf heute setzen, nächste Instanz anlegen) — in
        // einem einzigen Schreibvorgang statt zweien.
        self.submit(data::PendingOp::SetDone {
            keys: vec![todo.key.clone()],
            done,
        });
        let message = if done {
            format!("Erledigt: {}", todo.title)
        } else {
            format!("Reaktiviert: {}", todo.title)
        };
        self.show_undo_toast(&message);
    }

    fn toggle_myday(self: &Rc<Self>, todo: &TodoItem) {
        let today = Local::now().date_naive();
        let currently_in = todo.myday == Some(today);
        self.submit(data::PendingOp::SetMyday {
            key: todo.key.clone(),
            on: !currently_in,
        });
        let message = if currently_in {
            t("Remove from My Day")
        } else {
            t("Add to My Day")
        };
        self.show_undo_toast(&format!("{message}: {}", todo.title));
    }

    /// Fälligkeit setzen; `label` ist der Toast-Text mit `{}` für das Datum.
    fn set_due(self: &Rc<Self>, todo: &TodoItem, target: data::DueTarget, label: &str) {
        let due = data::due_for_target(target, todo.due);
        self.submit(data::PendingOp::SetDue {
            keys: vec![todo.key.clone()],
            target,
        });
        self.show_undo_toast(&label.replace("{}", &due.format("%Y-%m-%dT%H:%M").to_string()));
    }

    fn set_due_today(self: &Rc<Self>, todo: &TodoItem) {
        self.set_due(todo, data::DueTarget::Today, "Fällig heute ({})");
    }

    fn set_due_tomorrow(self: &Rc<Self>, todo: &TodoItem) {
        self.set_due(todo, data::DueTarget::Tomorrow, "Fällig morgen ({})");
    }

    fn set_due_weekend(self: &Rc<Self>, todo: &TodoItem) {
        self.set_due(todo, data::DueTarget::Weekend, "Fällig am Wochenende ({})");
    }

    fn set_due_in_days(self: &Rc<Self>, todo: &TodoItem, days: i64) {
        let mut updated = todo.clone();
        let base_time = todo.due.map(|d| d.time()).unwrap_or(DEFAULT_DUE_TIME);
        let target_date = Local::now().date_naive() + Duration::days(days);
        updated.due = Some(NaiveDateTime::new(target_date, base_time));
        self.save_item(&updated)
    }

    fn set_due_sometimes(self: &Rc<Self>, todo: &TodoItem) {
        self.set_due(todo, data::DueTarget::Sometime, "Fällig irgendwann ({})");
    }

    /// Tastenkürzel-Fenster (GtkShortcutsWindow), gruppiert nach Bereich.
    /// Mit gtk4 v4_12 gibt es noch keine programmatische Add-API
    /// (`add_section` kam erst mit 4.14), daher Aufbau über Builder-XML.
    fn show_shortcuts_window(self: &Rc<Self>) {
        let Some(parent) = self.window.upgrade() else {
            self.show_error(&t("No window available"));
            return;
        };

        fn shortcut(title: &str, accel: &str) -> String {
            format!(
                concat!(
                    "<child><object class=\"GtkShortcutsShortcut\">",
                    "<property name=\"title\">{}</property>",
                    "<property name=\"accelerator\">{}</property>",
                    "</object></child>"
                ),
                xml_escape(title),
                xml_escape(accel),
            )
        }
        fn group(title: &str, shortcuts: &[String]) -> String {
            format!(
                concat!(
                    "<child><object class=\"GtkShortcutsGroup\">",
                    "<property name=\"title\">{}</property>{}",
                    "</object></child>"
                ),
                xml_escape(title),
                shortcuts.concat(),
            )
        }

        let groups = [
            group(&t("General"), &[
                shortcut(&t("Show help"), "question F1"),
                shortcut(&t("New task"), "<Control>n"),
                shortcut(&t("Search"), "<Control>f"),
                shortcut(&t("Open settings"), "<Control>comma"),
                shortcut(&t("Reload"), "<Control>r"),
                shortcut(&t("Undo last action"), "<Control>z"),
                shortcut(&t("Close search/dialog"), "Escape"),
                shortcut(&t("Quit"), "<Control>q"),
            ]),
            group(&t("Task list"), &[
                shortcut(&t("Navigate"), "Up Down"),
                shortcut(&t("Toggle done"), "space"),
                shortcut(&t("Edit"), "Return"),
                shortcut(&t("Delete selected task"), "Delete"),
                shortcut(&t("In the planning picker: add to My Day"), "space"),
            ]),
            group(&t("Set due date"), &[
                shortcut(&t("Due today"), "h"),
                shortcut(&t("Due tomorrow"), "m"),
                shortcut(&t("Due this weekend"), "w"),
                shortcut(&t("Due next week"), "l"),
                shortcut(&t("Due sometimes"), "s"),
            ]),
            group(&t("Multi-selection"), &[
                shortcut(&t("Toggle selection"), "space"),
            ]),
        ];

        let xml = format!(
            concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<interface><object class=\"GtkShortcutsWindow\" id=\"shortcuts_window\">",
                "<property name=\"modal\">true</property>",
                "<child><object class=\"GtkShortcutsSection\">{}</object></child>",
                "</object></interface>"
            ),
            groups.concat(),
        );

        let builder = gtk::Builder::from_string(&xml);
        let Some(window) = builder.object::<gtk::ShortcutsWindow>("shortcuts_window") else {
            self.show_error(&t("No window available"));
            return;
        };
        window.set_transient_for(Some(&parent));
        window.present();
    }

    fn show_settings_dialog(self: &Rc<Self>, voice_btn: Option<gtk::Button>) {
        let Some(parent) = self.window.upgrade() else {
            self.show_error(&t("No window available"));
            return;
        };

        let dialog = adw::PreferencesWindow::builder()
            .title(t("Settings"))
            .transient_for(&parent)
            .modal(true)
            .default_width(480)
            .build();

        // --- General Page ---
        let general_page = adw::PreferencesPage::builder()
            .title(t("General"))
            .icon_name("preferences-system-symbolic")
            .build();
        dialog.add(&general_page);

        // --- Storage Group ---
        let storage_group = adw::PreferencesGroup::builder()
            .title(t("Storage"))
            .build();
        general_page.add(&storage_group);

        // WebDAV switch
        let use_webdav_now = self.preferences.borrow().use_webdav;
        let webdav_switch_row = adw::SwitchRow::builder()
            .title(t("Use WebDAV"))
            .active(use_webdav_now)
            .build();
        webdav_switch_row.add_prefix(&gtk::Image::from_icon_name("network-server-symbolic"));
        storage_group.add(&webdav_switch_row);

        // Local file row (visible when not in WebDAV mode)
        let current_db_path = self.preferences.borrow().db_path.clone().unwrap_or_default();
        let db_subtitle = if current_db_path.is_empty() { t("Select file…") } else { current_db_path.clone() };
        let db_row = adw::ActionRow::builder()
            .title(t("Database file"))
            .subtitle(&db_subtitle)
            .build();
        db_row.set_visible(!use_webdav_now);

        // "Select existing file" button
        let select_btn = gtk::Button::builder()
            .label(t("Select file…"))
            .valign(gtk::Align::Center)
            .build();
        select_btn.add_css_class("flat");
        db_row.add_suffix(&select_btn);

        // "Create new file" button
        let new_btn = gtk::Button::builder()
            .label(t("Create new file"))
            .valign(gtk::Align::Center)
            .build();
        new_btn.add_css_class("flat");
        db_row.add_suffix(&new_btn);

        storage_group.add(&db_row);

        // Connect WebDAV switch – toggles db_row visibility and calls set_use_webdav()
        let db_row_for_switch = db_row.clone();
        let state_for_switch = Rc::clone(self);
        webdav_switch_row.connect_active_notify(move |row| {
            let use_webdav = row.is_active();
            db_row_for_switch.set_visible(!use_webdav);
            state_for_switch.set_use_webdav(use_webdav);
        });

        // Connect "Select file" button – opens a file chooser dialog
        let fd_parent = dialog.clone();
        let state_select = Rc::clone(self);
        let db_row_select = db_row.clone();
        select_btn.connect_clicked(move |_| {
            let fd = gtk::FileDialog::builder()
                .title(t("Database file"))
                .build();
            let state = Rc::clone(&state_select);
            let row = db_row_select.clone();
            fd.open(Some(&fd_parent), gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path() {
                        if !path.exists() {
                            state.show_error(&t("File does not exist"));
                            return;
                        }
                        if data::todo_path() == path {
                            state.show_info(&t("This file is already active"));
                            return;
                        }
                        data::set_todo_path(path.clone());
                        data::set_backend_config(data::BackendConfig::Local(path.clone()));
                        {
                            let mut p = state.preferences.borrow_mut();
                            p.db_path = Some(path.to_string_lossy().into_owned());
                            p.use_webdav = false;
                        }
                        state.persist_preferences();
                        row.set_subtitle(&path.to_string_lossy());
                        if let Err(e) = state.reload() {
                            state.show_error(&t("Could not load data: {}").replace("{}", &e.to_string()));
                        } else {
                            state.show_info(&t("Using {}").replace("{}", &path.to_string_lossy()));
                        }
                    }
            });
        });

        // Connect "New file" button – opens a save dialog
        let fd_parent2 = dialog.clone();
        let state_new = Rc::clone(self);
        let db_row_new = db_row.clone();
        new_btn.connect_clicked(move |_| {
            let fd = gtk::FileDialog::builder()
                .title(t("Create new file"))
                .initial_name("todos.md")
                .build();
            let state = Rc::clone(&state_new);
            let row = db_row_new.clone();
            fd.save(Some(&fd_parent2), gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path() {
                        if !path.exists()
                            && std::fs::write(&path, "").is_err() {
                                state.show_error(&t("Could not write {}").replace("{}", &path.display().to_string()));
                                return;
                            }
                        data::set_todo_path(path.clone());
                        data::set_backend_config(data::BackendConfig::Local(path.clone()));
                        {
                            let mut p = state.preferences.borrow_mut();
                            p.db_path = Some(path.to_string_lossy().into_owned());
                            p.use_webdav = false;
                        }
                        state.persist_preferences();
                        row.set_subtitle(&path.to_string_lossy());
                        if let Err(e) = state.reload() {
                            state.show_error(&t("Could not load data: {}").replace("{}", &e.to_string()));
                        } else {
                            state.show_info(&t("Using {}").replace("{}", &path.to_string_lossy()));
                        }
                    }
            });
        });

        let general_group = adw::PreferencesGroup::builder()
            .title(t("General"))
            .build();
        general_page.add(&general_group);

        let show_done_row = adw::SwitchRow::builder()
            .title(t("Show completed tasks"))
            .active(self.show_completed())
            .build();
        show_done_row.add_prefix(&gtk::Image::from_icon_name("view-list-symbolic"));
        let state_done = Rc::clone(self);
        show_done_row.connect_active_notify(move |row| {
            state_done.set_show_completed(row.is_active());
        });
        general_group.add(&show_done_row);

        let delete_confirm_row = adw::SwitchRow::builder()
            .title(t("Delete without confirmation"))
            .subtitle(t("Skip the delete dialog and remove entries immediately."))
            .active(self.skip_delete_confirmation())
            .build();
        delete_confirm_row.add_prefix(&gtk::Image::from_icon_name("user-trash-symbolic"));
        let state_delete_pref = Rc::clone(self);
        delete_confirm_row.connect_active_notify(move |row| {
            state_delete_pref.set_skip_delete_confirmation(row.is_active());
        });
        general_group.add(&delete_confirm_row);

        let title_autocomplete_row = adw::SwitchRow::builder()
            .title(t("Title autocomplete"))
            .subtitle(t("Suggest existing task titles while typing."))
            .active(self.title_autocomplete_enabled())
            .build();
        title_autocomplete_row.add_prefix(&gtk::Image::from_icon_name("edit-find-symbolic"));
        let state_title_ac = Rc::clone(self);
        title_autocomplete_row.connect_active_notify(move |row| {
            state_title_ac.set_title_autocomplete_enabled(row.is_active());
        });
        general_group.add(&title_autocomplete_row);

        // Reminders
        let enable_reminders_row = adw::SwitchRow::builder()
            .title(t("Enable reminders"))
            .active(self.preferences.borrow().enable_reminders)
            .build();
        enable_reminders_row.add_prefix(&gtk::Image::from_icon_name("alarm-symbolic"));
        let state_reminders = Rc::clone(self);
        enable_reminders_row.connect_active_notify(move |row| {
            let mut prefs = state_reminders.preferences.borrow_mut();
            prefs.enable_reminders = row.is_active();
            let _ = write_preferences(&prefs);
        });
        general_group.add(&enable_reminders_row);

        let remind_minutes_row = adw::ActionRow::builder()
            .title(t("Reminder window (minutes)"))
            .build();
        let remind_spin = gtk::SpinButton::with_range(5.0, 120.0, 5.0);
        remind_spin.set_value(self.preferences.borrow().remind_before_minutes as f64);
        remind_spin.set_width_chars(4);
        let state_remind_min = Rc::clone(self);
        remind_spin.connect_value_changed(move |spin| {
            let mins = spin.value().round() as i64;
            let mut prefs = state_remind_min.preferences.borrow_mut();
            prefs.remind_before_minutes = mins;
            let _ = write_preferences(&prefs);
        });
        remind_minutes_row.add_suffix(&remind_spin);
        remind_minutes_row.set_activatable_widget(Some(&remind_spin));
        general_group.add(&remind_minutes_row);

        let ai_row = adw::SwitchRow::builder()
            .title(t("Use AI on add"))
            .subtitle(t("Run Smart Add automatically; fallback if AI times out."))
            .active(self.use_ai_on_new_topic())
            .build();
        ai_row.add_prefix(&gtk::Image::from_icon_name("starred-symbolic"));
        let state_ai = Rc::clone(self);
        ai_row.connect_active_notify(move |row| {
            state_ai.set_use_ai_on_new_topic(row.is_active());
        });
        general_group.add(&ai_row);

        let ai_timeout_row = adw::ActionRow::builder()
            .title(t("AI timeout (seconds)"))
            .subtitle(t("Cancel AI parsing after this time."))
            .build();
        let ai_timeout_spin = gtk::SpinButton::with_range(5.0, 120.0, 1.0);
        ai_timeout_spin.set_value(self.ai_timeout_secs() as f64);
        ai_timeout_spin.set_width_chars(4);
        let state_ai_timeout = Rc::clone(self);
        ai_timeout_spin.connect_value_changed(move |spin| {
            let secs = spin.value().round().clamp(5.0, 120.0) as u64;
            spin.set_value(secs as f64);
            state_ai_timeout.set_ai_timeout_secs(secs);
        });
        ai_timeout_row.add_suffix(&ai_timeout_spin);
        ai_timeout_row.set_activatable_widget(Some(&ai_timeout_spin));
        general_group.add(&ai_timeout_row);

        let ollama_url_row = adw::EntryRow::builder()
            .title(t("Ollama URL"))
            .text(self.ollama_url())
            .build();
        ollama_url_row.set_tooltip_text(Some(&t("Ollama server address (e.g., http://localhost:11434)")));
        let state_ollama_url = Rc::clone(self);
        ollama_url_row.connect_changed(move |row| {
            state_ollama_url.set_ollama_url(row.text().to_string());
        });
        general_group.add(&ollama_url_row);

        let ollama_model_row = adw::EntryRow::builder()
            .title(t("AI Model"))
            .text(self.ollama_model())
            .build();
        ollama_model_row.set_tooltip_text(Some(&t("Ollama model name (e.g., mistral:7b, llama3.1:8b)")));
        let state_ollama_model = Rc::clone(self);
        ollama_model_row.connect_changed(move |row| {
            state_ollama_model.set_ollama_model(row.text().to_string());
        });
        general_group.add(&ollama_model_row);

        let semantic_row = adw::SwitchRow::builder()
            .title(t("Semantic suggestions"))
            .subtitle(t("Suggest similar tasks via an embedding model (requires Ollama)"))
            .active(self.semantic_enabled())
            .build();
        semantic_row.add_prefix(&gtk::Image::from_icon_name("edit-find-symbolic"));
        let state_semantic = Rc::clone(self);
        semantic_row.connect_active_notify(move |row| {
            state_semantic.set_semantic_enabled(row.is_active());
        });
        general_group.add(&semantic_row);

        let embedding_model_row = adw::EntryRow::builder()
            .title(t("Embedding model"))
            .text(self.embedding_model())
            .build();
        embedding_model_row.set_tooltip_text(Some(&t("Ollama embedding model (e.g. bge-m3) — pull it first with 'ollama pull'")));
        let state_embedding_model = Rc::clone(self);
        embedding_model_row.connect_changed(move |row| {
            state_embedding_model.set_embedding_model(row.text().to_string());
        });
        general_group.add(&embedding_model_row);

        // --- WebDAV Page ---
        let webdav_page = adw::PreferencesPage::builder()
            .title(t("WebDAV"))
            .icon_name("network-server-symbolic")
            .build();
        dialog.add(&webdav_page);

        let webdav_group = adw::PreferencesGroup::builder()
            .title(t("WebDAV"))
            .build();
        webdav_page.add(&webdav_group);

        let (_, _, wd_path, wd_user, wd_pass) = self.get_webdav_prefs();
        // Note: wd_url is fetched inside the closure below or we can get it here if needed, 
        // but we need to bind it to the row.
        // Let's get the current values again to populate the fields.
        let (_, wd_url, _, _, _) = self.get_webdav_prefs();

        let url_row = adw::EntryRow::builder()
            .title(t("WebDAV URL"))
            .text(wd_url.unwrap_or_default())
            .build();
        let state_url = Rc::clone(self);
        url_row.connect_changed(move |row| {
            state_url.set_webdav_url(row.text().to_string());
        });
        webdav_group.add(&url_row);

        let path_row = adw::EntryRow::builder()
            .title(t("Path (relative)"))
            .text(wd_path.unwrap_or_default())
            .build();
        let state_path = Rc::clone(self);
        path_row.connect_changed(move |row| {
            state_path.set_webdav_path(row.text().to_string());
        });
        webdav_group.add(&path_row);

        let user_row = adw::EntryRow::builder()
            .title(t("Username"))
            .text(wd_user.unwrap_or_default())
            .build();
        let state_user = Rc::clone(self);
        user_row.connect_changed(move |row| {
            state_user.set_webdav_username(row.text().to_string());
        });
        webdav_group.add(&user_row);

        let pass_row = adw::PasswordEntryRow::builder()
            .title(t("Password"))
            .text(wd_pass.unwrap_or_default())
            .build();
        let state_pass = Rc::clone(self);
        pass_row.connect_changed(move |row| {
            state_pass.set_webdav_password(row.text().to_string());
        });
        webdav_group.add(&pass_row);

        // --- Nextcloud Login Flow v2 ---
        let nc_login_row = adw::ActionRow::builder()
            .title(t("Login with Nextcloud"))
            .activatable(true)
            .build();
        nc_login_row.add_suffix(&gtk::Image::from_icon_name("web-browser-symbolic"));

        // Hide user/pass when URL indicates Nextcloud Login Flow was used
        let current_url = url_row.text().to_string();
        let is_nc = current_url.contains("remote.php/dav/files");
        user_row.set_visible(!is_nc);
        pass_row.set_visible(!is_nc);

        // Also toggle user/pass visibility when URL changes
        let user_row_for_url = user_row.clone();
        let pass_row_for_url = pass_row.clone();
        url_row.connect_changed(move |row| {
            let url = row.text().to_string();
            let nc = url.contains("remote.php/dav/files");
            user_row_for_url.set_visible(!nc);
            pass_row_for_url.set_visible(!nc);
        });

        let url_row_for_nc = url_row.clone();
        let user_row_for_nc = user_row.clone();
        let pass_row_for_nc = pass_row.clone();
        let path_row_for_nc = path_row.clone();
        let state_for_nc = Rc::clone(self);
        let nc_polling = Rc::new(RefCell::new(false));
        let nc_polling_for_handler = Rc::clone(&nc_polling);

        nc_login_row.connect_activated(move |row| {
            let server_url = url_row_for_nc.text().to_string();
            // Strip Nextcloud WebDAV path to get the base server URL
            let server_url = if let Some(pos) = server_url.find("/remote.php/") {
                server_url[..pos].to_string()
            } else {
                server_url
            };
            if server_url.trim().is_empty() {
                state_for_nc.show_error(&t("No URL specified."));
                return;
            }

            if *nc_polling_for_handler.borrow() {
                return;
            }
            *nc_polling_for_handler.borrow_mut() = true;

            row.set_subtitle(&t("Waiting for browser login…"));

            let state_bg = state_for_nc.clone();
            let row_clone = row.clone();
            let url_row_bg = url_row_for_nc.clone();
            let user_row_bg = user_row_for_nc.clone();
            let pass_row_bg = pass_row_for_nc.clone();
            let path_row_bg = path_row_for_nc.clone();
            let nc_polling_bg = Rc::clone(&nc_polling_for_handler);

            let (init_sender, init_receiver) = std::sync::mpsc::channel();
            let server_url_clone = server_url.clone();
            std::thread::spawn(move || {
                let result = data::initiate_nextcloud_login(&server_url_clone);
                let _ = init_sender.send(result);
            });

            glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
                match init_receiver.try_recv() {
                    Ok(result) => {
                        match result {
                            Ok((login_url, endpoint, token)) => {
                                // Open browser
                                let launcher = gtk::UriLauncher::new(&login_url);
                                launcher.launch(gtk::Window::NONE, gio::Cancellable::NONE, |_| {});

                                // Start polling
                                let state_poll = state_bg.clone();
                                let row_poll = row_clone.clone();
                                let url_row_poll = url_row_bg.clone();
                                let user_row_poll = user_row_bg.clone();
                                let pass_row_poll = pass_row_bg.clone();
                                let path_row_poll = path_row_bg.clone();
                                let nc_polling_poll = Rc::clone(&nc_polling_bg);
                                let poll_count = Rc::new(RefCell::new(0u32));

                                glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
                                    let count = {
                                        let mut c = poll_count.borrow_mut();
                                        *c += 1;
                                        *c
                                    };
                                    if count > 120 {
                                        row_poll.set_subtitle("");
                                        *nc_polling_poll.borrow_mut() = false;
                                        state_poll.show_error(&t("Nextcloud login failed: {}").replace("{}", "timeout"));
                                        return glib::ControlFlow::Break;
                                    }

                                    let endpoint_c = endpoint.clone();
                                    let token_c = token.clone();
                                    let (poll_sender, poll_receiver) = std::sync::mpsc::channel();
                                    std::thread::spawn(move || {
                                        let result = data::poll_nextcloud_login(&endpoint_c, &token_c);
                                        let _ = poll_sender.send(result);
                                    });

                                    let state_inner = state_poll.clone();
                                    let row_inner = row_poll.clone();
                                    let url_row_inner = url_row_poll.clone();
                                    let user_row_inner = user_row_poll.clone();
                                    let pass_row_inner = pass_row_poll.clone();
                                    let path_row_inner = path_row_poll.clone();
                                    let nc_polling_inner = Rc::clone(&nc_polling_poll);
                                    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
                                        match poll_receiver.try_recv() {
                                            Ok(result) => {
                                                match result {
                                                    Ok(Some((server, login_name, app_password))) => {
                                                        user_row_inner.set_text(&login_name);
                                                        pass_row_inner.set_text(&app_password);
                                                        let webdav_url = format!(
                                                            "{}/remote.php/dav/files/{}",
                                                            server.trim_end_matches('/'),
                                                            login_name
                                                        );
                                                        state_inner.set_webdav_url(webdav_url.clone());
                                                        url_row_inner.set_text(&webdav_url);
                                                        state_inner.set_webdav_username(login_name.clone());
                                                        state_inner.set_webdav_password(app_password);

                                                        // The login only yields the account root; without a
                                                        // file name every write would land on a folder.
                                                        if path_row_inner.text().trim().is_empty() {
                                                            path_row_inner.set_text(DEFAULT_WEBDAV_PATH);
                                                        }

                                                        row_inner.set_subtitle("");
                                                        *nc_polling_inner.borrow_mut() = false;
                                                        state_inner.show_info(&t("Nextcloud login successful!"));
                                                    }
                                                    Ok(None) => {
                                                        // Still pending
                                                    }
                                                    Err(e) => {
                                                        row_inner.set_subtitle("");
                                                        *nc_polling_inner.borrow_mut() = false;
                                                        state_inner.show_error(&t("Nextcloud login failed: {}").replace("{}", &e.to_string()));
                                                    }
                                                }
                                                glib::ControlFlow::Break
                                            }
                                            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                                            Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
                                        }
                                    });

                                    glib::ControlFlow::Continue
                                });
                            }
                            Err(e) => {
                                row_clone.set_subtitle("");
                                *nc_polling_bg.borrow_mut() = false;
                                state_bg.show_error(&t("Nextcloud login failed: {}").replace("{}", &e.to_string()));
                            }
                        }
                        glib::ControlFlow::Break
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
                }
            });
        });
        webdav_group.add(&nc_login_row);

        let check_row = adw::ActionRow::builder()
            .title(t("Check connection"))
            .build();
        let check_button = gtk::Button::builder()
            .label(t("Check connection"))
            .valign(gtk::Align::Center)
            .build();
        check_button.add_css_class("flat");
        check_row.add_suffix(&check_button);
        
        let state_for_check = Rc::clone(self);
        check_button.connect_clicked(move |_| {
            let (_, url, path, user, pass) = state_for_check.get_webdav_prefs();
            
            let Some(u) = url else {
                state_for_check.show_error(&t("No URL specified."));
                return;
            };
            if u.trim().is_empty() {
                state_for_check.show_error(&t("No URL specified."));
                return;
            }

            let state_bg = state_for_check.clone();
            let (sender, receiver) = std::sync::mpsc::channel();
            
            let u_clone = u.clone();
            let path_clone = path.clone();
            let user_clone = user.clone();
            let pass_clone = pass.clone();

            std::thread::spawn(move || {
                let result = data::test_webdav_connection(&u_clone, path_clone.as_deref(), user_clone.as_deref(), pass_clone.as_deref());
                let _ = sender.send(result);
            });

            glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
                match receiver.try_recv() {
                    Ok(result) => {
                        match result {
                            Ok(_) => state_bg.show_info(&t("Connection successful!")),
                            Err(e) => {
                                eprintln!("{}", t("WebDAV Connection Error: {}").replace("{}", &e.to_string()));
                                state_bg.show_error(&t("Connection failed: {}").replace("{}", &e.to_string()));
                            }
                        }
                        glib::ControlFlow::Break
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
                }
            });
        });
        webdav_group.add(&check_row);

        // --- Voice Page ---
        let voice_page = adw::PreferencesPage::builder()
            .title(t("Voice"))
            .icon_name("audio-input-microphone-symbolic")
            .build();
        dialog.add(&voice_page);

        let voice_group = adw::PreferencesGroup::builder()
            .title(t("Voice"))
            .build();
        voice_page.add(&voice_group);

        let progress_bar = gtk::ProgressBar::builder()
            .visible(false)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        voice_group.add(&progress_bar);

        let use_whisper_row = adw::SwitchRow::builder()
            .title(t("Local Speech Recognition (Whisper)"))
            .subtitle(t("Downloads a ~480MB model for offline recognition"))
            .active(self.use_whisper())
            .build();
        use_whisper_row.add_prefix(&gtk::Image::from_icon_name("audio-input-microphone-symbolic"));
        
        let languages = vec!["auto", "en", "de", "es", "fr", "it", "ja", "zh", "nl", "pl", "pt", "ru", "tr", "sv"];
        let language_names = [
            t("Automatic"), t("English"), t("German"), t("Spanish"), t("French"), 
            t("Italian"), t("Japanese"), t("Chinese"), t("Dutch"), t("Polish"), 
            t("Portuguese"), t("Russian"), t("Turkish"), t("Swedish")
        ];
        let language_names_refs: Vec<&str> = language_names.iter().map(|s| s.as_str()).collect();
        
        let language_model = gtk::StringList::new(&language_names_refs);
        
        let language_row = adw::ComboRow::builder()
            .title(t("Recognition Language"))
            .model(&language_model)
            .build();

        // Set initial selection
        let current_lang = self.whisper_language();
        if let Some(idx) = languages.iter().position(|&l| l == current_lang) {
            language_row.set_selected(idx as u32);
        }

        let state_lang = Rc::clone(self);
        let languages_clone = languages.clone();
        language_row.connect_selected_notify(move |row| {
            let idx = row.selected() as usize;
            if idx < languages_clone.len() {
                state_lang.set_whisper_language(languages_clone[idx].to_string());
            }
        });

        let state_whisper = Rc::clone(self);
        let pb_whisper = progress_bar.clone();
        let vb_whisper = voice_btn.clone();
        let lang_row_clone = language_row.clone();
        
        // Disable language selection if whisper is disabled
        language_row.set_sensitive(self.use_whisper());

        use_whisper_row.connect_active_notify(move |row| {
            if row.is_active() {
                state_whisper.set_use_whisper(true, Some(pb_whisper.clone()), Some(row.clone()), vb_whisper.clone());
                lang_row_clone.set_sensitive(true);
            } else {
                state_whisper.set_use_whisper(false, None, None, vb_whisper.clone());
                lang_row_clone.set_sensitive(false);
            }
        });
        voice_group.add(&use_whisper_row);
        voice_group.add(&language_row);

        // --- About Page ---
        let about_page = adw::PreferencesPage::builder()
            .title(t("About"))
            .icon_name("help-about-symbolic")
            .build();
        dialog.add(&about_page);

        let about_group = adw::PreferencesGroup::builder()
            .build();
        about_page.add(&about_group);

        let banner = adw::Bin::builder()
            .margin_top(12)
            .margin_bottom(12)
            .build();
        let banner_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        banner_box.set_halign(gtk::Align::Center);
        
        let app_icon = gtk::Image::from_icon_name("me.dumke.Reinschrift");
        app_icon.set_pixel_size(128);
        banner_box.append(&app_icon);

        let app_name = gtk::Label::builder()
            .label("Reinschrift")
            .css_classes(["title-1"])
            .build();
        banner_box.append(&app_name);

        let app_version = gtk::Label::builder()
            .label(format!("{} {}", t("Version"), env!("CARGO_PKG_VERSION")))
            .css_classes(["dim-label"])
            .build();
        banner_box.append(&app_version);

        banner.set_child(Some(&banner_box));
        about_group.add(&banner);

        let info_group = adw::PreferencesGroup::builder()
            .build();
        about_page.add(&info_group);

        let dev_row = adw::ActionRow::builder()
            .title(t("Developer"))
            .subtitle("Dr. Daniel Dumke")
            .build();
        info_group.add(&dev_row);

        let site_row = adw::ActionRow::builder()
            .title(t("Website"))
            .subtitle("https://github.com/danst0/ReinschriftTodo")
            .activatable(true)
            .build();
        site_row.connect_activated(|_| {
            let launcher = gtk::FileLauncher::new(Some(&gio::File::for_uri("https://github.com/danst0/ReinschriftTodo")));
            launcher.launch(None::<&gtk::Window>, gio::Cancellable::NONE, |_| {});
        });
        info_group.add(&site_row);

        let license_row = adw::ActionRow::builder()
            .title(t("License"))
            .subtitle("GPL-3.0-or-later")
            .build();
        info_group.add(&license_row);

        dialog.present();
    }

    fn set_show_completed(&self, show: bool) {
        {
            let mut prefs = self.preferences.borrow_mut();
            if prefs.show_done == show {
                return;
            }
            prefs.show_done = show;
        }

        self.persist_preferences();
        self.repopulate_store();
    }

    fn set_filter(&self, filter: TodoFilter) {
        {
            let mut prefs = self.preferences.borrow_mut();
            if prefs.effective_filter() == filter && prefs.filter.is_some() {
                return;
            }
            prefs.set_filter(filter);
        }

        self.persist_preferences();
        self.update_filter_button();
        self.repopulate_store();
    }

    /// Change the filter through a closure on a copy of the current one.
    fn update_filter(&self, change: impl FnOnce(&mut TodoFilter)) {
        let mut filter = self.filter();
        change(&mut filter);
        self.set_filter(filter);
    }

    fn update_filter_button(&self) {
        let Some(button) = self.filter_button.borrow().clone() else {
            return;
        };
        let count = self.filter().active_count();
        if count == 0 {
            button.set_label(&t("Filter"));
            button.remove_css_class("accent");
        } else {
            button.set_label(&t("Filter ({})").replace("{}", &count.to_string()));
            button.add_css_class("accent");
        }
    }

    /// Projects and places of the open tasks, in their most used casing,
    /// alphabetically — the choices in the filter popover. Tags that are
    /// in the filter stay listed even when no open task carries them.
    fn filter_tag_choices(&self) -> (Vec<String>, Vec<String>) {
        let (canon_projects, canon_contexts) = self.canonical_tag_maps();
        let items = self.cached_items.borrow();
        let filter = self.filter();
        fn collect<'a>(
            tags: impl Iterator<Item = &'a String>,
            kept: &'a [String],
            canon: &HashMap<String, String>,
        ) -> Vec<String> {
            let mut seen: HashMap<String, String> = HashMap::new();
            for tag in tags.chain(kept.iter().filter(|k| !k.is_empty())) {
                seen.entry(tag.to_lowercase())
                    .or_insert_with(|| canonicalize_token(canon, tag));
            }
            let mut names: Vec<String> = seen.into_values().collect();
            names.sort_by_key(|n| n.to_lowercase());
            names
        }
        let open = || items.iter().filter(|i| !i.done);
        (
            collect(open().flat_map(|i| i.projects.iter()), &filter.projects, &canon_projects),
            collect(open().flat_map(|i| i.contexts.iter()), &filter.contexts, &canon_contexts),
        )
    }

    fn set_myday_view(&self, show: bool) {
        {
            let mut prefs = self.preferences.borrow_mut();
            if prefs.myday_view == show {
                return;
            }
            prefs.myday_view = show;
        }

        self.persist_preferences();
        self.repopulate_store();
    }

    fn set_skip_delete_confirmation(&self, skip: bool) {
        {
            let mut prefs = self.preferences.borrow_mut();
            if prefs.skip_delete_confirmation == skip {
                return;
            }
            prefs.skip_delete_confirmation = skip;
        }

        self.persist_preferences();
    }

    fn set_title_autocomplete_enabled(&self, enabled: bool) {
        {
            let mut prefs = self.preferences.borrow_mut();
            if prefs.title_autocomplete_enabled == enabled {
                return;
            }
            prefs.title_autocomplete_enabled = enabled;
        }
        self.persist_preferences();
    }

    fn set_use_ai_on_new_topic(&self, enabled: bool) {
        {
            let mut prefs = self.preferences.borrow_mut();
            if prefs.use_ai_on_new_topic == enabled {
                return;
            }
            prefs.use_ai_on_new_topic = enabled;
        }

        self.persist_preferences();
    }

    fn set_ai_timeout_secs(&self, secs: u64) {
        let clamped = secs.clamp(5, 120);
        {
            let mut prefs = self.preferences.borrow_mut();
            if prefs.ai_timeout_secs == clamped {
                return;
            }
            prefs.ai_timeout_secs = clamped;
        }
        self.persist_preferences();
    }

    fn set_ollama_url(&self, url: String) {
        let value = if url.trim().is_empty() { None } else { Some(url) };
        {
            let mut prefs = self.preferences.borrow_mut();
            if prefs.ollama_url == value {
                return;
            }
            prefs.ollama_url = value;
        }
        self.persist_preferences();
    }

    fn set_ollama_model(&self, model: String) {
        let value = if model.trim().is_empty() { None } else { Some(model) };
        {
            let mut prefs = self.preferences.borrow_mut();
            if prefs.ollama_model == value {
                return;
            }
            prefs.ollama_model = value;
        }
        self.persist_preferences();
    }

    fn set_semantic_enabled(self: &Rc<Self>, enabled: bool) {
        {
            let mut prefs = self.preferences.borrow_mut();
            if prefs.semantic_enabled == enabled {
                return;
            }
            prefs.semantic_enabled = enabled;
        }
        self.persist_preferences();
        if enabled {
            self.warm_semantic_index();
        }
    }

    fn set_embedding_model(&self, model: String) {
        let value = if model.trim().is_empty() { None } else { Some(model) };
        {
            let mut prefs = self.preferences.borrow_mut();
            if prefs.embedding_model == value {
                return;
            }
            prefs.embedding_model = value;
        }
        // Modellwechsel ⇒ anderer Vektorraum: In-Memory-Index verwerfen,
        // der Sidecar-Cache wird beim nächsten Zugriff neu aufgebaut.
        *self.embedding_cache.borrow_mut() = None;
        self.persist_preferences();
    }

    fn handle_add_submission(self: &Rc<Self>, entry: &gtk::Entry) {
        let title_text = entry.text().trim().to_string();
        if title_text.is_empty() {
            self.show_error(&t("Title must not be empty"));
            return;
        }

        let use_ai = self.use_ai_on_new_topic();
        // In the "Mein Tag" view, new todos land directly in today's plan.
        let add_to_myday = self.myday_view();

        let (line, marker) = match data::title_line(&title_text, add_to_myday) {
            Ok(rendered) => rendered,
            Err(err) => {
                self.show_error(&t("Could not create To-do: {}").replace("{}", &err.to_string()));
                return;
            }
        };
        self.submit(data::PendingOp::Add {
            line,
            marker: marker.clone(),
        });

        entry.set_text("");
        self.show_info(&t("Task added"));

        if !use_ai {
            return;
        }

        let runtime = self.ai_runtime.clone();
        let original = title_text.clone();
        let marker_for_update = Some(marker);

        glib::spawn_future_local(clone!(#[weak(rename_to = state)] self, async move {
            let Some(marker) = marker_for_update.clone() else {
                return;
            };

            let (known_projects, known_contexts) = state.collect_ai_tag_hints();
            let timeout_secs = state.ai_timeout_secs();
            let ollama_url = state.ollama_url();
            let ollama_model = state.ollama_model();
            let original_for_parse = original.clone();
            let outcome = runtime
                .spawn(async move {
                    tokio::time::timeout(
                        StdDuration::from_secs(timeout_secs),
                        request_ai_parse(original_for_parse, known_projects, known_contexts, ollama_url, ollama_model),
                    )
                    .await
                })
                .await;

            let outcome = match outcome {
                Ok(res) => res,
                Err(err) => {
                    state.show_error(&t("AI parsing failed: {}").replace("{}", &err.to_string()));
                    return;
                }
            };

            match outcome {
                Ok(Ok(parsed)) => {
                    let mut todo = build_todo_from_ai(&parsed, &original);
                    // Nur über den Marker auflösen: die Zeilennummer einer eben
                    // angelegten Aufgabe ist noch unbekannt.
                    todo.key = data::TodoKey {
                        line_index: usize::MAX,
                        marker: Some(marker.clone()),
                    };
                    // Keep the just-set myday plan; the AI rewrite would
                    // otherwise drop the token on full re-render.
                    if add_to_myday {
                        todo.myday = Some(Local::now().date_naive());
                    }

                    state.submit(data::PendingOp::Update { item: todo });
                    state.mark_recently_updated(marker.clone());
                    state.show_info(&t("Changes from file applied"));
                }
                Ok(Err(err)) => {
                    state.show_error(&t("AI parsing failed: {}").replace("{}", &err.to_string()));
                }
                Err(_) => {
                    state.show_error(&t("AI timed out. Added without AI."));
                }
            }
        }));
    }

    fn set_use_whisper(self: &Rc<Self>, use_whisper: bool, progress_bar: Option<gtk::ProgressBar>, switch_row: Option<adw::SwitchRow>, voice_btn: Option<gtk::Button>) {
        {
            let mut prefs = self.preferences.borrow_mut();
            if prefs.use_whisper == use_whisper {
                return;
            }
            prefs.use_whisper = use_whisper;
        }
        self.persist_preferences();

        if let Some(btn) = voice_btn {
            btn.set_visible(use_whisper);
        }

        if use_whisper {
            self.ensure_whisper_model(progress_bar, switch_row);
        } else {
            let path = self.whisper_model_path();
            if path.exists() {
                let _ = fs::remove_file(path);
            }
        }
    }

    fn ensure_whisper_model(self: &Rc<Self>, progress_bar: Option<gtk::ProgressBar>, switch_row: Option<adw::SwitchRow>) {
        let path = self.whisper_model_path();
        if path.exists() {
            // Basic integrity check: size should be around 480MB
            if let Ok(meta) = fs::metadata(&path)
                && meta.len() > 450 * 1024 * 1024 {
                    if let Some(row) = &switch_row {
                        row.set_sensitive(true);
                    }
                    return;
                }
            let _ = fs::remove_file(&path);
        }

        if let Some(pb) = &progress_bar {
            pb.set_visible(true);
            pb.set_fraction(0.0);
        }

        if let Some(row) = &switch_row {
            row.set_sensitive(false);
        }

        self.show_info(&t("Downloading voice model…"));

        let state = Rc::clone(self);
        let (sender, receiver) = std::sync::mpsc::channel();
        
        std::thread::spawn(move || {
            let url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin";
            let client = reqwest::blocking::Client::new();
            let mut response = match client.get(url).send() {
                Ok(r) => r,
                Err(e) => {
                    let _ = sender.send(Err(e.to_string()));
                    return;
                }
            };

            if !response.status().is_success() {
                let _ = sender.send(Err(format!("HTTP {}", response.status())));
                return;
            }

            let total_size = response.content_length().unwrap_or(0);
            let mut downloaded = 0;
            let mut buffer = [0; 32768]; // 32KB buffer
            let mut last_reported_progress = 0.0;
            
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }

            let mut file = match fs::File::create(&path) {
                Ok(f) => f,
                Err(e) => {
                    let _ = sender.send(Err(e.to_string()));
                    return;
                }
            };

            use std::io::Write;
            loop {
                match response.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Err(e) = file.write_all(&buffer[..n]) {
                            let _ = sender.send(Err(e.to_string()));
                            return;
                        }
                        downloaded += n as u64;
                        if total_size > 0 {
                            let progress = downloaded as f64 / total_size as f64;
                            // Only report progress if it changed by at least 0.1% or if we are done
                            if progress - last_reported_progress >= 0.005 || progress >= 1.0 {
                                let _ = sender.send(Ok(progress));
                                last_reported_progress = progress;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = sender.send(Err(e.to_string()));
                        return;
                    }
                }
            }
            let _ = sender.send(Ok(1.0));
        });

        let pb_clone = progress_bar.clone();
        let row_clone = switch_row.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            match receiver.try_recv() {
                Ok(Ok(fraction)) => {
                    if let Some(pb) = &pb_clone {
                        pb.set_fraction(fraction);
                    }
                    if fraction >= 1.0 {
                        state.show_info(&t("Voice model downloaded successfully"));
                        if let Some(pb) = &pb_clone {
                            pb.set_visible(false);
                        }
                        if let Some(row) = &row_clone {
                            row.set_sensitive(true);
                        }
                        return glib::ControlFlow::Break;
                    }
                    glib::ControlFlow::Continue
                }
                Ok(Err(e)) => {
                    state.show_error(&format!("{}: {}", t("Error downloading model"), e));
                    if let Some(pb) = &pb_clone {
                        pb.set_visible(false);
                    }
                    if let Some(row) = &row_clone {
                        row.set_sensitive(true);
                        row.set_active(false);
                    }
                    // Reset preference if download failed
                    {
                        let mut prefs = state.preferences.borrow_mut();
                        prefs.use_whisper = false;
                    }
                    state.persist_preferences();
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
    }

    fn set_whisper_language(&self, language: String) {
        {
            let mut prefs = self.preferences.borrow_mut();
            if prefs.whisper_language == language {
                return;
            }
            prefs.whisper_language = language;
        }
        self.persist_preferences();
    }

    fn set_use_webdav(&self, use_webdav: bool) {
        {
            let mut prefs = self.preferences.borrow_mut();
            if prefs.use_webdav == use_webdav {
                return;
            }
            prefs.use_webdav = use_webdav;
        }
        self.persist_preferences();
        
        if use_webdav {
            let (_, url, path, user, pass) = self.get_webdav_prefs();
            if let Some(u) = url {
                data::set_backend_config(data::BackendConfig::WebDav {
                    url: u,
                    path,
                    username: user,
                    password: pass,
                });
            }
        } else {
            let path = data::todo_path();
            data::set_backend_config(data::BackendConfig::Local(path));
        }

        if let Err(err) = self.reload() {
             self.show_error(&t("Could not load data: {}").replace("{}", &err.to_string()));
        }
    }

    fn set_webdav_url(&self, url: String) {
        {
            let mut prefs = self.preferences.borrow_mut();
            prefs.webdav_url = Some(url.clone());
        }
        self.persist_preferences();
        
        let (use_webdav, _, path, user, pass) = self.get_webdav_prefs();
        if use_webdav {
             data::set_backend_config(data::BackendConfig::WebDav {
                url,
                path,
                username: user,
                password: pass,
            });
        }
    }

    fn set_webdav_path(&self, path: String) {
        // An empty field means "not configured" — storing Some("") would only
        // append a slash to the URL and silently target the parent folder.
        let trimmed = path.trim();
        let normalized = (!trimmed.is_empty()).then(|| trimmed.to_string());
        {
            let mut prefs = self.preferences.borrow_mut();
            prefs.webdav_path = normalized.clone();
        }
        self.persist_preferences();
        
        let (use_webdav, url, _, user, pass) = self.get_webdav_prefs();
        if use_webdav
            && let Some(u) = url {
                data::set_backend_config(data::BackendConfig::WebDav {
                    url: u,
                    path: normalized,
                    username: user,
                    password: pass,
                });
            }
    }

    fn set_webdav_username(&self, username: String) {
        {
            let mut prefs = self.preferences.borrow_mut();
            prefs.webdav_username = Some(username.clone());
        }
        self.persist_preferences();

        let (use_webdav, url, path, _, pass) = self.get_webdav_prefs();
        if use_webdav
            && let Some(u) = url {
                data::set_backend_config(data::BackendConfig::WebDav {
                    url: u,
                    path,
                    username: Some(username),
                    password: pass,
                });
            }
    }

    fn set_webdav_password(&self, password: String) {
        {
            let mut prefs = self.preferences.borrow_mut();
            prefs.webdav_password = Some(password.clone());
        }
        self.persist_preferences();

        let (use_webdav, url, path, user, _) = self.get_webdav_prefs();
        if use_webdav
            && let Some(u) = url {
                data::set_backend_config(data::BackendConfig::WebDav {
                    url: u,
                    path,
                    username: user,
                    password: Some(password),
                });
            }
    }

    fn get_webdav_prefs(&self) -> (bool, Option<String>, Option<String>, Option<String>, Option<String>) {
        let prefs = self.preferences.borrow();
        (prefs.use_webdav, prefs.webdav_url.clone(), prefs.webdav_path.clone(), prefs.webdav_username.clone(), prefs.webdav_password.clone())
    }


    fn set_sort_mode(&self, mode: SortMode) {
        {
            let mut current = self.sort_mode.borrow_mut();
            if *current == mode {
                return;
            }
            *current = mode;
        }

        {
            let mut prefs = self.preferences.borrow_mut();
            prefs.sort_mode = Some(mode.as_key().to_string());
        }

        self.persist_preferences();

        self.repopulate_store();
    }

    /// "Mein Tag": oben die heute geplanten Aufgaben als flache Liste
    /// (erledigte bleiben durchgestrichen sichtbar, unter "Erledigt" ans Ende
    /// sortiert), darunter der Planungs-Picker mit fälligen Vorschlägen
    /// zuerst, je nach Sortiermodus nach Thema/Ort untergliedert.
    fn populate_myday_view(&self, out: &mut Vec<ListEntry>, items: Vec<TodoItem>, today: NaiveDate) {
        let mode = *self.sort_mode.borrow();
        let (canon_projects, canon_contexts) = self.canonical_tag_maps();

        let (planned, rest): (Vec<TodoItem>, Vec<TodoItem>) =
            items.into_iter().partition(|todo| todo.myday == Some(today));

        if planned.is_empty() {
            out.push(ListEntry::Header(t("Nothing planned yet — add tasks for today.")));
        } else {
            // Flache Liste ohne Themen-Header: aktive oben, erledigte darunter.
            let (active, done): (Vec<TodoItem>, Vec<TodoItem>) =
                planned.into_iter().partition(|todo| !todo.done);
            for item in active {
                out.push(ListEntry::Item(item));
            }
            if !done.is_empty() {
                out.push(ListEntry::Header(t("Completed")));
                for item in done {
                    out.push(ListEntry::Item(item));
                }
            }
        }

        let filter = self.filter();
        let (suggestions, other_open) = split_picker_candidates(rest, today, &filter);

        if suggestions.is_empty() && other_open.is_empty() {
            out.push(ListEntry::Header(if filter.is_active() {
                t("No tasks match the filter.")
            } else {
                t("No open tasks left to plan.")
            }));
            return;
        }

        // Innerhalb eines Picker-Abschnitts nach Thema/Ort untergliedern
        // (im Datums-Modus liefert group_label None → flache Liste).
        let mut append_picker_section = |title: String, mut items: Vec<TodoItem>| {
            if items.is_empty() {
                return;
            }
            out.push(ListEntry::Header(title));
            if mode != SortMode::Date {
                sort_items(&mut items, mode);
            }
            let mut last_group: Option<String> = None;
            for item in items {
                if let Some(label) =
                    self.group_label(mode, &item, &canon_projects, &canon_contexts)
                    && last_group.as_ref() != Some(&label) {
                        out.push(ListEntry::Header(label.clone()));
                        last_group = Some(label);
                    }
                out.push(ListEntry::PickerItem(item));
            }
        };

        append_picker_section(t("Suggestions (due)"), suggestions);
        append_picker_section(t("Other open tasks"), other_open);
    }

    fn repopulate_store(&self) {
        let mut selected_key = None;
        let mut scroll_pos = None;

        if let Some(scrolled) = self.scrolled_window.borrow().as_ref() {
            let adj = scrolled.vadjustment();
            scroll_pos = Some(adj.value());
        }

        if let Some(list_view) = self.list_view.borrow().as_ref()
            && let Some(model) = list_view.model()
                && let Ok(selection) = model.downcast::<gtk::SingleSelection>() {
                    let pos = selection.selected();
                    if pos != gtk::INVALID_LIST_POSITION
                        && let Some(obj) = self.store.item(pos)
                            && let Ok(boxed) = obj.downcast::<BoxedAnyObject>() {
                                let entry = boxed.borrow::<ListEntry>();
                                if let ListEntry::Item(todo) = &*entry {
                                    selected_key = Some(todo.key.clone());
                                }
                            }
                }

        let search_term = self.search_term.borrow().to_lowercase();
        let mut items = self.cached_items.borrow().clone();
        self.sort_items(&mut items);
        let mut out: Vec<ListEntry> = Vec::new();

        let include_done = self.show_completed();
        let filter = self.filter();
        let myday_only = self.myday_view();
        let today = Local::now().date_naive();

        if search_term.is_empty() {
            if myday_only {
                self.populate_myday_view(&mut out, items, today);
            } else {
                let mode = *self.sort_mode.borrow();
                let (canon_projects, canon_contexts) = self.canonical_tag_maps();
                let mut last_group: Option<String> = None;
                for item in items.into_iter().filter(|todo| {
                    (include_done || !todo.done) && filter.matches(todo, today)
                }) {
                    if let Some(label) =
                        self.group_label(mode, &item, &canon_projects, &canon_contexts)
                        && last_group.as_ref() != Some(&label) {
                            out.push(ListEntry::Header(label.clone()));
                            last_group = Some(label);
                        }
                    out.push(ListEntry::Item(item));
                }
                if out.is_empty() && filter.is_active() {
                    out.push(ListEntry::Header(t("No tasks match the filter.")));
                }
            }
        } else {
            // 1. Suchergebnisse in aktueller Liste
            let current_list_results: Vec<_> = items.iter().filter(|todo| {
                (include_done || !todo.done)
                    && filter.matches(todo, today)
                    && todo.title.to_lowercase().contains(&search_term)
            }).cloned().collect();

            if !current_list_results.is_empty() {
                out.push(ListEntry::Header(t("Search results in current list")));
                for item in current_list_results.clone() {
                    out.push(ListEntry::Item(item));
                }
            }

            // 2. Suchergebnisse bei allen offenen Todos
            let open_results: Vec<_> = items.iter().filter(|todo| {
                !todo.done && todo.title.to_lowercase().contains(&search_term)
            }).cloned().collect();
            
            let open_results_filtered: Vec<_> = open_results.into_iter().filter(|todo| {
                !current_list_results.iter().any(|c| c.key.line_index == todo.key.line_index && c.key.marker == todo.key.marker)
            }).collect();

            if !open_results_filtered.is_empty() {
                out.push(ListEntry::Header(t("Search results in all open To-dos")));
                for item in open_results_filtered {
                    out.push(ListEntry::Item(item));
                }
            }

            // 3. Suchergebnisse bei den abgeschlossenen Todos
            let done_results: Vec<_> = items.iter().filter(|todo| {
                todo.done && todo.title.to_lowercase().contains(&search_term)
            }).cloned().collect();

            let done_results_filtered: Vec<_> = done_results.into_iter().filter(|todo| {
                !current_list_results.iter().any(|c| c.key.line_index == todo.key.line_index && c.key.marker == todo.key.marker)
            }).collect();

            if !done_results_filtered.is_empty() {
                out.push(ListEntry::Header(t("Search results in completed To-dos")));
                for item in done_results_filtered {
                    out.push(ListEntry::Item(item));
                }
            }
        }

        let anchors = self
            .list_view
            .borrow()
            .as_ref()
            .map(visible_rows)
            .unwrap_or_default();
        let update = self.apply_entries(out);
        if let ListUpdate::Partial { refocus_y } = update {
            self.keep_rows_in_place(anchors, refocus_y);
        }

        // Auswahl wiederherstellen, ohne ihr hinterherzuscrollen — sonst
        // springt die Sicht z. B. der nach „Erledigt" verschobenen Aufgabe
        // hinterher. Die Scroll-Position bleibt beim Neuaufbau stabil.
        if let Some(key) = selected_key
            && let Some(list_view) = self.list_view.borrow().as_ref()
                && let Some(model) = list_view.model()
                    && let Ok(selection) = model.downcast::<gtk::SingleSelection>() {
                        for i in 0..self.store.n_items() {
                            if let Some(obj) = self.store.item(i)
                                && let Ok(boxed) = obj.downcast::<BoxedAnyObject>() {
                                    let entry = boxed.borrow::<ListEntry>();
                                    if let ListEntry::Item(todo) = &*entry
                                        && todo.key == key {
                                            selection.set_selected(i);
                                            break;
                                        }
                                }
                        }
                    }

        // Beim Teilaustausch übernimmt das `keep_rows_in_place`.
        if update == ListUpdate::Full
            && let Some(pos) = scroll_pos
            && let Some(scrolled) = self.scrolled_window.borrow().as_ref() {
                // Nach dem Komplettaustausch kennt die ListView ihre Höhe
                // zunächst nur geschätzt; ein einzelner Idle-Restore klemmt die
                // Position dann auf einen zu kleinen Wert. Daher über einige
                // Frames erneut anwenden, bis die Zeilen vermessen sind.
                let adj = scrolled.vadjustment();
                let frames = std::cell::Cell::new(0u8);
                scrolled.add_tick_callback(move |_, _| {
                    let max = (adj.upper() - adj.page_size()).max(0.0);
                    adj.set_value(pos.min(max));
                    frames.set(frames.get() + 1);
                    if frames.get() >= 3 {
                        glib::ControlFlow::Break
                    } else {
                        glib::ControlFlow::Continue
                    }
                });
            }
    }

    /// Nach dem nächsten Layout, noch vor dem Zeichnen, so scrollen, dass
    /// die oberste noch vorhandene der vorher sichtbaren Zeilen pixelgenau
    /// dort steht, wo sie stand. Die Anpassung klemmt GtkAdjustment selbst:
    /// rutscht unten nichts mehr nach, bewegt sich die Ansicht nach unten.
    ///
    /// Die ListView hält ihre Position sonst über einen eigenen Anker; der
    /// geht verloren, wenn die Zeile verschwindet, aus der heraus geklickt
    /// wurde (der „+“-Knopf im Picker), und die Sicht springt.
    fn keep_rows_in_place(&self, anchors: Vec<(BoxedAnyObject, f32)>, refocus_y: Option<f32>) {
        if anchors.is_empty() {
            return;
        }
        let Some(list_view) = self.list_view.borrow().clone() else {
            return;
        };
        let Some(clock) = list_view.frame_clock() else {
            return;
        };
        let store = self.store.clone();
        let handler: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::new(RefCell::new(None));
        let handler_inner = Rc::clone(&handler);
        let tries = Cell::new(0u8);
        let lv = list_view.clone();
        let id = clock.connect_layout(move |clock| {
            let now = visible_rows(&lv);
            let found = anchors.iter().find_map(|(obj, before)| {
                now.iter().find(|(o, _)| o == obj).map(|(_, after)| after - before)
            });
            let done = match found {
                Some(delta) => {
                    // Was die Adjustment nach dem Klemmen tatsächlich verschoben hat.
                    let mut applied = 0.0;
                    if delta.abs() >= 0.5 && let Some(adj) = lv.vadjustment() {
                        let before = adj.value();
                        adj.set_value(before + f64::from(delta));
                        applied = (adj.value() - before) as f32;
                    }
                    // Fokus der Zeile geben, die nach der Korrektur dort
                    // steht, wo die fokussierte stand (Tastaturbedienung).
                    // Gemessen wird noch im Layout vor der Korrektur.
                    if let Some(y) = refocus_y {
                        focus_row_at(&lv, y + applied);
                    }
                    true
                }
                // Die ListView ist so weit gesprungen, dass keine der
                // Zeilen mehr gebaut ist: erst zurückholen, dann messen.
                None => {
                    let target = anchors.iter().find_map(|(obj, _)| {
                        (0..store.n_items()).find(|&i| {
                            store.item(i).is_some_and(|o| o.as_ptr() == obj.upcast_ref::<glib::Object>().as_ptr())
                        })
                    });
                    tries.set(tries.get() + 1);
                    match target {
                        Some(pos) if tries.get() <= 3 => {
                            lv.scroll_to(pos, gtk::ListScrollFlags::NONE, None);
                            false
                        }
                        _ => true,
                    }
                }
            };
            if done && let Some(id) = handler_inner.borrow_mut().take() {
                clock.disconnect(id);
            }
        });
        *handler.borrow_mut() = Some(id);
    }

    /// Die Liste auf `entries` bringen, ohne sie erst zu leeren.
    ///
    /// Vorher wurde der Store bei jedem Neuaufbau geleert und neu befüllt:
    /// die ListView sprang dabei an den Anfang, baute jede Zeile neu und
    /// wurde dann zurückgescrollt — die Liste blinkte. Jetzt werden nur die
    /// Zeilen eingefügt oder entfernt, die sich geändert haben; alle anderen
    /// behalten Objekt, Zeile und Position.
    ///
    /// `Full`, wenn alles ersetzt wurde, weil sich Zustand geändert hat, den
    /// jede Zeile beim Binden ausliest.
    fn apply_entries(&self, entries: Vec<ListEntry>) -> ListUpdate {
        let context = RowContext {
            highlight: self.recently_updated.borrow().clone(),
            selection_mode: self.selection_mode.get(),
            selected: self.selected_markers.borrow().clone(),
            compact: self.compact.get(),
            myday: self.myday_view(),
        };
        let previous = self.row_context.replace(Some(context.clone()));
        let full_rebuild = previous.as_ref().is_none_or(|prev| {
            prev.selection_mode != context.selection_mode
                || prev.compact != context.compact
                || prev.myday != context.myday
        });

        // Marker, deren Hervorhebung oder Auswahl sich geändert hat: deren
        // Zeilen müssen neu gebunden werden, auch wenn der Eintrag gleich ist.
        let mut dirty: HashSet<String> = HashSet::new();
        if let Some(prev) = previous.as_ref() {
            if prev.highlight != context.highlight {
                dirty.extend(prev.highlight.iter().cloned());
                dirty.extend(context.highlight.iter().cloned());
            }
            dirty.extend(prev.selected.symmetric_difference(&context.selected).cloned());
        }
        let reusable = |entry: &ListEntry| match entry {
            ListEntry::Item(todo) => todo.key.marker.as_ref().is_none_or(|m| !dirty.contains(m)),
            _ => true,
        };

        let old: Vec<BoxedAnyObject> = (0..self.store.n_items())
            .filter_map(|i| self.store.item(i).and_downcast::<BoxedAnyObject>())
            .collect();
        if full_rebuild {
            let objects: Vec<BoxedAnyObject> = entries.into_iter().map(BoxedAnyObject::new).collect();
            self.store.splice(0, old.len() as u32, &objects);
            return ListUpdate::Full;
        }

        let same = |obj: &BoxedAnyObject, entry: &ListEntry| {
            reusable(entry) && *obj.borrow::<ListEntry>() == *entry
        };

        // Längste gemeinsame Teilfolge: alles darin bleibt mit seinem Objekt
        // an Ort und Stelle. Ein einziger splice über den ganzen geänderten
        // Bereich reicht nicht — wandert eine Aufgabe von unten (Picker)
        // nach oben, umfasst er fast die ganze Liste, und die ListView rät
        // dann nur noch, wo sie stand.
        let (n, m) = (old.len(), entries.len());
        let mut lcs = vec![0u32; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * (m + 1) + j] = if same(&old[i], &entries[j]) {
                    lcs[(i + 1) * (m + 1) + j + 1] + 1
                } else {
                    lcs[(i + 1) * (m + 1) + j].max(lcs[i * (m + 1) + j + 1])
                };
            }
        }

        // Abschnitte (alte Position, Anzahl entfernt, Bereich neu) sammeln …
        let mut hunks: Vec<(usize, usize, std::ops::Range<usize>)> = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && same(&old[i], &entries[j]) {
                i += 1;
                j += 1;
                continue;
            }
            let (start_i, start_j) = (i, j);
            while (i < n || j < m) && !(i < n && j < m && same(&old[i], &entries[j])) {
                if j >= m || (i < n && lcs[(i + 1) * (m + 1) + j] >= lcs[i * (m + 1) + j + 1]) {
                    i += 1;
                } else {
                    j += 1;
                }
            }
            hunks.push((start_i, i - start_i, start_j..j));
        }

        if hunks.is_empty() {
            return ListUpdate::Unchanged;
        }

        let refocus_y = self.release_list_focus();

        // … und von hinten anwenden, damit die vorderen Positionen stimmen.
        let mut entries: Vec<Option<ListEntry>> = entries.into_iter().map(Some).collect();
        for (pos, removed, range) in hunks.into_iter().rev() {
            let added: Vec<BoxedAnyObject> = range
                .filter_map(|k| entries[k].take())
                .map(BoxedAnyObject::new)
                .collect();
            self.store.splice(pos as u32, removed as u32, &added);
        }
        ListUpdate::Partial { refocus_y }
    }

    /// Liegt der Fokus in der Liste, ihn abnehmen und die y-Position seiner
    /// Zeile liefern. Verschwindet die fokussierte Zeile (der „+“-Knopf im
    /// Picker), gibt GtkListBase den Fokus sonst einer Ersatzzeile und
    /// scrollt zu ihr — bis an den Anfang der Liste.
    fn release_list_focus(&self) -> Option<f32> {
        let window = self.window.upgrade()?;
        let list_view = self.list_view.borrow().clone()?;
        let focus = gtk::prelude::GtkWindowExt::focus(&window)?;
        if !focus.is_ancestor(&list_view) {
            return None;
        }
        let mut row = focus;
        while row.parent().as_ref() != Some(list_view.upcast_ref()) {
            row = row.parent()?;
        }
        let y = row.compute_point(&list_view, &gtk::graphene::Point::new(0.0, 0.0))?.y();
        gtk::prelude::GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
        Some(y + row.height() as f32 / 2.0)
    }

    fn persist_preferences(&self) {
        let prefs = self.preferences.borrow().clone();
        if let Err(err) = write_preferences(&prefs) {
            eprintln!("{}: {err}", t("Could not save settings: {}"));
        }
    }

    fn save_item(self: &Rc<Self>, updated: &TodoItem) {
        self.submit(data::PendingOp::Update {
            item: updated.clone(),
        });
        self.show_undo_toast(&t("Updated: {}").replace("{}", &updated.title));
    }

    fn toggle_recording(self: &Rc<Self>, voice_btn: &gtk::Button, entry: &gtk::Entry) {
        if self.is_recording.load(AtomicOrdering::SeqCst) {
            self.is_recording.store(false, AtomicOrdering::SeqCst);
            voice_btn.remove_css_class("destructive-action");
            voice_btn.set_icon_name("audio-input-microphone-symbolic");
            return;
        }

        let model_path = self.whisper_model_path();
        if !model_path.exists() {
            self.show_error(&t("Speech model not found. Please enable it in the settings."));
            return;
        }

        self.is_recording.store(true, AtomicOrdering::SeqCst);
        voice_btn.add_css_class("destructive-action");
        voice_btn.set_icon_name("media-record-symbolic");

        let is_recording = self.is_recording.clone();
        let (sender, receiver) = std::sync::mpsc::channel::<VoiceMsg>();
        
        {
            let state_clone = Rc::clone(self);
            let voice_btn_clone = voice_btn.clone();
            let entry_clone = entry.clone();
            
            glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
                let mut finished = false;
                while let Ok(msg) = receiver.try_recv() {
                    match msg {
                        VoiceMsg::Error(e) => {
                            state_clone.show_error(&e);
                            voice_btn_clone.remove_css_class("destructive-action");
                            voice_btn_clone.remove_css_class("pulse");
                            voice_btn_clone.set_icon_name("audio-input-microphone-symbolic");
                            state_clone.is_recording.store(false, AtomicOrdering::SeqCst);
                            finished = true;
                        }
                        VoiceMsg::Transcription(text) => {
                            let current = entry_clone.text();
                            if current.is_empty() {
                                entry_clone.set_text(&text);
                            } else {
                                entry_clone.set_text(&format!("{} {}", current, text));
                            }
                        }
                        VoiceMsg::Transcribing => {
                            voice_btn_clone.remove_css_class("destructive-action");
                            voice_btn_clone.add_css_class("pulse");
                            voice_btn_clone.set_icon_name("audio-input-microphone-symbolic");
                        }
                        VoiceMsg::Finished => {
                            voice_btn_clone.remove_css_class("destructive-action");
                            voice_btn_clone.remove_css_class("pulse");
                            voice_btn_clone.set_icon_name("audio-input-microphone-symbolic");
                            state_clone.is_recording.store(false, AtomicOrdering::SeqCst);
                            finished = true;
                        }
                    }
                }
                if finished {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
        }

        let language = self.whisper_language();

        std::thread::spawn(move || {
            let host = cpal::default_host();
            let device = match host.default_input_device() {
                Some(d) => d,
                None => {
                    let _ = sender.send(VoiceMsg::Error("No input device found".to_string()));
                    return;
                }
            };

            let config = match device.default_input_config() {
                Ok(c) => c,
                Err(e) => {
                    let _ = sender.send(VoiceMsg::Error(format!("Input config error: {}", e)));
                    return;
                }
            };

            let audio_data = Arc::new(Mutex::new(Vec::new()));
            let audio_data_clone = audio_data.clone();

            let stream = match device.build_input_stream(
                &config.clone().into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mut buffer = audio_data_clone.lock().unwrap();
                    buffer.extend_from_slice(data);
                },
                |_| {},
                None,
            ) {
                Ok(s) => s,
                Err(e) => {
                    let _ = sender.send(VoiceMsg::Error(format!("Stream error: {}", e)));
                    return;
                }
            };

            if let Err(e) = stream.play() {
                let _ = sender.send(VoiceMsg::Error(format!("Stream play error: {}", e)));
                return;
            }

            while is_recording.load(AtomicOrdering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }

            drop(stream);

            let samples = audio_data.lock().unwrap().clone();
            if samples.is_empty() {
                let _ = sender.send(VoiceMsg::Finished);
                return;
            }

            // Convert to mono if necessary
            let channels = config.channels() as usize;
            let mono_samples = if channels > 1 {
                let mut mono = Vec::with_capacity(samples.len() / channels);
                for chunk in samples.chunks_exact(channels) {
                    let sum: f32 = chunk.iter().sum();
                    mono.push(sum / channels as f32);
                }
                mono
            } else {
                samples
            };

            // Resample to 16kHz if necessary (Whisper requirement)
            let sample_rate = config.sample_rate().0;
            let samples_16k = if sample_rate != 16000 {
                let mut resampled = Vec::new();
                let ratio = sample_rate as f32 / 16000.0;
                let mut i = 0.0;
                while i < mono_samples.len() as f32 {
                    resampled.push(mono_samples[i as usize]);
                    i += ratio;
                }
                resampled
            } else {
                mono_samples
            };

            println!("Starting transcription ({} samples, {}Hz, language: {})", samples_16k.len(), sample_rate, language);
            let _ = sender.send(VoiceMsg::Transcribing);

            let ctx = match WhisperContext::new_with_params(
                &model_path,
                WhisperContextParameters::default(),
            ) {
                Ok(c) => c,
                Err(e) => {
                    let _ = sender.send(VoiceMsg::Error(format!("Whisper error: {}", e)));
                    return;
                }
            };

            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_n_threads(4);
            if language != "auto" {
                params.set_language(Some(&language));
            } else {
                params.set_language(None);
            }

            let mut state_whisper = ctx.create_state().expect("failed to create state");
            if let Err(e) = state_whisper.full(params, &samples_16k) {
                let _ = sender.send(VoiceMsg::Error(format!("Transcription error: {}", e)));
                return;
            }

            let num_segments = state_whisper.full_n_segments();
            let mut result_text = String::new();
            for i in 0..num_segments {
                if let Some(segment) = state_whisper.get_segment(i)
                    && let Ok(text) = segment.to_str_lossy() {
                        result_text.push_str(&text);
                    }
            }

            let final_text = result_text.trim().to_string();
            if !final_text.is_empty() {
                let _ = sender.send(VoiceMsg::Transcription(final_text));
            }
            let _ = sender.send(VoiceMsg::Finished);
        });
    }

    fn open_entry_at(self: &Rc<Self>, position: u32) {
        let Some(obj) = self.store.item(position) else {
            return;
        };
        let Ok(todo_obj) = obj.downcast::<BoxedAnyObject>() else {
            return;
        };
        let (item, picker) = {
            let entry = todo_obj.borrow::<ListEntry>();
            match &*entry {
                ListEntry::Item(todo) => (Some(todo.clone()), false),
                ListEntry::PickerItem(todo) => (Some(todo.clone()), true),
                ListEntry::Header(_) => (None, false),
            }
        };

        let Some(todo) = item else {
            return;
        };

        // Im Planungs-Picker öffnet Aktivieren (Enter/Klick) den normalen
        // Bearbeiten-Dialog; in „Mein Tag" übernimmt nur das Plus (bzw. die
        // Leertaste).
        if picker {
            self.show_details_dialog(&todo);
            return;
        }

        // Im Auswahlmodus toggelt ein Klick auf die Zeile die Auswahl
        // statt den Bearbeiten-Dialog zu öffnen.
        if self.selection_mode.get() {
            self.toggle_selection_at(position);
            return;
        }

        self.show_details_dialog(&todo);
    }

    /// Auswahl an einer Listenposition umschalten (Klick auf die Zeile oder
    /// Leertaste im Auswahlmodus) und die Zeile neu binden, damit Checkbox
    /// und Hervorhebung folgen.
    fn toggle_selection_at(self: &Rc<Self>, position: u32) {
        let Some(obj) = self.store.item(position) else {
            return;
        };
        let Ok(todo_obj) = obj.downcast::<BoxedAnyObject>() else {
            return;
        };
        let marker = match &*todo_obj.borrow::<ListEntry>() {
            ListEntry::Item(todo) => todo.key.marker.clone(),
            _ => None,
        };
        let Some(marker) = marker else {
            return;
        };
        let selected = !self.selected_markers.borrow().contains(marker.as_str());
        self.toggle_selection_marker(&marker, selected);
        self.store.splice(position, 1, &[todo_obj]);
    }

    /// Löschen per Entf-Taste: respektiert die „Ohne Rückfrage löschen"-
    /// Einstellung und zeigt sonst denselben Bestätigungsdialog wie der
    /// Bearbeiten-Dialog; danach Undo-Toast.
    fn request_delete(self: &Rc<Self>, todo: &TodoItem) {
        let perform_delete = Rc::new({
            let state = Rc::clone(self);
            let todo = todo.clone();
            move || {
                state.submit(data::PendingOp::Delete {
                    keys: vec![todo.key.clone()],
                });
                state.show_undo_toast(&t("Deleted: {}").replace("{}", &todo.title));
            }
        });

        if self.skip_delete_confirmation() {
            perform_delete();
            return;
        }

        let Some(parent) = self.window.upgrade() else {
            perform_delete();
            return;
        };

        let confirm_dialog = AlertDialog::builder().modal(true).build();
        confirm_dialog.set_message(&t("Delete this entry?"));
        confirm_dialog.set_detail(&t("This cannot be undone."));
        confirm_dialog.set_buttons(&[&t("Delete"), &t("Cancel")]);
        confirm_dialog.set_default_button(1);
        confirm_dialog.set_cancel_button(1);

        let perform_delete_cb = perform_delete.clone();
        confirm_dialog.choose(
            Some(&parent),
            Option::<&gio::Cancellable>::None,
            move |result| {
                if let Ok(0) = result {
                    perform_delete_cb();
                }
            },
        );
    }

    /// Die zuletzt angelegte Aufgabe mit diesem Titel als neue offene
    /// Aufgabe duplizieren (Issue #6): Projekte, Kontexte, Notiz und
    /// Wiederholung bleiben erhalten; Fälligkeit wird wie bei einer
    /// Neuanlage auf heute gesetzt.
    fn duplicate_by_title(self: &Rc<Self>, title: &str) {
        let source = {
            let items = self.cached_items.borrow();
            items
                .iter()
                .filter(|item| item.title == title)
                .max_by_key(|item| item.key.line_index)
                .cloned()
        };
        let Some(source) = source else {
            return;
        };

        let mut copy = source;
        copy.done = false;
        copy.myday = None;
        let today = Local::now().date_naive();
        copy.due = Some(NaiveDateTime::new(today, DEFAULT_DUE_TIME));
        copy.key = data::TodoKey {
            line_index: 0,
            marker: None,
        };

        match data::item_line(&copy) {
            Ok((line, marker)) => {
                self.submit(data::PendingOp::Add {
                    line,
                    marker: marker.clone(),
                });
                self.mark_recently_updated(marker);
                self.show_undo_toast(&t("Duplicated: {}").replace("{}", title));
            }
            Err(err) => self.show_error(&err.to_string()),
        }
    }

    // ----- Mehrfachauswahl (Issue #8) ------------------------------------

    fn set_selection_mode(self: &Rc<Self>, active: bool) {
        if self.selection_mode.get() == active {
            return;
        }
        self.selection_mode.set(active);
        if !active {
            self.selected_markers.borrow_mut().clear();
        }
        if let Some(revealer) = self.selection_bar.borrow().as_ref() {
            revealer.set_reveal_child(active);
        }
        if let Some(toggle) = self.selection_toggle.borrow().as_ref()
            && toggle.is_active() != active {
                toggle.set_active(active);
            }
        self.update_selection_count();
        self.repopulate_store();
    }

    /// Kompaktmodus (Issue #12) umschalten und die sichtbaren Zeilen
    /// aktualisieren; das Fenster bekommt zusätzlich die CSS-Klasse
    /// `compact` für Touch-Anpassungen.
    fn set_compact(self: &Rc<Self>, compact: bool) {
        if self.compact.get() == compact {
            return;
        }
        self.compact.set(compact);
        if let Some(window) = self.window.upgrade() {
            if compact {
                window.add_css_class("compact");
            } else {
                window.remove_css_class("compact");
            }
        }
        self.refresh_row_compact_state();
        // Bereits gebundene Zeilen neu aufbauen: der Modus kann sich ändern,
        // bevor die ersten Zeilen gebunden wurden (Start bei schmalem Fenster).
        self.repopulate_store();
    }

    /// Sichtbarkeiten der Zeilen-Buttons an den Kompaktmodus anpassen.
    /// Nötig, weil die Zeilen beim Umschalten bereits gebunden sind.
    fn refresh_row_compact_state(&self) {
        let Some(list_view) = self.list_view.borrow().as_ref().cloned() else {
            return;
        };
        let compact = self.compact.get();
        let selection_mode = self.selection_mode.get();
        let children = list_view.observe_children();
        for i in 0..children.n_items() {
            let Some(child) = children.item(i) else {
                continue;
            };
            let Ok(list_item) = child.downcast::<gtk::ListItem>() else {
                continue;
            };
            for button_key in [
                "todo-myday-btn",
                "todo-today-btn",
                "todo-button",
                "todo-weekend-btn",
                "todo-sometimes-btn",
            ] {
                if let Some(btn_ref_ptr) = unsafe {
                    list_item.data::<glib::WeakRef<gtk::Button>>(button_key)
                }
                    && let Some(btn) = unsafe { btn_ref_ptr.as_ref() }.upgrade() {
                        btn.set_visible(!selection_mode && !compact);
                    }
            }
            if let Some(menu_ref_ptr) = unsafe {
                list_item.data::<glib::WeakRef<gtk::MenuButton>>("todo-menu-btn")
            }
                && let Some(menu_widget) = unsafe { menu_ref_ptr.as_ref() }.upgrade() {
                    menu_widget.set_visible(!selection_mode && compact);
                    if compact {
                        menu_widget.add_css_class("compact-touch");
                    } else {
                        menu_widget.remove_css_class("compact-touch");
                    }
                }
            if let Some(check_ref_ptr) = unsafe {
                list_item.data::<glib::WeakRef<gtk::CheckButton>>("todo-check")
            }
                && let Some(check_widget) = unsafe { check_ref_ptr.as_ref() }.upgrade() {
                    if compact {
                        check_widget.add_css_class("compact-touch");
                    } else {
                        check_widget.remove_css_class("compact-touch");
                    }
                }
        }
    }

    fn toggle_selection_marker(self: &Rc<Self>, marker: &str, selected: bool) {
        {
            let mut set = self.selected_markers.borrow_mut();
            if selected {
                set.insert(marker.to_string());
            } else {
                set.remove(marker);
            }
        }
        self.update_selection_count();
    }

    fn update_selection_count(&self) {
        if let Some(label) = self.selection_count_label.borrow().as_ref() {
            let count = self.selected_markers.borrow().len();
            label.set_text(&t("{} selected").replace("{}", &count.to_string()));
        }
    }

    /// Auswahl-Marker auf TodoKeys der aktuell geladenen Einträge abbilden.
    fn selected_keys(&self) -> Vec<data::TodoKey> {
        let set = self.selected_markers.borrow();
        self.cached_items
            .borrow()
            .iter()
            .filter(|item| {
                item.key
                    .marker
                    .as_deref()
                    .map(|m| set.contains(m))
                    .unwrap_or(false)
            })
            .map(|item| item.key.clone())
            .collect()
    }

    /// Gemeinsamer Abschluss aller Massenaktionen: Modus verlassen,
    /// Änderung einreihen, Undo-Toast mit Anzahl zeigen.
    fn submit_bulk_action(self: &Rc<Self>, op: data::PendingOp, count: usize, message: &str) {
        self.set_selection_mode(false);
        self.submit(op);
        self.show_undo_toast(&message.replace("{}", &count.to_string()));
    }

    fn bulk_complete(self: &Rc<Self>, done: bool) {
        let keys = self.selected_keys();
        if keys.is_empty() {
            return;
        }
        let message = if done {
            t("{} items completed")
        } else {
            t("{} items reopened")
        };
        let count = keys.len();
        self.submit_bulk_action(data::PendingOp::SetDone { keys, done }, count, &message);
    }

    fn bulk_set_due(self: &Rc<Self>, target: data::DueTarget) {
        let keys = self.selected_keys();
        if keys.is_empty() {
            return;
        }
        let count = keys.len();
        self.submit_bulk_action(
            data::PendingOp::SetDue { keys, target },
            count,
            &t("Due date set for {} items"),
        );
    }

    fn bulk_delete(self: &Rc<Self>) {
        let keys = self.selected_keys();
        if keys.is_empty() {
            return;
        }

        let perform_delete = Rc::new({
            let state = Rc::clone(self);
            move || {
                state.submit_bulk_action(
                    data::PendingOp::Delete { keys: keys.clone() },
                    keys.len(),
                    &t("{} items deleted"),
                );
            }
        });

        if self.skip_delete_confirmation() {
            perform_delete();
            return;
        }

        let Some(parent) = self.window.upgrade() else {
            perform_delete();
            return;
        };

        let count = self.selected_markers.borrow().len();
        let confirm_dialog = AlertDialog::builder().modal(true).build();
        confirm_dialog.set_message(
            &t("Delete {} items?").replace("{}", &count.to_string()),
        );
        confirm_dialog.set_detail(&t("This cannot be undone."));
        confirm_dialog.set_buttons(&[&t("Delete"), &t("Cancel")]);
        confirm_dialog.set_default_button(1);
        confirm_dialog.set_cancel_button(1);

        let perform_delete_cb = perform_delete.clone();
        confirm_dialog.choose(
            Some(&parent),
            Option::<&gio::Cancellable>::None,
            move |result| {
                if let Ok(0) = result {
                    perform_delete_cb();
                }
            },
        );
    }

    fn bulk_assign(self: &Rc<Self>) {
        let keys = self.selected_keys();
        if keys.is_empty() {
            return;
        }
        let Some(parent) = self.window.upgrade() else {
            self.show_error(&t("No window available"));
            return;
        };

        let dialog = adw::Window::builder()
            .title(t("Assign project/context"))
            .transient_for(&parent)
            .modal(true)
            .default_width(380)
            .build();
        dialog.set_destroy_with_parent(true);

        let key_controller = gtk::EventControllerKey::new();
        let dialog_for_esc = dialog.clone();
        key_controller.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gdk::Key::Escape {
                dialog_for_esc.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        dialog.add_controller(key_controller);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(16);
        content.set_margin_bottom(16);
        content.set_margin_start(20);
        content.set_margin_end(20);

        let (known_projects, known_contexts) = self.collect_existing_tags();

        let (project_entry, project_input_box) =
            create_suggestion_entry("", &known_projects, "+");
        let project_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        project_row.append(&gtk::Label::builder().label(t("Project (+)")).xalign(0.0).build());
        project_row.append(&project_input_box);
        content.append(&project_row);

        let (context_entry, context_input_box) =
            create_suggestion_entry("", &known_contexts, "@");
        let context_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        context_row.append(&gtk::Label::builder().label(t("Location (@)")).xalign(0.0).build());
        context_row.append(&context_input_box);
        content.append(&context_row);

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        let cancel_btn = gtk::Button::with_label(&t("Cancel"));
        let assign_btn = gtk::Button::with_label(&t("Assign"));
        assign_btn.add_css_class("suggested-action");
        buttons.append(&cancel_btn);
        buttons.append(&assign_btn);
        content.append(&buttons);
        dialog.set_content(Some(&content));
        dialog.set_default_widget(Some(&assign_btn));

        let dialog_cancel = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dialog_cancel.close();
        });

        let state = Rc::clone(self);
        let dialog_assign = dialog.clone();
        assign_btn.connect_clicked(move |_| {
            let projects = data::split_tag_input(&project_entry.text(), '+');
            let contexts = data::split_tag_input(&context_entry.text(), '@');
            if projects.is_empty() && contexts.is_empty() {
                dialog_assign.close();
                return;
            }
            dialog_assign.close();
            state.submit_bulk_action(
                data::PendingOp::Assign {
                    keys: keys.clone(),
                    projects,
                    contexts,
                },
                keys.len(),
                &t("{} items updated"),
            );
        });

        dialog.present();
    }

    fn show_details_dialog(self: &Rc<Self>, todo: &TodoItem) {
        let Some(parent) = self.window.upgrade() else {
            self.show_error(&t("No window available"));
            return;
        };

        let dialog = adw::Window::builder()
            .title(t("Edit task"))
            .transient_for(&parent)
            .modal(true)
            .default_width(420)
            .build();
        dialog.set_destroy_with_parent(true);

        let key_controller = gtk::EventControllerKey::new();
        let dialog_clone = dialog.clone();
        key_controller.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gdk::Key::Escape {
                dialog_clone.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        dialog.add_controller(key_controller);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(16);
        content.set_margin_bottom(16);
        content.set_margin_start(20);
        content.set_margin_end(20);

        let title_entry = gtk::Entry::builder().text(&todo.title).hexpand(true).build();
        title_entry.set_activates_default(true);
        if self.title_autocomplete_enabled() {
            let provider_state = Rc::clone(self);
            let title_provider: Rc<dyn Fn() -> Vec<String>> =
                Rc::new(move || provider_state.collect_existing_titles());
            attach_title_autocomplete(&title_entry, title_provider);
        }
        let title_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        title_row.append(&gtk::Label::builder().label(t("Title")).xalign(0.0).build());
        title_row.append(&title_entry);
        content.append(&title_row);

        // Collect existing tags for suggestions
        let (known_projects, known_contexts) = self.collect_existing_tags();

        let projects_display = if !todo.projects.is_empty() {
            todo.projects.iter()
                .map(|p| format!("+{}", p))
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            String::new()
        };
        let (project_entry, project_input_box) = create_suggestion_entry(&projects_display, &known_projects, "+");
        let project_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        project_row.append(&gtk::Label::builder().label(t("Project (+)")).xalign(0.0).build());
        project_row.append(&project_input_box);
        content.append(&project_row);

        let contexts_display = if !todo.contexts.is_empty() {
            todo.contexts.iter()
                .map(|c| format!("@{}", c))
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            String::new()
        };
        let (context_entry, context_input_box) = create_suggestion_entry(&contexts_display, &known_contexts, "@");
        let context_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        context_row.append(&gtk::Label::builder().label(t("Location (@)")).xalign(0.0).build());
        context_row.append(&context_input_box);
        content.append(&context_row);

        let note_buffer = gtk::TextBuffer::builder()
            .text(todo.note.as_deref().unwrap_or(""))
            .build();
        let note_view = gtk::TextView::builder()
            .buffer(&note_buffer)
            .wrap_mode(gtk::WrapMode::WordChar)
            .hexpand(true)
            .accepts_tab(false)
            .build();
        note_view.set_top_margin(4);
        note_view.set_bottom_margin(4);

        // The note takes all extra height when the dialog is enlarged, so the
        // buttons stay at the bottom instead of floating mid-window (issue #13).
        let note_scrolled = gtk::ScrolledWindow::builder()
            .child(&note_view)
            .min_content_height(96)
            .hexpand(true)
            .vexpand(true)
            .build();

        let note_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        note_row.set_vexpand(true);
        note_row.append(&gtk::Label::builder().label(t("Note")).xalign(0.0).build());
        note_row.append(&note_scrolled);
        content.append(&note_row);

        let due_entry = gtk::Entry::new();
        due_entry.set_placeholder_text(Some("YYYY-MM-DDTHH:MM"));
        if let Some(due) = todo.due {
            let due_string = due.format("%Y-%m-%dT%H:%M").to_string();
            due_entry.set_text(&due_string);
        }
        let due_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        due_row.append(&gtk::Label::builder().label(t("Due date")).xalign(0.0).build());
        let due_inputs = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        due_entry.set_activates_default(true);
        due_entry.set_hexpand(true);
        due_inputs.append(&due_entry);
        let due_today_btn = gtk::Button::with_label(&t("Today"));
        due_today_btn.add_css_class("flat");
        due_inputs.append(&due_today_btn);
        due_row.append(&due_inputs);
        content.append(&due_row);

        let recurrence_values = ["", "daily", "weekly", "monthly"];
        let recurrence_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        recurrence_row.append(&gtk::Label::builder().label(t("Recurrence")).xalign(0.0).build());
        let recurrence_list = gtk::StringList::new(&[]);
        recurrence_list.append(&t("None"));
        recurrence_list.append(&t("Daily"));
        recurrence_list.append(&t("Weekly"));
        recurrence_list.append(&t("Monthly"));
        let recurrence_dropdown = gtk::DropDown::new(Some(recurrence_list.clone()), None::<&gtk::Expression>);
        let rec_index = todo
            .recurrence
            .as_deref()
            .and_then(|r| recurrence_values.iter().position(|v| v == &r))
            .unwrap_or(0) as u32;
        recurrence_dropdown.set_selected(rec_index);
        recurrence_row.append(&recurrence_dropdown);
        content.append(&recurrence_row);

        let done_check = gtk::CheckButton::with_label(&t("Done"));
        done_check.set_active(todo.done);
        content.append(&done_check);

        let comment_entry = gtk::Entry::new();
        comment_entry.set_activates_default(true);
        let comment_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        comment_row.append(&gtk::Label::builder().label(t("Comment")).xalign(0.0).build());
        comment_row.append(&comment_entry);
        comment_row.set_visible(false);
        content.append(&comment_row);

        // Auf schmalen Bildschirmen (Issue #12) umbrechen die Buttons,
        // statt aus dem Dialog zu laufen.
        let buttons = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(4)
            .min_children_per_line(1)
            .row_spacing(6)
            .column_spacing(6)
            .halign(gtk::Align::End)
            .build();
        let cancel_btn = gtk::Button::with_label(&t("Cancel"));
        let delete_btn = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text(t("Delete"))
            .css_classes(["destructive-action"])
            .build();
        set_a11y_label(&delete_btn, &t("Delete"));
        let close_with_comment_btn = gtk::Button::with_label(&t("Close with comment"));
        let save_btn = gtk::Button::with_label(&t("Save"));
        save_btn.add_css_class("suggested-action");
        dialog.set_default_widget(Some(&save_btn));
        buttons.append(&cancel_btn);
        buttons.append(&delete_btn);
        buttons.append(&close_with_comment_btn);
        buttons.append(&save_btn);
        content.append(&buttons);
        dialog.set_content(Some(&content));

        // Let Ctrl+Enter inside the multiline note field trigger saving.
        let save_btn_for_note = save_btn.clone();
        let note_key = gtk::EventControllerKey::new();
        note_key.connect_key_pressed(move |_, key, _, state| {
            let is_enter = key == gdk::Key::Return || key == gdk::Key::KP_Enter;
            if is_enter && state.contains(gdk::ModifierType::CONTROL_MASK) {
                save_btn_for_note.emit_clicked();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        note_view.add_controller(note_key);

        let dialog_cancel = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dialog_cancel.close();
        });

        let dialog_delete = dialog.clone();
        let state_delete = self.clone();
        let todo_delete = todo.clone();
        delete_btn.connect_clicked(move |_| {
            let perform_delete = Rc::new({
                let state_delete = state_delete.clone();
                let todo_delete = todo_delete.clone();
                let dialog_delete = dialog_delete.clone();
                move || {
                    state_delete.submit(data::PendingOp::Delete {
                        keys: vec![todo_delete.key.clone()],
                    });
                    dialog_delete.close();
                }
            });

            if state_delete.skip_delete_confirmation() {
                perform_delete();
                return;
            }

            if let Some(parent) = state_delete.window.upgrade() {
                let confirm_dialog = AlertDialog::builder()
                    .modal(true)
                    .build();
                confirm_dialog.set_message(&t("Delete this entry?"));
                confirm_dialog.set_detail(&t("This cannot be undone."));
                confirm_dialog.set_buttons(&[&t("Delete"), &t("Cancel")]);
                confirm_dialog.set_default_button(1);
                confirm_dialog.set_cancel_button(1);

                let perform_delete_cb = perform_delete.clone();
                confirm_dialog.choose(
                    Some(&parent),
                    Option::<&gio::Cancellable>::None,
                    move |result| {
                        if let Ok(0) = result {
                            perform_delete_cb();
                        }
                    },
                );
            } else {
                perform_delete();
            }
        });

        let due_entry_for_button = due_entry.clone();
        due_today_btn.connect_clicked(move |_| {
            let now = Local::now().naive_local().format("%Y-%m-%dT%H:%M").to_string();
            due_entry_for_button.set_text(&now);
        });

        let dialog_save = dialog.clone();
        let state_for_save = Rc::clone(self);
        let base_item = todo.clone();
        let title_entry_save = title_entry.clone();
        let project_entry_save = project_entry.clone();
        let context_entry_save = context_entry.clone();
        let note_buffer_save = note_buffer.clone();
        let due_entry_save = due_entry.clone();
        let done_check_save = done_check.clone();
        let comment_entry_save = comment_entry.clone();
        let comment_row_save = comment_row.clone();
        let recurrence_dropdown_save = recurrence_dropdown.clone();
        save_btn.connect_clicked(move |_| {
            let mut title_text = title_entry_save.text().trim().to_string();
            if title_text.is_empty() {
                state_for_save.show_error(&t("Title must not be empty"));
                return;
            }

            if comment_row_save.is_visible() {
                let comment = comment_entry_save.text().trim().to_string();
                if !comment.is_empty() {
                    title_text = format!("{} ({})", title_text, comment);
                }
            }

            // `+`/`@` delimit the names here, so multi-word tags survive editing.
            let projects_value = data::split_tag_input(&project_entry_save.text(), '+');
            let contexts_value = data::split_tag_input(&context_entry_save.text(), '@');

            let due_text = due_entry_save.text().trim().to_string();
            let due_value = if due_text.is_empty() {
                None
            } else {
                match NaiveDateTime::parse_from_str(&due_text, "%Y-%m-%dT%H:%M") {
                    Ok(dt) => Some(dt),
                    Err(_) => match NaiveDate::parse_from_str(&due_text, "%Y-%m-%d") {
                        Ok(date) => Some(NaiveDateTime::new(date, DEFAULT_DUE_TIME)),
                        Err(_) => {
                            state_for_save.show_error(&t("Invalid date. Expected YYYY-MM-DD"));
                            return;
                        }
                    },
                }
            };

            let rec_values = ["", "daily", "weekly", "monthly"];
            let rec_idx = recurrence_dropdown_save.selected() as usize;
            let recurrence_value = rec_values
                .get(rec_idx)
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());

            let (note_start, note_end) = note_buffer_save.bounds();
            let note_text = note_buffer_save.text(&note_start, &note_end, false).to_string();
            let note_value = if note_text.trim().is_empty() {
                None
            } else {
                Some(note_text.trim().to_string())
            };

            let mut updated = base_item.clone();
            updated.title = title_text;
            updated.projects = projects_value;
            updated.contexts = contexts_value;
            updated.reference = base_item.reference.clone();
            updated.due = due_value;
            updated.recurrence = recurrence_value;
            updated.note = note_value;
            updated.done = done_check_save.is_active();

            state_for_save.save_item(&updated);
            dialog_save.close();
        });

        let comment_entry_close = comment_entry.clone();
        let comment_row_close = comment_row.clone();
        let close_btn_ref = close_with_comment_btn.clone();
        let done_check_close = done_check.clone();
        close_with_comment_btn.connect_clicked(move |_| {
            comment_row_close.set_visible(true);
            comment_entry_close.grab_focus();
            done_check_close.set_active(true);
            close_btn_ref.set_sensitive(false);
        });

        dialog.present();
    }

    fn sort_items(&self, items: &mut [TodoItem]) {
        sort_items(items, *self.sort_mode.borrow());
    }

    /// Kanonische Schreibweise je Projekt/Ort (kleingeschriebener Schlüssel →
    /// meistgenutzte Variante), damit Groß-/Kleinschreibungs-Varianten in einer
    /// Gruppe landen.
    fn canonical_tag_maps(&self) -> (HashMap<String, String>, HashMap<String, String>) {
        let items = self.cached_items.borrow();
        let projects = canonical_casing_map(
            items
                .iter()
                .flat_map(|item| item.projects.iter().map(|s| s.as_str())),
        );
        let contexts = canonical_casing_map(
            items
                .iter()
                .flat_map(|item| item.contexts.iter().map(|s| s.as_str())),
        );
        (projects, contexts)
    }

    fn group_label(
        &self,
        mode: SortMode,
        item: &TodoItem,
        canon_projects: &HashMap<String, String>,
        canon_contexts: &HashMap<String, String>,
    ) -> Option<String> {
        match mode {
            SortMode::Topic => Some(t("Topic: {}").replace(
                "{}",
                &item
                    .projects
                    .first()
                    .filter(|s| !s.is_empty())
                    .map(|s| canonicalize_token(canon_projects, s))
                    .unwrap_or_else(|| t("No project")),
            )),
            SortMode::Location => Some(t("Location: {}").replace(
                "{}",
                &item
                    .contexts
                    .first()
                    .filter(|s| !s.is_empty())
                    .map(|s| canonicalize_token(canon_contexts, s))
                    .unwrap_or_else(|| t("No location")),
            )),
            SortMode::Date => None,
        }
    }

    fn show_info(&self, message: &str) {
        let toast = adw::Toast::builder().title(message).build();
        self.overlay.add_toast(toast);
    }

    fn show_error(&self, message: &str) {
        let display_msg = if message.chars().count() > 120 {
            let truncated: String = message.chars().take(120).collect();
            format!("{}…", truncated)
        } else {
            message.to_string()
        };
        let toast = adw::Toast::builder()
            .title(&display_msg)
            .priority(adw::ToastPriority::High)
            .timeout(10)
            .build();
        self.overlay.add_toast(toast);
    }

    /// Handle a ConflictError by showing an alert dialog.
    /// Returns true if the user chose "Overwrite", false otherwise.
    fn handle_conflict(self: &Rc<Self>, err: &anyhow::Error) -> bool {
        if err.downcast_ref::<data::ConflictError>().is_none() {
            return false;
        }
        let Some(parent) = self.window.upgrade() else {
            return false;
        };
        let dialog = AlertDialog::builder().modal(true).build();
        dialog.set_message(&t("File was changed externally"));
        dialog.set_buttons(&[&tc("conflict dialog button", "Reload"), &t("Overwrite"), &t("Cancel")]);
        dialog.set_default_button(0);
        dialog.set_cancel_button(2);

        // The rejected write travels with the error, so "Overwrite" can push
        // exactly the change the user just made.
        let pending = err
            .downcast_ref::<data::ConflictError>()
            .and_then(|c| c.pending_content.clone());

        let state = Rc::clone(self);
        dialog.choose(
            Some(&parent),
            Option::<&gio::Cancellable>::None,
            move |result| {
                match result {
                    Ok(0) => {
                        // Reload — drop our change and take the remote state.
                        state.refresh_in_background();
                    }
                    Ok(1) => {
                        // Overwrite — force our own change through. Restoring
                        // the pre-mutation snapshot here (what this used to do)
                        // uploaded the *old* file and undid everything the
                        // other writer had stored in the meantime.
                        match pending.clone() {
                            Some(content) => state.enqueue(WriteJob::Overwrite(content)),
                            None => {
                                state.show_error(&t("The change could not be applied. Please try again after reloading."));
                                state.refresh_in_background();
                            }
                        }
                    }
                    _ => {}
                }
            },
        );
        true
    }

    /// Show a toast with an "Undo" button after a destructive action.
    fn show_undo_toast(self: &Rc<Self>, message: &str) {
        let toast = adw::Toast::builder()
            .title(message)
            .button_label(t("Undo"))
            .timeout(8)
            .build();
        let state = Rc::clone(self);
        toast.connect_button_clicked(move |_| {
            // Hinter den noch offenen Schreibaufträgen einreihen: rückgängig
            // gemacht wird erst, was auch gespeichert wurde.
            state.enqueue(WriteJob::Undo);
        });
        self.overlay.add_toast(toast);
    }

    /// Schedule periodic reminder checks (every 5 minutes).
    fn schedule_reminder_check(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(300, move || {
            let Some(state) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            state.check_reminders();
            glib::ControlFlow::Continue
        });
    }

    fn check_reminders(&self) {
        let prefs = self.preferences.borrow();
        if !prefs.enable_reminders {
            return;
        }
        let remind_minutes = prefs.remind_before_minutes;
        drop(prefs);

        let items = self.cached_items.borrow();
        let due_items = data::get_due_reminders(&items, remind_minutes);

        let Some(window) = self.window.upgrade() else {
            return;
        };
        let Some(app) = window.application() else {
            return;
        };

        let mut notified = self.notified_items.borrow_mut();
        for item in due_items {
            let key = item.key.marker.clone().unwrap_or_else(|| format!("line:{}", item.key.line_index));
            if notified.contains(&key) {
                continue;
            }
            notified.insert(key.clone());

            let notification = gio::Notification::new(&t("Task due"));
            notification.set_body(Some(&item.title));
            app.send_notification(Some(&key), &notification);
        }
    }

    fn install_monitor(self: &Rc<Self>) -> Result<()> {
        let file = gio::File::for_path(data::todo_path());
        let monitor = file.monitor_file(gio::FileMonitorFlags::NONE, Option::<&gio::Cancellable>::None)?;
        monitor.connect_changed(clone!(#[weak(rename_to = state)] self, move |_, _, _, event| {
            use gio::FileMonitorEvent as Event;
            let should_reload = matches!(
                event,
                Event::Changed
                    | Event::ChangesDoneHint
                    | Event::Created
                    | Event::Deleted
                    | Event::Moved
                    | Event::Renamed
                    | Event::AttributeChanged
            );

            if !should_reload {
                return;
            }

            // Über den Fingerprint gehen: so lösen die eigenen Schreibvorgänge
            // weder einen weiteren Reload noch den Hinweis unten aus.
            glib::spawn_future_local(clone!(#[weak] state, async move {
                match state.check_for_updates().await {
                    Ok(true) => {
                        if matches!(event, Event::ChangesDoneHint | Event::Changed | Event::Created) {
                            state.show_info(&t("Changes from file applied"));
                        }
                    }
                    Ok(false) => {}
                    Err(err) => {
                        state.show_error(&t("Update failed: {}").replace("{}", &err.to_string()));
                    }
                }
            }));
        }));
        *self.monitor.borrow_mut() = Some(monitor);
        Ok(())
    }
}

/// Ist die Aufgabe am Stichtag fällig? Fällig wird kalendertagweise
/// beurteilt, nicht nach Uhrzeit: was heute später fällig ist, zählt bereits
/// als fällig. Das "Irgendwann"-Sentinel-Jahr 9999 zählt nie als fällig.
fn due_range_label(range: DueRange) -> String {
    match range {
        DueRange::Any => t("Any time"),
        DueRange::Overdue => t("Overdue"),
        DueRange::Today => t("Due by today"),
        DueRange::Tomorrow => t("Due by tomorrow"),
        DueRange::Week => t("Due within 7 days"),
        DueRange::Month => t("Due within 30 days"),
        DueRange::Undated => t("No date"),
    }
}

/// Inhalt des Filter-Popovers: Fälligkeitszeitraum, Projekte, Orte (als
/// Häkchenlisten, mehrere wählbar). Jede
/// Änderung wirkt sofort auf die Liste und wird in den Einstellungen gemerkt.
fn build_filter_panel(state: &Rc<AppState>) -> gtk::Widget {
    let filter = state.filter();
    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .width_request(300)
        .build();

    let heading = |text: String| {
        let label = gtk::Label::builder().label(text).xalign(0.0).build();
        label.add_css_class("heading");
        label
    };

    // Früh angelegt, damit jede Änderung ihn (de)aktivieren kann.
    let reset = gtk::Button::with_label(&t("Reset filters"));
    reset.set_sensitive(filter.is_active());

    // Fälligkeit
    let due_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    due_box.append(&heading(t("Due date")));
    // Zeitraum als Untermenü im selben Popover (wie die Untermenüs von
    // GtkPopoverMenu) statt als DropDown: dessen verschachteltes Popup nimmt
    // dem Filter-Popover beim Schließen den Grab, danach schließt es sich bei
    // Klicks daneben nicht mehr.
    let undated = gtk::CheckButton::with_label(&t("Also tasks without a date"));
    undated.set_active(filter.include_undated);
    undated.set_sensitive(filter.due.is_bounded());

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::SlideLeftRight)
        .vhomogeneous(false)
        .interpolate_size(true)
        .build();

    let range_label = gtk::Label::builder()
        .label(due_range_label(filter.due))
        .xalign(0.0)
        .hexpand(true)
        .build();
    let range_row_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    range_row_content.append(&range_label);
    range_row_content.append(&gtk::Image::from_icon_name("go-next-symbolic"));
    let range_row = gtk::Button::builder().child(&range_row_content).build();
    set_a11y_label(&range_row, &t("Due date"));
    range_row.connect_clicked(clone!(#[weak] stack, move |_| {
        stack.set_visible_child_name("due");
    }));
    due_box.append(&range_row);
    due_box.append(&undated);

    // Unterseite: Zurück-Zeile und die Zeiträume, der gewählte mit Haken.
    let due_page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    let back_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    back_content.append(&gtk::Image::from_icon_name("go-previous-symbolic"));
    back_content.append(&heading(t("Due date")));
    let back = gtk::Button::builder().child(&back_content).build();
    back.add_css_class("flat");
    back.connect_clicked(clone!(#[weak] stack, move |_| {
        stack.set_visible_child_name("main");
    }));
    due_page.append(&back);
    due_page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let options = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let mut buttons = Vec::new();
    let mut checks = Vec::new();
    for range in DueRange::ALL.iter().copied() {
        let check = gtk::Image::from_icon_name("object-select-symbolic");
        check.set_opacity(if range == filter.due { 1.0 } else { 0.0 });
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        content.append(&gtk::Label::builder()
            .label(due_range_label(range))
            .xalign(0.0)
            .hexpand(true)
            .build());
        content.append(&check);
        let option = gtk::Button::builder().child(&content).build();
        option.add_css_class("flat");
        options.append(&option);
        buttons.push((range, option));
        checks.push((range, check));
    }
    let checks = Rc::new(checks);
    for (range, option) in buttons {
        option.connect_clicked(clone!(#[weak] state, #[weak] stack, #[weak] undated, #[weak] reset,
            #[weak] range_label, #[strong] checks, move |_| {
            for (r, check) in checks.iter() {
                check.set_opacity(if *r == range { 1.0 } else { 0.0 });
            }
            range_label.set_label(&due_range_label(range));
            undated.set_sensitive(range.is_bounded());
            state.update_filter(|f| f.due = range);
            reset.set_sensitive(state.filter().is_active());
            stack.set_visible_child_name("main");
        }));
    }
    due_page.append(&options);
    undated.connect_toggled(clone!(#[weak] state, move |check| {
        let on = check.is_active();
        state.update_filter(|f| f.include_undated = on);
    }));
    panel.append(&due_box);

    // Projekte und Orte
    let (projects, contexts) = state.filter_tag_choices();
    let tag_section = |title: String, sigil: &str, none_label: String, names: Vec<String>,
                       selected: &[String], is_project: bool| {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 6);
        section.append(&heading(title));
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let is_selected = |name: &str| {
            let lower = name.to_lowercase();
            selected.iter().any(|s| s.to_lowercase() == lower)
        };
        let entries = names
            .into_iter()
            .map(|name| (format!("{sigil}{name}"), name))
            .chain(std::iter::once((none_label, NO_TAG.to_string())));
        for (label, name) in entries {
            let toggle = gtk::CheckButton::with_label(&label);
            toggle.set_active(is_selected(&name));
            toggle.connect_toggled(clone!(#[weak] state, #[weak] reset, move |_| {
                state.update_filter(|f| {
                    if is_project {
                        f.toggle_project(&name);
                    } else {
                        f.toggle_context(&name);
                    }
                });
                reset.set_sensitive(state.filter().is_active());
            }));
            list.append(&toggle);
        }
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(200)
            .propagate_natural_height(true)
            .child(&list)
            .build();
        section.append(&scroller);
        section
    };
    panel.append(&tag_section(t("Projects"), "+", t("No project"), projects, &filter.projects, true));
    panel.append(&tag_section(t("Places"), "@", t("No location"), contexts, &filter.contexts, false));

    reset.connect_clicked(clone!(#[weak] state, move |button| {
        state.set_filter(TodoFilter::default());
        // Panel neu aufbauen, damit alle Schalter den leeren Filter zeigen.
        if let Some(popover) = button.ancestor(gtk::Popover::static_type())
            .and_then(|w| w.downcast::<gtk::Popover>().ok())
        {
            popover.set_child(Some(&build_filter_panel(&state)));
        }
    }));
    panel.append(&reset);

    stack.add_named(&panel, Some("main"));
    stack.add_named(&due_page, Some("due"));
    stack.upcast()
}

fn is_due_by(todo: &TodoItem, today: NaiveDate) -> bool {
    todo.due
        .map(|d| d.date() <= today && d.date().year() != 9999)
        .unwrap_or(false)
}

/// Teilt die noch nicht für heute geplanten Aufgaben in die beiden
/// Picker-Abschnitte "Vorschläge (fällig)" und "Weitere offene Aufgaben",
/// fällige zuerst. Der Listenfilter wirkt hier nur auf den Picker — was
/// bewusst für heute geplant wurde, bleibt in "Mein Tag" immer sichtbar.
fn split_picker_candidates(
    rest: Vec<TodoItem>,
    today: NaiveDate,
    filter: &TodoFilter,
) -> (Vec<TodoItem>, Vec<TodoItem>) {
    let mut candidates: Vec<TodoItem> = rest
        .into_iter()
        .filter(|todo| !todo.done && filter.matches(todo, today))
        .collect();
    candidates.sort_by_key(|todo| todo.due.unwrap_or(NaiveDateTime::MAX));
    candidates
        .into_iter()
        .partition(|todo| is_due_by(todo, today))
}

fn normalize_token(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_start_matches(['+', '@']);
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_lowercase())
    }
}

fn format_metadata(item: &TodoItem) -> String {
    let mut parts = Vec::new();

    let first_project_norm = item.projects.first().and_then(|p| normalize_token(p));

    if !item.projects.is_empty() {
        let projects_str = item.projects.iter()
            .map(|p| format!("+{}", p))
            .collect::<Vec<_>>()
            .join(" ");
        parts.push(projects_str);
    }
    if !item.contexts.is_empty() {
        let contexts_str = item.contexts.iter()
            .filter_map(|c| {
                let ctx_norm = normalize_token(c);
                if ctx_norm != first_project_norm {
                    Some(format!("@{}", c))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        if !contexts_str.is_empty() {
            parts.push(contexts_str);
        }
    }
    if let Some(due) = item.due {
        if due.date().year() == 9999 {
            parts.push(t("Sometimes"));
        } else {
            parts.push(t("Due: {}").replace("{}", &due.format("%Y-%m-%d %H:%M").to_string()));
        }
    }
    if let Some(rule) = &item.recurrence {
        let label = match rule.as_str() {
            "daily" => t("Daily"),
            "weekly" => t("Weekly"),
            "monthly" => t("Monthly"),
            _ => rule.clone(),
        };
        parts.push(format!("↻ {}", label));
    }
    if let Some(reference) = &item.reference {
        parts.push(format!("↗ {}", reference));
    }

    parts.join(" • ")
}

/// Interpret a `project`/`context` value returned by the model. The prompt asks
/// for one tag per field, so a plain value stays a single name even when it
/// contains spaces (`Big Project`); only an explicitly prefixed list
/// (`+haushalt +urlaub`) is split into several tags.
fn split_ai_tags(raw: &str, prefix: char) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let prefixed = trimmed.starts_with(prefix)
        || trimmed
            .split_whitespace()
            .any(|token| token.starts_with(prefix));
    if prefixed {
        data::split_tag_input(trimmed, prefix)
    } else {
        vec![trimmed.to_string()]
    }
}

/// Snap a model-supplied tag onto an existing one when it only differs in case.
fn match_known_tag(name: &str, known: &[String]) -> String {
    known
        .iter()
        .find(|existing| existing.to_lowercase() == name.to_lowercase())
        .cloned()
        .unwrap_or_else(|| name.to_string())
}

fn build_todo_from_ai(parsed: &AiParseResult, original_text: &str) -> data::TodoItem {
    let title = parsed
        .title
        .as_deref()
        .unwrap_or(original_text)
        .trim()
        .to_string();

    let contexts: Vec<String> = parsed
        .context
        .as_deref()
        .map(|c| split_ai_tags(c, '@'))
        .unwrap_or_default();

    let projects: Vec<String> = parsed
        .project
        .as_deref()
        .map(|p| split_ai_tags(p, '+'))
        .unwrap_or_default();

    // Always use original input text as note
    let note = Some(original_text.trim().to_string());

    let due = parsed
        .due
        .as_deref()
        .and_then(|d| NaiveDateTime::parse_from_str(d, "%Y-%m-%dT%H:%M").ok())
        .or_else(|| {
            parsed
                .due
                .as_deref()
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                .map(|date| NaiveDateTime::new(date, DEFAULT_DUE_TIME))
        })
        .unwrap_or_else(|| {
            let today = Local::now().date_naive();
            NaiveDateTime::new(today, DEFAULT_DUE_TIME)
        });

    data::TodoItem {
        key: data::TodoKey {
            line_index: 0,
            marker: None,
        },
        title,
        projects,
        contexts,
        due: Some(due),
        myday: None,
        reference: None,
        recurrence: None,
        note,
        done: false,
    }
}

async fn request_ai_parse(
    text: String,
    known_projects: Vec<String>,
    known_contexts: Vec<String>,
    ollama_url: String,
    ollama_model: String,
) -> Result<AiParseResult> {
    let chat_url = format!("{}/api/chat", ollama_url.trim_end_matches('/'));

    let projects_str = if known_projects.is_empty() {
        String::new()
    } else {
        format!("Existing projects: {}", known_projects.join(", "))
    };
    let contexts_str = if known_contexts.is_empty() {
        String::new()
    } else {
        format!("Existing contexts: {}", known_contexts.join(", "))
    };
    let history_hint = if !projects_str.is_empty() || !contexts_str.is_empty() {
        let parts: Vec<&str> = [projects_str.as_str(), contexts_str.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        // Hard constraint: the model must reuse an existing tag whenever one
        // fits, so that similar inputs land on the same project/context instead
        // of inventing a fresh tag each time.
        format!(
            "{}. \
             You MUST choose `project` and `context` from these existing lists whenever a listed tag reasonably fits the task. \
             Reuse the closest existing tag instead of inventing a synonym. \
             Only create a new tag if NONE of the existing ones fits; in that case prefer leaving `project` empty over guessing. ",
            parts.join(". ")
        )
    } else {
        String::new()
    };

    let system_prompt = format!(
        concat!(
            "Parse todo items. Today: {}. {}",
            "TASK: Extract structured data from input. ",
            "OUTPUT JSON: {{\"title\": str, \"due\": \"YYYY-MM-DD\"|null, \"context\": str, \"project\": str}} ",
            "FIELDS: `context` = the place or mode where the task happens (e.g. einkaufen, Garten, Computer, Keller). ",
            "`project` = the topic/area the task belongs to, describing what it is about (e.g. Haushalt, Urlaub, Familie). ",
            "`context` and `project` MUST NOT be the same word. ",
            "RULES: JSON only. Keep input language. German tags. Assign context and (when a fitting tag exists) project. Match existing tag case exactly. ",
            "Example: 'Kaufe morgen Milch' -> {{\"title\": \"Kaufe Milch\", \"due\": \"2026-01-18\", \"context\": \"einkaufen\", \"project\": \"haushalt\"}}"
        ),
        Local::now().format("%A, %Y-%m-%d"),
        history_hint
    );

    let payload = serde_json::json!({
        "model": ollama_model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": text},
        ],
        "stream": false,
        "format": "json",
        // Deterministic sampling so identical inputs yield identical tags and the
        // model sticks to the instructed existing-tag reuse.
        "options": {
            "temperature": 0.0,
            "top_p": 0.9,
        },
    });

    let client = reqwest::Client::new();
    let resp = client.post(chat_url).json(&payload).send().await?;
    let status = resp.status();
    let body = resp.text().await?;

    if !status.is_success() {
        return Err(anyhow!(format!("HTTP {}: {}", status, body)));
    }

    let envelope: AiChatResponse = serde_json::from_str(&body)
        .map_err(|e| anyhow!(format!("Parse response failed: {e}; body: {body}")))?;

    let mut raw = envelope.message.content.trim().to_string();
    if raw.starts_with("```") {
        raw = raw
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim()
            .to_string();
    }

    let mut parsed: AiParseResult = serde_json::from_str(&raw)
        .map_err(|e| anyhow!(format!("Parse JSON failed: {e}; raw: {raw}")))?;

    // Normalize tag case to match existing tags
    if let Some(ref project) = parsed.project {
        let normalized_projects: Vec<String> = split_ai_tags(project, '+')
            .into_iter()
            .map(|p| match_known_tag(&p, &known_projects))
            .collect();
        parsed.project = Some(normalized_projects.iter()
            .map(|p| format!("+{}", p))
            .collect::<Vec<_>>()
            .join(" "));
    }
    if let Some(ref context) = parsed.context {
        let normalized_contexts: Vec<String> = split_ai_tags(context, '@')
            .into_iter()
            .map(|c| match_known_tag(&c, &known_contexts))
            .collect();
        parsed.context = Some(normalized_contexts.iter()
            .map(|c| format!("@{}", c))
            .collect::<Vec<_>>()
            .join(" "));
    }

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reinschrift_core::parser::parse_line;

    fn items(lines: &[&str]) -> Vec<TodoItem> {
        lines
            .iter()
            .enumerate()
            .map(|(i, line)| parse_line(line, i).expect("parsable todo line"))
            .collect()
    }

    /// What the old "show only due" switch meant.
    fn due_by_today() -> TodoFilter {
        TodoFilter { due: DueRange::Today, include_undated: true, ..Default::default() }
    }

    #[test]
    fn week_filter_narrows_picker() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 10).unwrap();
        let filter = TodoFilter { due: DueRange::Week, ..Default::default() };
        let (suggestions, other_open) = split_picker_candidates(
            items(&[
                "- [ ] Fenster putzen due:2026-09-01T12:00 ^aaa1",
                "- [ ] Nächste Woche due:2026-09-16T12:00 ^aaa2",
                "- [ ] Viel später due:2026-10-20T12:00 ^aaa3",
                "- [ ] Ohne Datum ^aaa4",
            ]),
            today,
            &filter,
        );

        assert_eq!(titles(&suggestions), ["Fenster putzen"]);
        assert_eq!(titles(&other_open), ["Nächste Woche"]);
    }

    fn titles(items: &[TodoItem]) -> Vec<&str> {
        items.iter().map(|todo| todo.title.as_str()).collect()
    }

    #[test]
    fn picker_shows_everything_open_without_due_filter() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 10).unwrap();
        let (suggestions, other_open) = split_picker_candidates(
            items(&[
                "- [ ] Fenster putzen due:2026-09-01T12:00 ^aaa1",
                "- [ ] Pool füllen due:2027-05-01T12:00 ^aaa2",
                "- [ ] Irgendwann mal due:9999-12-31T12:00 ^aaa3",
                "- [ ] Ohne Datum ^aaa4",
                "- [x] Schon erledigt due:2026-09-01T12:00 ^aaa5",
            ]),
            today,
            &TodoFilter::default(),
        );

        assert_eq!(titles(&suggestions), ["Fenster putzen"]);
        assert_eq!(titles(&other_open), ["Pool füllen", "Irgendwann mal", "Ohne Datum"]);
    }

    #[test]
    fn due_filter_hides_future_and_someday_from_picker() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 10).unwrap();
        let (suggestions, other_open) = split_picker_candidates(
            items(&[
                "- [ ] Fenster putzen due:2026-09-01T12:00 ^aaa1",
                "- [ ] Pool füllen due:2027-05-01T12:00 ^aaa2",
                "- [ ] Irgendwann mal due:9999-12-31T12:00 ^aaa3",
                "- [ ] Ohne Datum ^aaa4",
            ]),
            today,
            &due_by_today(),
        );

        assert_eq!(titles(&suggestions), ["Fenster putzen"]);
        assert_eq!(titles(&other_open), ["Ohne Datum"]);
    }

    #[test]
    fn tasks_due_later_today_stay_suggestions_under_due_filter() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 10).unwrap();
        let (suggestions, other_open) = split_picker_candidates(
            items(&["- [ ] Heute Abend due:2026-09-10T23:00 ^aaa1"]),
            today,
            &due_by_today(),
        );

        assert_eq!(titles(&suggestions), ["Heute Abend"]);
        assert!(other_open.is_empty());
    }
}
