//! Due date reminder logic.

use chrono::{Datelike, Local, NaiveDate};

use crate::types::TodoItem;

/// Sentinel date used for "sometime" — year 9999.
const SOMETIME_YEAR: i32 = 9999;

/// Returns not-done items whose due date falls within the reminder window.
/// Excludes items with the year-9999 "sometime" sentinel.
pub fn get_due_reminders(todos: &[TodoItem], remind_before_minutes: i64) -> Vec<&TodoItem> {
    let now = Local::now().naive_local();
    let window_end = now + chrono::Duration::minutes(remind_before_minutes);

    todos
        .iter()
        .filter(|item| {
            if item.done {
                return false;
            }
            if let Some(due) = item.due {
                if due.date().year() == SOMETIME_YEAR {
                    return false;
                }
                due <= window_end
            } else {
                false
            }
        })
        .collect()
}

/// Returns the count and first 5 titles of overdue (past-due) items.
pub fn overdue_summary(todos: &[TodoItem]) -> (usize, Vec<String>) {
    let today = Local::now().date_naive();

    let overdue: Vec<&TodoItem> = todos
        .iter()
        .filter(|item| {
            if item.done {
                return false;
            }
            if let Some(due) = item.due {
                let due_date = due.date();
                due_date < today && due_date != NaiveDate::from_ymd_opt(SOMETIME_YEAR, 12, 31).unwrap()
            } else {
                false
            }
        })
        .collect();

    let count = overdue.len();
    let titles: Vec<String> = overdue.iter().take(5).map(|item| item.title.clone()).collect();
    (count, titles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{TodoItem, TodoKey};
    use chrono::{NaiveDateTime, NaiveTime};

    fn make_item(title: &str, due: Option<NaiveDateTime>, done: bool) -> TodoItem {
        TodoItem {
            key: TodoKey { line_index: 0, marker: None },
            title: title.to_string(),
            projects: vec![],
            contexts: vec![],
            due,
            reference: None,
            recurrence: None,
            note: None,
            done,
        }
    }

    #[test]
    fn test_overdue_excludes_done_and_sometime() {
        let yesterday = Local::now().date_naive() - chrono::Duration::days(1);
        let sometime = NaiveDate::from_ymd_opt(9999, 12, 31).unwrap();

        let items = vec![
            make_item("Overdue", Some(NaiveDateTime::new(yesterday, NaiveTime::from_hms_opt(12, 0, 0).unwrap())), false),
            make_item("Done", Some(NaiveDateTime::new(yesterday, NaiveTime::from_hms_opt(12, 0, 0).unwrap())), true),
            make_item("Sometime", Some(NaiveDateTime::new(sometime, NaiveTime::from_hms_opt(0, 0, 0).unwrap())), false),
        ];

        let (count, titles) = overdue_summary(&items);
        assert_eq!(count, 1);
        assert_eq!(titles, vec!["Overdue"]);
    }
}
