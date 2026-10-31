"""Convert between Markdown todo lines and iCalendar VTODO objects.

One VTODO per Markdown line. The caller chooses the UID (normally the
line's ``^marker``, or the UID an iOS device picked for a task it created).
iOS edits only a subset of the fields; the rest is preserved from the
existing line during a merge.

Tags: iOS shows neither CATEGORIES nor real tags for CalDAV lists, so
projects and places travel in the title — ``Mow the lawn #Garden @Home``.
``#x`` and ``+x`` typed on the device both mean a project; removing a tag
from the title removes it from the line.

Alarms: the Markdown line has no field for them, so the VALARMs a device
sends (at a time or, ``X-APPLE-PROXIMITY``, at a location) are kept by the
caller (``caldav.json``) and handed back to :func:`item_to_vtodo` unchanged.

Times: due dates in the Markdown file are naive local time (the server's
``TZ``). ``datetime.astimezone()`` on a naive value interprets it as system
local time, DST included, which is exactly that.
"""

from __future__ import annotations

import logging
import re
from datetime import date as dt_date
from datetime import datetime, timedelta, timezone
from typing import Any

from icalendar import Alarm, Calendar, Todo  # type: ignore[import-untyped]

from app.models.todo import DEFAULT_DUE_TIME, TodoItem
from app.utils.escaping import escape_note, unescape_note

logger = logging.getLogger(__name__)

#: A tag in an iOS title: ``#x`` / ``+x`` (project), ``@x`` (place), each
#: also quoted (``#"Big Project"``). Only at a word start, so "C++" or an
#: e-mail address stay text; an unquoted tag must start with a letter, so
#: "#1" or a phone number "+49 …" stay text too.
_TAG_RE = re.compile(r'(?<!\S)([#+@])(?:"((?:\\.|[^"])*)"|(\S+))')

#: How long gravestones survive after a DELETE (seconds).
GRAVESTONE_TTL = 600

_DTSTAMP = datetime(2026, 1, 1, tzinfo=timezone.utc)

_RECUR_TO_FREQ = {"daily": "DAILY", "weekly": "WEEKLY", "monthly": "MONTHLY"}
_FREQ_TO_RECUR = {v: k for k, v in _RECUR_TO_FREQ.items()}


def _tag(prefix: str, name: str) -> str:
    if any(ch.isspace() or ch in '"\\' for ch in name):
        return f'{prefix}"{escape_note(name)}"'
    return prefix + name


def _summary_tags(item: TodoItem) -> list[str]:
    """Tags shown after the title on iOS: ``#project``, ``@place``."""
    tags = [_tag("#", p) for p in item.projects]
    tags += [_tag("@", c) for c in item.contexts]
    return tags


def split_summary(summary: str) -> tuple[str, list[str], list[str]]:
    """An iOS title as ``(title, projects, places)``."""
    projects: list[str] = []
    places: list[str] = []

    def take(match: re.Match[str]) -> str:
        sign, quoted, plain = match.group(1), match.group(2), match.group(3)
        name = unescape_note(quoted) if quoted is not None else plain
        if not name or (quoted is None and not name[0].isalpha()):
            return match.group(0)
        target = places if sign == "@" else projects
        if name.casefold() not in (t.casefold() for t in target):
            target.append(name)
        return ""

    title = _TAG_RE.sub(take, summary)
    return clean_title(title), projects, places


def _same_tags(a: list[str], b: list[str]) -> bool:
    return sorted(x.casefold() for x in a) == sorted(x.casefold() for x in b)


def item_to_vtodo(item: TodoItem, uid: str, alarms: list[str] | None = None) -> Todo:
    """Convert a Markdown todo item to an iCalendar VTODO."""
    vtodo = Todo()
    vtodo.add("uid", uid)
    # Required by RFC 5545 (iOS drops a VTODO without it). Fixed, so the ETag
    # only changes when the task does.
    vtodo.add("dtstamp", _DTSTAMP)

    title = item.title.strip() or "(no title)"
    tags = _summary_tags(item)
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

    for text in alarms or ():
        try:
            vtodo.add_component(Alarm.from_ical(text))
        except Exception:  # a stored alarm that no longer parses is dropped
            logger.warning("CalDAV: skipping unreadable stored alarm for %s", uid)

    return vtodo


def item_to_ics(item: TodoItem, uid: str, alarms: list[str] | None = None) -> bytes:
    """Serialize a todo item as a full iCalendar object (single VTODO)."""
    cal = Calendar()
    cal.add("version", "2.0")
    cal.add("prodid", "-//Reinschrift//CalDAV//EN")
    cal.add_component(item_to_vtodo(item, uid, alarms))
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


def alarms_of(vtodo: Todo) -> list[str]:
    """The VALARMs of a VTODO, serialized for storage."""
    return [alarm.to_ical().decode("utf-8") for alarm in vtodo.walk("VALARM")]


def carried_alarms(alarms: list[str]) -> list[str]:
    """The alarms that still fit the next occurrence of a recurring task.

    Location alarms and alarms relative to the due date move along; an
    alarm at a fixed time belonged to this occurrence only.
    """
    kept = []
    for text in alarms:
        try:
            alarm = Alarm.from_ical(text)
        except Exception:
            continue
        trigger = alarm.get("trigger")
        if alarm.get("x-apple-proximity") is not None or (
                trigger is not None and isinstance(trigger.dt, timedelta)):
            kept.append(text)
    return kept


def log_alarms(vtodo: Todo, name: str) -> None:
    """Log the alarms a device sent, so new iOS formats can be checked."""
    for alarm in vtodo.walk("VALARM"):
        logger.info("CalDAV alarm from device for %s: %s", name,
                    alarm.to_ical().decode("utf-8", "replace").replace("\r\n", " | "))


def tags_from_device(vtodo: Todo) -> tuple[str, list[str], list[str]]:
    """Title, projects and places of a VTODO as the device sent it."""
    return split_summary(str(vtodo.get("summary", "")))


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

    Only fields iOS actually edits are returned (``title``, ``projects``,
    ``contexts``, ``due``, ``recurrence``, ``note``, ``done``). "My day",
    references etc. stay as they are in the line.
    """
    updates: dict[str, Any] = {}

    title, projects, places = tags_from_device(vtodo)
    if title and title != existing_item.title:
        updates["title"] = title
    if not _same_tags(projects, existing_item.projects):
        updates["projects"] = projects
    if not _same_tags(places, existing_item.contexts):
        updates["contexts"] = places

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
