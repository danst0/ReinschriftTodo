use std::cell::RefCell;
use std::cmp::Ordering;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Mutex;

use adw::prelude::*;
use adw::{self, Application};
use anyhow::Result;
use chrono::{Datelike, Duration, Local, NaiveDate};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use glib::{clone, BoxedAnyObject};
use gtk::gdk;
use gtk::gio;
use gtk::{AlertDialog, FileDialog, FileFilter};
use gtk::gio::prelude::*;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::data::{self, TodoItem};
use crate::i18n::t;

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
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum SortMode {
    Topic,
    Location,
    Date,
}

impl SortMode {
    fn from_index(index: u32) -> Self {
        match index {
            1 => SortMode::Location,
            2 => SortMode::Date,
            _ => SortMode::Topic,
        }
    }

    fn to_index(self) -> u32 {
        match self {
            SortMode::Topic => 0,
            SortMode::Location => 1,
            SortMode::Date => 2,
        }
    }

    fn from_key(key: &str) -> Self {
        match key {
            "location" => SortMode::Location,
            "date" => SortMode::Date,
            _ => SortMode::Topic,
        }
    }

    fn as_key(self) -> &'static str {
        match self {
            SortMode::Topic => "topic",
            SortMode::Location => "location",
            SortMode::Date => "date",
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Preferences {
    sort_mode: Option<String>,
    #[serde(default)]
    show_done: bool,
    #[serde(default)]
    db_path: Option<String>,
    #[serde(default)]
    show_due_only: bool,
    #[serde(default)]
    use_webdav: bool,
    #[serde(default)]
    webdav_url: Option<String>,
    #[serde(default)]
    webdav_path: Option<String>,
    #[serde(default)]
    webdav_username: Option<String>,
    #[serde(default)]
    webdav_password: Option<String>,
    #[serde(default)]
    use_whisper: bool,
    #[serde(default = "default_whisper_language")]
    whisper_language: String,
}

fn default_whisper_language() -> String {
    "auto".to_string()
}

fn schedule_poll(state: Rc<AppState>, interval: u32) {
    glib::timeout_add_seconds_local(interval, clone!(@weak state => @default-return glib::ControlFlow::Break, move || {
        let next_interval = match state.check_for_updates() {
            Ok(_) => 10,
            Err(e) => {
                eprintln!("{}", t("auto_reload_error").replace("{}", &e.to_string()));
                std::cmp::min(interval * 2, 300)
            }
        };
        schedule_poll(state, next_interval);
        glib::ControlFlow::Break
    }));
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
        }",
    );
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("Could not connect to a display."),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(&t("app_title"))
        .default_width(560)
        .default_height(780)
        .build();

    let header = adw::HeaderBar::builder()
        .title_widget(&gtk::Label::builder().label(&t("app_title")).build())
        .build();

    let search_entry = gtk::SearchEntry::builder()
        .placeholder_text(&t("search_placeholder"))
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

    let settings_btn = gtk::Button::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text(&t("settings"))
        .build();
    settings_btn.add_css_class("flat");
    header.pack_start(&settings_btn);

    let add_task_btn = gtk::ToggleButton::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text(&t("add"))
        .build();
    add_task_btn.add_css_class("flat");
    header.pack_end(&add_task_btn);

    let search_btn = gtk::ToggleButton::builder()
        .icon_name("system-search-symbolic")
        .tooltip_text(&t("search_placeholder"))
        .build();
    search_btn.add_css_class("flat");
    header.pack_end(&search_btn);

    let refresh_btn = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text(&t("reload"))
        .build();
    refresh_btn.add_css_class("flat");
    header.pack_end(&refresh_btn);

    let overlay = adw::ToastOverlay::new();
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);
    let store = gio::ListStore::new::<BoxedAnyObject>();
    let state = Rc::new(AppState::new(&window, &overlay, &store, debug_mode));

    // Neue To-do Eingabezeile unter den Filtereinstellungen
    let new_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    new_row.set_margin_start(12);
    new_row.set_margin_end(12);
    new_row.set_margin_top(6);
    new_row.set_margin_bottom(6);

    let new_entry = gtk::Entry::new();
    new_entry.set_placeholder_text(Some(&t("new_todo_placeholder")));
    new_entry.set_hexpand(true);
    new_row.append(&new_entry);

    let search_btn_for_stop = search_btn.clone();
    search_entry.connect_stop_search(move |_| {
        search_btn_for_stop.set_active(false);
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

    let voice_btn = gtk::Button::builder()
        .icon_name("audio-input-microphone-symbolic")
        .tooltip_text(&t("voice"))
        .css_classes(["flat"])
        .build();
    voice_btn.set_visible(state.use_whisper());
    new_row.append(&voice_btn);

    let state_for_voice = Rc::clone(&state);
    let voice_btn_clone = voice_btn.clone();
    let new_entry_for_voice = new_entry.clone();
    voice_btn.connect_clicked(move |_| {
        state_for_voice.toggle_recording(&voice_btn_clone, &new_entry_for_voice);
    });

    let add_btn = gtk::Button::with_label(&t("add"));
    add_btn.add_css_class("suggested-action");
    new_row.append(&add_btn);

    let state_for_settings_btn = Rc::clone(&state);
    let voice_btn_for_settings = voice_btn.clone();
    settings_btn.connect_clicked(move |_| {
        state_for_settings_btn.show_settings_dialog(Some(voice_btn_for_settings.clone()));
    });

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    controls.set_margin_start(12);
    controls.set_margin_end(12);
    controls.set_margin_top(6);
    controls.set_margin_bottom(6);

    let sort_label = gtk::Label::builder()
        .label(&t("sort_by"))
        .xalign(0.0)
        .build();
    controls.append(&sort_label);

    let sort_selector = gtk::DropDown::from_strings(&[&t("topics"), &t("locations"), &t("date")]);
    sort_selector.set_selected(state.sort_mode().to_index());
    controls.append(&sort_selector);

    let due_filter = gtk::CheckButton::with_label(&t("show_due_only"));
    due_filter.set_margin_start(18);
    due_filter.set_active(state.show_due_only());
    controls.append(&due_filter);

    let add_revealer = gtk::Revealer::builder()
        .child(&new_row)
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
        let title_text = new_entry_for_add.text().trim().to_string();
        if title_text.is_empty() {
            state_for_add.show_error(&t("title_empty_error"));
            return;
        }

        match data::add_todo(&title_text) {
            Ok(_) => {
                new_entry_for_add.set_text("");
                if let Err(err) = state_for_add.reload() {
                    state_for_add.show_error(&t("reload_error").replace("{}", &err.to_string()));
                } else {
                    state_for_add.show_info(&t("task_added"));
                }
            }
            Err(err) => {
                state_for_add.show_error(&t("create_error").replace("{}", &err.to_string()));
            }
        }
    });

    // Enter im Textfeld soll ebenfalls das To-do anlegen
    let state_for_add2 = Rc::clone(&state);
    let new_entry_for_add2 = new_entry.clone();
    new_entry.connect_activate(move |_| {
        let title_text = new_entry_for_add2.text().trim().to_string();
        if title_text.is_empty() {
            state_for_add2.show_error(&t("title_empty_error"));
            return;
        }

        match data::add_todo(&title_text) {
            Ok(_) => {
                new_entry_for_add2.set_text("");
                if let Err(err) = state_for_add2.reload() {
                    state_for_add2.show_error(&t("reload_error").replace("{}", &err.to_string()));
                } else {
                    state_for_add2.show_info(&t("task_added"));
                }
            }
            Err(err) => {
                state_for_add2.show_error(&t("create_error").replace("{}", &err.to_string()));
            }
        }
    });

    // Erzeuge das vertikale Content-Layout noch vor dem Einfügen der neuen Zeile
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&controls);
    content.append(&search_revealer);
    content.append(&add_revealer);
    content.append(&overlay);

    let list_view = create_list_view(&state);
    *state.list_view.borrow_mut() = Some(list_view.clone());
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
            search_btn_esc.set_active(false);
            add_task_btn_esc.set_active(false);
            glib::Propagation::Stop
        } else if key == gdk::Key::question && !has_ctrl {
            state_for_keys.show_cheatsheet();
            glib::Propagation::Stop
        } else if has_ctrl && (key == gdk::Key::n || key == gdk::Key::N) {
            add_task_btn_esc.set_active(!add_task_btn_esc.is_active());
            glib::Propagation::Stop
        } else if has_ctrl && (key == gdk::Key::f || key == gdk::Key::F) {
            search_btn_esc.set_active(!search_btn_esc.is_active());
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
    refresh_action.connect_activate(clone!(@weak state => move |_, _| {
        if let Err(err) = state.reload() {
            state.show_error(&t("load_error").replace("{}", &err.to_string()));
        }
    }));
    app.add_action(&refresh_action);
    app.set_accels_for_action("app.reload", &["<Primary>r"]);

    let settings_action = gio::SimpleAction::new("open-settings", None);
    let state_for_settings_action = Rc::clone(&state);
    settings_action.connect_activate(move |_, _| {
        state_for_settings_action.show_settings_dialog(None);
    });
    app.add_action(&settings_action);

    let close_action = gio::SimpleAction::new("close-window", None);
    let window_for_close = window.clone();
    close_action.connect_activate(move |_, _| {
        window_for_close.close();
    });
    app.add_action(&close_action);
    app.set_accels_for_action("app.close-window", &["<Primary>w", "<Primary>q", "<Alt>F4"]);

    refresh_btn.connect_clicked(clone!(@weak app => move |_| {
        let _ = app.activate_action("app.reload", None);
    }));

    // Keep state alive for the window lifetime so weak references can upgrade.
    unsafe {
        window.set_data("app-state", state.clone());
    }

    window.present();

    if let Err(err) = state.reload() {
        let err_msg = err.to_string();
        let msg = if err_msg == t("no_database_configured") {
            err_msg
        } else {
            format!("{}\n{}", t("load_error").replace("{}", &err_msg), t("select_valid_file"))
        };
        state.show_error(&msg);
        state.show_settings_dialog(None);
    }

    sort_selector.connect_selected_notify(clone!(@weak state => move |dropdown| {
        let mode = SortMode::from_index(dropdown.selected());
        state.set_sort_mode(mode);
    }));

    due_filter.connect_toggled(clone!(@weak state => move |btn| {
        state.set_show_due_only(btn.is_active());
    }));

    if let Err(err) = state.install_monitor() {
        state.show_error(&t("monitor_error").replace("{}", &err.to_string()));
    }

    schedule_poll(state, 10);

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

        let check = gtk::CheckButton::new();
        check.set_valign(gtk::Align::Center);
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

        let today_btn = gtk::Button::builder()
            .icon_name("x-office-calendar-symbolic")
            .tooltip_text(&t("set_due_today"))
            .build();
        today_btn.set_valign(gtk::Align::Center);
        today_btn.add_css_class("flat");
        container.append(&today_btn);

        let postpone_btn = gtk::Button::builder()
            .icon_name("go-next-symbolic")
            .tooltip_text(&t("postpone_tomorrow"))
            .build();
        postpone_btn.set_valign(gtk::Align::Center);
        postpone_btn.add_css_class("flat");
        container.append(&postpone_btn);

        let sometimes_btn = gtk::Button::builder()
            .icon_name("clock-symbolic")
            .tooltip_text(&t("postpone_sometimes"))
            .build();
        sometimes_btn.set_valign(gtk::Align::Center);
        sometimes_btn.add_css_class("flat");
        container.append(&sometimes_btn);

        stack.add_named(&container, Some("item"));
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
                ListEntry::Header(_) => return glib::Propagation::Proceed,
            };
            
            let Some(state) = state_item_key.upgrade() else { return glib::Propagation::Proceed; };
            
            let unicode = keyval.to_unicode();
            match keyval {
                gdk::Key::space => {
                    let _ = state.toggle_item(&todo, !todo.done);
                    glib::Propagation::Stop
                }
                _ if unicode == Some('t') || unicode == Some('T') => {
                    let _ = state.set_due_today(&todo);
                    glib::Propagation::Stop
                }
                _ if unicode == Some('+') || unicode == Some('*') || unicode == Some('=') || 
                     keyval == gdk::Key::plus || keyval == gdk::Key::KP_Add || 
                     keyval == gdk::Key::asterisk || keyval == gdk::Key::KP_Multiply => {
                    let _ = state.set_due_in_days(&todo, 1);
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
            list_item.set_data("todo-title", title.downgrade());
            list_item.set_data("todo-meta", meta.downgrade());
            list_item.set_data("todo-button", postpone_btn.downgrade());
        }

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
                ListEntry::Header(_) => return,
            };
            if btn.is_active() == todo.done {
                return;
            }

            if let Some(state) = state_for_handler.upgrade() {
                if let Err(err) = state.toggle_item(&todo, btn.is_active()) {
                    state.show_error(&t("update_error").replace("{}", &err.to_string()));
                }
            }
        });

        let postpone_list = list_item.downgrade();
        let postpone_state = factory_state.clone();
        postpone_btn.connect_clicked(move |_| {
            let Some(list_item) = postpone_list.upgrade() else {
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
                ListEntry::Header(_) => return,
            };

            if let Some(state) = postpone_state.upgrade() {
                state.show_due_shortcuts(&todo);
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
                ListEntry::Header(_) => return,
            };

            if let Some(state) = today_state.upgrade() {
                if let Err(err) = state.set_due_today(&todo) {
                    state.show_error(&t("set_due_error").replace("{}", &err.to_string()));
                }
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
                ListEntry::Header(_) => return,
            };

            if let Some(state) = sometimes_state.upgrade() {
                if let Err(err) = state.set_due_sometimes(&todo) {
                    state.show_error(&t("set_due_error").replace("{}", &err.to_string()));
                }
            }
        });

    });

    factory.connect_bind(|_, list_item_obj| {
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

        match &*entry {
            ListEntry::Header(label) => {
                stack.set_visible_child_name("header");
                if let Some(header_ref_ptr) = unsafe {
                    list_item.data::<glib::WeakRef<gtk::Label>>("header-label")
                } {
                    if let Some(header_label) = unsafe { header_ref_ptr.as_ref() }.upgrade() {
                        header_label.set_text(label);
                    }
                }
            }
            ListEntry::Item(todo) => {
                stack.set_visible_child_name("item");
                if let Some(check_ref_ptr) = unsafe {
                    list_item.data::<glib::WeakRef<gtk::CheckButton>>("todo-check")
                } {
                    if let Some(check_widget) = unsafe { check_ref_ptr.as_ref() }.upgrade() {
                        if check_widget.is_active() != todo.done {
                            check_widget.set_active(todo.done);
                        }
                    }
                }
                if let Some(title_ref_ptr) = unsafe {
                    list_item.data::<glib::WeakRef<gtk::Label>>("todo-title")
                } {
                    if let Some(title_widget) = unsafe { title_ref_ptr.as_ref() }.upgrade() {
                        title_widget.set_text(&todo.title);
                        if todo.done {
                            title_widget.add_css_class("dim-label");
                        } else {
                            title_widget.remove_css_class("dim-label");
                        }
                    }
                }
                if let Some(meta_ref_ptr) = unsafe {
                    list_item.data::<glib::WeakRef<gtk::Label>>("todo-meta")
                } {
                    if let Some(meta_widget) = unsafe { meta_ref_ptr.as_ref() }.upgrade() {
                        meta_widget.set_text(&format_metadata(todo));
                    }
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
            _debug_mode: debug_mode,
            last_fingerprint: RefCell::new(None),
        }
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

    fn use_whisper(&self) -> bool {
        self.preferences.borrow().use_whisper
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

    fn check_for_updates(&self) -> Result<()> {
        let current_fp = data::get_fingerprint()?;
        let last_fp = self.last_fingerprint.borrow().clone();

        if Some(current_fp) != last_fp {
            self.reload()?;
        }
        Ok(())
    }

    fn toggle_item(&self, todo: &TodoItem, done: bool) -> Result<()> {
        let today = Local::now().date_naive();
        let is_historic = todo.due.map(|d| d < today).unwrap_or(false);
        let is_recurring = todo.recurrence.is_some();

        if done && is_historic && is_recurring {
            let mut updated = todo.clone();
            updated.due = Some(today);
            updated.done = true;
            data::update_todo_details(&updated)?;
        } else {
            data::toggle_todo(&todo.key, done)?;
        }

        if done {
            if let Some(rule) = todo.recurrence.as_deref() {
                if let Some(next_due) = data::next_due_date(todo.due, rule) {
                    let mut next_item = todo.clone();
                    next_item.key = data::TodoKey { line_index: 0, marker: None };
                    next_item.done = false;
                    next_item.due = Some(next_due);
                    if let Err(err) = data::add_todo_full(&next_item) {
                        eprintln!("Failed to add recurring task: {err}");
                    }
                }
            }
        }

        self.reload()?;
        let message = if done {
            format!("Erledigt: {}", todo.title)
        } else {
            format!("Reaktiviert: {}", todo.title)
        };
        self.show_info(&message);
        Ok(())
    }

    fn set_due_today(&self, todo: &TodoItem) -> Result<()> {
        let today = data::set_due_today(&todo.key)?;
        self.reload()?;
        self.show_info(&format!("Fällig heute ({})", today));
        Ok(())
    }

    fn set_due_in_days(&self, todo: &TodoItem, days: i64) -> Result<()> {
        let mut updated = todo.clone();
        let target = Local::now().date_naive() + Duration::days(days);
        updated.due = Some(target);
        self.save_item(&updated)
    }

    fn set_due_sometimes(&self, todo: &TodoItem) -> Result<()> {
        let mut updated = todo.clone();
        updated.due = Some(NaiveDate::from_ymd_opt(9999, 12, 31).unwrap());
        self.save_item(&updated)
    }

    fn show_due_shortcuts(self: &Rc<Self>, todo: &TodoItem) {
        let Some(parent) = self.window.upgrade() else {
            self.show_error("Kein Fenster verfügbar");
            return;
        };

        let dialog = AlertDialog::builder()
            .modal(true)
            .build();
        dialog.set_message("Fälligkeit verschieben");
        dialog.set_detail("Bitte Ziel wählen");
        dialog.set_buttons(&["Morgen", "In 3 Tagen", "In 7 Tagen", "In einem Monat", "Irgendwann", "Abbrechen"]);
        dialog.set_default_button(0);
        dialog.set_cancel_button(5);

        let state = Rc::clone(self);
        let base_todo = todo.clone();
        dialog.choose(
            Some(&parent),
            Option::<&gio::Cancellable>::None,
            clone!(@strong state, @strong base_todo => move |result| {
                match result {
                    Ok(index) => {
                        let action = match index {
                            0 => Some(1),
                            1 => Some(3),
                            2 => Some(7),
                            3 => Some(30),
                            4 => None,
                            _ => return,
                        };

                        let outcome = match action {
                            Some(days) => state.set_due_in_days(&base_todo, days),
                            None => state.set_due_sometimes(&base_todo),
                        };

                        if let Err(err) = outcome {
                            state.show_error(&format!("Konnte verschieben: {err}"));
                        }
                    }
                    Err(err) => {
                        state.show_error(&format!("Konnte Dialog nicht anzeigen: {err}"));
                    }
                }
            }),
        );
    }

    fn show_cheatsheet(self: &Rc<Self>) {
        let Some(parent) = self.window.upgrade() else {
            self.show_error(&t("no_window"));
            return;
        };

        let dialog = adw::Window::builder()
            .title(&t("cheatsheet"))
            .transient_for(&parent)
            .modal(true)
            .default_width(400)
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
        content.set_margin_top(24);
        content.set_margin_bottom(24);
        content.set_margin_start(24);
        content.set_margin_end(24);

        let grid = gtk::Grid::builder()
            .column_spacing(24)
            .row_spacing(8)
            .build();

        let shortcuts = [
            ("key_help", "?"),
            ("key_new", "Ctrl + N"),
            ("key_search", "Ctrl + F"),
            ("key_reload", "Ctrl + R"),
            ("key_quit", "Ctrl + Q"),
            ("key_nav", "↑ / ↓"),
            ("key_toggle", "Space"),
            ("key_edit", "Enter"),
            ("key_today", "t"),
            ("key_tomorrow", "+"),
            ("key_sometimes", "s"),
        ];

        for (i, (key, shortcut)) in shortcuts.iter().enumerate() {
            let key_label = gtk::Label::builder()
                .label(*shortcut)
                .xalign(1.0)
                .build();
            key_label.add_css_class("dim-label");
            
            let desc_label = gtk::Label::builder()
                .label(&t(key))
                .xalign(0.0)
                .build();
            
            grid.attach(&key_label, 0, i as i32, 1, 1);
            grid.attach(&desc_label, 1, i as i32, 1, 1);
        }

        content.append(&grid);

        let close_btn = gtk::Button::with_label(&t("close"));
        close_btn.set_halign(gtk::Align::End);
        close_btn.set_margin_top(12);
        let dialog_close = dialog.clone();
        close_btn.connect_clicked(move |_| {
            dialog_close.close();
        });
        content.append(&close_btn);

        dialog.set_content(Some(&content));
        dialog.present();
    }

    fn show_settings_dialog(self: &Rc<Self>, voice_btn: Option<gtk::Button>) {
        let Some(parent) = self.window.upgrade() else {
            self.show_error(&t("no_window"));
            return;
        };

        // Enforce WebDAV mode
        self.set_use_webdav(true);

        let dialog = adw::PreferencesWindow::builder()
            .title(&t("settings"))
            .transient_for(&parent)
            .modal(true)
            .default_width(480)
            .build();

        // --- General Page ---
        let general_page = adw::PreferencesPage::builder()
            .title(&t("general"))
            .icon_name("preferences-system-symbolic")
            .build();
        dialog.add(&general_page);

        let general_group = adw::PreferencesGroup::builder()
            .title(&t("general"))
            .build();
        general_page.add(&general_group);

        let show_done_row = adw::SwitchRow::builder()
            .title(&t("show_completed"))
            .active(self.show_completed())
            .build();
        show_done_row.add_prefix(&gtk::Image::from_icon_name("view-list-symbolic"));
        let state_done = Rc::clone(self);
        show_done_row.connect_active_notify(move |row| {
            state_done.set_show_completed(row.is_active());
        });
        general_group.add(&show_done_row);

        let show_due_row = adw::SwitchRow::builder()
            .title(&t("show_due_only_mode"))
            .active(self.show_due_only())
            .build();
        show_due_row.add_prefix(&gtk::Image::from_icon_name("appointment-soon-symbolic"));
        let state_due = Rc::clone(self);
        show_due_row.connect_active_notify(move |row| {
            state_due.set_show_due_only(row.is_active());
        });
        general_group.add(&show_due_row);

        // --- WebDAV Page ---
        let webdav_page = adw::PreferencesPage::builder()
            .title(&t("webdav"))
            .icon_name("network-server-symbolic")
            .build();
        dialog.add(&webdav_page);

        let webdav_group = adw::PreferencesGroup::builder()
            .title(&t("webdav"))
            .build();
        webdav_page.add(&webdav_group);

        let (_, _, wd_path, wd_user, wd_pass) = self.get_webdav_prefs();
        // Note: wd_url is fetched inside the closure below or we can get it here if needed, 
        // but we need to bind it to the row.
        // Let's get the current values again to populate the fields.
        let (_, wd_url, _, _, _) = self.get_webdav_prefs();

        let url_row = adw::EntryRow::builder()
            .title(&t("webdav_url"))
            .text(wd_url.unwrap_or_default())
            .build();
        let state_url = Rc::clone(self);
        url_row.connect_changed(move |row| {
            state_url.set_webdav_url(row.text().to_string());
        });
        webdav_group.add(&url_row);

        let path_row = adw::EntryRow::builder()
            .title(&t("path_relative"))
            .text(wd_path.unwrap_or_default())
            .build();
        let state_path = Rc::clone(self);
        path_row.connect_changed(move |row| {
            state_path.set_webdav_path(row.text().to_string());
        });
        webdav_group.add(&path_row);

        let user_row = adw::EntryRow::builder()
            .title(&t("username"))
            .text(wd_user.unwrap_or_default())
            .build();
        let state_user = Rc::clone(self);
        user_row.connect_changed(move |row| {
            state_user.set_webdav_username(row.text().to_string());
        });
        webdav_group.add(&user_row);

        let pass_row = adw::PasswordEntryRow::builder()
            .title(&t("password"))
            .text(wd_pass.unwrap_or_default())
            .build();
        let state_pass = Rc::clone(self);
        pass_row.connect_changed(move |row| {
            state_pass.set_webdav_password(row.text().to_string());
        });
        webdav_group.add(&pass_row);

        let check_row = adw::ActionRow::builder()
            .title(&t("check_connection"))
            .build();
        let check_button = gtk::Button::builder()
            .label(&t("check_connection"))
            .valign(gtk::Align::Center)
            .build();
        check_button.add_css_class("flat");
        check_row.add_suffix(&check_button);
        
        let state_for_check = Rc::clone(self);
        check_button.connect_clicked(move |_| {
            let (_, url, path, user, pass) = state_for_check.get_webdav_prefs();
            
            let Some(u) = url else {
                state_for_check.show_error(&t("no_url_error"));
                return;
            };
            if u.trim().is_empty() {
                state_for_check.show_error(&t("no_url_error"));
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
                            Ok(_) => state_bg.show_info(&t("connection_success")),
                            Err(e) => {
                                eprintln!("{}", t("webdav_conn_error").replace("{}", &e.to_string()));
                                state_bg.show_error(&t("connection_failed").replace("{}", &e.to_string()));
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
            .title(&t("voice"))
            .icon_name("audio-input-microphone-symbolic")
            .build();
        dialog.add(&voice_page);

        let voice_group = adw::PreferencesGroup::builder()
            .title(&t("voice"))
            .build();
        voice_page.add(&voice_group);

        let progress_bar = gtk::ProgressBar::builder()
            .visible(false)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        voice_group.add(&progress_bar);

        let use_whisper_row = adw::SwitchRow::builder()
            .title(&t("use_whisper"))
            .subtitle(&t("whisper_desc"))
            .active(self.use_whisper())
            .build();
        use_whisper_row.add_prefix(&gtk::Image::from_icon_name("audio-input-microphone-symbolic"));
        
        let languages = vec!["auto", "en", "de", "es", "fr", "it", "ja", "zh", "nl", "pl", "pt", "ru", "tr", "sv"];
        let language_names = [
            t("lang_auto"), t("lang_en"), t("lang_de"), t("lang_es"), t("lang_fr"), 
            t("lang_it"), t("lang_ja"), t("lang_zh"), t("lang_nl"), t("lang_pl"), 
            t("lang_pt"), t("lang_ru"), t("lang_tr"), t("lang_sv")
        ];
        let language_names_refs: Vec<&str> = language_names.iter().map(|s| s.as_str()).collect();
        
        let language_model = gtk::StringList::new(&language_names_refs);
        
        let language_row = adw::ComboRow::builder()
            .title(&t("whisper_language"))
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
            .title(&t("about"))
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
            .label(&format!("{} 0.9.30", t("version")))
            .css_classes(["dim-label"])
            .build();
        banner_box.append(&app_version);

        banner.set_child(Some(&banner_box));
        about_group.add(&banner);

        let info_group = adw::PreferencesGroup::builder()
            .build();
        about_page.add(&info_group);

        let dev_row = adw::ActionRow::builder()
            .title(&t("developer"))
            .subtitle("Dr. Daniel Dumke")
            .build();
        info_group.add(&dev_row);

        let site_row = adw::ActionRow::builder()
            .title(&t("website"))
            .subtitle("https://github.com/danst0/ReinschriftTodo")
            .activatable(true)
            .build();
        site_row.connect_activated(|_| {
            let launcher = gtk::FileLauncher::new(Some(&gio::File::for_uri("https://github.com/danst0/ReinschriftTodo")));
            launcher.launch(None::<&gtk::Window>, gio::Cancellable::NONE, |_| {});
        });
        info_group.add(&site_row);

        let license_row = adw::ActionRow::builder()
            .title(&t("license"))
            .subtitle("CC-BY-NC-SA-4.0")
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
            if let Ok(meta) = fs::metadata(&path) {
                if meta.len() > 450 * 1024 * 1024 {
                    if let Some(row) = &switch_row {
                        row.set_sensitive(true);
                    }
                    return;
                }
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

        self.show_info(&t("downloading_model"));

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
                        state.show_info(&t("model_download_finished"));
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
                    state.show_error(&format!("{}: {}", t("model_download_error"), e));
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
                    path: path,
                    username: user,
                    password: pass,
                });
            }
        } else {
            let path = data::todo_path();
            data::set_backend_config(data::BackendConfig::Local(path));
        }

        if let Err(err) = self.reload() {
             self.show_error(&t("load_data_error").replace("{}", &err.to_string()));
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
                url: url,
                path: path,
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
        if use_webdav {
            if let Some(u) = url {
                data::set_backend_config(data::BackendConfig::WebDav {
                    url: u,
                    path: Some(path),
                    username: user,
                    password: pass,
                });
            }
        }
    }

    fn set_webdav_username(&self, username: String) {
        {
            let mut prefs = self.preferences.borrow_mut();
            prefs.webdav_username = Some(username.clone());
        }
        self.persist_preferences();

        let (use_webdav, url, path, _, pass) = self.get_webdav_prefs();
        if use_webdav {
            if let Some(u) = url {
                data::set_backend_config(data::BackendConfig::WebDav {
                    url: u,
                    path: path,
                    username: Some(username),
                    password: pass,
                });
            }
        }
    }

    fn set_webdav_password(&self, password: String) {
        {
            let mut prefs = self.preferences.borrow_mut();
            prefs.webdav_password = Some(password.clone());
        }
        self.persist_preferences();

        let (use_webdav, url, path, user, _) = self.get_webdav_prefs();
        if use_webdav {
            if let Some(u) = url {
                data::set_backend_config(data::BackendConfig::WebDav {
                    url: u,
                    path: path,
                    username: user,
                    password: Some(password),
                });
            }
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

    fn repopulate_store(&self) {
        let mut selected_key = None;
        let mut scroll_pos = None;

        if let Some(scrolled) = self.scrolled_window.borrow().as_ref() {
            let adj = scrolled.vadjustment();
            scroll_pos = Some(adj.value());
        }

        if let Some(list_view) = self.list_view.borrow().as_ref() {
            if let Some(model) = list_view.model() {
                if let Ok(selection) = model.downcast::<gtk::SingleSelection>() {
                    let pos = selection.selected();
                    if pos != gtk::INVALID_LIST_POSITION {
                        if let Some(obj) = self.store.item(pos) {
                            if let Ok(boxed) = obj.downcast::<BoxedAnyObject>() {
                                let entry = boxed.borrow::<ListEntry>();
                                if let ListEntry::Item(todo) = &*entry {
                                    selected_key = Some(todo.key.clone());
                                }
                            }
                        }
                    }
                }
            }
        }

        let search_term = self.search_term.borrow().to_lowercase();
        let mut items = self.cached_items.borrow().clone();
        self.sort_items(&mut items);
        self.store.remove_all();
        
        let include_done = self.show_completed();
        let due_only = self.show_due_only();
        let today = Local::now().date_naive();

        if search_term.is_empty() {
            let mode = *self.sort_mode.borrow();
            let mut last_group: Option<String> = None;
            for item in items.into_iter().filter(|todo| {
                let status_ok = include_done || !todo.done;
                let due_ok = if !due_only {
                    true
                } else {
                    todo.due.map(|d| d <= today).unwrap_or(true)
                };
                status_ok && due_ok
            }) {
                if let Some(label) = self.group_label(mode, &item) {
                    if last_group.as_ref() != Some(&label) {
                        self.store
                            .append(&BoxedAnyObject::new(ListEntry::Header(label.clone())));
                        last_group = Some(label);
                    }
                }
                self.store.append(&BoxedAnyObject::new(ListEntry::Item(item)));
            }
        } else {
            // 1. Suchergebnisse in aktueller Liste
            let current_list_results: Vec<_> = items.iter().filter(|todo| {
                let status_ok = include_done || !todo.done;
                let due_ok = if !due_only {
                    true
                } else {
                    todo.due.map(|d| d <= today).unwrap_or(true)
                };
                status_ok && due_ok && todo.title.to_lowercase().contains(&search_term)
            }).cloned().collect();

            if !current_list_results.is_empty() {
                self.store.append(&BoxedAnyObject::new(ListEntry::Header(t("search_results_current"))));
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
                self.store.append(&BoxedAnyObject::new(ListEntry::Header(t("search_results_open"))));
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
                self.store.append(&BoxedAnyObject::new(ListEntry::Header(t("search_results_done"))));
                for item in done_results_filtered {
                    self.store.append(&BoxedAnyObject::new(ListEntry::Item(item)));
                }
            }
        }

        let mut restored = false;

        if let Some(key) = selected_key {
            if let Some(list_view) = self.list_view.borrow().as_ref() {
                if let Some(model) = list_view.model() {
                    if let Ok(selection) = model.downcast::<gtk::SingleSelection>() {
                        for i in 0..self.store.n_items() {
                            if let Some(obj) = self.store.item(i) {
                                if let Ok(boxed) = obj.downcast::<BoxedAnyObject>() {
                                    let entry = boxed.borrow::<ListEntry>();
                                    if let ListEntry::Item(todo) = &*entry {
                                        if todo.key == key {
                                            selection.set_selected(i);
                                            list_view.scroll_to(i, gtk::ListScrollFlags::NONE, None);
                                            restored = true;
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if !restored {
            if let Some(pos) = scroll_pos {
                if let Some(scrolled) = self.scrolled_window.borrow().as_ref() {
                    let adj = scrolled.vadjustment();
                    glib::idle_add_local(move || {
                        let max = (adj.upper() - adj.page_size()).max(0.0);
                        adj.set_value(pos.min(max));
                        glib::ControlFlow::Break
                    });
                }
            }
        }
    }

    fn persist_preferences(&self) {
        let prefs = self.preferences.borrow().clone();
        if let Err(err) = write_preferences(&prefs) {
            eprintln!("{}: {err}", t("save_settings_error"));
        }
    }

    fn save_item(&self, updated: &TodoItem) -> Result<()> {
        data::update_todo_details(updated)?;
        self.reload()?;
        self.show_info(&t("updated_task").replace("{}", &updated.title));
        Ok(())
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
            self.show_error(&t("model_not_found"));
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
                &model_path.to_string_lossy(),
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

            let num_segments = state_whisper.full_n_segments().expect("failed to get segments");
            let mut result_text = String::new();
            for i in 0..num_segments {
                if let Ok(segment) = state_whisper.full_get_segment_text(i) {
                    result_text.push_str(&segment);
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
        let entry = todo_obj.borrow::<ListEntry>();
        let todo = match &*entry {
            ListEntry::Item(todo) => todo.clone(),
            ListEntry::Header(_) => return,
        };
        drop(entry);
        self.show_details_dialog(&todo);
    }

    fn show_details_dialog(self: &Rc<Self>, todo: &TodoItem) {
        let Some(parent) = self.window.upgrade() else {
            self.show_error(&t("no_window"));
            return;
        };

        let dialog = adw::Window::builder()
            .title(&t("edit_task"))
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

        let section_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        section_row.append(&gtk::Label::builder().label(&t("section")).xalign(0.0).build());
        let section_value = gtk::Label::builder()
            .label(&todo.section)
            .xalign(0.0)
            .build();
        section_value.add_css_class("dim-label");
        section_row.append(&section_value);
        content.append(&section_row);

        let title_entry = gtk::Entry::builder().text(&todo.title).hexpand(true).build();
        let title_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        title_row.append(&gtk::Label::builder().label(&t("title")).xalign(0.0).build());
        title_row.append(&title_entry);
        content.append(&title_row);

        let project_entry = gtk::Entry::new();
        if let Some(project) = &todo.project {
            project_entry.set_text(project);
        }
        let project_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        project_row.append(&gtk::Label::builder().label(&t("project_plus")).xalign(0.0).build());
        project_row.append(&project_entry);
        content.append(&project_row);

        let context_entry = gtk::Entry::new();
        if let Some(context) = &todo.context {
            context_entry.set_text(context);
        }
        let context_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        context_row.append(&gtk::Label::builder().label(&t("location_at")).xalign(0.0).build());
        context_row.append(&context_entry);
        content.append(&context_row);

        let due_entry = gtk::Entry::new();
        due_entry.set_placeholder_text(Some("YYYY-MM-DD"));
        if let Some(due) = todo.due {
            let due_string = due.format("%Y-%m-%d").to_string();
            due_entry.set_text(&due_string);
        }
        let due_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        due_row.append(&gtk::Label::builder().label(&t("due_date")).xalign(0.0).build());
        let due_inputs = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        due_entry.set_hexpand(true);
        due_inputs.append(&due_entry);
        let due_today_btn = gtk::Button::with_label(&t("today"));
        due_today_btn.add_css_class("flat");
        due_inputs.append(&due_today_btn);
        due_row.append(&due_inputs);
        content.append(&due_row);

        let recurrence_values = ["", "daily", "weekly", "monthly"];
        let recurrence_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        recurrence_row.append(&gtk::Label::builder().label(&t("recurrence")).xalign(0.0).build());
        let recurrence_list = gtk::StringList::new(&[]);
        recurrence_list.append(&t("recurrence_none"));
        recurrence_list.append(&t("recurrence_daily"));
        recurrence_list.append(&t("recurrence_weekly"));
        recurrence_list.append(&t("recurrence_monthly"));
        let recurrence_dropdown = gtk::DropDown::new(Some(recurrence_list.clone()), None::<&gtk::Expression>);
        let rec_index = todo
            .recurrence
            .as_deref()
            .and_then(|r| recurrence_values.iter().position(|v| v == &r))
            .unwrap_or(0) as u32;
        recurrence_dropdown.set_selected(rec_index);
        recurrence_row.append(&recurrence_dropdown);
        content.append(&recurrence_row);

        let done_check = gtk::CheckButton::with_label(&t("done"));
        done_check.set_active(todo.done);
        content.append(&done_check);

        let comment_entry = gtk::Entry::new();
        let comment_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        comment_row.append(&gtk::Label::builder().label(&t("comment")).xalign(0.0).build());
        comment_row.append(&comment_entry);
        comment_row.set_visible(false);
        content.append(&comment_row);

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        let cancel_btn = gtk::Button::with_label(&t("cancel"));
        let delete_btn = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text(&t("delete"))
            .css_classes(["destructive-action"])
            .build();
        let close_with_comment_btn = gtk::Button::with_label(&t("close_with_comment"));
        let save_btn = gtk::Button::with_label(&t("save"));
        save_btn.add_css_class("suggested-action");
        buttons.append(&cancel_btn);
        buttons.append(&delete_btn);
        buttons.append(&close_with_comment_btn);
        buttons.append(&save_btn);
        content.append(&buttons);
        dialog.set_content(Some(&content));

        let dialog_cancel = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dialog_cancel.close();
        });

        let dialog_delete = dialog.clone();
        let state_delete = self.clone();
        let todo_delete = todo.clone();
        delete_btn.connect_clicked(move |_| {
            if let Err(e) = data::delete_todo(&todo_delete) {
                state_delete.show_error(&t("delete_error").replace("{}", &e.to_string()));
            }
            dialog_delete.close();
        });

        let due_entry_for_button = due_entry.clone();
        due_today_btn.connect_clicked(move |_| {
            let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
            due_entry_for_button.set_text(&today);
        });

        let dialog_save = dialog.clone();
        let state_for_save = Rc::clone(self);
        let base_item = todo.clone();
        let title_entry_save = title_entry.clone();
        let project_entry_save = project_entry.clone();
        let context_entry_save = context_entry.clone();
        let due_entry_save = due_entry.clone();
        let done_check_save = done_check.clone();
        let comment_entry_save = comment_entry.clone();
        let comment_row_save = comment_row.clone();
        let recurrence_dropdown_save = recurrence_dropdown.clone();
        save_btn.connect_clicked(move |_| {
            let mut title_text = title_entry_save.text().trim().to_string();
            if title_text.is_empty() {
                state_for_save.show_error(&t("title_empty_error"));
                return;
            }

            if comment_row_save.is_visible() {
                let comment = comment_entry_save.text().trim().to_string();
                if !comment.is_empty() {
                    title_text = format!("{} ({})", title_text, comment);
                }
            }

            let project_text = project_entry_save.text().trim().to_string();
            let project_value = if project_text.is_empty() {
                None
            } else {
                Some(project_text)
            };

            let context_text = context_entry_save.text().trim().to_string();
            let context_value = if context_text.is_empty() {
                None
            } else {
                Some(context_text)
            };

            let due_text = due_entry_save.text().trim().to_string();
            let due_value = if due_text.is_empty() {
                None
            } else {
                match NaiveDate::parse_from_str(&due_text, "%Y-%m-%d") {
                    Ok(date) => Some(date),
                    Err(_) => {
                        state_for_save.show_error(&t("invalid_date_error"));
                        return;
                    }
                }
            };

            let rec_values = ["", "daily", "weekly", "monthly"];
            let rec_idx = recurrence_dropdown_save.selected() as usize;
            let recurrence_value = rec_values
                .get(rec_idx)
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());

            let mut updated = base_item.clone();
            updated.title = title_text;
            updated.project = project_value;
            updated.context = context_value;
            updated.reference = base_item.reference.clone();
            updated.due = due_value;
            updated.recurrence = recurrence_value;
            updated.done = done_check_save.is_active();

            if let Err(err) = state_for_save.save_item(&updated) {
                state_for_save.show_error(&t("save_task_error").replace("{}", &err.to_string()));
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
        match *self.sort_mode.borrow() {
            SortMode::Topic => items.sort_by(compare_by_project),
            SortMode::Location => items.sort_by(compare_by_context),
            SortMode::Date => items.sort_by(compare_by_due),
        }
    }

    fn group_label(&self, mode: SortMode, item: &TodoItem) -> Option<String> {
        match mode {
            SortMode::Topic => Some(t("topic_group").replace(
                "{}",
                item.project
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&t("no_project"))
            )),
            SortMode::Location => Some(t("location_group").replace(
                "{}",
                item.context
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&t("no_location"))
            )),
            SortMode::Date => None,
        }
    }

    fn show_info(&self, message: &str) {
        let toast = adw::Toast::builder().title(message).build();
        self.overlay.add_toast(toast);
    }

    fn show_error(&self, message: &str) {
        let toast = adw::Toast::builder()
            .title(message)
            .priority(adw::ToastPriority::High)
            .build();
        self.overlay.add_toast(toast);
    }

    fn install_monitor(self: &Rc<Self>) -> Result<()> {
        let file = gio::File::for_path(data::todo_path());
        let monitor = file.monitor_file(gio::FileMonitorFlags::NONE, Option::<&gio::Cancellable>::None)?;
        monitor.connect_changed(clone!(@weak self as state => move |_, _, _, event| {
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
                        state.show_info(&t("changes_applied"));
                    }
                }
                Err(err) => {
                    state.show_error(&t("update_failed").replace("{}", &err.to_string()));
                }
            }
        }));
        *self.monitor.borrow_mut() = Some(monitor);
        Ok(())
    }
}

fn format_metadata(item: &TodoItem) -> String {
    let mut parts = Vec::new();
    if !item.section.is_empty() {
        parts.push(item.section.clone());
    }
    if let Some(project) = &item.project {
        parts.push(format!("+{}", project));
    }
    if let Some(context) = &item.context {
        parts.push(format!("@{}", context));
    }
    if let Some(due) = item.due {
        if due.year() == 9999 {
            parts.push(t("sometimes"));
        } else {
            parts.push(t("due_label").replace("{}", &due.to_string()));
        }
    }
    if let Some(rule) = &item.recurrence {
        let label = match rule.as_str() {
            "daily" => t("recurrence_daily"),
            "weekly" => t("recurrence_weekly"),
            "monthly" => t("recurrence_monthly"),
            _ => rule.clone(),
        };
        parts.push(format!("↻ {}", label));
    }
    if let Some(reference) = &item.reference {
        parts.push(format!("↗ {}", reference));
    }

    parts.join(" • ")
}

fn load_preferences() -> Preferences {
    let path = preferences_path();
    if let Ok(data) = fs::read_to_string(&path) {
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        Preferences::default()
    }
}

fn write_preferences(prefs: &Preferences) -> std::io::Result<()> {
    let path = preferences_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let serialized = serde_json::to_string_pretty(prefs).unwrap_or_else(|_| "{}".into());
    fs::write(path, serialized)
}

fn preferences_path() -> PathBuf {
    let mut dir = glib::user_config_dir();
    dir.push("reinschrift_todo");
    dir.push("preferences.json");
    dir
}

fn compare_by_project(a: &TodoItem, b: &TodoItem) -> Ordering {
    compare_option_str(a.project.as_deref(), b.project.as_deref())
        .then_with(|| lexical_order(&a.section, &b.section))
        .then_with(|| lexical_order(&a.title, &b.title))
        .then_with(|| compare_option_str(a.context.as_deref(), b.context.as_deref()))
}

fn compare_by_context(a: &TodoItem, b: &TodoItem) -> Ordering {
    compare_option_str(a.context.as_deref(), b.context.as_deref())
        .then_with(|| lexical_order(&a.section, &b.section))
        .then_with(|| lexical_order(&a.title, &b.title))
        .then_with(|| compare_option_str(a.project.as_deref(), b.project.as_deref()))
}

fn compare_by_due(a: &TodoItem, b: &TodoItem) -> Ordering {
    compare_option_date(a.due, b.due)
        .then_with(|| compare_by_project(a, b))
}

fn compare_option_str(a: Option<&str>, b: Option<&str>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => lexical_order(a, b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn compare_option_date(a: Option<NaiveDate>, b: Option<NaiveDate>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => Ordering::Equal,
    }
}

fn lexical_order(a: &str, b: &str) -> Ordering {
    a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase())
}