//! Rendering logic for converting TodoItems back to markdown lines.

use anyhow::{bail, Result};
use chrono::{Local, NaiveDateTime};

use once_cell::sync::Lazy;
use regex::Regex;

use crate::i18n::t;
use crate::parser::{COMPLETION_RE, DUE_RE, ID_RE, MYDAY_RE};
use crate::types::TodoItem;
use crate::util::{escape_note, generate_marker, normalize_note, normalize_reference, normalize_token, FIELD_MARKERS};

/// Render a `+project` / `@context` token, quoting it when the name contains
/// whitespace or characters that would otherwise break the plain form.
fn render_tagged(prefix: char, name: &str) -> String {
    let needs_quote = name.chars().any(|c| c.is_whitespace() || c == '"' || c == '\\');
    if needs_quote {
        format!(r#"{prefix}"{}""#, escape_note(name))
    } else {
        format!("{prefix}{name}")
    }
}

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
            parts.push(render_tagged('+', &normalized));
        }
    }
    for context in &item.contexts {
        if let Some(normalized) = normalize_token(Some(context)) {
            parts.push(render_tagged('@', &normalized));
        }
    }
    if let Some(due) = item.due {
        parts.push(format!("due:{}", due.format("%Y-%m-%dT%H:%M")));
    }
    // Stale "my day" dates (before today) are dropped on re-render so old
    // planning tokens clean themselves up over time.
    if let Some(myday) = item.myday {
        if myday >= Local::now().date_naive() {
            parts.push(format!("myday:{}", myday.format("%Y-%m-%d")));
        }
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

/// Matches a myday token including its leading whitespace, for clean removal.
static MYDAY_STRIP_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\s*myday:\d{4}-\d{2}-\d{2}").unwrap());

/// Add (with today's date) or remove the `myday:` token in a line.
pub fn rewrite_myday(line: &str, add: bool) -> Result<String> {
    if !add {
        return Ok(MYDAY_STRIP_RE.replace_all(line, "").to_string());
    }

    let today = Local::now().date_naive();
    let segment = format!("myday:{}", today.format("%Y-%m-%d"));
    if MYDAY_RE.is_match(line) {
        return Ok(MYDAY_RE.replace(line, segment).to_string());
    }

    // Insert directly after the due token when present, otherwise before the
    // first field marker (same placement rule as due dates).
    if let Some(due_m) = DUE_RE.find(line) {
        let (head, tail) = line.split_at(due_m.end());
        Ok(format!("{head} {segment}{tail}"))
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
                    let done_str = format!("{} ", m.as_str().trim_start());
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
            // Add completion marker, keeping the ^ID a whitespace-separated
            // token (find_line_by_marker matches tokens).
            let today = Local::now().date_naive().format("%Y-%m-%d");
            if let Some(id_m) = ID_RE.find(line) {
                let mut s = line.to_string();
                s.insert_str(id_m.start(), &format!("✅ {today} "));
                s
            } else {
                format!("{line} ✅ {today}")
            }
        }
    } else {
        COMPLETION_RE.replace(line, "").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_myday_inserts_after_due() {
        let line = "- [ ] Task due:2026-06-10T12:00 rec:weekly ^abc12345";
        let updated = rewrite_myday(line, true).expect("rewrite ok");
        let today = Local::now().date_naive().format("%Y-%m-%d");
        assert_eq!(
            updated,
            format!("- [ ] Task due:2026-06-10T12:00 myday:{today} rec:weekly ^abc12345")
        );
    }

    #[test]
    fn rewrite_myday_without_due_inserts_before_fields() {
        let line = "- [ ] Task +proj ^abc12345";
        let updated = rewrite_myday(line, true).expect("rewrite ok");
        let today = Local::now().date_naive().format("%Y-%m-%d");
        assert_eq!(updated, format!("- [ ] Task myday:{today} +proj ^abc12345"));
    }

    #[test]
    fn rewrite_myday_is_idempotent() {
        let line = "- [ ] Task due:2026-06-10T12:00 ^abc12345";
        let once = rewrite_myday(line, true).expect("rewrite ok");
        let twice = rewrite_myday(&once, true).expect("rewrite ok");
        assert_eq!(once, twice);
    }

    #[test]
    fn rewrite_myday_replaces_stale_date() {
        let line = "- [ ] Task myday:2020-01-01 ^abc12345";
        let updated = rewrite_myday(line, true).expect("rewrite ok");
        let today = Local::now().date_naive().format("%Y-%m-%d");
        assert_eq!(updated, format!("- [ ] Task myday:{today} ^abc12345"));
    }

    #[test]
    fn rewrite_myday_removes_cleanly() {
        let line = "- [ ] Task due:2026-06-10T12:00 myday:2026-06-04 rec:weekly ^abc12345";
        let updated = rewrite_myday(line, false).expect("rewrite ok");
        assert_eq!(updated, "- [ ] Task due:2026-06-10T12:00 rec:weekly ^abc12345");
        assert!(!updated.contains("  "), "no double spaces in {updated}");
    }

    #[test]
    fn rewrite_myday_remove_without_token_is_noop() {
        let line = "- [ ] Task ^abc12345";
        let updated = rewrite_myday(line, false).expect("rewrite ok");
        assert_eq!(updated, line);
    }

    #[test]
    fn completion_marker_keeps_id_token_separated() {
        let line = "- [ ] Task ^abc12345";
        let done = rewrite_line(line, true).expect("rewrite ok");
        let today = Local::now().date_naive().format("%Y-%m-%d");
        assert_eq!(done, format!("- [x] Task ✅ {today} ^abc12345"));
        assert!(
            done.split_whitespace().any(|t| t == "^abc12345"),
            "marker must stay a token: {done}"
        );

        let reopened = rewrite_line(&done, false).expect("rewrite ok");
        assert_eq!(reopened, line);
    }

    #[test]
    fn completion_marker_without_id_appends() {
        let done = rewrite_line("- [ ] Task", true).expect("rewrite ok");
        let today = Local::now().date_naive().format("%Y-%m-%d");
        assert_eq!(done, format!("- [x] Task ✅ {today}"));
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
