"""Tests for the iCal service (Markdown line ↔ VTODO)."""

from datetime import date, datetime, timezone

from icalendar import Calendar, Todo

from app.models.todo import SOMETIME_SENTINEL, TodoItem
from app.services.ical_service import (
    clean_title,
    item_to_ics,
    item_to_vtodo,
    parse_due,
    parse_vtodo_from_ics,
    location_name,
    split_summary,
    vtodo_to_updates,
    vtodo_uid,
)


def make_item(**kwargs) -> TodoItem:
    defaults = {"line_index": 0, "marker": "abc123", "title": "Test item"}
    defaults.update(kwargs)
    return TodoItem(**defaults)


def make_vtodo(**props) -> Todo:
    vtodo = Todo()
    for key, value in props.items():
        vtodo.add(key, value)
    return vtodo


class TestItemToVtodo:
    def test_uid_is_the_given_name(self):
        assert vtodo_uid(item_to_vtodo(make_item(), "IOS-UID-1")) == "IOS-UID-1"

    def test_tags_follow_the_title(self):
        item = make_item(projects=["shopping", "home"], contexts=["errands"])
        summary = str(item_to_vtodo(item, "x")["summary"])
        assert summary == "Test item #shopping #home @errands"

    def test_all_day_due_is_a_date(self):
        vtodo = item_to_vtodo(make_item(due=datetime(2026, 2, 1, 0, 0)), "x")
        assert vtodo["due"].dt == date(2026, 2, 1)

    def test_timed_due_is_utc(self):
        local = datetime(2026, 7, 1, 9, 30)
        vtodo = item_to_vtodo(make_item(due=local), "x")
        assert vtodo["due"].dt == local.astimezone(timezone.utc)

    def test_sometime_is_not_exported(self):
        item = make_item(due=SOMETIME_SENTINEL, due_is_sometime=True)
        assert item_to_vtodo(item, "x").get("due") is None

    def test_completed(self):
        item = make_item(done=True, done_date=datetime(2026, 1, 15))
        vtodo = item_to_vtodo(item, "x")
        assert str(vtodo["status"]) == "COMPLETED"
        assert vtodo.get("completed") is not None

    def test_recurrence(self):
        vtodo = item_to_vtodo(make_item(recurrence="weekly"), "x")
        assert vtodo["rrule"]["FREQ"] == ["WEEKLY"]


class TestParseDue:
    def test_utc_is_converted_to_local_time(self):
        utc = datetime(2026, 7, 1, 7, 30, tzinfo=timezone.utc)
        assert parse_due(make_vtodo(due=utc)) == utc.astimezone().replace(tzinfo=None)

    def test_floating_time_stays_local(self):
        assert parse_due(make_vtodo(due=datetime(2026, 7, 1, 9, 30))) == datetime(2026, 7, 1, 9, 30)

    def test_date_is_all_day(self):
        assert parse_due(make_vtodo(due=date(2026, 2, 1))) == datetime(2026, 2, 1, 0, 0)

    def test_round_trip_keeps_the_time(self):
        due = datetime(2026, 11, 3, 18, 15)
        vtodo = item_to_vtodo(make_item(due=due), "x")
        assert parse_due(vtodo) == due


class TestVtodoToUpdates:
    def test_unchanged_item_gives_no_updates(self):
        item = make_item(projects=["shopping"], contexts=["errands"],
                         due=datetime(2026, 7, 1, 9, 30), note="n", recurrence="daily")
        assert vtodo_to_updates(item_to_vtodo(item, "x"), item) == {}

    def test_title_change_strips_tags(self):
        item = make_item(contexts=["errands"])
        vtodo = make_vtodo(summary="New title @errands", status="NEEDS-ACTION")
        assert vtodo_to_updates(vtodo, item) == {"title": "New title"}

    def test_completion(self):
        vtodo = make_vtodo(summary="Test item", status="COMPLETED")
        assert vtodo_to_updates(vtodo, make_item())["done"] is True

    def test_sometime_survives_a_missing_due(self):
        item = make_item(due=SOMETIME_SENTINEL, due_is_sometime=True)
        vtodo = make_vtodo(summary="Test item", status="NEEDS-ACTION")
        assert "due" not in vtodo_to_updates(vtodo, item)

    def test_removed_due(self):
        item = make_item(due=datetime(2026, 7, 1, 9, 30))
        vtodo = make_vtodo(summary="Test item", status="NEEDS-ACTION")
        assert vtodo_to_updates(vtodo, item)["due"] is None

    def test_unknown_rrule_keeps_the_rule(self):
        item = make_item(recurrence="daily")
        vtodo = make_vtodo(summary="Test item", rrule={"freq": ["YEARLY"]})
        assert "recurrence" not in vtodo_to_updates(vtodo, item)


class TestTitles:
    def test_clean_title_removes_line_breaks(self):
        assert clean_title("Buy\nmilk\r\n  now ") == "Buy milk now"

    def test_split_summary(self):
        assert split_summary("Title #a +b @c") == ("Title", ["a", "b"], ["c"])

    def test_split_summary_quoted(self):
        assert split_summary('Plan #"Big Project" @"Bei Oma"') == (
            "Plan", ["Big Project"], ["Bei Oma"])

    def test_text_that_only_looks_like_a_tag_stays(self):
        assert split_summary("Call +49 30 1234, issue #12, C++ via a@b.de") == (
            "Call +49 30 1234, issue #12, C++ via a@b.de", [], [])

    def test_duplicate_tags_count_once(self):
        assert split_summary("X #Garten +garten") == ("X", ["Garten"], [])


class TestLocation:
    def _vtodo_with(self, prop_line: str) -> Todo:
        ics = ("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:u\r\n"
               "SUMMARY:S\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\n"
               "TRIGGER;VALUE=DATE-TIME:19760401T005545Z\r\n"
               f"{prop_line}\r\nEND:VALARM\r\nEND:VTODO\r\nEND:VCALENDAR\r\n")
        return parse_vtodo_from_ics(ics.encode())

    def test_structured_location_title(self):
        vtodo = self._vtodo_with("X-APPLE-STRUCTURED-LOCATION;VALUE=URI;"
                                 "X-TITLE=Büro:geo:52.5,13.4")
        assert location_name(vtodo) == "Büro"

    def test_address_gives_its_first_part(self):
        vtodo = self._vtodo_with('X-APPLE-STRUCTURED-LOCATION;VALUE=URI;'
                                 'X-TITLE="Hauptstraße 5, 10115 Berlin":geo:52.5,13.4')
        assert location_name(vtodo) == "Hauptstraße 5"

    def test_no_location(self):
        assert location_name(make_vtodo(summary="S")) is None


class TestParseIcs:
    def test_parses_first_vtodo(self):
        cal = Calendar()
        cal.add("version", "2.0")
        cal.add_component(make_vtodo(uid="u1", summary="S"))
        assert vtodo_uid(parse_vtodo_from_ics(cal.to_ical())) == "u1"

    def test_garbage_gives_none(self):
        assert parse_vtodo_from_ics(b"not ical") is None

    def test_item_to_ics_round_trip(self):
        ics = item_to_ics(make_item(title="Hello"), "abc123")
        vtodo = parse_vtodo_from_ics(ics)
        assert str(vtodo["summary"]) == "Hello"
