"""Human-readable due dates for the list view.

The list shows due dates relative to today ("Tomorrow, 10:00") and groups the
date view into time horizons (overdue, today, tomorrow, this week, later,
sometime, no date). Both are derived here so templates and routes agree.
"""

from datetime import date, datetime, timedelta
from typing import Any, Mapping

# Display order of the horizons in the date view.
HORIZONS = ('overdue', 'today', 'tomorrow', 'week', 'later', 'sometime', 'none')

# Translation key of each horizon's section label.
HORIZON_LABEL_KEYS = {
    'overdue': 'overdue',
    'today': 'today',
    'tomorrow': 'tomorrow',
    'week': 'this_week',
    'later': 'later',
    'sometime': 'sometime',
    'none': 'no_due',
}

_FALLBACK_WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']


def parse_due(value: Any) -> datetime | None:
    if value is None or value == '':
        return None
    if isinstance(value, datetime):
        return value
    try:
        return datetime.fromisoformat(str(value))
    except ValueError:
        return None


def horizon_of(due: datetime | None, today: date) -> str:
    """Classify a due date into one of HORIZONS."""
    if due is None:
        return 'none'
    if due.year == 9999:
        return 'sometime'
    day = due.date()
    if day < today:
        return 'overdue'
    if day == today:
        return 'today'
    if day == today + timedelta(days=1):
        return 'tomorrow'
    # Rest of the current week (ISO weeks end on Sunday).
    if day <= today + timedelta(days=6 - today.weekday()):
        return 'week'
    return 'later'


def horizon_label(horizon: str, t: Mapping[str, Any]) -> str:
    key = HORIZON_LABEL_KEYS[horizon]
    return str(t.get(key, key))


def describe_due(todo: Mapping[str, Any], t: Mapping[str, Any],
                 now: datetime | None = None) -> dict[str, Any]:
    """Describe a todo's due date for display.

    Returns a dict with:
        horizon: one of HORIZONS
        label:   inline text, e.g. "Tomorrow, 10:00" ('' without a due date)
        day:     short day for the schedule gutter ("Wed", "26.09."), may be ''
        time:    "10:00" when the due date has a time of day, else ''
        late:    open and the due time has passed already
        marked:  open and due today or earlier (gets the highlighter)
    """
    now = now or datetime.now()
    today = now.date()
    due = parse_due(todo.get('due'))
    horizon = horizon_of(due, today)

    result: dict[str, Any] = {
        'horizon': horizon, 'label': '', 'day': '', 'time': '',
        'late': False, 'marked': False,
    }
    if due is None:
        return result
    if horizon == 'sometime':
        result['label'] = str(t.get('sometime', 'Sometime'))
        return result

    weekdays = t.get('weekdays_short') or _FALLBACK_WEEKDAYS
    weekday = weekdays[due.weekday()]
    short_date = due.strftime(str(t.get('date_short_fmt', '%d.%m.')))
    has_time = (due.hour, due.minute) != (0, 0)
    time_str = due.strftime('%H:%M') if has_time else ''

    if horizon == 'today':
        base, day = str(t.get('today', 'Today')), ''
    elif horizon == 'tomorrow':
        base, day = str(t.get('tomorrow', 'Tomorrow')), ''
    elif horizon == 'week':
        base, day = f"{weekday}, {short_date}", weekday
    else:  # overdue, later
        base, day = f"{weekday}, {short_date}", short_date

    result['label'] = f"{base}, {time_str}" if time_str else base
    result['day'] = day
    result['time'] = time_str
    is_open = not todo.get('done')
    result['late'] = is_open and (horizon == 'overdue' or (has_time and due < now))
    result['marked'] = is_open and horizon in ('overdue', 'today')
    return result
