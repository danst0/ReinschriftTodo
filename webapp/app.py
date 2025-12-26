import os
import re
import json
from datetime import datetime, timedelta
from flask import Flask, render_template, request, redirect, url_for, session, flash
import requests
from requests.auth import HTTPBasicAuth
from translations import TRANSLATIONS

app = Flask(__name__)
app.secret_key = os.environ.get('SECRET_KEY', 'dev_secret_key')
app.permanent_session_lifetime = timedelta(days=30)

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

# WebDAV Configuration
USE_WEBDAV = os.environ.get('USE_WEBDAV', 'false').lower() == 'true'
WEBDAV_URL = os.environ.get('WEBDAV_URL')
WEBDAV_USERNAME = os.environ.get('WEBDAV_USERNAME')
WEBDAV_PASSWORD = os.environ.get('WEBDAV_PASSWORD')

LINK_RE = re.compile(r"\[\[([^\]]+)\]\]")
PROJECT_RE = re.compile(r"\+([^\s]+)")
CONTEXT_RE = re.compile(r"@([^\s]+)")
DUE_RE = re.compile(r"due:(\d{4}-\d{2}-\d{2})")
ID_RE = re.compile(r"\^([A-Za-z0-9]+)")
COMPLETION_RE = re.compile(r"\s✅\s\d{4}-\d{2}-\d{2}")

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
    project = capture_token(PROJECT_RE, rest)
    context = capture_token(CONTEXT_RE, rest)
    due_str = capture_token(DUE_RE, rest)
    due = None
    if due_str:
        try:
            due = datetime.strptime(due_str, "%Y-%m-%d").date()
        except ValueError:
            pass
    
    reference = capture_token(LINK_RE, rest)
    marker = capture_token(ID_RE, rest)
    
    return {
        'line_index': line_index,
        'marker': marker,
        'title': title,
        'section': section,
        'project': project,
        'context': context,
        'due': due,
        'reference': reference,
        'done': done,
        'raw_line': line
    }

def capture_token(regex, text):
    match = regex.search(text)
    if match:
        return match.group(1).strip()
    return None

def extract_title(rest):
    markers = [" +", " @", " due:", " [[", " ✅", " ^", "+", "@", "due:", "[[", "✅", "^"]
    cut = len(rest)
    for marker in markers:
        idx = rest.find(marker)
        if idx != -1 and idx < cut:
            cut = idx
    
    raw = rest[:cut]
    cleaned = raw.strip()
    return cleaned if cleaned else rest.strip()

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

def add_todo(title):
    content = read_content()
    lines = content.splitlines()
    
    insert_index = len(lines)
    for i, line in enumerate(lines):
        if line.strip() == "---":
            insert_index = i
            break
    
    today = datetime.now().strftime("%Y-%m-%d")
    new_line = f"- [ ] {title} due:{today}"
    lines.insert(insert_index, new_line)
    
    write_content('\n'.join(lines) + '\n')

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
        return (0, datetime.min.date(), key_project)
    else:
        return (1, d, key_project)

@app.route('/')
def index():
    if 'logged_in' not in session:
        return redirect(url_for('login'))
    
    todos = load_todos()
    
    # Load saved settings
    settings = load_settings()
    
    # Determine effective values
    # If query params are present, they override settings and update them.
    # If not, we use settings (or defaults).
    
    show_done_val = request.args.get('show_done')
    show_due_only_val = request.args.get('show_due_only')
    sort_mode_val = request.args.get('sort_mode')
    
    new_settings = settings.copy()
    changed = False
    
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
        
    if changed:
        save_settings(new_settings)
    
    # Filter logic
    show_done = show_done_val == '1'
    show_due_only = show_due_only_val == '1'
    sort_mode = sort_mode_val
    q = request.args.get('q', '').lower()
    
    today = datetime.now().date()
    filtered_todos = []
    
    for todo in todos:
        if not show_done and todo['done']:
            continue
        
        if show_due_only:
            if todo['due'] and todo['due'] > today:
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
        
        return render_template('index.html', 
                               current_results=current_results,
                               open_results=open_results,
                               done_results=done_results,
                               q=q,
                               show_done=show_done,
                               show_due_only=show_due_only,
                               sort_mode=sort_mode)

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
            display_item['section'] = f"Thema: {todo['project'] if todo['project'] else 'Ohne Projekt'}"
        elif sort_mode == 'location':
            display_item['section'] = f"Ort: {todo['context'] if todo['context'] else 'Ohne Ort'}"
        elif sort_mode == 'date':
            # No grouping for date sort in Rust implementation (returns None)
            # But the template expects a section. Let's use a dummy or empty section?
            # Or maybe we should group by date?
            # Rust implementation returns None for group_label in Date mode.
            # In the template, if section changes, it prints a header.
            # If we set all sections to the same value, no headers will be printed (except the first one).
            display_item['section'] = "" 
        
        display_todos.append(display_item)

    if request.args.get('partial'):
        return render_template('_todo_list.html', todos=display_todos, show_done=show_done, show_due_only=show_due_only, sort_mode=sort_mode)

    return render_template('index.html', todos=display_todos, show_done=show_done, show_due_only=show_due_only, sort_mode=sort_mode, q=q)

@app.route('/login', methods=['GET', 'POST'])
def login():
    if request.method == 'POST':
        username = request.form.get('username')
        password = request.form.get('password')
        if username == os.environ.get('APP_USER') and password == os.environ.get('APP_PASSWORD'):
            session.permanent = True
            session['logged_in'] = True
            return redirect(url_for('index'))
        else:
            lang = get_locale()
            flash(TRANSLATIONS.get(lang, TRANSLATIONS['de'])['invalid_credentials'])
    return render_template('login.html')

@app.route('/toggle/<int:line_index>')
def toggle(line_index):
    if 'logged_in' not in session:
        return redirect(url_for('login'))
    
    # We need to know if it's currently done or not.
    # Ideally we pass the state, or we read it.
    # For simplicity, let's read and flip.
    # But wait, toggle_todo takes 'done' target state.
    
    # Let's just read the file again to check current state
    content = read_content()
    lines = content.splitlines()
    
    if line_index < len(lines):
        line = lines[line_index]
        is_done = "- [x]" in line or "- [X]" in line
        toggle_todo(line_index, not is_done)
    
    return redirect(url_for('index'))

@app.route('/postpone/<int:line_index>/<string:target>')
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
        
    # Calculate new date
    today = datetime.now().date()
    new_date = today
    if target == 'tomorrow':
        new_date = today + timedelta(days=1)
    elif target == 'sometimes':
        new_date = datetime(9999, 12, 31).date()
    
    # Update line
    # We need to replace or add due:YYYY-MM-DD
    # Let's reuse the logic from edit but simpler
    
    # Reconstruct line
    original_line = lines[line_index]
    marker = capture_token(ID_RE, original_line)
    
    new_line = "- [x] " if item['done'] else "- [ ] "
    new_line += item['title'].strip()
    
    if item['project'] and item['project'].strip():
        new_line += f" +{item['project'].strip()}"
        
    if item['context'] and item['context'].strip():
        new_line += f" @{item['context'].strip()}"
        
    # Always set the new due date
    new_line += f" due:{new_date.strftime('%Y-%m-%d')}"
        
    if item['reference'] and item['reference'].strip():
        new_line += f" [[{item['reference'].strip()}]]"
        
    # Preserve completion date if done
    if item['done']:
         match = COMPLETION_RE.search(original_line)
         if match:
             new_line += match.group(0)

    if marker:
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
        done = request.form.get('done') == 'on'

        if comment and comment.strip():
            title = f"{title.strip()} ({comment.strip()})"
        
        # Reconstruct line
        original_line = lines[line_index]
        marker = capture_token(ID_RE, original_line)
        
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
            
        if due_str and due_str.strip():
            new_line += f" due:{due_str.strip()}"
            
        if reference and reference.strip():
            new_line += f" [[{reference.strip()}]]"
            
        if completion_str:
            new_line += completion_str

        if marker:
            new_line += f" ^{marker}"
            
        lines[line_index] = new_line
        write_content('\n'.join(lines) + '\n')
        
        return redirect(url_for('index'))
    
    # GET request
    line = lines[line_index]
    # We need to parse it to pre-fill the form
    # We can reuse parse_line but we need a dummy section
    item = parse_line(line, line_index, "")
    if not item:
        return redirect(url_for('index'))
        
    return render_template('edit.html', todo=item)

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
        item['due'] = item['due'].strftime("%Y-%m-%d")
        
    return item

@app.route('/add', methods=['POST'])
def add():
    if 'logged_in' not in session:
        return redirect(url_for('login'))
    
    title = request.form.get('title')
    if title:
        add_todo(title)
    
    return redirect(url_for('index'))

if __name__ == '__main__':
    app.run(host='0.0.0.0', port=5000)
