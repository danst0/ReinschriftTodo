//! Text search over todos, shared by GUI and CLI (the webapp mirrors it in
//! `webapp/app/services/search.py`).
//!
//! The title is stored without its tokens, so `+Security` or `@Computer` never
//! show up in it. A query therefore works in two ways:
//!
//! - Words starting with `+` or `@` filter by project or place (substring,
//!   case-insensitive): `+sec` finds `+Security`. A hit counts as primary.
//! - The remaining text is searched in the title first (primary). Todos that
//!   only carry it in a project, place or note are secondary hits, listed
//!   below the title hits.
//!
//! Both parts combine: `+security ufw` finds Security tasks mentioning "ufw".

use crate::types::TodoItem;

/// How a todo matches a query.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SearchMatch {
    /// Matches the title or the requested `+project`/`@place`.
    Primary,
    /// The text appears only in a project, place or note.
    Secondary,
}

/// A parsed search query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchQuery {
    /// Lowercased `+project` fragments (without the `+`), all must match.
    pub projects: Vec<String>,
    /// Lowercased `@place` fragments (without the `@`), all must match.
    pub contexts: Vec<String>,
    /// Lowercased remaining text, words joined by single spaces.
    pub text: String,
}

impl SearchQuery {
    pub fn parse(query: &str) -> Self {
        let mut parsed = SearchQuery::default();
        let mut words = Vec::new();
        for word in query.split_whitespace() {
            let lower = word.to_lowercase();
            if let Some(tag) = lower.strip_prefix('+') {
                parsed.projects.push(tag.to_string());
            } else if let Some(tag) = lower.strip_prefix('@') {
                parsed.contexts.push(tag.to_string());
            } else {
                words.push(lower);
            }
        }
        parsed.text = words.join(" ");
        parsed
    }

    pub fn is_empty(&self) -> bool {
        self.projects.is_empty() && self.contexts.is_empty() && self.text.is_empty()
    }

    /// How `todo` matches, or `None` if it does not.
    pub fn matches(&self, todo: &TodoItem) -> Option<SearchMatch> {
        if self.is_empty() {
            return None;
        }
        if !tags_match(&self.projects, &todo.projects) || !tags_match(&self.contexts, &todo.contexts) {
            return None;
        }
        if self.text.is_empty() || todo.title.to_lowercase().contains(&self.text) {
            return Some(SearchMatch::Primary);
        }
        let in_tags = todo
            .projects
            .iter()
            .chain(&todo.contexts)
            .any(|tag| tag.to_lowercase().contains(&self.text));
        let in_note = todo
            .note
            .as_ref()
            .is_some_and(|note| note.to_lowercase().contains(&self.text));
        (in_tags || in_note).then_some(SearchMatch::Secondary)
    }
}

/// Every wanted fragment occurs in at least one of the todo's tags.
fn tags_match(wanted: &[String], tags: &[String]) -> bool {
    wanted
        .iter()
        .all(|w| tags.iter().any(|tag| tag.to_lowercase().contains(w)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::parse_line;

    fn item(line: &str) -> TodoItem {
        parse_line(line, 0).expect("parsable todo line")
    }

    fn search(query: &str, line: &str) -> Option<SearchMatch> {
        SearchQuery::parse(query).matches(&item(line))
    }

    #[test]
    fn title_hit_is_primary() {
        assert_eq!(search("Secret", "- [ ] Rotate secrets +Home"), Some(SearchMatch::Primary));
    }

    #[test]
    fn hit_only_in_project_or_note_is_secondary() {
        let line = "- [ ] Fix comments +Security @Computer ~note:\"sshd config\"";
        assert_eq!(search("sec", line), Some(SearchMatch::Secondary));
        assert_eq!(search("comp", line), Some(SearchMatch::Secondary));
        assert_eq!(search("sshd", line), Some(SearchMatch::Secondary));
        assert_eq!(search("nothing", line), None);
    }

    #[test]
    fn tag_prefix_filters_by_project_or_place() {
        let line = "- [ ] Fix comments +Security @Computer";
        assert_eq!(search("+sec", line), Some(SearchMatch::Primary));
        assert_eq!(search("@comp", line), Some(SearchMatch::Primary));
        assert_eq!(search("+comp", line), None);
        assert_eq!(search("@sec", line), None);
        // A title containing the word does not count for a tag query.
        assert_eq!(search("+sec", "- [ ] Security audit +Home"), None);
    }

    #[test]
    fn tag_and_text_combine() {
        let line = "- [ ] Limit UFW rule +Security ~note:\"octopus\"";
        assert_eq!(search("+security ufw", line), Some(SearchMatch::Primary));
        assert_eq!(search("+security octopus", line), Some(SearchMatch::Secondary));
        assert_eq!(search("+home ufw", line), None);
    }

    #[test]
    fn bare_prefix_matches_any_tag() {
        assert_eq!(search("+", "- [ ] A +Home"), Some(SearchMatch::Primary));
        assert_eq!(search("+", "- [ ] A"), None);
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert_eq!(search("   ", "- [ ] A"), None);
    }
}
