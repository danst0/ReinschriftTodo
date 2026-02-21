//! Parsing logic for todo items from markdown lines.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use once_cell::sync::Lazy;
use regex::Regex;

use crate::types::{TodoItem, TodoKey, DEFAULT_DUE_TIME};
use crate::util::{normalize_note, unescape_note, TITLE_MARKERS};

// Regex patterns for parsing todo fields
pub static LINK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[\[([^\]]+)\]\]").unwrap());
pub static PROJECT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\+([^\s]+)").unwrap());
pub static CONTEXT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"@([^\s]+)").unwrap());
pub static DUE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"due:(\d{4}-\d{2}-\d{2})(?:T(\d{2}:\d{2}))?").unwrap());
pub static ID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\^([A-Za-z0-9]+)").unwrap());
pub static COMPLETION_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\s✅\s\d{4}-\d{2}-\d{2}").unwrap());
pub static RECUR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"rec:([^\s]+)").unwrap());
pub static NOTE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"~note:"((?:\\.|[^"])*)""#).unwrap());

/// Parse a due date/time from a string.
pub fn parse_due(text: &str) -> Option<NaiveDateTime> {
    let caps = DUE_RE.captures(text)?;
    let date_part = caps.get(1)?.as_str();
    let time_part = caps.get(2).map(|m| m.as_str());

    let date = NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok()?;
    let time = match time_part {
        Some(raw) => NaiveTime::parse_from_str(raw, "%H:%M").unwrap_or(DEFAULT_DUE_TIME),
        None => DEFAULT_DUE_TIME,
    };

    Some(NaiveDateTime::new(date, time))
}

/// Parse a markdown line into a TodoItem.
pub fn parse_line(line: &str, line_index: usize) -> Option<TodoItem> {
    let trimmed = line.trim_start();
    let (done, rest) = if let Some(body) = trimmed.strip_prefix("- [x]") {
        (true, body.trim())
    } else if let Some(body) = trimmed.strip_prefix("- [X]") {
        (true, body.trim())
    } else if let Some(body) = trimmed.strip_prefix("- [ ]") {
        (false, body.trim())
    } else {
        return None;
    };

    let title = extract_title(rest);
    let rest_without_note = NOTE_RE.replace(rest, "");
    let projects = capture_all_tokens(&PROJECT_RE, &rest_without_note);
    let contexts = capture_all_tokens(&CONTEXT_RE, &rest_without_note);
    let due = parse_due(rest);
    let recurrence = capture_token(&RECUR_RE, rest);
    let reference = capture_token(&LINK_RE, rest);
    let marker = capture_token(&ID_RE, rest);
    let note = capture_token(&NOTE_RE, rest)
        .map(|raw| unescape_note(&raw))
        .and_then(|n| normalize_note(Some(&n)));

    Some(TodoItem {
        key: TodoKey { line_index, marker },
        title,
        projects,
        contexts,
        due,
        reference,
        recurrence,
        note,
        done,
    })
}

/// Capture all matches of a regex (for projects/contexts).
pub fn capture_all_tokens(regex: &Regex, text: &str) -> Vec<String> {
    regex
        .find_iter(text)
        .filter_map(|m| {
            let s = m.as_str();
            // Strip the prefix (+ or @)
            s.get(1..).map(|t| t.trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Capture the first match of a regex.
pub fn capture_token(regex: &Regex, text: &str) -> Option<String> {
    regex
        .captures(text)
        .and_then(|caps| caps.get(1).map(|m| m.as_str().trim().to_string()))
}

/// Extract the title from a todo line (everything before field markers).
pub fn extract_title(rest: &str) -> String {
    let mut cut = rest.len();
    for marker in TITLE_MARKERS {
        if let Some(idx) = rest.find(marker) {
            if idx < cut {
                cut = idx;
            }
        }
    }

    let raw = if cut == rest.len() { rest } else { &rest[..cut] };
    let cleaned = raw.trim();

    if cleaned.is_empty() {
        rest.trim().to_string()
    } else {
        cleaned.to_string()
    }
}

/// Find a line by its marker ID.
pub fn find_line_by_marker(lines: &[String], marker: &str) -> Option<usize> {
    let needle = format!("^{marker}");
    lines
        .iter()
        .position(|line| line.split_whitespace().any(|token| token == needle))
}
