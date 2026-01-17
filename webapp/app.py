import os
import re
import json
import time
import logging
import secrets
import string
from datetime import datetime, timedelta, time as dtime
from flask import Flask, render_template, request, redirect, url_for, session, flash, Response, stream_with_context
from werkzeug.middleware.proxy_fix import ProxyFix
import requests
from requests.auth import HTTPBasicAuth
from flask_wtf.csrf import CSRFProtect, CSRFError
from authlib.integrations.flask_client import OAuth
from translations import TRANSLATIONS

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(name)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

app = Flask(__name__)
app.secret_key = os.environ.get('SECRET_KEY', 'dev_secret_key')
app.permanent_session_lifetime = timedelta(days=30)
csrf = CSRFProtect(app)

# Honor reverse proxy headers (e.g., X-Forwarded-Proto) so url_for builds correct HTTPS URLs.
app.wsgi_app = ProxyFix(app.wsgi_app, x_for=1, x_proto=1, x_host=1, x_port=1, x_prefix=1)

# Ensure session cookies survive cross-site OAuth redirects.
app.config.update(
    SESSION_COOKIE_SAMESITE='None',
    SESSION_COOKIE_SECURE=True,
    SESSION_COOKIE_HTTPONLY=True,
)

oauth = OAuth(app)
oauth.register(
    name='oidc',
    server_metadata_url=os.environ.get('OIDC_ISSUER'),
    client_id=os.environ.get('OIDC_CLIENT_ID'),
    client_secret=os.environ.get('OIDC_CLIENT_SECRET'),
    client_kwargs={'scope': 'openid email profile'},
)

OIDC_REDIRECT_URI = os.environ.get('OIDC_REDIRECT_URI')

def get_locale():
    # Check session first
    if 'lang' in session:
        return session['lang']
    
    # Check Accept-Language header
    accept_languages = request.accept_languages.best_match(TRANSLATIONS.keys())
    if accept_languages:
        return accept_languages
    
    return 'de' # Default language

@app.context_processor
def inject_translations():
    lang = get_locale()
    return {
        't': TRANSLATIONS.get(lang, TRANSLATIONS['de']),
        'current_lang': lang
    }

TODO_PATH = os.environ.get('TODOS_DB_PATH', 'TodosDatenbank.md')
CONFIG_PATH = os.environ.get('CONFIG_PATH', '/config/settings.json')
DEFAULT_AI_TIMEOUT_SECS = int(os.environ.get('AI_TIMEOUT_SECS', '30'))

RECENT_CONTEXT_WINDOW_DAYS = 30

# WebDAV Configuration
USE_WEBDAV = os.environ.get('USE_WEBDAV', 'false').lower() == 'true'
WEBDAV_URL = os.environ.get('WEBDAV_URL')
WEBDAV_USERNAME = os.environ.get('WEBDAV_USERNAME')
WEBDAV_PASSWORD = os.environ.get('WEBDAV_PASSWORD')

LINK_RE = re.compile(r"\[\[([^\]]+)\]\]")
PROJECT_RE = re.compile(r"\+([^\s]+)")
CONTEXT_RE = re.compile(r"@([^\s]+)")
DUE_RE = re.compile(r"due:(\d{4}-\d{2}-\d{2})(?:T(\d{2}:\d{2}))?")
ID_RE = re.compile(r"\^([A-Za-z0-9]+)")
COMPLETION_RE = re.compile(r"\s✅\s\d{4}-\d{2}-\d{2}")
COMPLETION_DATE_RE = re.compile(r"✅\s(\d{4}-\d{2}-\d{2})")
RECUR_RE = re.compile(r"rec:([^\s]+)")
NOTE_RE = re.compile(r'~note:"((?:\\.|[^"])*)"')

DEFAULT_DUE_TIME = dtime(hour=0, minute=0)
SOMETIME_SENTINEL = datetime(year=9999, month=12, day=31, hour=0, minute=0)


def generate_marker(length=8):
    alphabet = string.ascii_letters + string.digits
    return ''.join(secrets.choice(alphabet) for _ in range(length))


def ensure_marker(existing):
    return existing if existing else generate_marker()


def parse_flag(value, default=True):
    if value is None:
        return default
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return value != 0
    if isinstance(value, str):
        normalized = value.strip().lower()
        if normalized in ('1', 'true', 'yes', 'on', 'y'):
            return True
        if normalized in ('0', 'false', 'no', 'off', 'n', ''):
            return False
    return default

def parse_due_token(text):
    match = DUE_RE.search(text)
    if not match:
        return None

    date_part = match.group(1)
    time_part = match.group(2)

    try:
        date_obj = datetime.strptime(date_part, "%Y-%m-%d").date()
    except ValueError:
        return None

    if time_part:
        try:
            time_obj = datetime.strptime(time_part, "%H:%M").time()
        except ValueError:
            time_obj = DEFAULT_DUE_TIME
    else:
        time_obj = DEFAULT_DUE_TIME

    return datetime.combine(date_obj, time_obj)

def parse_due_input(raw_value):
    if not raw_value:
        return None
    raw_value = raw_value.strip()
    for fmt in ["%Y-%m-%dT%H:%M", "%Y-%m-%d"]:
        try:
            parsed = datetime.strptime(raw_value, fmt)
            if fmt == "%Y-%m-%d":
                parsed = datetime.combine(parsed.date(), DEFAULT_DUE_TIME)
            return parsed
        except ValueError:
            continue
    return None

def format_due(dt_value):
    return dt_value.strftime("%Y-%m-%dT%H:%M")

def format_due_display(dt_value):
    """Format due date for display, omitting time if it's 00:00."""
    if dt_value.hour == 0 and dt_value.minute == 0:
        return dt_value.strftime("%Y-%m-%d")
    return dt_value.strftime("%Y-%m-%dT%H:%M")

def is_sometime(dt_value):
    return dt_value.date().year == 9999

def read_content():
    if USE_WEBDAV:
        if not WEBDAV_URL:
            return ""
        try:
            auth = None
            if WEBDAV_USERNAME and WEBDAV_PASSWORD:
                auth = HTTPBasicAuth(WEBDAV_USERNAME, WEBDAV_PASSWORD)
            
            response = requests.get(WEBDAV_URL, auth=auth, timeout=10)
            response.raise_for_status()
            return response.text
        except Exception as e:
            print(f"WebDAV read error: {e}")
            return ""
    else:
        if not os.path.exists(TODO_PATH):
            return ""
        with open(TODO_PATH, 'r', encoding='utf-8') as f:
            return f.read()

def write_content(content):
    if USE_WEBDAV:
        if not WEBDAV_URL:
            return
        try:
            auth = None
            if WEBDAV_USERNAME and WEBDAV_PASSWORD:
                auth = HTTPBasicAuth(WEBDAV_USERNAME, WEBDAV_PASSWORD)
            
            response = requests.put(WEBDAV_URL, data=content.encode('utf-8'), auth=auth, timeout=10)
            response.raise_for_status()
        except Exception as e:
            print(f"WebDAV write error: {e}")
    else:
        with open(TODO_PATH, 'w', encoding='utf-8') as f:
            f.write(content)

def load_settings():
    if not os.path.exists(CONFIG_PATH):
        return {}
    try:
        with open(CONFIG_PATH, 'r') as f:
            return json.load(f)
    except:
        return {}

def save_settings(settings):
    try:
        os.makedirs(os.path.dirname(CONFIG_PATH), exist_ok=True)
        with open(CONFIG_PATH, 'w') as f:
            json.dump(settings, f)
    except Exception as e:
        print(f"Error saving settings: {e}")

def load_todos():
    content = read_content()
    if not content:
        return []
    
    items = []
    lang = get_locale()
    current_section = TRANSLATIONS.get(lang, TRANSLATIONS['de']).get('no_section', 'Ohne Abschnitt')
    
    for line_index, line in enumerate(content.splitlines()):
        trimmed = line.strip()
        if trimmed.startswith("###"):
            current_section = trimmed.lstrip('#').strip()
            continue
        
        item = parse_line(line, line_index, current_section)
        if item:
            items.append(item)
    
    return items

def parse_line(line, line_index, section):
    trimmed = line.lstrip()
    done = False
    rest = ""
    done_date = None
    
    if trimmed.startswith("- [x]"):
        done = True
        rest = trimmed[5:].strip()
    elif trimmed.startswith("- [X]"):
        done = True
        rest = trimmed[5:].strip()
    elif trimmed.startswith("- [ ]"):
        done = False
        rest = trimmed[5:].strip()
    else:
        return None
    
    title = extract_title(rest)
    project = normalize_prefix(capture_token(PROJECT_RE, rest), '+')
    context = normalize_prefix(capture_token(CONTEXT_RE, rest), '@')
    due_dt = parse_due_token(rest)
    recurrence = capture_token(RECUR_RE, rest)
    due_display = format_due_display(due_dt) if due_dt else None
    due_is_sometime = is_sometime(due_dt) if due_dt else False

    if done:
        date_match = COMPLETION_DATE_RE.search(line)
        if date_match:
            try:
                done_date = datetime.strptime(date_match.group(1), "%Y-%m-%d").date()
            except ValueError:
                done_date = None
    
    reference = capture_token(LINK_RE, rest)
    marker = capture_token(ID_RE, rest)
    raw_note = capture_token(NOTE_RE, rest)
    note = normalize_note(unescape_note(raw_note)) if raw_note else None
    
    return {
        'line_index': line_index,
        'marker': marker,
        'title': title,
        'section': section,
        'project': project,
        'context': context,
        'due': due_dt,
        'due_display': due_display,
        'due_is_sometime': due_is_sometime,
        'reference': reference,
        'recurrence': recurrence,
        'note': note,
        'done': done,
        'done_date': done_date,
        'raw_line': line
    }

def capture_token(regex, text):
    match = regex.search(text)
    if match:
        return match.group(1).strip()
    return None


def normalize_prefix(token, prefix_char):
    """Remove redundant leading prefix characters like '+' or '@'."""
    if not token:
        return None
    normalized = token.lstrip(prefix_char).strip()
    return normalized if normalized else None


def strip_leading_markers(text):
    """Drop leading project/context markers so the title stays clean."""
    cleaned = text.lstrip()
    while True:
        match = re.match(r'^([+@][^\s]+)\s+(.*)$', cleaned)
        if not match:
            break
        cleaned = match.group(2).lstrip()
    return cleaned

def normalize_note(text):
    if text is None:
        return None
    trimmed = text.strip()
    return trimmed if trimmed else None

def escape_note(text):
    return text.replace('\\', '\\\\').replace('"', '\\"').replace('\r', '\\r').replace('\n', '\\n')

def unescape_note(text):
    if text is None:
        return ""
    out = []
    i = 0
    while i < len(text):
        ch = text[i]
        if ch == '\\' and i + 1 < len(text):
            nxt = text[i + 1]
            if nxt == 'n':
                out.append('\n')
            elif nxt == 'r':
                out.append('\r')
            elif nxt == '"':
                out.append('"')
            elif nxt == '\\':
                out.append('\\')
            else:
                out.append(nxt)
            i += 2
        else:
            out.append(ch)
            i += 1
    return ''.join(out)


def current_date():
    return datetime.now().date()


def recent_window_bounds(window_days=RECENT_CONTEXT_WINDOW_DAYS):
    today = current_date()
    window_start = today - timedelta(days=max(window_days - 1, 0))
    return today, window_start


def collect_recent_context(todos, window_days=RECENT_CONTEXT_WINDOW_DAYS):
    today, window_start = recent_window_bounds(window_days)
    open_todos = [t for t in todos if not t.get('done')]
    recent_closed = [
        t for t in todos
        if t.get('done') and t.get('done_date') and t['done_date'] >= window_start
    ]

    all_relevant = open_todos + recent_closed
    topics = sorted({t['project'] for t in all_relevant if t.get('project')})
    locations = sorted({t['context'] for t in all_relevant if t.get('context')})
    recent_full_text = [t['raw_line'].strip() for t in recent_closed if t.get('raw_line')]

    return {
        'today': today,
        'window_start': window_start,
        'topics': topics,
        'locations': locations,
        'recent_todos': recent_full_text,
    }


def build_context_reminders(context, window_days=RECENT_CONTEXT_WINDOW_DAYS):
    sections = []

    recent = context.get('recent_todos') or []
    if recent:
        recent_block = "- " + "\n- ".join(recent)
        sections.append(
            f"Recent closed todos (last {window_days} days, inclusive of today):\n{recent_block}"
        )

    topics = context.get('topics') or []
    if topics:
        sections.append(
            "Existing topics in use: " + ", ".join(topics) + ". Prefer using these when appropriate, but you may also create new fitting tags."
        )

    locations = context.get('locations') or []
    if locations:
        sections.append(
            "Existing locations in use: " + ", ".join(locations) + ". Prefer using these when appropriate, but you may also create new fitting tags."
        )

    if not sections:
        return ""

    return "\n\nContext reminders:\n" + "\n\n".join(sections)

def extract_title(rest):
    cleaned_rest = strip_leading_markers(rest)
    markers = [" +", " @", " due:", " rec:", " [[", " ✅", " ^", " ~note:", "+", "@", "due:", "rec:", "[[", "✅", "^", "~note:"]
    cut = len(cleaned_rest)
    for marker in markers:
        idx = cleaned_rest.find(marker)
        if idx != -1 and idx < cut:
            cut = idx
    
    raw = cleaned_rest[:cut]
    cleaned = raw.strip()
    return cleaned if cleaned else cleaned_rest.strip()

def toggle_todo(line_index, done):
    content = read_content()
    lines = content.splitlines()
    
    if line_index < len(lines):
        lines[line_index] = rewrite_line(lines[line_index], done)
        write_content('\n'.join(lines) + '\n')

def rewrite_line(line, done):
    updated = line
    # Remove existing completion marker first
    updated = COMPLETION_RE.sub("", updated)
    
    if done:
        updated = updated.replace("- [ ]", "- [x]", 1)
        updated = updated.replace("- [X]", "- [x]", 1)
        today = datetime.now().strftime("%Y-%m-%d")
        done_marker = f" ✅ {today}"
        
        # Place Done Date before ID if ID exists
        marker_match = ID_RE.search(updated)
        if marker_match:
            start = marker_match.start()
            updated = updated[:start].rstrip() + done_marker + " " + updated[start:].lstrip()
        else:
            updated = updated.rstrip() + done_marker
    else:
        updated = updated.replace("- [x]", "- [ ]", 1)
        updated = updated.replace("- [X]", "- [ ]", 1)
    
    return updated


def next_due_date(current_due, rule):
    base_time = current_due.time() if current_due else DEFAULT_DUE_TIME
    next_date = current_due.date() if current_due else datetime.now().date()
    today = datetime.now().date()
    rule_l = rule.lower()
    
    while True:
        if rule_l == 'daily':
            next_date = next_date + timedelta(days=1)
        elif rule_l == 'weekly':
            next_date = next_date + timedelta(days=7)
        elif rule_l == 'monthly':
            year = next_date.year + (next_date.month // 12)
            month = next_date.month % 12 + 1
            day = next_date.day
            found = False
            for d in range(31, 27, -1):
                try:
                    next_date = datetime(year, month, min(day, d)).date()
                    found = True
                    break
                except ValueError:
                    continue
            if not found:
                return None
        else:
            return None
            
        if next_date > today:
            break
            
    return datetime.combine(next_date, base_time)

def find_line_by_marker(lines, marker):
    needle = f"^{marker}"
    for idx, line in enumerate(lines):
        for token in line.split():
            if token.strip() == needle:
                return idx
    return None


def add_todo(title):
    content = read_content()
    lines = content.splitlines()
    
    insert_index = len(lines)
    for i, line in enumerate(lines):
        if line.strip() == "---":
            insert_index = i
            break
    
    default_due = datetime.combine(datetime.now().date(), DEFAULT_DUE_TIME)
    default_due_str = format_due(default_due)
    marker = generate_marker()
    if DUE_RE.search(title):
        new_line = f"- [ ] {title}"
    else:
        new_line = f"- [ ] {title} due:{default_due_str}"
    new_line += f" ^{marker}"
    lines.insert(insert_index, new_line)
    
    write_content('\n'.join(lines) + '\n')
    return {
        'marker': marker,
        'line_index': insert_index,
    }


def insert_line(line):
    content = read_content()
    lines = content.splitlines()

    insert_index = len(lines)
    for i, l in enumerate(lines):
        if l.strip() == "---":
            insert_index = i
            break

    lines.insert(insert_index, line)
    write_content('\n'.join(lines) + '\n')


def update_todo_by_marker(marker, updates):
    content = read_content()
    lines = content.splitlines()

    target_index = find_line_by_marker(lines, marker)
    if target_index is None:
        return False

    existing = parse_line(lines[target_index], target_index, "")
    if not existing:
        return False

    title = (updates.get('title') or existing['title'] or '').strip()
    if not title:
        title = existing['title'] or ''

    project = updates.get('project') if 'project' in updates else existing['project']
    context = updates.get('context') if 'context' in updates else existing['context']

    if 'note' in updates:
        note_value = updates.get('note')
        note_value = note_value.strip() if isinstance(note_value, str) else None
    else:
        note_value = existing.get('note')

    if 'due' in updates and updates.get('due') is not None:
        due_dt = parse_due_input(updates.get('due')) or existing.get('due')
    elif 'due' in updates and updates.get('due') is None:
        due_dt = existing.get('due')
    else:
        due_dt = existing.get('due')

    recurrence = existing.get('recurrence')
    reference = existing.get('reference')
    done = existing.get('done', False)

    new_line = "- [x]" if done else "- [ ]"
    new_line += f" {title}"

    if project:
        new_line += f" +{str(project).strip().lstrip('+')}"
    if context:
        new_line += f" @{str(context).strip().lstrip('@')}"
    if due_dt:
        new_line += f" due:{format_due(due_dt)}"
    if recurrence:
        new_line += f" rec:{recurrence}"
    if reference:
        new_line += f" [[{reference}]]"
    if note_value:
        new_line += f" ~note:\"{escape_note(note_value)}\""

    if done:
        done_date = existing.get('done_date') or datetime.now().date()
        new_line += f" ✅ {done_date.strftime('%Y-%m-%d')}"

    new_line += f" ^{marker}"

    lines[target_index] = new_line
    write_content('\n'.join(lines) + '\n')
    return True

def sort_key_topic(todo):
    # Project (asc), Section (asc), Title (asc), Context (asc)
    # Rust: Some < None (With Project comes before Without Project)
    p = todo['project']
    c = todo['context']
    return (
        0 if p else 1, p.lower() if p else "",
        todo['section'].lower(),
        todo['title'].lower(),
        0 if c else 1, c.lower() if c else ""
    )

def sort_key_location(todo):
    # Context (asc), Section (asc), Title (asc), Project (asc)
    # Rust: Some < None (With Context comes before Without Context)
    p = todo['project']
    c = todo['context']
    return (
        0 if c else 1, c.lower() if c else "",
        todo['section'].lower(),
        todo['title'].lower(),
        0 if p else 1, p.lower() if p else ""
    )

def sort_key_date(todo):
    # Due (asc), then Project sort
    # Rust: None < Some (No Date comes before With Date)
    d = todo['due']
    key_project = sort_key_topic(todo)
    
    if d is None:
        return (0, datetime.min, key_project)
    else:
        return (1, d, key_project)

@app.route('/')
def index():
    if 'logged_in' not in session:
        return redirect(url_for('login'))
    
    lang = get_locale()
    todos = load_todos()
    
    # Load saved settings
    settings = load_settings()
    
    # Determine effective values
    # If query params are present, they override settings and update them.
    # If not, we use settings (or defaults).
    
    show_done_val = request.args.get('show_done')
    show_due_only_val = request.args.get('show_due_only')
    sort_mode_val = request.args.get('sort_mode')
    auto_ai_on_add_vals = request.args.getlist('auto_ai_on_add')
    auto_ai_on_add_val = auto_ai_on_add_vals[-1] if auto_ai_on_add_vals else None
    skip_delete_confirm_vals = request.args.getlist('skip_delete_confirm')
    skip_delete_confirm_raw = skip_delete_confirm_vals[-1] if skip_delete_confirm_vals else None
    ai_timeout_secs_val = request.args.get('ai_timeout_secs')
    
    new_settings = settings.copy()
    changed = False

    def clamp_timeout_secs(val):
        try:
            secs = int(float(val))
        except (ValueError, TypeError):
            return DEFAULT_AI_TIMEOUT_SECS
        return max(5, min(120, secs))
    
    if show_done_val is not None:
        new_settings['show_done'] = show_done_val
        changed = True
    else:
        show_done_val = settings.get('show_done', '0')
        
    if show_due_only_val is not None:
        new_settings['show_due_only'] = show_due_only_val
        changed = True
    else:
        show_due_only_val = settings.get('show_due_only', '0')
        
    if sort_mode_val is not None:
        new_settings['sort_mode'] = sort_mode_val
        changed = True
    else:
        sort_mode_val = settings.get('sort_mode', 'topic')

    if auto_ai_on_add_val is not None:
        new_settings['auto_ai_on_add'] = auto_ai_on_add_val
        changed = True
    else:
        auto_ai_on_add_val = settings.get('auto_ai_on_add', '0')

    if skip_delete_confirm_raw is not None:
        skip_delete_confirm = parse_flag(skip_delete_confirm_raw, default=True)
        new_settings['skip_delete_confirm'] = '1' if skip_delete_confirm else '0'
        changed = True
    else:
        stored_skip_delete_confirm = settings.get('skip_delete_confirm')
        skip_delete_confirm = parse_flag(stored_skip_delete_confirm, default=True)
        if isinstance(stored_skip_delete_confirm, bool):
            new_settings['skip_delete_confirm'] = '1' if stored_skip_delete_confirm else '0'
            changed = True

    if ai_timeout_secs_val is not None:
        parsed_ai_timeout = clamp_timeout_secs(ai_timeout_secs_val)
        new_settings['ai_timeout_secs'] = parsed_ai_timeout
        changed = True
    else:
        parsed_ai_timeout = clamp_timeout_secs(settings.get('ai_timeout_secs', DEFAULT_AI_TIMEOUT_SECS))
        
    if changed:
        save_settings(new_settings)
    
    # Filter logic
    show_done = show_done_val == '1'
    show_due_only = show_due_only_val == '1'
    sort_mode = sort_mode_val
    auto_ai_on_add = auto_ai_on_add_val == '1'
    ai_timeout_secs = parsed_ai_timeout
    ai_timeout_ms = ai_timeout_secs * 1000
    q = request.args.get('q', '').lower()
    
    now = datetime.now()
    filtered_todos = []
    
    for todo in todos:
        if not show_done and todo['done']:
            continue
        
        if show_due_only:
            if todo['due'] and todo['due'] > now:
                continue
        
        filtered_todos.append(todo)
    
    if q:
        # Search logic
        # 1. Current list results
        current_results = [t.copy() for t in filtered_todos if q in t['title'].lower()]
        for t in current_results: t['section'] = None
        
        # 2. All open todos (excluding those already in current_results)
        open_results = [t.copy() for t in todos if not t['done'] and q in t['title'].lower()]
        open_results = [t for t in open_results if not any(c['line_index'] == t['line_index'] and c['marker'] == t['marker'] for c in current_results)]
        for t in open_results: t['section'] = None
        
        # 3. All completed todos (excluding those already in current_results)
        done_results = [t.copy() for t in todos if t['done'] and q in t['title'].lower()]
        done_results = [t for t in done_results if not any(c['line_index'] == t['line_index'] and c['marker'] == t['marker'] for c in current_results)]
        for t in done_results: t['section'] = None
        
        if request.args.get('partial'):
            return render_template('_search_results.html', 
                           current_results=current_results,
                           open_results=open_results,
                           done_results=done_results,
                           q=q)

        return render_template('index.html', 
                       current_results=current_results,
                       open_results=open_results,
                       done_results=done_results,
                       q=q,
                       show_done=show_done,
                       show_due_only=show_due_only,
                       sort_mode=sort_mode,
                       auto_ai_on_add=auto_ai_on_add,
                       skip_delete_confirm=skip_delete_confirm,
                       ai_timeout_secs=ai_timeout_secs,
                       ai_timeout_ms=ai_timeout_ms)

    # Sorting logic
    if sort_mode == 'location':
        filtered_todos.sort(key=sort_key_location)
    elif sort_mode == 'date':
        filtered_todos.sort(key=sort_key_date)
    else: # topic
        filtered_todos.sort(key=sort_key_topic)
    
    # Grouping logic for display
    # We need to adjust the 'section' field of the todo items for display purposes
    # based on the sort mode, similar to Rust's group_label
    
    display_todos = []
    for todo in filtered_todos:
        display_item = todo.copy()
        if sort_mode == 'topic':
            display_item['section'] = todo['project'] if todo['project'] else TRANSLATIONS.get(lang, TRANSLATIONS['de']).get('no_section', 'Ohne Abschnitt')
            display_item['group_key'] = todo['project'] if todo['project'] else ''
        elif sort_mode == 'location':
            display_item['section'] = f"Ort: {todo['context'] if todo['context'] else 'Ohne Ort'}"
            display_item['group_key'] = todo['context'] if todo['context'] else ''
        elif sort_mode == 'date':
            # No grouping for date sort in Rust implementation (returns None)
            # But the template expects a section. Let's use a dummy or empty section?
            # Or maybe we should group by date?
            # Rust implementation returns None for group_label in Date mode.
            # In the template, if section changes, it prints a header.
            # If we set all sections to the same value, no headers will be printed (except the first one).
            display_item['section'] = "" 
            display_item['group_key'] = ''
        else:
            display_item['group_key'] = ''
        
        display_todos.append(display_item)

    if request.args.get('partial'):
        return render_template('_todo_list.html', todos=display_todos, show_done=show_done, show_due_only=show_due_only, sort_mode=sort_mode)

    return render_template('index.html', todos=display_todos, show_done=show_done, show_due_only=show_due_only, sort_mode=sort_mode, q=q, auto_ai_on_add=auto_ai_on_add, skip_delete_confirm=skip_delete_confirm, ai_timeout_secs=ai_timeout_secs, ai_timeout_ms=ai_timeout_ms)

@app.route('/login/oidc')
def login_oidc():
    redirect_uri = OIDC_REDIRECT_URI or url_for('authorize', _external=True)
    return oauth.oidc.authorize_redirect(redirect_uri)

@app.route('/authorize')
def authorize():
    token = oauth.oidc.authorize_access_token()
    user_info = token.get('userinfo')
    if not user_info:
        user_info = oauth.oidc.userinfo(token=token)
    
    email = user_info.get('email', '').lower()
    allowed_user = os.environ.get('OIDC_ALLOWED_USER', '').lower()

    if allowed_user and email == allowed_user:
        session.permanent = True
        session['logged_in'] = True
        return redirect(url_for('index'))
    else:
        lang = get_locale()
        flash(TRANSLATIONS.get(lang, TRANSLATIONS['de'])['unauthorized_user'])
        return redirect(url_for('login'))

@app.route('/login', methods=['GET', 'POST'])
def login():
    show_password_login = bool(os.environ.get('APP_USER') and os.environ.get('APP_PASSWORD'))
    show_oidc_login = bool(os.environ.get('OIDC_ISSUER') and os.environ.get('OIDC_CLIENT_ID'))

    if request.method == 'POST' and show_password_login:
        username = request.form.get('username')
        password = request.form.get('password')
        if username == os.environ.get('APP_USER') and password == os.environ.get('APP_PASSWORD'):
            session.permanent = True
            session['logged_in'] = True
            return redirect(url_for('index'))
        else:
            lang = get_locale()
            flash(TRANSLATIONS.get(lang, TRANSLATIONS['de'])['invalid_credentials'])
    return render_template('login.html', show_password_login=show_password_login, show_oidc_login=show_oidc_login)

@app.route('/toggle/<int:line_index>', methods=['POST'])
def toggle(line_index):
    if 'logged_in' not in session:
        return redirect(url_for('login'))
    
    content = read_content()
    lines = content.splitlines()
    
    if line_index < len(lines):
        line = lines[line_index]
        is_done = "- [x]" in line or "- [X]" in line
        item = parse_line(line, line_index, "")
        
        now = datetime.now()
        if not is_done and item and item.get('recurrence') and item.get('due') and item['due'] < now:
            base_time = item['due'].time() if item.get('due') else DEFAULT_DUE_TIME
            due_dt = datetime.combine(now.date(), base_time)
            due_str = format_due(due_dt)
            new_line = re.sub(r'due:\d{4}-\d{2}-\d{2}(?:T\d{2}:\d{2})?', f'due:{due_str}', line)
            lines[line_index] = new_line
            write_content('\n'.join(lines) + '\n')
            toggle_todo(line_index, True)
        else:
            toggle_todo(line_index, not is_done)

        if item and item.get('recurrence') and not is_done:
            next_due = next_due_date(item.get('due'), item['recurrence'])
            if next_due:
                new_line = "- [ ] " + item['title'].strip()
                if item.get('project'):
                    project_clean = normalize_prefix(item['project'], '+')
                    if project_clean:
                        new_line += f" +{project_clean}"
                if item.get('context'):
                    context_clean = normalize_prefix(item['context'], '@')
                    if context_clean:
                        new_line += f" @{context_clean}"
                new_line += f" due:{format_due(next_due)}"
                new_line += f" rec:{item['recurrence']}"
                if item.get('reference'):
                    new_line += f" [[{item['reference'].strip()}]]"

                if item.get('note'):
                    new_line += f' ~note:"{escape_note(item["note"])}"'
                new_line += f" ^{ensure_marker(item.get('marker'))}"
                insert_line(new_line)
    
    return redirect(url_for('index'))

@app.route('/postpone/<int:line_index>/<string:target>', methods=['POST'])
def postpone(line_index, target):
    if 'logged_in' not in session:
        return redirect(url_for('login'))
    
    content = read_content()
    lines = content.splitlines()
    
    if line_index >= len(lines):
        return redirect(url_for('index'))
        
    line = lines[line_index]
    item = parse_line(line, line_index, "")
    if not item:
        return redirect(url_for('index'))
        
    # Calculate new date/time
    now = datetime.now()
    today = now.date()
    current_hour = now.hour
    
    if target == 'today':
        # Smart "Today" logic: at least 4 hours from now
        new_date = today
        if current_hour < 8:
            new_time = dtime(hour=12, minute=0)
        elif current_hour < 14:
            new_time = dtime(hour=18, minute=0)
        else:
            new_time = dtime(hour=18, minute=0)
    elif target == 'tomorrow':
        new_date = today + timedelta(days=1)
        new_time = dtime(hour=12, minute=0)
    elif target == 'weekend':
        # Next Saturday
        day_of_week = today.weekday()  # 0=Monday, 5=Saturday, 6=Sunday
        if day_of_week == 5:  # Saturday
            days_until_saturday = 7
        elif day_of_week == 6:  # Sunday
            days_until_saturday = 6
        else:
            days_until_saturday = 5 - day_of_week
        new_date = today + timedelta(days=days_until_saturday)
        new_time = dtime(hour=12, minute=0)
    elif target == 'sometime':
        new_date = datetime(9999, 12, 31).date()
        new_time = dtime(hour=0, minute=0)
    else:
        # Fallback: keep original date/time or set to today
        new_date = today
        new_time = item['due'].time() if item.get('due') else DEFAULT_DUE_TIME
    
    # Update line
    # We need to replace or add due:YYYY-MM-DD
    # Let's reuse the logic from edit but simpler
    
    # Reconstruct line
    original_line = lines[line_index]
    marker = ensure_marker(capture_token(ID_RE, original_line))
    
    new_line = "- [x] " if item['done'] else "- [ ] "
    new_line += item['title'].strip()
    
    if item['project'] and item['project'].strip():
        project_clean = normalize_prefix(item['project'], '+')
        if project_clean:
            new_line += f" +{project_clean}"
        
    if item['context'] and item['context'].strip():
        context_clean = normalize_prefix(item['context'], '@')
        if context_clean:
            new_line += f" @{context_clean}"
        
    # Always set the new due date/time
    new_line += f" due:{format_due(datetime.combine(new_date, new_time))}"
        
    if item['reference'] and item['reference'].strip():
        new_line += f" [[{item['reference'].strip()}]]"

    if item.get('note'):
        new_line += f' ~note:"{escape_note(item["note"])}"'
        
    # Preserve completion date if done
    if item['done']:
         match = COMPLETION_RE.search(original_line)
         if match:
             new_line += match.group(0)

    new_line += f" ^{marker}"
        
    lines[line_index] = new_line
    write_content('\n'.join(lines) + '\n')
    
    return redirect(url_for('index'))

@app.route('/logout')
def logout():
    session.pop('logged_in', None)
    return redirect(url_for('login'))

@app.route('/set_language/<lang>')
def set_language(lang):
    if lang in TRANSLATIONS:
        session['lang'] = lang
    return redirect(request.referrer or url_for('index'))

@app.route('/edit/<int:line_index>', methods=['GET', 'POST'])
def edit(line_index):
    if 'logged_in' not in session:
        return redirect(url_for('login'))
    
    content = read_content()
    lines = content.splitlines()
    
    if line_index >= len(lines):
        return redirect(url_for('index'))
        
    if request.method == 'POST':
        title = request.form.get('title')
        comment = request.form.get('comment')
        project = request.form.get('project')
        context = request.form.get('context')
        due_str = request.form.get('due')
        reference = request.form.get('reference')
        recurrence = request.form.get('recurrence')
        note_raw = request.form.get('note')
        done = request.form.get('done') == 'on'

        due_dt = parse_due_input(due_str)

        note_value = normalize_note(note_raw)

        if comment and comment.strip():
            title = f"{title.strip()} ({comment.strip()})"
        
        # Reconstruct line
        original_line = lines[line_index]
        marker = ensure_marker(capture_token(ID_RE, original_line))
        original_item = parse_line(original_line, line_index, "")
        was_done = original_item.get('done') if original_item else False
        
        # Handle completion date
        completion_str = ""
        if done:
            match = COMPLETION_RE.search(original_line)
            # If it was already done, preserve the date
            if match and ("- [x]" in original_line or "- [X]" in original_line):
                 completion_str = match.group(0)
            else:
                 # Otherwise add today's date
                 today = datetime.now().strftime("%Y-%m-%d")
                 completion_str = f" ✅ {today}"

        new_line = "- [x] " if done else "- [ ] "
        new_line += title.strip()
        
        if project and project.strip():
            clean_project = project.strip().lstrip('+')
            if clean_project:
                new_line += f" +{clean_project}"
            
        if context and context.strip():
            clean_context = context.strip().lstrip('@')
            if clean_context:
                new_line += f" @{clean_context}"
            
        if due_dt:
            new_line += f" due:{format_due(due_dt)}"
            
        if recurrence and recurrence.strip():
            rec_clean = recurrence.strip()
            new_line += f" rec:{rec_clean}"

        if reference and reference.strip():
            new_line += f" [[{reference.strip()}]]"

        if note_value:
            new_line += f' ~note:"{escape_note(note_value)}"'
            
        if completion_str:
            new_line += completion_str

        new_line += f" ^{marker}"
            
        lines[line_index] = new_line
        write_content('\n'.join(lines) + '\n')

        if recurrence and recurrence.strip() and not was_done and done:
            base_due = due_dt
            next_due = next_due_date(base_due, recurrence.strip())
            if next_due:
                clone_title = title.strip()
                new_rec_line = "- [ ] " + clone_title
                if project and project.strip():
                    new_rec_line += f" +{project.strip().lstrip('+')}"
                if context and context.strip():
                    new_rec_line += f" @{context.strip().lstrip('@')}"
                new_rec_line += f" due:{format_due(next_due)}"
                new_rec_line += f" rec:{recurrence.strip()}"
                if reference and reference.strip():
                    new_rec_line += f" [[{reference.strip()}]]"

                if note_value:
                    new_rec_line += f' ~note:"{escape_note(note_value)}"'
                new_rec_line += f" ^{generate_marker()}"
                insert_line(new_rec_line)
        
        return redirect(url_for('index'))
    
    # GET request
    line = lines[line_index]
    # We need to parse it to pre-fill the form
    # We can reuse parse_line but we need a dummy section
    item = parse_line(line, line_index, "")
    if not item:
        return redirect(url_for('index'))

    item['due_input'] = item['due_display'] if item.get('due_display') else ''
        
    return render_template('edit.html', todo=item)

@app.route('/delete/<int:line_index>', methods=['POST'])
def delete(line_index):
    if 'logged_in' not in session:
        return redirect(url_for('login'))
    
    content = read_content()
    lines = content.splitlines()
    
    if line_index < len(lines):
        lines.pop(line_index)
        write_content('\n'.join(lines) + '\n')
    
    return redirect(url_for('index'))

@app.route('/api/todo/<int:line_index>')
def get_todo_json(line_index):
    if 'logged_in' not in session:
        return {'error': 'Unauthorized'}, 401
    
    content = read_content()
    lines = content.splitlines()
    
    if line_index >= len(lines):
        return {'error': 'Not found'}, 404
        
    line = lines[line_index]
    item = parse_line(line, line_index, "")
    if not item:
        return {'error': 'Invalid item'}, 400
        
    # Convert date to string for JSON
    if item['due']:
        item['due'] = format_due(item['due'])
    if item.get('done_date'):
        item['done_date'] = item['done_date'].strftime("%Y-%m-%d")
        
    return item

@app.route('/api/parse', methods=['POST'])
def parse_nlp():
    if 'logged_in' not in session:
        return {'error': 'Unauthorized'}, 401
    
    data = request.get_json()
    text = data.get('text')
    if not text:
        logger.warning("NLP Parse: No text provided")
        return {'error': 'No text provided'}, 400

    logger.info(f"NLP Parse Request: {text}")

    now = datetime.now()
    today = now.strftime("%Y-%m-%d")
    weekday = now.strftime("%A")

    todos = load_todos()
    recent_context = collect_recent_context(todos, window_days=RECENT_CONTEXT_WINDOW_DAYS)
    
    # Load config
    app_config = {}
    config_file = os.path.join(os.path.dirname(__file__), 'config.json')
    if os.path.exists(config_file):
        try:
            with open(config_file, 'r') as f:
                app_config = json.load(f)
        except Exception as e:
            logger.error(f"Error loading config.json: {e}")

    # Construct Chat API URL
    base_url = os.environ.get('OLLAMA_URL', app_config.get('llm_url', "http://10.0.2.71:11434/api/generate"))
    if "/api/generate" in base_url:
        chat_url = base_url.replace("/api/generate", "/api/chat")
    else:
        chat_url = base_url.rstrip('/') + "/api/chat"

    # User mentioned qwen3:14b
    MODEL = os.environ.get('OLLAMA_MODEL', app_config.get('llm_model', 'qwen3:14b'))
    
    system_prompt = (
        "You are a specialized parser for todo items and notes. "
        f"Today is {weekday}, {today}. "
        "Your job is to determine if input is an ACTIONABLE todo/note or just casual conversation.\n\n"
        "FIRST: Decide if the input is a valid todo/note. Valid inputs include:\n"
        "- Direct tasks: 'Kaufe Milch', 'Ruf Mama an'\n"
        "- Statements of intent: 'ich muss noch einkaufen', 'ich sollte den Arzt anrufen'\n"
        "- Plans with actions: 'morgen Zahnarzt um 10 Uhr'\n"
        "- Notes to remember: 'Marias Geburtstag ist am 15. März'\n\n"
        "INVALID inputs (set 'rejected' to true):\n"
        "- Speculative thoughts without commitment: 'ich glaube ich gehe vielleicht...', 'könnte sein dass...'\n"
        "- Pure observations: 'Das Wetter ist schön', 'Es regnet'\n"
        "- Questions: 'Was soll ich heute machen?'\n"
        "- Casual chat: 'Ich finde Schwimmen toll'\n\n"
        "OUTPUT FORMAT - Return a JSON object with these fields:\n"
        "'rejected': boolean - true if input is NOT a valid todo/note, false if it IS valid\n"
        "'confidence': number 0.0-1.0 - how confident you are in your parsing\n"
        "'title': string|null - concise action-oriented title (convert statements to imperative if possible, e.g. 'ich muss einkaufen' → 'Einkaufen gehen')\n"
        "'note': string|null - extra details (who, how, timing, background)\n"
        "'due': string|null - datetime in YYYY-MM-DDTHH:MM 24h format; if only date given, use 00:00\n"
        "'context': string|null - location/situation without @ (e.g. 'office', 'home', 'phone', 'errands')\n"
        "'project': string|null - topic/category without + (e.g. 'work', 'health', 'finance', 'family')\n\n"
        "RULES:\n"
        "1. Return ONLY a valid JSON object, NO other text.\n"
        "2. ALL keys must be present in the response.\n"
        "3. If rejected=true, set title/note/due/context/project to null.\n"
        "4. If rejected=false, ALWAYS provide context and project (make reasonable guesses).\n"
        "5. The 'title' and 'note' MUST be in the same language as the input. Do NOT translate.\n"
        "6. Calculate relative dates from today.\n\n"
        "EXAMPLES:\n"
        "Input: 'Kaufe morgen Milch'\n"
        "Output: {\\\"rejected\\\": false, \\\"confidence\\\": 0.95, \\\"title\\\": \\\"Kaufe Milch\\\", \\\"note\\\": null, \\\"due\\\": \\\"2026-01-18T00:00\\\", \\\"context\\\": \\\"errands\\\", \\\"project\\\": \\\"household\\\"}\n\n"
        "Input: 'ich muss noch den Zahnarzt anrufen wegen dem Termin nächste Woche'\n"
        "Output: {\\\"rejected\\\": false, \\\"confidence\\\": 0.9, \\\"title\\\": \\\"Zahnarzt anrufen\\\", \\\"note\\\": \\\"Wegen Termin nächste Woche\\\", \\\"due\\\": null, \\\"context\\\": \\\"phone\\\", \\\"project\\\": \\\"health\\\"}\n\n"
        "Input: 'ich glaube ich gehe heute mit den Kindern schwimmen. Wir könnten nach Büttgen gehen'\n"
        "Output: {\\\"rejected\\\": true, \\\"confidence\\\": 0.85, \\\"title\\\": null, \\\"note\\\": null, \\\"due\\\": null, \\\"context\\\": null, \\\"project\\\": null}\n\n"
        "Input: 'Das Wetter ist heute wirklich schön'\n"
        "Output: {\\\"rejected\\\": true, \\\"confidence\\\": 0.95, \\\"title\\\": null, \\\"note\\\": null, \\\"due\\\": null, \\\"context\\\": null, \\\"project\\\": null}"
    )

    reminder_block = build_context_reminders(recent_context, window_days=RECENT_CONTEXT_WINDOW_DAYS)
    if reminder_block:
        system_prompt += "\n\n" + reminder_block

    try:
        logger.info(f"Sending request to Ollama ({chat_url}) with model {MODEL}")
        response = requests.post(chat_url, json={
            "model": MODEL,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": text}
            ],
            "stream": False,
            "format": "json"
        }, timeout=45)
        
        logger.info(f"Ollama response status: {response.status_code}")
        if response.status_code != 200:
            logger.error(f"Ollama Error Body: {response.text}")
        
        response.raise_for_status()
        result = response.json()
        raw_response = result['message']['content'].strip()
        logger.info(f"Ollama raw response: {raw_response}")
        
        # Strip markdown code blocks if present
        if raw_response.startswith("```"):
            lines = raw_response.splitlines()
            if lines[0].startswith("```"):
                lines = lines[1:]
            if lines and lines[-1].startswith("```"):
                lines = lines[:-1]
            raw_response = "\n".join(lines).strip()
            
        parsed = json.loads(raw_response)
        
        logger.info(f"Parsed JSON: {parsed}")
        
        # Check if input was rejected as non-actionable
        if parsed.get('rejected', False):
            logger.info("Input rejected as non-actionable (not a todo/note)")
            return None  # Silently ignore non-todo input
        
        # Check confidence threshold (80%)
        confidence = parsed.get('confidence', 1.0)
        if confidence < 0.8:
            logger.info(f"Input rejected due to low confidence: {confidence}")
            return None  # Silently ignore low-confidence parsing
        
        # Validate/fix keys
        if 'title' not in parsed or not parsed['title']:
            parsed['title'] = text
        if 'note' not in parsed:
            parsed['note'] = None
        if 'due' not in parsed:
            parsed['due'] = None
        if 'context' not in parsed:
            parsed['context'] = None
        if 'project' not in parsed:
            parsed['project'] = None

        if parsed.get('note') == "":
            parsed['note'] = None
        
        # Remove internal fields from response
        parsed.pop('rejected', None)
        parsed.pop('confidence', None)
            
        return parsed
    except Exception as e:
        logger.error(f"Ollama Error: {e}", exc_info=True)
        return {'error': str(e)}, 500


@app.route('/api/add', methods=['POST'])
@csrf.exempt
def api_add():
    if 'logged_in' not in session:
        return {'error': 'Unauthorized'}, 401

    payload = request.get_json(silent=True) or {}
    title = payload.get('title')
    if not title or not str(title).strip():
        return {'error': 'Title required'}, 400

    result = add_todo(title)
    return {'ok': True, 'marker': result['marker'], 'line_index': result['line_index']}


@app.route('/api/improve', methods=['POST'])
@csrf.exempt
def api_improve():
    if 'logged_in' not in session:
        return {'error': 'Unauthorized'}, 401

    data = request.get_json(silent=True) or {}
    marker = data.get('marker')
    if not marker:
        return {'error': 'Marker required'}, 400

    updated = {
        'title': data.get('title'),
        'note': data.get('note'),
        'due': data.get('due'),
        'context': data.get('context'),
        'project': data.get('project'),
    }

    if not update_todo_by_marker(marker, updated):
        return {'error': 'Todo not found'}, 404

    return {'ok': True}

@app.route('/add', methods=['POST'])
def add():
    if 'logged_in' not in session:
        return redirect(url_for('login'))
    
    title = request.form.get('title')
    if title:
        add_todo(title)
    
    # Preserve redirect for legacy form posts
    return redirect(url_for('index'))

if __name__ == '__main__':
    app.run(host='0.0.0.0', port=5000, threaded=True)
