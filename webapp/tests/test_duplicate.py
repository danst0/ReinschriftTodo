"""Tests for task duplication via title suggestions (issue #6)."""

import os
import sys
from datetime import date

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.services import todo_service
from app.services.undo_service import stamp_last_result
from app.services.parser import parse_line


class _MemoryStorage:
    """In-memory stand-in for read_content/write_content."""

    def __init__(self, content: str):
        self.content = content

    def read(self) -> str:
        return self.content

    def write(self, content: str) -> None:
        self.content = content
        # The real write path is where an undo entry is recorded, so a
        # stand-in that skipped this would hide undo bugs from these tests.
        stamp_last_result(content)


CONTENT = (
    '- [ ] Einkaufen +haushalt @laden due:2026-06-01T12:00 ^aaa111\n'
    '- [x] Steuererklärung +"Steuer 2025" rec:monthly [[ref]] ~note:"Belege prüfen" '
    'due:2026-05-01T09:00 myday:2026-05-01 ✅ 2026-05-02 ^bbb222\n'
    '- [ ] Einkaufen +haushalt due:2026-06-03T12:00 ^ccc333\n'
    '---\n'
    '## Archive\n'
)


@pytest.fixture
def storage(monkeypatch):
    """Patch todo_service storage with an in-memory buffer."""
    mem = _MemoryStorage(CONTENT)
    monkeypatch.setattr(todo_service, 'read_content', mem.read)
    monkeypatch.setattr(todo_service, 'write_content', mem.write)
    return mem


class TestDuplicateTodo:
    """duplicate_todo copies metadata into a fresh open task."""

    def test_duplicates_with_metadata(self, storage):
        result = todo_service.duplicate_todo('bbb222')
        assert result is not None
        assert result['marker'] != 'bbb222'

        lines = storage.content.splitlines()
        copy_line = lines[result['line_index']]
        copy = parse_line(copy_line, result['line_index'])

        assert copy.title == 'Steuererklärung'
        assert copy.projects == ['Steuer 2025']
        assert copy.recurrence == 'monthly'
        assert copy.reference == 'ref'
        assert copy.note == 'Belege prüfen'
        # Copy is open, due today (default), not planned for "my day".
        assert copy.done is False
        assert copy.done_date is None
        assert copy.due.date() == date.today()
        assert copy.myday is None
        assert copy.marker == result['marker']

    def test_inserts_before_separator(self, storage):
        result = todo_service.duplicate_todo('aaa111')
        lines = storage.content.splitlines()
        assert lines[result['line_index']].startswith('- [ ] Einkaufen')
        assert lines.index('---') == result['line_index'] + 1

    def test_source_remains_untouched(self, storage):
        todo_service.duplicate_todo('bbb222')
        assert ('- [x] Steuererklärung +"Steuer 2025" rec:monthly [[ref]] '
                '~note:"Belege prüfen" due:2026-05-01T09:00') in storage.content

    def test_unknown_marker_returns_none(self, storage):
        assert todo_service.duplicate_todo('missing') is None
        assert storage.content == CONTENT


class TestDuplicateRoutes:
    """Integration tests for /api/duplicate and suggestion markers."""

    @pytest.fixture
    def logged_in_client(self, client, storage, monkeypatch):
        from app.routes import api as api_routes
        monkeypatch.setattr(api_routes, 'read_content', storage.read)
        with client.session_transaction() as sess:
            sess['logged_in'] = True
        return client

    def test_duplicate_route(self, logged_in_client, storage):
        response = logged_in_client.post('/api/duplicate', json={'marker': 'aaa111'})
        assert response.status_code == 200
        data = response.get_json()
        assert data['ok'] is True
        assert data['marker'] and data['marker'] != 'aaa111'
        assert f"^{data['marker']}" in storage.content

    def test_duplicate_requires_marker(self, logged_in_client):
        response = logged_in_client.post('/api/duplicate', json={})
        assert response.status_code == 400

    def test_duplicate_unknown_marker_404(self, logged_in_client, storage):
        response = logged_in_client.post('/api/duplicate', json={'marker': 'missing'})
        assert response.status_code == 404
        assert storage.content == CONTENT

    def test_duplicate_pushes_undo(self, logged_in_client, storage):
        from app.services.undo_service import UNDO_SESSION_KEY, apply_undo
        logged_in_client.post('/api/duplicate', json={'marker': 'aaa111'})
        with logged_in_client.session_transaction() as sess:
            stack = sess.get(UNDO_SESSION_KEY, [])
        assert len(stack) == 1
        # The entry names only the new line, so undoing removes just that one.
        assert len(stack[0]['ops']) == 1
        assert stack[0]['ops'][0]['before'] is None
        restored, _ = apply_undo(stack[0], storage.content)
        assert restored == CONTENT

    def test_suggestions_include_markers(self, logged_in_client, monkeypatch, storage):
        from app.routes import api as api_routes
        monkeypatch.setattr(
            api_routes, 'load_todos',
            lambda: [parse_line(line, i) for i, line in enumerate(storage.content.splitlines())
                     if parse_line(line, i)],
        )
        response = logged_in_client.get('/api/title-suggestions')
        assert response.status_code == 200
        data = response.get_json()
        # Backwards-compatible title list plus marker-bearing items.
        assert data['titles'][0] == 'Einkaufen'
        by_title = {item['title']: item['marker'] for item in data['items']}
        # Most recently added occurrence wins as the duplicate template.
        assert by_title['Einkaufen'] == 'ccc333'
        assert by_title['Steuererklärung'] == 'bbb222'

    def test_duplicate_unauthorized(self, client):
        response = client.post('/api/duplicate', json={'marker': 'aaa111'})
        assert response.status_code == 401
