use std::cmp::Ordering;
use chrono::NaiveDateTime;
use crate::data::TodoItem;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SortMode {
    Topic,
    Location,
    Date,
}

impl SortMode {
    pub fn from_index(index: u32) -> Self {
        match index {
            1 => SortMode::Location,
            2 => SortMode::Date,
            _ => SortMode::Topic,
        }
    }

    pub fn to_index(self) -> u32 {
        match self {
            SortMode::Topic => 0,
            SortMode::Location => 1,
            SortMode::Date => 2,
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "location" => SortMode::Location,
            "date" => SortMode::Date,
            _ => SortMode::Topic,
        }
    }

    pub fn as_key(self) -> &'static str {
        match self {
            SortMode::Topic => "topic",
            SortMode::Location => "location",
            SortMode::Date => "date",
        }
    }
}

pub fn sort_items(items: &mut [TodoItem], mode: SortMode) {
    match mode {
        SortMode::Topic => items.sort_by(compare_by_project),
        SortMode::Location => items.sort_by(compare_by_context),
        SortMode::Date => items.sort_by(compare_by_due),
    }
}

pub fn compare_by_project(a: &TodoItem, b: &TodoItem) -> Ordering {
    compare_option_str(a.projects.first().map(|s| s.as_str()), b.projects.first().map(|s| s.as_str()))
        .then_with(|| lexical_order(&a.title, &b.title))
        .then_with(|| compare_option_str(a.contexts.first().map(|s| s.as_str()), b.contexts.first().map(|s| s.as_str())))
}

pub fn compare_by_context(a: &TodoItem, b: &TodoItem) -> Ordering {
    compare_option_str(a.contexts.first().map(|s| s.as_str()), b.contexts.first().map(|s| s.as_str()))
        .then_with(|| lexical_order(&a.title, &b.title))
        .then_with(|| compare_option_str(a.projects.first().map(|s| s.as_str()), b.projects.first().map(|s| s.as_str())))
}

pub fn compare_by_due(a: &TodoItem, b: &TodoItem) -> Ordering {
    compare_option_datetime(a.due, b.due)
        .then_with(|| compare_by_project(a, b))
}

pub fn compare_option_str(a: Option<&str>, b: Option<&str>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => lexical_order(a, b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

pub fn compare_option_datetime(a: Option<NaiveDateTime>, b: Option<NaiveDateTime>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => Ordering::Equal,
    }
}

pub fn lexical_order(a: &str, b: &str) -> Ordering {
    a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::TodoItem;
    use chrono::{NaiveDate, NaiveDateTime};

    fn make_item(title: &str, project: Option<&str>, context: Option<&str>, due: Option<NaiveDateTime>) -> TodoItem {
        TodoItem {
            key: crate::data::TodoKey { line_index: 0, marker: None },
            title: title.to_string(),
            projects: project.map(|p| vec![p.to_string()]).unwrap_or_default(),
            contexts: context.map(|c| vec![c.to_string()]).unwrap_or_default(),
            due,
            myday: None,
            reference: None,
            recurrence: None,
            note: None,
            done: false,
        }
    }

    #[test]
    fn test_sort_mode_from_index_and_key() {
        assert_eq!(SortMode::from_index(0), SortMode::Topic);
        assert_eq!(SortMode::from_index(1), SortMode::Location);
        assert_eq!(SortMode::from_index(2), SortMode::Date);
        assert_eq!(SortMode::from_key("topic"), SortMode::Topic);
        assert_eq!(SortMode::from_key("location"), SortMode::Location);
        assert_eq!(SortMode::from_key("date"), SortMode::Date);
    }

    #[test]
    fn test_compare_by_project() {
        let a = make_item("A", Some("work"), None, None);
        let b = make_item("B", Some("home"), None, None);
        assert_eq!(compare_by_project(&a, &b), "work".cmp("home"));
    }

    #[test]
    fn test_compare_by_context() {
        let a = make_item("A", None, Some("office"), None);
        let b = make_item("B", None, Some("home"), None);
        assert_eq!(compare_by_context(&a, &b), "office".cmp("home"));
    }

    #[test]
    fn test_compare_by_due() {
        let dt1 = NaiveDate::from_ymd_opt(2026, 1, 20).unwrap().and_hms_opt(9, 0, 0).unwrap();
        let dt2 = NaiveDate::from_ymd_opt(2026, 1, 21).unwrap().and_hms_opt(9, 0, 0).unwrap();
        let a = make_item("A", Some("work"), None, Some(dt1));
        let b = make_item("B", Some("work"), None, Some(dt2));
        assert_eq!(compare_by_due(&a, &b), dt1.cmp(&dt2));
    }

    #[test]
    fn test_compare_option_str() {
        assert_eq!(compare_option_str(Some("a"), Some("b")), "a".cmp("b"));
        assert_eq!(compare_option_str(Some("a"), None), std::cmp::Ordering::Less);
        assert_eq!(compare_option_str(None, Some("b")), std::cmp::Ordering::Greater);
        assert_eq!(compare_option_str(None, None), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_compare_option_datetime() {
        let dt1 = NaiveDate::from_ymd_opt(2026, 1, 20).unwrap().and_hms_opt(9, 0, 0).unwrap();
        let dt2 = NaiveDate::from_ymd_opt(2026, 1, 21).unwrap().and_hms_opt(9, 0, 0).unwrap();
        assert_eq!(compare_option_datetime(Some(dt1), Some(dt2)), dt1.cmp(&dt2));
        assert_eq!(compare_option_datetime(Some(dt1), None), std::cmp::Ordering::Greater);
        assert_eq!(compare_option_datetime(None, Some(dt2)), std::cmp::Ordering::Less);
        assert_eq!(compare_option_datetime(None, None), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_lexical_order_case_insensitive() {
        assert_eq!(lexical_order("abc", "ABC"), std::cmp::Ordering::Equal);
        assert_eq!(lexical_order("abc", "abd"), std::cmp::Ordering::Less);
    }
}
