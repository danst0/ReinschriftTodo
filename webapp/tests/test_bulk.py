"""Tests for bulk operations (multi-select mass actions, issue #8)."""

import os
import sys

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.services import todo_service
from app.services.undo_service import stamp_last_result


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
    '- [ ] Eins +work due:2026-06-01T12:00 ^aaa111\n'
    '- [ ] Zwei due:2026-06-02T09:00 ^bbb222\n'
    '- [x] Drei +work due:2026-06-01T10:00 ✅ 2026-06-01 ^ccc333\n'
    '- [ ] Gießen due:2020-01-01T09:00 rec:daily ^ddd444\n'
    '- [ ] Vier ~note:"Mit Notiz" ^eee555\n'
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


def _line(storage, marker):
    return [l for l in storage.content.splitlines() if f'^{marker}' in l][0]


class TestDeleteTodosBatch:
    """delete_todos_batch removes the right lines in one write."""

    def test_deletes_correct_lines_bottom_up(self, storage):
        # Ascending indexes [0, 2]: naive in-order removal would delete
        # "Eins" and then the *shifted* line after it.
        result = todo_service.delete_todos_batch([0, 2])
        assert result == {'updated': 2, 'failed': []}
        lines = storage.content.splitlines()
        assert '^aaa111' not in storage.content
        assert '^ccc333' not in storage.content
        assert '^bbb222' in storage.content
        assert '^ddd444' in storage.content
        assert lines[-2:] == ['---', '## Archive']

    def test_out_of_range_and_negative_fail(self, storage):
        result = todo_service.delete_todos_batch([1, 99, -1])
        assert result['updated'] == 1
        assert set(result['failed']) == {99, -1}
        assert '^bbb222' not in storage.content

    def test_duplicate_indexes_delete_once(self, storage):
        result = todo_service.delete_todos_batch([0, 0])
        assert result['updated'] == 1
        assert '^bbb222' in storage.content


class TestToggleTodosBatch:
    """toggle_todos_batch flips completion in one write."""

    def test_completes_multiple(self, storage):
        result = todo_service.toggle_todos_batch([0, 1], True)
        assert result == {'updated': 2, 'failed': []}
        assert _line(storage, 'aaa111').startswith('- [x]')
        assert _line(storage, 'bbb222').startswith('- [x]')
        assert '✅' in _line(storage, 'aaa111')

    def test_reopens_multiple(self, storage):
        result = todo_service.toggle_todos_batch([2], False)
        assert result['updated'] == 1
        line = _line(storage, 'ccc333')
        assert line.startswith('- [ ]')
        assert '✅' not in line

    def test_skips_invalid_indexes(self, storage):
        result = todo_service.toggle_todos_batch([0, 42], True)
        assert result['updated'] == 1
        assert result['failed'] == [42]

    def test_completing_recurring_spawns_next_occurrence(self, storage):
        from datetime import date, timedelta

        result = todo_service.toggle_todos_batch([3], True)
        assert result['updated'] == 1

        # Overdue occurrence rescheduled to today and completed.
        completed = _line(storage, 'ddd444')
        today = date.today().strftime('%Y-%m-%d')
        assert completed.startswith('- [x]')
        assert f'due:{today}T09:00' in completed

        # Next occurrence appended before the '---' separator, open, with
        # a fresh marker and the recurrence preserved.
        lines = storage.content.splitlines()
        separator = lines.index('---')
        spawned = lines[separator - 1]
        assert spawned.startswith('- [ ] Gießen')
        assert 'rec:daily' in spawned
        assert '^ddd444' not in spawned
        tomorrow = (date.today() + timedelta(days=1)).strftime('%Y-%m-%d')
        assert f'due:{tomorrow}T09:00' in spawned

    def test_reopen_does_not_spawn(self, storage):
        before = len(storage.content.splitlines())
        todo_service.toggle_todos_batch([3], False)
        assert len(storage.content.splitlines()) == before


class TestAssignTodosBatch:
    """assign_todos_batch adds projects/contexts without duplicating."""

    def test_assigns_project_and_context(self, storage):
        result = todo_service.assign_todos_batch([0, 1], project='steuer', context='buero')
        assert result == {'updated': 2, 'failed': []}
        for marker in ('aaa111', 'bbb222'):
            line = _line(storage, marker)
            assert '+steuer' in line
            assert '@buero' in line

    def test_does_not_duplicate_existing_project(self, storage):
        result = todo_service.assign_todos_batch([0], project='work')
        assert result['updated'] == 0
        assert _line(storage, 'aaa111').count('+work') == 1

    def test_preserves_note_and_tokens(self, storage):
        result = todo_service.assign_todos_batch([4], context='zuhause')
        assert result['updated'] == 1
        line = _line(storage, 'eee555')
        assert '~note:"Mit Notiz"' in line
        assert '@zuhause' in line

    def test_without_project_or_context_is_noop(self, storage):
        result = todo_service.assign_todos_batch([0])
        assert result['updated'] == 0
        assert storage.content == CONTENT

    def test_quotes_multiword_project(self, storage):
        result = todo_service.assign_todos_batch([1], project='Steuererklärung 2026')
        assert result['updated'] == 1
        assert '+"Steuererklärung 2026"' in _line(storage, 'bbb222')


class TestBulkRoutes:
    """Integration tests for the batch API endpoints."""

    @pytest.fixture
    def logged_in_client(self, client, storage, monkeypatch):
        from app.routes import api as api_routes
        monkeypatch.setattr(api_routes, 'read_content', storage.read)
        with client.session_transaction() as sess:
            sess['logged_in'] = True
        return client

    def test_delete_batch(self, logged_in_client, storage):
        response = logged_in_client.post('/api/delete-batch', json={'line_indexes': [0, 1]})
        assert response.status_code == 200
        data = response.get_json()
        assert data['ok'] is True
        assert data['updated'] == 2
        assert '^aaa111' not in storage.content

    def test_delete_batch_empty_list(self, logged_in_client):
        response = logged_in_client.post('/api/delete-batch', json={'line_indexes': []})
        assert response.status_code == 400

    def test_delete_batch_undo_restores(self, logged_in_client, storage):
        from app.services.undo_service import UNDO_SESSION_KEY, apply_undo
        logged_in_client.post('/api/delete-batch', json={'line_indexes': [0, 2]})
        assert '^aaa111' not in storage.content
        with logged_in_client.session_transaction() as sess:
            stack = sess.get(UNDO_SESSION_KEY, [])
        # Exactly one undo entry per batch, naming both deleted lines.
        assert len(stack) == 1
        assert stack[0]['description'] == 'delete'
        assert len(stack[0]['ops']) == 2
        restored, skipped = apply_undo(stack[0], storage.content)
        assert restored == CONTENT
        assert skipped == 0

    def test_toggle_batch(self, logged_in_client, storage):
        response = logged_in_client.post(
            '/api/toggle-batch', json={'line_indexes': [0, 1], 'done': True})
        assert response.status_code == 200
        assert response.get_json()['updated'] == 2
        assert _line(storage, 'aaa111').startswith('- [x]')

    def test_toggle_batch_requires_boolean_done(self, logged_in_client):
        response = logged_in_client.post(
            '/api/toggle-batch', json={'line_indexes': [0], 'done': 'yes'})
        assert response.status_code == 400

    def test_assign_batch(self, logged_in_client, storage):
        response = logged_in_client.post(
            '/api/assign-batch', json={'line_indexes': [1], 'project': 'steuer'})
        assert response.status_code == 200
        assert response.get_json()['updated'] == 1
        assert '+steuer' in _line(storage, 'bbb222')

    def test_assign_batch_requires_project_or_context(self, logged_in_client):
        response = logged_in_client.post(
            '/api/assign-batch', json={'line_indexes': [0]})
        assert response.status_code == 400

    def test_batch_unauthorized(self, client):
        for endpoint in ('/api/delete-batch', '/api/toggle-batch', '/api/assign-batch'):
            response = client.post(endpoint, json={'line_indexes': [0]})
            assert response.status_code == 401, endpoint
