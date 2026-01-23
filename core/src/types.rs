//! Core data types for todo items.

use chrono::{NaiveDateTime, NaiveTime};
use serde::Serialize;

/// Default time component for due dates (midnight).
pub const DEFAULT_DUE_TIME: NaiveTime = match NaiveTime::from_hms_opt(0, 0, 0) {
    Some(t) => t,
    None => panic!("midnight time unavailable"),
};

/// Unique identifier for a todo item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TodoKey {
    pub line_index: usize,
    pub marker: Option<String>,
}

/// A parsed todo item with all its metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TodoItem {
    pub key: TodoKey,
    pub title: String,
    pub projects: Vec<String>,
    pub contexts: Vec<String>,
    pub due: Option<NaiveDateTime>,
    pub reference: Option<String>,
    pub recurrence: Option<String>,
    pub note: Option<String>,
    pub done: bool,
}
