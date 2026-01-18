use anyhow::{bail, Result};
use chrono::{Duration, Local, NaiveDate, NaiveDateTime, NaiveTime};
use reinschrift_core::{
    data, load_todos, toggle_todo, set_due_today, update_todo_details, delete_todo, add_todo,
    add_todo_full, get_backend_config, test_webdav_connection,
    TodoItem, TodoKey, SortMode, sort_items,
};
use std::io::{self, Write};

use crate::output::{OutputContext, format_id};

const DEFAULT_DUE_TIME: NaiveTime = match NaiveTime::from_hms_opt(0, 0, 0) {
    Some(t) => t,
    None => panic!("invalid time"),
};

fn parse_space_separated_tags(input: &str, prefix: char) -> Vec<String> {
    input
        .split_whitespace()
        .map(|s| s.trim_start_matches(prefix).to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn list(
    ctx: &OutputContext,
    sort: &str,
    show_all: bool,
    due_only: bool,
    project_filter: Option<&str>,
    context_filter: Option<&str>,
) -> Result<()> {
    let mut items = load_todos()?;

    // Filter completed items unless show_all
    if !show_all {
        items.retain(|item| !item.done);
    }

    // Filter by due date if due_only
    if due_only {
        let today = Local::now().date_naive();
        items.retain(|item| {
            item.due.map(|d| d.date() <= today).unwrap_or(false)
        });
    }

    // Filter by project
    if let Some(p) = project_filter {
        let p_lower = p.to_lowercase();
        items.retain(|item| {
            item.projects.iter()
                .any(|proj| proj.to_lowercase().contains(&p_lower))
        });
    }

    // Filter by context
    if let Some(c) = context_filter {
        let c_lower = c.to_lowercase();
        items.retain(|item| {
            item.contexts.iter()
                .any(|ctx| ctx.to_lowercase().contains(&c_lower))
        });
    }

    // Sort items
    let mode = SortMode::from_key(sort);
    sort_items(&mut items, mode);

    if items.is_empty() {
        ctx.info("No todos found.");
        return Ok(());
    }

    // Print with grouping based on sort mode
    match mode {
        SortMode::Topic => {
            ctx.print_todos_grouped(&items, |item| {
                format!("Topic: {}", item.projects.first().map(|s| s.as_str()).unwrap_or("No project"))
            });
        }
        SortMode::Location => {
            ctx.print_todos_grouped(&items, |item| {
                format!("Location: {}", item.contexts.first().map(|s| s.as_str()).unwrap_or("No location"))
            });
        }
        SortMode::Date => {
            ctx.print_todos(&items);
        }
    }

    Ok(())
}

pub fn add(
    ctx: &OutputContext,
    title: &str,
    project: Option<&str>,
    context: Option<&str>,
    due: Option<&str>,
    recurrence: Option<&str>,
    note: Option<&str>,
) -> Result<()> {
    // If only title is provided, use the simple add_todo function
    if project.is_none() && context.is_none() && due.is_none() && recurrence.is_none() && note.is_none() {
        let _ = add_todo(title)?;
        ctx.success(&format!("Added: {}", title));
        return Ok(());
    }

    // Otherwise, create a full TodoItem
    let today = Local::now().date_naive();
    let due_dt = if let Some(due_str) = due {
        Some(parse_date(due_str)?)
    } else {
        Some(NaiveDateTime::new(today, DEFAULT_DUE_TIME))
    };

    let item = TodoItem {
        key: TodoKey { line_index: 0, marker: None },
        title: title.to_string(),
        projects: project.map(|s| parse_space_separated_tags(s, '+')).unwrap_or_default(),
        contexts: context.map(|s| parse_space_separated_tags(s, '@')).unwrap_or_default(),
        due: due_dt,
        reference: None,
        recurrence: recurrence.map(|s| s.to_string()),
        note: note.map(|s| s.to_string()),
        done: false,
    };

    let _ = add_todo_full(&item)?;
    ctx.success(&format!("Added: {}", title));
    Ok(())
}

pub fn edit(
    ctx: &OutputContext,
    id: &str,
    title: Option<&str>,
    project: Option<&str>,
    context: Option<&str>,
    due: Option<&str>,
    recurrence: Option<&str>,
    note: Option<&str>,
) -> Result<()> {
    let items = load_todos()?;
    let mut item = find_item_by_id(&items, id)?;

    // Update fields if provided
    if let Some(t) = title {
        item.title = t.to_string();
    }
    if let Some(p) = project {
        item.projects = if p.is_empty() { Vec::new() } else { parse_space_separated_tags(p, '+') };
    }
    if let Some(c) = context {
        item.contexts = if c.is_empty() { Vec::new() } else { parse_space_separated_tags(c, '@') };
    }
    if let Some(d) = due {
        item.due = if d.is_empty() { None } else { Some(parse_date(d)?) };
    }
    if let Some(r) = recurrence {
        item.recurrence = if r.is_empty() { None } else { Some(r.to_string()) };
    }
    if let Some(n) = note {
        item.note = if n.is_empty() { None } else { Some(n.to_string()) };
    }

    update_todo_details(&item)?;
    ctx.success(&format!("Updated: {}", item.title));
    Ok(())
}

pub fn delete(ctx: &OutputContext, ids: &[String], force: bool) -> Result<()> {
    let items = load_todos()?;

    for id in ids {
        let item = find_item_by_id(&items, id)?;

        if !force {
            print!("Delete '{}' ({})? [y/N] ", item.title, format_id(&item));
            io::stdout().flush()?;
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            if !input.trim().eq_ignore_ascii_case("y") {
                ctx.info(&format!("Skipped: {}", item.title));
                continue;
            }
        }

        delete_todo(&item)?;
        ctx.success(&format!("Deleted: {}", item.title));
    }

    Ok(())
}

pub fn done(ctx: &OutputContext, ids: &[String]) -> Result<()> {
    let items = load_todos()?;

    for id in ids {
        let item = find_item_by_id(&items, id)?;
        toggle_todo(&item.key, true)?;
        ctx.success(&format!("Completed: {}", item.title));
    }

    Ok(())
}

pub fn undone(ctx: &OutputContext, ids: &[String]) -> Result<()> {
    let items = load_todos()?;

    for id in ids {
        let item = find_item_by_id(&items, id)?;
        toggle_todo(&item.key, false)?;
        ctx.success(&format!("Reopened: {}", item.title));
    }

    Ok(())
}

pub fn today(ctx: &OutputContext, id: &str) -> Result<()> {
    let items = load_todos()?;
    let item = find_item_by_id(&items, id)?;
    let new_due = set_due_today(&item.key)?;
    ctx.success(&format!("Set due date to {}: {}", new_due.date(), item.title));
    Ok(())
}

pub fn tomorrow(ctx: &OutputContext, id: &str) -> Result<()> {
    let items = load_todos()?;
    let mut item = find_item_by_id(&items, id)?;

    let tomorrow = Local::now().date_naive() + Duration::days(1);
    let due_dt = NaiveDateTime::new(tomorrow, DEFAULT_DUE_TIME);
    item.due = Some(due_dt);

    update_todo_details(&item)?;
    ctx.success(&format!("Set due date to {}: {}", tomorrow, item.title));
    Ok(())
}

pub fn search(ctx: &OutputContext, query: &str, show_all: bool) -> Result<()> {
    let mut items = load_todos()?;
    let query_lower = query.to_lowercase();

    // Filter by search query
    items.retain(|item| {
        item.title.to_lowercase().contains(&query_lower)
            || item.projects.iter().any(|p| p.to_lowercase().contains(&query_lower))
            || item.contexts.iter().any(|c| c.to_lowercase().contains(&query_lower))
            || item.note.as_ref().map(|n| n.to_lowercase().contains(&query_lower)).unwrap_or(false)
    });

    // Filter completed unless show_all
    if !show_all {
        items.retain(|item| !item.done);
    }

    if items.is_empty() {
        ctx.info(&format!("No results for '{}'", query));
        return Ok(());
    }

    ctx.info(&format!("Found {} results for '{}':", items.len(), query));
    ctx.print_todos(&items);
    Ok(())
}

pub fn config_show(ctx: &OutputContext) -> Result<()> {
    let config = get_backend_config();

    if ctx.json {
        let config_json = match &config {
            data::BackendConfig::Local(p) => {
                format!(r#"{{"type":"local","path":"{}"}}"#, p.display())
            }
            data::BackendConfig::WebDav { url, path, username, .. } => {
                format!(
                    r#"{{"type":"webdav","url":"{}","path":{},"username":{}}}"#,
                    url,
                    path.as_ref().map(|p| format!("\"{}\"", p)).unwrap_or_else(|| "null".to_string()),
                    username.as_ref().map(|u| format!("\"{}\"", u)).unwrap_or_else(|| "null".to_string())
                )
            }
        };
        println!("{}", config_json);
    } else {
        println!("Configuration:");
        match &config {
            data::BackendConfig::Local(path) => {
                println!("  Backend: Local file");
                println!("  Database path: {}", path.display());
            }
            data::BackendConfig::WebDav { url, path, username, .. } => {
                println!("  Backend: WebDAV");
                println!("  URL: {}", url);
                if let Some(p) = path {
                    println!("  Path: {}", p);
                }
                if let Some(u) = username {
                    println!("  Username: {}", u);
                }
            }
        }
    }

    Ok(())
}

pub fn config_webdav_test(ctx: &OutputContext) -> Result<()> {
    let config = get_backend_config();

    match config {
        data::BackendConfig::WebDav { url, path, username, password } => {
            ctx.info(&format!("Testing connection to {}...", url));
            match test_webdav_connection(&url, path.as_deref(), username.as_deref(), password.as_deref()) {
                Ok(()) => {
                    ctx.success("WebDAV connection successful!");
                }
                Err(e) => {
                    ctx.error(&format!("Connection failed: {}", e));
                }
            }
        }
        data::BackendConfig::Local(_) => {
            ctx.error("WebDAV is not configured. Current backend is local file.");
        }
    }

    Ok(())
}

pub fn config_set(ctx: &OutputContext, key: &str, value: &str) -> Result<()> {
    ctx.info(&format!("Setting {} = {}", key, value));
    ctx.error("Configuration setting via CLI is not yet implemented.");
    ctx.info("Please edit the preferences file directly or use the GUI.");
    Ok(())
}

// Helper functions

fn find_item_by_id(items: &[TodoItem], id: &str) -> Result<TodoItem> {
    // Try marker format (^ID)
    if let Some(marker) = id.strip_prefix('^') {
        if let Some(item) = items.iter().find(|i| i.key.marker.as_deref() == Some(marker)) {
            return Ok(item.clone());
        }
    }

    // Try line number format (#N)
    if let Some(line_str) = id.strip_prefix('#') {
        if let Ok(line_num) = line_str.parse::<usize>() {
            if let Some(item) = items.iter().find(|i| i.key.line_index == line_num) {
                return Ok(item.clone());
            }
        }
    }

    // Try as bare marker
    if let Some(item) = items.iter().find(|i| i.key.marker.as_deref() == Some(id)) {
        return Ok(item.clone());
    }

    // Try as bare line number
    if let Ok(line_num) = id.parse::<usize>() {
        if let Some(item) = items.iter().find(|i| i.key.line_index == line_num) {
            return Ok(item.clone());
        }
    }

    bail!("Todo not found: {}", id)
}

fn parse_date(s: &str) -> Result<NaiveDateTime> {
    // Try full datetime format
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M") {
        return Ok(dt);
    }

    // Try date-only format
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(NaiveDateTime::new(date, DEFAULT_DUE_TIME));
    }

    // Try special values
    let today = Local::now().date_naive();
    match s.to_lowercase().as_str() {
        "today" => Ok(NaiveDateTime::new(today, DEFAULT_DUE_TIME)),
        "tomorrow" => Ok(NaiveDateTime::new(today + Duration::days(1), DEFAULT_DUE_TIME)),
        _ => bail!("Invalid date format: {}. Expected YYYY-MM-DD", s),
    }
}
