"""Todo CRUD service."""

import logging
import re
from dataclasses import replace
from datetime import date, datetime
from typing import Optional, Any

from app.exceptions import StaleReferenceError
from app.models.todo import (
    TodoItem, ID_RE, COMPLETION_RE, DUE_RE, MYDAY_STRIP_RE, DEFAULT_DUE_TIME
)
from app.services.storage import read_content, write_content
from app.services.parser import parse_line, find_line_by_marker, parse_due_input
from app.services.date_service import next_due_date
from app.utils.markers import generate_marker, ensure_marker
from app.utils.escaping import escape_note
from app.utils.helpers import format_due, normalize_prefix, split_tag_input


def render_tagged(prefix: str, name: str) -> str:
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


def _render_todo_line(item: TodoItem, original_line: str,
                      due: Optional[datetime] = None) -> str:
    """Reconstruct a markdown line from a parsed item.

    Mirrors the Rust renderer: quoted +project/@context tokens, canonical
    field order, completion date preserved from the original line.

    Args:
        item: Parsed todo item to render.
        original_line: Source line (used to preserve the completion date).
        due: Override for the due date; defaults to the item's own.
    """
    marker = ensure_marker(item.marker)

    new_line = "- [x] " if item.done else "- [ ] "
    new_line += item.title.strip()

    for project in item.projects:
        project_clean = normalize_prefix(project, '+')
        if project_clean:
            new_line += f" {render_tagged('+', project_clean)}"

    for context in item.contexts:
        context_clean = normalize_prefix(context, '@')
        if context_clean:
            new_line += f" {render_tagged('@', context_clean)}"

    due_value = due if due is not None else item.due
    if due_value:
        new_line += f" due:{format_due(due_value)}"
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
    return new_line


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


def resolve_index(lines: list[str], line_index: int,
                  marker: Optional[str] = None) -> Optional[int]:
    """Resolve a todo's line, preferring its marker over the index.

    The index comes from a page that was rendered earlier; if the file changed
    since, it points at whatever moved into that slot. Writing there touches a
    todo the user never clicked — and if that line happened to be completed
    already, toggling it puts a finished task back on the list.

    Returns None when the caller supplied a marker that is no longer in the
    file. The todo was deleted or rewritten elsewhere, and the index that
    travelled with it is worthless. Lines without a marker (hand-written ones)
    still resolve by index; there is nothing better to go on.
    """
    if marker:
        return find_line_by_marker(lines, marker)
    if 0 <= line_index < len(lines):
        return line_index
    return None


def _resolve_or_raise(lines: list[str], line_index: int,
                      marker: Optional[str]) -> int:
    """Resolve a line or refuse the write, rather than hitting a foreign todo."""
    index = resolve_index(lines, line_index, marker)
    if index is None:
        raise StaleReferenceError(marker or "")
    return index


def _resolve_batch(lines: list[str], line_indexes: list[int],
                   markers: Optional[list[str]] = None
                   ) -> tuple[list[tuple[int, int]], list[int]]:
    """Resolve a batch of todos, pairing each index with its marker.

    ``markers`` is positional: entry *n* belongs to ``line_indexes[n]``. A
    missing or empty entry means "no marker known", which falls back to the
    index. One vanished todo should not sink the whole batch, so entries that
    cannot be resolved are reported instead of raising.

    Returns:
        (resolved, failed) — resolved holds (requested_index, actual_index)
        pairs, failed holds the requested indexes that are gone.
    """
    resolved: list[tuple[int, int]] = []
    failed: list[int] = []

    for position, line_index in enumerate(line_indexes):
        marker = markers[position] if markers and position < len(markers) else None
        index = resolve_index(lines, line_index, marker)
        if index is None:
            failed.append(line_index)
        else:
            resolved.append((line_index, index))

    return resolved, failed


def toggle_todo(line_index: int, done: bool, marker: Optional[str] = None) -> None:
    """Toggle a todo's completion status.

    Args:
        line_index: Line index of the todo, as rendered on the page.
        done: New completion status.
        marker: Marker ID of the todo; wins over the index when present.
    """
    content = read_content()
    lines = content.splitlines()

    index = _resolve_or_raise(lines, line_index, marker)
    lines[index] = rewrite_line(lines[index], done)
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


def _separator_index(lines: list[str]) -> int:
    """Where new todos go: just above the ``---`` separator, else at the end."""
    for i, line in enumerate(lines):
        if line.strip() == "---":
            return i
    return len(lines)


def insert_line(line: str) -> None:
    """Insert a line at the appropriate position.

    Args:
        line: Line to insert.
    """
    content = read_content()
    lines = content.splitlines()

    lines.insert(_separator_index(lines), line)
    write_content('\n'.join(lines) + '\n')


def delete_todo(line_index: int, marker: Optional[str] = None) -> bool:
    """Delete a todo by line index.

    Args:
        line_index: Line index to delete, as rendered on the page.
        marker: Marker ID of the todo; wins over the index when present.

    Returns:
        True if deleted, False if index out of range.
    """
    content = read_content()
    lines = content.splitlines()

    index = _resolve_or_raise(lines, line_index, marker)
    lines.pop(index)
    write_content('\n'.join(lines) + '\n')
    return True


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

    # Handle projects - can be a '+'-delimited string or a list
    if 'projects' in updates:
        projects_input = updates.get('projects')
        if isinstance(projects_input, str):
            projects = split_tag_input(projects_input, '+')
        elif isinstance(projects_input, list):
            projects = [p.strip().lstrip('+').strip() for p in projects_input if p and p.strip().lstrip('+').strip()]
        else:
            projects = existing.projects
    else:
        projects = existing.projects

    # Handle contexts - can be a '@'-delimited string or a list
    if 'contexts' in updates:
        contexts_input = updates.get('contexts')
        if isinstance(contexts_input, str):
            contexts = split_tag_input(contexts_input, '@')
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
        new_line += f" {render_tagged('+', project)}"
    for context in contexts:
        new_line += f" {render_tagged('@', context)}"
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


def handle_toggle_with_recurrence(line_index: int,
                                  marker: Optional[str] = None) -> None:
    """Handle toggling a todo that has recurrence.

    One read-modify-write for the whole operation. Splitting it up used to mean
    re-reading the file between the completion and the spawned occurrence, which
    both re-opened the window for a foreign writer and reset the ETag guard to
    the innermost read — so the guard proved nothing about the state the user
    actually clicked on.

    Args:
        line_index: Line index of the todo, as rendered on the page.
        marker: Marker ID of the todo; wins over the index when present.
    """
    content = read_content()
    lines = content.splitlines()

    index = _resolve_or_raise(lines, line_index, marker)

    line = lines[index]
    is_done = "- [x]" in line or "- [X]" in line
    item = parse_line(line, index)

    if not item:
        raise StaleReferenceError(marker or "")

    now = datetime.now()

    # Handle overdue recurring tasks
    if not is_done and item.recurrence and item.due and item.due < now:
        base_time = item.due.time() if item.due else DEFAULT_DUE_TIME
        due_dt = datetime.combine(now.date(), base_time)
        due_str = format_due(due_dt)
        line = re.sub(r'due:\d{4}-\d{2}-\d{2}(?:T\d{2}:\d{2})?', f'due:{due_str}', line)
        lines[index] = rewrite_line(line, True)
    else:
        lines[index] = rewrite_line(line, not is_done)

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
            # Fresh marker: the completed occurrence keeps the old one.
            new_line += f" ^{generate_marker()}"
            lines.insert(_separator_index(lines), new_line)

    write_content('\n'.join(lines) + '\n')


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


def postpone_todo(line_index: int, target: str,
                  marker: Optional[str] = None) -> bool:
    """Postpone a todo to a new date.

    Args:
        line_index: Line index of the todo, as rendered on the page.
        target: Postpone target ('today', 'tomorrow', 'weekend', 'sometime').
        marker: Marker ID of the todo; wins over the index when present.

    Returns:
        True if updated, False if not found.
    """
    from app.services.date_service import calculate_postpone_date

    content = read_content()
    lines = content.splitlines()

    index = _resolve_or_raise(lines, line_index, marker)

    line = lines[index]
    item = parse_line(line, index)
    if not item:
        return False

    new_datetime, new_time = calculate_postpone_date(target, item.due)

    lines[index] = _render_todo_line(item, lines[index], due=new_datetime)
    write_content('\n'.join(lines) + '\n')
    return True


def postpone_todos_batch(line_indexes: list[int], target: str,
                         markers: Optional[list[str]] = None) -> dict:
    """Postpone multiple todos to a new date in a single operation.

    This avoids WebDAV locking issues by doing a single read-modify-write cycle.

    Args:
        line_indexes: List of line indexes to postpone, as rendered on the page.
        target: Postpone target ('today', 'tomorrow', 'weekend', 'sometime').
        markers: Marker IDs positionally matching line_indexes; each wins over
            its index when present.

    Returns:
        Dict with 'updated' count and 'failed' list of indexes that couldn't be updated.
    """
    from app.services.date_service import calculate_postpone_date

    content = read_content()
    lines = content.splitlines()

    updated = 0
    resolved, failed = _resolve_batch(lines, line_indexes, markers)

    for line_index, index in resolved:
        line = lines[index]
        item = parse_line(line, index)
        if not item:
            failed.append(line_index)
            continue

        new_datetime, _ = calculate_postpone_date(target, item.due)
        lines[index] = _render_todo_line(item, lines[index], due=new_datetime)
        updated += 1

    if updated > 0:
        write_content('\n'.join(lines) + '\n')

    return {'updated': updated, 'failed': failed}


def toggle_todos_batch(line_indexes: list[int], done: bool,
                       markers: Optional[list[str]] = None) -> dict:
    """Toggle multiple todos in a single read-modify-write cycle.

    Completing a recurring task mirrors handle_toggle_with_recurrence: an
    overdue occurrence is rescheduled to today before completion, and the
    next occurrence is appended within the same write.

    Args:
        line_indexes: List of line indexes to toggle, as rendered on the page.
        done: New completion status for all items.
        markers: Marker IDs positionally matching line_indexes; each wins over
            its index when present.

    Returns:
        Dict with 'updated' count and 'failed' list of indexes.
    """
    content = read_content()
    lines = content.splitlines()

    updated = 0
    spawned: list[str] = []
    now = datetime.now()

    resolved, failed = _resolve_batch(lines, line_indexes, markers)

    for line_index, index in resolved:
        line = lines[index]
        item = parse_line(line, index)
        if not item:
            failed.append(line_index)
            continue

        if done and item.recurrence and not item.done:
            # Reschedule overdue recurring tasks to today before completing.
            if item.due and item.due < now:
                due_dt = datetime.combine(now.date(), item.due.time())
                line = re.sub(
                    r'due:\d{4}-\d{2}-\d{2}(?:T\d{2}:\d{2})?',
                    f'due:{format_due(due_dt)}', line,
                )
            next_due = next_due_date(item.due, item.recurrence)
            if next_due:
                next_item = parse_line(line, index)
                if next_item:
                    next_item.done = False
                    next_item.marker = generate_marker()
                    next_item.myday = None
                    spawned.append(_render_todo_line(next_item, "", due=next_due))

        lines[index] = rewrite_line(line, done)
        updated += 1

    # Append next occurrences only after all index-based edits, so the
    # insertions cannot shift lines the indexes still refer to.
    if spawned:
        insert_index = _separator_index(lines)
        for offset, new_line in enumerate(spawned):
            lines.insert(insert_index + offset, new_line)

    if updated > 0:
        write_content('\n'.join(lines) + '\n')

    return {'updated': updated, 'failed': failed}


def delete_todos_batch(line_indexes: list[int],
                       markers: Optional[list[str]] = None) -> dict:
    """Delete multiple todos in a single read-modify-write cycle.

    Lines are removed bottom-up so earlier removals cannot shift lines
    that later removals still refer to.

    Args:
        line_indexes: List of line indexes to delete, as rendered on the page.
        markers: Marker IDs positionally matching line_indexes; each wins over
            its index when present.

    Returns:
        Dict with 'updated' count and 'failed' list of indexes.
    """
    content = read_content()
    lines = content.splitlines()

    resolved, failed = _resolve_batch(lines, line_indexes, markers)
    valid = sorted({index for _, index in resolved}, reverse=True)

    for index in valid:
        del lines[index]

    if valid:
        write_content('\n'.join(lines) + '\n')

    return {'updated': len(valid), 'failed': failed}


def duplicate_todo(marker: str) -> Optional[dict]:
    """Duplicate the todo identified by marker as a new open task.

    The copy keeps projects, contexts, note, recurrence and reference, gets
    a fresh marker and the default due date (today, like add_todo), is not
    done and not planned for "my day".

    Args:
        marker: Marker ID of the todo to copy.

    Returns:
        Dict with 'marker' and 'line_index' of the copy, or None if the
        marker was not found or the line is not a todo.
    """
    content = read_content()
    lines = content.splitlines()

    source_index = find_line_by_marker(lines, marker)
    if source_index is None:
        return None

    item = parse_line(lines[source_index], source_index)
    if not item:
        return None

    new_marker = generate_marker()
    copy = replace(item, marker=new_marker, done=False, myday=None)
    default_due = datetime.combine(datetime.now().date(), DEFAULT_DUE_TIME)
    new_line = _render_todo_line(copy, "", due=default_due)

    insert_index = len(lines)
    for i, line in enumerate(lines):
        if line.strip() == "---":
            insert_index = i
            break
    lines.insert(insert_index, new_line)

    write_content('\n'.join(lines) + '\n')
    return {'marker': new_marker, 'line_index': insert_index}


def assign_todos_batch(line_indexes: list[int], project: Optional[str] = None,
                       context: Optional[str] = None,
                       markers: Optional[list[str]] = None) -> dict:
    """Add a project and/or context to multiple todos in a single
    read-modify-write cycle. Items already carrying the token are left
    untouched and do not count as updated.

    Args:
        line_indexes: List of line indexes to update, as rendered on the page.
        project: Project name to add (without '+' prefix).
        context: Context name to add (without '@' prefix).
        markers: Marker IDs positionally matching line_indexes; each wins over
            its index when present.

    Returns:
        Dict with 'updated' count and 'failed' list of indexes.
    """
    project_clean = normalize_prefix(project, '+') if project else None
    context_clean = normalize_prefix(context, '@') if context else None
    if not project_clean and not context_clean:
        return {'updated': 0, 'failed': list(line_indexes)}

    content = read_content()
    lines = content.splitlines()

    updated = 0
    resolved, failed = _resolve_batch(lines, line_indexes, markers)

    for line_index, index in resolved:
        item = parse_line(lines[index], index)
        if not item:
            failed.append(line_index)
            continue

        changed = False
        if project_clean:
            existing_projects = {normalize_prefix(p, '+') for p in item.projects}
            if project_clean not in existing_projects:
                item.projects.append(project_clean)
                changed = True
        if context_clean:
            existing_contexts = {normalize_prefix(c, '@') for c in item.contexts}
            if context_clean not in existing_contexts:
                item.contexts.append(context_clean)
                changed = True

        if changed:
            lines[index] = _render_todo_line(item, lines[index])
            updated += 1

    if updated > 0:
        write_content('\n'.join(lines) + '\n')

    return {'updated': updated, 'failed': failed}
