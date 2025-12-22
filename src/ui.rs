use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

use adw::prelude::*;
use adw::{self, Application};
use anyhow::Result;
use chrono::NaiveDate;
use glib::{clone, BoxedAnyObject};
use gtk::gio;
use gtk::gio::prelude::*;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;

use crate::data::{self, TodoItem};

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
}

pub fn build_ui(app: &Application) -> Result<()> {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Todos Datenbank")
        .default_width(560)
        .default_height(780)
        .build();

    let header = adw::HeaderBar::builder()
        .title_widget(&gtk::Label::builder().label("Todos Datenbank").xalign(0.0).build())
        .build();

    let refresh_btn = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text("Neu laden (Ctrl+R)")
        .build();
    header.pack_end(&refresh_btn);

    let overlay = adw::ToastOverlay::new();
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);
    let store = gio::ListStore::new::<BoxedAnyObject>();
    let state = Rc::new(AppState::new(&overlay, &store));

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    controls.set_margin_start(12);
    controls.set_margin_end(12);
    controls.set_margin_top(6);
    controls.set_margin_bottom(6);

    let sort_label = gtk::Label::builder()
        .label("Sortieren nach:")
        .xalign(0.0)
        .build();
    controls.append(&sort_label);

    let sort_selector = gtk::DropDown::from_strings(&["+ Themen", "@ Orte", "Datum"]);
    sort_selector.set_selected(state.sort_mode().to_index());
    controls.append(&sort_selector);

    let list_view = create_list_view(&state);
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&list_view)
        .vexpand(true)
        .hexpand(true)
        .build();
    overlay.set_child(Some(&scrolled));

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&controls);
    content.append(&overlay);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&content));

    window.set_content(Some(&toolbar_view));

    let refresh_action = gio::SimpleAction::new("reload", None);
    refresh_action.connect_activate(clone!(@weak state => move |_, _| {
        if let Err(err) = state.reload() {
            state.show_error(&format!("Konnte To-dos nicht laden: {err}"));
        }
    }));
    app.add_action(&refresh_action);
    app.set_accels_for_action("app.reload", &["<Primary>r"]);

    refresh_btn.connect_clicked(clone!(@weak app => move |_| {
        let _ = app.activate_action("app.reload", None);
    }));

    state.reload()?;
    sort_selector.connect_selected_notify(clone!(@weak state => move |dropdown| {
        let mode = SortMode::from_index(dropdown.selected());
        state.set_sort_mode(mode);
    }));

    if let Err(err) = state.install_monitor() {
        state.show_error(&format!("Dateiüberwachung nicht verfügbar: {err}"));
    }

    // Keep state alive for the window lifetime so weak references can upgrade.
    unsafe {
        window.set_data("app-state", state.clone());
    }

    window.present();

    Ok(())
}

fn create_list_view(state: &Rc<AppState>) -> gtk::ListView {
    let factory = gtk::SignalListItemFactory::new();
    let state_weak = Rc::downgrade(state);

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

        let postpone_btn = gtk::Button::builder()
            .icon_name("go-next-symbolic")
            .tooltip_text("Auf morgen verschieben")
            .build();
        postpone_btn.set_valign(gtk::Align::Center);
        postpone_btn.add_css_class("flat");
        container.append(&postpone_btn);

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
        let state_for_handler = state_weak.clone();
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
                    state.show_error(&format!("Konnte Eintrag nicht aktualisieren: {err}"));
                }
            }
        });

        let postpone_list = list_item.downgrade();
        let postpone_state = state_weak.clone();
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
                if let Err(err) = state.postpone_item(&todo) {
                    state.show_error(&format!("Konnte verschieben: {err}"));
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
    gtk::ListView::new(Some(model), Some(factory))
}

struct AppState {
    store: gio::ListStore,
    overlay: adw::ToastOverlay,
    monitor: RefCell<Option<gio::FileMonitor>>,
    cached_items: RefCell<Vec<TodoItem>>,
    sort_mode: RefCell<SortMode>,
}

impl AppState {
    fn new(overlay: &adw::ToastOverlay, store: &gio::ListStore) -> Self {
        Self {
            store: store.clone(),
            overlay: overlay.clone(),
            monitor: RefCell::new(None),
            cached_items: RefCell::new(Vec::new()),
            sort_mode: RefCell::new(SortMode::Topic),
        }
    }

    fn store(&self) -> gio::ListStore {
        self.store.clone()
    }

    fn sort_mode(&self) -> SortMode {
        *self.sort_mode.borrow()
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

    fn postpone_item(&self, todo: &TodoItem) -> Result<()> {
        let new_due = data::postpone_to_tomorrow(&todo.key)?;
        self.reload()?;
        self.show_info(&format!("Verschoben auf {}", new_due));
        Ok(())
    }

    fn set_sort_mode(&self, mode: SortMode) {
        {
            let mut current = self.sort_mode.borrow_mut();
            if *current == mode {
                return;
            }
            *current = mode;
        }

        self.repopulate_store();
    }

    fn repopulate_store(&self) {
        let mut items = self.cached_items.borrow().clone();
        self.sort_items(&mut items);
        self.store.remove_all();
        let mode = *self.sort_mode.borrow();
        let mut last_group: Option<String> = None;
        for item in items.into_iter().filter(|todo| !todo.done) {
            if let Some(label) = self.group_label(mode, &item) {
                if last_group.as_ref() != Some(&label) {
                    self.store
                        .append(&BoxedAnyObject::new(ListEntry::Header(label.clone())));
                    last_group = Some(label);
                }
            }
            self.store.append(&BoxedAnyObject::new(ListEntry::Item(item)));
        }
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
            SortMode::Topic => Some(format!(
                "Thema: {}",
                item.project
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Ohne Projekt")
            )),
            SortMode::Location => Some(format!(
                "Ort: {}",
                item.context
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Ohne Ort")
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
        monitor.connect_changed(clone!(@weak self as state => move |_, _, _, _| {
            if let Err(err) = state.reload() {
                state.show_error(&format!("Aktualisierung fehlgeschlagen: {err}"));
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
        parts.push(format!("Fällig: {}", due));
    }
    if let Some(reference) = &item.reference {
        parts.push(format!("↗ {}", reference));
    }

    parts.join(" • ")
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