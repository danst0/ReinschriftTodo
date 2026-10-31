"""Tests for the CalDAV service."""

import os
import shutil
import tempfile
from datetime import date, datetime, timedelta

import pytest
from icalendar import Alarm, Calendar, Todo

from app import create_app
from app.services import caldav_service
from app.services.caldav_service import (
    LIST,
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


def _ics(uid, title, status="NEEDS-ACTION", due=None, freq=None, alarms=()):
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
    for alarm in alarms:
        vtodo.add_component(Alarm.from_ical(alarm))
    cal.add_component(vtodo)
    return cal.to_ical()


def _file(path):
    with open(path, encoding="utf-8") as fh:
        return fh.read()


def _put(*args, **kwargs):
    return _fresh_request(put_item, *args, **kwargs)


class TestListCalendars:
    def test_one_list(self):
        assert list_calendars() == [LIST]


class TestResources:
    def test_all_tasks_in_the_one_list(self):
        titles = [r.item.title for r in Snapshot().resources(LIST)]
        assert titles == ["Buy groceries", "Call doctor", "Review PR", "Plan vacation",
                          "Meeting notes"]

    def test_projects_and_places_follow_the_title_as_tags(self):
        res = get_resource(LIST, "abc123")
        assert b"SUMMARY:Buy groceries #shopping @errands" in res.ics
        assert b"DTSTAMP:" in res.ics

    def test_done_older_than_30_days_is_hidden(self):
        assert "ghi789" not in [r.name for r in Snapshot().resources(LIST)]

    def test_recent_done_is_shown(self, temp_todo_file):
        recent = (date.today() - timedelta(days=3)).isoformat()
        with open(temp_todo_file, "w") as f:
            f.write(f"- [x] Recent +work ✅ {recent} ^rec1\n")
        assert [r.name for r in Snapshot().resources(LIST)] == ["rec1"]

    def test_reading_never_writes(self, temp_todo_file):
        with open(temp_todo_file, "w") as f:
            f.write("- [ ] No marker\n")
        before = _file(temp_todo_file)
        [res] = Snapshot().resources(LIST)
        assert res.name.startswith("h-")
        assert _file(temp_todo_file) == before

    def test_get_checks_the_list(self):
        assert get_resource(LIST, "abc123") is not None
        assert get_resource("shopping", "abc123") is None

    def test_ctag_is_stable_and_changes(self):
        tag = calendar_ctag(LIST)
        assert calendar_ctag(LIST) == tag
        _put(LIST, "abc123", _ics("abc123", "New title"))
        assert calendar_ctag(LIST) != tag


class TestPutItem:
    def test_update_title_keeps_other_fields(self, temp_todo_file):
        status, etag = _put(LIST, "abc123", _ics("abc123", "Buy milk #shopping @errands",
                                                       due=datetime(2026, 1, 20, 12, 0)))
        assert status == 200 and etag
        line = next(l for l in _file(temp_todo_file).splitlines() if "^abc123" in l)
        assert line == "- [ ] Buy milk +shopping @errands due:2026-01-20T12:00 ^abc123"

    def test_etag_matches_a_following_get(self):
        _, etag = _put(LIST, "abc123", _ics("abc123", "X"))
        assert get_resource(LIST, "abc123").etag == etag

    def test_if_match_mismatch(self):
        assert _put(LIST, "abc123", _ics("abc123", "X"), if_match='"nope"')[0] == 412

    def test_if_none_match_on_existing(self):
        assert _put(LIST, "abc123", _ics("abc123", "X"), if_none_match="*")[0] == 412

    def test_completion_keeps_due_and_adds_date(self, temp_todo_file):
        _put(LIST, "def456", _ics("def456", "Call doctor #health @phone", "COMPLETED",
                                      due=datetime(2026, 1, 21, 9, 0)))
        line = next(l for l in _file(temp_todo_file).splitlines() if "^def456" in l)
        assert line.startswith("- [x] Call doctor")
        assert "due:2026-01-21T09:00" in line
        assert f"✅ {date.today().isoformat()}" in line

    def test_recurring_completion_spawns_next(self, temp_todo_file):
        _put(LIST, "jkl012", _ics("jkl012", "Review PR #dev @computer", "COMPLETED",
                                   due=datetime(2026, 1, 22, 10, 0), freq="DAILY"))
        lines = [l for l in _file(temp_todo_file).splitlines() if "Review PR" in l]
        assert len(lines) == 2
        assert any(l.startswith("- [ ]") and "rec:daily" in l for l in lines)

    def test_new_item_keeps_the_ios_uid(self, temp_todo_file):
        status, _ = _put(LIST, "IOS-UID-1", _ics("IOS-UID-1", "From\niPhone"))
        assert status == 201
        [res] = [r for r in Snapshot().resources(LIST) if r.name == "IOS-UID-1"]
        assert res.item.title == "From iPhone"
        assert res.item.projects == []
        assert res.item.marker and res.item.marker != "IOS-UID-1"
        assert b"UID:IOS-UID-1" in res.ics

    def test_unknown_list_is_rejected(self):
        assert _put("shopping", "IOS-UID-2", _ics("IOS-UID-2", "Loose"))[0] == 404

    def test_first_edit_of_unmarked_line_keeps_its_name(self, temp_todo_file):
        with open(temp_todo_file, "w") as f:
            f.write("- [ ] No marker\n")
        [res] = Snapshot().resources(LIST)
        assert _put(LIST, res.name, _ics(res.name, "Renamed"))[0] == 200
        assert "^" in _file(temp_todo_file)
        assert get_resource(LIST, res.name).item.title == "Renamed"

    def test_bad_body(self):
        assert _put(LIST, "abc123", b"nope")[0] == 400


LOCATION_ALARM = (
    "BEGIN:VALARM\r\n"
    "UID:ALARM-1\r\n"
    "ACTION:DISPLAY\r\n"
    "DESCRIPTION:Reminder\r\n"
    "TRIGGER;VALUE=DATE-TIME:19760401T005545Z\r\n"
    "X-APPLE-PROXIMITY:ARRIVE\r\n"
    "X-APPLE-STRUCTURED-LOCATION;VALUE=URI;X-APPLE-RADIUS=100;"
    "X-TITLE=Zuhause:geo:52.520008,13.404954\r\n"
    "END:VALARM\r\n"
)

TIME_ALARM = (
    "BEGIN:VALARM\r\n"
    "ACTION:DISPLAY\r\n"
    "DESCRIPTION:Reminder\r\n"
    "TRIGGER;VALUE=DATE-TIME:20260120T080000Z\r\n"
    "END:VALARM\r\n"
)


def _line(path, marker):
    return next(l for l in _file(path).splitlines() if f"^{marker}" in l)


class TestTags:
    def test_hashtag_from_the_device_becomes_a_project(self, temp_todo_file):
        _put(LIST, "abc123", _ics("abc123", "Buy groceries #shopping #home @errands"))
        assert _line(temp_todo_file, "abc123").startswith(
            "- [ ] Buy groceries +shopping +home @errands")

    def test_removing_a_tag_on_the_device_removes_it(self, temp_todo_file):
        _put(LIST, "abc123", _ics("abc123", "Buy groceries @errands"))
        assert "+shopping" not in _line(temp_todo_file, "abc123")

    def test_new_task_with_tags(self, temp_todo_file):
        _put(LIST, "IOS-T", _ics("IOS-T", "Mow the lawn #Garten @Daheim"))
        res = get_resource(LIST, "IOS-T")
        assert (res.item.title, res.item.projects, res.item.contexts) == (
            "Mow the lawn", ["Garten"], ["Daheim"])

    def test_multi_word_project_round_trip(self, temp_todo_file):
        with open(temp_todo_file, "w") as f:
            f.write('- [ ] Plan +"Big Project" ^big1\n')
        res = get_resource(LIST, "big1")
        assert 'SUMMARY:Plan #"Big Project"' in res.ics.decode()
        _put(LIST, "big1", _ics("big1", 'Plan it #"Big Project"'))
        assert _line(temp_todo_file, "big1") == '- [ ] Plan it +"Big Project" ^big1'


class TestAlarms:
    def test_alarm_from_the_device_survives_the_next_sync(self):
        _, etag = _put(LIST, "abc123", _ics("abc123", "Buy groceries #shopping @errands",
                                            alarms=[TIME_ALARM]))
        res = get_resource(LIST, "abc123")
        assert b"BEGIN:VALARM" in res.ics and b"TRIGGER" in res.ics
        assert res.etag == etag

    def test_alarm_removed_on_the_device_is_gone(self):
        _put(LIST, "abc123", _ics("abc123", "Buy groceries", alarms=[TIME_ALARM]))
        _put(LIST, "abc123", _ics("abc123", "Buy groceries"))
        assert b"VALARM" not in get_resource(LIST, "abc123").ics

    def test_location_names_the_place(self, temp_todo_file):
        _put(LIST, "IOS-L", _ics("IOS-L", "Take out the bins", alarms=[LOCATION_ALARM]))
        res = get_resource(LIST, "IOS-L")
        assert res.item.contexts == ["Zuhause"]
        assert b"X-APPLE-PROXIMITY:ARRIVE" in res.ics
        assert b"SUMMARY:Take out the bins @Zuhause" in res.ics

    def test_a_place_in_the_title_wins_over_the_location(self):
        _put(LIST, "abc123", _ics("abc123", "Buy groceries @errands",
                                  alarms=[LOCATION_ALARM]))
        assert get_resource(LIST, "abc123").item.contexts == ["errands"]

    def test_next_occurrence_keeps_the_location_alarm(self, temp_todo_file):
        _put(LIST, "jkl012", _ics("jkl012", "Review PR #dev @computer",
                                  due=datetime(2026, 1, 22, 10, 0), freq="DAILY",
                                  alarms=[LOCATION_ALARM, TIME_ALARM]))
        _put(LIST, "jkl012", _ics("jkl012", "Review PR #dev @computer", "COMPLETED",
                                  due=datetime(2026, 1, 22, 10, 0), freq="DAILY",
                                  alarms=[LOCATION_ALARM, TIME_ALARM]))
        [nxt] = [r for r in Snapshot().resources(LIST)
                 if r.item.title == "Review PR" and not r.item.done]
        assert b"X-APPLE-PROXIMITY" in nxt.ics
        assert b"20260120T080000Z" not in nxt.ics

    def test_alarms_of_deleted_lines_are_forgotten(self, temp_todo_file):
        _put(LIST, "abc123", _ics("abc123", "Buy groceries", alarms=[TIME_ALARM]))
        with open(temp_todo_file, "w") as f:
            f.write("- [ ] Other ^oth1\n")
        _put(LIST, "oth1", _ics("oth1", "Other"))
        assert "abc123" not in Snapshot().alarms


class TestDeleteItem:
    def test_delete(self, temp_todo_file):
        assert _fresh_request(delete_item, LIST, "abc123") == 204
        assert "Buy groceries" not in _file(temp_todo_file)

    def test_delete_nonexistent(self):
        assert _fresh_request(delete_item, LIST, "nope") == 404

    def test_delete_wrong_list(self):
        assert _fresh_request(delete_item, "shopping", "abc123") == 404

    def test_move_as_delete_then_put_keeps_fields(self, temp_todo_file):
        _fresh_request(delete_item, LIST, "abc123")
        assert _put(LIST, "abc123", _ics("abc123", "Buy groceries +shopping @errands",
                                         due=datetime(2026, 1, 20, 12, 0)))[0] == 200
        line = next(l for l in _file(temp_todo_file).splitlines() if "^abc123" in l)
        assert line == "- [ ] Buy groceries +shopping @errands due:2026-01-20T12:00 ^abc123"

    def test_gravestone_expires(self, monkeypatch):
        _fresh_request(delete_item, LIST, "abc123")
        monkeypatch.setattr(caldav_service, "GRAVESTONE_TTL", -1)
        assert _put(LIST, "abc123", _ics("abc123", "Back"))[0] == 201
