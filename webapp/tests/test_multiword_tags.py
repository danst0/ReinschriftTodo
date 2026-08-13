"""Round-trip tests for multi-word projects/contexts (issue #9).

The edit form shows tags as `+Big Project`, so saving that back must yield one
tag — not one per word. See `split_tag_input` for the rule.
"""

import os
import sys

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.services import todo_service
from app.services.parser import parse_line


class _MemoryStorage:
    """In-memory stand-in for read_content/write_content."""

    def __init__(self, content: str):
        self.content = content

    def read(self) -> str:
        return self.content

    def write(self, content: str) -> None:
        self.content = content


CONTENT = (
    '- [ ] Unterlagen sortieren +"Big Project" @"Home Office" '
    'due:2026-06-01T12:00 ^aaa111\n'
    '- [ ] Zwei +steuer @keller due:2026-06-02T09:00 ^bbb222\n'
)


@pytest.fixture
def storage(monkeypatch):
    """Patch todo_service storage with an in-memory buffer."""
    mem = _MemoryStorage(CONTENT)
    monkeypatch.setattr(todo_service, 'read_content', mem.read)
    monkeypatch.setattr(todo_service, 'write_content', mem.write)
    return mem


def _line_for(mem, marker):
    return next(l for l in mem.content.splitlines() if f'^{marker}' in l)


class TestEditRoundTrip:
    """Saving the dialog's own rendering must not change the tags."""

    def test_unchanged_save_keeps_multi_word_tags(self, storage):
        """The regression from issue #9: open, save, tags shredded."""
        # What the edit form shows for this todo:
        assert todo_service.update_todo_by_marker('aaa111', {
            'projects': '+Big Project',
            'contexts': '@Home Office',
        })
        item = parse_line(_line_for(storage, 'aaa111'), 0)
        assert item.projects == ['Big Project']
        assert item.contexts == ['Home Office']

    def test_stored_line_uses_quoted_form(self, storage):
        """Multi-word names are written back quoted so the parser round-trips."""
        todo_service.update_todo_by_marker('aaa111', {'projects': '+Big Project'})
        assert '+"Big Project"' in _line_for(storage, 'aaa111')

    def test_several_tags_split_at_prefix(self, storage):
        """`+` delimits, so two multi-word projects stay two tags."""
        todo_service.update_todo_by_marker('bbb222', {
            'projects': '+Big Project +Kleines Ding',
            'contexts': '@Home Office @Im Auto',
        })
        item = parse_line(_line_for(storage, 'bbb222'), 0)
        assert item.projects == ['Big Project', 'Kleines Ding']
        assert item.contexts == ['Home Office', 'Im Auto']

    def test_legacy_whitespace_input_still_splits(self, storage):
        """Input without any prefix keeps the historical behaviour."""
        todo_service.update_todo_by_marker('bbb222', {
            'projects': 'steuer buero',
            'contexts': 'keller garten',
        })
        item = parse_line(_line_for(storage, 'bbb222'), 0)
        assert item.projects == ['steuer', 'buero']
        assert item.contexts == ['keller', 'garten']

    def test_list_input_is_untouched(self, storage):
        """A JSON list of names is taken verbatim, spaces included."""
        todo_service.update_todo_by_marker('bbb222', {
            'projects': ['Big Project'],
            'contexts': ['Home Office'],
        })
        item = parse_line(_line_for(storage, 'bbb222'), 0)
        assert item.projects == ['Big Project']
        assert item.contexts == ['Home Office']
