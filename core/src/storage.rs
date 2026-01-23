//! Unified storage abstraction for local files and WebDAV.

use std::fs;

use anyhow::{bail, Context, Result};

use crate::config::{get_backend_config, BackendConfig};
use crate::i18n::t;
use crate::webdav::{get_fingerprint_webdav, read_content_webdav, write_content_webdav};

/// Get a fingerprint (for change detection) from the configured backend.
pub fn get_fingerprint() -> Result<String> {
    let config = get_backend_config();
    match config {
        BackendConfig::Local(path) => {
            let metadata = fs::metadata(&path)?;
            let mtime = metadata.modified()?;
            Ok(format!("{:?}", mtime))
        }
        BackendConfig::WebDav {
            url,
            path,
            username,
            password,
        } => get_fingerprint_webdav(&url, path.as_deref(), username.as_deref(), password.as_deref()),
    }
}

/// Read content from the configured backend.
pub fn read_content() -> Result<String> {
    let config = get_backend_config();
    match config {
        BackendConfig::Local(path) => {
            if path.as_os_str().is_empty() {
                bail!(t("no_database_configured"));
            }
            fs::read_to_string(&path)
                .with_context(|| t("read_error").replace("{}", &path.display().to_string()))
        }
        BackendConfig::WebDav {
            url,
            path,
            username,
            password,
        } => read_content_webdav(&url, path, username, password),
    }
}

/// Write content to the configured backend.
pub fn write_content(content: String) -> Result<()> {
    let config = get_backend_config();
    match config {
        BackendConfig::Local(path) => fs::write(&path, content)
            .with_context(|| t("write_error").replace("{}", &path.display().to_string())),
        BackendConfig::WebDav {
            url,
            path,
            username,
            password,
        } => write_content_webdav(&url, path, username, password, content),
    }
}
