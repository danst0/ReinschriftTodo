"""Public share routes — accessible without login via opaque token in URL.

The token in the URL is the capability: anyone with it can view & modify
todos within the share's scope — either a ``+project`` or an ``@context``
(location). CSRF protection is exempt for this blueprint because the token
already serves as bearer authentication — an attacker who can forge the
token has the same access regardless of CSRF.
"""

from __future__ import annotations

import datetime
import logging
import re

from flask import (
    Blueprint,
    abort,
    jsonify,
    render_template,
    request,
)

from app.extensions import csrf
from app.models.todo import CONTEXT_RE, PROJECT_RE
from app.services import (
    add_todo,
    find_line_by_marker,
    handle_toggle_with_recurrence,
    load_todos,
    parse_line,
    read_content,
)
from app.services.share_service import (
    SCOPE_CONTEXT,
    SCOPE_PROJECT,
    get_share_by_token,
    share_scope,
)

logger = logging.getLogger(__name__)

share_bp = Blueprint('share', __name__)

SCOPE_SIGILS = {SCOPE_PROJECT: '+', SCOPE_CONTEXT: '@'}
#: Same patterns as the parser, but anchored to a word start so an address
#: like "info@example.com" is not mistaken for an @context and silently
#: stripped out of the recipient's text.
SCOPE_TAG_RES = {
    SCOPE_PROJECT: re.compile(r'(?<!\S)' + PROJECT_RE.pattern),
    SCOPE_CONTEXT: re.compile(r'(?<!\S)' + CONTEXT_RE.pattern),
}
#: Control characters (newlines included) would break one submission into
#: several markdown lines — see :func:`_normalize_title`.
CONTROL_CHARS_RE = re.compile(r'[\x00-\x1f\x7f]')


def _resolve_share(token: str) -> tuple[str, str]:
    """Look up a share by token and return its ``(scope_type, name)``.

    Aborts with a generic 404 (not "token not found") to avoid leaking
    whether a given token exists, which would help an attacker enumerate
    valid tokens. A stored entry without a usable scope is treated the same
    way — it can only result from corrupted settings.
    """
    share = get_share_by_token(token)
    if not share:
        abort(404)
    scope = share_scope(share)
    if not scope:
        logger.warning("Share %s… has no project/context scope", token[:4])
        abort(404)
    return scope


def _in_scope(item, scope_type: str, name: str) -> bool:
    """True if the todo's *primary* tag is the share's tag.

    Only the first tag counts, because that is the one the owner's list
    groups by: sharing the '@Baumarkt' header must not also hand out todos
    that merely mention @Baumarkt behind some other location. Casing is
    compared with ``lower()`` — the same normalization
    :func:`canonical_casing_map` uses to build those groups, so the share
    covers exactly one group and never a neighbouring one (``casefold()``
    would fold 'ß' into 'ss' and merge two distinct groups).
    """
    tags = item.projects if scope_type == SCOPE_PROJECT else item.contexts
    return bool(tags) and tags[0].lower() == name.lower()


def _normalize_title(text: str) -> str:
    """Flatten recipient input into a single line of text.

    Control characters become spaces and whitespace runs collapse. Without
    this a submitted '\\n' would split one entry into several markdown
    lines, and every line after the first would land in the file untagged —
    outside the share's scope entirely.
    """
    return ' '.join(CONTROL_CHARS_RE.sub(' ', text).split())


def _strip_scope_tags(text: str, scope_type: str) -> str:
    """Remove tokens of the share's own kind from text.

    Lets a recipient type '@Baumarkt' on the @Baumarkt share without ending
    up with the tag twice. Everything else is left alone and rejected later
    by :func:`_only_scope_tag`, rather than silently deleted.
    """
    return SCOPE_TAG_RES[scope_type].sub('', text).strip()


def _only_scope_tag(composed: str, scope_type: str, name: str) -> bool:
    """True if the composed line carries nothing but a title and the share tag.

    The public add route is unauthenticated, so it is deny-by-default: the
    line is parsed exactly as the storage layer will read it back, and any
    metadata the recipient managed to smuggle in — a foreign +project or
    @context, a '^marker' that would collide with an existing todo's id, a
    due date, recurrence or note — makes the submission invalid instead of
    being written to the shared file.
    """
    item = parse_line(f"- [ ] {composed}", 0)
    if item is None:
        return False
    expected_projects = [name] if scope_type == SCOPE_PROJECT else []
    expected_contexts = [name] if scope_type == SCOPE_CONTEXT else []
    return (
        item.projects == expected_projects
        and item.contexts == expected_contexts
        and item.marker is None
        and item.due is None
        and item.recurrence is None
        and item.note is None
        and not item.in_myday
        and item.myday is None
        and not item.done
        and bool(item.title.strip())
    )


def _quote_if_needed(name: str) -> str:
    """Render a tag name in the form expected by the todo grammar.

    Multi-word names need to be quoted (matching Rust core's parser).
    """
    if any(ch.isspace() for ch in name):
        escaped = name.replace('\\', '\\\\').replace('"', '\\"')
        return f'"{escaped}"'
    return name


@share_bp.route('/s/<token>')
def view(token: str):
    """Render the share view: open todos within the share's scope."""
    scope_type, name = _resolve_share(token)

    todos = load_todos()
    visible = [t for t in todos if not t.done and _in_scope(t, scope_type, name)]
    # Stable ordering: by due date, then by title
    visible.sort(key=lambda t: (t.due or datetime.datetime.max, t.title.lower()))

    return render_template(
        'shared.html',
        token=token,
        scope_type=scope_type,
        scope_name=name,
        scope_sigil=SCOPE_SIGILS[scope_type],
        todos=[t.to_dict() for t in visible],
    )


@share_bp.route('/s/<token>/toggle/<marker>', methods=['POST'])
@csrf.exempt
def toggle(token: str, marker: str):
    """Mark a todo as done (open → done only) within the share's scope.

    Restrictions:
        * The todo must currently be open. Recipients only see open todos,
          so resurrecting completed ones via a stale marker is rejected.
        * The todo must carry the share's tag. Without this check, one share
          token could be used to toggle arbitrary todos by guessing markers.
    """
    scope_type, name = _resolve_share(token)

    content = read_content()
    lines = content.splitlines()
    line_index = find_line_by_marker(lines, marker)
    if line_index is None:
        return jsonify({'error': 'not_found'}), 404

    item = parse_line(lines[line_index], line_index)
    if not item:
        return jsonify({'error': 'invalid'}), 400

    if not _in_scope(item, scope_type, name):
        return jsonify({'error': 'forbidden'}), 403

    if item.done:
        return jsonify({'error': 'already_done'}), 409

    handle_toggle_with_recurrence(line_index)
    return jsonify({'ok': True})


@share_bp.route('/s/<token>/add', methods=['POST'])
@csrf.exempt
def add(token: str):
    """Add a new todo into the share's scope.

    Recipients may only contribute plain task text: the share's own tag is
    enforced, and anything else that would parse as metadata is rejected
    (``invalid_title``) rather than stripped, so nothing the recipient typed
    disappears silently and nothing lands outside the share's scope.
    """
    scope_type, name = _resolve_share(token)

    payload = request.get_json(silent=True) or {}
    title = _normalize_title(payload.get('title') or '')
    if not title:
        return jsonify({'error': 'title_required'}), 400

    sanitized = _strip_scope_tags(title, scope_type)
    if not sanitized:
        return jsonify({'error': 'title_required'}), 400

    scope_token = f"{SCOPE_SIGILS[scope_type]}{_quote_if_needed(name)}"
    composed = f"{sanitized} {scope_token}"
    if not _only_scope_tag(composed, scope_type, name):
        return jsonify({'error': 'invalid_title'}), 400

    result = add_todo(composed)
    return jsonify({'ok': True, 'marker': result['marker']})
