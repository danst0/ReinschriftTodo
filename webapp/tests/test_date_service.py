"""Tests for the date service."""

import pytest
from datetime import datetime, date, timedelta, time as dtime

import sys
import os
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.services.date_service import (
    current_date,
    next_due_date,
    next_saturday,
    calculate_postpone_date,
    resolve_postpone_target,
    resolve_postpone_target_batch,
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

    def test_postpone_day_after_tomorrow(self):
        """Test the second stage of the "Morgen" button."""
        result_dt, result_time = calculate_postpone_date('day_after_tomorrow')

        day_after_tomorrow = datetime.now().date() + timedelta(days=2)
        assert result_dt.date() == day_after_tomorrow
        assert result_time.hour == 12
        assert result_time.minute == 0

    def test_postpone_weekend_after_next(self):
        """Test the second stage of the "Nächstes Wochenende" button."""
        result_dt, result_time = calculate_postpone_date('weekend_after_next')

        expected = next_saturday(datetime.now().date()) + timedelta(days=7)
        assert result_dt.date() == expected
        assert result_dt.date().weekday() == 5  # Saturday
        assert result_time.hour == 12


class TestNextSaturday:
    """Tests for next_saturday (mirror of core weekend_date)."""

    def test_monday_to_friday_look_at_the_coming_saturday(self):
        # 2026-10-19 is a Monday.
        monday = date(2026, 10, 19)
        assert next_saturday(monday) == date(2026, 10, 24)
        assert next_saturday(date(2026, 10, 23)) == date(2026, 10, 24), "Friday looks at tomorrow"

    def test_weekend_looks_a_week_ahead(self):
        saturday = date(2026, 10, 24)
        assert next_saturday(saturday) == date(2026, 10, 31)
        assert next_saturday(date(2026, 10, 25)) == date(2026, 10, 31), "Sunday, too"


class TestResolvePostponeTarget:
    """Tests for the second-press stage of "Morgen"/"Nächstes Wochenende"."""

    def test_tomorrow_bumps_to_day_after_tomorrow_and_back(self):
        today = datetime.now().date()
        noon = dtime(12, 0)

        first = datetime.combine(today + timedelta(days=1), noon)
        second = datetime.combine(today + timedelta(days=2), noon)

        assert resolve_postpone_target('tomorrow', first) == 'day_after_tomorrow'
        assert resolve_postpone_target('tomorrow', second) == 'tomorrow'

    def test_tomorrow_stays_for_any_other_date(self):
        today = datetime.now().date()
        noon = dtime(12, 0)

        assert resolve_postpone_target('tomorrow', None) == 'tomorrow'
        assert resolve_postpone_target('tomorrow', datetime.combine(today, noon)) == 'tomorrow'
        later = datetime.combine(today + timedelta(days=5), noon)
        assert resolve_postpone_target('tomorrow', later) == 'tomorrow'

    def test_time_of_day_does_not_matter(self):
        today = datetime.now().date()
        evening = datetime.combine(today + timedelta(days=1), dtime(23, 30))

        assert resolve_postpone_target('tomorrow', evening) == 'day_after_tomorrow'

    def test_weekend_bumps_only_from_the_coming_saturday(self):
        today = datetime.now().date()
        noon = dtime(12, 0)
        coming = datetime.combine(next_saturday(today), noon)
        after_next = datetime.combine(next_saturday(today) + timedelta(days=7), noon)

        assert resolve_postpone_target('weekend', coming) == 'weekend_after_next'
        assert resolve_postpone_target('weekend', after_next) == 'weekend'

    def test_weekend_stays_for_any_other_date(self):
        today = datetime.now().date()
        noon = dtime(12, 0)

        assert resolve_postpone_target('weekend', None) == 'weekend'
        assert resolve_postpone_target('weekend', datetime.combine(today, noon)) == 'weekend'

    def test_other_targets_pass_through(self):
        today = datetime.now().date()
        coming = datetime.combine(next_saturday(today), dtime(12, 0))

        assert resolve_postpone_target('today', coming) == 'today'
        assert resolve_postpone_target('sometime', coming) == 'sometime'


class TestResolvePostponeTargetBatch:
    """Tests for the batch variant of the second-press stage."""

    def test_all_on_first_stage_bump_together(self):
        today = datetime.now().date()
        dues = [datetime.combine(today + timedelta(days=1), dtime(12, 0))] * 3

        assert resolve_postpone_target_batch('tomorrow', dues) == 'day_after_tomorrow'

    def test_mixed_group_stays_on_the_first_stage(self):
        today = datetime.now().date()
        noon = dtime(12, 0)
        dues = [datetime.combine(today + timedelta(days=1), noon), None]

        assert resolve_postpone_target_batch('tomorrow', dues) == 'tomorrow'

    def test_empty_selection_stays(self):
        assert resolve_postpone_target_batch('tomorrow', []) == 'tomorrow'
        assert resolve_postpone_target_batch('weekend', []) == 'weekend'


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
