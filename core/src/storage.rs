//! Unified storage abstraction for local files and WebDAV.

use std::fs;

use anyhow::{bail, Context, Result};

use crate::config::{get_backend_config, BackendConfig};
use crate::conflict::{ConflictError, ReadSnapshot};
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
                bail!(t("No database file configured. Please select or create one in the settings."));
            }
            fs::read_to_string(&path)
                .with_context(|| t("Could not read {}").replace("{}", &path.display().to_string()))
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
            .with_context(|| t("Could not write {}").replace("{}", &path.display().to_string())),
        BackendConfig::WebDav {
            url,
            path,
            username,
            password,
        } => write_content_webdav(&url, path, username, password, content, None),
    }
}

/// Read content together with its fingerprint (for conflict detection).
pub fn read_content_with_fingerprint() -> Result<ReadSnapshot> {
    let content = read_content()?;
    let fingerprint = get_fingerprint()?;
    Ok(ReadSnapshot {
        content,
        fingerprint,
    })
}

/// Write content only if the fingerprint has not changed since it was read.
/// Returns `ConflictError` (as `anyhow::Error`) on mismatch.
pub fn write_content_checked(content: String, expected_fingerprint: &str) -> Result<()> {
    let config = get_backend_config();
    match config {
        BackendConfig::Local(path) => {
            let current = get_fingerprint()?;
            if current != expected_fingerprint {
                return Err(ConflictError {
                    local_fingerprint: expected_fingerprint.to_string(),
                    remote_fingerprint: current,
                }
                .into());
            }
            fs::write(&path, content)
                .with_context(|| t("Could not write {}").replace("{}", &path.display().to_string()))
        }
        BackendConfig::WebDav {
            url,
            path,
            username,
            password,
        } => write_content_webdav(
            &url,
            path,
            username,
            password,
            content,
            Some(expected_fingerprint),
        ),
    }
}
