"""Public share routes — accessible without login via opaque token in URL.

The token in the URL is the capability: anyone with it can view & modify
todos within the share's project scope. CSRF protection is exempt for this
blueprint because the token already serves as bearer authentication — an
attacker who can forge the token has the same access regardless of CSRF.
"""

from __future__ import annotations

import logging

from flask import (
    Blueprint,
    abort,
    current_app,
    jsonify,
    render_template,
    request,
)

from app.extensions import csrf
from app.models.todo import PROJECT_RE
from app.services import (
    add_todo,
    find_line_by_marker,
    handle_toggle_with_recurrence,
    load_todos,
    parse_line,
    read_content,
)
from app.services.share_service import get_share_by_token

logger = logging.getLogger(__name__)

share_bp = Blueprint('share', __name__)


def _resolve_share(token: str) -> dict:
    """Look up share by token or abort with 404.

    Uses a generic 404 (not "token not found") to avoid leaking whether a
    given token exists, which would help an attacker enumerate valid tokens.
    """
    share = get_share_by_token(token)
    if not share:
        abort(404)
    return share


def _strip_projects(text: str) -> str:
    """Remove all +project tokens from text. Keeps @contexts and other metadata."""
    return PROJECT_RE.sub('', text).strip()


def _quote_project_if_needed(project: str) -> str:
    """Render a project name in the form expected by the todo grammar.

    Multi-word projects need to be quoted (matching Rust core's parser).
    """
    if any(ch.isspace() for ch in project):
        escaped = project.replace('\\', '\\\\').replace('"', '\\"')
        return f'"{escaped}"'
    return project


@share_bp.route('/s/<token>')
def view(token: str):
    """Render the share view: open todos within the share's project scope."""
    share = _resolve_share(token)
    project = share['project']

    todos = load_todos()
    visible = [
        t for t in todos
        if not t.done and project in t.projects
    ]
    # Stable ordering: by due date, then by title
    visible.sort(key=lambda t: (t.due or __import__('datetime').datetime.max, t.title.lower()))

    return render_template(
        'shared.html',
        token=token,
        project=project,
        todos=[t.to_dict() for t in visible],
    )


@share_bp.route('/s/<token>/toggle/<marker>', methods=['POST'])
@csrf.exempt
def toggle(token: str, marker: str):
    """Mark a todo as done (open → done only) within the share's scope.

    Restrictions:
        * The todo must currently be open. Recipients only see open todos,
          so resurrecting completed ones via a stale marker is rejected.
        * The todo must carry the share's project tag. Without this check,
          one share token could be used to toggle arbitrary todos by guessing
          markers.
    """
    share = _resolve_share(token)
    project = share['project']

    content = read_content()
    lines = content.splitlines()
    line_index = find_line_by_marker(lines, marker)
    if line_index is None:
        return jsonify({'error': 'not_found'}), 404

    item = parse_line(lines[line_index], line_index)
    if not item:
        return jsonify({'error': 'invalid'}), 400

    if project not in item.projects:
        return jsonify({'error': 'forbidden'}), 403

    if item.done:
        return jsonify({'error': 'already_done'}), 409

    handle_toggle_with_recurrence(line_index)
    return jsonify({'ok': True})


@share_bp.route('/s/<token>/add', methods=['POST'])
@csrf.exempt
def add(token: str):
    """Add a new todo into the share's project scope.

    The share's project tag is enforced — any +project markers the recipient
    typed are stripped before appending the share's project.
    """
    share = _resolve_share(token)
    project = share['project']

    payload = request.get_json(silent=True) or {}
    title = (payload.get('title') or '').strip()
    if not title:
        return jsonify({'error': 'title_required'}), 400

    sanitized = _strip_projects(title)
    if not sanitized:
        return jsonify({'error': 'title_required'}), 400

    project_token = f"+{_quote_project_if_needed(project)}"
    composed = f"{sanitized} {project_token}"

    result = add_todo(composed)
    return jsonify({'ok': True, 'marker': result['marker']})
