#!/usr/bin/env python3
"""Einmaliges Konvertierungsskript: core/src/i18n/*.json -> gettext .po/.pot.

Die englischen Texte (en.json) werden zur msgid-Quelle, alle anderen
Sprachen werden als Übersetzungen übernommen. Das Skript bricht ab, wenn
dabei eine Übersetzung verloren ginge oder Platzhalter nicht zusammenpassen.

Aufruf (aus dem Repo-Root):
    python3 po/convert.py

Erzeugt: po/reinschrift.pot, po/{de,es,fr,ja,sv}.po, po/LINGUAS
"""

import json
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
JSON_DIR = REPO / "core" / "src" / "i18n"
PO_DIR = REPO / "po"

SOURCE_LANG = "en"
TARGET_LANGS = ["de", "es", "fr", "ja", "sv"]

# Schlüssel, deren englischer Text mit einem anderen Schlüssel kollidiert,
# aber abweichend übersetzt wird -> brauchen einen gettext-Kontext (msgctxt).
# Muss zu den tc()-Aufrufen im Rust-Code passen!
CONTEXTS = {
    "conflict_reload": "conflict dialog button",
}

PLURAL_FORMS = {
    "de": "nplurals=2; plural=(n != 1);",
    "es": "nplurals=2; plural=(n != 1);",
    "fr": "nplurals=2; plural=(n > 1);",
    "ja": "nplurals=1; plural=0;",
    "sv": "nplurals=2; plural=(n != 1);",
}

LANG_TEAM = {
    "de": "German",
    "es": "Spanish",
    "fr": "French",
    "ja": "Japanese",
    "sv": "Swedish",
}


def po_escape(s: str) -> str:
    return (
        s.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("\t", "\\t")
        .replace("\n", "\\n")
    )


def header(lang: str | None) -> str:
    lines = [
        'msgid ""',
        'msgstr ""',
        '"Project-Id-Version: reinschrift\\n"',
        '"Report-Msgid-Bugs-To: https://github.com/danst0/ReinschriftTodo/issues\\n"',
        '"POT-Creation-Date: 2026-06-07 12:00+0200\\n"',
        '"PO-Revision-Date: 2026-06-07 12:00+0200\\n"',
        '"MIME-Version: 1.0\\n"',
        '"Content-Type: text/plain; charset=UTF-8\\n"',
        '"Content-Transfer-Encoding: 8bit\\n"',
    ]
    if lang:
        lines.insert(5, f'"Language-Team: {LANG_TEAM[lang]}\\n"')
        lines.insert(5, f'"Language: {lang}\\n"')
        lines.append(f'"Plural-Forms: {PLURAL_FORMS[lang]}\\n"')
    return "\n".join(lines) + "\n"


def load() -> dict[str, dict[str, str]]:
    data = {}
    for lang in [SOURCE_LANG, *TARGET_LANGS]:
        path = JSON_DIR / f"{lang}.json"
        data[lang] = json.loads(path.read_text(encoding="utf-8"))
    return data


def main() -> int:
    data = load()
    en = data[SOURCE_LANG]
    errors = []

    # Vollständigkeit: jede Sprache muss exakt die en-Schlüssel haben.
    for lang in TARGET_LANGS:
        missing = set(en) - set(data[lang])
        extra = set(data[lang]) - set(en)
        if missing:
            errors.append(f"{lang}.json: fehlende Schlüssel {sorted(missing)}")
        if extra:
            errors.append(f"{lang}.json: überzählige Schlüssel {sorted(extra)}")

    # Entries: (msgctxt, msgid) -> {"keys": [...], "tr": {lang: msgstr}}
    entries: dict[tuple[str | None, str], dict] = {}
    for key, msgid in en.items():
        ctx = CONTEXTS.get(key)
        ident = (ctx, msgid)
        tr = {lang: data[lang][key] for lang in TARGET_LANGS if key in data[lang]}
        if ident in entries:
            # Zusammenführen nur erlaubt, wenn alle Übersetzungen identisch
            # sind – sonst ginge eine Übersetzung verloren.
            prev = entries[ident]
            for lang, val in tr.items():
                if prev["tr"].get(lang) != val:
                    errors.append(
                        f"Kollision für msgid {msgid!r} (Schlüssel "
                        f"{prev['keys']} vs. {key!r}, Sprache {lang}): "
                        f"{prev['tr'].get(lang)!r} != {val!r} — msgctxt in "
                        f"CONTEXTS ergänzen!"
                    )
            prev["keys"].append(key)
        else:
            entries[ident] = {"keys": [key], "tr": tr}

        # Platzhalter-Konsistenz: Anzahl '{}' muss überall übereinstimmen.
        n = msgid.count("{}")
        for lang, val in tr.items():
            if val.count("{}") != n:
                errors.append(
                    f"Platzhalter-Abweichung bei {key!r} ({lang}): "
                    f"en={msgid!r}, {lang}={val!r}"
                )

    if errors:
        for e in errors:
            print(f"FEHLER: {e}", file=sys.stderr)
        return 1

    def entry_text(ctx: str | None, msgid: str, msgstr: str, keys: list[str]) -> str:
        out = [f"#. ehemaliger JSON-Schlüssel: {', '.join(keys)}"]
        if ctx is not None:
            out.append(f'msgctxt "{po_escape(ctx)}"')
        out.append(f'msgid "{po_escape(msgid)}"')
        out.append(f'msgstr "{po_escape(msgstr)}"')
        return "\n".join(out) + "\n"

    PO_DIR.mkdir(exist_ok=True)

    pot = [header(None)]
    for (ctx, msgid), e in entries.items():
        pot.append(entry_text(ctx, msgid, "", e["keys"]))
    (PO_DIR / "reinschrift.pot").write_text("\n".join(pot), encoding="utf-8")

    for lang in TARGET_LANGS:
        po = [header(lang)]
        for (ctx, msgid), e in entries.items():
            po.append(entry_text(ctx, msgid, e["tr"][lang], e["keys"]))
        (PO_DIR / f"{lang}.po").write_text("\n".join(po), encoding="utf-8")

    (PO_DIR / "LINGUAS").write_text(
        "# Verfügbare Übersetzungen (Quellsprache der msgids: en)\n"
        + "\n".join(TARGET_LANGS)
        + "\n",
        encoding="utf-8",
    )

    n_entries = len(entries)
    print(f"OK: {n_entries} Einträge -> reinschrift.pot + {len(TARGET_LANGS)} .po-Dateien")
    return 0


if __name__ == "__main__":
    sys.exit(main())
