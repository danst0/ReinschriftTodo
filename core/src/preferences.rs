use serde::{Deserialize, Serialize};

use crate::filter::{DueRange, TodoFilter};
use std::fs;
use std::path::PathBuf;

fn default_whisper_language() -> String {
    "auto".to_string()
}

fn default_ai_timeout_secs() -> u64 {
    30
}

fn default_skip_delete_confirmation() -> bool {
    true
}

fn default_remind_before_minutes() -> i64 {
    30
}

fn default_title_autocomplete_enabled() -> bool {
    true
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub struct Preferences {
    #[serde(default)]
    pub sort_mode: Option<String>,
    #[serde(default)]
    pub show_done: bool,
    #[serde(default = "default_skip_delete_confirmation")]
    pub skip_delete_confirmation: bool,
    #[serde(default)]
    pub enable_reminders: bool,
    #[serde(default = "default_remind_before_minutes")]
    pub remind_before_minutes: i64,
    #[serde(default)]
    pub db_path: Option<String>,
    /// Replaced by `filter`; still read so older settings keep working.
    #[serde(default)]
    pub show_due_only: bool,
    #[serde(default)]
    pub filter: Option<TodoFilter>,
    #[serde(default)]
    pub myday_view: bool,
    #[serde(default)]
    pub use_webdav: bool,
    #[serde(default)]
    pub webdav_url: Option<String>,
    #[serde(default)]
    pub webdav_path: Option<String>,
    #[serde(default)]
    pub webdav_username: Option<String>,
    #[serde(default)]
    pub webdav_password: Option<String>,
    #[serde(default)]
    pub use_whisper: bool,
    #[serde(default = "default_whisper_language")]
    pub whisper_language: String,
    #[serde(default)]
    pub use_ai_on_new_topic: bool,
    #[serde(default = "default_ai_timeout_secs")]
    pub ai_timeout_secs: u64,
    #[serde(default)]
    pub ollama_url: Option<String>,
    #[serde(default)]
    pub ollama_model: Option<String>,
    #[serde(default = "default_title_autocomplete_enabled")]
    pub title_autocomplete_enabled: bool,
    #[serde(default)]
    pub semantic_enabled: bool,
    #[serde(default)]
    pub embedding_model: Option<String>,
    #[serde(default)]
    pub window_width: Option<i32>,
    #[serde(default)]
    pub window_height: Option<i32>,
    #[serde(default)]
    pub window_maximized: bool,
}

impl Preferences {
    /// The list filter in effect. Settings from before the filter existed
    /// only know "show only due": due by today, tasks without a date kept.
    pub fn effective_filter(&self) -> TodoFilter {
        match &self.filter {
            Some(filter) => filter.clone(),
            None if self.show_due_only => TodoFilter {
                due: DueRange::Today,
                include_undated: true,
                ..Default::default()
            },
            None => TodoFilter::default(),
        }
    }

    /// Store a filter, retiring the old "show only due" flag.
    pub fn set_filter(&mut self, filter: TodoFilter) {
        self.filter = Some(filter);
        self.show_due_only = false;
    }
}

/// Returns the path to the shared preferences file.
/// Uses XDG_CONFIG_HOME if set, otherwise ~/.config
pub fn preferences_path() -> PathBuf {
    let config_dir = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let mut home = dirs_path();
            home.push(".config");
            home
        });
    config_dir.join("reinschrift_todo").join("preferences.json")
}

fn dirs_path() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Load preferences from the shared config file.
pub fn load_preferences() -> Preferences {
    let path = preferences_path();
    if let Ok(data) = fs::read_to_string(&path) {
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        Preferences::default()
    }
}

/// Write preferences to the shared config file.
pub fn write_preferences(prefs: &Preferences) -> std::io::Result<()> {
    let path = preferences_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let serialized = serde_json::to_string_pretty(prefs).unwrap_or_else(|_| "{}".into());
    fs::write(path, serialized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_show_due_only_becomes_due_today_with_undated() {
        let prefs: Preferences = serde_json::from_str(r#"{"show_due_only": true}"#).unwrap();
        let filter = prefs.effective_filter();
        assert_eq!(filter.due, DueRange::Today);
        assert!(filter.include_undated);
    }

    #[test]
    fn stored_filter_wins_and_retires_flag() {
        let mut prefs: Preferences = serde_json::from_str(r#"{"show_due_only": true}"#).unwrap();
        prefs.set_filter(TodoFilter { due: DueRange::Week, ..Default::default() });
        assert!(!prefs.show_due_only);
        assert_eq!(prefs.effective_filter().due, DueRange::Week);
    }
}
