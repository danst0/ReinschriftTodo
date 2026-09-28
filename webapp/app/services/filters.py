"""List filters: due date range, projects and places.

Mirrors core/src/filter.rs so the web list filters the way the GNOME app
does. The bounded ranges count from today and include overdue todos
("within 7 days" = everything to be done by a week from today); todos due
"sometime" never fall into a bounded range.
"""

from dataclasses import dataclass, field
from datetime import date, datetime, timedelta
from typing import Any, Mapping

DUE_RANGES = ('any', 'overdue', 'today', 'tomorrow', 'week', 'month', 'undated')
BOUNDED_RANGES = ('overdue', 'today', 'tomorrow', 'week', 'month')

# Stands for "no project"/"no place" in TodoFilter.projects/.contexts.
NO_TAG = ''

_LAST_DAY_OFFSET = {'overdue': -1, 'today': 0, 'tomorrow': 1, 'week': 7, 'month': 30}


def _tags_match(wanted: list[str], have: list[str]) -> bool:
    if not wanted:
        return True
    have_lower = {h.lower() for h in have}
    return any((not have) if w == NO_TAG else w.lower() in have_lower for w in wanted)


def _toggle(tags: list[str], name: str) -> list[str]:
    lower = name.lower()
    if any(t.lower() == lower for t in tags):
        return [t for t in tags if t.lower() != lower]
    return tags + [name]


@dataclass
class TodoFilter:
    due: str = 'any'
    include_undated: bool = False
    projects: list[str] = field(default_factory=list)
    contexts: list[str] = field(default_factory=list)

    @property
    def active_count(self) -> int:
        return int(self.due != 'any') + int(bool(self.projects)) + int(bool(self.contexts))

    @property
    def is_active(self) -> bool:
        return self.active_count > 0

    @property
    def is_bounded(self) -> bool:
        return self.due in BOUNDED_RANGES

    def matches(self, todo: Any, today: date) -> bool:
        return (self.matches_due(todo, today)
                and _tags_match(self.projects, list(_get(todo, 'projects') or []))
                and _tags_match(self.contexts, list(_get(todo, 'contexts') or [])))

    def matches_due(self, todo: Any, today: date) -> bool:
        due = _due_date(_get(todo, 'due'))
        if self.due == 'any':
            return True
        if self.due == 'undated':
            return due is None or due.year == 9999
        if due is None:
            return self.include_undated
        if due.year == 9999:
            return False
        return due <= today + timedelta(days=_LAST_DAY_OFFSET[self.due])

    def toggled_project(self, name: str) -> 'TodoFilter':
        return TodoFilter(self.due, self.include_undated, _toggle(self.projects, name), list(self.contexts))

    def toggled_context(self, name: str) -> 'TodoFilter':
        return TodoFilter(self.due, self.include_undated, list(self.projects), _toggle(self.contexts, name))

    # Persistence in settings.json -------------------------------------------

    def to_settings(self) -> dict[str, Any]:
        return {
            'filter_due': self.due,
            'filter_undated': '1' if self.include_undated else '0',
            'filter_projects': list(self.projects),
            'filter_contexts': list(self.contexts),
        }

    @classmethod
    def from_settings(cls, settings: Mapping[str, Any]) -> 'TodoFilter':
        """Read the stored filter. Settings from before the filter existed
        only know "hide future": due by today, undated todos kept."""
        if 'filter_due' not in settings:
            if str(settings.get('show_due_only', '0')) == '1':
                return cls(due='today', include_undated=True)
            return cls()
        due = settings.get('filter_due')
        return cls(
            due=due if due in DUE_RANGES else 'any',
            include_undated=str(settings.get('filter_undated', '0')) == '1',
            projects=[str(p) for p in settings.get('filter_projects') or []],
            contexts=[str(c) for c in settings.get('filter_contexts') or []],
        )

    @classmethod
    def from_args(cls, args: Any) -> 'TodoFilter':
        """Read a filter submitted by the filter panel (a MultiDict)."""
        due = args.get('filter_due', 'any')
        return cls(
            due=due if due in DUE_RANGES else 'any',
            include_undated=args.get('filter_undated') == '1',
            projects=args.getlist('filter_project'),
            contexts=args.getlist('filter_context'),
        )


def filter_choices(todos: Any, todo_filter: TodoFilter) -> dict[str, list[str]]:
    """Projects and places of the open todos (most used casing, sorted) to
    offer in the filter panel. Tags already in the filter stay listed even
    when no open todo carries them any more."""
    from app.utils.helpers import canonical_casing_map, canonicalize_token

    def collect(attr: str, kept: list[str]) -> list[str]:
        values = [v for todo in todos if not _get(todo, 'done')
                  for v in (_get(todo, attr) or [])]
        canon = canonical_casing_map(values)
        names: dict[str, str] = {}
        for value in values + [k for k in kept if k != NO_TAG]:
            names.setdefault(value.lower(), canonicalize_token(canon, value))
        return sorted(names.values(), key=str.lower)

    return {
        'projects': collect('projects', todo_filter.projects),
        'contexts': collect('contexts', todo_filter.contexts),
    }


def _get(todo: Any, name: str) -> Any:
    if isinstance(todo, Mapping):
        return todo.get(name)
    return getattr(todo, name, None)


def _due_date(value: Any) -> date | None:
    if value is None or value == '':
        return None
    if isinstance(value, datetime):
        return value.date()
    if isinstance(value, date):
        return value
    try:
        return datetime.fromisoformat(str(value)).date()
    except ValueError:
        return None
