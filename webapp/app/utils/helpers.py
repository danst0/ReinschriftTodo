"""General helper functions."""

from datetime import datetime


def parse_flag(value, default: bool = True) -> bool:
    """Parse a boolean flag from various input types."""
    if value is None:
        return default
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return value != 0
    if isinstance(value, str):
        normalized = value.strip().lower()
        if normalized in ('1', 'true', 'yes', 'on', 'y'):
            return True
        if normalized in ('0', 'false', 'no', 'off', 'n', ''):
            return False
    return default


def format_due(dt_value: datetime) -> str:
    """Format a datetime for storage (always includes time)."""
    return dt_value.strftime("%Y-%m-%dT%H:%M")


def format_due_display(dt_value: datetime) -> str:
    """Format due date for display, omitting time if it's 00:00."""
    if dt_value.hour == 0 and dt_value.minute == 0:
        return dt_value.strftime("%Y-%m-%d")
    return dt_value.strftime("%Y-%m-%dT%H:%M")


def is_sometime(dt_value: datetime | None) -> bool:
    """Check if a datetime represents the 'sometime' sentinel value."""
    if dt_value is None:
        return False
    return dt_value.date().year == 9999


def normalize_prefix(token: str | None, prefix_char: str) -> str | None:
    """Remove redundant leading prefix characters like '+' or '@'."""
    if not token:
        return None
    normalized = token.lstrip(prefix_char).strip()
    return normalized if normalized else None
