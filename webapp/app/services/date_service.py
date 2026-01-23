"""Date calculation service."""

from datetime import datetime, timedelta, time as dtime
from typing import Optional

from app.models.todo import DEFAULT_DUE_TIME, SOMETIME_SENTINEL


def current_date() -> datetime:
    """Get current date (midnight)."""
    return datetime.now().replace(hour=0, minute=0, second=0, microsecond=0)


def next_due_date(current_due: Optional[datetime], rule: str) -> Optional[datetime]:
    """Calculate the next due date based on recurrence rule.

    Args:
        current_due: Current due datetime.
        rule: Recurrence rule ('daily', 'weekly', 'monthly').

    Returns:
        Next due datetime or None if rule is invalid.
    """
    base_time = current_due.time() if current_due else DEFAULT_DUE_TIME
    next_date = current_due.date() if current_due else datetime.now().date()
    today = datetime.now().date()
    rule_l = rule.lower()

    while True:
        if rule_l == 'daily':
            next_date = next_date + timedelta(days=1)
        elif rule_l == 'weekly':
            next_date = next_date + timedelta(days=7)
        elif rule_l == 'monthly':
            year = next_date.year + (next_date.month // 12)
            month = next_date.month % 12 + 1
            day = next_date.day
            found = False
            for d in range(31, 27, -1):
                try:
                    next_date = datetime(year, month, min(day, d)).date()
                    found = True
                    break
                except ValueError:
                    continue
            if not found:
                return None
        else:
            return None

        if next_date > today:
            break

    return datetime.combine(next_date, base_time)


def calculate_postpone_date(target: str) -> tuple[datetime, dtime]:
    """Calculate the new date/time for a postpone operation.

    Args:
        target: Postpone target ('today', 'tomorrow', 'weekend', 'sometime').

    Returns:
        Tuple of (date, time) for the new due date.
    """
    now = datetime.now()
    today = now.date()
    current_hour = now.hour

    if target == 'today':
        # Smart "Today" logic: at least 4 hours from now
        new_date = today
        if current_hour < 8:
            new_time = dtime(hour=12, minute=0)
        elif current_hour < 14:
            new_time = dtime(hour=18, minute=0)
        else:
            new_time = dtime(hour=18, minute=0)
    elif target == 'tomorrow':
        new_date = today + timedelta(days=1)
        new_time = dtime(hour=12, minute=0)
    elif target == 'weekend':
        # Next Saturday
        day_of_week = today.weekday()  # 0=Monday, 5=Saturday, 6=Sunday
        if day_of_week == 5:  # Saturday
            days_until_saturday = 7
        elif day_of_week == 6:  # Sunday
            days_until_saturday = 6
        else:
            days_until_saturday = 5 - day_of_week
        new_date = today + timedelta(days=days_until_saturday)
        new_time = dtime(hour=12, minute=0)
    elif target == 'sometime':
        new_date = SOMETIME_SENTINEL.date()
        new_time = dtime(hour=0, minute=0)
    else:
        # Fallback: keep original date/time or set to today
        new_date = today
        new_time = DEFAULT_DUE_TIME

    return datetime.combine(new_date, new_time), new_time


def recent_window_bounds(window_days: int = 30) -> tuple[datetime, datetime]:
    """Calculate the bounds for the recent activity window.

    Args:
        window_days: Number of days in the window.

    Returns:
        Tuple of (today, window_start) dates.
    """
    today = current_date()
    window_start = today - timedelta(days=max(window_days - 1, 0))
    return today, window_start
