"""Tests for the session-based undo service."""

import sys
import os
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.services.undo_service import (
    MAX_UNDO_DEPTH, can_undo, pop_undo, push_undo, stamp_last_result,
)


def record(before, after, description):
    """Put one completed mutation on the stack.

    An entry is only born once the mutation reaches storage, so a test that
    wants one has to show both sides of the change — that pairing is what
    keeps a half-finished action from leaving a restorable state behind.
    """
    push_undo(before, description)
    stamp_last_result(after)


def tick(marker, done=False):
    box = 'x' if done else ' '
    return f'- [{box}] Task {marker} ^{marker}\n'


class TestPushUndo:
    """Tests for push_undo."""

    def test_push_single_entry(self, app):
        """A completed mutation becomes one entry describing the changed line."""
        with app.test_request_context():
            from flask import session
            record(tick('aaa1'), tick('aaa1', done=True), 'toggle')
            stack = session.get('_undo_stack', [])
            assert len(stack) == 1
            assert stack[0]['description'] == 'toggle'
            assert [op['marker'] for op in stack[0]['ops']] == ['aaa1']

    def test_push_without_a_write_stores_nothing(self, app):
        """Until the mutation lands there is nothing to undo."""
        with app.test_request_context():
            from flask import session
            push_undo(tick('aaa1'), 'toggle')
            assert session.get('_undo_stack', []) == []

    def test_a_write_that_changed_nothing_stores_nothing(self, app):
        with app.test_request_context():
            from flask import session
            record(tick('aaa1'), tick('aaa1'), 'toggle')
            assert session.get('_undo_stack', []) == []

    def test_push_multiple_entries(self, app):
        """Test pushing multiple entries preserves order."""
        with app.test_request_context():
            from flask import session
            record(tick('aaa1'), '', 'add')
            record(tick('bbb2'), '', 'delete')
            record(tick('ccc3'), '', 'edit')
            stack = session.get('_undo_stack', [])
            assert len(stack) == 3
            assert stack[0]['description'] == 'add'
            assert stack[2]['description'] == 'edit'

    def test_push_respects_max_depth(self, app):
        """Test that stack is capped at MAX_UNDO_DEPTH."""
        with app.test_request_context():
            from flask import session
            for i in range(MAX_UNDO_DEPTH + 3):
                record(tick(f'm{i}'), tick(f'm{i}', done=True), f'action{i}')
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
            record(tick('aaa1'), '', 'add')
            record(tick('bbb2'), '', 'delete')
            entry = pop_undo()
            assert entry['description'] == 'delete'
            assert [op['marker'] for op in entry['ops']] == ['bbb2']

    def test_pop_removes_entry(self, app):
        """Test popping removes the entry from the stack."""
        with app.test_request_context():
            from flask import session
            record(tick('aaa1'), '', 'add')
            record(tick('bbb2'), '', 'delete')
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
            record(tick('aaa1'), '', 'first')
            record(tick('bbb2'), '', 'second')
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
        """Test can_undo is True after a completed mutation."""
        with app.test_request_context():
            record(tick('aaa1'), tick('aaa1', done=True), 'action')
            assert can_undo() is True

    def test_false_after_popping_all(self, app):
        """Test can_undo is False after popping all entries."""
        with app.test_request_context():
            record(tick('aaa1'), tick('aaa1', done=True), 'action')
            pop_undo()
            assert can_undo() is False
