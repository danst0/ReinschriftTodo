"""API routes blueprint - JSON endpoints."""

from flask import Blueprint, request, session, jsonify, current_app

from app.exceptions import ConflictError
from app.extensions import csrf
from app.services import (
    read_content,
    parse_line,
    find_line_by_marker,
    add_todo,
    update_todo_by_marker,
    parse_nlp,
    postpone_todos_batch,
    load_todos,
)
from app.services.ai_service import parse_nlp_with_debug, get_top_tags
from app.services.storage import write_content
from app.services.undo_service import push_undo, pop_undo, can_undo
from app.utils.helpers import format_due

api_bp = Blueprint('api', __name__)


def require_login_json(f):
    """Decorator to require login for JSON API routes."""
    from functools import wraps

    @wraps(f)
    def decorated(*args, **kwargs):
        if 'logged_in' not in session:
            return jsonify({'error': 'Unauthorized'}), 401
        return f(*args, **kwargs)
    return decorated


@api_bp.route('/todo/<int:line_index>')
@require_login_json
def get_todo_json(line_index):
    """Get a todo as JSON."""
    content = read_content()
    lines = content.splitlines()

    if line_index >= len(lines):
        return jsonify({'error': 'Not found'}), 404

    line = lines[line_index]
    item = parse_line(line, line_index)
    if not item:
        return jsonify({'error': 'Invalid item'}), 400

    # Convert to dict for JSON
    result = item.to_dict() if hasattr(item, 'to_dict') else dict(item)

    return jsonify(result)


@api_bp.route('/parse', methods=['POST'])
@require_login_json
def api_parse_nlp():
    """Parse natural language input using AI."""
    data = request.get_json()
    text = data.get('text') if data else None

    if not text:
        return jsonify({'error': 'No text provided'}), 400

    result = parse_nlp(text)

    if result is None:
        # Input was rejected or parsing failed silently
        return jsonify(None)

    if isinstance(result, tuple):
        # Error tuple from old code path
        return jsonify(result[0]), result[1]

    return jsonify(result)


@api_bp.route('/add', methods=['POST'])
@csrf.exempt
@require_login_json
def api_add():
    """Add a new todo via API."""
    payload = request.get_json(silent=True) or {}
    title = payload.get('title')

    if not title or not str(title).strip():
        return jsonify({'error': 'Title required'}), 400

    result = add_todo(title)
    return jsonify({'ok': True, 'marker': result['marker'], 'line_index': result['line_index']})


@api_bp.route('/improve', methods=['POST'])
@csrf.exempt
@require_login_json
def api_improve():
    """Improve a todo with AI-parsed data."""
    data = request.get_json(silent=True) or {}
    marker = data.get('marker')

    if not marker:
        return jsonify({'error': 'Marker required'}), 400

    updated = {
        'title': data.get('title'),
        'note': data.get('note'),
        'due': data.get('due'),
        'contexts': data.get('contexts'),
        'projects': data.get('projects'),
    }

    if not update_todo_by_marker(marker, updated):
        return jsonify({'error': 'Todo not found'}), 404

    return jsonify({'ok': True})


@api_bp.route('/parse-debug', methods=['POST'])
@require_login_json
def api_parse_debug():
    """Parse natural language input with full debug information.

    Returns detailed debug info including system prompt, raw response,
    timing, and parsed result. Only available when AI_DEBUG_ENABLED=true.
    """
    if not current_app.config.get('AI_DEBUG_ENABLED', False):
        return jsonify({'error': 'Debug mode not enabled'}), 403

    data = request.get_json()
    text = data.get('text') if data else None

    if not text:
        return jsonify({'error': 'No text provided'}), 400

    result = parse_nlp_with_debug(text)
    return jsonify(result)


@api_bp.route('/postpone-batch', methods=['POST'])
@csrf.exempt
@require_login_json
def api_postpone_batch():
    """Postpone multiple todos in a single operation.

    Expects JSON: {"line_indexes": [0, 1, 2], "target": "tomorrow"}
    Returns: {"ok": true, "updated": 3, "failed": []}
    """
    data = request.get_json(silent=True) or {}
    line_indexes = data.get('line_indexes', [])
    target = data.get('target', '')

    if not line_indexes:
        return jsonify({'error': 'No line indexes provided'}), 400

    if not target:
        return jsonify({'error': 'No target provided'}), 400

    valid_targets = ('today', 'tomorrow', 'weekend', 'sometime')
    if target not in valid_targets:
        return jsonify({'error': f'Invalid target. Must be one of: {valid_targets}'}), 400

    try:
        line_indexes = [int(idx) for idx in line_indexes]
    except (TypeError, ValueError):
        return jsonify({'error': 'Invalid line indexes'}), 400

    result = postpone_todos_batch(line_indexes, target)
    return jsonify({'ok': True, **result})


@api_bp.route('/move-to-section', methods=['POST'])
@csrf.exempt
@require_login_json
def api_move_to_section():
    """Move a todo from one project/context section to another.

    Expects JSON: {"marker": "ABC", "group_mode": "topic", "from_key": "A", "to_key": "C"}
    For topic mode, replaces the from_key project with to_key project.
    For location mode, replaces the from_key context with to_key context.
    """
    data = request.get_json(silent=True) or {}
    marker = data.get('marker')
    group_mode = data.get('group_mode')
    from_key = data.get('from_key', '')
    to_key = data.get('to_key', '')

    if not marker:
        return jsonify({'error': 'Marker required'}), 400

    if group_mode not in ('topic', 'location'):
        return jsonify({'error': 'Invalid group_mode'}), 400

    if from_key == to_key:
        return jsonify({'error': 'Source and target are the same'}), 400

    # Load and find the todo
    content = read_content()
    lines = content.splitlines()
    target_index = find_line_by_marker(lines, marker)
    if target_index is None:
        return jsonify({'error': 'Todo not found'}), 404

    item = parse_line(lines[target_index], target_index)
    if not item:
        return jsonify({'error': 'Invalid item'}), 400

    if group_mode == 'topic':
        projects = list(item.projects)
        # Remove from_key project (if present)
        if from_key and from_key in projects:
            projects.remove(from_key)
        # Add to_key project (if not empty and not already present)
        if to_key and to_key not in projects:
            projects.append(to_key)
        if not update_todo_by_marker(marker, {'projects': projects}):
            return jsonify({'error': 'Update failed'}), 500
    else:
        # location mode
        contexts = list(item.contexts)
        if from_key and from_key in contexts:
            contexts.remove(from_key)
        if to_key and to_key not in contexts:
            contexts.append(to_key)
        if not update_todo_by_marker(marker, {'contexts': contexts}):
            return jsonify({'error': 'Update failed'}), 500

    return jsonify({'ok': True})


@api_bp.route('/undo', methods=['POST'])
@csrf.exempt
@require_login_json
def api_undo():
    """Undo the last destructive action."""
    entry = pop_undo()
    if not entry:
        return jsonify({'error': 'Nothing to undo'}), 404
    try:
        write_content(entry['content'])
        return jsonify({'ok': True, 'description': entry['description']})
    except Exception as e:
        return jsonify({'error': str(e)}), 500


@api_bp.route('/suggestions')
@require_login_json
def get_suggestions():
    """Return top 5 projects and contexts for autocomplete."""
    todos = load_todos()
    projects, contexts = get_top_tags(todos, max_projects=5, max_contexts=5)
    return jsonify({
        'projects': projects,
        'contexts': contexts
    })
