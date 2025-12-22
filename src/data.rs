use std::fs;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{Local, NaiveDate};
use once_cell::sync::Lazy;
use regex::Regex;

pub const TODO_DB_PATH: &str = "/home/danst/Nextcloud/InOmnibusVeritas/TodosDatenbank.md";

static LINK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[\[([^\]]+)\]\]").unwrap());
static PROJECT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\+([^\s]+)").unwrap());
static CONTEXT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"@([^\s]+)").unwrap());
static DUE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"due:(\d{4}-\d{2}-\d{2})").unwrap());
static ID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\^([A-Za-z0-9]+)").unwrap());
static COMPLETION_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s✅\s\d{4}-\d{2}-\d{2}").unwrap());

#[derive(Clone, Debug)]
pub struct TodoKey {
    pub line_index: usize,
    pub marker: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TodoItem {
    pub key: TodoKey,
    pub title: String,
    pub section: String,
    pub project: Option<String>,
    pub context: Option<String>,
    pub due: Option<NaiveDate>,
    pub reference: Option<String>,
    pub done: bool,
}

pub fn todo_path() -> &'static Path {
    Path::new(TODO_DB_PATH)
}

pub fn load_todos() -> Result<Vec<TodoItem>> {
    let content = fs::read_to_string(todo_path())
        .with_context(|| format!("Konnte {} nicht lesen", todo_path().display()))?;

    let mut items = Vec::new();
    let mut current_section = String::from("Ohne Abschnitt");

    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("###") {
            current_section = trimmed.trim_start_matches('#').trim().to_string();
            continue;
        }

        if let Some(item) = parse_line(line, line_index, &current_section) {
            items.push(item);
        }
    }

    Ok(items)
}

pub fn toggle_todo(key: &TodoKey, done: bool) -> Result<()> {
    let content = fs::read_to_string(todo_path())
        .with_context(|| format!("Konnte {} nicht lesen", todo_path().display()))?;
    let mut lines: Vec<String> = content.lines().map(|line| line.to_string()).collect();
    let had_trailing_newline = content.ends_with('\n');

    let mut target_index = None;
    if let Some(marker) = &key.marker {
        target_index = find_line_by_marker(&lines, marker);
    }
    if target_index.is_none() && key.line_index < lines.len() {
        target_index = Some(key.line_index);
    }

    let index = target_index.ok_or_else(|| anyhow!("Konnte To-do in der Datei nicht finden"))?;
    let updated_line = rewrite_line(&lines[index], done)
        .with_context(|| format!("Konnte Zeile {} nicht aktualisieren", index + 1))?;
    lines[index] = updated_line;

    let mut output = lines.join("\n");
    if had_trailing_newline {
        output.push('\n');
    }

    fs::write(todo_path(), output)
        .with_context(|| format!("Konnte {} nicht schreiben", todo_path().display()))?;

    Ok(())
}

fn parse_line(line: &str, line_index: usize, section: &str) -> Option<TodoItem> {
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
    let project = capture_token(&PROJECT_RE, rest);
    let context = capture_token(&CONTEXT_RE, rest);
    let due = capture_token(&DUE_RE, rest).and_then(|value| NaiveDate::parse_from_str(&value, "%Y-%m-%d").ok());
    let reference = capture_token(&LINK_RE, rest);
    let marker = capture_token(&ID_RE, rest);

    Some(TodoItem {
        key: TodoKey {
            line_index,
            marker,
        },
        title,
        section: section.to_string(),
        project,
        context,
        due,
        reference,
        done,
    })
}

fn capture_token(regex: &Regex, text: &str) -> Option<String> {
    regex
        .captures(text)
        .and_then(|caps| caps.get(1).map(|m| m.as_str().trim().to_string()))
}

fn extract_title(rest: &str) -> String {
    const MARKERS: [&str; 12] = [" +", " @", " due:", " [[", " ✅", " ^", "+", "@", "due:", "[[", "✅", "^"];
    let mut cut = rest.len();
    for marker in MARKERS {
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

fn find_line_by_marker(lines: &[String], marker: &str) -> Option<usize> {
    let needle = format!("^{marker}");
    lines
        .iter()
        .position(|line| line.split_whitespace().any(|token| token == needle))
}

fn rewrite_line(line: &str, done: bool) -> Result<String> {
    let mut updated = line.to_string();
    let has_checked = updated.contains("- [x]") || updated.contains("- [X]");
    let has_unchecked = updated.contains("- [ ]");

    if done {
        if !has_checked {
            if has_unchecked {
                updated = updated.replacen("- [ ]", "- [x]", 1);
            } else {
                bail!("Zeile enthält keine Checkbox");
            }
        } else if updated.contains("- [X]") {
            updated = updated.replacen("- [X]", "- [x]", 1);
        }
    } else if has_checked {
        updated = updated.replacen("- [x]", "- [ ]", 1);
        updated = updated.replacen("- [X]", "- [ ]", 1);
    } else if !has_unchecked {
        bail!("Zeile enthält keine Checkbox");
    }

    updated = apply_completion_marker(&updated, done);

    Ok(updated)
}

fn apply_completion_marker(line: &str, done: bool) -> String {
    if done {
        if COMPLETION_RE.is_match(line) {
            line.to_string()
        } else {
            let marker = format!(" ✅ {}", Local::now().format("%Y-%m-%d"));
            if let Some(idx) = line.rfind(" ^") {
                let (head, tail) = line.split_at(idx);
                format!("{head}{marker}{tail}")
            } else {
                format!("{line}{marker}")
            }
        }
    } else {
        COMPLETION_RE.replace(line, "").to_string()
    }
}