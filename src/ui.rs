use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use adw::{self, Application};
use anyhow::Result;
use glib::{clone, BoxedAnyObject};
use gtk::gio;
use gtk::gio::prelude::*;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;

use crate::data::{self, TodoItem};

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
    let store = gio::ListStore::new::<BoxedAnyObject>();
    let state = Rc::new(AppState::new(&overlay, &store));

    let list_view = create_list_view(&state);
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&list_view)
        .vexpand(true)
        .hexpand(true)
        .build();
    overlay.set_child(Some(&scrolled));

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&overlay));

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
    if let Err(err) = state.install_monitor() {
        state.show_error(&format!("Dateiüberwachung nicht verfügbar: {err}"));
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
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 12);
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
        list_item.set_child(Some(&container));

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
            let todo = todo_obj.borrow::<TodoItem>().clone();
            if btn.is_active() == todo.done {
                return;
            }

            if let Some(state) = state_for_handler.upgrade() {
                if let Err(err) = state.toggle_item(&todo, btn.is_active()) {
                    state.show_error(&format!("Konnte Eintrag nicht aktualisieren: {err}"));
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
        let todo = todo_obj.borrow::<TodoItem>();

        if let Some(root) = list_item
            .child()
            .and_then(|child: gtk::Widget| child.downcast::<gtk::Box>().ok())
        {
            if let Some(check_widget) = root
                .first_child()
                .and_then(|w: gtk::Widget| w.downcast::<gtk::CheckButton>().ok())
            {
                if check_widget.is_active() != todo.done {
                    check_widget.set_active(todo.done);
                }
            }
            if let Some(column_widget) = root
                .last_child()
                .and_then(|w: gtk::Widget| w.downcast::<gtk::Box>().ok())
            {
                if let Some(title_widget) = column_widget
                    .first_child()
                    .and_then(|w: gtk::Widget| w.downcast::<gtk::Label>().ok())
                {
                    title_widget.set_text(&todo.title);
                    if todo.done {
                        title_widget.add_css_class("dim-label");
                    } else {
                        title_widget.remove_css_class("dim-label");
                    }
                }
                if let Some(meta_widget) = column_widget
                    .last_child()
                    .and_then(|w: gtk::Widget| w.downcast::<gtk::Label>().ok())
                {
                    meta_widget.set_text(&format_metadata(&todo));
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
}

impl AppState {
    fn new(overlay: &adw::ToastOverlay, store: &gio::ListStore) -> Self {
        Self {
            store: store.clone(),
            overlay: overlay.clone(),
            monitor: RefCell::new(None),
        }
    }

    fn store(&self) -> gio::ListStore {
        self.store.clone()
    }

    fn reload(&self) -> Result<()> {
        let items = data::load_todos()?;
        self.store.remove_all();
        for item in items {
            self.store.append(&BoxedAnyObject::new(item));
        }
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