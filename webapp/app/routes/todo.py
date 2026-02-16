"""Todo routes blueprint - toggle, postpone, edit, delete, add."""

import re
from datetime import datetime
from flask import Blueprint, render_template, request, redirect, url_for, session, jsonify

from translations import TRANSLATIONS
from app.exceptions import ConflictError
from app.models.todo import ID_RE, COMPLETION_RE
from app.services import (
    read_content,
    write_content,
    parse_line,
    parse_due_input,
    handle_toggle_with_recurrence,
    postpone_todo,
    add_todo,
    delete_todo,
    next_due_date,
    insert_line,
)
from app.services.storage import read_content_with_fingerprint, write_content_checked
from app.services.undo_service import push_undo, pop_undo
from app.utils.markers import ensure_marker, generate_marker
from app.utils.escaping import escape_note, normalize_note
from app.utils.helpers import format_due, normalize_prefix

todo_bp = Blueprint('todo', __name__)


def require_login(f):
    """Decorator to require login for routes."""
    from functools import wraps

    @wraps(f)
    def decorated(*args, **kwargs):
        if 'logged_in' not in session:
            if request.headers.get('X-Requested-With') == 'XMLHttpRequest':
                return jsonify({'error': 'Not logged in'}), 401
            return redirect(url_for('auth.login'))
        return f(*args, **kwargs)
    return decorated


@todo_bp.route('/toggle/<int:line_index>', methods=['POST'])
@require_login
def toggle(line_index):
    """Toggle a todo's completion status."""
    content = read_content()
    push_undo(content, 'toggle')
    handle_toggle_with_recurrence(line_index)
    return redirect(url_for('main.index'))


@todo_bp.route('/postpone/<int:line_index>/<string:target>', methods=['POST'])
@require_login
def postpone(line_index, target):
    """Postpone a todo to a new date."""
    content = read_content()
    push_undo(content, 'postpone')
    postpone_todo(line_index, target)
    return redirect(url_for('main.index'))


@todo_bp.route('/edit/<int:line_index>', methods=['GET', 'POST'])
@require_login
def edit(line_index):
    """Edit a todo."""
    content = read_content()
    lines = content.splitlines()

    if line_index >= len(lines):
        if request.headers.get('X-Requested-With') == 'XMLHttpRequest':
            return jsonify({'error': 'Invalid line index'}), 400
        return redirect(url_for('main.index'))

    if request.method == 'POST':
        return _handle_edit_post(lines, line_index)

    # GET request
    line = lines[line_index]
    item = parse_line(line, line_index)
    if not item:
        return redirect(url_for('main.index'))

    # Convert to dict for template
    item_dict = item.to_dict() if hasattr(item, 'to_dict') else dict(item)
    item_dict['due_input'] = item.due_display if item.due_display else ''

    return render_template('edit.html', todo=item_dict)


def _handle_edit_post(lines, line_index):
    """Handle POST request for edit."""
    title = request.form.get('title')
    comment = request.form.get('comment')
    projects_raw = request.form.get('projects', '')
    contexts_raw = request.form.get('contexts', '')
    due_str = request.form.get('due')
    reference = request.form.get('reference')
    recurrence = request.form.get('recurrence')
    note_raw = request.form.get('note')
    done = request.form.get('done') == 'on'

    # Parse space-separated projects and contexts
    projects = [p.strip().lstrip('+') for p in projects_raw.split() if p.strip().lstrip('+')]
    contexts = [c.strip().lstrip('@') for c in contexts_raw.split() if c.strip().lstrip('@')]

    due_dt = parse_due_input(due_str)
    note_value = normalize_note(note_raw)

    if comment and comment.strip():
        title = f"{title.strip()} ({comment.strip()})"

    # Reconstruct line
    original_line = lines[line_index]
    from app.services.parser import capture_token
    marker = ensure_marker(capture_token(ID_RE, original_line))
    original_item = parse_line(original_line, line_index)
    was_done = original_item.done if original_item else False

    # Handle completion date
    completion_str = ""
    if done:
        match = COMPLETION_RE.search(original_line)
        if match and ("- [x]" in original_line or "- [X]" in original_line):
            completion_str = match.group(0)
        else:
            today = datetime.now().strftime("%Y-%m-%d")
            completion_str = f" ✅ {today}"

    new_line = "- [x] " if done else "- [ ] "
    new_line += title.strip()

    for project in projects:
        new_line += f" +{project}"

    for context in contexts:
        new_line += f" @{context}"

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

    try:
        lines[line_index] = new_line
        write_content('\n'.join(lines) + '\n')

        # Handle recurrence when completing
        if recurrence and recurrence.strip() and not was_done and done:
            base_due = due_dt
            next_due = next_due_date(base_due, recurrence.strip())
            if next_due:
                clone_title = title.strip()
                new_rec_line = "- [ ] " + clone_title
                for project in projects:
                    new_rec_line += f" +{project}"
                for context in contexts:
                    new_rec_line += f" @{context}"
                new_rec_line += f" due:{format_due(next_due)}"
                new_rec_line += f" rec:{recurrence.strip()}"
                if reference and reference.strip():
                    new_rec_line += f" [[{reference.strip()}]]"
                if note_value:
                    new_rec_line += f' ~note:"{escape_note(note_value)}"'
                new_rec_line += f" ^{generate_marker()}"
                insert_line(new_rec_line)

        if request.headers.get('X-Requested-With') == 'XMLHttpRequest':
            return jsonify({
                'success': True,
                'line_index': line_index,
                'message': 'Saved successfully'
            })

        return redirect(url_for('main.index'))

    except Exception as e:
        if request.headers.get('X-Requested-With') == 'XMLHttpRequest':
            return jsonify({'error': str(e)}), 500
        return f"Error saving todo: {str(e)}", 500


@todo_bp.route('/delete/<int:line_index>', methods=['POST'])
@require_login
def delete(line_index):
    """Delete a todo."""
    content = read_content()
    push_undo(content, 'delete')
    delete_todo(line_index)
    return redirect(url_for('main.index'))


@todo_bp.route('/add', methods=['POST'])
@require_login
def add():
    """Add a new todo from form."""
    title = request.form.get('title')
    if title:
        content = read_content()
        push_undo(content, 'add')
        add_todo(title)
    return redirect(url_for('main.index'))


@todo_bp.route('/undo', methods=['POST'])
@require_login
def undo():
    """Undo the last destructive action."""
    entry = pop_undo()
    if entry:
        write_content(entry['content'])
    return redirect(url_for('main.index'))
