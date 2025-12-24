use std::{env, fs};
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{Local, NaiveDate};
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::blocking::Client;

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
    let configured = env::var("TODOS_DB_PATH").unwrap_or_else(|_| {
        let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join("TodosDatenbank.md").to_string_lossy().to_string()
    });
    Mutex::new(BackendConfig::Local(PathBuf::from(configured)))
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
    let configured = env::var("TODOS_DB_PATH").unwrap_or_else(|_| {
        let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join("TodosDatenbank.md").to_string_lossy().to_string()
    });
    Mutex::new(PathBuf::from(configured))
});

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

fn read_content() -> Result<String> {
    let config = get_backend_config();
    match config {
        BackendConfig::Local(path) => {
             fs::read_to_string(&path)
                .with_context(|| format!("Konnte {} nicht lesen", path.display()))
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
            bail!("Verbindung zu '{}' fehlgeschlagen: {}", full_url, e);
        }
    }
}

fn write_content(content: String) -> Result<()> {
    let config = get_backend_config();
    match config {
        BackendConfig::Local(path) => {
             fs::write(&path, content)
                .with_context(|| format!("Konnte {} nicht schreiben", path.display()))
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

    let index = target_index.ok_or_else(|| anyhow!("Konnte To-do in der Datei nicht finden"))?;
    let updated_line = rewrite_line(&lines[index], done)
        .with_context(|| format!("Konnte Zeile {} nicht aktualisieren", index + 1))?;
    lines[index] = updated_line;

    let mut output = lines.join("\n");
    if had_trailing_newline {
        output.push('\n');
    }

    write_content(output)?;

    Ok(())
}

pub fn set_due_today(key: &TodoKey) -> Result<NaiveDate> {
    let today = Local::now().date_naive();
    update_line(key, |line| rewrite_due(line, today))?;
    Ok(today)
}

pub fn update_todo_details(item: &TodoItem) -> Result<()> {
    let rendered = render_line(item)?;
    update_line(&item.key, |_| Ok(rendered))
}

pub fn add_todo(title: &str) -> Result<()> {
    let title = title.trim();
    if title.is_empty() {
        bail!("Titel darf nicht leer sein");
    }

    let content = read_content()?;
    let mut lines: Vec<String> = content.lines().map(|line| line.to_string()).collect();

    let insert_index = lines
        .iter()
        .position(|line| line.trim() == "---")
        .unwrap_or(lines.len());
    let today = Local::now().date_naive();
    lines.insert(
        insert_index,
        format!("- [ ] {} due:{}", title, today.format("%Y-%m-%d")),
    );

    let mut output = lines.join("\n");
    if content.ends_with('\n') {
        output.push('\n');
    }

    write_content(output)?;

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

    let index = target_index.ok_or_else(|| anyhow!("Konnte To-do in der Datei nicht finden"))?;
    let updated_line = rewrite(&lines[index])
        .with_context(|| format!("Konnte Zeile {} nicht aktualisieren", index + 1))?;
    lines[index] = updated_line;

    let mut output = lines.join("\n");
    if had_trailing_newline {
        output.push('\n');
    }

    write_content(output)?;

    Ok(())
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

fn render_line(item: &TodoItem) -> Result<String> {
    let title = item.title.trim();
    if title.is_empty() {
        bail!("Titel darf nicht leer sein");
    }

    let checkbox = if item.done { "- [x]" } else { "- [ ]" };
    let mut parts = vec![format!("{checkbox} {title}")];

    if let Some(project) = normalize_token(item.project.as_deref()) {
        parts.push(format!("+{project}"));
    }
    if let Some(context) = normalize_token(item.context.as_deref()) {
        parts.push(format!("@{context}"));
    }
    if let Some(due) = item.due {
        parts.push(format!("due:{}", due.format("%Y-%m-%d")));
    }
    if let Some(reference) = normalize_reference(item.reference.as_deref()) {
        parts.push(format!("[[{reference}]]"));
    }
    if let Some(marker) = &item.key.marker {
        if !marker.is_empty() {
            parts.push(format!("^{marker}"));
        }
    }

    let line = parts.join(" ");
    Ok(apply_completion_marker(&line, item.done))
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

fn apply_completion_marker(line: &str, done: bool) -> String {
    if done {
        if COMPLETION_RE.is_match(line) {
            line.to_string()
        } else {
            // Add completion date if not present
            let today = Local::now().date_naive().format("%Y-%m-%d");
            format!("{line} ✅ {today}")
        }
    } else {
        COMPLETION_RE.replace(line, "").to_string()
    }
}

fn rewrite_due(line: &str, new_due: NaiveDate) -> Result<String> {
    let segment = format!("due:{}", new_due.format("%Y-%m-%d"));
    if DUE_RE.is_match(line) {
        Ok(DUE_RE.replace(line, segment).to_string())
    } else {
        Ok(insert_due_segment(line, &segment))
    }
}

fn insert_due_segment(line: &str, segment: &str) -> String {
    const MARKERS: [&str; 5] = [" +", " @", " [[", " ✅", " ^"];
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
