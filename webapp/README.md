# Reinschrift web app

The self-hostable web app of [Reinschrift](../README.md): the same Markdown todo file in the browser, installable as a PWA on phones, with push reminders, share links and CalDAV sync for iOS Reminders.

## Getting started (Docker)

A pre-built image `ghcr.io/danst0/reinschrift-web` is published on every release (`latest` and one tag per release, e.g. `1.2.3`).

```bash
git clone https://github.com/danst0/ReinschriftTodo && cd ReinschriftTodo/webapp
cp docker-compose.example.yml docker-compose.yml
touch ../TodosDatenbank.md        # must exist, or Docker mounts an empty directory instead
$EDITOR docker-compose.yml        # at least SECRET_KEY, APP_USER, APP_PASSWORD, TZ
docker compose up -d
```

Open `http://localhost:5000` and log in. The todo file is mounted at `/data/TodosDatenbank.md`; point the volume at the file your desktop app uses (e.g. inside your Nextcloud folder), or use WebDAV instead (below). The `/config` volume keeps the settings, sessions, push subscriptions and CalDAV state — keep it across updates.

Generate a secret key once, e.g. with `python3 -c 'import secrets; print(secrets.token_hex(32))'`. Without `SECRET_KEY` the app falls back to a fixed development key and anyone could forge a session.

Updating: `docker compose pull && docker compose up -d`.

### Behind a reverse proxy (HTTPS)

Push reminders, the PWA install and CalDAV for iOS need HTTPS. Put the app behind a reverse proxy (Traefik, Caddy, nginx, …) that forwards `X-Forwarded-Proto`/`X-Forwarded-Host` (the app honours them) and passes the WebDAV methods `PROPFIND` and `REPORT` through to `/caldav/`.

## Configuration

Everything is configured via environment variables in `docker-compose.yml`.

### Storage

| Variable | Meaning |
|---|---|
| `TODOS_DB_PATH` | The todo file inside the container (default in the example: `/data/TodosDatenbank.md`) |
| `CONFIG_PATH` | Settings file; its directory also holds sessions, push and CalDAV state (default `/config/settings.json`) |
| `USE_WEBDAV` | `true` reads and writes the file on a WebDAV server instead of `TODOS_DB_PATH` |
| `WEBDAV_URL` | Full URL of the file, e.g. `https://cloud.example.com/remote.php/dav/files/USER/todos.md` |
| `WEBDAV_USERNAME`, `WEBDAV_PASSWORD` | WebDAV credentials (for Nextcloud: an app password) |
| `TZ` | Time zone — due dates in the file are local time, reminders need to know which (e.g. `Europe/Berlin`) |

### Login

| Variable | Meaning |
|---|---|
| `SECRET_KEY` | Signs sessions and CSRF tokens — **set a random value** |
| `APP_USER`, `APP_PASSWORD` | User name and password for the login form |
| `OIDC_ISSUER` | OpenID Connect discovery URL (`…/.well-known/openid-configuration`) |
| `OIDC_CLIENT_ID`, `OIDC_CLIENT_SECRET` | OIDC client |
| `OIDC_ALLOWED_USER` | E-mail address that may log in via OIDC |
| `OIDC_REDIRECT_URI` | Callback, `https://your-domain/authorize` (derived from the request if unset) |

The login form appears when `APP_USER` and `APP_PASSWORD` are set, the OIDC button when `OIDC_ISSUER` and `OIDC_CLIENT_ID` are set; both can be active at once.

### Push reminders

| Variable | Meaning |
|---|---|
| `PUSH_ENABLED` | `true` by default |
| `PUSH_LEAD_MINUTES` | Reminder lead time for tasks with a time (default `15`) |
| `PUSH_ALL_DAY_TIME` | When tasks without a time are announced (default `08:00`) |
| `VAPID_PRIVATE_KEY` | Optional; otherwise a key is generated and kept in `/config` |
| `VAPID_SUBJECT` | Contact for the push services, e.g. `mailto:you@example.com` |

Each device subscribes itself: *Settings → Push reminders on this device* in the web app. On iOS this works only for the app added to the home screen (PWA).

### AI (optional)

| Variable | Meaning |
|---|---|
| `OLLAMA_URL`, `OLLAMA_MODEL` | Ollama server and model for *Smart Add* (natural language → task) |
| `AI_TIMEOUT_SECS` | Timeout for AI requests (default `30`) |
| `SEMANTIC_ENABLED`, `EMBEDDING_MODEL` | Similar-task suggestions via an embedding model (default `bge-m3`, pull it first) |

### iOS Reminders (CalDAV)

| Variable | Meaning |
|---|---|
| `CALDAV_ENABLED` | `true` serves the tasks under `/caldav/` (default `false`) |
| `CALDAV_USER`, `CALDAV_PASSWORD` | Credentials for the device; fall back to `APP_USER`/`APP_PASSWORD` |

Setup on the iPhone: *Settings → Apps → Reminders → Accounts → Add Account → Other → Add CalDAV Account*, server `https://your-domain/caldav/`, then user name and password. The web app's settings page shows server and user once CalDAV is on.

How it maps: all tasks are in one list, "Reinschrift"; projects appear as hashtags and places with `@` after the title (`Mow the lawn #Garden @Home`). Edit them right there: type `#Garden` (or `+Garden`) to add a project, `@Home` for a place, delete a tag to remove it; names with spaces are quoted (`#"Big Project"`). iOS keeps real tags to iCloud lists, so in this list they are plain text — no tag browser or smart lists.

Reminders you set on the iPhone (at a time or at a location) are kept on the server and come back on every sync; they do not change the task in the file. When a recurring task is completed, its location reminders and those relative to the due date move on to the next occurrence. Completing, editing, deleting and adding sync both ways; completing a recurring task creates its next occurrence. Completed tasks from the last 30 days stay visible on the device, older ones only in the file.

## Using the web app

- **Install as an app**: in the browser menu choose *Install* / *Add to Home Screen*.
- **Share a project or place**: the menu of a project or place group creates a link (`/s/<token>`) that lets others see, tick off and add tasks of just that group — without a login. Anyone with the link has that access; *Remove link* in the same dialog revokes it.
- **Settings** (gear icon): language, Smart Add, delete confirmation, title autocomplete, classic typeface, push reminders, CalDAV details and *Send Feedback*.
- **Keyboard shortcuts**: press `?`.

## Local development

```bash
cd webapp
pip install -r requirements.txt -r requirements-dev.txt
TODOS_DB_PATH=TodosDatenbank.md CONFIG_PATH=./settings.json APP_USER=dev APP_PASSWORD=dev python run.py
# → http://localhost:5000

pytest tests/ -v
mypy app/
```
