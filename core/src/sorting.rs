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
