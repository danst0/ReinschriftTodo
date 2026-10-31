"""CalDAV service — project→list mapping, ETag/CTag, UID/Marker store.

Manages the mapping between Markdown ``^marker`` IDs and iCalendar UIDs,
including gravestones for recently-deleted items (survive a few minutes so
that iOS list moves that DELETE-then-PUT do not lose data).
"""

from __future__ import annotations

import fcntl
import hashlib
import json
import logging
import os
import time
from datetime import datetime, timedelta
from typing import Any

from app.models.todo import TodoItem, DEFAULT_DUE_TIME, SOMETIME_SENTINEL
from app.services.ical_service import (
    GRAVESTONE_TTL,
    Todo,
    item_to_ics,
    item_to_vtodo,
    parse_vtodo_from_ics,
    vtodo_to_updates,
    vtodo_uid,
)
from app.services.parser import find_line_by_marker, marker_of, parse_line
from app.services.storage import read_content, write_content
from app.services.todo_service import (
    _separator_index,
    _render_todo_line,
    handle_toggle_with_recurrence,
)
from app.services.date_service import next_due_date
from app.utils.helpers import format_due
from app.utils.markers import generate_marker, ensure_marker

logger = logging.getLogger(__name__)

INBOX = "Inbox"


def _caldav_path() -> str:
    """Path to caldav.json, next to CONFIG_PATH on the /config volume."""
    config_dir = os.environ.get(
        "CONFIG_PATH", "/config/settings.json"
    )
    return os.path.join(os.path.dirname(config_dir) or ".", "caldav.json")


def _load_state() -> dict[str, Any]:
    """Load caldav.json state with advisory locking."""
    path = _caldav_path()
    try:
        with open(path, "r") as fh:
            fcntl.flock(fh, fcntl.LOCK_SH)
            raw = fh.read()
    except FileNotFoundError:
        return {}
    if not raw.strip():
        return {}
    return json.loads(raw)


def _save_state(state: dict[str, Any]) -> None:
    path = _caldav_path()
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    tmp = path + ".tmp"
    with open(tmp, "w") as fh:
        fcntl.flock(fh, fcntl.LOCK_EX)
        json.dump(state, fh, indent=2)
        fh.flush()
        os.fsync(fh.fileno())
    os.replace(tmp, path)


def _uid_for_marker(marker: str) -> str:
    return marker


def _marker_for_uid(uid: str) -> str:
    state = _load_state()
    mapping = state.get("uid_to_marker", {})
    return mapping.get(uid, uid)


def _marker_for_href(href: str) -> str:
    raw = href.rstrip("/").rsplit("/", 1)[-1]
    if raw.endswith(".ics"):
        raw = raw[:-4]
    state = _load_state()
    mapping = state.get("href_to_marker", {})
    return mapping.get(raw, raw)


def _remember_uid_mapping(uid: str, href: str, marker: str) -> None:
    state = _load_state()
    state.setdefault("uid_to_marker", {})[uid] = marker
    state.setdefault("href_to_marker", {})[href] = marker
    _save_state(state)


def _forget_uid_mapping(uid: str) -> None:
    state = _load_state()
    state.get("uid_to_marker", {}).pop(uid, None)
    _save_state(state)


def _add_gravestone(uid: str, marker: str, line: str) -> None:
    state = _load_state()
    state.setdefault("gravestones", {})[uid] = {
        "marker": marker,
        "line": line,
        "deleted_at": time.time(),
    }
    _save_state(state)


def _check_gravestone(uid: str) -> dict[str, Any] | None:
    state = _load_state()
    gs = state.get("gravestones", {})
    entry = gs.get(uid)
    if entry is None:
        return None
    if time.time() - entry["deleted_at"] > GRAVESTONE_TTL:
        gs.pop(uid, None)
        _save_state(state)
        return None
    return entry


def _remove_gravestone(uid: str) -> None:
    state = _load_state()
    state.get("gravestones", {}).pop(uid, None)
    _save_state(state)


def _purge_expired_gravestones() -> int:
    state = _load_state()
    gs = state.get("gravestones", {})
    cutoff = time.time() - GRAVESTONE_TTL
    expired = [uid for uid, entry in gs.items() if entry["deleted_at"] < cutoff]
    for uid in expired:
        gs.pop(uid, None)
    if expired:
        _save_state(state)
    return len(expired)


def _list_for_project(project: str) -> str:
    return project or INBOX


def _etag_for_ics(ics_bytes: bytes) -> str:
    return '"' + hashlib.sha256(ics_bytes).hexdigest()[:16] + '"'


def _ctag_for_lines(lines: list[str]) -> str:
    """CTag is a hash of all relevant lines — changing one changes the CTag."""
    h = hashlib.sha256()
    for line in lines:
        h.update(line.encode("utf-8"))
    return hashlib.sha256(h.digest()).hexdigest()[:16]


def _render_ics(item: TodoItem, list_name: str, tag_titles: bool = True) -> bytes:
    return item_to_ics(item, list_name, tag_titles=tag_titles)


class CalendarInfo:
    """Lightweight holder for calendar metadata."""

    def __init__(self, name: str, display_name: str, color: str = "#007AFF"):
        self.name = name
        self.display_name = display_name
        self.color = color


def list_calendars() -> list[CalendarInfo]:
    """Return the list of calendars based on projects in the file."""
    content = read_content()
    if not content:
        return [CalendarInfo(INBOX, INBOX)]

    projects: set[str] = set()
    for line in content.splitlines():
        item = parse_line(line, 0)
        if item and not item.done:
            if item.projects:
                for p in item.projects:
                    projects.add(p)
            else:
                projects.add("")

    if "" in projects:
        projects.remove("")

    result: list[CalendarInfo] = []
    has_inbox = any(
        not parse_line(line, 0).projects
        for line in content.splitlines()
        if parse_line(line, 0) and not parse_line(line, 0).done  # type: ignore[union-attr]
    )
    if has_inbox:
        result.append(CalendarInfo(INBOX, INBOX))
    for project in sorted(projects):
        result.append(CalendarInfo(project, project))
    return result


def calendar_items(list_name: str, include_done: bool = False) -> list[tuple[TodoItem, bytes, str]]:
    """Load all items for a calendar, return (item, ics_bytes, etag)."""
    content = read_content()
    if not content:
        return []

    items: list[tuple[TodoItem, bytes, str]] = []
    now = datetime.now()
    cutoff = now - timedelta(days=30)

    for line_idx, line in enumerate(content.splitlines()):
        item = parse_line(line, line_idx)
        if item is None:
            continue

        if item.done and not include_done:
            if item.done_date and item.done_date < cutoff:
                continue

        proj = item.projects[0] if item.projects else ""
        if _list_for_project(proj) != list_name:
            continue

        ics = _render_ics(item, list_name)
        etag = _etag_for_ics(ics)
        items.append((item, ics, etag))

    return items


def calendar_ctag(list_name: str) -> str:
    """CTag for a calendar — changes whenever any item in the list changes."""
    items = calendar_items(list_name, include_done=True)
    etags = [etag for _, _, etag in items]
    h = hashlib.sha256()
    for etag in sorted(etags):
        h.update(etag.encode("utf-8"))
    return h.hexdigest()[:16]


def get_item_ics(list_name: str, uid_or_href: str) -> bytes | None:
    """Return the iCalendar data for a specific item, or None."""
    marker = _marker_for_href(uid_or_href) if "/" in uid_or_href else _marker_for_uid(uid_or_href)
    content = read_content()
    if not content:
        return None

    for line_idx, line in enumerate(content.splitlines()):
        item = parse_line(line, line_idx)
        if item is None:
            continue
        if item.marker == marker:
            return _render_ics(item, list_name)
    return None


def get_item_etag(list_name: str, uid_or_href: str) -> str | None:
    ics = get_item_ics(list_name, uid_or_href)
    if ics is None:
        return None
    return _etag_for_ics(ics)


def put_item(list_name: str, uid_or_href: str, ics_bytes: bytes,
             if_match: str | None = None) -> tuple[int, str | None]:
    """Process a PUT from iOS — merge VTODO with existing line.

    Returns (http_status, etag_or_error).

    Handles:
    - New items (from URL with no matching marker) → insert a fresh line.
    - Recurring task completion → toggle & spawn next occurrence.
    - Existing item update → merge fields iOS controls, preserve the rest.
    - Gravestone resurrection (DELETE then PUT within TTL) → restore line.
    """
    vtodo = parse_vtodo_from_ics(ics_bytes)
    if vtodo is None:
        return 400, None

    uid = vtodo_uid(vtodo)
    content = read_content()
    lines = content.splitlines() if content else []

    marker = _marker_for_uid(uid)
    existing_idx = find_line_by_marker(lines, marker)

    # Gravestone resurrection
    if existing_idx is None:
        gs = _check_gravestone(uid)
        if gs is not None:
            lines.insert(_separator_index(lines), gs["line"])
            _remove_gravestone(uid)
            content = "\n".join(lines) + "\n"
            write_content(content)
            lines = content.splitlines()
            existing_idx = find_line_by_marker(lines, marker)

    existing = None
    if existing_idx is not None:
        existing = parse_line(lines[existing_idx], existing_idx)

    if existing is None and existing_idx is None:
        # New item from iOS — create a Marker for it
        ics_ref = uid_or_href.rstrip("/").rsplit("/", 1)[-1]
        if ics_ref.endswith(".ics"):
            ics_ref = ics_ref[:-4]

        # Use the caldav href as a fallback identifier; generate a proper marker
        new_marker = generate_marker()
        _remember_uid_mapping(uid, ics_ref, new_marker)
        marker = new_marker

        title = str(vtodo.get("summary", "") or "(New task)")
        new_line = "- [ ] " + title

        proj = list_name if list_name != INBOX else ""
        if proj:
            new_line += f" +{proj}"

        due_dt = None
        due_raw = vtodo.get("due")
        if due_raw is not None:
            from app.services.ical_service import _parse_due_from_vtodo
            due_dt = _parse_due_from_vtodo(vtodo)
            if due_dt:
                new_line += f" due:{format_due(due_dt)}"

        recur_str = None
        rrule = vtodo.get("rrule")
        if rrule is not None and rrule.get("FREQ"):
            from app.services.ical_service import _rrule_freq_to_recur
            recur_str = _rrule_freq_to_recur(str(rrule["FREQ"][0]))
            if recur_str:
                new_line += f" rec:{recur_str}"

        note = str(vtodo.get("description", "")) if vtodo.get("description") else None
        if note:
            from app.utils.escaping import escape_note
            new_line += f' ~note:"{escape_note(note)}"'

        new_line += f" ^{marker}"
        lines.insert(_separator_index(lines), new_line)
        write_content("\n".join(lines) + "\n")
        etag = _etag_for_ics(_render_ics(
            TodoItem(line_index=0, marker=marker, title=title, projects=[proj] if proj else [],
                     due=due_dt, recurrence=recur_str, note=note, raw_line=new_line),
            list_name))
        return 201, etag

    if existing is None:
        return 404, None

    content = read_content()
    lines = content.splitlines()
    existing_idx = find_line_by_marker(lines, marker)
    if existing_idx is None:
        return 404, None
    existing = parse_line(lines[existing_idx], existing_idx)
    if existing is None:
        return 404, None

    updates = vtodo_to_updates(vtodo, existing)
    recurrence_done = updates.pop("recurrence_done", False)

    if recurrence_done:
        handle_toggle_with_recurrence(existing_idx, marker)
    else:
        merged = _apply_updates(existing, updates)
        new_line = _render_todo_line(merged, existing.raw_line)
        lines[existing_idx] = new_line
        write_content("\n".join(lines) + "\n")

    etag = get_item_etag(list_name, uid)
    return 200, etag


def _apply_updates(item: TodoItem, updates: dict[str, Any]) -> TodoItem:
    """Merge iOS-provided fields into a TodoItem, preserving the rest."""
    from copy import deepcopy
    merged = deepcopy(item)

    if "title" in updates and updates["title"]:
        merged.title = updates["title"]
    if "due" in updates:
        merged.due = updates["due"]
    if "done" in updates:
        merged.done = updates["done"]
    if "done_date" in updates:
        merged.done_date = updates["done_date"]
    if "recurrence" in updates:
        merged.recurrence = updates["recurrence"]
    if "note" in updates:
        merged.note = updates["note"]

    if merged.done:
        merged.due = None
        merged.recurrence = None

    return merged


def delete_item(list_name: str, uid_or_href: str) -> bool:
    """Delete a todo line and leave a gravestone.

    Returns True if the item existed and was deleted.
    """
    marker = _marker_for_href(uid_or_href) if "/" in uid_or_href else _marker_for_uid(uid_or_href)
    content = read_content()
    if not content:
        return False

    lines = content.splitlines()
    idx = find_line_by_marker(lines, marker)
    if idx is None:
        return False

    item = parse_line(lines[idx], idx)
    popped = lines.pop(idx)
    write_content("\n".join(lines) + "\n")

    if item and item.marker:
        uid = _uid_for_marker(item.marker)
        _add_gravestone(uid, item.marker, popped)
    return True


def principal_url(username: str, base_url: str) -> str:
    return f"{base_url.rstrip('/')}/caldav/principal/{username}/"


def calendar_home_url(username: str, base_url: str) -> str:
    return f"{base_url.rstrip('/')}/caldav/calendars/{username}/"


def ensure_markers() -> int:
    """Add markers to any todo lines that lack them. Returns number added."""
    content = read_content()
    if not content:
        return 0

    lines = content.splitlines()
    added = 0
    for idx, line in enumerate(lines):
        item = parse_line(line, idx)
        if item is None:
            continue
        if marker_of(line) is not None:
            continue
        new_marker = generate_marker()
        lines[idx] = line + f" ^{new_marker}"
        added += 1

    if added:
        write_content("\n".join(lines) + "\n")
    return added