//! Utility functions for string manipulation and marker generation.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Markers that delimit fields in a todo line.
/// Used by extract_title and insert_due_segment. Only space-prefixed variants:
/// an unspaced `+` or `^` at the very start of the title would otherwise eat the
/// title (e.g. when a user types `+Projekt erledigen`).
pub const FIELD_MARKERS: [&str; 9] =
    [" +", " @", " due:", " myday:", " rec:", " [[", " ~note:", " ^", " ✅"];

/// Generate a unique marker ID using timestamp and process ID.
pub fn generate_marker() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| std::time::Duration::from_secs(0))
        .as_nanos();
    let pid = std::process::id() as u128;
    let mixed = now ^ (pid << 64);
    let mut encoded = encode_base36(mixed);

    // Keep IDs short and alphanumeric (e.g., 8 chars)
    if encoded.len() > 8 {
        encoded.truncate(8);
    } else {
        while encoded.len() < 8 {
            encoded.push('0');
        }
    }

    encoded
}

/// Encode a value as base36 string.
pub fn encode_base36(mut value: u128) -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }

    let mut buf = Vec::new();
    while value > 0 {
        let idx = (value % 36) as usize;
        buf.push(ALPHABET[idx] as char);
        value /= 36;
    }

    buf.into_iter().rev().collect()
}

/// Normalize a token (project/context) by trimming and removing leading +/@.
/// Internal whitespace is preserved so multi-word tokens (e.g. `Steuererklärung 2024`)
/// round-trip through parse+render via the quoted form.
pub fn normalize_token(value: Option<&str>) -> Option<String> {
    value
        .map(|s| {
            let trimmed = s.trim();
            let mut out = String::new();
            let mut found = false;
            for c in trimmed.chars() {
                if !found && (c == '+' || c == '@') {
                    continue;
                }
                found = true;
                out.push(c);
            }
            out.trim().to_string()
        })
        .filter(|s| !s.is_empty())
}

/// Split the free-form content of a dedicated project/context entry field into
/// individual tag names.
///
/// The `prefix` (`+` or `@`) acts as the delimiter whenever it is present, so
/// spaces inside a name are preserved: `+Big Project +Other` yields
/// `["Big Project", "Other"]`. This is what makes multi-word names survive a
/// round-trip through the edit dialogs, which render them as `+Big Project`.
/// Input without any prefix keeps the historical whitespace split
/// (`steuer buero` → `["steuer", "buero"]`), and the quoted form used in the
/// markdown file (`+"Big Project"`) is accepted as well.
///
/// Only for fields that contain nothing but tags — free-text input such as the
/// quick-add entry stays quote-based, since there the prefix cannot swallow
/// everything up to the next tag without eating the title.
pub fn split_tag_input(raw: &str, prefix: char) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let starts = prefix_positions(trimmed, prefix);
    if starts.is_empty() {
        return trimmed.split_whitespace().map(str::to_string).collect();
    }

    let mut names: Vec<String> = Vec::new();
    // Bare text before the first prefix (e.g. `foo +bar`) follows the legacy rule.
    names.extend(trimmed[..starts[0]].split_whitespace().map(str::to_string));

    for (idx, start) in starts.iter().enumerate() {
        let end = starts.get(idx + 1).copied().unwrap_or(trimmed.len());
        let segment = trimmed[start + prefix.len_utf8()..end].trim();
        if let Some(name) = normalize_token(Some(&unquote_tag(segment))) {
            names.push(name);
        }
    }

    names
}

/// Byte offsets of every `prefix` that starts a token (string start or after
/// whitespace) — a `+` inside a name like `C++` must not split it.
fn prefix_positions(text: &str, prefix: char) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut at_token_start = true;
    for (idx, ch) in text.char_indices() {
        if ch == prefix && at_token_start {
            positions.push(idx);
        }
        at_token_start = ch.is_whitespace();
    }
    positions
}

/// Strip the quoted form (`"Big Project"`) if present, unescaping the content.
fn unquote_tag(segment: &str) -> String {
    if segment.len() >= 2 && segment.starts_with('"') && segment.ends_with('"') {
        unescape_note(&segment[1..segment.len() - 1])
    } else {
        segment.to_string()
    }
}

/// Build a map from the lowercased form of each token (project/context) to its
/// most frequently used casing. Ties are broken lexically for determinism.
/// Used to aggregate case variants (e.g. `PixelMatrix` vs. `Pixelmatrix`) into
/// one canonical display form.
pub fn canonical_casing_map<'a>(
    values: impl IntoIterator<Item = &'a str>,
) -> HashMap<String, String> {
    let mut variants: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for value in values {
        if value.is_empty() {
            continue;
        }
        *variants
            .entry(value.to_lowercase())
            .or_default()
            .entry(value.to_string())
            .or_insert(0) += 1;
    }
    variants
        .into_iter()
        .filter_map(|(key, counts)| {
            counts
                .into_iter()
                // max_by: higher count wins; on ties the lexically smaller casing
                // compares greater so the result is deterministic.
                .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
                .map(|(casing, _)| (key, casing))
        })
        .collect()
}

/// Resolve a token to its canonical casing via [`canonical_casing_map`];
/// unknown tokens are returned unchanged.
pub fn canonicalize_token(map: &HashMap<String, String>, value: &str) -> String {
    map.get(&value.to_lowercase())
        .cloned()
        .unwrap_or_else(|| value.to_string())
}

/// Normalize a reference string by trimming whitespace.
pub fn normalize_reference(value: Option<&str>) -> Option<String> {
    value
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Normalize a note string by trimming whitespace.
pub fn normalize_note(value: Option<&str>) -> Option<String> {
    value
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Escape special characters in a note for storage.
pub fn escape_note(note: &str) -> String {
    note.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

/// Unescape special characters in a note after reading.
pub fn unescape_note(input: &str) -> String {
    let mut out = String::new();
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                match next {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    other => {
                        out.push(other);
                    }
                }
            } else {
                out.push('\\');
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_casing_prefers_most_used_variant() {
        let values = ["PixelMatrix", "Pixelmatrix", "PixelMatrix", "Keller"];
        let map = canonical_casing_map(values.into_iter());
        assert_eq!(map.get("pixelmatrix").map(String::as_str), Some("PixelMatrix"));
        assert_eq!(map.get("keller").map(String::as_str), Some("Keller"));
        assert_eq!(canonicalize_token(&map, "pixelmatrix"), "PixelMatrix");
        assert_eq!(canonicalize_token(&map, "Unbekannt"), "Unbekannt");
    }

    #[test]
    fn split_tag_input_keeps_spaces_after_prefix() {
        assert_eq!(split_tag_input("+Big Project", '+'), vec!["Big Project"]);
        assert_eq!(
            split_tag_input("+Big Project +Other Thing", '+'),
            vec!["Big Project", "Other Thing"]
        );
        assert_eq!(
            split_tag_input("@Home Office @Auf Reisen", '@'),
            vec!["Home Office", "Auf Reisen"]
        );
    }

    #[test]
    fn split_tag_input_without_prefix_splits_on_whitespace() {
        assert_eq!(split_tag_input("steuer buero", '+'), vec!["steuer", "buero"]);
        assert_eq!(split_tag_input("keller", '@'), vec!["keller"]);
    }

    #[test]
    fn split_tag_input_accepts_quoted_form() {
        assert_eq!(
            split_tag_input(r#"+"Big Project" +plain"#, '+'),
            vec!["Big Project", "plain"]
        );
    }

    #[test]
    fn split_tag_input_ignores_prefix_inside_name() {
        assert_eq!(split_tag_input("+C++ Kurs", '+'), vec!["C++ Kurs"]);
    }

    #[test]
    fn split_tag_input_handles_edges() {
        assert!(split_tag_input("   ", '+').is_empty());
        assert!(split_tag_input("+", '+').is_empty());
        assert_eq!(split_tag_input("+  Big Project  ", '+'), vec!["Big Project"]);
        // Bare text before the first prefix keeps the legacy whitespace split.
        assert_eq!(split_tag_input("a b +Big Project", '+'), vec!["a", "b", "Big Project"]);
    }

    #[test]
    fn canonical_casing_breaks_ties_lexically() {
        let map = canonical_casing_map(["Werkstatt", "werkstatt"].into_iter());
        // Equal counts: the lexically smaller casing wins deterministically.
        assert_eq!(map.get("werkstatt").map(String::as_str), Some("Werkstatt"));
    }
}
