//! Configuration management for backend storage.

use std::env;
use std::path::PathBuf;
use std::sync::Mutex;

use once_cell::sync::Lazy;

/// Backend storage configuration.
#[derive(Clone, Debug)]
pub enum BackendConfig {
    Local(PathBuf),
    WebDav {
        url: String,
        path: Option<String>,
        username: Option<String>,
        password: Option<String>,
    },
}

static BACKEND_CONFIG: Lazy<Mutex<BackendConfig>> = Lazy::new(|| {
    let configured = default_todo_path();
    Mutex::new(BackendConfig::Local(configured))
});

/// Set the backend configuration.
pub fn set_backend_config(config: BackendConfig) {
    if let Ok(mut c) = BACKEND_CONFIG.lock() {
        *c = config;
    }
}

/// Get the current backend configuration.
pub fn get_backend_config() -> BackendConfig {
    BACKEND_CONFIG.lock().unwrap().clone()
}

static TODO_PATH: Lazy<Mutex<PathBuf>> = Lazy::new(|| Mutex::new(default_todo_path()));

/// Get the default todo path from environment or empty.
pub fn default_todo_path() -> PathBuf {
    env::var("TODOS_DB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::new())
}

/// Get the current todo file path.
pub fn todo_path() -> PathBuf {
    TODO_PATH.lock().expect("todo path lock poisoned").clone()
}

/// Set the todo file path and update backend config.
pub fn set_todo_path(new_path: PathBuf) {
    if let Ok(mut path) = TODO_PATH.lock() {
        *path = new_path.clone();
    }
    set_backend_config(BackendConfig::Local(new_path));
}

/// A stable name for the database a configuration points at.
///
/// Used to tie queued writes to their database: a write recorded against one
/// file must never be replayed into another. For WebDAV only the host, the
/// path and the user count — the base URL itself can change at runtime when
/// the Nextcloud fallback kicks in, and still means the same file.
pub fn backend_identity(config: &BackendConfig) -> String {
    match config {
        BackendConfig::Local(path) => format!("file:{}", path.display()),
        BackendConfig::WebDav {
            url,
            path,
            username,
            ..
        } => {
            let host = reqwest::Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_else(|| url.clone());
            format!(
                "webdav:{}|{}|{}",
                host,
                path.as_deref().unwrap_or("").trim_matches('/'),
                username.as_deref().unwrap_or("")
            )
        }
    }
}
