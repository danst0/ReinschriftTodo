"""Tests for the sorting service."""

import pytest
from datetime import datetime

import sys
import os
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.services.sorting import (
    sort_key_topic,
    sort_key_location,
    sort_key_date,
    sort_todos,
)


@pytest.fixture
def sample_todos():
    """Sample todos for sorting tests."""
    return [
        {
            'title': 'Task A',
            'projects': ['beta'],
            'contexts': ['home'],
            'due': datetime(2026, 1, 25),
        },
        {
            'title': 'Task B',
            'projects': ['alpha'],
            'contexts': ['work'],
            'due': datetime(2026, 1, 20),
        },
        {
            'title': 'Task C',
            'projects': [],
            'contexts': ['home'],
            'due': None,
        },
        {
            'title': 'Task D',
            'projects': ['alpha'],
            'contexts': [],
            'due': datetime(2026, 1, 22),
        },
    ]


class TestSortKeyTopic:
    """Tests for topic (project) sorting."""

    def test_items_with_project_first(self):
        """Test that items with project sort before items without."""
        with_project = {'projects': ['alpha'], 'contexts': [], 'title': 'A'}
        without_project = {'projects': [], 'contexts': [], 'title': 'A'}

        assert sort_key_topic(with_project) < sort_key_topic(without_project)

    def test_alphabetical_project_order(self):
        """Test projects are sorted alphabetically."""
        alpha = {'projects': ['alpha'], 'contexts': [], 'title': 'X'}
        beta = {'projects': ['beta'], 'contexts': [], 'title': 'X'}

        assert sort_key_topic(alpha) < sort_key_topic(beta)

    def test_case_insensitive(self):
        """Test case-insensitive sorting."""
        upper = {'projects': ['ALPHA'], 'contexts': [], 'title': 'X'}
        lower = {'projects': ['alpha'], 'contexts': [], 'title': 'X'}

        # Should be equal when compared case-insensitively
        assert sort_key_topic(upper) == sort_key_topic(lower)


class TestSortKeyLocation:
    """Tests for location (context) sorting."""

    def test_items_with_context_first(self):
        """Test that items with context sort before items without."""
        with_context = {'projects': [], 'contexts': ['home'], 'title': 'A'}
        without_context = {'projects': [], 'contexts': [], 'title': 'A'}

        assert sort_key_location(with_context) < sort_key_location(without_context)

    def test_alphabetical_context_order(self):
        """Test contexts are sorted alphabetically."""
        home = {'projects': [], 'contexts': ['home'], 'title': 'X'}
        work = {'projects': [], 'contexts': ['work'], 'title': 'X'}

        assert sort_key_location(home) < sort_key_location(work)


class TestSortKeyDate:
    """Tests for date sorting."""

    def test_items_without_due_first(self):
        """Test that items without due date sort first."""
        with_due = {'projects': [], 'contexts': [], 'title': 'A', 'due': datetime(2026, 1, 20)}
        without_due = {'projects': [], 'contexts': [], 'title': 'A', 'due': None}

        assert sort_key_date(without_due) < sort_key_date(with_due)

    def test_earlier_dates_first(self):
        """Test earlier dates sort before later dates."""
        earlier = {'projects': [], 'contexts': [], 'title': 'A', 'due': datetime(2026, 1, 20)}
        later = {'projects': [], 'contexts': [], 'title': 'A', 'due': datetime(2026, 1, 25)}

        assert sort_key_date(earlier) < sort_key_date(later)


class TestSortTodos:
    """Tests for sort_todos function."""

    def test_sort_by_topic(self, sample_todos):
        """Test sorting by topic."""
        result = sort_todos(sample_todos, 'topic')

        # Alpha project should come first
        assert result[0]['projects'] == ['alpha']
        # Items without project should be last
        assert result[-1]['projects'] == []

    def test_sort_by_location(self, sample_todos):
        """Test sorting by location."""
        result = sort_todos(sample_todos, 'location')

        # Home context should come first (alphabetically)
        assert result[0]['contexts'] == ['home']
        # Items without context should be last
        assert result[-1]['contexts'] == []

    def test_sort_by_date(self, sample_todos):
        """Test sorting by date."""
        result = sort_todos(sample_todos, 'date')

        # Items without due date should be first
        assert result[0]['due'] is None
        # Then sorted by date
        assert result[1]['due'] == datetime(2026, 1, 20)

    def test_invalid_sort_mode_defaults_to_topic(self, sample_todos):
        """Test that invalid sort mode defaults to topic sorting."""
        result = sort_todos(sample_todos, 'invalid')

        # Should behave like topic sort
        expected = sort_todos(sample_todos, 'topic')
        assert [t['title'] for t in result] == [t['title'] for t in expected]
