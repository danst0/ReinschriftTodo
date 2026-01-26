"""Main routes blueprint - index, language selection."""

from datetime import datetime
from flask import Blueprint, render_template, redirect, url_for, session, request, current_app

from translations import TRANSLATIONS
from app.services import (
    load_todos,
    load_settings,
    save_settings,
    sort_todos,
)
from app.utils.helpers import parse_flag

main_bp = Blueprint('main', __name__)

DEFAULT_AI_TIMEOUT_SECS = 30


def get_locale() -> str:
    """Get current locale from session or request."""
    if 'lang' in session:
        return session['lang']
    accept_languages = request.accept_languages.best_match(TRANSLATIONS.keys())
    return accept_languages or 'de'


def require_login(f):
    """Decorator to require login for routes."""
    from functools import wraps

    @wraps(f)
    def decorated(*args, **kwargs):
        if 'logged_in' not in session:
            return redirect(url_for('auth.login'))
        return f(*args, **kwargs)
    return decorated


def clamp_timeout_secs(val) -> int:
    """Clamp AI timeout to valid range."""
    try:
        secs = int(float(val))
    except (ValueError, TypeError):
        return DEFAULT_AI_TIMEOUT_SECS
    return max(5, min(120, secs))


@main_bp.route('/')
@require_login
def index():
    """Main todo list view."""
    lang = get_locale()
    todos = load_todos()

    # Load saved settings
    settings = load_settings()

    # Handle query parameters and settings
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
        if not show_done and todo.done:
            continue

        if show_due_only:
            if todo.due and todo.due > now:
                continue

        filtered_todos.append(todo)

    # Convert TodoItem objects to dicts for template compatibility
    todos_as_dicts = [_todo_to_dict(t) for t in filtered_todos]

    if q:
        # Search logic
        all_todos_dicts = [_todo_to_dict(t) for t in todos]

        # 1. Current list results
        current_results = [t.copy() for t in todos_as_dicts if q in t['title'].lower()]
        for t in current_results:
            t['section'] = None

        # 2. All open todos (excluding those already in current_results)
        open_results = [t.copy() for t in all_todos_dicts if not t['done'] and q in t['title'].lower()]
        open_results = [t for t in open_results if not any(
            c['line_index'] == t['line_index'] and c['marker'] == t['marker']
            for c in current_results
        )]
        for t in open_results:
            t['section'] = None

        # 3. All completed todos (excluding those already in current_results)
        done_results = [t.copy() for t in all_todos_dicts if t['done'] and q in t['title'].lower()]
        done_results = [t for t in done_results if not any(
            c['line_index'] == t['line_index'] and c['marker'] == t['marker']
            for c in current_results
        )]
        for t in done_results:
            t['section'] = None

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
                              ai_timeout_ms=ai_timeout_ms,
                              ai_debug_enabled=current_app.config.get('AI_DEBUG_ENABLED', False))

    # Sorting
    sorted_todos = sort_todos(todos_as_dicts, sort_mode)

    # Grouping logic for display
    t = TRANSLATIONS.get(lang, TRANSLATIONS['de'])
    display_todos = []
    for todo in sorted_todos:
        display_item = todo.copy()
        first_project = todo['projects'][0] if todo['projects'] else None
        first_context = todo['contexts'][0] if todo['contexts'] else None

        if sort_mode == 'topic':
            display_item['section'] = first_project if first_project else t.get('no_project', 'No Project')
            display_item['group_key'] = first_project if first_project else ''
        elif sort_mode == 'location':
            location_label = t.get('location', 'Location')
            no_location_label = t.get('no_location', 'No Location')
            display_item['section'] = f"{location_label}: {first_context if first_context else no_location_label}"
            display_item['group_key'] = first_context if first_context else ''
        elif sort_mode == 'date':
            display_item['section'] = ""
            display_item['group_key'] = ''
        else:
            display_item['group_key'] = ''

        display_todos.append(display_item)

    if request.args.get('partial'):
        return render_template('_todo_list.html',
                              todos=display_todos,
                              show_done=show_done,
                              show_due_only=show_due_only,
                              sort_mode=sort_mode)

    return render_template('index.html',
                          todos=display_todos,
                          show_done=show_done,
                          show_due_only=show_due_only,
                          sort_mode=sort_mode,
                          q=q,
                          auto_ai_on_add=auto_ai_on_add,
                          skip_delete_confirm=skip_delete_confirm,
                          ai_timeout_secs=ai_timeout_secs,
                          ai_timeout_ms=ai_timeout_ms,
                          ai_debug_enabled=current_app.config.get('AI_DEBUG_ENABLED', False))


@main_bp.route('/set_language/<lang>')
def set_language(lang):
    """Set the user's preferred language."""
    if lang in TRANSLATIONS:
        session['lang'] = lang
    return redirect(request.referrer or url_for('main.index'))


def _todo_to_dict(todo) -> dict:
    """Convert a TodoItem to a dict for template compatibility."""
    if hasattr(todo, 'to_dict'):
        return todo.to_dict()

    # Already a dict
    return dict(todo)
