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
- Die Liste zeigt alle offenen und erledigten Einträge aus der Markdown-Datei.
- Ein Klick auf die Checkbox aktualisiert den Eintrag (Checkbox + `✅ YYYY-MM-DD`) direkt im Markdown.
- Über den Refresh-Button (oder `Ctrl+R`) lässt sich die Datei jederzeit neu einlesen.
- Änderungen außerhalb der App werden über einen Dateimonitor automatisch erkannt und eingelesen (sofern das Dateisystem es unterstützt).
