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

use std::collections::{HashMap, HashSet};
use reinschrift_core::embeddings;
use reinschrift_core::util::{canonical_casing_map, canonicalize_token};
use reinschrift_core::{data, TodoItem, SortMode, sort_items, t, tc, Preferences, load_preferences, write_preferences};

enum VoiceMsg {
    Error(String),
    Transcription(String),
    Transcribing,
    Finished,
}

#[derive(Clone)]
enum ListEntry {
    Header(String),
    Item(TodoItem),
    /// Kandidat im "Mein Tag"-Planungs-Picker (noch nicht für heute geplant).
    PickerItem(TodoItem),
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
    glib::timeout_add_seconds_local(interval, clone!(#[weak] state, #[upgrade_or] glib::ControlFlow::Break, move || {
        let next_interval = match state.check_for_updates() {
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
        glib::ControlFlow::Break
    }));
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

                // Check if already present
                let already_present = current
                    .split_whitespace()
                    .any(|s| s == suggestion_with_prefix);

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
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(t("Reinschrift"))
        .default_width(saved_prefs.window_width.filter(|w| *w > 0).unwrap_or(560))
        .default_height(saved_prefs.window_height.filter(|h| *h > 0).unwrap_or(780))
        .build();
    if saved_prefs.window_maximized {
        window.maximize();
    }

    let header = adw::HeaderBar::builder()
        .title_widget(&gtk::Label::builder().label(t("Reinschrift")).build())
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

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    controls.set_margin_start(12);
    controls.set_margin_end(12);
    controls.set_margin_top(6);
    controls.set_margin_bottom(6);

    let sort_label = gtk::Label::builder()
        .label(t("Sort by:"))
        .xalign(0.0)
        .build();
    controls.append(&sort_label);

    let sort_selector = gtk::DropDown::from_strings(&[&t("+ Topics"), &t("@ Locations"), &t("Date")]);
    sort_selector.set_selected(state.sort_mode().to_index());
    set_a11y_label(&sort_selector, &t("Sort by:"));
    controls.append(&sort_selector);

    let due_filter = gtk::CheckButton::with_label(&t("Show only due"));
    due_filter.set_margin_start(18);
    due_filter.set_active(state.show_due_only());
    controls.append(&due_filter);

    let myday_filter = gtk::ToggleButton::builder()
        .label(t("My Day"))
        .tooltip_text(t("My Day"))
        .build();
    myday_filter.set_margin_start(18);
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
            match data::undo() {
                Ok(Some(desc)) => {
                    if let Err(e) = state_for_keys.reload() {
                        state_for_keys.show_error(&t("Could not reload To-dos: {}").replace("{}", &e.to_string()));
                    } else {
                        state_for_keys.show_info(&t("Undone: {}").replace("{}", &desc));
                    }
                }
                Ok(None) => {
                    state_for_keys.show_info(&t("Nothing to undo"));
                }
                Err(e) => {
                    state_for_keys.show_error(&e.to_string());
                }
            }
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
        if let Err(err) = state.reload() {
            state.show_error(&t("Could not load To-dos: {}").replace("{}", &err.to_string()));
        }
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

    // Embedding-Index im Hintergrund vorwärmen, damit die ersten
    // semantischen Vorschläge nicht den vollen Index-Aufbau abwarten müssen.
    state.warm_semantic_index();

    sort_selector.connect_selected_notify(clone!(#[weak] state, move |dropdown| {
        let mode = SortMode::from_index(dropdown.selected());
        state.set_sort_mode(mode);
    }));

    due_filter.connect_toggled(clone!(#[weak] state, move |btn| {
        state.set_show_due_only(btn.is_active());
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
            let todo = match &*entry {
                ListEntry::Item(todo) => todo.clone(),
                _ => return glib::Propagation::Proceed,
            };
            
            let Some(state) = state_item_key.upgrade() else { return glib::Propagation::Proceed; };
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
                    let _ = state.toggle_item(&todo, !todo.done);
                    glib::Propagation::Stop
                }
                gdk::Key::Delete | gdk::Key::KP_Delete => {
                    state.request_delete(&todo);
                    glib::Propagation::Stop
                }
                _ if unicode == Some('h') || unicode == Some('H') => {
                    let _ = state.set_due_today(&todo);
                    glib::Propagation::Stop
                }
                _ if unicode == Some('m') || unicode == Some('M') => {
                    let _ = state.set_due_tomorrow(&todo);
                    glib::Propagation::Stop
                }
                _ if unicode == Some('w') || unicode == Some('W') => {
                    let _ = state.set_due_weekend(&todo);
                    glib::Propagation::Stop
                }
                _ if unicode == Some('l') || unicode == Some('L') => {
                    let _ = state.set_due_in_days(&todo, 7);
                    glib::Propagation::Stop
                }
                _ if unicode == Some('s') || unicode == Some('S') => {
                    let _ = state.set_due_sometimes(&todo);
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
                if let Err(err) = state.toggle_item(&todo, btn.is_active()) {
                    state.show_error(&t("Could not update entry: {}").replace("{}", &err.to_string()));
                }
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

            if let Some(state) = myday_state.upgrade()
                && let Err(err) = state.toggle_myday(&todo) {
                    state.show_error(&t("Could not update entry: {}").replace("{}", &err.to_string()));
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

            if let Some(state) = tomorrow_state.upgrade()
                && let Err(err) = state.set_due_tomorrow(&todo) {
                    state.show_error(&t("Could not set due date: {}").replace("{}", &err.to_string()));
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

            if let Some(state) = weekend_state.upgrade()
                && let Err(err) = state.set_due_weekend(&todo) {
                    state.show_error(&t("Could not set due date: {}").replace("{}", &err.to_string()));
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

            if let Some(state) = today_state.upgrade()
                && let Err(err) = state.set_due_today(&todo) {
                    state.show_error(&t("Could not set due date: {}").replace("{}", &err.to_string()));
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

            if let Some(state) = sometimes_state.upgrade()
                && let Err(err) = state.set_due_sometimes(&todo) {
                    state.show_error(&t("Could not set due date: {}").replace("{}", &err.to_string()));
                }
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

            if let Some(state) = picker_state.upgrade()
                && let Err(err) = state.toggle_myday(&todo) {
                    state.show_error(&t("Could not update entry: {}").replace("{}", &err.to_string()));
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
        let Some(stack_ref_ptr) = (unsafe { list_item.data::<glib::WeakRef<gtk::Stack>>("stack") }) else {
            return;
        };
        let Some(stack) = unsafe { stack_ref_ptr.as_ref() }.upgrade() else {
            return;
        };

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
                            btn.set_visible(!selection_mode);
                        }
                }

                if let Some(check_ref_ptr) = unsafe {
                    list_item.data::<glib::WeakRef<gtk::CheckButton>>("todo-check")
                }
                    && let Some(check_widget) = unsafe { check_ref_ptr.as_ref() }.upgrade() {
                        check_widget.set_visible(!selection_mode);
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
    let list_view = gtk::ListView::new(Some(model), Some(factory));
    list_view.set_single_click_activate(true);
    let activate_state = state_weak.clone();
    list_view.connect_activate(move |_, position| {
        if let Some(state) = activate_state.upgrade() {
            state.open_entry_at(position);
        }
    });
    list_view
}

struct AppState {
    store: gio::ListStore,
    overlay: adw::ToastOverlay,
    monitor: RefCell<Option<gio::FileMonitor>>,
    cached_items: RefCell<Vec<TodoItem>>,
    last_fingerprint: RefCell<Option<String>>,
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
    /// Auswahl per Marker — line_index verschiebt sich bei jedem Reload.
    selected_markers: RefCell<HashSet<String>>,
    selection_bar: RefCell<Option<gtk::Revealer>>,
    selection_count_label: RefCell<Option<gtk::Label>>,
    selection_toggle: RefCell<Option<gtk::ToggleButton>>,
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
            selected_markers: RefCell::new(HashSet::new()),
            selection_bar: RefCell::new(None),
            selection_count_label: RefCell::new(None),
            selection_toggle: RefCell::new(None),
            embedding_cache: RefCell::new(None),
            embedding_index_building: Cell::new(false),
            query_embed_cache: RefCell::new(HashMap::new()),
            last_fingerprint: RefCell::new(None),
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

    fn show_due_only(&self) -> bool {
        self.preferences.borrow().show_due_only
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

    fn reload(&self) -> Result<()> {
        let items = data::load_todos()?;
        *self.cached_items.borrow_mut() = items;
        if let Ok(fp) = data::get_fingerprint() {
            *self.last_fingerprint.borrow_mut() = Some(fp);
        }
        self.repopulate_store();
        Ok(())
    }

    /// Liefert `true`, wenn sich die Datenbank geändert hat und neu
    /// geladen wurde (Aufrufer wärmt dann den Embedding-Index nach).
    fn check_for_updates(&self) -> Result<bool> {
        let current_fp = data::get_fingerprint()?;
        let last_fp = self.last_fingerprint.borrow().clone();

        if Some(current_fp) != last_fp {
            self.reload()?;
            return Ok(true);
        }
        Ok(false)
    }

    fn toggle_item(self: &Rc<Self>, todo: &TodoItem, done: bool) -> Result<()> {
        let now = Local::now().naive_local();
        let is_historic = todo.due.map(|d| d < now).unwrap_or(false);
        let is_recurring = todo.recurrence.is_some();

        let result = if done && is_historic && is_recurring {
            let mut updated = todo.clone();
            let time = todo.due.map(|d| d.time()).unwrap_or(DEFAULT_DUE_TIME);
            let today = Local::now().date_naive();
            updated.due = Some(NaiveDateTime::new(today, time));
            updated.done = true;
            data::update_todo_details(&updated)
        } else {
            data::toggle_todo(&todo.key, done)
        };

        if let Err(ref err) = result {
            if self.handle_conflict(err) {
                return Ok(());
            }
            return result;
        }

        if done
            && let Some(rule) = todo.recurrence.as_deref()
                && let Some(next_due) = data::next_due_date(todo.due, rule) {
                    let mut next_item = todo.clone();
                    next_item.key = data::TodoKey { line_index: 0, marker: None };
                    next_item.done = false;
                    next_item.due = Some(next_due);
                    // Die neue Instanz plant sich nicht von selbst für "Mein Tag" —
                    // sonst taucht sie sofort wieder unerledigt neben der gerade
                    // abgehakten Aufgabe auf.
                    next_item.myday = None;
                    if let Err(err) = data::add_todo_full(&next_item) {
                        eprintln!("Failed to add recurring task: {err}");
                    }
                }

        self.reload()?;
        let message = if done {
            format!("Erledigt: {}", todo.title)
        } else {
            format!("Reaktiviert: {}", todo.title)
        };
        self.show_undo_toast(&message);
        Ok(())
    }

    fn toggle_myday(self: &Rc<Self>, todo: &TodoItem) -> Result<()> {
        let today = Local::now().date_naive();
        let currently_in = todo.myday == Some(today);
        let result = if currently_in {
            data::unset_myday(&todo.key)
        } else {
            data::set_myday_today(&todo.key)
        };
        match result {
            Ok(()) => {
                self.reload()?;
                let message = if currently_in {
                    t("Remove from My Day")
                } else {
                    t("Add to My Day")
                };
                self.show_undo_toast(&format!("{message}: {}", todo.title));
                Ok(())
            }
            Err(err) => {
                if self.handle_conflict(&err) { return Ok(()); }
                Err(err)
            }
        }
    }

    fn set_due_today(self: &Rc<Self>, todo: &TodoItem) -> Result<()> {
        match data::set_due_today(&todo.key, todo.due) {
            Ok(today) => {
                self.reload()?;
                self.show_undo_toast(&format!("Fällig heute ({})", today.format("%Y-%m-%dT%H:%M")));
                Ok(())
            }
            Err(err) => {
                if self.handle_conflict(&err) { return Ok(()); }
                Err(err)
            }
        }
    }

    fn set_due_tomorrow(self: &Rc<Self>, todo: &TodoItem) -> Result<()> {
        match data::set_due_tomorrow(&todo.key) {
            Ok(tomorrow) => {
                self.reload()?;
                self.show_undo_toast(&format!("Fällig morgen ({})", tomorrow.format("%Y-%m-%dT%H:%M")));
                Ok(())
            }
            Err(err) => {
                if self.handle_conflict(&err) { return Ok(()); }
                Err(err)
            }
        }
    }

    fn set_due_weekend(self: &Rc<Self>, todo: &TodoItem) -> Result<()> {
        match data::set_due_weekend(&todo.key) {
            Ok(weekend) => {
                self.reload()?;
                self.show_undo_toast(&format!("Fällig am Wochenende ({})", weekend.format("%Y-%m-%dT%H:%M")));
                Ok(())
            }
            Err(err) => {
                if self.handle_conflict(&err) { return Ok(()); }
                Err(err)
            }
        }
    }

    fn set_due_in_days(self: &Rc<Self>, todo: &TodoItem, days: i64) -> Result<()> {
        let mut updated = todo.clone();
        let base_time = todo.due.map(|d| d.time()).unwrap_or(DEFAULT_DUE_TIME);
        let target_date = Local::now().date_naive() + Duration::days(days);
        updated.due = Some(NaiveDateTime::new(target_date, base_time));
        self.save_item(&updated)
    }

    fn set_due_sometimes(self: &Rc<Self>, todo: &TodoItem) -> Result<()> {
        match data::set_due_sometime(&todo.key) {
            Ok(sometime) => {
                self.reload()?;
                self.show_undo_toast(&format!("Fällig irgendwann ({})", sometime.format("%Y-%m-%dT%H:%M")));
                Ok(())
            }
            Err(err) => {
                if self.handle_conflict(&err) { return Ok(()); }
                Err(err)
            }
        }
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
                shortcut(&t("In the planning picker: add to My Day"), "Return"),
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

        let show_due_row = adw::SwitchRow::builder()
            .title(t("Show only due (until today)"))
            .active(self.show_due_only())
            .build();
        show_due_row.add_prefix(&gtk::Image::from_icon_name("appointment-soon-symbolic"));
        let state_due = Rc::clone(self);
        show_due_row.connect_active_notify(move |row| {
            state_due.set_show_due_only(row.is_active());
        });
        general_group.add(&show_due_row);

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

    fn set_show_due_only(&self, show: bool) {
        {
            let mut prefs = self.preferences.borrow_mut();
            if prefs.show_due_only == show {
                return;
            }
            prefs.show_due_only = show;
        }

        self.persist_preferences();
        self.repopulate_store();
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

        let add_key = match data::add_todo(&title_text) {
            Ok(key) => key,
            Err(err) => {
                self.show_error(&t("Could not create To-do: {}").replace("{}", &err.to_string()));
                return;
            }
        };

        if add_to_myday
            && let Err(err) = data::set_myday_today(&add_key) {
                self.show_error(&t("Could not update entry: {}").replace("{}", &err.to_string()));
            }

        entry.set_text("");
        if let Err(err) = self.reload() {
            self.show_error(&t("Could not reload To-dos: {}").replace("{}", &err.to_string()));
        } else {
            self.show_info(&t("Task added"));
        }

        if !use_ai {
            return;
        }

        let runtime = self.ai_runtime.clone();
        let original = title_text.clone();
        let marker_for_update = add_key.marker.clone();

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
                    todo.key = data::TodoKey {
                        line_index: 0,
                        marker: Some(marker.clone()),
                    };
                    // Keep the just-set myday plan; the AI rewrite would
                    // otherwise drop the token on full re-render.
                    if add_to_myday {
                        todo.myday = Some(Local::now().date_naive());
                    }

                    match data::update_todo_details(&todo) {
                        Ok(_) => {
                            if let Err(err) = state.reload() {
                                state.show_error(&t("Could not reload To-dos: {}").replace("{}", &err.to_string()));
                            } else {
                                state.mark_recently_updated(marker.clone());
                                state.show_info(&t("Changes from file applied"));
                            }
                        }
                        Err(err) => {
                            state.show_error(&t("Could not create To-do: {}").replace("{}", &err.to_string()));
                        }
                    }
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
        {
            let mut prefs = self.preferences.borrow_mut();
            prefs.webdav_path = Some(path.clone());
        }
        self.persist_preferences();
        
        let (use_webdav, url, _, user, pass) = self.get_webdav_prefs();
        if use_webdav
            && let Some(u) = url {
                data::set_backend_config(data::BackendConfig::WebDav {
                    url: u,
                    path: Some(path),
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
    fn populate_myday_view(&self, items: Vec<TodoItem>, today: NaiveDate) {
        let mode = *self.sort_mode.borrow();
        let (canon_projects, canon_contexts) = self.canonical_tag_maps();

        let (planned, rest): (Vec<TodoItem>, Vec<TodoItem>) =
            items.into_iter().partition(|todo| todo.myday == Some(today));

        if planned.is_empty() {
            self.store
                .append(&BoxedAnyObject::new(ListEntry::Header(t("Nothing planned yet — add tasks for today."))));
        } else {
            // Flache Liste ohne Themen-Header: aktive oben, erledigte darunter.
            let (active, done): (Vec<TodoItem>, Vec<TodoItem>) =
                planned.into_iter().partition(|todo| !todo.done);
            for item in active {
                self.store.append(&BoxedAnyObject::new(ListEntry::Item(item)));
            }
            if !done.is_empty() {
                self.store
                    .append(&BoxedAnyObject::new(ListEntry::Header(t("Completed"))));
                for item in done {
                    self.store.append(&BoxedAnyObject::new(ListEntry::Item(item)));
                }
            }
        }

        // Planungs-Picker: alle anderen offenen Aufgaben, fällige zuerst
        // (das "Irgendwann"-Sentinel-Jahr 9999 zählt nicht als fällig).
        let mut candidates: Vec<TodoItem> =
            rest.into_iter().filter(|todo| !todo.done).collect();
        candidates.sort_by_key(|todo| todo.due.unwrap_or(NaiveDateTime::MAX));
        // Fällig wird kalendertagweise beurteilt, nicht nach Uhrzeit: was
        // heute später fällig ist, gehört in die Vorschläge und nicht zu den
        // übrigen offenen Aufgaben.
        let (suggestions, other_open): (Vec<TodoItem>, Vec<TodoItem>) =
            candidates.into_iter().partition(|todo| {
                todo.due
                    .map(|d| d.date() <= today && d.date().year() != 9999)
                    .unwrap_or(false)
            });

        if suggestions.is_empty() && other_open.is_empty() {
            self.store.append(&BoxedAnyObject::new(ListEntry::Header(
                t("No open tasks left to plan."),
            )));
            return;
        }

        // Innerhalb eines Picker-Abschnitts nach Thema/Ort untergliedern
        // (im Datums-Modus liefert group_label None → flache Liste).
        let append_picker_section = |title: String, mut items: Vec<TodoItem>| {
            if items.is_empty() {
                return;
            }
            self.store
                .append(&BoxedAnyObject::new(ListEntry::Header(title)));
            if mode != SortMode::Date {
                sort_items(&mut items, mode);
            }
            let mut last_group: Option<String> = None;
            for item in items {
                if let Some(label) =
                    self.group_label(mode, &item, &canon_projects, &canon_contexts)
                    && last_group.as_ref() != Some(&label) {
                        self.store
                            .append(&BoxedAnyObject::new(ListEntry::Header(label.clone())));
                        last_group = Some(label);
                    }
                self.store
                    .append(&BoxedAnyObject::new(ListEntry::PickerItem(item)));
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
        self.store.remove_all();
        
        let include_done = self.show_completed();
        let due_only = self.show_due_only();
        let myday_only = self.myday_view();
        let now = Local::now().naive_local();
        let today = now.date();

        if search_term.is_empty() {
            if myday_only {
                self.populate_myday_view(items, today);
            } else {
                let mode = *self.sort_mode.borrow();
                let (canon_projects, canon_contexts) = self.canonical_tag_maps();
                let mut last_group: Option<String> = None;
                for item in items.into_iter().filter(|todo| {
                    let status_ok = include_done || !todo.done;
                    let due_ok = if !due_only {
                        true
                    } else {
                        todo.due.map(|d| d <= now).unwrap_or(true)
                    };
                    status_ok && due_ok
                }) {
                    if let Some(label) =
                        self.group_label(mode, &item, &canon_projects, &canon_contexts)
                        && last_group.as_ref() != Some(&label) {
                            self.store
                                .append(&BoxedAnyObject::new(ListEntry::Header(label.clone())));
                            last_group = Some(label);
                        }
                    self.store.append(&BoxedAnyObject::new(ListEntry::Item(item)));
                }
            }
        } else {
            // 1. Suchergebnisse in aktueller Liste
            let current_list_results: Vec<_> = items.iter().filter(|todo| {
                let status_ok = include_done || !todo.done;
                let due_ok = if !due_only {
                    true
                } else {
                    todo.due.map(|d| d <= now).unwrap_or(true)
                };
                status_ok && due_ok && todo.title.to_lowercase().contains(&search_term)
            }).cloned().collect();

            if !current_list_results.is_empty() {
                self.store.append(&BoxedAnyObject::new(ListEntry::Header(t("Search results in current list"))));
                for item in current_list_results.clone() {
                    self.store.append(&BoxedAnyObject::new(ListEntry::Item(item)));
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
                self.store.append(&BoxedAnyObject::new(ListEntry::Header(t("Search results in all open To-dos"))));
                for item in open_results_filtered {
                    self.store.append(&BoxedAnyObject::new(ListEntry::Item(item)));
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
                self.store.append(&BoxedAnyObject::new(ListEntry::Header(t("Search results in completed To-dos"))));
                for item in done_results_filtered {
                    self.store.append(&BoxedAnyObject::new(ListEntry::Item(item)));
                }
            }
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

        if let Some(pos) = scroll_pos
            && let Some(scrolled) = self.scrolled_window.borrow().as_ref() {
                // Nach remove_all + Neubefüllung kennt die ListView ihre Höhe
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

    fn persist_preferences(&self) {
        let prefs = self.preferences.borrow().clone();
        if let Err(err) = write_preferences(&prefs) {
            eprintln!("{}: {err}", t("Could not save settings: {}"));
        }
    }

    fn save_item(self: &Rc<Self>, updated: &TodoItem) -> Result<()> {
        match data::update_todo_details(updated) {
            Ok(()) => {
                self.reload()?;
                self.show_undo_toast(&t("Updated: {}").replace("{}", &updated.title));
                Ok(())
            }
            Err(err) => {
                if self.handle_conflict(&err) { return Ok(()); }
                Err(err)
            }
        }
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
                ListEntry::Item(todo) => (Some(todo.clone()), None),
                ListEntry::PickerItem(todo) => (None, Some(todo.clone())),
                ListEntry::Header(_) => (None, None),
            }
        };

        // Aktivieren (Enter/Klick) im Planungs-Picker übernimmt die
        // Aufgabe in „Mein Tag" — so ist der Picker tastaturbedienbar.
        if let Some(todo) = picker {
            if let Err(err) = self.toggle_myday(&todo) {
                self.show_error(&t("Could not update entry: {}").replace("{}", &err.to_string()));
            }
            return;
        }

        let Some(todo) = item else {
            return;
        };

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
            move || match data::delete_todo(&todo) {
                Ok(_) => {
                    if let Err(err) = state.reload() {
                        state.show_error(&t("Could not reload To-dos: {}").replace("{}", &err.to_string()));
                    } else {
                        state.show_undo_toast(&t("Deleted: {}").replace("{}", &todo.title));
                    }
                }
                Err(e) => state.show_error(&t("Could not delete task: {}").replace("{}", &e.to_string())),
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

        match data::add_todo_full(&copy) {
            Ok(key) => {
                if let Err(err) = self.reload() {
                    self.show_error(&t("Could not reload To-dos: {}").replace("{}", &err.to_string()));
                    return;
                }
                if let Some(marker) = key.marker {
                    self.mark_recently_updated(marker);
                }
                self.show_undo_toast(&t("Duplicated: {}").replace("{}", title));
            }
            Err(err) => {
                if self.handle_conflict(&err) {
                    return;
                }
                self.show_error(&err.to_string());
            }
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
    /// Liste neu laden, Undo-Toast mit Anzahl zeigen.
    fn finish_bulk_action(self: &Rc<Self>, result: Result<usize>, message: &str) {
        match result {
            Ok(count) => {
                self.set_selection_mode(false);
                if let Err(err) = self.reload() {
                    self.show_error(&t("Could not reload To-dos: {}").replace("{}", &err.to_string()));
                    return;
                }
                self.show_undo_toast(&message.replace("{}", &count.to_string()));
            }
            Err(err) => {
                if self.handle_conflict(&err) {
                    return;
                }
                self.show_error(&err.to_string());
            }
        }
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
        self.finish_bulk_action(data::toggle_todos(&keys, done), &message);
    }

    fn bulk_set_due(self: &Rc<Self>, target: data::DueTarget) {
        let keys = self.selected_keys();
        if keys.is_empty() {
            return;
        }
        self.finish_bulk_action(data::set_due_batch(&keys, target), &t("Due date set for {} items"));
    }

    fn bulk_delete(self: &Rc<Self>) {
        let keys = self.selected_keys();
        if keys.is_empty() {
            return;
        }

        let perform_delete = Rc::new({
            let state = Rc::clone(self);
            move || {
                state.finish_bulk_action(data::delete_todos(&keys), &t("{} items deleted"));
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
            let project = project_entry.text().trim().trim_start_matches('+').trim().to_string();
            let context = context_entry.text().trim().trim_start_matches('@').trim().to_string();
            if project.is_empty() && context.is_empty() {
                dialog_assign.close();
                return;
            }
            let project_opt = (!project.is_empty()).then_some(project.as_str());
            let context_opt = (!context.is_empty()).then_some(context.as_str());
            let result = data::assign_project_context_batch(&keys, project_opt, context_opt);
            dialog_assign.close();
            state.finish_bulk_action(result, &t("{} items updated"));
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

        let note_scrolled = gtk::ScrolledWindow::builder()
            .child(&note_view)
            .min_content_height(96)
            .hexpand(true)
            .build();

        let note_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
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

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
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
                    match data::delete_todo(&todo_delete) {
                        Ok(_) => {
                            if let Err(err) = state_delete.reload() {
                                state_delete.show_error(&t("Could not reload To-dos: {}").replace("{}", &err.to_string()));
                            }
                        }
                        Err(e) => state_delete.show_error(&t("Could not delete task: {}").replace("{}", &e.to_string())),
                    }
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

            let project_text = project_entry_save.text().trim().to_string();
            let projects_value: Vec<String> = project_text
                .split_whitespace()
                .map(|s| s.trim_start_matches('+').to_string())
                .filter(|s| !s.is_empty())
                .collect();

            let context_text = context_entry_save.text().trim().to_string();
            let contexts_value: Vec<String> = context_text
                .split_whitespace()
                .map(|s| s.trim_start_matches('@').to_string())
                .filter(|s| !s.is_empty())
                .collect();

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

            if let Err(err) = state_for_save.save_item(&updated) {
                state_for_save.show_error(&t("Could not save task: {}").replace("{}", &err.to_string()));
            } else {
                dialog_save.close();
            }
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

        let state = Rc::clone(self);
        dialog.choose(
            Some(&parent),
            Option::<&gio::Cancellable>::None,
            move |result| {
                match result {
                    Ok(0) => {
                        // Reload
                        if let Err(e) = state.reload() {
                            state.show_error(&t("Could not reload To-dos: {}").replace("{}", &e.to_string()));
                        }
                    }
                    Ok(1) => {
                        // Overwrite — pop the undo entry which still contains the pre-mutation
                        // snapshot and is no longer useful, then let user retry manually.
                        let _ = data::undo();
                        if let Err(e) = state.reload() {
                            state.show_error(&t("Could not reload To-dos: {}").replace("{}", &e.to_string()));
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
            match data::undo() {
                Ok(Some(desc)) => {
                    if let Err(e) = state.reload() {
                        state.show_error(&t("Could not reload To-dos: {}").replace("{}", &e.to_string()));
                    } else {
                        state.show_info(&t("Undone: {}").replace("{}", &desc));
                    }
                }
                Ok(None) => {
                    state.show_info(&t("Nothing to undo"));
                }
                Err(e) => {
                    state.show_error(&e.to_string());
                }
            }
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

            match state.reload() {
                Ok(_) => {
                    if matches!(event, Event::ChangesDoneHint | Event::Changed | Event::Created) {
                        state.show_info(&t("Changes from file applied"));
                    }
                }
                Err(err) => {
                    state.show_error(&t("Update failed: {}").replace("{}", &err.to_string()));
                }
            }
        }));
        *self.monitor.borrow_mut() = Some(monitor);
        Ok(())
    }
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
        .map(|c| {
            c.split_whitespace()
                .map(|s| s.trim().trim_start_matches('@').to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let projects: Vec<String> = parsed
        .project
        .as_deref()
        .map(|p| {
            p.split_whitespace()
                .map(|s| s.trim().trim_start_matches('+').to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
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
        let normalized_projects: Vec<String> = project
            .split_whitespace()
            .map(|p| {
                let p_clean = p.trim_start_matches('+');
                for existing in &known_projects {
                    if existing.to_lowercase() == p_clean.to_lowercase() {
                        return existing.clone();
                    }
                }
                p_clean.to_string()
            })
            .collect();
        parsed.project = Some(normalized_projects.iter()
            .map(|p| format!("+{}", p))
            .collect::<Vec<_>>()
            .join(" "));
    }
    if let Some(ref context) = parsed.context {
        let normalized_contexts: Vec<String> = context
            .split_whitespace()
            .map(|c| {
                let c_clean = c.trim_start_matches('@');
                for existing in &known_contexts {
                    if existing.to_lowercase() == c_clean.to_lowercase() {
                        return existing.clone();
                    }
                }
                c_clean.to_string()
            })
            .collect();
        parsed.context = Some(normalized_contexts.iter()
            .map(|c| format!("@{}", c))
            .collect::<Vec<_>>()
            .join(" "));
    }

    Ok(parsed)
}