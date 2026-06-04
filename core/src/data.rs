//! Data module - re-exports from new modular structure for backward compatibility.
//!
//! This module maintains the original API surface by re-exporting
//! from the new focused modules:
//! - `types`: TodoItem, TodoKey, DEFAULT_DUE_TIME
//! - `config`: BackendConfig, path management
//! - `storage`: read/write/fingerprint
//! - `webdav`: WebDAV client operations
//! - `parser`: parsing logic
//! - `renderer`: rendering logic
//! - `todo`: business logic
//! - `util`: helpers

// Re-export types
pub use crate::types::{TodoItem, TodoKey, DEFAULT_DUE_TIME};

// Re-export config
pub use crate::config::{
    default_todo_path, get_backend_config, set_backend_config, set_todo_path, todo_path,
    BackendConfig,
};

// Re-export storage
pub use crate::storage::get_fingerprint;

// Re-export webdav
pub use crate::webdav::{initiate_nextcloud_login, poll_nextcloud_login, test_webdav_connection};

// Re-export parser functions (used by tests in lib.rs)
#[allow(unused_imports)]
pub(crate) use crate::parser::{extract_title, parse_line};

// Re-export renderer functions (used by tests in lib.rs)
#[allow(unused_imports)]
pub(crate) use crate::renderer::{render_line, rewrite_due, rewrite_line};

// Re-export todo business logic
pub use crate::todo::{
    add_todo, add_todo_full, delete_todo, load_todos, next_due_date, set_due_sometime,
    set_due_today, set_due_tomorrow, set_due_weekend, set_myday_today, toggle_todo, undo,
    unique_titles_by_frequency, unset_myday, update_todo_details,
};

// Re-export conflict types
pub use crate::conflict::ConflictError;

// Re-export undo helpers
pub use crate::undo::can_undo;

// Re-export reminders
pub use crate::reminders::{get_due_reminders, overdue_summary};

// Re-export util (used by tests in lib.rs)
#[allow(unused_imports)]
pub(crate) use crate::util::encode_base36;
