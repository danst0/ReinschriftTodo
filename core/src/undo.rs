//! Undo stack for reverting destructive todo operations.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{LazyLock, Mutex};

const MAX_UNDO_DEPTH: usize = 10;

/// A snapshot of file content before a mutation.
#[derive(Debug, Clone)]
pub struct UndoEntry {
    pub content: String,
    pub description: String,
    /// Hash of the content this mutation produced, i.e. the state the undo
    /// expects to find. If the file no longer hashes to this, somebody else
    /// wrote in the meantime and restoring `content` would silently discard
    /// their work.
    pub expected_hash: u64,
}

/// Hash file content for change detection (not for security).
pub fn content_hash(content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

/// Push a snapshot onto the undo stack.
///
/// Call this only *after* the mutation was written: a failed write leaves no
/// entry, so "Undo" can never restore a state that never existed remotely.
pub fn push_undo(content: String, description: String, expected_hash: u64) {
    push_entry(UndoEntry {
        content,
        description,
        expected_hash,
    });
}

/// Push a prepared entry (used to put one back when its undo was refused).
pub fn push_entry(entry: UndoEntry) {
    if let Ok(mut stack) = UNDO_STACK.lock() {
        if stack.len() >= MAX_UNDO_DEPTH {
            stack.remove(0);
        }
        stack.push(entry);
    }
}

static UNDO_STACK: LazyLock<Mutex<Vec<UndoEntry>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Pop the most recent snapshot from the undo stack.
pub fn pop_undo() -> Option<UndoEntry> {
    UNDO_STACK.lock().ok()?.pop()
}

/// Check whether the undo stack has any entries.
pub fn can_undo() -> bool {
    UNDO_STACK
        .lock()
        .ok()
        .map(|s| !s.is_empty())
        .unwrap_or(false)
}
