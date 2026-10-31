"""Tests for the iCal service (Markdown line ↔ VTODO)."""

from datetime import datetime, date

import pytest

from app.models.todo import TodoItem
from app.services.ical_service import (
    item_to_vtodo,
    vtodo_to_updates,
    item_to_ics,
    parse_vtodo_from_ics,
    vtodo_uid,
    _strip_tags_from_summary,
    _parse_due_from_vtodo,
    _parse_rrule,
    _rrule_freq_to_recur,
)


def make_item(**kwargs) -> TodoItem:
    defaults = {
        "line_index": 0,
        "marker": "abc123",
        "title": "Test item",
        "projects": [],
        "contexts": [],
        "due": None,
        "due_is_sometime": False,
        "myday": None,
        "in_myday": False,
        "reference": None,
        "recurrence": None,
        "note": None,
        "done": False,
        "done_date": None,
        "raw_line": "",
    }
    defaults.update(kwargs)
    return TodoItem(**defaults)


class TestItemToVtodo:
    def test_basic_fields(self):
        item = make_item(title="Buy milk")
        vtodo = item_to_vtodo(item, "shopping")
        assert str(vtodo.get("uid")) == "abc123"
        assert str(vtodo.get("summary")) == "Buy milk"
        assert str(vtodo.get("status")) == "NEEDS-ACTION"

    def test_tagged_summary(self):
        item = make_item(title="Call doctor", projects=["health"], contexts=["phone"])
        vtodo = item_to_vtodo(item, "health", tag_titles=True)
        summary = str(vtodo.get("summary"))
        assert "+health" in summary
        assert "@phone" in summary
        assert "Call doctor" in summary

    def test_no_tags_in_summary_when_disabled(self):
        item = make_item(title="Call doctor", projects=["health"])
        vtodo = item_to_vtodo(item, "health", tag_titles=False)
        assert str(vtodo.get("summary")) == "Call doctor"

    def test_empty_title(self):
        item = make_item(title="")
        vtodo = item_to_vtodo(item, "inbox")
        assert str(vtodo.get("summary")) == "(no title)"

    def test_note_becomes_description(self):
        item = make_item(title="Task", note="Details here")
        vtodo = item_to_vtodo(item, "list")
        assert str(vtodo.get("description")) == "Details here"

    def test_no_note_means_no_description(self):
        item = make_item(title="Task")
        vtodo = item_to_vtodo(item, "list")
        assert vtodo.get("description") is None

    def test_due_all_day(self):
        item = make_item(
            title="All day", due=datetime(2026, 1, 20, 0, 0)
        )
        vtodo = item_to_vtodo(item, "list")
        due = vtodo.get("due")
        assert due is not None
        from icalendar.prop import vDDDTypes
        assert isinstance(due.dt, date)
        assert due.dt == date(2026, 1, 20)

    def test_due_with_time(self):
        item = make_item(
            title="Timed", due=datetime(2026, 1, 20, 14, 30)
        )
        vtodo = item_to_vtodo(item, "list")
        due = vtodo.get("due")
        assert due is not None
        assert isinstance(due.dt, datetime)

    def test_sometime_due_omitted(self):
        item = make_item(
            title="Someday", due=datetime(9999, 12, 31, 0, 0), due_is_sometime=True
        )
        vtodo = item_to_vtodo(item, "list")
        assert vtodo.get("due") is None

    def test_completed(self):
        item = make_item(
            title="Done", done=True, done_date=datetime(2026, 1, 15, 10, 0)
        )
        vtodo = item_to_vtodo(item, "list")
        assert str(vtodo.get("status")) == "COMPLETED"
        assert vtodo.get("completed") is not None

    def test_recurrence_daily(self):
        item = make_item(title="Daily task", recurrence="daily")
        vtodo = item_to_vtodo(item, "list")
        rrule = vtodo.get("rrule")
        assert rrule is not None
        assert rrule["FREQ"] == ["DAILY"]

    def test_recurrence_weekly(self):
        item = make_item(title="Weekly task", recurrence="weekly")
        vtodo = item_to_vtodo(item, "list")
        rrule = vtodo.get("rrule")
        assert rrule["FREQ"] == ["WEEKLY"]

    def test_recurrence_monthly(self):
        item = make_item(title="Monthly", recurrence="monthly")
        vtodo = item_to_vtodo(item, "list")
        rrule = vtodo.get("rrule")
        assert rrule["FREQ"] == ["MONTHLY"]

    def test_categories_from_projects(self):
        item = make_item(title="Task", projects=["work", "dev"])
        vtodo = item_to_vtodo(item, "work")
        cats = vtodo.get("categories")
        assert cats is not None
        assert sorted(cats) == ["dev", "work"]


class TestVtodoToUpdates:
    def _modify_ics(self, ics: bytes, **modifications) -> bytes:
        """Re-serialize a VTODO after applying modifications."""
        from icalendar import Calendar
        vtodo = parse_vtodo_from_ics(ics)
        assert vtodo is not None
        for key in modifications:
            vtodo.pop(key, None)
        for key, value in modifications.items():
            if value is not None:
                vtodo.add(key, value)
        cal = Calendar()
        cal.add("version", "2.0")
        cal.add("prodid", "-//Test//EN")
        cal.add_component(vtodo)
        return cal.to_ical()

    def test_status_completed(self):
        item = make_item(title="Task")
        ics = item_to_ics(item, "list")
        ics_new = self._modify_ics(ics, status="COMPLETED")
        vtodo = parse_vtodo_from_ics(ics_new)

        updates = vtodo_to_updates(vtodo, item)
        assert updates["done"] is True
        assert updates["recurrence_done"] is False

    def test_title_change(self):
        item = make_item(title="Original")
        ics = item_to_ics(item, "list")
        ics_new = self._modify_ics(ics, summary="Changed")
        vtodo = parse_vtodo_from_ics(ics_new)

        updates = vtodo_to_updates(vtodo, item)
        assert updates["title"] == "Changed"

    def test_due_change(self):
        item = make_item(title="Task", due=datetime(2026, 1, 20, 0, 0))
        ics = item_to_ics(item, "list")
        ics_new = self._modify_ics(ics, due=date(2026, 2, 1))
        vtodo = parse_vtodo_from_ics(ics_new)

        updates = vtodo_to_updates(vtodo, item)
        assert updates["due"] == datetime(2026, 2, 1, 0, 0)

    def test_tags_preserved_in_summary_strip(self):
        item = make_item(title="Call doctor", projects=["health"])
        summary = "+health Call doctor"
        stripped = _strip_tags_from_summary(summary, item)
        assert "Call doctor" in stripped
        assert "+health" not in stripped

    def test_recurrence_done_detected(self):
        item = make_item(title="Daily", recurrence="daily")
        ics = item_to_ics(item, "list")
        ics_new = self._modify_ics(ics, status="COMPLETED")
        vtodo = parse_vtodo_from_ics(ics_new)

        updates = vtodo_to_updates(vtodo, item)
        assert updates["done"] is True
        assert updates["recurrence_done"] is True

    def test_note_change(self):
        item = make_item(title="Task", note="Old note")
        ics = item_to_ics(item, "list")
        ics_new = self._modify_ics(ics, description="New note")
        vtodo = parse_vtodo_from_ics(ics_new)

        updates = vtodo_to_updates(vtodo, item)
        assert updates["note"] == "New note"


class TestIcsRoundtrip:
    def test_roundtrip(self):
        item = make_item(
            title="Buy groceries",
            projects=["shopping"],
            contexts=["errands"],
            due=datetime(2026, 1, 20, 12, 0),
            note="Milk and eggs",
            recurrence="weekly",
        )
        ics = item_to_ics(item, "shopping")
        vtodo = parse_vtodo_from_ics(ics)
        assert vtodo is not None
        assert vtodo_uid(vtodo) == "abc123"

    def test_completed_roundtrip(self):
        item = make_item(
            title="Finished",
            done=True,
            done_date=datetime(2026, 1, 15, 10, 0),
        )
        ics = item_to_ics(item, "list")
        vtodo = parse_vtodo_from_ics(ics)
        assert str(vtodo.get("status")) == "COMPLETED"


class TestRecurMapping:
    def test_daily(self):
        assert _rrule_freq_to_recur("DAILY") == "daily"

    def test_weekly(self):
        assert _rrule_freq_to_recur("WEEKLY") == "weekly"

    def test_monthly(self):
        assert _rrule_freq_to_recur("MONTHLY") == "monthly"

    def test_unknown(self):
        assert _rrule_freq_to_recur("HOURLY") is None


class TestParseRrule:
    def test_parse_weekly_rrule(self):
        item = make_item(title="Weekly", recurrence="weekly")
        ics = item_to_ics(item, "list")
        vtodo = parse_vtodo_from_ics(ics)
        assert _parse_rrule(vtodo) == "weekly"


class TestParseDueFromVtodo:
    def test_date_only(self):
        item = make_item(title="D", due=datetime(2026, 1, 20, 0, 0))
        ics = item_to_ics(item, "list")
        vtodo = parse_vtodo_from_ics(ics)
        parsed = _parse_due_from_vtodo(vtodo)
        assert parsed == datetime(2026, 1, 20, 0, 0)

    def test_date_time(self):
        item = make_item(title="D", due=datetime(2026, 1, 20, 14, 30))
        ics = item_to_ics(item, "list")
        vtodo = parse_vtodo_from_ics(ics)
        parsed = _parse_due_from_vtodo(vtodo)
        assert parsed is not None
        assert parsed.hour == 14
        assert parsed.minute == 30

    def test_no_due(self):
        item = make_item(title="D")
        ics = item_to_ics(item, "list")
        vtodo = parse_vtodo_from_ics(ics)
        assert _parse_due_from_vtodo(vtodo) is None

    def test_bad_ics(self):
        assert parse_vtodo_from_ics(b"not valid ics") is None