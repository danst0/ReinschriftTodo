"""Tests for the list filter (mirrors core/src/filter.rs)."""

import os
import sys
from datetime import date, timedelta

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from werkzeug.datastructures import MultiDict

from app.services.filters import NO_TAG, TodoFilter, filter_choices
from app.services.parser import parse_line

TODAY = date(2026, 9, 28)


def item(line):
    return parse_line(line, 0)


class TestTodoFilter:

    def test_default_lets_everything_through(self):
        f = TodoFilter()
        assert not f.is_active
        assert f.matches(item("- [ ] A due:2020-01-01"), TODAY)
        assert f.matches(item("- [ ] B"), TODAY)

    def test_week_includes_overdue_and_seven_days_ahead(self):
        f = TodoFilter(due='week')
        assert f.matches(item("- [ ] Late due:2026-09-01"), TODAY)
        assert f.matches(item("- [ ] In a week due:2026-10-05T23:00"), TODAY)
        assert not f.matches(item("- [ ] Later due:2026-10-06"), TODAY)
        assert not f.matches(item("- [ ] Someday due:9999-12-31"), TODAY)
        assert not f.matches(item("- [ ] Undated"), TODAY)

    def test_include_undated(self):
        f = TodoFilter(due='today', include_undated=True)
        assert f.matches(item("- [ ] Undated"), TODAY)
        assert not f.matches(item("- [ ] Someday due:9999-12-31"), TODAY)

    def test_overdue_and_undated_ranges(self):
        assert TodoFilter(due='overdue').matches(item("- [ ] L due:2026-09-27T23:59"), TODAY)
        assert not TodoFilter(due='overdue').matches(item("- [ ] T due:2026-09-28"), TODAY)
        assert TodoFilter(due='undated').matches(item("- [ ] S due:9999-12-31"), TODAY)
        assert not TodoFilter(due='undated').matches(item("- [ ] D due:2026-09-28"), TODAY)

    def test_projects_any_of_case_insensitive_and_none(self):
        f = TodoFilter().toggled_project('haushalt')
        assert f.matches(item("- [ ] A +Haushalt"), TODAY)
        assert not f.matches(item("- [ ] B +Arbeit"), TODAY)
        f = f.toggled_project(NO_TAG)
        assert f.matches(item("- [ ] C"), TODAY)
        assert f.toggled_project('HAUSHALT').projects == [NO_TAG]

    def test_settings_roundtrip_and_migration(self):
        f = TodoFilter(due='week', include_undated=True, projects=['A'], contexts=[NO_TAG])
        assert TodoFilter.from_settings(f.to_settings()) == f
        old = TodoFilter.from_settings({'show_due_only': '1'})
        assert old == TodoFilter(due='today', include_undated=True)
        assert TodoFilter.from_settings({'filter_due': 'bogus'}).due == 'any'

    def test_from_args(self):
        args = MultiDict([('filter_due', 'month'), ('filter_project', 'A'),
                          ('filter_project', ''), ('filter_context', 'Post')])
        f = TodoFilter.from_args(args)
        assert f == TodoFilter(due='month', projects=['A', ''], contexts=['Post'])
        assert f.active_count == 3

    def test_choices_keep_selected_tags(self):
        todos = [item("- [ ] A +Haushalt @Post"), item("- [x] B +Alt ✅ 2026-09-01")]
        choices = filter_choices(todos, TodoFilter(projects=['Weg']))
        assert choices == {'projects': ['Haushalt', 'Weg'], 'contexts': ['Post']}


class TestFilterRoutes:

    def _login(self, client):
        with client.session_transaction() as sess:
            sess['logged_in'] = True

    def _patch(self, monkeypatch, lines, settings=None):
        items = [parse_line(line, i) for i, line in enumerate(lines)]
        stored = dict(settings or {})
        monkeypatch.setattr('app.routes.main.load_todos', lambda: [i for i in items if i])
        monkeypatch.setattr('app.routes.main.load_settings', lambda: dict(stored))
        monkeypatch.setattr('app.routes.main.save_settings', lambda s: stored.update(s))
        return stored

    def _lines(self):
        today = date.today()
        return [
            f"- [ ] Soon task +Home due:{(today + timedelta(days=3)).isoformat()} ^aaa111",
            f"- [ ] Far task +Home due:{(today + timedelta(days=40)).isoformat()} ^bbb222",
            f"- [ ] Work task +Work due:{today.isoformat()} ^ccc333",
            "- [ ] Undated task ^ddd444",
        ]

    def test_filter_set_applies_and_persists(self, client, monkeypatch):
        self._login(client)
        stored = self._patch(monkeypatch, self._lines())
        html = client.get('/?filter_set=1&filter_due=week&filter_project=home&partial=1',
                          ).get_data(as_text=True)
        assert 'Soon task' in html
        assert 'Far task' not in html
        assert 'Work task' not in html
        assert 'Undated task' not in html
        assert 'filter-chip' in html
        assert stored['filter_due'] == 'week'
        assert stored['filter_projects'] == ['home']

        # Without parameters the stored filter still applies.
        html = client.get('/?partial=1').get_data(as_text=True)
        assert 'Far task' not in html

    def test_old_show_due_only_setting_keeps_undated(self, client, monkeypatch):
        self._login(client)
        self._patch(monkeypatch, self._lines(), settings={'show_due_only': '1'})
        html = client.get('/?partial=1').get_data(as_text=True)
        assert 'Work task' in html
        assert 'Undated task' in html
        assert 'Soon task' not in html

    def test_no_match_offers_reset(self, client, monkeypatch):
        self._login(client)
        self._patch(monkeypatch, self._lines())
        html = client.get('/?filter_set=1&filter_due=overdue&partial=1').get_data(as_text=True)
        assert 'Keine Aufgabe passt zum Filter.' in html
        assert 'data-filter-reset' in html

    def test_filter_narrows_myday_picker(self, client, monkeypatch):
        self._login(client)
        self._patch(monkeypatch, self._lines())
        html = client.get('/?view=myday&filter_set=1&filter_project=Work&partial=1').get_data(as_text=True)
        assert 'Work task' in html
        assert 'Soon task' not in html
