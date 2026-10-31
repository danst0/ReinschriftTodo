"""CalDAV service — the task list, ETag/CTag, UID/marker store.

All tasks are in one list, ``LIST``; projects and places are appended to
the title (``… #Garten @Daheim``). A task's resource name (UID and ``<name>.ics``) is its ``^marker``; tasks
created on iOS keep the UID the device chose, mapped to the marker the
server gave the line. Lines without a marker are served under a name
derived from the line itself and only get a marker once iOS edits them —
reading never writes the todo file.

State lives in ``caldav.json`` next to ``CONFIG_PATH``: the UID↔marker
mapping, the alarms a device set per marker (the line has no field for
them, and without them iOS would drop every reminder on the next sync) and
gravestones for recently deleted tasks (a DELETE + PUT of the
same task — e.g. a move to another iOS list and back — keeps the line's
other fields across that gap).

Every mutation is one read-modify-write of the todo file, guarded by the
fingerprint of that read (see ``storage.write_content``).
"""

from __future__ import annotations

import contextlib
import fcntl
import hashlib
import json
import os
import time
from collections.abc import Iterator
from dataclasses import dataclass, replace
from datetime import datetime, timedelta
from typing import Any

from app.models.todo import TodoItem
from app.services.ical_service import (
    GRAVESTONE_TTL,
    alarms_of,
    carried_alarms,
    is_completed,
    item_to_ics,
    log_alarms,
    parse_due,
    parse_note,
    parse_recurrence,
    parse_vtodo_from_ics,
    tags_from_device,
    vtodo_to_updates,
)
from app.services.parser import find_line_by_marker, parse_line
from app.services.storage import read_content, write_content
from app.services.todo_service import (
    _render_todo_line,
    _separator_index,
    rewrite_line,
    toggle_lines,
)
from app.utils.markers import generate_marker

#: The one list every task is in.
LIST = "Reinschrift"

#: Completed tasks stay visible on the device this long.
DONE_VISIBLE_DAYS = 30

_HASH_PREFIX = "h-"


# ----------------------------------------------------------------
# caldav.json
# ----------------------------------------------------------------
def _caldav_path() -> str:
    config_path = os.environ.get("CONFIG_PATH", "/config/settings.json")
    return os.path.join(os.path.dirname(config_path) or ".", "caldav.json")


def _empty_state() -> dict[str, Any]:
    return {"uid_to_marker": {}, "alarms": {}, "gravestones": {}}


def _read_state() -> dict[str, Any]:
    try:
        with open(_caldav_path(), "r", encoding="utf-8") as fh:
            raw = fh.read()
    except FileNotFoundError:
        return _empty_state()
    state = _empty_state()
    if raw.strip():
        loaded = json.loads(raw)
        if isinstance(loaded, dict):
            state.update(loaded)
    return state


@contextlib.contextmanager
def _locked_state() -> Iterator[dict[str, Any]]:
    """Read-modify-write of caldav.json under an exclusive lock.

    The state is written back (atomically) when the block exits normally.
    """
    path = _caldav_path()
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    with open(path + ".lock", "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        state = _read_state()
        yield state
        tmp = path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as fh:
            json.dump(state, fh, indent=2)
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(tmp, path)


def _purge_gravestones(state: dict[str, Any]) -> None:
    cutoff = time.time() - GRAVESTONE_TTL
    graves: dict[str, Any] = state["gravestones"]
    for name in [n for n, e in graves.items() if e.get("deleted_at", 0) < cutoff]:
        graves.pop(name, None)


# ----------------------------------------------------------------
# Snapshot of the todo file
# ----------------------------------------------------------------
@dataclass
class Resource:
    """One task as a CalDAV resource."""

    name: str
    index: int
    item: TodoItem
    list_name: str
    ics: bytes
    etag: str


def _line_hash_name(line: str) -> str:
    return _HASH_PREFIX + hashlib.sha256(line.strip().encode("utf-8")).hexdigest()[:16]




def _etag_for_ics(ics: bytes) -> str:
    return '"' + hashlib.sha256(ics).hexdigest()[:16] + '"'


def _visible(item: TodoItem, now: datetime) -> bool:
    if not item.done:
        return True
    return item.done_date is not None and item.done_date >= now - timedelta(days=DONE_VISIBLE_DAYS)


class Snapshot:
    """The todo file as read once for this request."""

    def __init__(self, content: str | None = None, state: dict[str, Any] | None = None):
        self.lines: list[str] = (read_content() if content is None else content).splitlines()
        self.state = _read_state() if state is None else state
        uid_to_marker: dict[str, str] = self.state["uid_to_marker"]
        self.uid_to_marker = uid_to_marker
        self.marker_to_uid = {m: u for u, m in uid_to_marker.items()}
        self.alarms: dict[str, list[str]] = self.state.setdefault("alarms", {})

    def name_of(self, item: TodoItem, line: str) -> str:
        if item.marker:
            return self.marker_to_uid.get(item.marker, item.marker)
        return _line_hash_name(line)

    def index_of(self, name: str) -> int | None:
        if name.startswith(_HASH_PREFIX) and name not in self.uid_to_marker:
            for idx, line in enumerate(self.lines):
                if _line_hash_name(line) == name and parse_line(line, idx) is not None:
                    return idx
            return None
        return find_line_by_marker(self.lines, self.uid_to_marker.get(name, name))

    def resource_at(self, idx: int) -> Resource | None:
        item = parse_line(self.lines[idx], idx)
        if item is None:
            return None
        name = self.name_of(item, self.lines[idx])
        ics = item_to_ics(item, name, alarms=self.alarms.get(item.marker or ""))
        return Resource(name, idx, item, LIST, ics, _etag_for_ics(ics))

    def resources(self, list_name: str | None = None) -> list[Resource]:
        now = datetime.now()
        result = []
        for idx in range(len(self.lines)):
            res = self.resource_at(idx)
            if res is None or not _visible(res.item, now):
                continue
            if list_name is None or res.list_name == list_name:
                result.append(res)
        return result

    def get(self, list_name: str, name: str) -> Resource | None:
        idx = self.index_of(name)
        if idx is None:
            return None
        res = self.resource_at(idx)
        if res is None or res.list_name != list_name:
            return None
        return res


# ----------------------------------------------------------------
# Read API
# ----------------------------------------------------------------
def list_calendars(snapshot: Snapshot | None = None) -> list[str]:
    """The lists offered to the device: just the one."""
    return [LIST]


def calendar_ctag(list_name: str, snapshot: Snapshot | None = None) -> str:
    """Changes whenever a task in the list (or the set of tasks) changes."""
    snap = snapshot or Snapshot()
    h = hashlib.sha256()
    for res in sorted(snap.resources(list_name), key=lambda r: r.name):
        h.update(f"{res.name}{res.etag}".encode("utf-8"))
    return h.hexdigest()[:16]


def get_resource(list_name: str, name: str) -> Resource | None:
    return Snapshot().get(list_name, name)


# ----------------------------------------------------------------
# Write API
# ----------------------------------------------------------------
def _write_lines(lines: list[str]) -> None:
    write_content("\n".join(lines) + "\n")


def put_item(list_name: str, name: str, ics_bytes: bytes,
             if_match: str | None = None,
             if_none_match: str | None = None) -> tuple[int, str | None]:
    """Create or update a task from an iOS PUT.

    Returns ``(http_status, etag_or_message)``.
    """
    vtodo = parse_vtodo_from_ics(ics_bytes)
    if vtodo is None:
        return 400, "No VTODO in request body"
    if list_name != LIST:
        return 404, "Unknown list"

    log_alarms(vtodo, name)

    with _locked_state() as state:
        _purge_gravestones(state)
        snap = Snapshot(state=state)
        lines = snap.lines
        idx = snap.index_of(name)

        # A task moved between lists arrives as DELETE + PUT: bring the line back.
        grave = state["gravestones"].get(name) if idx is None else None
        if grave is not None:
            lines.insert(_separator_index(lines), grave["line"])
            idx = snap.index_of(name)

        current = snap.resource_at(idx) if idx is not None else None

        if if_none_match == "*" and current is not None:
            return 412, "Resource exists"
        if if_match and if_match != "*" and (current is None or current.etag != if_match):
            return 412, "ETag mismatch"

        before = _markers(lines)
        if current is None:
            marker = _insert_new(lines, list_name, name, vtodo, state)
        else:
            marker = _apply_put(lines, current, list_name, name, vtodo, state)

        alarms = alarms_of(vtodo)
        if alarms:
            snap.alarms[marker] = alarms
        else:
            snap.alarms.pop(marker, None)
        # Completing a recurring task added its next occurrence: the
        # location and due-relative alarms move on to it.
        carried = carried_alarms(alarms) if current is not None else []
        for new_marker in _markers(lines) - before - {marker}:
            if carried:
                snap.alarms[new_marker] = carried
        _forget_alarms_of_missing_lines(lines, snap)

        _write_lines(lines)
        state["gravestones"].pop(name, None)

        written = Snapshot(content="\n".join(lines), state=state)
        new_idx = written.index_of(name)
        res = written.resource_at(new_idx) if new_idx is not None else None
        return (201 if current is None else 200), (res.etag if res else None)


def _markers(lines: list[str]) -> set[str]:
    return {item.marker for idx, line in enumerate(lines)
            if (item := parse_line(line, idx)) is not None and item.marker}


def _forget_alarms_of_missing_lines(lines: list[str], snap: Snapshot) -> None:
    present = _markers(lines)
    for marker in [m for m in snap.alarms if m not in present]:
        snap.alarms.pop(marker, None)


def _insert_new(lines: list[str], list_name: str, name: str, vtodo: Any,
                state: dict[str, Any]) -> str:
    marker = generate_marker()
    if name != marker:
        state["uid_to_marker"][name] = marker
    title, projects, places = tags_from_device(vtodo)
    item = TodoItem(
        line_index=0,
        marker=marker,
        title=title or "(no title)",
        projects=projects,
        contexts=places,
        due=parse_due(vtodo),
        recurrence=parse_recurrence(vtodo),
        note=parse_note(vtodo),
    )
    line = _render_todo_line(item, "")
    if is_completed(vtodo):
        line = rewrite_line(line, True)
    lines.insert(_separator_index(lines), line)
    return marker


def _apply_put(lines: list[str], current: Resource, list_name: str, name: str,
               vtodo: Any, state: dict[str, Any]) -> str:
    item = current.item
    updates = vtodo_to_updates(vtodo, item)
    done = updates.pop("done", None)

    merged = replace(item, **updates)
    if not merged.marker:
        # First edit of a line without a marker: give it one, keep the name.
        merged.marker = generate_marker()
        state["uid_to_marker"][name] = merged.marker

    if merged != item:
        lines[current.index] = _render_todo_line(merged, lines[current.index])

    if done is not None:
        # The same path as a click in the web app: completion date, and the
        # next occurrence for a recurring task.
        toggle_lines(lines, current.index, merged.marker)
    return merged.marker


def delete_item(list_name: str, name: str, if_match: str | None = None) -> int:
    """Delete a task and leave a gravestone. Returns the HTTP status."""
    with _locked_state() as state:
        _purge_gravestones(state)
        snap = Snapshot(state=state)
        current = snap.get(list_name, name)
        if current is None:
            return 404
        if if_match and if_match != "*" and current.etag != if_match:
            return 412

        line = snap.lines.pop(current.index)
        _write_lines(snap.lines)
        state["gravestones"][name] = {"line": line, "deleted_at": time.time()}
        return 204
