# Übersetzungen / Translations

Reinschrift verwendet GNU **gettext** (Domain `reinschrift`). Die Quellsprache
der Strings im Rust-Code ist **Englisch** — der englische Text ist zugleich die
msgid. Unübersetzte Strings erscheinen daher auf Englisch (Standard-gettext-
Fallback).

## Dateien

| Datei | Zweck |
|---|---|
| `reinschrift.pot` | Vorlage (Template) mit allen übersetzbaren Strings |
| `de.po`, `es.po`, `fr.po`, `ja.po`, `sv.po` | Übersetzungen |
| `LINGUAS` | Liste der ausgelieferten Sprachen |
| `convert.py` | Historisches Einmal-Skript der Migration vom alten JSON-System (v0.24.x); die referenzierten JSON-Dateien existieren nicht mehr |

## Strings aus dem Quellcode extrahieren (POT aktualisieren)

Mit [`xtr`](https://crates.io/crates/xtr) (gettext-Extraktor für Rust):

```bash
cargo install xtr

# Aus dem Repo-Root; extrahiert t!/gettext-Aufrufe aus allen Crates
xtr core/src/lib.rs gui/src/main.rs cli/src/main.rs \
    --default-domain reinschrift \
    --copyright-holder "Daniel Dumke" \
    --package-name reinschrift \
    -o po/reinschrift.pot
```

Hinweis: Die Übersetzungsfunktionen heißen in diesem Projekt `t()` (gettext)
und `tc()` (pgettext mit msgctxt, z. B.
`tc("conflict dialog button", "Reload")`). Falls die verwendete xtr-Version
diese Namen nicht automatisch erkennt, mit Keyword-Angabe extrahieren:

```bash
xtr ... -k t -k tc:1c,2
```

## Bestehende Übersetzungen aktualisieren

Nach einer POT-Aktualisierung neue/geänderte Strings in die .po-Dateien
übernehmen:

```bash
for po in po/*.po; do
    msgmerge --update --backup=off "$po" po/reinschrift.pot
done
```

Anschließend fehlende Übersetzungen (mit `msgattrib --untranslated` auffindbar)
ergänzen und prüfen:

```bash
msgfmt --check --statistics -o /dev/null po/de.po
```

## Neue Sprache hinzufügen

```bash
msginit --locale=<lang> --input=po/reinschrift.pot --output=po/<lang>.po
```

Danach `<lang>` in `po/LINGUAS` und in `KNOWN_LANGS`
(`core/src/i18n.rs`) eintragen.

## Wie die Kataloge geladen werden

- **Flatpak:** `/app/share/locale` (das Manifest `me.dumke.Reinschrift.yml`
  kompiliert die .po-Dateien beim Build per `msgfmt`)
- **Installierte Builds (z. B. AUR):** `<prefix>/share/locale` relativ zur
  Binärdatei bzw. `/usr/share/locale`
- **Entwicklung/Tests (`cargo run`, `cargo test`):** `core/build.rs`
  kompiliert `po/*.po` automatisch nach `OUT_DIR/locale` — dafür müssen die
  gettext-Werkzeuge (`msgfmt`) installiert sein
- Übersteuerung für Sonderfälle: Umgebungsvariable `REINSCHRIFT_LOCALEDIR`

Die Sprachwahl folgt gettext-Konventionen (`LANGUAGE`, `LC_ALL`,
`LC_MESSAGES`, `LANG`); zusätzlich erzwingt `--language <lang>` (GUI und CLI)
eine Sprache unabhängig von der System-Locale.
