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

Standardmäßig greift die App auf die Datei `TodosDatenbank.md` im Projektverzeichnis zu. Wenn du eine andere Datei verwenden möchtest, setze vor dem Start die Umgebungsvariable `TODOS_DB_PATH`, z. B. `TODOS_DB_PATH=/pfad/zur/TodosDatenbank.md cargo run`.

## Bedienung
- Die Liste blendet erledigte Einträge aus und zeigt nur noch offene Aufgaben; falls du erledigte Aufgaben sehen möchtest, kannst du sie im Einstellungsfenster temporär einblenden.
- Direkt neben der Sortierauswahl kannst du die Checkbox "Nur fällige anzeigen" aktivieren, um Aufgaben mit Fälligkeit heute/überfällig sowie Aufgaben ohne Datum zu sehen und zukünftige Einträge auszublenden (Einstellung wird gespeichert).
- Oben kannst du per Auswahlfeld bestimmen, ob die Liste nach Projekten (`+`), Orten (`@`) oder Fälligkeitsdatum sortiert wird. Bei Projekten/Orten wird zusätzlich je Gruppe ein Zwischenüberschrift angezeigt; beim Datum stehen Aufgaben ohne Fälligkeitsdatum ganz oben. Die App merkt sich deine letzte Auswahl für den nächsten Start.
- Ein Klick auf die Checkbox aktualisiert den Eintrag (Checkbox + `✅ YYYY-MM-DD`) direkt im Markdown.
- Ein Doppelklick auf den Text eines Eintrags öffnet ein Detailfenster, in dem du Titel, Projekt, Ort, Fälligkeitsdatum, Referenz und Status bearbeiten kannst.
- Über das Kalender-Symbol setzt du die Fälligkeit auf heute, der Pfeil direkt daneben verschiebt sie auf morgen.
- Über den Refresh-Button (oder `Ctrl+R`) lässt sich die Datei jederzeit neu einlesen.
- Änderungen außerhalb der App werden über einen Dateimonitor automatisch erkannt und eingelesen (sofern das Dateisystem es unterstützt).
- Ein Klick auf das Hamburger-Symbol öffnet ein Einstellungsfenster, in dem du erledigte Aufgaben ein-/ausblendest, den Filter "Nur fällige" steuerst und die zu verwendende Markdown-Datei auswählst. Die Änderungen werden dauerhaft gespeichert.
- Über die Tastaturkürzel `Ctrl+W`, `Ctrl+Q` und `Alt+F4` kannst du das Fenster jederzeit schließen.

## Offene Todos
- *(keine)*

## Erledigte Todos
- [x] Auf Strg-W und Strg-Q und Alt-F4 reagieren
- [x] Hamburgermenü mit Einstellungen
- [x] Einstellung, ob erledigte Tdodos angezeigt werden (default ist aus)
- [x] Einstellung, auf welche Datei zugegriffen wird