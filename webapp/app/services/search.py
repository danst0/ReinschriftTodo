"""Text search over todos.

Mirrors core/src/search.rs so the web search finds what the GNOME app finds.
The title is stored without its tokens, so `+Security` or `@Computer` never
show up in it. A query therefore works in two ways:

- Words starting with `+` or `@` filter by project or place (substring,
  case-insensitive): `+sec` finds `+Security`. A hit counts as primary.
- The remaining text is searched in the title first (primary). Todos that
  only carry it in a project, place or note are secondary hits, listed below
  the title hits.

Both parts combine: `+security ufw` finds Security tasks mentioning "ufw".
"""

from dataclasses import dataclass, field
from typing import Any, Literal, Optional

SearchMatch = Literal['primary', 'secondary']

PRIMARY: SearchMatch = 'primary'
SECONDARY: SearchMatch = 'secondary'


def _get(todo: Any, name: str) -> Any:
    """Read a field from a TodoItem or its dict form."""
    if isinstance(todo, dict):
        return todo.get(name)
    return getattr(todo, name, None)


def _tags_match(wanted: list[str], tags: list[str]) -> bool:
    """Every wanted fragment occurs in at least one of the todo's tags."""
    return all(any(w in tag.lower() for tag in tags) for w in wanted)


@dataclass
class SearchQuery:
    """A parsed search query (all parts lowercased)."""

    projects: list[str] = field(default_factory=list)
    contexts: list[str] = field(default_factory=list)
    text: str = ''

    @classmethod
    def parse(cls, query: str) -> 'SearchQuery':
        parsed = cls()
        words = []
        for word in query.split():
            lower = word.lower()
            if lower.startswith('+'):
                parsed.projects.append(lower[1:])
            elif lower.startswith('@'):
                parsed.contexts.append(lower[1:])
            else:
                words.append(lower)
        parsed.text = ' '.join(words)
        return parsed

    def is_empty(self) -> bool:
        return not self.projects and not self.contexts and not self.text

    def matches(self, todo: Any) -> Optional[SearchMatch]:
        """How `todo` matches, or None if it does not."""
        if self.is_empty():
            return None
        projects = _get(todo, 'projects') or []
        contexts = _get(todo, 'contexts') or []
        if not _tags_match(self.projects, projects) or not _tags_match(self.contexts, contexts):
            return None
        title = (_get(todo, 'title') or '').lower()
        if not self.text or self.text in title:
            return PRIMARY
        in_tags = any(self.text in tag.lower() for tag in projects + contexts)
        in_note = self.text in (_get(todo, 'note') or '').lower()
        return SECONDARY if in_tags or in_note else None
