"""Tests for the date service."""

import pytest
from datetime import datetime, date, timedelta

import sys
import os
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.services.date_service import (
    current_date,
    next_due_date,
    calculate_postpone_date,
    recent_window_bounds,
)


class TestCurrentDate:
    """Tests for current_date function."""

    def test_returns_datetime(self):
        """Test that current_date returns a datetime."""
        result = current_date()
        assert isinstance(result, datetime)

    def test_has_zero_time(self):
        """Test that current_date has zeroed time components."""
        result = current_date()
        assert result.hour == 0
        assert result.minute == 0
        assert result.second == 0
        assert result.microsecond == 0


class TestNextDueDate:
    """Tests for next_due_date function."""

    def test_daily_recurrence(self):
        """Test daily recurrence calculation."""
        base = datetime(2026, 1, 20, 12, 0)
        result = next_due_date(base, 'daily')

        assert result is not None
        assert result.date() > datetime.now().date()
        assert result.hour == 12
        assert result.minute == 0

    def test_weekly_recurrence(self):
        """Test weekly recurrence calculation."""
        base = datetime(2026, 1, 20, 12, 0)
        result = next_due_date(base, 'weekly')

        assert result is not None
        # Should be at least 7 days after today
        assert result.date() > datetime.now().date()
        assert result.hour == 12

    def test_monthly_recurrence(self):
        """Test monthly recurrence calculation."""
        base = datetime(2026, 1, 15, 14, 30)
        result = next_due_date(base, 'monthly')

        assert result is not None
        assert result.date() > datetime.now().date()
        assert result.hour == 14
        assert result.minute == 30

    def test_invalid_recurrence(self):
        """Test invalid recurrence rule returns None."""
        base = datetime(2026, 1, 20)
        result = next_due_date(base, 'invalid')
        assert result is None

    def test_none_current_due(self):
        """Test handling None current_due."""
        result = next_due_date(None, 'daily')
        assert result is not None
        assert result.date() > datetime.now().date()


class TestCalculatePostponeDate:
    """Tests for calculate_postpone_date function."""

    def test_postpone_today(self):
        """Test postpone to today."""
        result_dt, result_time = calculate_postpone_date('today')

        assert result_dt.date() == datetime.now().date()
        # Time should be 12:00 or 18:00 depending on current hour
        assert result_time.hour in [12, 18]

    def test_postpone_tomorrow(self):
        """Test postpone to tomorrow."""
        result_dt, result_time = calculate_postpone_date('tomorrow')

        tomorrow = datetime.now().date() + timedelta(days=1)
        assert result_dt.date() == tomorrow
        assert result_time.hour == 12
        assert result_time.minute == 0

    def test_postpone_weekend(self):
        """Test postpone to weekend."""
        result_dt, result_time = calculate_postpone_date('weekend')

        # Should be a Saturday
        assert result_dt.date().weekday() == 5  # Saturday
        assert result_dt.date() >= datetime.now().date()
        assert result_time.hour == 12

    def test_postpone_sometime(self):
        """Test postpone to sometime."""
        result_dt, result_time = calculate_postpone_date('sometime')

        assert result_dt.year == 9999
        assert result_dt.month == 12
        assert result_dt.day == 31
        assert result_time.hour == 0


class TestRecentWindowBounds:
    """Tests for recent_window_bounds function."""

    def test_default_window(self):
        """Test default 30-day window."""
        today, start = recent_window_bounds()

        assert today.date() == datetime.now().date()
        expected_start = datetime.now().date() - timedelta(days=29)
        assert start.date() == expected_start

    def test_custom_window(self):
        """Test custom window size."""
        today, start = recent_window_bounds(7)

        expected_start = datetime.now().date() - timedelta(days=6)
        assert start.date() == expected_start

    def test_single_day_window(self):
        """Test single day window."""
        today, start = recent_window_bounds(1)
        assert today.date() == start.date()
