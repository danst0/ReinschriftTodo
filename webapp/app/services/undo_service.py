"""Session-based undo stack for reverting destructive operations."""

import hashlib

from flask import session

MAX_UNDO_DEPTH = 5
UNDO_SESSION_KEY = '_undo_stack'


def content_hash(content: str) -> str:
    """Hash file content for change detection (not for security)."""
    return hashlib.sha256(content.encode('utf-8')).hexdigest()


def push_undo(content: str, description: str) -> None:
    """Push a snapshot onto the undo stack.

    Args:
        content: File content before the mutation.
        description: Human-readable description of the action.
    """
    stack = session.get(UNDO_SESSION_KEY, [])
    # 'expected_hash' is filled in once the mutation actually reached storage;
    # an undo without it cannot tell whether somebody else has written since.
    stack.append({'content': content, 'description': description, 'expected_hash': None})
    if len(stack) > MAX_UNDO_DEPTH:
        stack = stack[-MAX_UNDO_DEPTH:]
    session[UNDO_SESSION_KEY] = stack


def pop_undo() -> dict | None:
    """Pop the most recent snapshot from the undo stack.

    Returns:
        Dict with 'content' and 'description', or None if empty.
    """
    stack: list[dict] = session.get(UNDO_SESSION_KEY, [])
    if not stack:
        return None
    entry = stack.pop()
    session[UNDO_SESSION_KEY] = stack
    return entry


def can_undo() -> bool:
    """Check whether the undo stack has entries."""
    return bool(session.get(UNDO_SESSION_KEY, []))


def stamp_last_result(content: str) -> None:
    """Record the state the most recent pushed mutation produced.

    An undo restores a whole file. Without knowing which state it is meant to
    replace, it would also roll back everything other clients stored in the
    meantime — which is how a stale "Undo" wipes fresh work.
    """
    stack: list[dict] = session.get(UNDO_SESSION_KEY, [])
    if not stack:
        return
    stack[-1]['expected_hash'] = content_hash(content)
    session[UNDO_SESSION_KEY] = stack


def push_entry(entry: dict) -> None:
    """Put an entry back after its undo was refused."""
    stack: list[dict] = session.get(UNDO_SESSION_KEY, [])
    stack.append(entry)
    session[UNDO_SESSION_KEY] = stack
