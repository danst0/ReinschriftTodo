use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Kompiliert die Übersetzungskataloge (po/*.po) per msgfmt nach
/// OUT_DIR/locale/<lang>/LC_MESSAGES/reinschrift.mo, damit Entwicklungs-
/// und Test-Läufe (cargo run / cargo test) ohne installierte .mo-Dateien
/// funktionieren. Installierte Builds (Flatpak, Pakete) nutzen stattdessen
/// <prefix>/share/locale — siehe core/src/i18n.rs.
fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let po_dir = manifest_dir.parent().unwrap().join("po");
    let locale_root = PathBuf::from(env::var("OUT_DIR").unwrap()).join("locale");

    println!(
        "cargo:rustc-env=REINSCHRIFT_DEV_LOCALEDIR={}",
        locale_root.display()
    );
    println!("cargo:rerun-if-changed={}", po_dir.display());

    let Ok(entries) = fs::read_dir(&po_dir) else {
        println!("cargo:warning=po/-Verzeichnis nicht gefunden — Dev-Build ohne Übersetzungen");
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("po") {
            continue;
        }
        let lang = path.file_stem().unwrap().to_string_lossy().to_string();
        let mo_dir = locale_root.join(&lang).join("LC_MESSAGES");
        fs::create_dir_all(&mo_dir).unwrap();
        let mo = mo_dir.join("reinschrift.mo");
        let status = Command::new("msgfmt")
            .arg("--check")
            .arg("-o")
            .arg(&mo)
            .arg(&path)
            .status();
        match status {
            Ok(s) if s.success() => {}
            _ => println!(
                "cargo:warning=msgfmt für {lang}.po fehlgeschlagen — Übersetzung fehlt im Dev-Build (gettext-Werkzeuge installieren?)"
            ),
        }
    }
}
