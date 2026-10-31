"""Convert between Markdown todo lines and iCalendar VTODO objects.

One VTODO per Markdown line. The caller chooses the UID (normally the
line's ``^marker``, or the UID an iOS device picked for a task it created).
iOS edits only a subset of the fields; the rest is preserved from the
existing line during a merge.

Times: due dates in the Markdown file are naive local time (the server's
``TZ``). ``datetime.astimezone()`` on a naive value interprets it as system
local time, DST included, which is exactly that.
"""

from __future__ import annotations

from datetime import date as dt_date
from datetime import datetime, timezone
from typing import Any

from icalendar import Calendar, Todo  # type: ignore[import-untyped]

from app.models.todo import DEFAULT_DUE_TIME, TodoItem

#: How long gravestones survive after a DELETE (seconds).
GRAVESTONE_TTL = 600

_DTSTAMP = datetime(2026, 1, 1, tzinfo=timezone.utc)

_RECUR_TO_FREQ = {"daily": "DAILY", "weekly": "WEEKLY", "monthly": "MONTHLY"}
_FREQ_TO_RECUR = {v: k for k, v in _RECUR_TO_FREQ.items()}


def _summary_tags(item: TodoItem, list_project: str | None) -> list[str]:
    """Tags shown after the title on iOS.

    iOS does not show CATEGORIES for reminders, so places and any further
    projects go into SUMMARY. The project the list is named after is left
    out — the list already says it.
    """
    tags = [f"+{p}" for p in item.projects if p != list_project]
    tags += [f"@{c}" for c in item.contexts]
    return tags


def item_to_vtodo(item: TodoItem, uid: str, list_project: str | None = None) -> Todo:
    """Convert a Markdown todo item to an iCalendar VTODO."""
    vtodo = Todo()
    vtodo.add("uid", uid)
    # Required by RFC 5545 (iOS drops a VTODO without it). Fixed, so the ETag
    # only changes when the task does.
    vtodo.add("dtstamp", _DTSTAMP)

    title = item.title.strip() or "(no title)"
    tags = _summary_tags(item, list_project)
    if tags:
        title = title + " " + " ".join(tags)
    vtodo.add("summary", title)

    if item.note:
        vtodo.add("description", item.note)

    if item.due and not item.due_is_sometime:
        _add_due(vtodo, item.due)

    if item.done:
        vtodo.add("status", "COMPLETED")
        if item.done_date:
            vtodo.add("completed", _to_utc(item.done_date))
    else:
        vtodo.add("status", "NEEDS-ACTION")

    freq = _RECUR_TO_FREQ.get((item.recurrence or "").lower())
    if freq:
        vtodo.add("rrule", {"freq": [freq]})

    if item.projects or item.contexts:
        vtodo.add("categories", item.projects + item.contexts)

    return vtodo


def item_to_ics(item: TodoItem, uid: str, list_project: str | None = None) -> bytes:
    """Serialize a todo item as a full iCalendar object (single VTODO)."""
    cal = Calendar()
    cal.add("version", "2.0")
    cal.add("prodid", "-//Reinschrift//CalDAV//EN")
    cal.add_component(item_to_vtodo(item, uid, list_project))
    ics: bytes = cal.to_ical()
    return ics


def _add_due(vtodo: Todo, due_dt: datetime) -> None:
    """Add a DUE property; all-day (time 00:00) becomes VALUE=DATE."""
    if due_dt.time() == DEFAULT_DUE_TIME:
        vtodo.add("due", due_dt.date())
    else:
        vtodo.add("due", _to_utc(due_dt))


def _to_utc(value: datetime) -> datetime:
    """Naive values are local time; aware values keep their zone."""
    return value.astimezone(timezone.utc)


def _to_local_naive(value: datetime) -> datetime:
    """An aware value converted to local time; floating values stay as-is."""
    if value.tzinfo is None:
        return value
    return value.astimezone().replace(tzinfo=None)


def clean_title(text: str) -> str:
    """One line of text: iOS titles must not break the Markdown file."""
    return " ".join(text.split())


def strip_summary_tags(summary: str, item: TodoItem) -> str:
    """Remove the tags :func:`item_to_vtodo` appended to the title."""
    cleaned = summary
    for p in item.projects:
        cleaned = cleaned.replace(f"+{p}", "", 1)
    for c in item.contexts:
        cleaned = cleaned.replace(f"@{c}", "", 1)
    return clean_title(cleaned)


def parse_due(vtodo: Todo) -> datetime | None:
    """DUE as naive local time; an all-day date becomes 00:00."""
    due = vtodo.get("due")
    if due is None:
        return None
    value = due.dt
    if isinstance(value, datetime):
        return _to_local_naive(value)
    if isinstance(value, dt_date):
        return datetime.combine(value, DEFAULT_DUE_TIME)
    return None


def parse_recurrence(vtodo: Todo) -> str | None:
    rrule = vtodo.get("rrule")
    if rrule is None:
        return None
    freq_list = rrule.get("FREQ", [])
    if not freq_list:
        return None
    return _FREQ_TO_RECUR.get(str(freq_list[0]).upper())


def parse_note(vtodo: Todo) -> str | None:
    description = vtodo.get("description")
    return str(description) if description else None


def is_completed(vtodo: Todo) -> bool:
    return str(vtodo.get("status", "NEEDS-ACTION")).upper() == "COMPLETED"


def vtodo_to_updates(vtodo: Todo, existing_item: TodoItem) -> dict[str, Any]:
    """Fields of an iOS VTODO that differ from the Markdown line.

    Only fields iOS actually edits are returned (``title``, ``due``,
    ``recurrence``, ``note``, ``done``). Projects, places, "my day",
    references etc. stay as they are in the line.
    """
    updates: dict[str, Any] = {}

    title = strip_summary_tags(str(vtodo.get("summary", "")), existing_item)
    if title and title != existing_item.title:
        updates["title"] = title

    # A "sometime" date is not exported, so iOS sends none back — keep it.
    if not existing_item.due_is_sometime or vtodo.get("due") is not None:
        new_due = parse_due(vtodo)
        if new_due != existing_item.due:
            updates["due"] = new_due

    new_recur = parse_recurrence(vtodo)
    if new_recur != existing_item.recurrence:
        # An unknown RRULE (e.g. yearly) maps to None; keep the line's own rule.
        if new_recur is not None or vtodo.get("rrule") is None:
            updates["recurrence"] = new_recur

    new_note = parse_note(vtodo)
    if new_note != existing_item.note:
        updates["note"] = new_note

    done = is_completed(vtodo)
    if done != existing_item.done:
        updates["done"] = done

    return updates


def parse_vtodo_from_ics(ics_text: bytes) -> Todo | None:
    """Parse the first VTODO from raw iCalendar bytes, or None."""
    try:
        cal = Calendar.from_ical(ics_text)
    except Exception:
        return None
    for component in cal.walk("VTODO"):
        found: Todo = component
        return found
    return None


def vtodo_uid(vtodo: Todo) -> str:
    """Extract the UID from a VTODO."""
    return str(vtodo.get("uid", ""))
