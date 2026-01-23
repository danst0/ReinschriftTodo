"""Tests for the parser service."""

import pytest
from datetime import datetime

import sys
import os
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.services.parser import (
    parse_line,
    parse_due_token,
    parse_due_input,
    extract_title,
    capture_token,
    capture_all_tokens,
    find_line_by_marker,
)
from app.models.todo import PROJECT_RE, CONTEXT_RE, ID_RE


class TestParseLine:
    """Tests for parse_line function."""

    def test_parse_basic_todo(self, sample_todo_line):
        """Test parsing a basic todo line."""
        result = parse_line(sample_todo_line, 0)

        assert result is not None
        assert result.title == "Buy groceries"
        assert result.projects == ["shopping"]
        assert result.contexts == ["errands"]
        assert result.marker == "abc123"
        assert result.done is False
        assert result.line_index == 0

    def test_parse_completed_todo(self, sample_done_todo_line):
        """Test parsing a completed todo line."""
        result = parse_line(sample_done_todo_line, 2)

        assert result is not None
        assert result.title == "Finish report"
        assert result.done is True
        assert result.done_date is not None
        assert result.done_date.date() == datetime(2026, 1, 15).date()

    def test_parse_recurring_todo(self, sample_recurring_todo_line):
        """Test parsing a recurring todo line."""
        result = parse_line(sample_recurring_todo_line, 3)

        assert result is not None
        assert result.recurrence == "daily"

    def test_parse_sometime_todo(self, sample_sometime_todo_line):
        """Test parsing a 'sometime' todo line."""
        result = parse_line(sample_sometime_todo_line, 4)

        assert result is not None
        assert result.due is not None
        assert result.due.year == 9999
        assert result.due_is_sometime is True

    def test_parse_note_todo(self, sample_note_todo_line):
        """Test parsing a todo with note."""
        result = parse_line(sample_note_todo_line, 5)

        assert result is not None
        assert result.note == "Discuss Q1 goals"

    def test_parse_non_todo_line(self):
        """Test that non-todo lines return None."""
        assert parse_line("# Header", 0) is None
        assert parse_line("Some text", 0) is None
        assert parse_line("", 0) is None
        assert parse_line("---", 0) is None

    def test_parse_uppercase_x(self):
        """Test parsing completed todo with uppercase X."""
        line = "- [X] Done task ^abc"
        result = parse_line(line, 0)
        assert result is not None
        assert result.done is True

    def test_parse_multiple_projects_contexts(self):
        """Test parsing todo with multiple projects and contexts."""
        line = "- [ ] Task +proj1 +proj2 @ctx1 @ctx2 ^abc"
        result = parse_line(line, 0)

        assert result is not None
        assert result.projects == ["proj1", "proj2"]
        assert result.contexts == ["ctx1", "ctx2"]


class TestParseDue:
    """Tests for due date parsing."""

    def test_parse_due_with_time(self):
        """Test parsing due date with time."""
        result = parse_due_token("due:2026-01-20T14:30")

        assert result is not None
        assert result.year == 2026
        assert result.month == 1
        assert result.day == 20
        assert result.hour == 14
        assert result.minute == 30

    def test_parse_due_without_time(self):
        """Test parsing due date without time."""
        result = parse_due_token("due:2026-01-20")

        assert result is not None
        assert result.year == 2026
        assert result.month == 1
        assert result.day == 20
        assert result.hour == 0
        assert result.minute == 0

    def test_parse_due_no_match(self):
        """Test parsing text without due date."""
        result = parse_due_token("no due date here")
        assert result is None

    def test_parse_due_input_datetime_local(self):
        """Test parsing HTML datetime-local input."""
        result = parse_due_input("2026-01-20T14:30")

        assert result is not None
        assert result.year == 2026
        assert result.hour == 14

    def test_parse_due_input_date_only(self):
        """Test parsing date-only input."""
        result = parse_due_input("2026-01-20")

        assert result is not None
        assert result.hour == 0

    def test_parse_due_input_invalid(self):
        """Test parsing invalid input."""
        assert parse_due_input("invalid") is None
        assert parse_due_input("") is None
        assert parse_due_input(None) is None


class TestExtractTitle:
    """Tests for title extraction."""

    def test_extract_title_basic(self):
        """Test basic title extraction."""
        result = extract_title("Buy groceries +shopping @errands")
        assert result == "Buy groceries"

    def test_extract_title_with_due(self):
        """Test title extraction with due date."""
        result = extract_title("Buy groceries due:2026-01-20")
        assert result == "Buy groceries"

    def test_extract_title_with_marker(self):
        """Test title extraction with marker."""
        result = extract_title("Buy groceries ^abc123")
        assert result == "Buy groceries"

    def test_extract_title_only(self):
        """Test title-only line."""
        result = extract_title("Just a title")
        assert result == "Just a title"

    def test_extract_title_leading_tags(self):
        """Test title with leading tags."""
        result = extract_title("+proj @ctx Real title")
        assert result == "Real title"


class TestCaptureTokens:
    """Tests for token capture functions."""

    def test_capture_single_token(self):
        """Test capturing single token."""
        result = capture_token(ID_RE, "task ^abc123")
        assert result == "abc123"

    def test_capture_no_match(self):
        """Test capture with no match."""
        result = capture_token(ID_RE, "no marker here")
        assert result is None

    def test_capture_all_projects(self):
        """Test capturing all projects."""
        result = capture_all_tokens(PROJECT_RE, "+proj1 text +proj2")
        assert result == ["proj1", "proj2"]

    def test_capture_all_contexts(self):
        """Test capturing all contexts."""
        result = capture_all_tokens(CONTEXT_RE, "@ctx1 text @ctx2")
        assert result == ["ctx1", "ctx2"]


class TestFindLineByMarker:
    """Tests for finding lines by marker."""

    def test_find_existing_marker(self):
        """Test finding an existing marker."""
        lines = [
            "- [ ] Task one ^abc",
            "- [ ] Task two ^def",
            "- [ ] Task three ^ghi",
        ]
        result = find_line_by_marker(lines, "def")
        assert result == 1

    def test_find_nonexistent_marker(self):
        """Test finding a non-existent marker."""
        lines = ["- [ ] Task ^abc"]
        result = find_line_by_marker(lines, "xyz")
        assert result is None

    def test_find_marker_empty_lines(self):
        """Test finding marker in empty list."""
        result = find_line_by_marker([], "abc")
        assert result is None
