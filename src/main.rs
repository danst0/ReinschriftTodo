mod data;
mod ui;

use anyhow::{bail, Context, Result};
use adw::prelude::*;
use gtk::glib;

const APP_ID: &str = "me.dumke.ReinschriftTodo";

fn main() -> Result<()> {
    gtk::glib::set_application_name("Todos Datenbank");
    adw::init().context("Konnte libadwaita nicht initialisieren")?;

    let app = adw::Application::builder().application_id(APP_ID).build();

    app.connect_activate(|app| {
        if let Err(err) = ui::build_ui(app) {
            eprintln!("Fehler beim Aufbau der Oberfläche: {err:?}");
        }
    });

    let status = app.run();
    if status != glib::ExitCode::SUCCESS {
        bail!("Anwendung wurde mit Status {:?} beendet", status);
    }

    Ok(())
}
