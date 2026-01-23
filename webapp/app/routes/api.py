"""API routes blueprint - JSON endpoints."""

from flask import Blueprint, request, session, jsonify

from app.extensions import csrf
from app.services import (
    read_content,
    parse_line,
    add_todo,
    update_todo_by_marker,
    parse_nlp,
)
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
