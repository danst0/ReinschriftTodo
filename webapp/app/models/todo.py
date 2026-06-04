"""TodoItem dataclass and regex patterns for parsing."""

import re
from dataclasses import dataclass, field
from datetime import date, datetime, time as dtime
from typing import Optional


# Regex patterns for parsing todo fields (matching Rust core/src/parser.rs)
LINK_RE = re.compile(r"\[\[([^\]]+)\]\]")
# Projects and contexts accept either a quoted form (group 1, with backslash
# escapes — useful for multi-word names like +"Steuererklärung 2024") or a
# whitespace-delimited plain form (group 2).
PROJECT_RE = re.compile(r'\+(?:"((?:\\.|[^"])*)"|([^\s]+))')
CONTEXT_RE = re.compile(r'@(?:"((?:\\.|[^"])*)"|([^\s]+))')
DUE_RE = re.compile(r"due:(\d{4}-\d{2}-\d{2})(?:T(\d{2}:\d{2}))?")
ID_RE = re.compile(r"\^([A-Za-z0-9]+)")
COMPLETION_RE = re.compile(r"\s✅\s\d{4}-\d{2}-\d{2}")
COMPLETION_DATE_RE = re.compile(r"✅\s(\d{4}-\d{2}-\d{2})")
RECUR_RE = re.compile(r"rec:([^\s]+)")
MYDAY_RE = re.compile(r"myday:(\d{4}-\d{2}-\d{2})")
MYDAY_STRIP_RE = re.compile(r"\s*myday:\d{4}-\d{2}-\d{2}")
NOTE_RE = re.compile(r'~note:"((?:\\.|[^"])*)"')

# Constants
DEFAULT_DUE_TIME = dtime(hour=0, minute=0)
SOMETIME_SENTINEL = datetime(year=9999, month=12, day=31, hour=0, minute=0)

# Markers that indicate end of title
TITLE_MARKERS = [
    " +", " @", " due:", " myday:", " rec:", " [[", " ✅", " ^", " ~note:",
    "+", "@", "due:", "myday:", "rec:", "[[", "✅", "^", "~note:"
]


@dataclass
class TodoItem:
    """A parsed todo item with all its metadata."""

    line_index: int
    marker: Optional[str] = None
    title: str = ""
    projects: list[str] = field(default_factory=list)
    contexts: list[str] = field(default_factory=list)
    due: Optional[datetime] = None
    due_display: Optional[str] = None
    due_is_sometime: bool = False
    myday: Optional[date] = None
    in_myday: bool = False
    reference: Optional[str] = None
    recurrence: Optional[str] = None
    note: Optional[str] = None
    done: bool = False
    done_date: Optional[datetime] = None
    raw_line: str = ""

    # Display helpers
    section: Optional[str] = None
    group_key: str = ""

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization."""
        result = {
            'line_index': self.line_index,
            'marker': self.marker,
            'title': self.title,
            'projects': self.projects,
            'contexts': self.contexts,
            'due': self.due.strftime("%Y-%m-%dT%H:%M") if self.due else None,
            'due_display': self.due_display,
            'due_is_sometime': self.due_is_sometime,
            'myday': self.myday.strftime("%Y-%m-%d") if self.myday else None,
            'in_myday': self.in_myday,
            'reference': self.reference,
            'recurrence': self.recurrence,
            'note': self.note,
            'done': self.done,
            'done_date': self.done_date.strftime("%Y-%m-%d") if self.done_date else None,
            'raw_line': self.raw_line,
        }
        return result
