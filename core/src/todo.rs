//! Business logic for todo operations.

use anyhow::{anyhow, bail, Context, Result};
use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, Timelike};

use crate::i18n::t;
use crate::parser::{find_line_by_marker, parse_line};
use crate::renderer::{render_line, rewrite_due, rewrite_line};
use crate::storage::{read_content, write_content};
use crate::types::{TodoItem, TodoKey, DEFAULT_DUE_TIME};
use crate::util::generate_marker;

/// Load all todos from the configured backend.
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

/// Toggle a todo's completion state.
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

/// Set a todo's due date to today (smart time selection).
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

/// Set a todo's due date to tomorrow at noon.
pub fn set_due_tomorrow(key: &TodoKey) -> Result<NaiveDateTime> {
    let tomorrow = Local::now().date_naive() + chrono::Duration::days(1);
    let time = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
    let due_dt = NaiveDateTime::new(tomorrow, time);
    update_line(key, |line| rewrite_due(line, due_dt))?;
    Ok(due_dt)
}

/// Set a todo's due date to the next Saturday.
pub fn set_due_weekend(key: &TodoKey) -> Result<NaiveDateTime> {
    let today = Local::now().date_naive();
    let weekday = today.weekday().num_days_from_monday(); // 0=Mon, 5=Sat, 6=Sun

    let days_until_saturday: i64 = if weekday == 5 {
        7 // Today is Saturday, go to next Saturday
    } else if weekday == 6 {
        6 // Today is Sunday, go to next Saturday
    } else {
        (5 - weekday) as i64 // Monday-Friday: days until this Saturday
    };

    let target_date = today + chrono::Duration::days(days_until_saturday);
    let time = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
    let due_dt = NaiveDateTime::new(target_date, time);
    update_line(key, |line| rewrite_due(line, due_dt))?;
    Ok(due_dt)
}

/// Set a todo's due date to "sometime" (far future).
pub fn set_due_sometime(key: &TodoKey) -> Result<NaiveDateTime> {
    let sometime = NaiveDate::from_ymd_opt(9999, 12, 31).unwrap();
    let due_dt = NaiveDateTime::new(sometime, DEFAULT_DUE_TIME);
    update_line(key, |line| rewrite_due(line, due_dt))?;
    Ok(due_dt)
}

/// Update a todo's details (full re-render).
pub fn update_todo_details(item: &TodoItem) -> Result<()> {
    let rendered = render_line(item)?;
    update_line(&item.key, |_| Ok(rendered))
}

/// Delete a todo.
pub fn delete_todo(item: &TodoItem) -> Result<()> {
    delete_line(&item.key)
}

/// Add a new todo with just a title.
pub fn add_todo(title: &str) -> Result<TodoKey> {
    let title = title.trim();
    if title.is_empty() {
        bail!(t("title_empty_error"));
    }
    let today = Local::now().date_naive();
    let due_dt = NaiveDateTime::new(today, DEFAULT_DUE_TIME);
    let marker = generate_marker();
    let line = format!(
        "- [ ] {} due:{} ^{}",
        title,
        due_dt.format("%Y-%m-%dT%H:%M"),
        marker
    );
    insert_line(line, marker)
}

/// Add a new todo from a full TodoItem.
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

/// Insert a new line before the "---" separator (or at end).
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

/// Update a specific line using a rewrite function.
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

/// Delete a line by its key.
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

/// Add months to a date, handling month-end edge cases.
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

/// Calculate the next due date based on recurrence rule.
pub fn next_due_date(current_due: Option<NaiveDateTime>, rule: &str) -> Option<NaiveDateTime> {
    let time = current_due.map(|d| d.time()).unwrap_or(DEFAULT_DUE_TIME);
    let mut next = current_due
        .map(|d| d.date())
        .unwrap_or_else(|| Local::now().date_naive());
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
