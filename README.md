# Reinschrift

Rust/libadwaita Anwendung, die die Aufgaben aus deiner Markdown-Datei `TodosDatenbank.md` lädt, sie in einer GNOME-Oberfläche anzeigt und das Abhaken direkt zurück in dieselbe Datei schreibt.

## Beschreibung für Flathub
Reinschrift kombiniert eine native GNOME-Oberfläche mit der Einfachheit von reinem Text. Deine Aufgaben bleiben eine ganz normale Markdown-Datei – leicht zu sichern, per Git versionierbar und auf jedem Gerät mit deinem Lieblingseditor editierbar. Ob du die Datei über Nextcloud/WebDAV teilst, im Terminal bearbeitest oder Automationen darüber laufen lässt: Das Format bleibt offen, portabel und transparent. Die App liest Änderungen live ein, schreibt direkt zurück und macht damit klassische To-do-Listen im Klartext alltagstauglich – ohne proprietäre Silos.

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

## Web App
Eine einfache Web-Oberfläche ist im Ordner `webapp/` verfügbar. Sie nutzt Docker Compose.

Starten:
```bash
cd webapp
docker-compose up --build
```
Die App ist dann unter `http://localhost:5000` erreichbar.
Login-Daten (konfigurierbar in `docker-compose.yml`):
- Benutzer: `admin`
- Passwort: `secret`

## Offene Todos
- [x] make the webapp mobile first compliant
- [x] webapp: add the details edit view
- [x] webapp: add the postpone functions as with the rust app
- [x] webapp: logout button on the bottom of the page

## Erledigte Todos
- [x] Esc closes the Preferences Window
- [x] for the webapp please use long lifed cookies to prevent further needs for login
- [x] Add a license CC BY NC SA
- [x] add another way to connect to the database, via a direct connection to nextcloud via webdav. please add it as an option in the settings window
- [x] remove the checkmark from the TodosDatenbank file
- [x] add a simple Webapp with the same layout and functionality. Web App based on docker compose, python, flask, login based on user and password in docker compose env 
- [x] Auf Strg-W und Strg-Q und Alt-F4 reagieren
- [x] Hamburgermenü mit Einstellungen
- [x] Einstellung, ob erledigte Tdodos angezeigt werden (default ist aus)
- [x] Einstellung, auf welche Datei zugegriffen wird