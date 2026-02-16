//! Conflict detection types for concurrent file access.

use std::fmt;

/// Bundles file content with its fingerprint for optimistic concurrency.
#[derive(Debug, Clone)]
pub struct ReadSnapshot {
    pub content: String,
    pub fingerprint: String,
}

/// Error returned when a write is attempted against a stale fingerprint.
#[derive(Debug, Clone)]
pub struct ConflictError {
    pub local_fingerprint: String,
    pub remote_fingerprint: String,
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
