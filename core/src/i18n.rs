//! Internationalisierung über GNU gettext (Domain "reinschrift").
//!
//! Die Quellsprache der Strings im Code ist Englisch (msgid). Übersetzungen
//! liegen als .po-Dateien in po/ und werden als .mo-Kataloge geladen:
//! - Flatpak: /app/share/locale
//! - Installierte Builds: <prefix>/share/locale (relativ zur Binärdatei)
//! - Entwicklung/Tests: von core/build.rs per msgfmt nach OUT_DIR kompiliert
//!
//! Fällt für unübersetzte Sprachen auf Englisch (die msgid) zurück —
//! Standard-gettext-Verhalten; ersetzt den früheren Deutsch-Fallback des
//! JSON-Systems.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use gettextrs::{
    LocaleCategory, bind_textdomain_codeset, bindtextdomain, gettext, pgettext, setlocale,
    textdomain,
};

pub const GETTEXT_DOMAIN: &str = "reinschrift";

static INIT: OnceLock<()> = OnceLock::new();

/// Sprachen, für die Übersetzungskataloge ausgeliefert werden.
const KNOWN_LANGS: [&str; 5] = ["de", "es", "fr", "ja", "sv"];

fn has_catalog(dir: &Path) -> bool {
    KNOWN_LANGS.iter().any(|lang| {
        dir.join(lang)
            .join("LC_MESSAGES")
            .join(format!("{GETTEXT_DOMAIN}.mo"))
            .exists()
    })
}

/// Ermittelt das Verzeichnis mit den .mo-Katalogen.
fn locale_dir() -> PathBuf {
    // Explizite Übersteuerung (z. B. für Tests oder ungewöhnliche Prefixe)
    if let Ok(dir) = env::var("REINSCHRIFT_LOCALEDIR")
        && !dir.is_empty() {
            return PathBuf::from(dir);
        }

    // Flatpak-Sandbox
    if Path::new("/.flatpak-info").exists() {
        return PathBuf::from("/app/share/locale");
    }

    // Installierte Binärdatei: <prefix>/bin/reinschrift -> <prefix>/share/locale
    if let Ok(exe) = env::current_exe()
        && let Some(prefix) = exe.parent().and_then(|bin| bin.parent()) {
            let candidate = prefix.join("share").join("locale");
            if has_catalog(&candidate) {
                return candidate;
            }
        }

    for system_dir in ["/usr/share/locale", "/usr/local/share/locale"] {
        let candidate = PathBuf::from(system_dir);
        if has_catalog(&candidate) {
            return candidate;
        }
    }

    // Entwicklungs-Fallback: build.rs kompiliert po/*.po nach OUT_DIR/locale
    PathBuf::from(env!("REINSCHRIFT_DEV_LOCALEDIR"))
}

/// Aktiviert eine LC_MESSAGES-Locale, in der glibc $LANGUAGE auswertet.
/// In C, POSIX und C.UTF-8 ignoriert glibc $LANGUAGE komplett; damit
/// --language auch in minimalen Umgebungen (LANG unbelegt) funktioniert,
/// wird eine zur gewünschten Sprache passende UTF-8-Locale probiert.
fn activate_messages_locale() {
    fn is_c_locale(value: &[u8]) -> bool {
        value == b"POSIX"
            || value.eq_ignore_ascii_case(b"C")
            || value.eq_ignore_ascii_case(b"C.UTF-8")
            || value.eq_ignore_ascii_case(b"C.utf8")
    }

    match setlocale(LocaleCategory::LcMessages, "") {
        Some(value) if !is_c_locale(&value) => return,
        _ => {}
    }

    let mut candidates: Vec<&str> = Vec::new();
    if let Ok(language) = env::var("LANGUAGE") {
        for lang in language.split(':').filter(|l| !l.is_empty()) {
            let base = lang.split(['_', '.', '@']).next().unwrap_or(lang);
            candidates.push(match base {
                "de" => "de_DE.UTF-8",
                "en" => "en_US.UTF-8",
                "es" => "es_ES.UTF-8",
                "fr" => "fr_FR.UTF-8",
                "ja" => "ja_JP.UTF-8",
                "sv" => "sv_SE.UTF-8",
                _ => continue,
            });
        }
    }
    candidates.push("en_US.UTF-8");
    for candidate in candidates {
        if setlocale(LocaleCategory::LcMessages, candidate).is_some() {
            return;
        }
    }
}

fn ensure_init() {
    INIT.get_or_init(|| {
        setlocale(LocaleCategory::LcAll, "");
        activate_messages_locale();
        let dir = locale_dir();
        let _ = bindtextdomain(GETTEXT_DOMAIN, dir);
        let _ = bind_textdomain_codeset(GETTEXT_DOMAIN, "UTF-8");
        let _ = textdomain(GETTEXT_DOMAIN);
    });
}

/// Erzwingt eine Sprache (z. B. über --language) unabhängig von der
/// System-Locale. Muss früh im Programmstart aufgerufen werden, bevor
/// weitere Threads laufen.
pub fn set_language(lang: String) {
    // SAFETY: Wird von GUI/CLI direkt nach dem Argument-Parsing im noch
    // single-threaded Programmstart aufgerufen.
    unsafe {
        env::set_var("LANGUAGE", &lang);
    }
    // Falls bereits initialisiert wurde, Locale-Neubewertung anstoßen.
    if INIT.get().is_some() {
        activate_messages_locale();
    }
}

/// Übersetzt einen englischen Quelltext (msgid) in die aktuelle Sprache.
/// Ohne Übersetzung wird die msgid (Englisch) zurückgegeben.
pub fn t(msgid: &str) -> String {
    ensure_init();
    gettext(msgid)
}

/// Übersetzung mit Kontext (msgctxt) für mehrdeutige englische Texte,
/// z. B. tc("conflict dialog button", "Reload").
pub fn tc(context: &str, msgid: &str) -> String {
    ensure_init();
    pgettext(context, msgid)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ein einzelner Test, da set_language Prozess-Zustand (Env, Locale)
    // verändert und Tests parallel laufen.
    #[test]
    fn test_language_switch_and_fallback() {
        set_language("en".to_string());
        assert_eq!(t("Settings"), "Settings");

        set_language("de".to_string());
        assert_eq!(t("Settings"), "Einstellungen");
        // msgctxt-Eintrag wird getrennt aufgelöst
        assert_eq!(tc("conflict dialog button", "Reload"), "Neu laden");

        set_language("sv".to_string());
        assert_eq!(t("Settings"), "Inställningar");
        assert_eq!(tc("conflict dialog button", "Reload"), "Ladda om");

        // Unbekannte msgid und nicht unterstützte Sprache -> msgid (Englisch).
        // Über eine Variable, damit xtr den Test-String nicht extrahiert.
        let missing = "__nonexistent_msgid__";
        assert_eq!(t(missing), missing);
        set_language("it".to_string());
        assert_eq!(t("Settings"), "Settings");
    }
}
