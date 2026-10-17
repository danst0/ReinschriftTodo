//! "Feedback senden": GitHub-Issue und E-Mail als gleichwertige Wege.
//!
//! Die App verschickt selbst nichts – sie baut nur Links, die Browser bzw.
//! Mailprogramm mit vorausgefülltem Text öffnen. Am Ende des Texts stehen
//! Version und System, damit Rückfragen danach entfallen. Mirror der URLs:
//! `webapp/app/utils/feedback.py`.

use std::env;
use std::fs;
use std::path::Path;

pub const GITHUB_NEW_ISSUE_URL: &str = "https://github.com/danst0/ReinschriftTodo/issues/new";
pub const FEEDBACK_EMAIL: &str = "reinschrift-feedback@dumke.me";

/// Systemangaben unter dem Feedback-Text, je eine Zeile `Name: Wert`.
/// `client` ist z. B. "Desktop" oder "CLI"; `extra` hängt GUI-spezifische
/// Angaben wie die GTK-Version an.
pub fn system_info(client: &str, extra: &[(&str, String)]) -> String {
    let install = if is_flatpak() { "Flatpak" } else { "native" };
    let mut lines = vec![
        format!("Reinschrift: {}", env!("CARGO_PKG_VERSION")),
        format!("Client: {client} ({install})"),
    ];
    if let Some(os) = os_name() {
        lines.push(format!("System: {os}"));
    }
    if let Some(lang) = language() {
        lines.push(format!("Language: {lang}"));
    }
    for (key, value) in extra {
        lines.push(format!("{key}: {value}"));
    }
    lines.join("\n")
}

/// Vorbelegter Text: Leerzeilen zum Schreiben oben, darunter die Systemangaben.
pub fn body(system_info: &str) -> String {
    format!("\n\n\n---\n{system_info}\n")
}

pub fn github_url(system_info: &str) -> String {
    format!("{GITHUB_NEW_ISSUE_URL}?body={}", percent_encode(&body(system_info)))
}

pub fn mailto_url(system_info: &str) -> String {
    format!(
        "mailto:{FEEDBACK_EMAIL}?subject={}&body={}",
        percent_encode("Reinschrift Feedback"),
        percent_encode(&body(system_info)),
    )
}

fn is_flatpak() -> bool {
    env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists()
}

/// PRETTY_NAME des Hosts; im Flatpak liegt dessen os-release unter /run/host,
/// /etc/os-release wäre dort die Laufzeitumgebung.
fn os_name() -> Option<String> {
    ["/run/host/os-release", "/etc/os-release"]
        .iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .find_map(|content| {
            content.lines().find_map(|line| {
                line.strip_prefix("PRETTY_NAME=")
                    .map(|v| v.trim_matches('"').to_string())
            })
        })
}

fn language() -> Option<String> {
    ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| env::var(key).ok())
        .find(|v| !v.is_empty())
}

/// Prozentkodierung für Query-Werte (RFC 3986: alles außer unreserved).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_escapes_reserved_and_utf8() {
        assert_eq!(percent_encode("a b&c=ü\n"), "a%20b%26c%3D%C3%BC%0A");
    }

    #[test]
    fn urls_carry_body_with_system_info() {
        let info = "Reinschrift: 1.0.0\nClient: CLI (native)";
        let gh = github_url(info);
        assert!(gh.starts_with("https://github.com/danst0/ReinschriftTodo/issues/new?body="));
        assert!(gh.contains("Client%3A%20CLI%20%28native%29"));
        let mail = mailto_url(info);
        assert!(mail.starts_with("mailto:reinschrift-feedback@dumke.me?subject=Reinschrift%20Feedback&body="));
        assert!(mail.contains("Reinschrift%3A%201.0.0"));
    }

    #[test]
    fn system_info_lists_version_client_and_extra() {
        let info = system_info("Desktop", &[("GTK", "4.20.1".to_string())]);
        assert!(info.starts_with(&format!("Reinschrift: {}\n", env!("CARGO_PKG_VERSION"))));
        assert!(info.contains("Client: Desktop ("));
        assert!(info.ends_with("GTK: 4.20.1"));
    }
}
