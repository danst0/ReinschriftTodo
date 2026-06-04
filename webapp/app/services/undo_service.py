"""Session-based undo stack for reverting destructive operations."""

from flask import session

MAX_UNDO_DEPTH = 5
UNDO_SESSION_KEY = '_undo_stack'


def push_undo(content: str, description: str) -> None:
    """Push a snapshot onto the undo stack.

    Args:
        content: File content before the mutation.
        description: Human-readable description of the action.
    """
    stack = session.get(UNDO_SESSION_KEY, [])
    stack.append({'content': content, 'description': description})
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
