"""Tests for the CalDAV service."""

import os
import tempfile
import time
from datetime import datetime

import pytest

from app.services.caldav_service import (
    list_calendars,
    calendar_items,
    calendar_ctag,
    get_item_ics,
    get_item_etag,
    put_item,
    delete_item,
    ensure_markers,
    INBOX,
    _purge_expired_gravestones,
    _add_gravestone,
    _check_gravestone,
    _remember_uid_mapping,
    _forget_uid_mapping,
    _marker_for_uid,
    _marker_for_href,
)
from app.services.ical_service import GRAVESTONE_TTL


@pytest.fixture(autouse=True)
def setup_env(temp_todo_file):
    """Point TODOS_DB_PATH and CONFIG_PATH at isolated test dirs."""
    old_db = os.environ.get("TODOS_DB_PATH")
    old_config = os.environ.get("CONFIG_PATH")
    config_dir = tempfile.mkdtemp(prefix="caldav_svc_")
    os.environ["TODOS_DB_PATH"] = temp_todo_file
    os.environ["CONFIG_PATH"] = os.path.join(config_dir, "settings.json")
    yield
    if old_db is not None:
        os.environ["TODOS_DB_PATH"] = old_db
    else:
        os.environ.pop("TODOS_DB_PATH", None)
    if old_config is not None:
        os.environ["CONFIG_PATH"] = old_config
    else:
        os.environ.pop("CONFIG_PATH", None)
    import shutil
    shutil.rmtree(config_dir, ignore_errors=True)


class TestListCalendars:
    def test_finds_projects(self):
        cals = list_calendars()
        names = [c.name for c in cals]
        assert "shopping" in names
        assert "health" in names
        assert "work" in names
        assert "dev" in names
        assert "personal" in names

    def test_inbox_appears_when_item_has_no_project(self, temp_todo_file):
        with open(temp_todo_file, 'w') as f:
            f.write("- [ ] Plain task without project\n\n---\n")
        cals = list_calendars()
        names = [c.name for c in cals]
        assert INBOX in names

    def test_projects_sorted(self):
        cals = list_calendars()
        project_names = [c.name for c in cals]
        assert project_names == sorted(project_names)


class TestCalendarItems:
    def test_returns_items_for_list(self):
        items = calendar_items("shopping")
        titles = [item[0].title for item in items]
        assert "Buy groceries" in titles

    def test_empty_list(self):
        items = calendar_items("nonexistent")
        assert len(items) == 0

    def test_inbox_items(self, temp_todo_file):
        with open(temp_todo_file, 'w') as f:
            f.write("- [ ] Inbox task no project ^ibt001\n\n---\n")
        items = calendar_items(INBOX)
        titles = [item[0].title for item in items]
        assert "Inbox task no project" in titles

    def test_each_item_has_etag(self):
        items = calendar_items("dev")
        for _, ics, etag in items:
            assert etag.startswith('"')
            assert len(etag) > 2

    def test_done_items_included_with_flag(self):
        items = calendar_items("work", include_done=True)
        titles = [item[0].title for item in items]
        assert "Finish report" in titles

    def test_done_items_excluded_by_default(self):
        items = calendar_items("work")
        titles = [item[0].title for item in items]
        assert "Finish report" not in titles


class TestCalendarCtag:
    def test_ctag_is_stable(self):
        tag1 = calendar_ctag("shopping")
        tag2 = calendar_ctag("shopping")
        assert tag1 == tag2

    def test_ctag_changes_after_modification(self):
        tag1 = calendar_ctag("shopping")
        put_item("shopping", "abc123", _make_ics_update("abc123", "New title"), None)
        tag2 = calendar_ctag("shopping")
        assert tag1 != tag2


class TestGetItem:
    def test_get_existing_item(self):
        ics = get_item_ics("shopping", "abc123")
        assert ics is not None
        assert b"Buy groceries" in ics

    def test_get_etag(self):
        etag = get_item_etag("shopping", "abc123")
        assert etag is not None
        assert etag.startswith('"')

    def test_get_nonexistent(self):
        assert get_item_ics("shopping", "nonexistent") is None


class TestPutItem:
    def test_update_title(self):
        ics = _make_ics_update("abc123", "New title!")
        status, etag = put_item("shopping", "abc123", ics)
        assert status in (200, 201)

        items = calendar_items("shopping")
        titles = [item[0].title for item in items]
        assert "New title!" in titles
        assert "Buy groceries" not in titles

    def test_update_due_date(self):
        ics = _make_ics_update("abc123", "Buy groceries", due_date="20260201")
        status, _ = put_item("shopping", "abc123", ics)
        assert status in (200, 201)

        items = calendar_items("shopping")
        for item, _, _ in items:
            if item.marker == "abc123":
                assert item.due is not None
                assert item.due.strftime("%Y-%m-%d") == "2026-02-01"

    def test_complete_todo(self):
        ics = _make_ics_update("jkl012", "Review PR", status="COMPLETED")
        status, _ = put_item("dev", "jkl012", ics)
        assert status == 200

        items = calendar_items("dev", include_done=True)
        for item, _, _ in items:
            if item.marker == "jkl012":
                assert item.done is True

    def test_new_item_from_ios(self):
        import uuid
        new_uid = uuid.uuid4().hex[:8]
        ics = _make_raw_ics(new_uid, "From iPhone", "needs-action")
        status, etag = put_item(INBOX, new_uid, ics)
        assert status == 201
        assert etag is not None

        items = calendar_items(INBOX)
        titles = [item[0].title for item in items]
        assert "From iPhone" in titles

    def test_new_item_gets_list_project(self):
        import uuid
        new_uid = uuid.uuid4().hex[:8]
        ics = _make_raw_ics(new_uid, "Bucket task", "needs-action")
        status, _ = put_item("shopping", new_uid, ics)
        assert status == 201

        items = calendar_items("shopping")
        for item, _, _ in items:
            if item.title == "Bucket task":
                assert "shopping" in item.projects


class TestDeleteItem:
    def test_delete_existing(self):
        result = delete_item("shopping", "abc123")
        assert result is True
        items = calendar_items("shopping")
        titles = [item[0].title for item in items]
        assert "Buy groceries" not in titles

    def test_delete_nonexistent(self):
        result = delete_item("shopping", "nonexistent")
        assert result is False

    def test_delete_creates_gravestone(self):
        delete_item("shopping", "abc123")
        gs = _check_gravestone("abc123")
        assert gs is not None
        assert gs["marker"] == "abc123"


class TestGravestoneResurrection:
    def test_put_after_delete_restores(self):
        delete_item("shopping", "abc123")
        gs = _check_gravestone("abc123")
        assert gs is not None

        ics = _make_ics_update("abc123", "Restored task")
        status, _ = put_item("shopping", "abc123", ics)
        assert status == 200

        items = calendar_items("shopping")
        titles = [item[0].title for item in items]
        assert "Restored task" in titles

        gs2 = _check_gravestone("abc123")
        assert gs2 is None


class TestEnsureMarkers:
    def test_adds_markers_to_unmarked_lines(self, temp_todo_file):
        content = """- [ ] No marker task
- [ ] Has marker ^existing
"""
        with open(temp_todo_file, 'w') as f:
            f.write(content)

        added = ensure_markers()
        assert added == 1

        with open(temp_todo_file) as f:
            text = f.read()
        assert "^existing" in text
        assert "No marker task" in text


def _make_ics_update(uid: str, title: str, status: str = "needs-action",
                     due_date: str | None = None) -> bytes:
    """Build a minimal iCalendar body for a PUT update."""
    from icalendar import Calendar, Todo
    cal = Calendar()
    cal.add("version", "2.0")
    cal.add("prodid", "-//Test//EN")
    vtodo = Todo()
    vtodo.add("uid", uid)
    vtodo.add("summary", title)
    vtodo.add("status", status.upper())
    if due_date:
        from datetime import date
        year, month, day = int(due_date[0:4]), int(due_date[4:6]), int(due_date[6:8])
        vtodo.add("due", date(year, month, day))
    cal.add_component(vtodo)
    return cal.to_ical()


def _make_raw_ics(uid: str, title: str, status: str = "needs-action") -> bytes:
    return _make_ics_update(uid, title, status=status)