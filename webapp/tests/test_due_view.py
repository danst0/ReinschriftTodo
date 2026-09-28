"""Tests for relative due dates and the schedule (date view) grouping."""

import os
import sys
from datetime import date, datetime, timedelta

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.utils.due_view import describe_due, horizon_of
from translations import TRANSLATIONS

DE = TRANSLATIONS['de']
# A Monday, so "this week" runs through Sunday the 4th.
NOW = datetime(2026, 9, 28, 14, 30)


class TestHorizon:

    def test_horizons_around_monday(self):
        today = NOW.date()
        assert horizon_of(None, today) == 'none'
        assert horizon_of(datetime(9999, 12, 31), today) == 'sometime'
        assert horizon_of(datetime(2026, 9, 27, 23, 59), today) == 'overdue'
        assert horizon_of(datetime(2026, 9, 28), today) == 'today'
        assert horizon_of(datetime(2026, 9, 29, 10), today) == 'tomorrow'
        assert horizon_of(datetime(2026, 10, 4), today) == 'week'
        assert horizon_of(datetime(2026, 10, 5), today) == 'later'

    def test_sunday_has_no_rest_of_week(self):
        sunday = date(2026, 10, 4)
        assert horizon_of(datetime(2026, 10, 5), sunday) == 'tomorrow'
        assert horizon_of(datetime(2026, 10, 6), sunday) == 'later'


class TestDescribeDue:

    def test_tomorrow_with_time(self):
        dv = describe_due({'due': '2026-09-29T10:00'}, DE, NOW)
        assert dv['label'] == 'Morgen, 10:00'
        assert dv['time'] == '10:00'
        assert dv['day'] == ''
        assert not dv['marked'] and not dv['late']

    def test_date_only_has_no_time(self):
        dv = describe_due({'due': '2026-09-30T00:00'}, DE, NOW)
        assert dv['label'] == 'Mi, 30.09.'
        assert dv['day'] == 'Mi'
        assert dv['time'] == ''

    def test_today_passed_time_is_late_and_marked(self):
        dv = describe_due({'due': '2026-09-28T09:00'}, DE, NOW)
        assert dv['label'] == 'Heute, 09:00'
        assert dv['late'] and dv['marked']

    def test_overdue_shows_date(self):
        dv = describe_due({'due': '2026-09-26T00:00'}, DE, NOW)
        assert dv['horizon'] == 'overdue'
        assert dv['label'] == 'Sa, 26.09.'
        assert dv['day'] == '26.09.'
        assert dv['late'] and dv['marked']

    def test_done_is_neither_late_nor_marked(self):
        dv = describe_due({'due': '2026-09-26T00:00', 'done': True}, DE, NOW)
        assert not dv['late'] and not dv['marked']

    def test_sometime_and_none(self):
        assert describe_due({'due': '9999-12-31T00:00'}, DE, NOW)['label'] == 'Irgendwann'
        assert describe_due({'due': None}, DE, NOW)['label'] == ''

    def test_other_language(self):
        dv = describe_due({'due': '2026-09-30T08:15'}, TRANSLATIONS['en'], NOW)
        assert dv['label'] == 'Wed, Sep 30, 08:15'


class TestScheduleView:

    def _login(self, client):
        with client.session_transaction() as sess:
            sess['logged_in'] = True

    def _patch(self, monkeypatch, lines):
        from app.services.parser import parse_line
        items = [parse_line(l, i) for i, l in enumerate(lines)]
        monkeypatch.setattr('app.routes.main.load_todos', lambda: [i for i in items if i])
        monkeypatch.setattr('app.routes.main.load_settings', lambda: {})
        monkeypatch.setattr('app.routes.main.save_settings', lambda s: None)

    def test_date_view_groups_by_horizon_undated_last(self, client, monkeypatch):
        today = date.today()
        self._login(client)
        self._patch(monkeypatch, [
            "- [ ] Undated task ^aaa111",
            f"- [ ] Tomorrow task due:{(today + timedelta(days=1)).isoformat()}T10:00 ^bbb222",
            f"- [ ] Overdue task due:{(today - timedelta(days=3)).isoformat()} ^ccc333",
            f"- [ ] Today task due:{today.isoformat()} ^ddd444",
        ])
        html = client.get('/?sort_mode=date&partial=1').get_data(as_text=True)
        order = [html.index(s) for s in (
            'Überfällig', 'Overdue task', 'class="section-title">Heute', 'Today task',
            'class="section-title">Morgen', 'Tomorrow task',
            'Ohne Datum', 'Undated task')]
        assert order == sorted(order)
        assert 'is-schedule' in html
        # Today's open task gets the highlighter.
        assert '<mark>Today task</mark>' in html

    def test_topic_view_shows_relative_due_inline(self, client, monkeypatch):
        tomorrow = date.today() + timedelta(days=1)
        self._login(client)
        self._patch(monkeypatch, [
            f"- [ ] Tyres +Car @Garage due:{tomorrow.isoformat()}T10:00 ^aaa111",
        ])
        html = client.get('/?sort_mode=topic&partial=1').get_data(as_text=True)
        assert 'Morgen, 10:00' in html
        assert 'is-schedule' not in html
        # The group is the header; the project is not repeated in the row.
        assert 'class="section-title">Car' in html
        assert '+Car' not in html
        assert '@Garage' in html
