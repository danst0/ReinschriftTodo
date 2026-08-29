"""Session-based undo for the last few mutations.

Undo used to restore a whole file snapshot taken before the mutation. That
silently rolled back everything other clients had written in the meantime: the
snapshot was read at the start of the request, and anything that landed between
that read and the undo simply ceased to exist. It is the one path that wrote
without regard for the current state — every other write is guarded by
``If-Match``.

So an entry no longer holds a file. It holds what the mutation did to
*individual todo lines*, and undoing replays the inverse of that onto the file
as it is now. Lines the entry says nothing about — foreign writes included —
are never touched.
"""

from flask import g, has_request_context, session

from app.services.parser import find_line_by_marker, marker_of

MAX_UNDO_DEPTH = 5
UNDO_SESSION_KEY = '_undo_stack'
PENDING_ATTR = '_undo_pending'


def _line_index(content: str) -> dict[str, str]:
    """Map every marker in ``content`` to the line carrying it."""
    lines: dict[str, str] = {}
    for line in content.splitlines():
        marker = marker_of(line)
        if marker is not None:
            lines.setdefault(marker, line)
    return lines


def _structure(content: str) -> list[str]:
    """The lines carrying no marker: headings, the separator, blanks."""
    return [line for line in content.splitlines() if marker_of(line) is None]


def _preceding_marker(content: str, marker: str) -> str | None:
    """The marker of the todo directly above ``marker`` — where to put it back."""
    previous = None
    for line in content.splitlines():
        found = marker_of(line)
        if found == marker:
            return previous
        if found is not None:
            previous = found
    return None


def _diff_ops(before: str, after: str) -> list[dict] | None:
    """Describe, per todo line, what the mutation changed.

    Returns ``None`` when the change cannot be expressed line by line, which
    happens once a line without a marker moves: a heading or the ``---``
    separator has no identity to replay against, and guessing its position in a
    file somebody else has edited would corrupt it. No undo is better than a
    wrong one.
    """
    if _structure(before) != _structure(after):
        return None

    before_lines, after_lines = _line_index(before), _line_index(after)
    ops: list[dict] = []
    for marker in dict.fromkeys([*before_lines, *after_lines]):
        was, now = before_lines.get(marker), after_lines.get(marker)
        if was == now:
            continue
        op = {'marker': marker, 'before': was, 'after': now}
        if now is None:
            # Deleted: remember where it sat so it returns to its old place.
            op['preceding'] = _preceding_marker(before, marker)
        ops.append(op)
    return ops


def push_undo(content: str, description: str) -> None:
    """Record the state the mutation about to run starts from.

    Nothing reaches the session yet — only a write that actually lands turns
    this into an entry (see :func:`stamp_last_result`). An action that fails
    halfway therefore leaves nothing behind that a later undo could restore.

    Args:
        content: File content before the mutation.
        description: Human-readable description of the action.
    """
    if not has_request_context():
        return
    setattr(g, PENDING_ATTR, {
        'before': content,
        'description': description,
        'pushed': False,
    })


def stamp_last_result(content: str) -> None:
    """Turn the pending mutation into an undo entry, given what it produced.

    Called for every write, so this is the one place that sees both sides of a
    mutation and can reduce it to the lines it actually touched.
    """
    if not has_request_context():
        return
    pending = getattr(g, PENDING_ATTR, None)
    if pending is None:
        return

    ops = _diff_ops(pending['before'], content)
    if not ops:
        # Nothing changed, or nothing that can be replayed line by line.
        return

    entry = {'ops': ops, 'description': pending['description']}
    stack: list[dict] = session.get(UNDO_SESSION_KEY, [])
    if pending['pushed'] and stack:
        # A second write inside the same request continues the same action;
        # it must extend that entry, not stack a half one on top of it.
        stack[-1] = entry
    else:
        stack.append(entry)
        pending['pushed'] = True
    session[UNDO_SESSION_KEY] = stack[-MAX_UNDO_DEPTH:]


def pop_undo() -> dict | None:
    """Pop the most recent entry from the undo stack.

    Returns:
        Dict with 'ops' and 'description', or None if empty.
    """
    stack: list[dict] = session.get(UNDO_SESSION_KEY, [])
    if not stack:
        return None
    entry = stack.pop()
    session[UNDO_SESSION_KEY] = stack
    return entry


def push_entry(entry: dict) -> None:
    """Put an entry back after its undo was refused."""
    stack: list[dict] = session.get(UNDO_SESSION_KEY, [])
    stack.append(entry)
    session[UNDO_SESSION_KEY] = stack


def can_undo() -> bool:
    """Check whether the undo stack has entries."""
    return bool(session.get(UNDO_SESSION_KEY, []))


def _restore_position(lines: list[str], op: dict) -> int:
    """Where a deleted line goes back in — below the todo it used to follow."""
    from app.services.todo_service import _separator_index

    if 'preceding' not in op:
        return _separator_index(lines)

    preceding = op['preceding']
    if preceding is None:
        # It had no todo above it, so it goes back ahead of all of them rather
        # than to where a brand new todo would land.
        for index, line in enumerate(lines):
            if marker_of(line) is not None:
                return index
        return _separator_index(lines)

    found = find_line_by_marker(lines, preceding)
    return found + 1 if found is not None else _separator_index(lines)


def apply_undo(entry: dict, content: str) -> tuple[str | None, int]:
    """Replay the inverse of a recorded mutation onto ``content``.

    Returns:
        The resulting content and the number of changes left alone. A line that
        no longer looks the way the mutation left it has been written by
        somebody else since, so it stays as it is rather than being rolled back
        along with ours. ``None`` means nothing was left to undo.
    """
    lines = content.splitlines()
    applied = skipped = 0

    for op in entry.get('ops', []):
        was, now = op.get('before'), op.get('after')
        index = find_line_by_marker(lines, op['marker'])

        if index is None:
            if now is None and was is not None:
                # We deleted it and nobody has re-created it: put it back.
                lines.insert(_restore_position(lines, op), was)
                applied += 1
            else:
                skipped += 1
            continue

        if lines[index] != now:
            skipped += 1
            continue

        if was is None:
            lines.pop(index)
        else:
            lines[index] = was
        applied += 1

    if not applied:
        return None, skipped
    return '\n'.join(lines) + '\n', skipped
