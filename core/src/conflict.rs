//! Conflict detection types for concurrent file access.

use std::fmt;

/// Bundles file content with its fingerprint for optimistic concurrency.
#[derive(Debug, Clone)]
pub struct ReadSnapshot {
    pub content: String,
    pub fingerprint: String,
}

/// Error returned when a write is attempted against a stale fingerprint.
///
/// `pending_content` carries the content the caller wanted to write. It lets a
/// UI offer "overwrite" without recomputing the edit — without it, the only
/// honest options are reload and discard.
#[derive(Debug, Clone)]
pub struct ConflictError {
    pub local_fingerprint: String,
    pub remote_fingerprint: String,
    pub pending_content: Option<String>,
}

impl ConflictError {
    /// Create a conflict error without a pending write.
    pub fn new(local_fingerprint: String, remote_fingerprint: String) -> Self {
        Self {
            local_fingerprint,
            remote_fingerprint,
            pending_content: None,
        }
    }

    /// Attach the content whose write was rejected.
    pub fn with_pending(mut self, content: String) -> Self {
        self.pending_content = Some(content);
        self
    }
}

impl fmt::Display for ConflictError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Conflict detected: local fingerprint '{}' does not match remote '{}'",
            self.local_fingerprint, self.remote_fingerprint
        )
    }
}

impl std::error::Error for ConflictError {}

/// Attach `content` to a conflict error travelling as `anyhow::Error`.
/// Non-conflict errors pass through untouched.
pub fn attach_pending(err: anyhow::Error, content: String) -> anyhow::Error {
    match err.downcast_ref::<ConflictError>() {
        Some(conflict) => conflict.clone().with_pending(content).into(),
        None => err,
    }
}
