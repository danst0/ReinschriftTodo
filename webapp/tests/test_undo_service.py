"""Tests for the session-based undo service."""

import sys
import os
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.services.undo_service import push_undo, pop_undo, can_undo, MAX_UNDO_DEPTH


class TestPushUndo:
    """Tests for push_undo."""

    def test_push_single_entry(self, app):
        """Test pushing a single undo entry."""
        with app.test_request_context():
            from flask import session
            push_undo('content1', 'toggle')
            stack = session.get('_undo_stack', [])
            assert len(stack) == 1
            assert stack[0]['content'] == 'content1'
            assert stack[0]['description'] == 'toggle'
            # Not stamped until the mutation actually reached storage.
            assert stack[0]['expected_hash'] is None

    def test_push_multiple_entries(self, app):
        """Test pushing multiple entries preserves order."""
        with app.test_request_context():
            from flask import session
            push_undo('first', 'add')
            push_undo('second', 'delete')
            push_undo('third', 'edit')
            stack = session.get('_undo_stack', [])
            assert len(stack) == 3
            assert stack[0]['description'] == 'add'
            assert stack[2]['description'] == 'edit'

    def test_push_respects_max_depth(self, app):
        """Test that stack is capped at MAX_UNDO_DEPTH."""
        with app.test_request_context():
            from flask import session
            for i in range(MAX_UNDO_DEPTH + 3):
                push_undo(f'content{i}', f'action{i}')
            stack = session.get('_undo_stack', [])
            assert len(stack) == MAX_UNDO_DEPTH
            # Oldest entries should be discarded
            assert stack[0]['description'] == 'action3'
            assert stack[-1]['description'] == f'action{MAX_UNDO_DEPTH + 2}'


class TestPopUndo:
    """Tests for pop_undo."""

    def test_pop_returns_last_entry(self, app):
        """Test popping returns the most recent entry."""
        with app.test_request_context():
            push_undo('first', 'add')
            push_undo('second', 'delete')
            entry = pop_undo()
            assert entry['content'] == 'second'
            assert entry['description'] == 'delete'

    def test_pop_removes_entry(self, app):
        """Test popping removes the entry from the stack."""
        with app.test_request_context():
            from flask import session
            push_undo('first', 'add')
            push_undo('second', 'delete')
            pop_undo()
            stack = session.get('_undo_stack', [])
            assert len(stack) == 1
            assert stack[0]['description'] == 'add'

    def test_pop_empty_stack_returns_none(self, app):
        """Test popping from an empty stack returns None."""
        with app.test_request_context():
            assert pop_undo() is None

    def test_pop_all_entries(self, app):
        """Test popping all entries empties the stack."""
        with app.test_request_context():
            push_undo('a', 'first')
            push_undo('b', 'second')
            assert pop_undo() is not None
            assert pop_undo() is not None
            assert pop_undo() is None


class TestCanUndo:
    """Tests for can_undo."""

    def test_empty_stack_returns_false(self, app):
        """Test can_undo is False with empty stack."""
        with app.test_request_context():
            assert can_undo() is False

    def test_non_empty_stack_returns_true(self, app):
        """Test can_undo is True after push."""
        with app.test_request_context():
            push_undo('content', 'action')
            assert can_undo() is True

    def test_false_after_popping_all(self, app):
        """Test can_undo is False after popping all entries."""
        with app.test_request_context():
            push_undo('content', 'action')
            pop_undo()
            assert can_undo() is False
