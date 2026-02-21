"""Parsing service for todo lines."""

import re
from datetime import datetime, time as dtime
from typing import Optional

from app.models.todo import (
    TodoItem, LINK_RE, PROJECT_RE, CONTEXT_RE, DUE_RE, ID_RE,
    COMPLETION_RE, COMPLETION_DATE_RE, RECUR_RE, NOTE_RE,
    DEFAULT_DUE_TIME, TITLE_MARKERS
)
from app.utils.escaping import unescape_note, normalize_note
from app.utils.helpers import format_due_display, is_sometime, normalize_prefix


def parse_due_token(text: str) -> Optional[datetime]:
    """Parse a due date/time from a line of text.

    Args:
        text: Line text containing a due: token.

    Returns:
        Parsed datetime or None if not found/invalid.
    """
    match = DUE_RE.search(text)
    if not match:
        return None

    date_part = match.group(1)
    time_part = match.group(2)

    try:
        date_obj = datetime.strptime(date_part, "%Y-%m-%d").date()
    except ValueError:
        return None

    if time_part:
        try:
            time_obj = datetime.strptime(time_part, "%H:%M").time()
        except ValueError:
            time_obj = DEFAULT_DUE_TIME
    else:
        time_obj = DEFAULT_DUE_TIME

    return datetime.combine(date_obj, time_obj)


def parse_due_input(raw_value: str | None) -> Optional[datetime]:
    """Parse a due date from form input.

    Args:
        raw_value: Raw input string from form field.

    Returns:
        Parsed datetime or None if invalid.
    """
    if not raw_value:
        return None
    raw_value = raw_value.strip()
    for fmt in ["%Y-%m-%dT%H:%M", "%Y-%m-%d"]:
        try:
            parsed = datetime.strptime(raw_value, fmt)
            if fmt == "%Y-%m-%d":
                parsed = datetime.combine(parsed.date(), DEFAULT_DUE_TIME)
            return parsed
        except ValueError:
            continue
    return None


def capture_token(regex: re.Pattern, text: str) -> Optional[str]:
    """Capture the first match of a regex pattern.

    Args:
        regex: Compiled regex pattern with a capture group.
        text: Text to search.

    Returns:
        Captured group or None.
    """
    match = regex.search(text)
    if match:
        return match.group(1).strip()
    return None


def capture_all_tokens(regex: re.Pattern, text: str) -> list[str]:
    """Capture all matches of a regex pattern.

    Args:
        regex: Compiled regex pattern.
        text: Text to search.

    Returns:
        List of matched tokens (without prefix characters).
    """
    return [m.group(1).strip() for m in regex.finditer(text) if m.group(1).strip()]


def strip_leading_markers(text: str) -> str:
    """Remove leading project/context markers from text.

    Args:
        text: Input text possibly starting with +project or @context.

    Returns:
        Cleaned text with leading markers removed.
    """
    cleaned = text.lstrip()
    while True:
        match = re.match(r'^([+@][^\s]+)\s+(.*)$', cleaned)
        if not match:
            break
        cleaned = match.group(2).lstrip()
    return cleaned


def extract_title(rest: str) -> str:
    """Extract the title portion from a todo line.

    Args:
        rest: Line content after the checkbox marker.

    Returns:
        Extracted title string.
    """
    cleaned_rest = strip_leading_markers(rest)
    cut = len(cleaned_rest)

    for marker in TITLE_MARKERS:
        idx = cleaned_rest.find(marker)
        if idx != -1 and idx < cut:
            cut = idx

    raw = cleaned_rest[:cut]
    cleaned = raw.strip()
    return cleaned if cleaned else cleaned_rest.strip()


def parse_line(line: str, line_index: int) -> Optional[TodoItem]:
    """Parse a markdown line into a TodoItem.

    Args:
        line: Raw markdown line.
        line_index: Zero-based line index in the file.

    Returns:
        Parsed TodoItem or None if not a valid todo line.
    """
    trimmed = line.lstrip()
    done = False
    rest = ""
    done_date = None

    if trimmed.startswith("- [x]"):
        done = True
        rest = trimmed[5:].strip()
    elif trimmed.startswith("- [X]"):
        done = True
        rest = trimmed[5:].strip()
    elif trimmed.startswith("- [ ]"):
        done = False
        rest = trimmed[5:].strip()
    else:
        return None

    title = extract_title(rest)
    rest_without_note = NOTE_RE.sub('', rest)
    projects = [normalize_prefix(p, '+') for p in capture_all_tokens(PROJECT_RE, rest_without_note)]
    projects = [p for p in projects if p]  # Filter out None/empty
    contexts = [normalize_prefix(c, '@') for c in capture_all_tokens(CONTEXT_RE, rest_without_note)]
    contexts = [c for c in contexts if c]  # Filter out None/empty
    due_dt = parse_due_token(rest)
    recurrence = capture_token(RECUR_RE, rest)
    due_display = format_due_display(due_dt) if due_dt else None
    due_is_sometime = is_sometime(due_dt)

    if done:
        date_match = COMPLETION_DATE_RE.search(line)
        if date_match:
            try:
                done_date = datetime.strptime(date_match.group(1), "%Y-%m-%d")
            except ValueError:
                done_date = None

    reference = capture_token(LINK_RE, rest)
    marker = capture_token(ID_RE, rest)
    raw_note = capture_token(NOTE_RE, rest)
    note = normalize_note(unescape_note(raw_note)) if raw_note else None

    return TodoItem(
        line_index=line_index,
        marker=marker,
        title=title,
        projects=projects,
        contexts=contexts,
        due=due_dt,
        due_display=due_display,
        due_is_sometime=due_is_sometime,
        reference=reference,
        recurrence=recurrence,
        note=note,
        done=done,
        done_date=done_date,
        raw_line=line
    )


def find_line_by_marker(lines: list[str], marker: str) -> Optional[int]:
    """Find a line index by its marker ID.

    Args:
        lines: List of file lines.
        marker: Marker ID to search for.

    Returns:
        Line index or None if not found.
    """
    needle = f"^{marker}"
    for idx, line in enumerate(lines):
        for token in line.split():
            if token.strip() == needle:
                return idx
    return None
