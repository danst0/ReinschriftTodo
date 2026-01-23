"""Tests for utility functions."""

import pytest
from datetime import datetime

import sys
import os
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.utils.markers import generate_marker, ensure_marker
from app.utils.escaping import escape_note, unescape_note, normalize_note
from app.utils.helpers import parse_flag, format_due, format_due_display, is_sometime


class TestGenerateMarker:
    """Tests for marker generation."""

    def test_default_length(self):
        """Test default marker length."""
        marker = generate_marker()
        assert len(marker) == 8

    def test_custom_length(self):
        """Test custom marker length."""
        marker = generate_marker(12)
        assert len(marker) == 12

    def test_alphanumeric_only(self):
        """Test marker contains only alphanumeric characters."""
        marker = generate_marker()
        assert marker.isalnum()

    def test_uniqueness(self):
        """Test markers are unique."""
        markers = {generate_marker() for _ in range(100)}
        assert len(markers) == 100


class TestEnsureMarker:
    """Tests for ensure_marker function."""

    def test_returns_existing(self):
        """Test returns existing marker."""
        result = ensure_marker("existing123")
        assert result == "existing123"

    def test_generates_new_for_none(self):
        """Test generates new marker for None."""
        result = ensure_marker(None)
        assert result is not None
        assert len(result) == 8

    def test_generates_new_for_empty(self):
        """Test generates new marker for empty string."""
        result = ensure_marker("")
        assert result is not None
        assert len(result) == 8


class TestEscapeNote:
    """Tests for note escaping."""

    def test_escape_quotes(self):
        """Test escaping quotes."""
        result = escape_note('Say "hello"')
        assert result == 'Say \\"hello\\"'

    def test_escape_newlines(self):
        """Test escaping newlines."""
        result = escape_note("Line1\nLine2")
        assert result == "Line1\\nLine2"

    def test_escape_backslashes(self):
        """Test escaping backslashes."""
        result = escape_note("Path\\to\\file")
        assert result == "Path\\\\to\\\\file"

    def test_escape_carriage_return(self):
        """Test escaping carriage returns."""
        result = escape_note("Line1\r\nLine2")
        assert result == "Line1\\r\\nLine2"


class TestUnescapeNote:
    """Tests for note unescaping."""

    def test_unescape_quotes(self):
        """Test unescaping quotes."""
        result = unescape_note('Say \\"hello\\"')
        assert result == 'Say "hello"'

    def test_unescape_newlines(self):
        """Test unescaping newlines."""
        result = unescape_note("Line1\\nLine2")
        assert result == "Line1\nLine2"

    def test_unescape_none(self):
        """Test unescaping None."""
        result = unescape_note(None)
        assert result == ""

    def test_roundtrip(self):
        """Test escape/unescape roundtrip."""
        original = 'Complex "text"\nwith lines'
        escaped = escape_note(original)
        unescaped = unescape_note(escaped)
        assert unescaped == original


class TestNormalizeNote:
    """Tests for note normalization."""

    def test_strips_whitespace(self):
        """Test whitespace stripping."""
        result = normalize_note("  text  ")
        assert result == "text"

    def test_returns_none_for_empty(self):
        """Test returns None for empty."""
        assert normalize_note("") is None
        assert normalize_note("   ") is None

    def test_returns_none_for_none(self):
        """Test returns None for None input."""
        assert normalize_note(None) is None


class TestParseFlag:
    """Tests for parse_flag function."""

    def test_parse_true_values(self):
        """Test parsing truthy values."""
        assert parse_flag('1') is True
        assert parse_flag('true') is True
        assert parse_flag('yes') is True
        assert parse_flag('on') is True
        assert parse_flag(True) is True
        assert parse_flag(1) is True

    def test_parse_false_values(self):
        """Test parsing falsy values."""
        assert parse_flag('0') is False
        assert parse_flag('false') is False
        assert parse_flag('no') is False
        assert parse_flag('off') is False
        assert parse_flag(False) is False
        assert parse_flag(0) is False

    def test_default_for_none(self):
        """Test default value for None."""
        assert parse_flag(None) is True
        assert parse_flag(None, default=False) is False

    def test_case_insensitive(self):
        """Test case insensitivity."""
        assert parse_flag('TRUE') is True
        assert parse_flag('False') is False


class TestFormatDue:
    """Tests for format_due function."""

    def test_format_datetime(self):
        """Test formatting datetime."""
        dt = datetime(2026, 1, 20, 14, 30)
        result = format_due(dt)
        assert result == "2026-01-20T14:30"


class TestFormatDueDisplay:
    """Tests for format_due_display function."""

    def test_with_time(self):
        """Test formatting with non-zero time."""
        dt = datetime(2026, 1, 20, 14, 30)
        result = format_due_display(dt)
        assert result == "2026-01-20T14:30"

    def test_without_time(self):
        """Test formatting with zero time."""
        dt = datetime(2026, 1, 20, 0, 0)
        result = format_due_display(dt)
        assert result == "2026-01-20"


class TestIsSometime:
    """Tests for is_sometime function."""

    def test_sometime_value(self):
        """Test detecting sometime value."""
        dt = datetime(9999, 12, 31, 0, 0)
        assert is_sometime(dt) is True

    def test_normal_date(self):
        """Test normal date is not sometime."""
        dt = datetime(2026, 1, 20)
        assert is_sometime(dt) is False

    def test_none_is_not_sometime(self):
        """Test None is not sometime."""
        assert is_sometime(None) is False
