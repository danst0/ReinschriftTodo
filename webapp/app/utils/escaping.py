"""Note escaping and unescaping utilities."""


def escape_note(text: str) -> str:
    """Escape special characters in note text for storage."""
    return (text
            .replace('\\', '\\\\')
            .replace('"', '\\"')
            .replace('\r', '\\r')
            .replace('\n', '\\n'))


def unescape_note(text: str | None) -> str:
    """Unescape special characters in note text from storage."""
    if text is None:
        return ""

    out = []
    i = 0
    while i < len(text):
        ch = text[i]
        if ch == '\\' and i + 1 < len(text):
            nxt = text[i + 1]
            if nxt == 'n':
                out.append('\n')
            elif nxt == 'r':
                out.append('\r')
            elif nxt == '"':
                out.append('"')
            elif nxt == '\\':
                out.append('\\')
            else:
                out.append(nxt)
            i += 2
        else:
            out.append(ch)
            i += 1
    return ''.join(out)


def normalize_note(text: str | None) -> str | None:
    """Normalize note text, returning None for empty strings."""
    if text is None:
        return None
    trimmed = text.strip()
    return trimmed if trimmed else None
