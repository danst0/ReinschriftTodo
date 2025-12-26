use std::cell::RefCell;
use std::cmp::Ordering;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use adw::{self, Application};
use anyhow::Result;
use chrono::{Datelike, Duration, Local, NaiveDate};
use glib::{clone, BoxedAnyObject};
use gtk::gio;
use gtk::{AlertDialog, FileDialog, FileFilter};
use gtk::gio::prelude::*;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json;

use crate::data::{self, TodoItem};
use crate::i18n::t;

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
}

fn schedule_poll(state: Rc<AppState>, interval: u32) {
    glib::timeout_add_seconds_local(interval, clone!(@weak state => @default-return glib::ControlFlow::Break, move || {
        let next_interval = match state.reload() {
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

pub fn build_ui(app: &Application) -> Result<()> {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(&t("app_title"))
        .default_width(560)
        .default_height(780)
        .build();

    let header = adw::HeaderBar::builder()
        .build();

    let search_entry = gtk::SearchEntry::builder()
        .placeholder_text(&t("search_placeholder"))
        .build();
    header.set_title_widget(Some(&search_entry));

    let settings_btn = gtk::Button::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text(&t("settings"))
        .build();
    settings_btn.add_css_class("flat");
    header.pack_start(&settings_btn);

    let refresh_btn = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text(&t("reload"))
        .build();
    header.pack_end(&refresh_btn);

    let overlay = adw::ToastOverlay::new();
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);
    let store = gio::ListStore::new::<BoxedAnyObject>();
    let state = Rc::new(AppState::new(&window, &overlay, &store));

    let state_for_search = Rc::clone(&state);
    search_entry.connect_search_changed(move |entry| {
        let text = entry.text().to_string();
        *state_for_search.search_term.borrow_mut() = text;
        state_for_search.repopulate_store();
    });

    let state_for_settings_btn = Rc::clone(&state);
    settings_btn.connect_clicked(move |_| {
        state_for_settings_btn.show_settings_dialog();
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

    let add_btn = gtk::Button::with_label(&t("add"));
    add_btn.add_css_class("suggested-action");
    new_row.append(&add_btn);

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
    content.append(&overlay);

    // Füge die neue Eingabezeile unter den Filtereinstellungen ein
    content.append(&new_row);

    let list_view = create_list_view(&state);
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&list_view)
        .vexpand(true)
        .hexpand(true)
        .build();
    overlay.set_child(Some(&scrolled));

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&content));

    window.set_content(Some(&toolbar_view));

    // Setze Fokus direkt ins neue Eingabefeld beim Start
    new_entry.grab_focus();

    // Wenn das Fenster den Fokus erhält, setze den Cursor in das Eingabefeld
    let new_entry_for_focus = new_entry.clone();
    window.connect_notify_local(Some("is-active"), move |window, _| {
        if window.is_active() {
            new_entry_for_focus.grab_focus();
        }
    });

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
        state_for_settings_action.show_settings_dialog();
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
        state.show_error(&format!("{}\n{}", t("load_error").replace("{}", &err.to_string()), t("select_valid_file")));
        state.show_settings_dialog();
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

    let model = gtk::NoSelection::new(Some(state.store()));
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
    sort_mode: RefCell<SortMode>,
    window: glib::WeakRef<adw::ApplicationWindow>,
    preferences: RefCell<Preferences>,
    search_term: RefCell<String>,
}

impl AppState {
    fn new(window: &adw::ApplicationWindow, overlay: &adw::ToastOverlay, store: &gio::ListStore) -> Self {
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
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            let default_path = std::path::PathBuf::from(home).join("TodosDatenbank.md");
            
            if current_at_start != default_path {
                // Command line argument was used
                prefs.db_path = Some(current_at_start.to_string_lossy().into_owned());
            } else if let Some(db_path) = prefs.db_path.clone() {
                // No command line argument, use saved preference
                data::set_todo_path(PathBuf::from(db_path));
            } else {
                // No command line and no preference, use default
                prefs.db_path = Some(current_at_start.to_string_lossy().into_owned());
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

    fn reload(&self) -> Result<()> {
        let items = data::load_todos()?;
        *self.cached_items.borrow_mut() = items;
        self.repopulate_store();
        Ok(())
    }

    fn toggle_item(&self, todo: &TodoItem, done: bool) -> Result<()> {
        data::toggle_todo(&todo.key, done)?;
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

    fn choose_database_file(
        self: &Rc<Self>,
        parent: &impl IsA<gtk::Window>,
        on_success: Option<Rc<dyn Fn(PathBuf)>>,
    ) {
        let mut builder = FileDialog::builder()
            .title("Datenbankdatei auswählen")
            .modal(true);

        if let Some(folder) = data::todo_path().parent() {
            let folder_file = gio::File::for_path(folder);
            builder = builder.initial_folder(&folder_file);
        }

        if let Some(name) = data::todo_path().file_name().and_then(|n| n.to_str()) {
            builder = builder.initial_name(name);
        }

        let markdown_filter = FileFilter::new();
        markdown_filter.set_name(Some("Markdown-Dateien"));
        markdown_filter.add_pattern("*.md");
        let filters = gio::ListStore::new::<FileFilter>();
        filters.append(&markdown_filter);
        let builder = builder
            .filters(&filters)
            .default_filter(&markdown_filter)
            .accept_label("Auswählen");

        let dialog = builder.build();
        let callback = on_success.clone();

        dialog.open(
            Some(parent),
            Option::<&gio::Cancellable>::None,
            clone!(@strong self as state => move |result| {
                match result {
                    Ok(file) => {
                        if let Some(path) = file.path() {
                            let path_buf = path;
                            if state.set_database_path(path_buf.clone()) {
                                if let Some(cb) = callback.clone() {
                                    cb(path_buf);
                                }
                            }
                        }
                    }
                    Err(err) => {
                        if err.matches(gio::IOErrorEnum::Cancelled) {
                            return;
                        }
                        state.show_error(&format!("Datei konnte nicht gewählt werden: {err}"));
                    }
                }
            }),
        );
    }

    fn show_settings_dialog(self: &Rc<Self>) {
        let Some(parent) = self.window.upgrade() else {
            self.show_error(&t("no_window"));
            return;
        };

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
            .title(&t("show_due_only_settings"))
            .active(self.show_due_only())
            .build();
        show_due_row.add_prefix(&gtk::Image::from_icon_name("appointment-soon-symbolic"));
        let state_due = Rc::clone(self);
        show_due_row.connect_active_notify(move |row| {
            state_due.set_show_due_only(row.is_active());
        });
        general_group.add(&show_due_row);

        // --- Local File Page ---
        let local_page = adw::PreferencesPage::builder()
            .title(&t("local_file"))
            .icon_name("drive-harddisk-symbolic")
            .build();
        dialog.add(&local_page);

        let local_group = adw::PreferencesGroup::builder()
            .title(&t("local_file"))
            .build();
        local_page.add(&local_group);

        let use_local_row = adw::SwitchRow::builder()
            .title(&t("use_this_method"))
            .build();
        let (use_webdav, wd_url, wd_path, wd_user, wd_pass) = self.get_webdav_prefs();
        use_local_row.set_active(!use_webdav);
        local_group.add(&use_local_row);

        let file_row = adw::ActionRow::builder()
            .title(&t("database_file"))
            .subtitle(&data::todo_path().display().to_string())
            .build();
        file_row.add_prefix(&gtk::Image::from_icon_name("document-open-symbolic"));
        let select_button = gtk::Button::builder()
            .label(&t("select_file"))
            .valign(gtk::Align::Center)
            .build();
        select_button.add_css_class("flat");
        file_row.add_suffix(&select_button);
        
        let state_file = Rc::clone(self);
        let dialog_weak = dialog.downgrade();
        let file_row_weak = file_row.downgrade();
        select_button.connect_clicked(move |_| {
            let Some(win) = dialog_weak.upgrade() else { return; };
            let row_weak = file_row_weak.clone();
            state_file.choose_database_file(&win, Some(Rc::new(move |path| {
                if let Some(row) = row_weak.upgrade() {
                    row.set_subtitle(&path.display().to_string());
                }
            })));
        });
        local_group.add(&file_row);

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

        let use_webdav_row = adw::SwitchRow::builder()
            .title(&t("use_this_method"))
            .active(use_webdav)
            .build();
        webdav_group.add(&use_webdav_row);

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

        // --- Sync Toggles ---
        let use_local_row_for_webdav = use_local_row.clone();
        let use_webdav_row_for_local = use_webdav_row.clone();
        let state_local = Rc::clone(self);
        use_local_row.connect_active_notify(move |row| {
            let active = row.is_active();
            if active == use_webdav_row_for_local.is_active() {
                use_webdav_row_for_local.set_active(!active);
                state_local.set_use_webdav(!active);
            }
        });

        let use_local_row_for_webdav2 = use_local_row_for_webdav.clone();
        let state_webdav = Rc::clone(self);
        use_webdav_row.connect_active_notify(move |row| {
            let active = row.is_active();
            if active == use_local_row_for_webdav2.is_active() {
                use_local_row_for_webdav2.set_active(!active);
                state_webdav.set_use_webdav(active);
            }
        });

        dialog.present();
    }

    fn set_database_path(self: &Rc<Self>, path: PathBuf) -> bool {
        if !path.exists() {
            self.show_error(&t("file_not_exists"));
            return false;
        }

        let canonical = match path.canonicalize() {
            Ok(p) => p,
            Err(err) => {
                self.show_error(&t("open_path_error").replace("{}", &err.to_string()));
                return false;
            }
        };

        let previous = data::todo_path();
        if previous == canonical {
            self.show_info(&t("file_already_active"));
            return false;
        }

        data::set_todo_path(canonical.clone());
        if let Err(err) = self.reload() {
            data::set_todo_path(previous);
            if let Err(rollback_err) = self.reload() {
                eprintln!("{}", t("rollback_failed").replace("{}", &rollback_err.to_string()));
            }
            self.show_error(&t("load_error").replace("{}", &err.to_string()));
            return false;
        }

        {
            let mut prefs = self.preferences.borrow_mut();
            prefs.db_path = Some(canonical.to_string_lossy().into_owned());
        }
        self.persist_preferences();

        if let Some(old_monitor) = self.monitor.borrow_mut().take() {
            drop(old_monitor);
        }
        if let Err(err) = self.install_monitor() {
            self.show_error(&t("monitor_error").replace("{}", &err.to_string()));
        }

        self.show_info(&t("using_file").replace("{}", &canonical.display().to_string()));
        true
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
        let close_with_comment_btn = gtk::Button::with_label(&t("close_with_comment"));
        let save_btn = gtk::Button::with_label(&t("save"));
        save_btn.add_css_class("suggested-action");
        buttons.append(&cancel_btn);
        buttons.append(&close_with_comment_btn);
        buttons.append(&save_btn);
        content.append(&buttons);
        dialog.set_content(Some(&content));

        let dialog_cancel = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dialog_cancel.close();
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
        save_btn.connect_clicked(move |_| {
            let title_text = title_entry_save.text().trim().to_string();
            if title_text.is_empty() {
                state_for_save.show_error(&t("title_empty_error"));
                return;
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

            let mut updated = base_item.clone();
            updated.title = title_text;
            updated.project = project_value;
            updated.context = context_value;
            updated.reference = base_item.reference.clone();
            updated.due = due_value;
            updated.done = done_check_save.is_active();

            if let Err(err) = state_for_save.save_item(&updated) {
                state_for_save.show_error(&t("save_task_error").replace("{}", &err.to_string()));
            } else {
                dialog_save.close();
            }
        });

        let dialog_close_comment = dialog.clone();
        let state_for_close_comment = Rc::clone(self);
        let base_item_close = todo.clone();
        let title_entry_close = title_entry.clone();
        let project_entry_close = project_entry.clone();
        let context_entry_close = context_entry.clone();
        let due_entry_close = due_entry.clone();
        let comment_entry_close = comment_entry.clone();
        let comment_row_close = comment_row.clone();
        let close_btn_ref = close_with_comment_btn.clone();
        close_with_comment_btn.connect_clicked(move |_| {
            if !comment_row_close.is_visible() {
                comment_row_close.set_visible(true);
                comment_entry_close.grab_focus();
                close_btn_ref.add_css_class("suggested-action");
                return;
            }

            let comment = comment_entry_close.text().trim().to_string();
            if comment.is_empty() {
                return;
            }

            let title_text = title_entry_close.text().trim().to_string();
            let full_title = format!("{} ({})", title_text, comment);

            let project_text = project_entry_close.text().trim().to_string();
            let project_value = if project_text.is_empty() {
                None
            } else {
                Some(project_text)
            };

            let context_text = context_entry_close.text().trim().to_string();
            let context_value = if context_text.is_empty() {
                None
            } else {
                Some(context_text)
            };

            let due_text = due_entry_close.text().trim().to_string();
            let due_value = if due_text.is_empty() {
                None
            } else {
                match NaiveDate::parse_from_str(&due_text, "%Y-%m-%d") {
                    Ok(date) => Some(date),
                    Err(_) => {
                        state_for_close_comment.show_error(&t("invalid_date_error"));
                        return;
                    }
                }
            };

            let mut updated = base_item_close.clone();
            updated.title = full_title;
            updated.project = project_value;
            updated.context = context_value;
            updated.due = due_value;
            updated.done = true;

            if let Err(err) = state_for_close_comment.save_item(&updated) {
                state_for_close_comment.show_error(&t("save_task_error").replace("{}", &err.to_string()));
            } else {
                dialog_close_comment.close();
            }
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