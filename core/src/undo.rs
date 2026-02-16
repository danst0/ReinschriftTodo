//! Undo stack for reverting destructive todo operations.

use std::sync::{LazyLock, Mutex};

const MAX_UNDO_DEPTH: usize = 10;

/// A snapshot of file content before a mutation.
#[derive(Debug, Clone)]
pub struct UndoEntry {
    pub content: String,
    pub description: String,
}

static UNDO_STACK: LazyLock<Mutex<Vec<UndoEntry>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Push a snapshot onto the undo stack (before applying a mutation).
pub fn push_undo(content: String, description: String) {
    if let Ok(mut stack) = UNDO_STACK.lock() {
        if stack.len() >= MAX_UNDO_DEPTH {
            stack.remove(0);
        }
        stack.push(UndoEntry {
            content,
            description,
        });
    }
}

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
