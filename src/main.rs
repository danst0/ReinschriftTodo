mod data;
mod ui;
mod i18n;

use anyhow::{bail, Context, Result};
use adw::prelude::*;
use gtk::glib;
use i18n::t;

const APP_ID: &str = "me.dumke.Reinschrift";

fn main() -> Result<()> {
    let mut filtered_args: Vec<String> = std::env::args().collect();
    if let Some(pos) = filtered_args.iter().position(|x| x == "--database") {
        filtered_args.remove(pos);
        if pos < filtered_args.len() {
            let db_path = filtered_args.remove(pos);
            let absolute_path = std::fs::canonicalize(&db_path).unwrap_or_else(|_| std::path::PathBuf::from(db_path));
            data::set_todo_path(absolute_path);
        }
    }

    if let Some(pos) = filtered_args.iter().position(|x| x == "--language") {
        filtered_args.remove(pos);
        if pos < filtered_args.len() {
            let lang = filtered_args.remove(pos);
            i18n::set_language(lang);
        }
    }

    gtk::glib::set_application_name(&t("app_title"));
    adw::init().context(t("init_adw_error"))?;

    let app = adw::Application::builder().application_id(APP_ID).build();

    app.connect_activate(|app| {
        if let Err(err) = ui::build_ui(app, false) {
            eprintln!("{}: {err:?}", t("build_ui_error"));
        }
    });

    let status = app.run_with_args(&filtered_args);
    if status != glib::ExitCode::SUCCESS {
        bail!("{}: {:?}", t("app_exit_status"), status);
    }

    Ok(())
}
