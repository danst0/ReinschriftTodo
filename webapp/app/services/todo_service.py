"""Todo CRUD service."""

import logging
import re
import shlex
from datetime import date, datetime
from typing import Optional, Any

from app.models.todo import (
    TodoItem, ID_RE, COMPLETION_RE, DUE_RE, MYDAY_STRIP_RE, DEFAULT_DUE_TIME
)
from app.services.storage import read_content, write_content
from app.services.parser import parse_line, find_line_by_marker, parse_due_input
from app.services.date_service import next_due_date
from app.utils.markers import generate_marker, ensure_marker
from app.utils.escaping import escape_note
from app.utils.helpers import format_due, normalize_prefix


def _split_tag_input(raw: str, prefix_char: str) -> list[str]:
    """Tokenize free-form project/context input, honoring shell-like quoting."""
    try:
        tokens = shlex.split(raw)
    except ValueError:
        # Unmatched quotes — fall back to whitespace split so the user still
        # sees their input saved.
        tokens = raw.split()
    return [t for t in (tok.strip().lstrip(prefix_char).strip() for tok in tokens) if t]


def _render_tagged(prefix: str, name: str) -> str:
    """Render a +project / @context token, quoting when the name needs it."""
    needs_quote = any(ch.isspace() or ch in '"\\' for ch in name)
    if needs_quote:
        return f'{prefix}"{escape_note(name)}"'
    return f'{prefix}{name}'

def _myday_segment(item: TodoItem) -> str:
    """Render the myday token for line reconstruction.

    Stale dates (before today) are dropped so old planning tokens clean
    themselves up over time — mirrors the Rust renderer.
    """
    if item.myday and item.myday >= date.today():
        return f" myday:{item.myday.strftime('%Y-%m-%d')}"
    return ""


logger = logging.getLogger(__name__)


def load_todos() -> list[TodoItem]:
    """Load all todos from storage.

    Returns:
        List of parsed TodoItem objects.
    """
    content = read_content()
    if not content:
        return []

    items = []
    for line_index, line in enumerate(content.splitlines()):
        item = parse_line(line, line_index)
        if item:
            items.append(item)

    return items


def toggle_todo(line_index: int, done: bool) -> None:
    """Toggle a todo's completion status.

    Args:
        line_index: Line index of the todo.
        done: New completion status.
    """
    content = read_content()
    lines = content.splitlines()

    if line_index < len(lines):
        lines[line_index] = rewrite_line(lines[line_index], done)
        write_content('\n'.join(lines) + '\n')


def rewrite_line(line: str, done: bool) -> str:
    """Rewrite a todo line with new completion status.

    Args:
        line: Original line.
        done: New completion status.

    Returns:
        Updated line.
    """
    updated = line
    # Remove existing completion marker first
    updated = COMPLETION_RE.sub("", updated)

    if done:
        updated = updated.replace("- [ ]", "- [x]", 1)
        updated = updated.replace("- [X]", "- [x]", 1)
        today = datetime.now().strftime("%Y-%m-%d")
        done_marker = f" ✅ {today}"

        # Place Done Date before ID if ID exists
        marker_match = ID_RE.search(updated)
        if marker_match:
            start = marker_match.start()
            updated = updated[:start].rstrip() + done_marker + " " + updated[start:].lstrip()
        else:
            updated = updated.rstrip() + done_marker
    else:
        updated = updated.replace("- [x]", "- [ ]", 1)
        updated = updated.replace("- [X]", "- [ ]", 1)

    return updated


def add_todo(title: str, myday: bool = False) -> dict[str, Any]:
    """Add a new todo.

    Args:
        title: Todo title (may include inline metadata).
        myday: Plan the new todo for today (adds a myday: token).

    Returns:
        Dict with 'marker' and 'line_index'.
    """
    content = read_content()
    lines = content.splitlines()

    insert_index = len(lines)
    for i, line in enumerate(lines):
        if line.strip() == "---":
            insert_index = i
            break

    default_due = datetime.combine(datetime.now().date(), DEFAULT_DUE_TIME)
    default_due_str = format_due(default_due)
    marker = generate_marker()

    if DUE_RE.search(title):
        new_line = f"- [ ] {title}"
    else:
        new_line = f"- [ ] {title} due:{default_due_str}"
    if myday:
        new_line += f" myday:{date.today().strftime('%Y-%m-%d')}"
    new_line += f" ^{marker}"
    lines.insert(insert_index, new_line)

    write_content('\n'.join(lines) + '\n')
    return {
        'marker': marker,
        'line_index': insert_index,
    }


def insert_line(line: str) -> None:
    """Insert a line at the appropriate position.

    Args:
        line: Line to insert.
    """
    content = read_content()
    lines = content.splitlines()

    insert_index = len(lines)
    for i, l in enumerate(lines):
        if l.strip() == "---":
            insert_index = i
            break

    lines.insert(insert_index, line)
    write_content('\n'.join(lines) + '\n')


def delete_todo(line_index: int) -> bool:
    """Delete a todo by line index.

    Args:
        line_index: Line index to delete.

    Returns:
        True if deleted, False if index out of range.
    """
    content = read_content()
    lines = content.splitlines()

    if line_index < len(lines):
        lines.pop(line_index)
        write_content('\n'.join(lines) + '\n')
        return True
    return False


def update_todo_by_marker(marker: str, updates: dict[str, Any]) -> bool:
    """Update a todo by its marker ID.

    Args:
        marker: Marker ID.
        updates: Dictionary of fields to update.

    Returns:
        True if updated, False if not found.
    """
    content = read_content()
    lines = content.splitlines()

    target_index = find_line_by_marker(lines, marker)
    if target_index is None:
        return False

    existing = parse_line(lines[target_index], target_index)
    if not existing:
        return False

    title = (updates.get('title') or existing.title or '').strip()
    if not title:
        title = existing.title or ''

    # Handle projects - can be a string (shell-quoted) or list
    if 'projects' in updates:
        projects_input = updates.get('projects')
        if isinstance(projects_input, str):
            projects = _split_tag_input(projects_input, '+')
        elif isinstance(projects_input, list):
            projects = [p.strip().lstrip('+').strip() for p in projects_input if p and p.strip().lstrip('+').strip()]
        else:
            projects = existing.projects
    else:
        projects = existing.projects

    # Handle contexts - can be a string (shell-quoted) or list
    if 'contexts' in updates:
        contexts_input = updates.get('contexts')
        if isinstance(contexts_input, str):
            contexts = _split_tag_input(contexts_input, '@')
        elif isinstance(contexts_input, list):
            contexts = [c.strip().lstrip('@').strip() for c in contexts_input if c and c.strip().lstrip('@').strip()]
        else:
            contexts = existing.contexts
    else:
        contexts = existing.contexts

    if 'note' in updates:
        note_value = updates.get('note')
        note_value = note_value.strip() if isinstance(note_value, str) else None
    else:
        note_value = existing.note

    if 'due' in updates and updates.get('due') is not None:
        due_dt = parse_due_input(updates.get('due')) or existing.due
    elif 'due' in updates and updates.get('due') is None:
        due_dt = existing.due
    else:
        due_dt = existing.due

    recurrence = existing.recurrence
    reference = existing.reference
    done = existing.done

    new_line = "- [x]" if done else "- [ ]"
    new_line += f" {title}"

    for project in projects:
        new_line += f" {_render_tagged('+', project)}"
    for context in contexts:
        new_line += f" {_render_tagged('@', context)}"
    if due_dt:
        new_line += f" due:{format_due(due_dt)}"
    new_line += _myday_segment(existing)
    if recurrence:
        new_line += f" rec:{recurrence}"
    if reference:
        new_line += f" [[{reference}]]"
    if note_value:
        new_line += f' ~note:"{escape_note(note_value)}"'

    if done:
        done_date = existing.done_date or datetime.now()
        new_line += f" ✅ {done_date.strftime('%Y-%m-%d')}"

    new_line += f" ^{marker}"

    lines[target_index] = new_line
    write_content('\n'.join(lines) + '\n')
    return True


def handle_toggle_with_recurrence(line_index: int) -> None:
    """Handle toggling a todo that has recurrence.

    Args:
        line_index: Line index of the todo.
    """
    content = read_content()
    lines = content.splitlines()

    if line_index >= len(lines):
        return

    line = lines[line_index]
    is_done = "- [x]" in line or "- [X]" in line
    item = parse_line(line, line_index)

    if not item:
        return

    now = datetime.now()

    # Handle overdue recurring tasks
    if not is_done and item.recurrence and item.due and item.due < now:
        base_time = item.due.time() if item.due else DEFAULT_DUE_TIME
        due_dt = datetime.combine(now.date(), base_time)
        due_str = format_due(due_dt)
        new_line = re.sub(r'due:\d{4}-\d{2}-\d{2}(?:T\d{2}:\d{2})?', f'due:{due_str}', line)
        lines[line_index] = new_line
        write_content('\n'.join(lines) + '\n')
        toggle_todo(line_index, True)
    else:
        toggle_todo(line_index, not is_done)

    # Create next occurrence if completing a recurring task
    if item.recurrence and not is_done:
        next_due = next_due_date(item.due, item.recurrence)
        if next_due:
            new_line = "- [ ] " + item.title.strip()
            for project in item.projects:
                project_clean = normalize_prefix(project, '+')
                if project_clean:
                    new_line += f" +{project_clean}"
            for context in item.contexts:
                context_clean = normalize_prefix(context, '@')
                if context_clean:
                    new_line += f" @{context_clean}"
            new_line += f" due:{format_due(next_due)}"
            new_line += f" rec:{item.recurrence}"
            if item.reference:
                new_line += f" [[{item.reference.strip()}]]"
            if item.note:
                new_line += f' ~note:"{escape_note(item.note)}"'
            new_line += f" ^{ensure_marker(item.marker)}"
            insert_line(new_line)


def set_myday(marker: str, on: bool) -> bool:
    """Add or remove a todo's "my day" token (today's plan).

    Args:
        marker: Marker ID of the todo.
        on: True to plan for today, False to remove from today's plan.

    Returns:
        True if updated, False if the marker was not found.
    """
    content = read_content()
    lines = content.splitlines()

    target_index = find_line_by_marker(lines, marker)
    if target_index is None:
        return False

    line = MYDAY_STRIP_RE.sub("", lines[target_index])
    if on:
        segment = f" myday:{date.today().strftime('%Y-%m-%d')}"
        due_match = DUE_RE.search(line)
        if due_match:
            # Insert directly after the due token (canonical position).
            pos = due_match.end()
            line = line[:pos] + segment + line[pos:]
        else:
            marker_match = ID_RE.search(line)
            if marker_match:
                pos = marker_match.start()
                line = line[:pos].rstrip() + segment + " " + line[pos:]
            else:
                line = line.rstrip() + segment

    lines[target_index] = line
    write_content('\n'.join(lines) + '\n')
    return True


def postpone_todo(line_index: int, target: str) -> bool:
    """Postpone a todo to a new date.

    Args:
        line_index: Line index of the todo.
        target: Postpone target ('today', 'tomorrow', 'weekend', 'sometime').

    Returns:
        True if updated, False if not found.
    """
    from app.services.date_service import calculate_postpone_date

    content = read_content()
    lines = content.splitlines()

    if line_index >= len(lines):
        return False

    line = lines[line_index]
    item = parse_line(line, line_index)
    if not item:
        return False

    new_datetime, new_time = calculate_postpone_date(target, item.due)

    # Reconstruct line
    original_line = lines[line_index]
    marker = ensure_marker(item.marker)

    new_line = "- [x] " if item.done else "- [ ] "
    new_line += item.title.strip()

    for project in item.projects:
        project_clean = normalize_prefix(project, '+')
        if project_clean:
            new_line += f" +{project_clean}"

    for context in item.contexts:
        context_clean = normalize_prefix(context, '@')
        if context_clean:
            new_line += f" @{context_clean}"

    # Always set the new due date/time
    new_line += f" due:{format_due(new_datetime)}"
    new_line += _myday_segment(item)

    if item.recurrence:
        new_line += f" rec:{item.recurrence}"

    if item.reference and item.reference.strip():
        new_line += f" [[{item.reference.strip()}]]"

    if item.note:
        new_line += f' ~note:"{escape_note(item.note)}"'

    # Preserve completion date if done
    if item.done:
        match = COMPLETION_RE.search(original_line)
        if match:
            new_line += match.group(0)

    new_line += f" ^{marker}"

    lines[line_index] = new_line
    write_content('\n'.join(lines) + '\n')
    return True


def postpone_todos_batch(line_indexes: list[int], target: str) -> dict:
    """Postpone multiple todos to a new date in a single operation.

    This avoids WebDAV locking issues by doing a single read-modify-write cycle.

    Args:
        line_indexes: List of line indexes to postpone.
        target: Postpone target ('today', 'tomorrow', 'weekend', 'sometime').

    Returns:
        Dict with 'updated' count and 'failed' list of indexes that couldn't be updated.
    """
    from app.services.date_service import calculate_postpone_date

    content = read_content()
    lines = content.splitlines()

    updated = 0
    failed = []

    for line_index in line_indexes:
        if line_index >= len(lines):
            failed.append(line_index)
            continue

        line = lines[line_index]
        item = parse_line(line, line_index)
        if not item:
            failed.append(line_index)
            continue

        new_datetime, _ = calculate_postpone_date(target, item.due)
        original_line = lines[line_index]
        marker = ensure_marker(item.marker)

        new_line = "- [x] " if item.done else "- [ ] "
        new_line += item.title.strip()

        for project in item.projects:
            project_clean = normalize_prefix(project, '+')
            if project_clean:
                new_line += f" +{project_clean}"

        for context in item.contexts:
            context_clean = normalize_prefix(context, '@')
            if context_clean:
                new_line += f" @{context_clean}"

        new_line += f" due:{format_due(new_datetime)}"
        new_line += _myday_segment(item)

        if item.recurrence:
            new_line += f" rec:{item.recurrence}"

        if item.reference and item.reference.strip():
            new_line += f" [[{item.reference.strip()}]]"

        if item.note:
            new_line += f' ~note:"{escape_note(item.note)}"'

        if item.done:
            match = COMPLETION_RE.search(original_line)
            if match:
                new_line += match.group(0)

        new_line += f" ^{marker}"

        lines[line_index] = new_line
        updated += 1

    if updated > 0:
        write_content('\n'.join(lines) + '\n')

    return {'updated': updated, 'failed': failed}
