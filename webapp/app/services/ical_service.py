"""Convert between Markdown todo lines and iCalendar VTODO objects.

One VTODO per Markdown line — the marker (``^abc123``) is the stable UID.
iOS edits only a subset of the fields; the rest is preserved from the
existing line during a merge.
"""

from __future__ import annotations

import uuid
from datetime import datetime, timezone, timedelta, date as dt_date
from typing import Any
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from icalendar import Calendar, Todo  # type: ignore[import-untyped]

from app.models.todo import TodoItem, DEFAULT_DUE_TIME


def _server_tz() -> ZoneInfo:
    tz_name: str
    try:
        import os
        tz_name = os.environ.get("TZ", "UTC")
        return ZoneInfo(tz_name)
    except (ZoneInfoNotFoundError, KeyError):
        return ZoneInfo("UTC")


_SERVER_TZ = _server_tz()


def item_to_vtodo(item: TodoItem, list_name: str, tag_titles: bool = True) -> Todo:
    """Convert a Markdown todo line to an iCalendar VTODO.

    Args:
        item: Parsed todo item.
        list_name: CalDAV collection name (used in HREF).
        tag_titles: If True, prefix +project/@context to SUMMARY so they
            are visible on iOS (iOS does not show CATEGORIES for reminders).

    Returns:
        iCalendar Todo component.
    """
    vtodo = Todo()
    vtodo.add("uid", item.marker or str(uuid.uuid4()))

    title = item.title.strip() or "(no title)"
    if tag_titles:
        tags = []
        for p in item.projects:
            tags.append(f"+{p}")
        for c in item.contexts:
            tags.append(f"@{c}")
        if tags:
            title = " ".join(tags) + " " + title
    vtodo.add("summary", title)

    if item.note:
        vtodo.add("description", item.note)

    if item.due and not item.due_is_sometime:
        _add_due(vtodo, item.due)

    if item.done and item.done_date:
        vtodo.add("status", "COMPLETED")
        completed = item.done_date
        if not isinstance(completed, datetime):
            completed = datetime.combine(completed, DEFAULT_DUE_TIME)
        vtodo.add("completed", _to_utc(completed))
    elif item.done:
        vtodo.add("status", "COMPLETED")
    else:
        vtodo.add("status", "NEEDS-ACTION")

    if item.recurrence:
        freq = _recur_to_rrule_freq(item.recurrence)
        if freq:
            vtodo.add("rrule", {"freq": [freq]})

    if item.projects:
        vtodo.add("categories", item.projects)
    elif item.contexts:
        vtodo.add("categories", item.contexts)

    return vtodo


def item_to_ics(item: TodoItem, list_name: str, tag_titles: bool = True) -> bytes:
    """Serialize a todo item as a full iCalendar object (single VTODO)."""
    cal = Calendar()
    cal.add("version", "2.0")
    cal.add("prodid", "-//Reinschrift//CalDAV//EN")
    cal.add_component(item_to_vtodo(item, list_name, tag_titles=tag_titles))
    return cal.to_ical()


def _recur_to_rrule_freq(rule: str) -> str | None:
    rule_l = rule.lower()
    if rule_l == "daily":
        return "DAILY"
    elif rule_l == "weekly":
        return "WEEKLY"
    elif rule_l == "monthly":
        return "MONTHLY"
    return None


def _rrule_freq_to_recur(freq: str) -> str | None:
    freq_u = freq.upper()
    if freq_u == "DAILY":
        return "daily"
    elif freq_u == "WEEKLY":
        return "weekly"
    elif freq_u == "MONTHLY":
        return "monthly"
    return None


def _add_due(vtodo: Todo, due_dt: datetime) -> None:
    """Add a DUE property; all-day (time 00:00) becomes VALUE=DATE."""
    if due_dt.hour == 0 and due_dt.minute == 0:
        vtodo.add("due", dt_date(due_dt.year, due_dt.month, due_dt.day))
    else:
        aware = due_dt.replace(tzinfo=_SERVER_TZ)
        vtodo.add("due", _to_utc(aware))


def _to_utc(dt_value: datetime) -> datetime:
    """Convert a timezone-aware datetime to UTC for iCalendar output."""
    if dt_value.tzinfo is None:
        dt_value = dt_value.replace(tzinfo=_SERVER_TZ)
    return dt_value.astimezone(timezone.utc)


def vtodo_to_updates(vtodo: Todo, existing_item: TodoItem) -> dict[str, Any]:
    """Extract fields from an iOS VTODO that should update a Markdown line.

    Only fields that iOS actually edits are returned. The caller must merge
    these with the existing item (projects, contexts, myday, reference etc.
    are preserved from the line).

    Args:
        vtodo: Parsed iCalendar Todo from iOS.
        existing_item: The current state of the Markdown line.

    Returns:
        Dict with keys that have changed: ``title``, ``due``, ``done``,
        ``done_date``, ``recurrence``, ``note``, ``recurrence_done``.
    """
    updates: dict[str, Any] = {}

    new_title = str(vtodo.get("summary", ""))
    new_done = str(vtodo.get("status", "NEEDS-ACTION")).upper() == "COMPLETED"

    title_stripped = _strip_tags_from_summary(new_title, existing_item)
    if title_stripped != existing_item.title:
        updates["title"] = title_stripped

    # DUE: iOS sends VALUE=DATE for all-day, otherwise DATE-TIME
    new_due = _parse_due_from_vtodo(vtodo)
    if new_due != existing_item.due:
        updates["due"] = new_due

    # Recurrence
    new_recur = _parse_rrule(vtodo)
    if new_recur:
        updates["recurrence"] = new_recur
    elif existing_item.recurrence and not vtodo.get("rrule"):
        updates["recurrence"] = None

    # Note
    new_note = str(vtodo.get("description", "")) if vtodo.get("description") else None
    if new_note != existing_item.note:
        updates["note"] = new_note

    # Done status
    recurrence_done = False
    if new_done != existing_item.done:
        updates["done"] = new_done
        if new_done:
            done_date = _parse_completed(vtodo)
            updates["done_date"] = done_date
            if existing_item.recurrence and not existing_item.done:
                recurrence_done = True

    updates["recurrence_done"] = recurrence_done
    return updates


def _strip_tags_from_summary(summary: str, existing_item: TodoItem) -> str:
    """Remove +project/@context prefixes from summary when tags are in the title.

    The server prefixes tags for iOS display; on write-back, strip them
    so the title returned from iOS matches ``existing_item.title``.
    """
    cleaned = summary
    for p in existing_item.projects:
        cleaned = cleaned.replace(f"+{p}", "", 1)
    for c in existing_item.contexts:
        cleaned = cleaned.replace(f"@{c}", "", 1)
    return cleaned.strip()


def _parse_due_from_vtodo(vtodo: Todo) -> datetime | None:
    due = vtodo.get("due")
    if due is None:
        return None
    dt_val = _vtodo_dt(due.dt)
    if dt_val is None:
        return None
    # iCalendar dates are naive; treat as server local time
    if isinstance(due.dt, dt_date) and not isinstance(due.dt, datetime):
        return datetime.combine(due.dt, DEFAULT_DUE_TIME)
    if dt_val.tzinfo is None:
        server_tz = _SERVER_TZ
        dt_val = dt_val.replace(tzinfo=timezone.utc).astimezone(server_tz)
    return dt_val.replace(tzinfo=None)


def _parse_completed(vtodo: Todo) -> datetime | None:
    completed = vtodo.get("completed")
    if completed is None:
        return datetime.now()
    dt_val = _vtodo_dt(completed.dt)
    if dt_val is None:
        return datetime.now()
    if dt_val.tzinfo is None:
        return dt_val
    return dt_val.replace(tzinfo=None)


def _parse_rrule(vtodo: Todo) -> str | None:
    rrule = vtodo.get("rrule")
    if rrule is None:
        return None
    freq_list = rrule.get("FREQ", [])
    if not freq_list:
        return None
    return _rrule_freq_to_recur(str(freq_list[0]))


def _vtodo_dt(dt_val: Any) -> datetime | None:
    """Handle both date and datetime values from icalendar."""
    if isinstance(dt_val, datetime):
        return dt_val
    if isinstance(dt_val, dt_date):
        return datetime.combine(dt_val, DEFAULT_DUE_TIME)
    return None


def parse_vtodo_from_ics(ics_text: bytes) -> Todo | None:
    """Parse a VTODO from raw iCalendar bytes.

    Returns the first VTODO found, or None.
    """
    try:
        cal = Calendar.from_ical(ics_text)
    except Exception:
        return None
    for component in cal.walk("VTODO"):
        return component  # type: ignore[return-value]
    return None


def vtodo_uid(vtodo: Todo) -> str:
    """Extract the UID from a VTODO."""
    return str(vtodo.get("uid", ""))


def vtodo_status(vtodo: Todo) -> str:
    """Extract the STATUS from a VTODO."""
    return str(vtodo.get("status", "NEEDS-ACTION")).upper()


#: How long gravestones survive after a DELETE (seconds).
GRAVESTONE_TTL = 600