"""General helper functions."""

from collections import Counter, defaultdict
from collections.abc import Iterable
from datetime import datetime

from app.utils.escaping import unescape_note


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


def canonical_casing_map(values: Iterable[str]) -> dict[str, str]:
    """Map each token's lowercased form to its most frequently used casing.

    Aggregates case variants of projects/contexts (e.g. 'PixelMatrix' vs.
    'Pixelmatrix') into one canonical display form. Ties are broken lexically
    for determinism.
    """
    variants: dict[str, Counter[str]] = defaultdict(Counter)
    for value in values:
        if value:
            variants[value.lower()][value] += 1
    return {
        key: min(counts.items(), key=lambda kv: (-kv[1], kv[0]))[0]
        for key, counts in variants.items()
    }


def canonicalize_token(mapping: dict[str, str], value: str) -> str:
    """Resolve a token to its canonical casing; unknown tokens pass through."""
    return mapping.get(value.lower(), value)


def normalize_prefix(token: str | None, prefix_char: str) -> str | None:
    """Remove redundant leading prefix characters like '+' or '@'."""
    if not token:
        return None
    normalized = token.lstrip(prefix_char).strip()
    return normalized if normalized else None


def _unquote_tag(segment: str) -> str:
    """Strip the quoted form ('"Big Project"') used in the markdown file."""
    if len(segment) >= 2 and segment.startswith('"') and segment.endswith('"'):
        return unescape_note(segment[1:-1])
    return segment


def split_tag_input(raw: str | None, prefix_char: str) -> list[str]:
    """Split a dedicated project/context input field into individual tag names.

    Mirrors `split_tag_input` in the Rust core: the prefix ('+' or '@') acts as
    the delimiter whenever it is present, so spaces inside a name survive
    ('+Big Project +Other' -> ['Big Project', 'Other']). Input without any
    prefix keeps the historical whitespace split ('a b' -> ['a', 'b']), and the
    quoted form ('+"Big Project"') is accepted too.

    Only for fields that contain nothing but tags — free text such as the
    quick-add box stays quote-based, otherwise a prefix would eat the title.
    """
    trimmed = (raw or '').strip()
    if not trimmed:
        return []

    # Prefix positions that start a token — a '+' inside a name (C++) must not split.
    starts = [
        idx for idx, ch in enumerate(trimmed)
        if ch == prefix_char and (idx == 0 or trimmed[idx - 1].isspace())
    ]
    if not starts:
        return trimmed.split()

    names = trimmed[:starts[0]].split()
    for idx, start in enumerate(starts):
        end = starts[idx + 1] if idx + 1 < len(starts) else len(trimmed)
        name = _unquote_tag(trimmed[start + 1:end].strip()).lstrip(prefix_char).strip()
        if name:
            names.append(name)
    return names


def split_ai_tags(raw: str | None, prefix_char: str) -> list[str]:
    """Interpret a project/context value returned by the model.

    The prompt asks for one tag per field, so a plain value stays a single name
    even when it contains spaces ('Big Project'); only an explicitly prefixed
    list ('+haushalt +urlaub') is split into several tags.
    """
    trimmed = (raw or '').strip()
    if not trimmed:
        return []
    if trimmed.startswith(prefix_char) or any(
        token.startswith(prefix_char) for token in trimmed.split()
    ):
        return split_tag_input(trimmed, prefix_char)
    return [trimmed]
