"""Tests for postpone line reconstruction (token preservation)."""

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


@pytest.fixture
def storage(monkeypatch):
    """Patch todo_service storage with an in-memory buffer."""
    mem = _MemoryStorage(
        '- [ ] Weekly review +work due:2026-06-01T12:00 rec:weekly ^aaa111\n'
        '- [ ] Daily standup due:2026-06-01T09:00 rec:daily [[notes]] ~note:"Check agenda" ^bbb222\n'
        '- [ ] Plain task due:2026-06-01T12:00 ^ccc333\n'
    )
    monkeypatch.setattr(todo_service, 'read_content', mem.read)
    monkeypatch.setattr(todo_service, 'write_content', mem.write)
    return mem


def _line(storage, marker):
    return [l for l in storage.content.splitlines() if f'^{marker}' in l][0]


class TestPostponePreservesTokens:
    """postpone_todo / postpone_todos_batch must keep rec:, [[ref]] and notes."""

    def test_postpone_keeps_recurrence(self, storage):
        assert todo_service.postpone_todo(0, 'tomorrow') is True
        line = _line(storage, 'aaa111')
        assert 'rec:weekly' in line
        assert '+work' in line

    def test_postpone_keeps_recurrence_reference_and_note(self, storage):
        assert todo_service.postpone_todo(1, 'weekend') is True
        line = _line(storage, 'bbb222')
        assert 'rec:daily' in line
        assert '[[notes]]' in line
        assert '~note:"Check agenda"' in line

    def test_postpone_token_order_matches_canonical(self, storage):
        # Canonical order: due: → rec: → [[ref]] → ~note: → ^marker
        assert todo_service.postpone_todo(1, 'tomorrow') is True
        line = _line(storage, 'bbb222')
        assert line.index('due:') < line.index('rec:') < line.index('[[') < line.index('~note:') < line.index('^bbb222')

    def test_postpone_without_recurrence_unchanged(self, storage):
        assert todo_service.postpone_todo(2, 'tomorrow') is True
        line = _line(storage, 'ccc333')
        assert 'rec:' not in line

    def test_postpone_batch_keeps_recurrence(self, storage):
        result = todo_service.postpone_todos_batch([0, 1], 'sometime')
        assert result['updated'] == 2
        assert 'rec:weekly' in _line(storage, 'aaa111')
        assert 'rec:daily' in _line(storage, 'bbb222')

    def test_postpone_roundtrips_through_parser(self, storage):
        from app.services.parser import parse_line
        assert todo_service.postpone_todo(0, 'tomorrow') is True
        item = parse_line(_line(storage, 'aaa111'), 0)
        assert item.recurrence == 'weekly'
        assert item.title == 'Weekly review'
        assert item.projects == ['work']
