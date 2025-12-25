mod data;
mod ui;
mod i18n;

use anyhow::{bail, Context, Result};
use adw::prelude::*;
use gtk::glib;
use i18n::t;

const APP_ID: &str = "me.dumke.Reinschrift";

fn main() -> Result<()> {
    gtk::glib::set_application_name(&t("app_title"));
    adw::init().context(t("init_adw_error"))?;

    let app = adw::Application::builder().application_id(APP_ID).build();

    app.connect_activate(|app| {
        if let Err(err) = ui::build_ui(app) {
            eprintln!("{}: {err:?}", t("build_ui_error"));
        }
    });

    let status = app.run();
    if status != glib::ExitCode::SUCCESS {
        bail!("{}: {:?}", t("app_exit_status"), status);
    }

    Ok(())
}
