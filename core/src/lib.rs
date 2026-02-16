pub mod config;
pub mod conflict;
pub mod data;
pub mod i18n;
pub mod parser;
pub mod preferences;
pub mod reminders;
pub mod renderer;
pub mod sorting;
pub mod storage;
pub mod todo;
pub mod types;
pub mod undo;
pub mod util;
pub mod webdav;

pub use data::*;
pub use i18n::t;
pub use preferences::*;
pub use sorting::*;

#[cfg(test)]
mod tests {
	use chrono::NaiveDate;

	#[test]
	fn test_parse_line_basic() {
		let line = "- [ ] Buy milk +groceries @home due:2026-01-20T09:00 ^ABC123";
		let item = crate::data::parse_line(line, 0).unwrap();
		assert_eq!(item.title, "Buy milk");
		assert_eq!(item.projects, vec!["groceries"]);
		assert_eq!(item.contexts, vec!["home"]);
		assert_eq!(item.due, Some(NaiveDate::from_ymd_opt(2026, 1, 20).unwrap().and_hms_opt(9, 0, 0).unwrap()));
		assert_eq!(item.key.marker, Some("ABC123".to_string()));
		assert!(!item.done);
	}

	#[test]
	fn test_parse_line_done() {
		let line = "- [x] Finish report +work due:2026-01-21T18:00 ^XYZ789";
		let item = crate::data::parse_line(line, 1).unwrap();
		assert_eq!(item.title, "Finish report");
		assert!(item.done);
		assert_eq!(item.projects, vec!["work"]);
		assert_eq!(item.due, Some(NaiveDate::from_ymd_opt(2026, 1, 21).unwrap().and_hms_opt(18, 0, 0).unwrap()));
		assert_eq!(item.key.marker, Some("XYZ789".to_string()));
	}

	#[test]
	fn test_extract_title_edge_cases() {
		assert_eq!(crate::data::extract_title("Task only"), "Task only");
		assert_eq!(crate::data::extract_title("Task +project @context"), "Task");
		assert_eq!(crate::data::extract_title("Just a title due:2026-01-20"), "Just a title");
	}

	#[test]
	fn test_render_line_roundtrip() {
		let line = "- [ ] Call Alice +friends due:2026-01-22T10:00 ^ID123";
		let item = crate::data::parse_line(line, 0).unwrap();
		let rendered = crate::data::render_line(&item).unwrap();
		let reparsed = crate::data::parse_line(&rendered, 0).unwrap();
		assert_eq!(item.title, reparsed.title);
		assert_eq!(item.projects, reparsed.projects);
		assert_eq!(item.due, reparsed.due);
		assert_eq!(item.key.marker, reparsed.key.marker);
	}

	#[test]
	fn test_toggle_checkbox() {
		let unchecked = "- [ ] Task ^ID1";
		let checked = crate::data::rewrite_line(unchecked, true).unwrap();
		assert!(checked.contains("- [x]"));
		let unchecked2 = crate::data::rewrite_line(&checked, false).unwrap();
		assert!(unchecked2.contains("- [ ]"));
	}

	#[test]
	fn test_due_parsing_and_insertion() {
		let line = "- [ ] Task ^ID2";
		let due = NaiveDate::from_ymd_opt(2026, 2, 1).unwrap().and_hms_opt(12, 0, 0).unwrap();
		let updated = crate::data::rewrite_due(line, due).unwrap();
		assert!(updated.contains("due:2026-02-01T12:00"));
	}

	#[test]
	fn test_next_due_date_daily() {
		let today = chrono::Local::now().date_naive();
		let dt = today.and_hms_opt(8, 0, 0).unwrap();
		let next = crate::data::next_due_date(Some(dt), "daily").unwrap();
		assert!(next.date() > today);
	}

	#[test]
	fn test_encode_base36() {
		let encoded = crate::data::encode_base36(123456789);
		assert_eq!(encoded, "21i3v9");
	}
}
