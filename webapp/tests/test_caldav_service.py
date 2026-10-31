"""Tests for the CalDAV service."""

import os
import shutil
import tempfile
from datetime import date, datetime, timedelta

import pytest
from icalendar import Calendar, Todo

from app import create_app
from app.services import caldav_service
from app.services.caldav_service import (
    INBOX,
    Snapshot,
    calendar_ctag,
    delete_item,
    get_resource,
    list_calendars,
    put_item,
)


@pytest.fixture(autouse=True)
def setup_env(temp_todo_file):
    """Isolated todo file and caldav.json, inside an app/request context."""
    config_dir = tempfile.mkdtemp(prefix="caldav_svc_")
    old = {k: os.environ.get(k) for k in ("TODOS_DB_PATH", "CONFIG_PATH")}
    os.environ["TODOS_DB_PATH"] = temp_todo_file
    os.environ["CONFIG_PATH"] = os.path.join(config_dir, "settings.json")
    app = create_app("testing")
    app.config["TODO_PATH"] = temp_todo_file
    with app.test_request_context():
        yield
    for key, value in old.items():
        if value is None:
            os.environ.pop(key, None)
        else:
            os.environ[key] = value
    shutil.rmtree(config_dir, ignore_errors=True)


def _fresh_request(fn, *args, **kwargs):
    """Run a write in its own request, as the real routes do."""
    from flask import current_app
    with current_app.test_request_context():
        return fn(*args, **kwargs)


def _ics(uid, title, status="NEEDS-ACTION", due=None, freq=None):
    cal = Calendar()
    cal.add("version", "2.0")
    cal.add("prodid", "-//Test//EN")
    vtodo = Todo()
    vtodo.add("uid", uid)
    vtodo.add("summary", title)
    vtodo.add("status", status)
    if due is not None:
        vtodo.add("due", due)
    if freq is not None:
        vtodo.add("rrule", {"freq": [freq]})
    cal.add_component(vtodo)
    return cal.to_ical()


def _file(path):
    with open(path, encoding="utf-8") as fh:
        return fh.read()


def _put(*args, **kwargs):
    return _fresh_request(put_item, *args, **kwargs)


class TestListCalendars:
    def test_inbox_first_then_projects(self):
        assert list_calendars() == [INBOX, "dev", "health", "personal", "shopping", "work"]

    def test_old_done_tasks_do_not_keep_a_list_alive(self, temp_todo_file):
        with open(temp_todo_file, "w") as f:
            f.write("- [x] Old +archive ✅ 2025-01-01 ^old1\n")
        assert list_calendars() == [INBOX]


class TestResources:
    def test_items_of_a_list(self):
        titles = [r.item.title for r in Snapshot().resources("shopping")]
        assert titles == ["Buy groceries"]

    def test_done_older_than_30_days_is_hidden(self):
        assert [r.item.title for r in Snapshot().resources("work")] == ["Meeting notes"]

    def test_recent_done_is_shown(self, temp_todo_file):
        recent = (date.today() - timedelta(days=3)).isoformat()
        with open(temp_todo_file, "w") as f:
            f.write(f"- [x] Recent +work ✅ {recent} ^rec1\n")
        assert [r.name for r in Snapshot().resources("work")] == ["rec1"]

    def test_reading_never_writes(self, temp_todo_file):
        with open(temp_todo_file, "w") as f:
            f.write("- [ ] No marker\n")
        before = _file(temp_todo_file)
        [res] = Snapshot().resources(INBOX)
        assert res.name.startswith("h-")
        assert _file(temp_todo_file) == before

    def test_get_checks_the_list(self):
        assert get_resource("shopping", "abc123") is not None
        assert get_resource("health", "abc123") is None

    def test_ctag_is_stable_and_changes(self):
        tag = calendar_ctag("shopping")
        assert calendar_ctag("shopping") == tag
        _put("shopping", "abc123", _ics("abc123", "New title"))
        assert calendar_ctag("shopping") != tag


class TestPutItem:
    def test_update_title_keeps_other_fields(self, temp_todo_file):
        status, etag = _put("shopping", "abc123", _ics("abc123", "@errands Buy milk",
                                                       due=datetime(2026, 1, 20, 12, 0)))
        assert status == 200 and etag
        line = next(l for l in _file(temp_todo_file).splitlines() if "^abc123" in l)
        assert line == "- [ ] Buy milk +shopping @errands due:2026-01-20T12:00 ^abc123"

    def test_etag_matches_a_following_get(self):
        _, etag = _put("shopping", "abc123", _ics("abc123", "X"))
        assert get_resource("shopping", "abc123").etag == etag

    def test_if_match_mismatch(self):
        assert _put("shopping", "abc123", _ics("abc123", "X"), if_match='"nope"')[0] == 412

    def test_if_none_match_on_existing(self):
        assert _put("shopping", "abc123", _ics("abc123", "X"), if_none_match="*")[0] == 412

    def test_completion_keeps_due_and_adds_date(self, temp_todo_file):
        _put("health", "def456", _ics("def456", "@phone Call doctor", "COMPLETED",
                                      due=datetime(2026, 1, 21, 9, 0)))
        line = next(l for l in _file(temp_todo_file).splitlines() if "^def456" in l)
        assert line.startswith("- [x] Call doctor")
        assert "due:2026-01-21T09:00" in line
        assert f"✅ {date.today().isoformat()}" in line

    def test_recurring_completion_spawns_next(self, temp_todo_file):
        _put("dev", "jkl012", _ics("jkl012", "@computer Review PR", "COMPLETED",
                                   due=datetime(2026, 1, 22, 10, 0), freq="DAILY"))
        lines = [l for l in _file(temp_todo_file).splitlines() if "Review PR" in l]
        assert len(lines) == 2
        assert any(l.startswith("- [ ]") and "rec:daily" in l for l in lines)

    def test_new_item_keeps_the_ios_uid(self, temp_todo_file):
        status, _ = _put("shopping", "IOS-UID-1", _ics("IOS-UID-1", "From\niPhone"))
        assert status == 201
        [res] = [r for r in Snapshot().resources("shopping") if r.name == "IOS-UID-1"]
        assert res.item.title == "From iPhone"
        assert res.item.projects == ["shopping"]
        assert res.item.marker and res.item.marker != "IOS-UID-1"
        assert b"UID:IOS-UID-1" in res.ics

    def test_new_item_in_inbox_has_no_project(self):
        _put(INBOX, "IOS-UID-2", _ics("IOS-UID-2", "Loose"))
        [res] = [r for r in Snapshot().resources(INBOX) if r.name == "IOS-UID-2"]
        assert res.item.projects == []

    def test_move_to_another_list(self):
        _put("health", "abc123", _ics("abc123", "@errands Buy groceries"))
        res = get_resource("health", "abc123")
        assert res is not None and res.item.projects == ["health"]

    def test_first_edit_of_unmarked_line_keeps_its_name(self, temp_todo_file):
        with open(temp_todo_file, "w") as f:
            f.write("- [ ] No marker\n")
        [res] = Snapshot().resources(INBOX)
        assert _put(INBOX, res.name, _ics(res.name, "Renamed"))[0] == 200
        assert "^" in _file(temp_todo_file)
        assert get_resource(INBOX, res.name).item.title == "Renamed"

    def test_bad_body(self):
        assert _put("shopping", "abc123", b"nope")[0] == 400


class TestDeleteItem:
    def test_delete(self, temp_todo_file):
        assert _fresh_request(delete_item, "shopping", "abc123") == 204
        assert "Buy groceries" not in _file(temp_todo_file)

    def test_delete_nonexistent(self):
        assert _fresh_request(delete_item, "shopping", "nope") == 404

    def test_delete_wrong_list(self):
        assert _fresh_request(delete_item, "health", "abc123") == 404

    def test_move_as_delete_then_put_keeps_fields(self, temp_todo_file):
        _fresh_request(delete_item, "shopping", "abc123")
        assert _put("health", "abc123", _ics("abc123", "@errands Buy groceries",
                                             due=datetime(2026, 1, 20, 12, 0)))[0] == 200
        line = next(l for l in _file(temp_todo_file).splitlines() if "^abc123" in l)
        assert line == "- [ ] Buy groceries +health @errands due:2026-01-20T12:00 ^abc123"

    def test_gravestone_expires(self, monkeypatch):
        _fresh_request(delete_item, "shopping", "abc123")
        monkeypatch.setattr(caldav_service, "GRAVESTONE_TTL", -1)
        assert _put("shopping", "abc123", _ics("abc123", "Back"))[0] == 201
