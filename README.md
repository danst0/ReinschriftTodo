# Todos Extension

Rust/libadwaita Anwendung, die die Aufgaben aus deiner Markdown-Datei `TodosDatenbank.md` lädt, sie in einer GNOME-Oberfläche anzeigt und das Abhaken direkt zurück in dieselbe Datei schreibt.

## Voraussetzungen
- Rust Toolchain (Edition 2024)
- GTK4 und Libadwaita Laufzeitbibliotheken (`libgtk-4-dev`, `libadwaita-1-dev` o.ä.)

## Entwicklung
```bash
cd "/home/danst/Nextcloud/Projekte/2025-12 Todos Extension"
cargo run --release
```

Beim Start erwartet die App, dass die produktive Markdown-Datei unter `/home/danst/Nextcloud/InOmnibusVeritas/TodosDatenbank.md` erreichbar ist. Du kannst den Pfad in `src/data.rs` über die Konstante `TODO_DB_PATH` anpassen oder eine Symlink auf die Datei setzen.

## Bedienung
- Die Liste blendet erledigte Einträge aus und zeigt nur noch offene Aufgaben aus der Markdown-Datei.
- Oben kannst du per Auswahlfeld bestimmen, ob die Liste nach Projekten (`+`), Orten (`@`) oder Fälligkeitsdatum sortiert wird. Bei Projekten/Orten wird zusätzlich je Gruppe ein Zwischenüberschrift angezeigt; beim Datum stehen Aufgaben ohne Fälligkeitsdatum ganz oben.
- Ein Klick auf die Checkbox aktualisiert den Eintrag (Checkbox + `✅ YYYY-MM-DD`) direkt im Markdown.
- Über das Pfeilsymbol rechts neben einem Eintrag verschiebst du dessen `due:`-Datum automatisch auf morgen.
- Über den Refresh-Button (oder `Ctrl+R`) lässt sich die Datei jederzeit neu einlesen.
- Änderungen außerhalb der App werden über einen Dateimonitor automatisch erkannt und eingelesen (sofern das Dateisystem es unterstützt).

## Offene Todos
- [ ] Auf Strg-W und Strg-Q und Alt-F4 reagieren
- [ ] Hamburgermenü mit Einstellungen
- [ ] Einstellung, ob erledigte Tdodos angezeigt werden (default ist aus)