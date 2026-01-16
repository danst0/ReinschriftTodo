# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Reinschrift is a dual-platform todo application:
- **Desktop**: Native Rust + libadwaita (GTK4) application for GNOME
- **Web**: Python Flask application with Docker support

Todos are stored as plain Markdown files, making them version-controllable and editor-agnostic. Supports WebDAV/Nextcloud synchronization.

## Build Commands

### Desktop Application (Rust)
```bash
# Development build & run
cargo run --release

# With custom database path
TODOS_DB_PATH=/path/to/db.md cargo run

# With custom language
cargo run -- --language en

# Production build
cargo build --release

# Pre-commit check (runs automatically via git hooks)
cargo check --locked --workspace

# Skip cargo check on commit
SKIP_CARGO_CHECK=1 git commit -m "message"
```

### Web Application (Flask)
```bash
cd webapp
docker-compose up --build
# Access at http://localhost:5000
```

### Git Hooks Setup
```bash
git config core.hooksPath .githooks
```

## Architecture

### Desktop App (src/)

- **main.rs**: Entry point, argument parsing (--database, --language)
- **ui.rs**: GTK4/libadwaita UI (~3000 lines)
  - Sort modes: Topic (+project), Location (@context), Date (due:)
  - Features: search, voice transcription (Whisper), AI task parsing
  - Preferences: WebDAV config, AI settings, display preferences
- **data.rs**: Markdown parsing, file I/O, WebDAV sync
  - Regex-based parsing for: +projects, @contexts, due:dates, ^IDs, rec:recurrence, ~note:"text"
  - File fingerprinting for change detection
  - Real-time file monitoring
- **i18n.rs**: Translations (de, en, es, fr, ja, sv) with German fallback

### Web App (webapp/)

- **app.py**: Flask server with form-based and OIDC authentication
- **templates/**: Jinja2 templates for todo list, editor, login
- **static/**: CSS, JS, favicon

## Markdown Todo Format

```markdown
- [ ] Task title +project @context due:2026-01-20 ^ID123 rec:weekly ~note:"Additional details"
- [x] Completed task ✅ 2026-01-15
```

**Parsed fields**: title, project (+), context (@), due date, reference ID (^), recurrence (rec:), note (~note:), completion status (✅)

## Key Dependencies

**Rust**: gtk4/adw (0.8/0.6), chrono, regex, reqwest, whisper-rs, cpal, tokio, serde

**Python**: Flask, markdown, requests, Flask-WTF, Authlib

## Environment Variables

Desktop: `TODOS_DB_PATH`

Web: `TODOS_DB_PATH`, `SECRET_KEY`, `APP_USER`, `APP_PASSWORD`, `OIDC_*`, `WEBDAV_*`, `AI_TIMEOUT_SECS`
