use std::{env, fs};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::i18n::t;
use anyhow::{anyhow, bail, Context, Result};
use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::blocking::Client;
use serde::Serialize;

#[derive(Clone, Debug)]
pub enum BackendConfig {
    Local(PathBuf),
    WebDav {
        url: String,
        path: Option<String>,
        username: Option<String>,
        password: Option<String>,
    },
}

static BACKEND_CONFIG: Lazy<Mutex<BackendConfig>> = Lazy::new(|| {
    let configured = default_todo_path();
    Mutex::new(BackendConfig::Local(configured))
});

pub fn set_backend_config(config: BackendConfig) {
    if let Ok(mut c) = BACKEND_CONFIG.lock() {
        *c = config;
    }
}

pub fn get_backend_config() -> BackendConfig {
    BACKEND_CONFIG.lock().unwrap().clone()
}

static TODO_PATH: Lazy<Mutex<PathBuf>> = Lazy::new(|| {
    Mutex::new(default_todo_path())
});

pub fn default_todo_path() -> PathBuf {
    env::var("TODOS_DB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::new())
}

static LINK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[\[([^\]]+)\]\]").unwrap());
static PROJECT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\+([^\s]+)").unwrap());
static CONTEXT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"@([^\s]+)").unwrap());
static DUE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"due:(\d{4}-\d{2}-\d{2})(?:T(\d{2}:\d{2}))?").unwrap());
static ID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\^([A-Za-z0-9]+)").unwrap());
static COMPLETION_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s✅\s\d{4}-\d{2}-\d{2}").unwrap());
static RECUR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"rec:([^\s]+)").unwrap());
static NOTE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"~note:"((?:\\.|[^"])*)""#).unwrap());

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TodoKey {
    pub line_index: usize,
    pub marker: Option<String>,
}

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

const DEFAULT_DUE_TIME: NaiveTime = NaiveTime::from_hms_opt(0, 0, 0).expect("midnight time available");

pub fn todo_path() -> PathBuf {
    TODO_PATH
        .lock()
        .expect("todo path lock poisoned")
        .clone()
}

pub fn set_todo_path(new_path: PathBuf) {
    if let Ok(mut path) = TODO_PATH.lock() {
        *path = new_path.clone();
    }
    set_backend_config(BackendConfig::Local(new_path));
}

pub fn get_fingerprint() -> Result<String> {
    let config = get_backend_config();
    match config {
        BackendConfig::Local(path) => {
            let metadata = fs::metadata(&path)?;
            let mtime = metadata.modified()?;
            Ok(format!("{:?}", mtime))
        }
        BackendConfig::WebDav { url, path, username, password } => {
            let client = Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()?;

            let construct_url = |base: &str| -> String {
                if let Some(p) = &path {
                    format!("{}/{}", base.trim_end_matches('/'), p.trim_start_matches('/'))
                } else {
                    base.to_string()
                }
            };

            let full_url = construct_url(&url);
            let mut req = client.head(&full_url);
            if let (Some(u), Some(p)) = (&username, &password) {
                req = req.basic_auth(u, Some(p));
            }
            let resp = req.send()?;
            if !resp.status().is_success() {
                bail!("WebDAV error: {}", resp.status());
            }
            
            let etag = resp.headers().get("etag").and_then(|v| v.to_str().ok()).unwrap_or("");
            let last_mod = resp.headers().get("last-modified").and_then(|v| v.to_str().ok()).unwrap_or("");
            
            Ok(format!("{}-{}", etag, last_mod))
        }
    }
}

fn read_content() -> Result<String> {
    let config = get_backend_config();
    match config {
        BackendConfig::Local(path) => {
            if path.as_os_str().is_empty() {
                bail!(t("no_database_configured"));
            }
             fs::read_to_string(&path)
                .with_context(|| t("read_error").replace("{}", &path.display().to_string()))
        }
        BackendConfig::WebDav { url, path, username, password } => {
            let client = Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()?;

            let construct_url = |base: &str| -> String {
                if let Some(p) = &path {
                    format!("{}/{}", base.trim_end_matches('/'), p.trim_start_matches('/'))
                } else {
                    base.to_string()
                }
            };

            let try_request = |target_url: &str| -> Result<String> {
                let mut req = client.get(target_url);
                if let (Some(u), Some(p)) = (&username, &password) {
                    req = req.basic_auth(u, Some(p));
                }
                let resp = req.send()?;
                if !resp.status().is_success() {
                    if resp.status() == reqwest::StatusCode::NOT_FOUND {
                        bail!("404 Not Found");
                    }
                    bail!("WebDAV error: {}", resp.status());
                }
                Ok(resp.text()?)
            };

            let full_url = construct_url(&url);
            match try_request(&full_url) {
                Ok(content) => Ok(content),
                Err(e) => {
                    // Fallback logic
                    if let Some(user) = &username {
                        if !url.contains("remote.php/dav/files") {
                            let candidate_base = format!("{}/remote.php/dav/files/{}", url.trim_end_matches('/'), user);
                            let candidate_full = construct_url(&candidate_base);
                            if let Ok(content) = try_request(&candidate_full) {
                                // Update internal config to use this working base URL for future requests
                                set_backend_config(BackendConfig::WebDav {
                                    url: candidate_base,
                                    path: path.clone(),
                                    username: username.clone(),
                                    password: password.clone(),
                                });
                                return Ok(content);
                            }
                        }
                    }
                    // Return original error
                    if e.to_string().contains("404 Not Found") {
                         bail!("WebDAV error: 404 Not Found. (Hint: For Nextcloud, ensure URL ends with /remote.php/dav/files/USERNAME)");
                    }
                    Err(e)
                }
            }
        }
    }
}

pub fn test_webdav_connection(base_url: &str, path: Option<&str>, username: Option<&str>, password: Option<&str>) -> Result<()> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let construct_url = |base: &str| -> String {
        if let Some(p) = path {
            format!("{}/{}", base.trim_end_matches('/'), p.trim_start_matches('/'))
        } else {
            base.to_string()
        }
    };

    let try_connect = |target_url: &str| -> Result<()> {
        // Try HEAD first
        let mut req = client.head(target_url);
        if let (Some(u), Some(p)) = (username, password) {
            req = req.basic_auth(u, Some(p));
        }
        
        let resp = req.send()?;
        if resp.status().is_success() {
            return Ok(());
        }

        // Fallback to GET
        let mut req_get = client.get(target_url);
        if let (Some(u), Some(p)) = (username, password) {
            req_get = req_get.basic_auth(u, Some(p));
        }
        let resp_get = req_get.send()?;
        if !resp_get.status().is_success() {
             if resp_get.status() == reqwest::StatusCode::NOT_FOUND {
                 bail!("404 Not Found");
             }
             bail!("HTTP {}", resp_get.status());
        }
        Ok(())
    };

    let full_url = construct_url(base_url);
    match try_connect(&full_url) {
        Ok(_) => return Ok(()),
        Err(e) => {
            // If it failed, and we have a username, try to guess Nextcloud path
            if let Some(user) = username {
                if !base_url.contains("remote.php/dav/files") {
                    let candidate_base = format!("{}/remote.php/dav/files/{}", base_url.trim_end_matches('/'), user);
                    let candidate_full = construct_url(&candidate_base);
                    if try_connect(&candidate_full).is_ok() {
                        // Success with fallback!
                        // We don't update config here because this is just a test function.
                        // But we return Ok to indicate connection is possible.
                        return Ok(());
                    }
                }
            }
            // Return original error with context
            bail!(t("connection_error").replace("{}", &full_url).replace("{}", &e.to_string()));
        }
    }
}

fn write_content(content: String) -> Result<()> {
    let config = get_backend_config();
    match config {
        BackendConfig::Local(path) => {
             fs::write(&path, content)
                .with_context(|| t("write_error").replace("{}", &path.display().to_string()))
        }
        BackendConfig::WebDav { url, path, username, password } => {
            let client = Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()?;

            let construct_url = |base: &str| -> String {
                if let Some(p) = &path {
                    format!("{}/{}", base.trim_end_matches('/'), p.trim_start_matches('/'))
                } else {
                    base.to_string()
                }
            };

            let try_request = |target_url: &str| -> Result<()> {
                let mut req = client.put(target_url);
                if let (Some(u), Some(p)) = (&username, &password) {
                    req = req.basic_auth(u, Some(p));
                }
                req = req.body(content.clone());
                let resp = req.send()?;
                if !resp.status().is_success() {
                    if resp.status() == reqwest::StatusCode::NOT_FOUND {
                        bail!("404 Not Found");
                    }
                    bail!("WebDAV error: {}", resp.status());
                }
                Ok(())
            };

            let full_url = construct_url(&url);
            match try_request(&full_url) {
                Ok(_) => Ok(()),
                Err(e) => {
                    // Fallback logic
                    if let Some(user) = &username {
                        if !url.contains("remote.php/dav/files") {
                            let candidate_base = format!("{}/remote.php/dav/files/{}", url.trim_end_matches('/'), user);
                            let candidate_full = construct_url(&candidate_base);
                            if try_request(&candidate_full).is_ok() {
                                // Update internal config
                                set_backend_config(BackendConfig::WebDav {
                                    url: candidate_base,
                                    path: path.clone(),
                                    username: username.clone(),
                                    password: password.clone(),
                                });
                                return Ok(());
                            }
                        }
                    }
                    if e.to_string().contains("404 Not Found") {
                         bail!("WebDAV error: 404 Not Found. (Hint: For Nextcloud, ensure URL ends with /remote.php/dav/files/USERNAME)");
                    }
                    Err(e)
                }
            }
        }
    }
}

pub fn load_todos() -> Result<Vec<TodoItem>> {
    let content = read_content()?;

    let mut items = Vec::new();

    for (line_index, line) in content.lines().enumerate() {
        if let Some(item) = parse_line(line, line_index) {
            items.push(item);
        }
    }

    Ok(items)
}

pub fn toggle_todo(key: &TodoKey, done: bool) -> Result<()> {
    let content = read_content()?;
    let mut lines: Vec<String> = content.lines().map(|line| line.to_string()).collect();
    let had_trailing_newline = content.ends_with('\n');

    let mut target_index = None;
    if let Some(marker) = &key.marker {
        target_index = find_line_by_marker(&lines, marker);
    }
    if target_index.is_none() && key.line_index < lines.len() {
        target_index = Some(key.line_index);
    }

    let index = target_index.ok_or_else(|| anyhow!(t("todo_not_found")))?;
    let updated_line = rewrite_line(&lines[index], done)
        .with_context(|| t("line_update_error").replace("{}", &(index + 1).to_string()))?;
    lines[index] = updated_line;

    let mut output = lines.join("\n");
    if had_trailing_newline {
        output.push('\n');
    }

    write_content(output)?;

    Ok(())
}

pub fn set_due_today(key: &TodoKey) -> Result<NaiveDateTime> {
    let now = Local::now();
    let today = now.date_naive();
    let current_hour = now.hour();
    
    // Smart "Today" logic: at least 4 hours from now
    let time = if current_hour < 8 {
        NaiveTime::from_hms_opt(12, 0, 0).unwrap()
    } else if current_hour < 14 {
        NaiveTime::from_hms_opt(18, 0, 0).unwrap()
    } else {
        NaiveTime::from_hms_opt(18, 0, 0).unwrap()
    };
    
    let due_dt = NaiveDateTime::new(today, time);
    update_line(key, |line| rewrite_due(line, due_dt))?;
    Ok(due_dt)
}

pub fn set_due_tomorrow(key: &TodoKey) -> Result<NaiveDateTime> {
    let tomorrow = Local::now().date_naive() + chrono::Duration::days(1);
    let time = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
    let due_dt = NaiveDateTime::new(tomorrow, time);
    update_line(key, |line| rewrite_due(line, due_dt))?;
    Ok(due_dt)
}

pub fn set_due_weekend(key: &TodoKey) -> Result<NaiveDateTime> {
    let today = Local::now().date_naive();
    let weekday = today.weekday().num_days_from_monday(); // 0=Mon, 5=Sat, 6=Sun
    
    let days_until_saturday: i64 = if weekday == 5 {
        7  // Today is Saturday, go to next Saturday
    } else if weekday == 6 {
        6  // Today is Sunday, go to next Saturday
    } else {
        (5 - weekday) as i64  // Monday-Friday: days until this Saturday
    };
    
    let target_date = today + chrono::Duration::days(days_until_saturday);
    let time = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
    let due_dt = NaiveDateTime::new(target_date, time);
    update_line(key, |line| rewrite_due(line, due_dt))?;
    Ok(due_dt)
}

pub fn set_due_sometime(key: &TodoKey) -> Result<NaiveDateTime> {
    let sometime = NaiveDate::from_ymd_opt(9999, 12, 31).unwrap();
    let due_dt = NaiveDateTime::new(sometime, DEFAULT_DUE_TIME);
    update_line(key, |line| rewrite_due(line, due_dt))?;
    Ok(due_dt)
}

pub fn update_todo_details(item: &TodoItem) -> Result<()> {
    let rendered = render_line(item)?;
    update_line(&item.key, |_| Ok(rendered))
}

pub fn delete_todo(item: &TodoItem) -> Result<()> {
    delete_line(&item.key)
}

pub fn add_todo(title: &str) -> Result<TodoKey> {
    let title = title.trim();
    if title.is_empty() {
        bail!(t("title_empty_error"));
    }
    let today = Local::now().date_naive();
    let due_dt = NaiveDateTime::new(today, DEFAULT_DUE_TIME);
    let marker = generate_marker();
    let line = format!("- [ ] {} due:{} ^{}", title, due_dt.format("%Y-%m-%dT%H:%M"), marker);
    insert_line(line, marker)
}

pub fn add_todo_full(item: &TodoItem) -> Result<TodoKey> {
    let mut clone = item.clone();
    clone.done = false;
    if clone
        .key
        .marker
        .as_ref()
        .map(|m| m.is_empty())
        .unwrap_or(true)
    {
        clone.key.marker = Some(generate_marker());
    }
    clone.key.line_index = 0;

    let marker = clone
        .key
        .marker
        .clone()
        .unwrap_or_else(generate_marker);
    let line = render_line(&clone)?;
    insert_line(line, marker)
}

fn insert_line(line: String, marker: String) -> Result<TodoKey> {
    let content = read_content()?;
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

    let insert_index = lines
        .iter()
        .position(|l| l.trim() == "---")
        .unwrap_or(lines.len());

    lines.insert(insert_index, line);

    let mut output = lines.join("\n");
    if content.ends_with('\n') {
        output.push('\n');
    }

    write_content(output)?;
    Ok(TodoKey {
        line_index: insert_index,
        marker: Some(marker),
    })
}

fn parse_due(text: &str) -> Option<NaiveDateTime> {
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

pub(crate) fn parse_line(line: &str, line_index: usize) -> Option<TodoItem> {
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
    let projects = capture_all_tokens(&PROJECT_RE, rest);
    let contexts = capture_all_tokens(&CONTEXT_RE, rest);
    let due = parse_due(rest);
    let recurrence = capture_token(&RECUR_RE, rest);
    let reference = capture_token(&LINK_RE, rest);
    let marker = capture_token(&ID_RE, rest);
    let note = capture_token(&NOTE_RE, rest)
        .map(|raw| unescape_note(&raw))
        .and_then(|n| normalize_note(Some(&n)));

    Some(TodoItem {
        key: TodoKey {
            line_index,
            marker,
        },
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

fn capture_all_tokens(regex: &Regex, text: &str) -> Vec<String> {
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

fn capture_token(regex: &Regex, text: &str) -> Option<String> {
    regex
        .captures(text)
        .and_then(|caps| caps.get(1).map(|m| m.as_str().trim().to_string()))
}

pub(crate) fn extract_title(rest: &str) -> String {
    const MARKERS: [&str; 16] = [" +", " @", " due:", " rec:", " [[", " ✅", " ^", " ~note:", "+", "@", "due:", "rec:", "[[", "✅", "^", "~note:"];
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

fn update_line<F>(key: &TodoKey, rewrite: F) -> Result<()>
where
    F: FnOnce(&str) -> Result<String>,
{
    let content = read_content()?;
    let mut lines: Vec<String> = content.lines().map(|line| line.to_string()).collect();
    let had_trailing_newline = content.ends_with('\n');

    let mut target_index = None;
    if let Some(marker) = &key.marker {
        target_index = find_line_by_marker(&lines, marker);
    }
    if target_index.is_none() && key.line_index < lines.len() {
        target_index = Some(key.line_index);
    }

    let index = target_index.ok_or_else(|| anyhow!(t("todo_not_found")))?;
    let updated_line = rewrite(&lines[index])
        .with_context(|| t("line_update_error").replace("{}", &(index + 1).to_string()))?;
    lines[index] = updated_line;

    let mut output = lines.join("\n");
    if had_trailing_newline {
        output.push('\n');
    }

    write_content(output)?;

    Ok(())
}

fn delete_line(key: &TodoKey) -> Result<()> {
    let content = read_content()?;
    let mut lines: Vec<String> = content.lines().map(|line| line.to_string()).collect();
    let index = key.line_index;
    if index >= lines.len() {
        bail!(t("todo_not_found"));
    }

    lines.remove(index);

    let mut output = lines.join("\n");
    output.push('\n');

    write_content(output)?;

    Ok(())
}

pub(crate) fn rewrite_line(line: &str, done: bool) -> Result<String> {
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

fn add_months(date: NaiveDate, months: i32) -> Option<NaiveDate> {
    let total_months = date.year() * 12 + (date.month0() as i32) + months;
    let new_year = total_months.div_euclid(12);
    let new_month0 = total_months.rem_euclid(12);
    let new_month = (new_month0 + 1) as u32;

    // Find last valid day of target month
    let last_day = (28..=31)
        .rev()
        .find_map(|d| NaiveDate::from_ymd_opt(new_year, new_month, d))?;

    let day = date.day().min(last_day.day());
    NaiveDate::from_ymd_opt(new_year, new_month, day)
}

pub fn next_due_date(current_due: Option<NaiveDateTime>, rule: &str) -> Option<NaiveDateTime> {
    let time = current_due.map(|d| d.time()).unwrap_or(DEFAULT_DUE_TIME);
    let mut next = current_due.map(|d| d.date()).unwrap_or_else(|| Local::now().date_naive());
    let today = Local::now().date_naive();

    loop {
        next = match rule.to_lowercase().as_str() {
            "daily" => next.checked_add_signed(chrono::Duration::days(1))?,
            "weekly" => next.checked_add_signed(chrono::Duration::days(7))?,
            "monthly" => add_months(next, 1)?,
            _ => return None,
        };

        if next > today {
            break;
        }
    }
    Some(NaiveDateTime::new(next, time))
}

pub(crate) fn render_line(item: &TodoItem) -> Result<String> {
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

fn generate_marker() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| std::time::Duration::from_secs(0))
        .as_nanos();
    let pid = std::process::id() as u128;
    let mixed = now ^ (pid << 64);
    let mut encoded = encode_base36(mixed);

    // Keep IDs short and alphanumeric (e.g., 8 chars)
    if encoded.len() > 8 {
        encoded.truncate(8);
    } else {
        while encoded.len() < 8 {
            encoded.push('0');
        }
    }

    encoded
}

pub(crate) fn encode_base36(mut value: u128) -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }

    let mut buf = Vec::new();
    while value > 0 {
        let idx = (value % 36) as usize;
        buf.push(ALPHABET[idx] as char);
        value /= 36;
    }

    buf.into_iter().rev().collect()
}

fn normalize_token(value: Option<&str>) -> Option<String> {
    value
        .map(|s| {
            let trimmed = s.trim().replace(' ', "");
            // Remove all leading + or @
            let chars = trimmed.chars();
            let mut out = String::new();
            let mut found = false;
            for c in chars {
                if !found && (c == '+' || c == '@') {
                    continue;
                }
                found = true;
                out.push(c);
            }
            out
        })
        .filter(|s| !s.is_empty())
}

fn normalize_reference(value: Option<&str>) -> Option<String> {
    value
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn normalize_note(value: Option<&str>) -> Option<String> {
    value
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn escape_note(note: &str) -> String {
    note
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

fn unescape_note(input: &str) -> String {
    let mut out = String::new();
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                match next {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    other => {
                        out.push(other);
                    }
                }
            } else {
                out.push('\\');
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn apply_completion_marker(line: &str, done: bool) -> String {
    if done {
        if let Some(m) = COMPLETION_RE.find(line) {
            // Bereits vorhanden. Prüfen, ob es nach der ID steht.
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
            // Hinzufügen
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

pub(crate) fn rewrite_due(line: &str, new_due: NaiveDateTime) -> Result<String> {
    let segment = format!("due:{}", new_due.format("%Y-%m-%dT%H:%M"));
    if DUE_RE.is_match(line) {
        Ok(DUE_RE.replace(line, segment).to_string())
    } else {
        Ok(insert_due_segment(line, &segment))
    }
}

fn insert_due_segment(line: &str, segment: &str) -> String {
    const MARKERS: [&str; 7] = [" +", " @", " rec:", " [[", " ✅", " ~note:", " ^"];
    let mut insert_at = line.len();
    for marker in MARKERS {
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
