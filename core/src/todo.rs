//! Business logic for todo operations.

use anyhow::{anyhow, bail, Context, Result};
use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, Timelike};

use crate::i18n::t;
use crate::parser::{find_line_by_marker, parse_line};
use crate::renderer::{render_line, rewrite_due, rewrite_line, rewrite_myday};
use crate::storage::{read_content, read_content_with_fingerprint, write_content, write_content_checked};
use crate::types::{TodoItem, TodoKey, DEFAULT_DUE_TIME};
use crate::undo::push_undo;
use crate::util::{generate_marker, normalize_token};

/// Target for due-date operations, shared by single and batch variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DueTarget {
    Today,
    Tomorrow,
    Weekend,
    Sometime,
}

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

/// Core helper: read file with fingerprint, push undo, apply rewrite, write with conflict check.
/// The `rewrite` closure receives the lines and must return the modified output string.
fn mutate_file<F>(description: &str, rewrite: F) -> Result<()>
where
    F: FnOnce(&mut Vec<String>, bool) -> Result<String>,
{
    let snapshot = read_content_with_fingerprint()?;
    push_undo(snapshot.content.clone(), description.to_string());

    let mut lines: Vec<String> = snapshot.content.lines().map(|l| l.to_string()).collect();
    let had_trailing_newline = snapshot.content.ends_with('\n');

    let output = rewrite(&mut lines, had_trailing_newline)?;
    write_content_checked(output, &snapshot.fingerprint)?;
    Ok(())
}

/// Find the target line index for a given key.
fn resolve_key(lines: &[String], key: &TodoKey) -> Result<usize> {
    let mut target_index = None;
    if let Some(marker) = &key.marker {
        target_index = find_line_by_marker(lines, marker);
    }
    if target_index.is_none() && key.line_index < lines.len() {
        target_index = Some(key.line_index);
    }
    target_index.ok_or_else(|| anyhow!(t("todo_not_found")))
}

/// Toggle a todo's completion state.
pub fn toggle_todo(key: &TodoKey, done: bool) -> Result<()> {
    let key = key.clone();
    let action = if done { "complete" } else { "reopen" };
    mutate_file(action, |lines, had_trailing_newline| {
        let index = resolve_key(lines, &key)?;
        let updated_line = rewrite_line(&lines[index], done)
            .with_context(|| t("line_update_error").replace("{}", &(index + 1).to_string()))?;
        lines[index] = updated_line;

        let mut output = lines.join("\n");
        if had_trailing_newline {
            output.push('\n');
        }
        Ok(output)
    })
}

/// Compute the due date for "today" (smart time selection).
fn due_today_dt(current_due: Option<NaiveDateTime>) -> NaiveDateTime {
    let now = Local::now();
    let today = now.date_naive();

    let time = match current_due {
        Some(dt) if dt.date() > today => NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
        _ => {
            let current_hour = now.hour();
            if current_hour < 8 {
                NaiveTime::from_hms_opt(12, 0, 0).unwrap()
            } else if current_hour < 14 {
                NaiveTime::from_hms_opt(18, 0, 0).unwrap()
            } else {
                NaiveTime::from_hms_opt(18, 0, 0).unwrap()
            }
        }
    };

    NaiveDateTime::new(today, time)
}

/// Compute the due date for "tomorrow" (noon).
fn due_tomorrow_dt() -> NaiveDateTime {
    let tomorrow = Local::now().date_naive() + chrono::Duration::days(1);
    NaiveDateTime::new(tomorrow, NaiveTime::from_hms_opt(12, 0, 0).unwrap())
}

/// Compute the due date for the next Saturday (noon).
fn due_weekend_dt() -> NaiveDateTime {
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
    NaiveDateTime::new(target_date, NaiveTime::from_hms_opt(12, 0, 0).unwrap())
}

/// Compute the due date for "sometime" (far future).
fn due_sometime_dt() -> NaiveDateTime {
    let sometime = NaiveDate::from_ymd_opt(9999, 12, 31).unwrap();
    NaiveDateTime::new(sometime, DEFAULT_DUE_TIME)
}

/// Set a todo's due date to today (smart time selection).
pub fn set_due_today(key: &TodoKey, current_due: Option<NaiveDateTime>) -> Result<NaiveDateTime> {
    let due_dt = due_today_dt(current_due);
    update_line(key, "set due today", |line| rewrite_due(line, due_dt))?;
    Ok(due_dt)
}

/// Set a todo's due date to tomorrow at noon.
pub fn set_due_tomorrow(key: &TodoKey) -> Result<NaiveDateTime> {
    let due_dt = due_tomorrow_dt();
    update_line(key, "set due tomorrow", |line| rewrite_due(line, due_dt))?;
    Ok(due_dt)
}

/// Set a todo's due date to the next Saturday.
pub fn set_due_weekend(key: &TodoKey) -> Result<NaiveDateTime> {
    let due_dt = due_weekend_dt();
    update_line(key, "set due weekend", |line| rewrite_due(line, due_dt))?;
    Ok(due_dt)
}

/// Set a todo's due date to "sometime" (far future).
pub fn set_due_sometime(key: &TodoKey) -> Result<NaiveDateTime> {
    let due_dt = due_sometime_dt();
    update_line(key, "set due sometime", |line| rewrite_due(line, due_dt))?;
    Ok(due_dt)
}

/// Add a todo to "my day" (today's plan).
pub fn set_myday_today(key: &TodoKey) -> Result<()> {
    update_line(key, "add to my day", |line| rewrite_myday(line, true))
}

/// Remove a todo from "my day".
pub fn unset_myday(key: &TodoKey) -> Result<()> {
    update_line(key, "remove from my day", |line| rewrite_myday(line, false))
}

/// Update a todo's details (full re-render).
pub fn update_todo_details(item: &TodoItem) -> Result<()> {
    let rendered = render_line(item)?;
    update_line(&item.key, "edit", |_| Ok(rendered))
}

/// Delete a todo.
pub fn delete_todo(item: &TodoItem) -> Result<()> {
    delete_line(&item.key)
}

/// Toggle multiple todos in a single read/write pass. Unresolvable keys are
/// skipped; returns the number of toggled items.
///
/// Completing a recurring task mirrors the single-item behavior: an overdue
/// occurrence is rescheduled to today before completion, and the next
/// occurrence is appended within the same write.
pub fn toggle_todos(keys: &[TodoKey], done: bool) -> Result<usize> {
    if keys.is_empty() {
        return Ok(0);
    }
    let keys = keys.to_vec();
    let action = if done { "complete" } else { "reopen" };
    let mut count = 0usize;
    mutate_file(action, |lines, had_trailing_newline| {
        let now = Local::now().naive_local();
        let mut spawned: Vec<String> = Vec::new();
        for key in &keys {
            let Ok(index) = resolve_key(lines, key) else { continue };
            let item = parse_line(&lines[index], index);

            let mut updated = lines[index].clone();
            let spawns_recurrence = done
                && item
                    .as_ref()
                    .map(|i| i.recurrence.is_some() && !i.done)
                    .unwrap_or(false);
            if spawns_recurrence {
                let item = item.as_ref().unwrap();
                // Reschedule overdue recurring tasks to today before completing.
                if let Some(due) = item.due {
                    if due < now {
                        let today_due =
                            NaiveDateTime::new(Local::now().date_naive(), due.time());
                        updated = rewrite_due(&updated, today_due)?;
                    }
                }
            }

            let Ok(toggled) = rewrite_line(&updated, done) else { continue };
            lines[index] = toggled;
            count += 1;

            if spawns_recurrence {
                let item = item.as_ref().unwrap();
                if let Some(next_due) =
                    next_due_date(item.due, item.recurrence.as_deref().unwrap_or_default())
                {
                    let mut next_item = item.clone();
                    next_item.key = TodoKey {
                        line_index: 0,
                        marker: Some(generate_marker()),
                    };
                    next_item.done = false;
                    next_item.due = Some(next_due);
                    spawned.push(render_line(&next_item)?);
                }
            }
        }

        // Append next occurrences only after all index-based edits, so the
        // insertions cannot shift lines that keys still need to resolve to.
        if !spawned.is_empty() {
            let insert_index = lines
                .iter()
                .position(|l| l.trim() == "---")
                .unwrap_or(lines.len());
            for (offset, line) in spawned.into_iter().enumerate() {
                lines.insert(insert_index + offset, line);
            }
        }

        let mut output = lines.join("\n");
        if had_trailing_newline {
            output.push('\n');
        }
        Ok(output)
    })?;
    Ok(count)
}

/// Set the due date of multiple todos in a single read/write pass.
/// Unresolvable keys are skipped; returns the number of updated items.
pub fn set_due_batch(keys: &[TodoKey], target: DueTarget) -> Result<usize> {
    if keys.is_empty() {
        return Ok(0);
    }
    let keys = keys.to_vec();
    let mut count = 0usize;
    mutate_file("set due", |lines, had_trailing_newline| {
        for key in &keys {
            let Ok(index) = resolve_key(lines, key) else { continue };
            let due_dt = match target {
                DueTarget::Today => {
                    // Preserve the smart-time selection by passing the item's
                    // current due date, like the single-item variant.
                    let current_due = parse_line(&lines[index], index).and_then(|i| i.due);
                    due_today_dt(current_due)
                }
                DueTarget::Tomorrow => due_tomorrow_dt(),
                DueTarget::Weekend => due_weekend_dt(),
                DueTarget::Sometime => due_sometime_dt(),
            };
            let Ok(updated) = rewrite_due(&lines[index], due_dt) else { continue };
            lines[index] = updated;
            count += 1;
        }

        let mut output = lines.join("\n");
        if had_trailing_newline {
            output.push('\n');
        }
        Ok(output)
    })?;
    Ok(count)
}

/// Add a project and/or context to multiple todos in a single read/write
/// pass. Items that already carry the token are left untouched. Returns the
/// number of changed items.
pub fn assign_project_context_batch(
    keys: &[TodoKey],
    project: Option<&str>,
    context: Option<&str>,
) -> Result<usize> {
    let project = normalize_token(project);
    let context = normalize_token(context);
    if keys.is_empty() || (project.is_none() && context.is_none()) {
        return Ok(0);
    }
    let keys = keys.to_vec();
    let mut count = 0usize;
    mutate_file("assign", |lines, had_trailing_newline| {
        for key in &keys {
            let Ok(index) = resolve_key(lines, key) else { continue };
            let Some(mut item) = parse_line(&lines[index], index) else { continue };

            let mut changed = false;
            if let Some(p) = &project {
                let exists = item
                    .projects
                    .iter()
                    .any(|existing| normalize_token(Some(existing)).as_deref() == Some(p));
                if !exists {
                    item.projects.push(p.clone());
                    changed = true;
                }
            }
            if let Some(c) = &context {
                let exists = item
                    .contexts
                    .iter()
                    .any(|existing| normalize_token(Some(existing)).as_deref() == Some(c));
                if !exists {
                    item.contexts.push(c.clone());
                    changed = true;
                }
            }

            if changed {
                let Ok(rendered) = render_line(&item) else { continue };
                lines[index] = rendered;
                count += 1;
            }
        }

        let mut output = lines.join("\n");
        if had_trailing_newline {
            output.push('\n');
        }
        Ok(output)
    })?;
    Ok(count)
}

/// Delete multiple todos in a single read/write pass. Keys are resolved
/// marker-first against the same snapshot, then removed bottom-up so earlier
/// removals cannot shift indices that later removals still rely on.
pub fn delete_todos(keys: &[TodoKey]) -> Result<usize> {
    if keys.is_empty() {
        return Ok(0);
    }
    let keys = keys.to_vec();
    let mut count = 0usize;
    mutate_file("delete", |lines, _| {
        let mut indices: Vec<usize> = keys
            .iter()
            .filter_map(|key| resolve_key(lines, key).ok())
            .collect();
        indices.sort_unstable();
        indices.dedup();
        for index in indices.into_iter().rev() {
            lines.remove(index);
            count += 1;
        }

        let mut output = lines.join("\n");
        output.push('\n');
        Ok(output)
    })?;
    Ok(count)
}

/// Return all unique non-empty titles, sorted by descending occurrence count
/// (ties broken alphabetically). Loads from disk; callers with cached items
/// should use a local helper instead to avoid IO per keystroke.
pub fn unique_titles_by_frequency() -> Result<Vec<String>> {
    use std::collections::HashMap;
    let items = load_todos()?;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for item in &items {
        let t = item.title.trim();
        if !t.is_empty() {
            *counts.entry(t.to_string()).or_insert(0) += 1;
        }
    }
    let mut titles: Vec<(String, usize)> = counts.into_iter().collect();
    titles.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(titles.into_iter().map(|(t, _)| t).collect())
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
    let snapshot = read_content_with_fingerprint()?;
    push_undo(snapshot.content.clone(), "add".to_string());

    let mut lines: Vec<String> = snapshot.content.lines().map(|l| l.to_string()).collect();

    let insert_index = lines
        .iter()
        .position(|l| l.trim() == "---")
        .unwrap_or(lines.len());

    lines.insert(insert_index, line);

    let mut output = lines.join("\n");
    if snapshot.content.ends_with('\n') {
        output.push('\n');
    }

    write_content_checked(output, &snapshot.fingerprint)?;
    Ok(TodoKey {
        line_index: insert_index,
        marker: Some(marker),
    })
}

/// Update a specific line using a rewrite function.
fn update_line<F>(key: &TodoKey, description: &str, rewrite: F) -> Result<()>
where
    F: FnOnce(&str) -> Result<String>,
{
    let key = key.clone();
    mutate_file(description, |lines, had_trailing_newline| {
        let index = resolve_key(lines, &key)?;
        let updated_line = rewrite(&lines[index])
            .with_context(|| t("line_update_error").replace("{}", &(index + 1).to_string()))?;
        lines[index] = updated_line;

        let mut output = lines.join("\n");
        if had_trailing_newline {
            output.push('\n');
        }
        Ok(output)
    })
}

/// Delete a line by its key.
fn delete_line(key: &TodoKey) -> Result<()> {
    let key = key.clone();
    mutate_file("delete", |lines, _| {
        let index = key.line_index;
        if index >= lines.len() {
            bail!(t("todo_not_found"));
        }

        lines.remove(index);

        let mut output = lines.join("\n");
        output.push('\n');
        Ok(output)
    })
}

/// Undo the most recent mutation by restoring the previous file content.
/// Returns the description of the undone action, or None if the stack is empty.
pub fn undo() -> Result<Option<String>> {
    let entry = match crate::undo::pop_undo() {
        Some(e) => e,
        None => return Ok(None),
    };
    write_content(entry.content)?;
    Ok(Some(entry.description))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::set_todo_path;
    use crate::undo::{can_undo, pop_undo};
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// The todo path and undo stack are process-global, so file-backed tests
    /// must not run concurrently.
    fn file_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Write `content` to a fresh temp file, point the backend at it and
    /// drain the undo stack.
    fn setup(content: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("reinschrift_todo_test_{}.md", generate_marker()));
        std::fs::write(&path, content).expect("write temp todo file");
        set_todo_path(path.clone());
        while pop_undo().is_some() {}
        path
    }

    fn read(path: &PathBuf) -> String {
        std::fs::read_to_string(path).expect("read temp todo file")
    }

    fn key(marker: &str) -> TodoKey {
        TodoKey {
            // Deliberately bogus index: resolution must be marker-first.
            line_index: 999,
            marker: Some(marker.to_string()),
        }
    }

    #[test]
    fn delete_todos_resolves_markers_and_removes_bottom_up() {
        let _guard = file_lock();
        let path = setup(
            "- [ ] Eins ^aaa1\n- [ ] Zwei ^bbb2\n- [ ] Drei ^ccc3\n- [ ] Vier ^ddd4\n",
        );

        let count = delete_todos(&[key("ccc3"), key("aaa1")]).expect("delete ok");
        assert_eq!(count, 2);
        assert_eq!(read(&path), "- [ ] Zwei ^bbb2\n- [ ] Vier ^ddd4\n");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn delete_todos_by_index_only_is_order_safe() {
        let _guard = file_lock();
        let path = setup("- [ ] Eins\n- [ ] Zwei\n- [ ] Drei\n");

        // Indices 0 and 2 given ascending: naive in-order removal would
        // delete "Eins" and then the *shifted* line after it.
        let keys = [
            TodoKey { line_index: 0, marker: None },
            TodoKey { line_index: 2, marker: None },
        ];
        let count = delete_todos(&keys).expect("delete ok");
        assert_eq!(count, 2);
        assert_eq!(read(&path), "- [ ] Zwei\n");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn toggle_todos_flips_multiple_and_skips_bad_keys() {
        let _guard = file_lock();
        let path = setup("- [ ] Eins ^aaa1\n- [ ] Zwei ^bbb2\n");

        let count =
            toggle_todos(&[key("aaa1"), key("bbb2"), key("missing")], true).expect("toggle ok");
        assert_eq!(count, 2);
        let content = read(&path);
        assert_eq!(content.matches("- [x]").count(), 2);
        assert_eq!(content.matches('✅').count(), 2);

        let count = toggle_todos(&[key("aaa1")], false).expect("reopen ok");
        assert_eq!(count, 1);
        let content = read(&path);
        assert!(content.contains("- [ ] Eins"));
        assert!(content.contains("- [x] Zwei"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn toggle_todos_spawns_next_occurrence_for_recurring() {
        let _guard = file_lock();
        let path = setup("- [ ] Gießen due:2020-01-01T09:00 rec:daily ^aaa1\n- [ ] Andere ^bbb2\n");

        let count = toggle_todos(&[key("aaa1")], true).expect("toggle ok");
        assert_eq!(count, 1);
        let content = read(&path);
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3, "next occurrence appended: {content}");

        // Overdue occurrence is rescheduled to today and completed.
        let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
        assert!(lines[0].starts_with("- [x] Gießen"), "{}", lines[0]);
        assert!(lines[0].contains(&format!("due:{today}T09:00")), "{}", lines[0]);

        // Spawned occurrence is open, in the future, with a fresh marker.
        let spawned = lines[2];
        assert!(spawned.starts_with("- [ ] Gießen"), "{spawned}");
        assert!(spawned.contains("rec:daily"), "{spawned}");
        assert!(!spawned.contains("^aaa1"), "{spawned}");
        let tomorrow = (Local::now().date_naive() + chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        assert!(spawned.contains(&format!("due:{tomorrow}T09:00")), "{spawned}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn set_due_batch_updates_all_targets() {
        let _guard = file_lock();
        let path = setup("- [ ] Eins ^aaa1\n- [ ] Zwei due:2020-01-01T08:00 ^bbb2\n");

        let count = set_due_batch(&[key("aaa1"), key("bbb2")], DueTarget::Tomorrow)
            .expect("set due ok");
        assert_eq!(count, 2);
        let tomorrow = (Local::now().date_naive() + chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let content = read(&path);
        assert_eq!(content.matches(&format!("due:{tomorrow}T12:00")).count(), 2);

        let count = set_due_batch(&[key("aaa1")], DueTarget::Sometime).expect("set due ok");
        assert_eq!(count, 1);
        assert!(read(&path).contains("due:9999-12-31T00:00"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn assign_batch_adds_without_duplicating() {
        let _guard = file_lock();
        let path = setup("- [ ] Eins +steuer ^aaa1\n- [ ] Zwei ^bbb2\n");

        let count = assign_project_context_batch(
            &[key("aaa1"), key("bbb2")],
            Some("steuer"),
            Some("buero"),
        )
        .expect("assign ok");
        // "Eins" only gains the context, "Zwei" gains both — both count as changed.
        assert_eq!(count, 2);
        let content = read(&path);
        assert_eq!(content.matches("+steuer").count(), 2);
        assert_eq!(content.matches("@buero").count(), 2);

        // Re-assigning the same tokens changes nothing.
        let count = assign_project_context_batch(&[key("aaa1"), key("bbb2")], Some("steuer"), Some("buero"))
            .expect("assign ok");
        assert_eq!(count, 0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn batch_ops_push_a_single_undo_entry() {
        let _guard = file_lock();
        let original = "- [ ] Eins ^aaa1\n- [ ] Zwei ^bbb2\n- [ ] Drei ^ccc3\n";
        let path = setup(original);

        let count = delete_todos(&[key("aaa1"), key("ccc3")]).expect("delete ok");
        assert_eq!(count, 2);
        assert!(can_undo());

        let description = undo().expect("undo ok");
        assert_eq!(description.as_deref(), Some("delete"));
        assert_eq!(read(&path), original);
        assert!(!can_undo(), "exactly one undo entry per batch");
        std::fs::remove_file(&path).ok();
    }
}
