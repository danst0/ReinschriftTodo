//! Parsing logic for todo items from markdown lines.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use once_cell::sync::Lazy;
use regex::Regex;

use crate::types::{TodoItem, TodoKey, DEFAULT_DUE_TIME};
use crate::util::{normalize_note, unescape_note, FIELD_MARKERS};

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

/// Strip leading `+project` / `@context` tokens so the title begins with real text.
fn strip_leading_markers(text: &str) -> &str {
    let mut cleaned = text.trim_start();
    loop {
        let Some(first) = cleaned.chars().next() else { break };
        if first != '+' && first != '@' {
            break;
        }
        let Some(space_idx) = cleaned.find(char::is_whitespace) else { break };
        // Require at least one char of token body between the prefix and whitespace.
        if space_idx <= 1 {
            break;
        }
        let after = cleaned[space_idx..].trim_start();
        if after.is_empty() {
            break;
        }
        cleaned = after;
    }
    cleaned
}

/// Extract the title from a todo line (everything before field markers).
pub fn extract_title(rest: &str) -> String {
    let cleaned_rest = strip_leading_markers(rest);

    let mut cut = cleaned_rest.len();
    for marker in FIELD_MARKERS {
        if let Some(idx) = cleaned_rest.find(marker) {
            if idx < cut {
                cut = idx;
            }
        }
    }

    let raw = &cleaned_rest[..cut];
    let cleaned = raw.trim();

    if cleaned.is_empty() {
        cleaned_rest.trim().to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_title_strips_leading_project() {
        assert_eq!(
            extract_title("+Steuererklärung erledigen due:2026-03-03T00:00 ^85rlhbg3"),
            "erledigen"
        );
    }

    #[test]
    fn extract_title_strips_multiple_leading_tags() {
        assert_eq!(extract_title("+a +b @c buy milk due:2026-01-01"), "buy milk");
    }

    #[test]
    fn extract_title_keeps_tags_inside_title() {
        assert_eq!(extract_title("buy milk +groceries @shop"), "buy milk");
    }

    #[test]
    fn extract_title_plain() {
        assert_eq!(extract_title("Just a title"), "Just a title");
    }

    #[test]
    fn extract_title_with_due_only() {
        assert_eq!(extract_title("Buy groceries due:2026-01-20"), "Buy groceries");
    }

    #[test]
    fn extract_title_with_marker_only() {
        assert_eq!(extract_title("Buy groceries ^abc123"), "Buy groceries");
    }

    #[test]
    fn parse_line_clean_title_when_starting_with_project() {
        let item =
            parse_line("- [ ] +Steuererklärung erledigen due:2026-03-03T00:00 ^85rlhbg3", 0)
                .expect("valid line");
        assert_eq!(item.title, "erledigen");
        assert_eq!(item.projects, vec!["Steuererklärung".to_string()]);
        assert_eq!(item.key.marker.as_deref(), Some("85rlhbg3"));
    }
}
