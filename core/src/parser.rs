//! Parsing logic for todo items from markdown lines.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use once_cell::sync::Lazy;
use regex::Regex;

use crate::types::{TodoItem, TodoKey, DEFAULT_DUE_TIME};
use crate::util::{normalize_note, unescape_note, FIELD_MARKERS};

// Regex patterns for parsing todo fields
pub static LINK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[\[([^\]]+)\]\]").unwrap());
// Projects and contexts accept either a quoted form (group 1, with backslash
// escapes — useful for multi-word names like +"Steuererklärung 2024") or a
// whitespace-delimited plain form (group 2).
pub static PROJECT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"\+(?:"((?:\\.|[^"])*)"|([^\s]+))"#).unwrap());
pub static CONTEXT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"@(?:"((?:\\.|[^"])*)"|([^\s]+))"#).unwrap());
pub static DUE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"due:(\d{4}-\d{2}-\d{2})(?:T(\d{2}:\d{2}))?").unwrap());
pub static ID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\^([A-Za-z0-9]+)").unwrap());
pub static COMPLETION_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\s✅\s\d{4}-\d{2}-\d{2}").unwrap());
pub static RECUR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"rec:([^\s]+)").unwrap());
pub static MYDAY_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"myday:(\d{4}-\d{2}-\d{2})").unwrap());
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

/// Parse a "my day" date from a string.
pub fn parse_myday(text: &str) -> Option<NaiveDate> {
    let caps = MYDAY_RE.captures(text)?;
    NaiveDate::parse_from_str(caps.get(1)?.as_str(), "%Y-%m-%d").ok()
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
    let myday = parse_myday(rest);
    let recurrence = capture_token(&RECUR_RE, rest);
    let reference = capture_token(&LINK_RE, rest);
    let marker = marker_of(rest);
    let note = capture_token(&NOTE_RE, rest)
        .map(|raw| unescape_note(&raw))
        .and_then(|n| normalize_note(Some(&n)));

    Some(TodoItem {
        key: TodoKey { line_index, marker },
        title,
        projects,
        contexts,
        due,
        myday,
        reference,
        recurrence,
        note,
        done,
    })
}

/// Capture all matches of a regex (for projects/contexts).
///
/// Supports both the quoted form (capture group 1, backslash-escaped) and the
/// plain whitespace-delimited form (capture group 2) produced by PROJECT_RE /
/// CONTEXT_RE.
pub fn capture_all_tokens(regex: &Regex, text: &str) -> Vec<String> {
    regex
        .captures_iter(text)
        .filter_map(|caps| {
            if let Some(quoted) = caps.get(1) {
                Some(unescape_note(quoted.as_str()))
            } else {
                caps.get(2).map(|m| m.as_str().to_string())
            }
        })
        .map(|s| s.trim().to_string())
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
/// Quote-aware: `+"name with spaces"` is treated as a single leading token.
fn strip_leading_markers(text: &str) -> &str {
    let mut cleaned = text.trim_start();
    while let Some(first) = cleaned.chars().next() {
        let re = match first {
            '+' => &*PROJECT_RE,
            '@' => &*CONTEXT_RE,
            _ => break,
        };
        let Some(m) = re.find(cleaned) else { break };
        if m.start() != 0 {
            break;
        }
        let after = &cleaned[m.end()..];
        // Require whitespace separator between the token and whatever follows,
        // otherwise the remainder belongs to the title.
        let Some(first_after) = after.chars().next() else { break };
        if !first_after.is_whitespace() {
            break;
        }
        let after_trimmed = after.trim_start();
        if after_trimmed.is_empty() {
            break;
        }
        cleaned = after_trimmed;
    }
    cleaned
}

/// Extract the title from a todo line (everything before field markers).
pub fn extract_title(rest: &str) -> String {
    let cleaned_rest = strip_leading_markers(rest);

    let mut cut = cleaned_rest.len();
    for marker in FIELD_MARKERS {
        if let Some(idx) = cleaned_rest.find(marker)
            && idx < cut {
                cut = idx;
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
    // Prefer the line whose *own* marker this is. A line may carry several
    // `^id` tokens — an Obsidian block link plus the todo marker — and the same
    // block link can appear on many lines. Matching those first would rewrite
    // an unrelated line, leaving the todo the user acted on untouched.
    if let Some(index) = lines
        .iter()
        .position(|line| marker_of(line).as_deref() == Some(marker))
    {
        return Some(index);
    }

    let needle = format!("^{marker}");
    lines
        .iter()
        .position(|line| line.split_whitespace().any(|token| token == needle))
}

/// Locate the `^id` that identifies a todo: the last whitespace-delimited one.
///
/// Returns its byte offset and the id itself. Reinschrift appends its marker at
/// the end, so anything before it (Obsidian block links such as `^3pip9m`)
/// belongs to the note, not to the todo.
pub fn marker_match(text: &str) -> Option<(usize, String)> {
    let is_standalone = |start: usize| {
        start == 0
            || text.as_bytes()[start - 1].is_ascii_whitespace()
    };

    let mut fallback = None;
    let mut standalone = None;
    for caps in ID_RE.captures_iter(text) {
        let whole = caps.get(0)?;
        let id = caps.get(1)?.as_str().to_string();
        if is_standalone(whole.start()) {
            standalone = Some((whole.start(), id));
        } else {
            fallback = Some((whole.start(), id));
        }
    }
    standalone.or(fallback)
}

/// The marker identifying this line, if any.
pub fn marker_of(text: &str) -> Option<String> {
    marker_match(text).map(|(_, id)| id)
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

    #[test]
    fn parse_line_quoted_project_keeps_spaces() {
        let item = parse_line(
            r#"- [ ] +"Steuererklärung 2024" erledigen due:2026-01-01 ^abc12345"#,
            0,
        )
        .expect("valid line");
        assert_eq!(item.title, "erledigen");
        assert_eq!(item.projects, vec!["Steuererklärung 2024".to_string()]);
    }

    #[test]
    fn parse_line_quoted_context_keeps_spaces() {
        let item = parse_line(
            r#"- [ ] Task @"Home Office" something ^abc12345"#,
            0,
        )
        .expect("valid line");
        assert_eq!(item.contexts, vec!["Home Office".to_string()]);
    }

    #[test]
    fn parse_line_mixes_quoted_and_plain_projects() {
        let item = parse_line(
            r#"- [ ] Task +plain +"Name With Spaces" more ^abc12345"#,
            0,
        )
        .expect("valid line");
        assert_eq!(
            item.projects,
            vec!["plain".to_string(), "Name With Spaces".to_string()]
        );
    }

    #[test]
    fn parse_line_quoted_project_with_escaped_quote() {
        let item = parse_line(
            r#"- [ ] Task +"Name with \"quote\"" ^abc12345"#,
            0,
        )
        .expect("valid line");
        assert_eq!(item.projects, vec![r#"Name with "quote""#.to_string()]);
    }

    #[test]
    fn render_line_quotes_multi_word_project() {
        use crate::types::{TodoItem, TodoKey};
        let item = TodoItem {
            key: TodoKey { line_index: 0, marker: Some("abc12345".to_string()) },
            title: "erledigen".to_string(),
            projects: vec!["Steuererklärung 2024".to_string()],
            contexts: vec![],
            due: None,
            myday: None,
            reference: None,
            recurrence: None,
            note: None,
            done: false,
        };
        let line = crate::renderer::render_line(&item).expect("render ok");
        assert!(
            line.contains(r#"+"Steuererklärung 2024""#),
            "expected quoted form in {line}"
        );
    }

    #[test]
    fn parse_line_extracts_myday() {
        let item = parse_line(
            "- [ ] Task due:2026-06-10T12:00 myday:2026-06-04 ^abc12345",
            0,
        )
        .expect("valid line");
        assert_eq!(item.myday, NaiveDate::from_ymd_opt(2026, 6, 4));
        assert_eq!(item.title, "Task");
    }

    #[test]
    fn extract_title_stops_at_myday() {
        assert_eq!(extract_title("Buy groceries myday:2026-06-04"), "Buy groceries");
    }

    #[test]
    fn roundtrip_myday_today() {
        use crate::types::{TodoItem, TodoKey};
        let today = chrono::Local::now().date_naive();
        let original = TodoItem {
            key: TodoKey { line_index: 0, marker: Some("abc12345".to_string()) },
            title: "erledigen".to_string(),
            projects: vec![],
            contexts: vec![],
            due: None,
            myday: Some(today),
            reference: None,
            recurrence: None,
            note: None,
            done: false,
        };
        let line = crate::renderer::render_line(&original).expect("render ok");
        let parsed = parse_line(&line, 0).expect("parse ok");
        assert_eq!(parsed.myday, Some(today));
    }

    #[test]
    fn render_drops_stale_myday() {
        use crate::types::{TodoItem, TodoKey};
        let yesterday = chrono::Local::now().date_naive() - chrono::Duration::days(1);
        let original = TodoItem {
            key: TodoKey { line_index: 0, marker: Some("abc12345".to_string()) },
            title: "erledigen".to_string(),
            projects: vec![],
            contexts: vec![],
            due: None,
            myday: Some(yesterday),
            reference: None,
            recurrence: None,
            note: None,
            done: false,
        };
        let line = crate::renderer::render_line(&original).expect("render ok");
        assert!(!line.contains("myday:"), "stale myday should be dropped in {line}");
    }

    #[test]
    fn roundtrip_quoted_project() {
        use crate::types::{TodoItem, TodoKey};
        let original = TodoItem {
            key: TodoKey { line_index: 0, marker: Some("abc12345".to_string()) },
            title: "erledigen".to_string(),
            projects: vec!["Steuererklärung 2024".to_string()],
            contexts: vec!["Home Office".to_string()],
            due: None,
            myday: None,
            reference: None,
            recurrence: None,
            note: None,
            done: false,
        };
        let line = crate::renderer::render_line(&original).expect("render ok");
        let parsed = parse_line(&line, 0).expect("parse ok");
        assert_eq!(parsed.title, original.title);
        assert_eq!(parsed.projects, original.projects);
        assert_eq!(parsed.contexts, original.contexts);
    }
}
