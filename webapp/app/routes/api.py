"""API routes blueprint - JSON endpoints."""

from flask import Blueprint, request, session, jsonify, current_app, url_for

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
    toggle_todos_batch,
    delete_todos_batch,
    assign_todos_batch,
    duplicate_todo,
    set_myday,
    load_todos,
)
from app.services.ai_service import parse_nlp_with_debug, get_top_tags
from app.services.storage import write_content
from app.services.share_service import (
    ALLOWED_TTL_DAYS,
    SCOPE_PROJECT,
    SCOPE_TYPES,
    create_share,
    delete_share_by_scope,
    get_share_by_scope,
    purge_expired_shares,
)
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

    result = add_todo(title, myday=bool(payload.get('myday')))
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

    push_undo(read_content(), 'postpone')
    result = postpone_todos_batch(line_indexes, target)
    return jsonify({'ok': True, **result})


def _parse_line_indexes(data: dict):
    """Validate the line_indexes field of a batch payload.

    Returns:
        Tuple of (line_indexes, error_response). Exactly one is None.
    """
    line_indexes = data.get('line_indexes', [])
    if not line_indexes:
        return None, (jsonify({'error': 'No line indexes provided'}), 400)
    try:
        return [int(idx) for idx in line_indexes], None
    except (TypeError, ValueError):
        return None, (jsonify({'error': 'Invalid line indexes'}), 400)


@api_bp.route('/toggle-batch', methods=['POST'])
@csrf.exempt
@require_login_json
def api_toggle_batch():
    """Toggle multiple todos in a single operation.

    Expects JSON: {"line_indexes": [0, 1, 2], "done": true}
    Returns: {"ok": true, "updated": 3, "failed": []}
    """
    data = request.get_json(silent=True) or {}
    line_indexes, error = _parse_line_indexes(data)
    if error:
        return error

    done = data.get('done')
    if not isinstance(done, bool):
        return jsonify({'error': 'Field "done" must be a boolean'}), 400

    push_undo(read_content(), 'complete' if done else 'reopen')
    result = toggle_todos_batch(line_indexes, done)
    return jsonify({'ok': True, **result})


@api_bp.route('/delete-batch', methods=['POST'])
@csrf.exempt
@require_login_json
def api_delete_batch():
    """Delete multiple todos in a single operation.

    Expects JSON: {"line_indexes": [0, 1, 2]}
    Returns: {"ok": true, "updated": 3, "failed": []}
    """
    data = request.get_json(silent=True) or {}
    line_indexes, error = _parse_line_indexes(data)
    if error:
        return error

    push_undo(read_content(), 'delete')
    result = delete_todos_batch(line_indexes)
    return jsonify({'ok': True, **result})


@api_bp.route('/assign-batch', methods=['POST'])
@csrf.exempt
@require_login_json
def api_assign_batch():
    """Add a project and/or context to multiple todos in a single operation.

    Expects JSON: {"line_indexes": [0, 1], "project": "tax", "context": "home"}
    At least one of project/context is required.
    Returns: {"ok": true, "updated": 2, "failed": []}
    """
    data = request.get_json(silent=True) or {}
    line_indexes, error = _parse_line_indexes(data)
    if error:
        return error

    project = (data.get('project') or '').strip() or None
    context = (data.get('context') or '').strip() or None
    if not project and not context:
        return jsonify({'error': 'Project or context required'}), 400

    push_undo(read_content(), 'assign')
    result = assign_todos_batch(line_indexes, project=project, context=context)
    return jsonify({'ok': True, **result})


@api_bp.route('/myday/toggle', methods=['POST'])
@csrf.exempt
@require_login_json
def api_myday_toggle():
    """Add or remove a todo from "My Day" (today's plan).

    Expects JSON: {"marker": "ABC123", "on": true}
    Returns: {"ok": true}
    """
    data = request.get_json(silent=True) or {}
    marker = data.get('marker')
    on = data.get('on')

    if not marker:
        return jsonify({'error': 'Marker required'}), 400

    if not isinstance(on, bool):
        return jsonify({'error': 'Field "on" must be a boolean'}), 400

    push_undo(read_content(), 'myday')
    if not set_myday(marker, on):
        return jsonify({'error': 'Todo not found'}), 404

    return jsonify({'ok': True})


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

    if from_key.lower() == to_key.lower():
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

    # Section keys are canonical casings, so match case-insensitively against
    # whatever casing is stored on the todo itself.
    if group_mode == 'topic':
        projects = list(item.projects)
        # Remove from_key project (if present, any casing)
        if from_key:
            projects = [p for p in projects if p.lower() != from_key.lower()]
        # Add to_key project (if not empty and not already present in any casing)
        if to_key and to_key.lower() not in (p.lower() for p in projects):
            projects.append(to_key)
        if not update_todo_by_marker(marker, {'projects': projects}):
            return jsonify({'error': 'Update failed'}), 500
    else:
        # location mode
        contexts = list(item.contexts)
        if from_key:
            contexts = [c for c in contexts if c.lower() != from_key.lower()]
        if to_key and to_key.lower() not in (c.lower() for c in contexts):
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


DEFAULT_SHARE_TTL_DAYS = 30


def _share_response(share: dict | None, scope_type: str, name: str) -> dict:
    """Serialize a share. ``project`` is kept for backward compatibility and
    is only set for project-scoped shares."""
    base = {
        'scope': scope_type,
        'name': name,
        'project': name if scope_type == SCOPE_PROJECT else None,
    }
    if not share:
        return {**base, 'token': None, 'url': None, 'expires_at': None}
    return {
        **base,
        'token': share['token'],
        'url': url_for('share.view', token=share['token'], _external=True),
        'created': share.get('created'),
        'expires_at': share.get('expires_at'),
    }


def _share_scope_args(name: str) -> tuple[str, str] | None:
    """Resolve ``(scope_type, name)`` from the URL path and ``?type=`` query.

    An explicitly empty ``type=`` is rejected rather than defaulting to
    project — a client that builds the query from an unset variable must not
    silently revoke the wrong scope's link. The path segment arrives already
    percent-decoded from Werkzeug; decoding it again would corrupt names
    containing '%'.

    Returns None when the request is malformed; callers turn that into a 400.
    """
    scope_type = request.args.get('type', SCOPE_PROJECT).strip()
    if scope_type not in SCOPE_TYPES:
        return None
    name = name.strip()
    if not name:
        return None
    return scope_type, name


@api_bp.route('/shares/<path:name>', methods=['GET'])
@require_login_json
def api_get_share(name: str):
    """Return the existing share for a project/context, if any."""
    purge_expired_shares()
    scope = _share_scope_args(name)
    if not scope:
        return jsonify({'error': 'scope_required'}), 400
    scope_type, scope_name = scope
    share = get_share_by_scope(scope_type, scope_name)
    return jsonify(_share_response(share, scope_type, scope_name))


@api_bp.route('/shares/<path:name>', methods=['POST'])
@csrf.exempt
@require_login_json
def api_create_share(name: str):
    """Create (or return existing) share token for a project/context.

    The scope kind comes from ``?type=project|context`` (default:
    ``project``). Accepts an optional JSON body ``{"ttl_days":
    7|30|90|null}``. When omitted, defaults to 30 days. ``null`` creates a
    share that never expires.
    """
    purge_expired_shares()
    scope = _share_scope_args(name)
    if not scope:
        return jsonify({'error': 'scope_required'}), 400
    scope_type, scope_name = scope

    payload = request.get_json(silent=True) or {}
    ttl_days: int | None
    if 'ttl_days' in payload:
        raw = payload.get('ttl_days')
        if raw is None:
            ttl_days = None
        elif isinstance(raw, int) and not isinstance(raw, bool) and raw in ALLOWED_TTL_DAYS:
            ttl_days = raw
        else:
            return jsonify({'error': 'invalid_ttl'}), 400
    else:
        ttl_days = DEFAULT_SHARE_TTL_DAYS

    share = create_share(scope_name, ttl_days=ttl_days, scope_type=scope_type)
    return jsonify(_share_response(share, scope_type, scope_name))


@api_bp.route('/shares/<path:name>', methods=['DELETE'])
@csrf.exempt
@require_login_json
def api_delete_share(name: str):
    """Revoke the share for a project/context."""
    purge_expired_shares()
    scope = _share_scope_args(name)
    if not scope:
        return jsonify({'error': 'scope_required'}), 400
    scope_type, scope_name = scope
    ok = delete_share_by_scope(scope_type, scope_name)
    return jsonify({'ok': ok})


@api_bp.route('/title-suggestions')
@require_login_json
def get_title_suggestions():
    """Return all unique non-empty titles, sorted by descending occurrence
    count (ties broken alphabetically). Includes completed tasks.

    `items` carries a representative marker per title (the most recently
    added occurrence) so the frontend can offer a duplicate action.
    """
    todos = load_todos()
    counts: dict[str, int] = {}
    markers: dict[str, str | None] = {}
    for item in todos:
        title = (item.title or '').strip()
        if title:
            counts[title] = counts.get(title, 0) + 1
            # Later lines win: the most recently added occurrence is the
            # best template for a copy.
            markers[title] = item.marker or markers.get(title)
    titles = sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))
    return jsonify({
        'titles': [t for t, _ in titles],
        'items': [{'title': t, 'marker': markers.get(t)} for t, _ in titles],
    })


@api_bp.route('/semantic-suggestions')
@require_login_json
def api_semantic_suggestions():
    """Semantically similar todos for a query (Ollama embeddings).

    Query params: q (text), mode ('tags' | 'search', default 'tags').
    Returns {"enabled": bool, "items": [...]} — enabled is false when the
    feature is off or the embedding backend is unavailable, so the
    frontend silently falls back to substring behavior. Never 500s.
    """
    from app.services.embedding_service import is_semantic_enabled, query_similar

    q = (request.args.get('q') or '').strip()
    mode = request.args.get('mode', 'tags')
    if not q:
        return jsonify({'enabled': is_semantic_enabled(), 'items': []})
    items = query_similar(q, mode=mode)
    if items is None:
        return jsonify({'enabled': False, 'items': []})
    return jsonify({'enabled': True, 'items': items})


@api_bp.route('/check-duplicate')
@require_login_json
def api_check_duplicate():
    """Warn about a very similar OPEN task before adding a new one.

    Query param: title. Returns {"enabled": bool, "match": {...} | null}.
    Conservative threshold; the frontend only warns, never blocks.
    """
    from app.services.embedding_service import find_duplicate

    title = (request.args.get('title') or '').strip()
    if not title:
        return jsonify({'enabled': False, 'match': None})
    available, match = find_duplicate(title)
    return jsonify({'enabled': available, 'match': match})


@api_bp.route('/duplicate', methods=['POST'])
@csrf.exempt
@require_login_json
def api_duplicate():
    """Duplicate an existing todo as a new open task.

    Expects JSON: {"marker": "ABC123"}
    The copy keeps projects, contexts, note, recurrence and reference,
    gets a fresh marker and the default due date (today), and is not done.
    Returns: {"ok": true, "marker": "...", "line_index": N}
    """
    data = request.get_json(silent=True) or {}
    marker = data.get('marker')

    if not marker:
        return jsonify({'error': 'Marker required'}), 400

    snapshot = read_content()
    result = duplicate_todo(marker)
    if result is None:
        return jsonify({'error': 'Todo not found'}), 404

    push_undo(snapshot, 'add')
    return jsonify({'ok': True, **result})
