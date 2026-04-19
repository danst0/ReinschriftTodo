//! Utility functions for string manipulation and marker generation.

use std::time::{SystemTime, UNIX_EPOCH};

/// Markers that delimit fields in a todo line.
/// Used by extract_title and insert_due_segment. Only space-prefixed variants:
/// an unspaced `+` or `^` at the very start of the title would otherwise eat the
/// title (e.g. when a user types `+Projekt erledigen`).
pub const FIELD_MARKERS: [&str; 8] = [" +", " @", " due:", " rec:", " [[", " ~note:", " ^", " ✅"];

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
pub fn normalize_token(value: Option<&str>) -> Option<String> {
    value
        .map(|s| {
            let trimmed = s.trim().replace(' ', "");
            // Remove all leading + or @
            let chars = trimmed.chars();
            let mut out = String::new();
            let mut found = false;
            for c in chars {
                if !found && (c == '+' || c == '@') {
                    continue;
                }
                found = true;
                out.push(c);
            }
            out
        })
        .filter(|s| !s.is_empty())
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
