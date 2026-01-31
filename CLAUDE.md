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
│       ├── lib.rs          # Module declarations and re-exports
│       ├── data.rs         # Backward-compatible re-exports from modules
│       ├── types.rs        # TodoItem, TodoKey, DEFAULT_DUE_TIME
│       ├── config.rs       # BackendConfig, path management
│       ├── webdav.rs       # WebDAV client operations
│       ├── storage.rs      # Unified read/write abstraction
│       ├── parser.rs       # Regex patterns, parse_line, extract_title
│       ├── renderer.rs     # render_line, rewrite_line, rewrite_due
│       ├── todo.rs         # Business logic (load, toggle, add, delete)
│       ├── util.rs         # String helpers, marker generation
│       ├── i18n.rs         # Internationalization
│       ├── sorting.rs      # Sorting functions
│       ├── preferences.rs  # User preferences
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

# Docker deployment
docker-compose up --build
# Access at http://localhost:5000

# Local development
python run.py
# Or: flask run

# Run tests
pip install -r requirements-dev.txt
pytest tests/ -v

# Type checking
mypy app/
```

### Git Hooks Setup
```bash
git config core.hooksPath .githooks
```

## Architecture

### Core Library (core/)

Modular design with focused responsibilities:

- **types.rs**: Core data structures
  - `TodoItem`, `TodoKey` structs with Serialize support
  - `DEFAULT_DUE_TIME` constant
- **config.rs**: Configuration management
  - `BackendConfig` enum (Local/WebDav)
  - Path management (`todo_path`, `set_todo_path`)
- **webdav.rs**: WebDAV client operations
  - URL construction, Nextcloud fallback logic
  - `test_webdav_connection`, read/write operations
- **storage.rs**: Unified storage abstraction
  - `read_content`, `write_content`, `get_fingerprint`
  - Dispatches to local filesystem or WebDAV
- **parser.rs**: Markdown parsing
  - Regex patterns for: +projects, @contexts, due:dates, ^IDs, rec:recurrence, ~note:"text"
  - `parse_line`, `extract_title`, `find_line_by_marker`
- **renderer.rs**: Line rendering
  - `render_line`, `rewrite_line`, `rewrite_due`
  - Completion marker handling
- **todo.rs**: Business logic
  - `load_todos`, `toggle_todo`, `add_todo`, `delete_todo`
  - Due date operations (`set_due_today/tomorrow/weekend/sometime`)
  - `next_due_date` for recurrence
- **util.rs**: String utilities
  - `generate_marker`, `encode_base36`
  - Note escaping/unescaping, token normalization
- **data.rs**: Backward-compatible re-exports from all modules
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

Modular Flask application with proper separation of concerns:

```
webapp/
├── app/
│   ├── __init__.py          # Flask app factory
│   ├── config.py            # Configuration classes
│   ├── extensions.py        # Flask extensions (CSRF, OAuth)
│   ├── exceptions.py        # Custom exception classes
│   ├── models/
│   │   └── todo.py          # TodoItem dataclass, regex patterns
│   ├── services/
│   │   ├── parser.py        # parse_line, extract_title
│   │   ├── storage.py       # read/write (local + WebDAV)
│   │   ├── todo_service.py  # CRUD operations
│   │   ├── date_service.py  # Date calculations, next_due_date
│   │   ├── sorting.py       # Sort key functions
│   │   └── ai_service.py    # AI parsing, context building
│   ├── routes/
│   │   ├── main.py          # Index, language
│   │   ├── auth.py          # Login, OIDC
│   │   ├── todo.py          # Toggle, postpone, edit, delete
│   │   └── api.py           # JSON API endpoints
│   └── utils/
│       ├── markers.py       # generate_marker
│       ├── escaping.py      # escape/unescape_note
│       └── helpers.py       # parse_flag, format helpers
├── static/
│   ├── css/styles.css
│   └── js/
│       ├── app.js           # Main entry point
│       └── modules/         # ES6 modules (api, modal, drag-drop, etc.)
├── templates/               # Jinja2 templates
├── tests/                   # pytest test suite
├── run.py                   # Entry point
├── app.py                   # Legacy entry point (deprecated)
├── requirements.txt
└── pyproject.toml           # Project configuration
```

**JS Modules (static/js/modules/):**
- **drag-drop.js**: Drag-and-drop between section groups (projects/contexts) with mouse and touch (long-press) support
- **swipe.js**: Swipe gestures on todo items (right=complete, left=postpone)
- **auto-reload.js**: Background partial reload every 30s
- **pull-refresh.js**: Pull-to-refresh on mobile
- **api.js**: CSRF-aware fetch wrapper
- **modal.js**: Edit modal with bottom-sheet on mobile
- **todo-actions.js**: Toggle, postpone, group postpone
- **ai.js**: Smart Add with Ollama/LLM parsing
- **filters.js**: Filter UI state management
- **keyboard.js**: Keyboard shortcuts
- **voice.js**: Web Speech API voice input

**Key Services:**
- **parser.py**: Regex patterns matching Rust core, parse_line, extract_title
- **storage.py**: Unified storage abstraction (local + WebDAV)
- **todo_service.py**: load_todos, toggle_todo, add_todo, delete_todo
- **ai_service.py**: Ollama/LLM integration for natural language parsing

**Routes (Blueprints):**
- `main_bp`: Index view, language selection
- `auth_bp`: Login (form + OIDC), logout
- `todo_bp`: Toggle, postpone, edit, delete, add
- `api_bp`: JSON endpoints (/api/todo, /api/parse, /api/add, /api/move-to-section)

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

## Versioning & Release

Before every commit, update the version and all Flatpak manifests:

1. **Increment version**: Increase patch by 1. If patch reaches 9, reset to 0 and increment minor. If minor reaches 9, reset to 0 and increment major. (e.g. 0.18.8 → 0.18.9 → 0.19.0 → 0.19.1)
2. **Update files**:
   - `Cargo.toml` — `version` field in `[workspace.package]`
   - `me.dumke.Reinschrift.yml` — `tag:` value
   - `me.dumke.Reinschrift.metainfo.xml` — add new `<release>` entry at top of `<releases>` with today's date
3. **Commit message**: Prefix with `v<version>:` (e.g. `v0.18.12: Add feature X`)

## Environment Variables

Desktop/CLI: `TODOS_DB_PATH`

Web: `TODOS_DB_PATH`, `SECRET_KEY`, `APP_USER`, `APP_PASSWORD`, `OIDC_*`, `WEBDAV_*`, `AI_TIMEOUT_SECS`
