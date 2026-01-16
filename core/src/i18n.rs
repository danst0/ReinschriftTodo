use std::sync::{Mutex, OnceLock};
use std::collections::HashMap;
use std::env;

static OVERRIDE_LANG: OnceLock<Mutex<Option<String>>> = OnceLock::new();

pub fn set_language(lang: String) {
    let m = OVERRIDE_LANG.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = m.lock() {
        *guard = Some(lang);
    }
}

/// Detect system language from environment variables
fn system_languages() -> Vec<String> {
    let mut langs = Vec::new();

    // Check LANGUAGE first (colon-separated list)
    if let Ok(language) = env::var("LANGUAGE") {
        for lang in language.split(':') {
            if !lang.is_empty() {
                langs.push(lang.to_string());
            }
        }
    }

    // Then LC_ALL
    if let Ok(lc_all) = env::var("LC_ALL") {
        if !lc_all.is_empty() && lc_all != "C" && lc_all != "POSIX" {
            langs.push(lc_all);
        }
    }

    // Then LC_MESSAGES
    if let Ok(lc_messages) = env::var("LC_MESSAGES") {
        if !lc_messages.is_empty() && lc_messages != "C" && lc_messages != "POSIX" {
            langs.push(lc_messages);
        }
    }

    // Finally LANG
    if let Ok(lang) = env::var("LANG") {
        if !lang.is_empty() && lang != "C" && lang != "POSIX" {
            langs.push(lang);
        }
    }

    // Default to English if nothing found
    if langs.is_empty() {
        langs.push("en".to_string());
    }

    langs
}

pub fn t(key: &str) -> String {
    static TRANSLATIONS: OnceLock<HashMap<&'static str, HashMap<String, String>>> = OnceLock::new();

    let translations = TRANSLATIONS.get_or_init(|| {
        let mut m = HashMap::new();

        let de_json = include_str!("i18n/de.json");
        let de: HashMap<String, String> = serde_json::from_str(de_json).expect("Failed to parse de.json");
        m.insert("de", de);

        let en_json = include_str!("i18n/en.json");
        let en: HashMap<String, String> = serde_json::from_str(en_json).expect("Failed to parse en.json");
        m.insert("en", en);

        let es_json = include_str!("i18n/es.json");
        let es: HashMap<String, String> = serde_json::from_str(es_json).expect("Failed to parse es.json");
        m.insert("es", es);

        let fr_json = include_str!("i18n/fr.json");
        let fr: HashMap<String, String> = serde_json::from_str(fr_json).expect("Failed to parse fr.json");
        m.insert("fr", fr);

        let ja_json = include_str!("i18n/ja.json");
        let ja: HashMap<String, String> = serde_json::from_str(ja_json).expect("Failed to parse ja.json");
        m.insert("ja", ja);

        let sv_json = include_str!("i18n/sv.json");
        let sv: HashMap<String, String> = serde_json::from_str(sv_json).expect("Failed to parse sv.json");
        m.insert("sv", sv);

        m
    });

    let langs = if let Some(Some(override_lang)) = OVERRIDE_LANG.get().map(|m| m.lock().ok().and_then(|g| g.clone())) {
        vec![override_lang]
    } else {
        system_languages()
    };

    for lang in langs {
        let lang_str = lang.as_str();
        let lang_code = lang_str.split('_').next().unwrap_or(lang_str).split('.').next().unwrap_or(lang_str);
        if let Some(map) = translations.get(lang_code) {
            if let Some(val) = map.get(key) {
                return val.clone();
            }
        }
    }

    // Fallback to German as requested
    translations.get("de").and_then(|m| m.get(key)).map(|s| s.clone()).unwrap_or_else(|| key.to_string())
}
