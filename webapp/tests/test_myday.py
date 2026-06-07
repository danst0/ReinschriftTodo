"""Tests for the "My Day" (Mein Tag) feature."""

import os
import sys
from datetime import date, datetime, timedelta

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.services.parser import parse_line, parse_myday_token
from app.services import todo_service


TODAY = date.today().strftime("%Y-%m-%d")
YESTERDAY = (date.today() - timedelta(days=1)).strftime("%Y-%m-%d")


class _MemoryStorage:
    """In-memory stand-in for read_content/write_content."""

    def __init__(self, content: str):
        self.content = content

    def read(self) -> str:
        return self.content

    def write(self, content: str) -> None:
        self.content = content


@pytest.fixture
def storage(monkeypatch):
    """Patch todo_service storage with an in-memory buffer."""
    mem = _MemoryStorage(
        f"- [ ] Plan day +work due:2026-12-20T12:00 ^aaa111\n"
        f"- [ ] No due task +home ^bbb222\n"
        f"- [ ] Already planned due:2026-12-21T12:00 myday:{TODAY} ^ccc333\n"
        f"- [ ] Stale plan due:2026-12-22T12:00 myday:{YESTERDAY} ^ddd444\n"
        f"- [ ] Recurring task due:2020-01-01T12:00 myday:{TODAY} rec:daily ^eee555\n"
    )
    monkeypatch.setattr(todo_service, 'read_content', mem.read)
    monkeypatch.setattr(todo_service, 'write_content', mem.write)
    return mem


class TestMydayParsing:
    """Parser-level tests for the myday token."""

    def test_parse_myday_token(self):
        assert parse_myday_token("x myday:2026-06-04 y") == date(2026, 6, 4)
        assert parse_myday_token("no token here") is None

    def test_parse_line_extracts_myday(self):
        item = parse_line(f"- [ ] Task due:2026-12-20T12:00 myday:{TODAY} ^abc123", 0)
        assert item.myday == date.today()
        assert item.in_myday is True
        assert item.title == "Task"

    def test_parse_line_stale_myday_not_in_myday(self):
        item = parse_line(f"- [ ] Task myday:{YESTERDAY} ^abc123", 0)
        assert item.myday == date.today() - timedelta(days=1)
        assert item.in_myday is False

    def test_title_stops_at_myday(self):
        item = parse_line(f"- [ ] Buy groceries myday:{TODAY} ^abc123", 0)
        assert item.title == "Buy groceries"

    def test_to_dict_includes_myday(self):
        item = parse_line(f"- [ ] Task myday:{TODAY} ^abc123", 0)
        d = item.to_dict()
        assert d['myday'] == TODAY
        assert d['in_myday'] is True


class TestSetMyday:
    """Service-level tests for set_myday."""

    def test_add_inserts_after_due(self, storage):
        assert todo_service.set_myday('aaa111', True) is True
        assert (
            f"- [ ] Plan day +work due:2026-12-20T12:00 myday:{TODAY} ^aaa111"
            in storage.content.splitlines()
        )

    def test_add_without_due_inserts_before_marker(self, storage):
        assert todo_service.set_myday('bbb222', True) is True
        assert (
            f"- [ ] No due task +home myday:{TODAY} ^bbb222"
            in storage.content.splitlines()
        )

    def test_add_is_idempotent(self, storage):
        todo_service.set_myday('aaa111', True)
        once = storage.content
        todo_service.set_myday('aaa111', True)
        assert storage.content == once

    def test_add_replaces_stale_date(self, storage):
        assert todo_service.set_myday('ddd444', True) is True
        line = [l for l in storage.content.splitlines() if '^ddd444' in l][0]
        assert f"myday:{TODAY}" in line
        assert YESTERDAY not in line

    def test_remove_strips_token(self, storage):
        assert todo_service.set_myday('ccc333', False) is True
        line = [l for l in storage.content.splitlines() if '^ccc333' in l][0]
        assert 'myday:' not in line
        assert '  ' not in line

    def test_unknown_marker_returns_false(self, storage):
        before = storage.content
        assert todo_service.set_myday('zzz999', True) is False
        assert storage.content == before


class TestMydayPreservation:
    """Line reconstruction must keep active myday tokens and drop stale ones."""

    def test_update_by_marker_keeps_today_myday(self, storage):
        assert todo_service.update_todo_by_marker('ccc333', {'title': 'Renamed'}) is True
        line = [l for l in storage.content.splitlines() if '^ccc333' in l][0]
        assert f"myday:{TODAY}" in line
        assert 'Renamed' in line

    def test_update_by_marker_drops_stale_myday(self, storage):
        assert todo_service.update_todo_by_marker('ddd444', {'title': 'Renamed'}) is True
        line = [l for l in storage.content.splitlines() if '^ddd444' in l][0]
        assert 'myday:' not in line

    def test_postpone_keeps_today_myday(self, storage):
        assert todo_service.postpone_todo(2, 'tomorrow') is True
        line = [l for l in storage.content.splitlines() if '^ccc333' in l][0]
        assert f"myday:{TODAY}" in line

    def test_postpone_drops_stale_myday(self, storage):
        assert todo_service.postpone_todo(3, 'tomorrow') is True
        line = [l for l in storage.content.splitlines() if '^ddd444' in l][0]
        assert 'myday:' not in line

    def test_postpone_batch_keeps_today_myday(self, storage):
        result = todo_service.postpone_todos_batch([2], 'tomorrow')
        assert result['updated'] == 1
        line = [l for l in storage.content.splitlines() if '^ccc333' in l][0]
        assert f"myday:{TODAY}" in line

    def test_recurrence_clone_has_no_myday(self, storage):
        # Completing the overdue recurring task creates a next occurrence.
        todo_service.handle_toggle_with_recurrence(4)
        lines = storage.content.splitlines()
        clones = [l for l in lines if 'Recurring task' in l and '- [ ]' in l]
        assert len(clones) == 1, f"expected one open clone, got: {clones}"
        assert 'myday:' not in clones[0]


class TestAddToMyday:
    """Adding while in the Mein Tag view plans the new todo for today."""

    def test_add_todo_with_myday_flag(self, storage):
        result = todo_service.add_todo('Neue Aufgabe', myday=True)
        line = [l for l in storage.content.splitlines() if f"^{result['marker']}" in l][0]
        assert f"myday:{TODAY}" in line
        item = parse_line(line, 0)
        assert item.in_myday is True
        assert item.title == 'Neue Aufgabe'

    def test_add_todo_without_flag_has_no_myday(self, storage):
        result = todo_service.add_todo('Neue Aufgabe')
        line = [l for l in storage.content.splitlines() if f"^{result['marker']}" in l][0]
        assert 'myday:' not in line

    def test_add_todo_myday_with_inline_due(self, storage):
        result = todo_service.add_todo('Aufgabe due:2026-12-24', myday=True)
        line = [l for l in storage.content.splitlines() if f"^{result['marker']}" in l][0]
        assert 'due:2026-12-24' in line
        assert f"myday:{TODAY}" in line

    def test_api_add_with_myday(self, client, monkeypatch):
        with client.session_transaction() as sess:
            sess['logged_in'] = True
        calls = []
        monkeypatch.setattr(
            'app.routes.api.add_todo',
            lambda title, myday=False: calls.append((title, myday)) or {'marker': 'm1', 'line_index': 0},
        )
        response = client.post('/api/add', json={'title': 'Task', 'myday': True})
        assert response.status_code == 200
        assert calls == [('Task', True)]

    def test_api_add_defaults_to_no_myday(self, client, monkeypatch):
        with client.session_transaction() as sess:
            sess['logged_in'] = True
        calls = []
        monkeypatch.setattr(
            'app.routes.api.add_todo',
            lambda title, myday=False: calls.append((title, myday)) or {'marker': 'm1', 'line_index': 0},
        )
        response = client.post('/api/add', json={'title': 'Task'})
        assert response.status_code == 200
        assert calls == [('Task', False)]

    def test_form_add_with_myday(self, client, monkeypatch):
        with client.session_transaction() as sess:
            sess['logged_in'] = True
        calls = []
        monkeypatch.setattr(
            'app.routes.todo.add_todo',
            lambda title, myday=False: calls.append((title, myday)) or {'marker': 'm1', 'line_index': 0},
        )
        monkeypatch.setattr('app.routes.todo.read_content', lambda: '')
        monkeypatch.setattr('app.routes.todo.push_undo', lambda *a, **k: None)
        response = client.post('/add', data={'title': 'Task', 'myday': '1'})
        assert response.status_code == 302
        assert calls == [('Task', True)]


class TestMydayView:
    """Route-level tests for the ?view=myday index mode."""

    def _login(self, client):
        with client.session_transaction() as sess:
            sess['logged_in'] = True

    def _patch(self, monkeypatch, lines, settings=None):
        from app.services.parser import parse_line
        items = [parse_line(l, i) for i, l in enumerate(lines)]
        saved = {}
        monkeypatch.setattr('app.routes.main.load_todos', lambda: [i for i in items if i])
        monkeypatch.setattr('app.routes.main.load_settings', lambda: dict(settings or {}))
        monkeypatch.setattr('app.routes.main.save_settings', saved.update)
        return saved

    def test_myday_view_shows_only_planned_including_done(self, client, monkeypatch):
        self._login(client)
        self._patch(monkeypatch, [
            f"- [ ] Planned task due:2026-12-20T12:00 myday:{TODAY} ^aaa111",
            f"- [x] Done planned task myday:{TODAY} ✅ {TODAY} ^bbb222",
            "- [ ] Unplanned task due:2020-01-01T12:00 ^ccc333",
        ])
        response = client.get('/?view=myday')
        assert response.status_code == 200
        html = response.get_data(as_text=True)
        assert 'Planned task' in html
        assert 'Done planned task' in html
        # Unplanned task appears only in the picker, not as a todo-item
        assert 'myday-picker' in html

    def test_myday_empty_state_and_picker(self, client, monkeypatch):
        self._login(client)
        self._patch(monkeypatch, [
            "- [ ] Overdue task due:2020-01-01T12:00 ^aaa111",
            "- [ ] Future task due:2099-01-01T12:00 ^bbb222",
            "- [ ] Sometime task due:9999-12-31T00:00 ^ccc333",
        ])
        response = client.get('/?view=myday')
        html = response.get_data(as_text=True)
        assert 'myday-empty' in html
        # Overdue lands in suggestions; future and sometime in other open
        assert html.index('Overdue task') < html.index('Future task')
        assert 'Sometime task' in html

    def test_myday_done_sorted_below_active(self, client, monkeypatch):
        # Completed planned todos move below the active ones, under a
        # "Done" header — even when their date sort would rank them first.
        self._login(client)
        self._patch(monkeypatch, [
            f"- [x] Done planned task myday:{TODAY} ✅ {TODAY} ^aaa111",
            f"- [ ] Active planned task due:2026-12-20T12:00 myday:{TODAY} ^bbb222",
        ])
        response = client.get('/?view=myday&partial=1')
        html = response.get_data(as_text=True)
        assert (html.index('Active planned task')
                < html.index('Erledigt')
                < html.index('Done planned task'))

    def test_myday_picker_grouped_by_topic(self, client, monkeypatch):
        # In topic mode the picker sections are subdivided by project.
        self._login(client)
        self._patch(monkeypatch, [
            "- [ ] Overdue B due:2020-01-02T12:00 +work ^aaa111",
            "- [ ] Overdue A due:2020-01-01T12:00 +home ^bbb222",
            "- [ ] Future task due:2099-01-01T12:00 ^ccc333",
        ], settings={'sort_mode': 'topic'})
        response = client.get('/?view=myday&partial=1')
        html = response.get_data(as_text=True)
        assert 'myday-picker-group' in html
        # Suggestions sorted by project, not by due date
        assert html.index('Thema: home') < html.index('Thema: work')
        assert html.index('Overdue A') < html.index('Overdue B')
        # Item without project falls under "Ohne Projekt"
        assert html.index('Thema: Ohne Projekt') < html.index('Future task')

    def test_myday_picker_grouped_by_location(self, client, monkeypatch):
        self._login(client)
        self._patch(monkeypatch, [
            "- [ ] Overdue task due:2020-01-01T12:00 @office ^aaa111",
        ], settings={'sort_mode': 'location'})
        response = client.get('/?view=myday&partial=1')
        html = response.get_data(as_text=True)
        assert 'Ort: office' in html

    def test_myday_picker_flat_in_date_mode(self, client, monkeypatch):
        # Date mode keeps the flat, due-first picker without sub-headers.
        self._login(client)
        self._patch(monkeypatch, [
            "- [ ] Overdue task due:2020-01-01T12:00 +work ^aaa111",
        ], settings={'sort_mode': 'date'})
        response = client.get('/?view=myday&partial=1')
        html = response.get_data(as_text=True)
        assert 'myday-picker-group' not in html

    def test_myday_partial_returns_fragment(self, client, monkeypatch):
        self._login(client)
        self._patch(monkeypatch, [
            f"- [ ] Planned task myday:{TODAY} ^aaa111",
        ])
        response = client.get('/?view=myday&partial=1')
        html = response.get_data(as_text=True)
        assert 'Planned task' in html
        assert 'myday-picker' in html
        assert '<html' not in html

    def test_view_is_persisted(self, client, monkeypatch):
        self._login(client)
        saved = self._patch(monkeypatch, ["- [ ] Task ^aaa111"])
        client.get('/?view=myday')
        assert saved.get('view') == 'myday'

    def test_persisted_view_used_without_param(self, client, monkeypatch):
        self._login(client)
        self._patch(monkeypatch, [
            f"- [ ] Planned task myday:{TODAY} ^aaa111",
        ], settings={'view': 'myday'})
        response = client.get('/')
        html = response.get_data(as_text=True)
        assert 'myday-picker' in html

    def test_invalid_view_falls_back_to_all(self, client, monkeypatch):
        self._login(client)
        self._patch(monkeypatch, ["- [ ] Task ^aaa111"])
        response = client.get('/?view=bogus')
        html = response.get_data(as_text=True)
        assert response.status_code == 200
        assert 'myday-picker' not in html


class TestMydayApi:
    """Route-level tests for /api/myday/toggle."""

    def _login(self, client):
        with client.session_transaction() as sess:
            sess['logged_in'] = True

    def test_unauthorized(self, client):
        response = client.post('/api/myday/toggle', json={'marker': 'abc', 'on': True})
        assert response.status_code == 401

    def test_marker_required(self, client):
        self._login(client)
        response = client.post('/api/myday/toggle', json={'on': True})
        assert response.status_code == 400

    def test_on_must_be_boolean(self, client):
        self._login(client)
        response = client.post('/api/myday/toggle', json={'marker': 'abc', 'on': 'yes'})
        assert response.status_code == 400

    def test_unknown_marker_404(self, client, monkeypatch):
        self._login(client)
        monkeypatch.setattr('app.routes.api.set_myday', lambda marker, on: False)
        monkeypatch.setattr('app.routes.api.read_content', lambda: '')
        response = client.post('/api/myday/toggle', json={'marker': 'zzz', 'on': True})
        assert response.status_code == 404

    def test_toggle_success(self, client, monkeypatch):
        self._login(client)
        calls = []
        monkeypatch.setattr(
            'app.routes.api.set_myday',
            lambda marker, on: calls.append((marker, on)) or True,
        )
        monkeypatch.setattr('app.routes.api.read_content', lambda: '')
        response = client.post('/api/myday/toggle', json={'marker': 'abc123', 'on': True})
        assert response.status_code == 200
        assert response.get_json() == {'ok': True}
        assert calls == [('abc123', True)]
