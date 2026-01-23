//! Rendering logic for converting TodoItems back to markdown lines.

use anyhow::{bail, Result};
use chrono::{Local, NaiveDateTime};

use crate::i18n::t;
use crate::parser::{COMPLETION_RE, DUE_RE, ID_RE};
use crate::types::TodoItem;
use crate::util::{escape_note, generate_marker, normalize_note, normalize_reference, normalize_token, FIELD_MARKERS};

/// Render a TodoItem to a markdown line.
pub fn render_line(item: &TodoItem) -> Result<String> {
    let title = item.title.trim();
    if title.is_empty() {
        bail!(t("title_empty_error"));
    }

    let checkbox = if item.done { "- [x]" } else { "- [ ]" };
    let mut parts = vec![format!("{checkbox} {title}")];

    for project in &item.projects {
        if let Some(normalized) = normalize_token(Some(project)) {
            parts.push(format!("+{normalized}"));
        }
    }
    for context in &item.contexts {
        if let Some(normalized) = normalize_token(Some(context)) {
            parts.push(format!("@{normalized}"));
        }
    }
    if let Some(due) = item.due {
        parts.push(format!("due:{}", due.format("%Y-%m-%dT%H:%M")));
    }
    if let Some(recur) = normalize_token(item.recurrence.as_deref()) {
        parts.push(format!("rec:{recur}"));
    }
    if let Some(reference) = normalize_reference(item.reference.as_deref()) {
        parts.push(format!("[[{reference}]]"));
    }

    if let Some(note) = normalize_note(item.note.as_deref()) {
        let escaped = escape_note(&note);
        parts.push(format!(r#"~note:"{escaped}""#));
    }

    if item.done {
        let today = Local::now().date_naive().format("%Y-%m-%d");
        parts.push(format!("✅ {today}"));
    }

    let marker = item
        .key
        .marker
        .as_deref()
        .filter(|m| !m.is_empty())
        .map(|m| m.to_string())
        .unwrap_or_else(generate_marker);

    parts.push(format!("^{marker}"));

    Ok(parts.join(" "))
}

/// Rewrite a line to toggle its done state.
pub fn rewrite_line(line: &str, done: bool) -> Result<String> {
    let mut updated = line.to_string();
    let has_checked = updated.contains("- [x]") || updated.contains("- [X]");
    let has_unchecked = updated.contains("- [ ]");

    if done {
        if !has_checked {
            if has_unchecked {
                updated = updated.replacen("- [ ]", "- [x]", 1);
            } else {
                bail!(t("no_checkbox_error"));
            }
        } else if updated.contains("- [X]") {
            updated = updated.replacen("- [X]", "- [x]", 1);
        }
    } else if has_checked {
        updated = updated.replacen("- [x]", "- [ ]", 1);
        updated = updated.replacen("- [X]", "- [ ]", 1);
    } else if !has_unchecked {
        bail!(t("no_checkbox_error"));
    }

    updated = apply_completion_marker(&updated, done);

    Ok(updated)
}

/// Rewrite the due date in a line.
pub fn rewrite_due(line: &str, new_due: NaiveDateTime) -> Result<String> {
    let segment = format!("due:{}", new_due.format("%Y-%m-%dT%H:%M"));
    if DUE_RE.is_match(line) {
        Ok(DUE_RE.replace(line, segment).to_string())
    } else {
        Ok(insert_due_segment(line, &segment))
    }
}

/// Apply or remove completion marker (✅ date).
pub fn apply_completion_marker(line: &str, done: bool) -> String {
    if done {
        if let Some(m) = COMPLETION_RE.find(line) {
            // Already present. Check if it's after the ID.
            if let Some(id_m) = ID_RE.find(line) {
                if m.start() > id_m.start() {
                    let done_str = m.as_str().to_string();
                    let mut s = line.to_string();
                    s.replace_range(m.range(), "");
                    if let Some(new_id_m) = ID_RE.find(&s) {
                        s.insert_str(new_id_m.start(), &done_str);
                        return s;
                    }
                }
            }
            line.to_string()
        } else {
            // Add completion marker
            let today = Local::now().date_naive().format("%Y-%m-%d");
            let done_marker = format!(" ✅ {today}");
            if let Some(id_m) = ID_RE.find(line) {
                let mut s = line.to_string();
                s.insert_str(id_m.start(), &done_marker);
                s
            } else {
                format!("{line}{done_marker}")
            }
        }
    } else {
        COMPLETION_RE.replace(line, "").to_string()
    }
}

/// Insert a due segment into a line at the appropriate position.
fn insert_due_segment(line: &str, segment: &str) -> String {
    let mut insert_at = line.len();
    for marker in FIELD_MARKERS {
        if let Some(idx) = line.find(marker) {
            insert_at = insert_at.min(idx);
        }
    }

    let (head, tail) = line.split_at(insert_at);
    let needs_space = !head.ends_with(' ') && !head.is_empty();
    if needs_space {
        format!("{head} {segment}{tail}")
    } else {
        format!("{head}{segment}{tail}")
    }
}
