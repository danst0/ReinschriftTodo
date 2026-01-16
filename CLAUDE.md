# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Reinschrift is a multi-platform todo application:
- **Desktop GUI**: Native Rust + libadwaita (GTK4) application for GNOME
- **CLI**: Command-line interface for terminal workflows
- **Web**: Python Flask application with Docker support

Todos are stored as plain Markdown files, making them version-controllable and editor-agnostic. Supports WebDAV/Nextcloud synchronization.

## Project Structure (Cargo Workspace)

```
reinschrift/
├── Cargo.toml              # Workspace root
├── core/                   # Shared library (reinschrift-core)
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs          # Re-exports
│       ├── data.rs         # Data handling, file I/O, WebDAV
│       ├── i18n.rs         # Internationalization
│       ├── sorting.rs      # Sorting functions
│       └── i18n/           # Translation JSON files
├── gui/                    # GTK application (reinschrift-gui)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs         # GUI entry point
│       └── ui.rs           # GTK4/libadwaita UI
├── cli/                    # CLI application (reinschrift-cli)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs         # CLI entry point (clap)
│       ├── commands.rs     # Command implementations
│       └── output.rs       # Colored terminal output
└── webapp/                 # Web application (Python Flask)
```

## Build Commands

### Workspace (all Rust crates)
```bash
# Build entire workspace
cargo build --workspace --release

# Check entire workspace
cargo check --workspace

# Build specific crate
cargo build -p reinschrift-gui --release
cargo build -p reinschrift-cli --release
```

### GUI Application
```bash
# Development build & run
cargo run -p reinschrift-gui --release

# With custom database path
TODOS_DB_PATH=/path/to/db.md cargo run -p reinschrift-gui

# With custom language
cargo run -p reinschrift-gui -- --language en
```

### CLI Application
```bash
# Run CLI
cargo run -p reinschrift-cli -- list
cargo run -p reinschrift-cli -- add "New task +project @context"
cargo run -p reinschrift-cli -- done ^marker

# After build, binaries are at:
# - target/release/reinschrift_todo (GUI)
# - target/release/reinschrift (CLI)
```

### CLI Command Reference
```
reinschrift [OPTIONS] <COMMAND>

OPTIONS:
    -d, --database <PATH>    Path to markdown database
    -l, --language <LANG>    Language (de, en, es, fr, ja, sv)
    -j, --json               JSON output for scripting
        --no-color           Disable colored output

COMMANDS:
    list (ls)       List todos with filtering/sorting
    add             Add a new todo
    edit            Edit an existing todo
    delete (rm)     Delete todo(s)
    done            Mark as completed
    undone          Mark as incomplete
    today           Set due date to today
    tomorrow        Set due date to tomorrow
    search          Search todos
    config          WebDAV and AI configuration
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

### Core Library (core/)

- **data.rs**: Markdown parsing, file I/O, WebDAV sync
  - Regex-based parsing for: +projects, @contexts, due:dates, ^IDs, rec:recurrence, ~note:"text"
  - File fingerprinting for change detection
  - `TodoItem` and `TodoKey` structs with Serialize support
- **i18n.rs**: Translations (de, en, es, fr, ja, sv) with German fallback
  - Environment-based language detection (LANGUAGE, LC_ALL, LANG)
- **sorting.rs**: `SortMode` enum and sorting functions
  - Topic (+project), Location (@context), Date (due:)

### GUI App (gui/)

- **main.rs**: Entry point, argument parsing (--database, --language)
- **ui.rs**: GTK4/libadwaita UI (~2800 lines)
  - Features: search, voice transcription (Whisper), AI task parsing
  - Preferences: WebDAV config, AI settings, display preferences

### CLI App (cli/)

- **main.rs**: Clap-based argument parsing
- **commands.rs**: Implementation of all CLI commands
- **output.rs**: Colored terminal output and JSON formatting

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

**Core**: anyhow, chrono, regex, serde, serde_json, reqwest, once_cell

**GUI**: reinschrift-core, gtk4/adw (0.8/0.6), whisper-rs, cpal, tokio

**CLI**: reinschrift-core, clap (4), colored (2)

**Python**: Flask, markdown, requests, Flask-WTF, Authlib

## Environment Variables

Desktop/CLI: `TODOS_DB_PATH`

Web: `TODOS_DB_PATH`, `SECRET_KEY`, `APP_USER`, `APP_PASSWORD`, `OIDC_*`, `WEBDAV_*`, `AI_TIMEOUT_SECS`
