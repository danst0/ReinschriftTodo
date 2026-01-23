"""Marker generation utilities."""

import secrets
import string


def generate_marker(length: int = 8) -> str:
    """Generate a random alphanumeric marker ID."""
    alphabet = string.ascii_letters + string.digits
    return ''.join(secrets.choice(alphabet) for _ in range(length))


def ensure_marker(existing: str | None) -> str:
    """Return existing marker or generate a new one."""
    return existing if existing else generate_marker()
