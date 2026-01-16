mod commands;
mod output;

use anyhow::Result;
use clap::{Parser, Subcommand};
use reinschrift_core::{data, i18n, load_preferences, set_todo_path};

#[derive(Parser)]
#[command(name = "reinschrift")]
#[command(author, version, about = "A todo application with markdown storage")]
struct Cli {
    /// Language (de, en, es, fr, ja, sv)
    #[arg(short, long, global = true)]
    language: Option<String>,

    /// Output in JSON format for scripting
    #[arg(short, long, global = true)]
    json: bool,

    /// Disable colored output
    #[arg(long, global = true)]
    no_color: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List todos with filtering and sorting
    #[command(alias = "ls")]
    List {
        /// Sort mode: topic, location, or date
        #[arg(short, long, default_value = "topic")]
        sort: String,

        /// Show all todos (including completed)
        #[arg(short, long)]
        all: bool,

        /// Show only todos that are due (until today)
        #[arg(long)]
        due_only: bool,

        /// Filter by project
        #[arg(short, long)]
        project: Option<String>,

        /// Filter by context
        #[arg(short, long)]
        context: Option<String>,
    },

    /// Add a new todo
    Add {
        /// Todo title (can include +project @context due:DATE)
        title: String,

        /// Project (+project)
        #[arg(short, long)]
        project: Option<String>,

        /// Context (@context)
        #[arg(short, long)]
        context: Option<String>,

        /// Due date (YYYY-MM-DD)
        #[arg(short = 'd', long)]
        due: Option<String>,

        /// Recurrence (daily, weekly, monthly)
        #[arg(short, long)]
        recurrence: Option<String>,

        /// Note text
        #[arg(short, long)]
        note: Option<String>,
    },

    /// Edit an existing todo
    Edit {
        /// Todo ID (^marker or #line_number)
        id: String,

        /// New title
        #[arg(short, long)]
        title: Option<String>,

        /// Project (+project)
        #[arg(short, long)]
        project: Option<String>,

        /// Context (@context)
        #[arg(short, long)]
        context: Option<String>,

        /// Due date (YYYY-MM-DD)
        #[arg(short = 'd', long)]
        due: Option<String>,

        /// Recurrence (daily, weekly, monthly)
        #[arg(short, long)]
        recurrence: Option<String>,

        /// Note text
        #[arg(short, long)]
        note: Option<String>,
    },

    /// Delete todo(s)
    #[command(alias = "rm")]
    Delete {
        /// Todo IDs (^marker or #line_number)
        ids: Vec<String>,

        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
    },

    /// Mark todo(s) as completed
    Done {
        /// Todo IDs (^marker or #line_number)
        ids: Vec<String>,
    },

    /// Mark todo(s) as incomplete
    Undone {
        /// Todo IDs (^marker or #line_number)
        ids: Vec<String>,
    },

    /// Set due date to today
    Today {
        /// Todo ID (^marker or #line_number)
        id: String,
    },

    /// Set due date to tomorrow
    Tomorrow {
        /// Todo ID (^marker or #line_number)
        id: String,
    },

    /// Search todos
    Search {
        /// Search query
        query: String,

        /// Search all todos (including completed)
        #[arg(short, long)]
        all: bool,
    },

    /// Configuration management
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show current configuration
    Show,

    /// Test WebDAV connection
    WebdavTest,

    /// Set a configuration value
    Set {
        /// Configuration key
        key: String,

        /// Configuration value
        value: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load shared preferences (same file as GUI)
    let prefs = load_preferences();

    // Configure backend from preferences
    if prefs.use_webdav {
        if let Some(url) = prefs.webdav_url.clone() {
            data::set_backend_config(data::BackendConfig::WebDav {
                url,
                path: prefs.webdav_path.clone(),
                username: prefs.webdav_username.clone(),
                password: prefs.webdav_password.clone(),
            });
        }
    } else if let Some(db_path) = prefs.db_path.clone() {
        set_todo_path(db_path.into());
    }

    // Set language if provided
    if let Some(lang) = cli.language {
        i18n::set_language(lang);
    }

    // Create output context
    let ctx = output::OutputContext::new(cli.json, !cli.no_color);

    // Dispatch commands
    match cli.command {
        Commands::List { sort, all, due_only, project, context } => {
            commands::list(&ctx, &sort, all, due_only, project.as_deref(), context.as_deref())
        }
        Commands::Add { title, project, context, due, recurrence, note } => {
            commands::add(&ctx, &title, project.as_deref(), context.as_deref(), due.as_deref(), recurrence.as_deref(), note.as_deref())
        }
        Commands::Edit { id, title, project, context, due, recurrence, note } => {
            commands::edit(&ctx, &id, title.as_deref(), project.as_deref(), context.as_deref(), due.as_deref(), recurrence.as_deref(), note.as_deref())
        }
        Commands::Delete { ids, force } => {
            commands::delete(&ctx, &ids, force)
        }
        Commands::Done { ids } => {
            commands::done(&ctx, &ids)
        }
        Commands::Undone { ids } => {
            commands::undone(&ctx, &ids)
        }
        Commands::Today { id } => {
            commands::today(&ctx, &id)
        }
        Commands::Tomorrow { id } => {
            commands::tomorrow(&ctx, &id)
        }
        Commands::Search { query, all } => {
            commands::search(&ctx, &query, all)
        }
        Commands::Config { action } => {
            match action {
                ConfigAction::Show => commands::config_show(&ctx),
                ConfigAction::WebdavTest => commands::config_webdav_test(&ctx),
                ConfigAction::Set { key, value } => commands::config_set(&ctx, &key, &value),
            }
        }
    }
}
