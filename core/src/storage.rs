//! Unified storage abstraction for local files and WebDAV.

use std::fs;

use anyhow::{bail, Context, Result};

use crate::config::{get_backend_config, BackendConfig};
use crate::conflict::{ConflictError, ReadSnapshot};
use crate::i18n::t;
use crate::webdav::{
    get_fingerprint_webdav, read_content_webdav, read_content_with_fingerprint_webdav,
    write_content_webdav,
};

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

/// Read content together with a fingerprint that provably describes *that*
/// content.
///
/// Reading the two independently is a lost-update bug: a write landing between
/// the content read and the fingerprint read yields stale content carrying the
/// fresh validator, so the `If-Match` guard on the way out passes and the other
/// writer's changes are overwritten without a conflict ever being reported.
pub fn read_content_with_fingerprint() -> Result<ReadSnapshot> {
    let config = get_backend_config();
    match config {
        BackendConfig::Local(path) => {
            if path.as_os_str().is_empty() {
                bail!(t("No database file configured. Please select or create one in the settings."));
            }
            // Read between two stats and retry while they disagree: the content
            // then belongs to the mtime we report.
            let describe = || t("Could not read {}").replace("{}", &path.display().to_string());
            for _ in 0..3 {
                let before = get_fingerprint().with_context(describe)?;
                let content = fs::read_to_string(&path).with_context(describe)?;
                let after = get_fingerprint().with_context(describe)?;
                if before == after {
                    return Ok(ReadSnapshot {
                        content,
                        fingerprint: after,
                    });
                }
            }
            bail!(t("The file keeps changing while it is being read. Please try again."));
        }
        BackendConfig::WebDav {
            url,
            path,
            username,
            password,
        } => {
            let (content, fingerprint) =
                read_content_with_fingerprint_webdav(&url, path, username, password)?;
            Ok(ReadSnapshot {
                content,
                fingerprint,
            })
        }
    }
}

/// Write `content` unconditionally, bypassing conflict detection.
///
/// Only for an explicit "overwrite" after the user was shown a conflict.
pub fn force_write_content(content: String) -> Result<()> {
    write_content(content)
}

/// Write content only if the fingerprint has not changed since it was read.
/// Returns `ConflictError` (as `anyhow::Error`) on mismatch.
pub fn write_content_checked(content: String, expected_fingerprint: &str) -> Result<()> {
    let config = get_backend_config();
    match config {
        BackendConfig::Local(path) => {
            let current = get_fingerprint()?;
            if current != expected_fingerprint {
                return Err(ConflictError::new(expected_fingerprint.to_string(), current)
                    .with_pending(content)
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
