//! List filters shared by GUI and CLI (the webapp mirrors them in
//! `webapp/app/services/filters.py`).
//!
//! A filter narrows the list by due date range, projects and places. The
//! bounded ranges count from today and include overdue tasks: "within 7 days"
//! is everything that has to be done by a week from today. Tasks due
//! "sometime" never fall into a bounded range.

use chrono::{Datelike, Duration, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::types::TodoItem;

/// Which due dates to show.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DueRange {
    /// No restriction.
    #[default]
    Any,
    /// Due before today.
    Overdue,
    /// Due today or earlier.
    Today,
    /// Due tomorrow or earlier.
    Tomorrow,
    /// Due within the next 7 days (or earlier).
    Week,
    /// Due within the next 30 days (or earlier).
    Month,
    /// No due date, or due "sometime".
    Undated,
}

impl DueRange {
    /// All ranges in display order.
    pub const ALL: [DueRange; 7] = [
        DueRange::Any,
        DueRange::Overdue,
        DueRange::Today,
        DueRange::Tomorrow,
        DueRange::Week,
        DueRange::Month,
        DueRange::Undated,
    ];

    pub fn as_key(self) -> &'static str {
        match self {
            DueRange::Any => "any",
            DueRange::Overdue => "overdue",
            DueRange::Today => "today",
            DueRange::Tomorrow => "tomorrow",
            DueRange::Week => "week",
            DueRange::Month => "month",
            DueRange::Undated => "undated",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|range| range.as_key() == key)
    }

    /// Last day a task may be due on to fall into this range; `None` for
    /// the ranges that are not bounded by a day.
    pub fn last_day(self, today: NaiveDate) -> Option<NaiveDate> {
        match self {
            DueRange::Any | DueRange::Undated => None,
            DueRange::Overdue => Some(today - Duration::days(1)),
            DueRange::Today => Some(today),
            DueRange::Tomorrow => Some(today + Duration::days(1)),
            DueRange::Week => Some(today + Duration::days(7)),
            DueRange::Month => Some(today + Duration::days(30)),
        }
    }

    /// Whether `include_undated` has an effect for this range.
    pub fn is_bounded(self) -> bool {
        !matches!(self, DueRange::Any | DueRange::Undated)
    }
}

/// A list filter. The default filter lets everything through.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoFilter {
    #[serde(default)]
    pub due: DueRange,
    /// With a bounded range: also show tasks without a due date.
    #[serde(default)]
    pub include_undated: bool,
    /// Show only tasks with one of these projects (case-insensitive).
    /// [`NO_TAG`] stands for "no project". Empty: no restriction.
    #[serde(default)]
    pub projects: Vec<String>,
    /// Same for places (contexts).
    #[serde(default)]
    pub contexts: Vec<String>,
}

/// Entry in [`TodoFilter::projects`]/[`TodoFilter::contexts`] that matches
/// tasks without any project/place.
pub const NO_TAG: &str = "";

fn is_sometime(date: NaiveDate) -> bool {
    date.year() == 9999
}

fn tags_match(wanted: &[String], have: &[String]) -> bool {
    if wanted.is_empty() {
        return true;
    }
    wanted.iter().any(|w| {
        if w.is_empty() {
            have.is_empty()
        } else {
            let w = w.to_lowercase();
            have.iter().any(|h| h.to_lowercase() == w)
        }
    })
}

impl TodoFilter {
    /// Whether the filter restricts anything.
    pub fn is_active(&self) -> bool {
        self.active_count() > 0
    }

    /// Number of active filter dimensions (for a badge on the filter button).
    pub fn active_count(&self) -> usize {
        usize::from(self.due != DueRange::Any)
            + usize::from(!self.projects.is_empty())
            + usize::from(!self.contexts.is_empty())
    }

    pub fn matches(&self, item: &TodoItem, today: NaiveDate) -> bool {
        self.matches_due(item, today)
            && tags_match(&self.projects, &item.projects)
            && tags_match(&self.contexts, &item.contexts)
    }

    pub fn matches_due(&self, item: &TodoItem, today: NaiveDate) -> bool {
        let date = item.due.map(|d| d.date());
        match self.due {
            DueRange::Any => true,
            DueRange::Undated => date.is_none_or(is_sometime),
            range => match date {
                None => self.include_undated,
                Some(d) if is_sometime(d) => false,
                Some(d) => range.last_day(today).is_some_and(|last| d <= last),
            },
        }
    }

    /// Toggle a project in the filter (case-insensitive).
    pub fn toggle_project(&mut self, name: &str) {
        toggle_tag(&mut self.projects, name);
    }

    /// Toggle a place in the filter (case-insensitive).
    pub fn toggle_context(&mut self, name: &str) {
        toggle_tag(&mut self.contexts, name);
    }
}

fn toggle_tag(tags: &mut Vec<String>, name: &str) {
    let lower = name.to_lowercase();
    if let Some(pos) = tags.iter().position(|t| t.to_lowercase() == lower) {
        tags.remove(pos);
    } else {
        tags.push(name.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::parse_line;

    fn item(line: &str) -> TodoItem {
        parse_line(line, 0).expect("parsable todo line")
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 28).unwrap()
    }

    fn due(range: DueRange) -> TodoFilter {
        TodoFilter { due: range, ..Default::default() }
    }

    #[test]
    fn default_filter_lets_everything_through() {
        let f = TodoFilter::default();
        assert!(!f.is_active());
        assert!(f.matches(&item("- [ ] A due:2020-01-01"), today()));
        assert!(f.matches(&item("- [ ] B"), today()));
    }

    #[test]
    fn week_includes_overdue_and_seven_days_ahead() {
        let f = due(DueRange::Week);
        assert!(f.matches(&item("- [ ] Late due:2026-09-01"), today()));
        assert!(f.matches(&item("- [ ] Today due:2026-09-28T18:00"), today()));
        assert!(f.matches(&item("- [ ] In a week due:2026-10-05T23:00"), today()));
        assert!(!f.matches(&item("- [ ] Later due:2026-10-06"), today()));
        assert!(!f.matches(&item("- [ ] Someday due:9999-12-31"), today()));
        assert!(!f.matches(&item("- [ ] Undated"), today()));
    }

    #[test]
    fn include_undated_adds_tasks_without_date_but_not_someday() {
        let f = TodoFilter { due: DueRange::Today, include_undated: true, ..Default::default() };
        assert!(f.matches(&item("- [ ] Undated"), today()));
        assert!(!f.matches(&item("- [ ] Someday due:9999-12-31"), today()));
        assert!(!f.matches(&item("- [ ] Tomorrow due:2026-09-29"), today()));
    }

    #[test]
    fn overdue_excludes_today() {
        let f = due(DueRange::Overdue);
        assert!(f.matches(&item("- [ ] Late due:2026-09-27T23:59"), today()));
        assert!(!f.matches(&item("- [ ] Today due:2026-09-28T00:00"), today()));
    }

    #[test]
    fn undated_range_covers_no_date_and_someday() {
        let f = due(DueRange::Undated);
        assert!(f.matches(&item("- [ ] Undated"), today()));
        assert!(f.matches(&item("- [ ] Someday due:9999-12-31"), today()));
        assert!(!f.matches(&item("- [ ] Dated due:2026-09-28"), today()));
    }

    #[test]
    fn projects_match_any_case_insensitive_and_no_project() {
        let mut f = TodoFilter::default();
        f.toggle_project("haushalt");
        assert!(f.matches(&item("- [ ] A +Haushalt"), today()));
        assert!(!f.matches(&item("- [ ] B +Arbeit"), today()));
        assert!(!f.matches(&item("- [ ] C"), today()));
        f.toggle_project(NO_TAG);
        assert!(f.matches(&item("- [ ] C"), today()));
        f.toggle_project("HAUSHALT");
        assert_eq!(f.projects, vec![NO_TAG.to_string()]);
    }

    #[test]
    fn dimensions_combine() {
        let mut f = due(DueRange::Week);
        f.toggle_context("Rechner");
        assert_eq!(f.active_count(), 2);
        assert!(f.matches(&item("- [ ] A @Rechner due:2026-09-30"), today()));
        assert!(!f.matches(&item("- [ ] B @Telefon due:2026-09-30"), today()));
        assert!(!f.matches(&item("- [ ] C @Rechner due:2026-12-01"), today()));
    }

    #[test]
    fn serde_uses_lowercase_keys_and_defaults() {
        let f: TodoFilter = serde_json::from_str(r#"{"due":"week"}"#).unwrap();
        assert_eq!(f.due, DueRange::Week);
        assert!(f.projects.is_empty());
        assert_eq!(DueRange::from_key("month"), Some(DueRange::Month));
        assert_eq!(DueRange::from_key("bogus"), None);
    }
}
