"""Share service: manages public bearer-token links scoped to one tag.

A share is scoped either to a ``+project`` or to an ``@context`` (location).
Shares are stored as a list under the ``shares`` key in the application
settings file (see :func:`load_settings`/:func:`save_settings`). Each entry
carries exactly one scope key — ``project`` or ``context``::

    {
        "token": "<22-char base64url>",
        "project": "<name>",   # or "context": "<name>"
        "created": "<iso>",
        "expires_at": "<iso>"  # optional; missing/None means no expiration
    }

Entries written before context sharing existed only ever have ``project``,
so the format stays backward compatible.

The URL path component ``/s/<token>`` *is* the capability — anyone with the
link gets access to the share's scope without authentication.
"""

from __future__ import annotations

import hmac
import logging
import secrets
from datetime import datetime, timedelta, timezone
from typing import Any

from app.services.storage import load_settings, save_settings

logger = logging.getLogger(__name__)

ALLOWED_TTL_DAYS = {7, 30, 90}

SCOPE_PROJECT = 'project'
SCOPE_CONTEXT = 'context'
#: Order matters: a malformed entry carrying both keys resolves as a project.
SCOPE_TYPES = (SCOPE_PROJECT, SCOPE_CONTEXT)


def _load_shares() -> list[dict[str, Any]]:
    settings = load_settings()
    shares = settings.get('shares')
    return list(shares) if isinstance(shares, list) else []


def _save_shares(shares: list[dict[str, Any]]) -> None:
    settings = load_settings()
    settings['shares'] = shares
    save_settings(settings)


def share_scope(share: dict[str, Any]) -> tuple[str, str] | None:
    """Return ``(scope_type, name)`` for a share, or None if malformed."""
    for scope_type in SCOPE_TYPES:
        value = share.get(scope_type)
        if isinstance(value, str) and value:
            return scope_type, value
    return None


def _is_expired(share: dict[str, Any], now: datetime) -> bool:
    """Return True if the share has a known-past ``expires_at``.

    Shares without an ``expires_at`` field are unbounded. A malformed
    timestamp is treated as unbounded too — we'd rather keep a link working
    than silently invalidate it due to data corruption.
    """
    raw = share.get('expires_at')
    if not raw:
        return False
    try:
        expires = datetime.fromisoformat(raw)
    except (TypeError, ValueError):
        return False
    return expires <= now


def list_shares() -> list[dict[str, Any]]:
    """Return all shares (copy)."""
    return _load_shares()


def get_share_by_token(token: str) -> dict[str, Any] | None:
    """Look up a share by its token using constant-time comparison.

    Constant-time on the token side prevents timing attacks that would
    otherwise allow incremental enumeration of valid tokens. Expired shares
    are treated as non-existent.
    """
    if not token or not isinstance(token, str):
        return None
    now = datetime.now(timezone.utc)
    for share in _load_shares():
        stored = share.get('token', '')
        if isinstance(stored, str) and hmac.compare_digest(stored, token):
            if _is_expired(share, now):
                return None
            return share
    return None


def get_share_by_scope(scope_type: str, name: str) -> dict[str, Any] | None:
    """Return the existing share for a ``+project``/``@context`` or None.

    The name is matched case-insensitively, matching how the main list
    aggregates tag casing variants into one group.
    """
    if scope_type not in SCOPE_TYPES or not name:
        return None
    wanted = name.casefold()
    for share in _load_shares():
        scope = share_scope(share)
        if scope and scope[0] == scope_type and scope[1].casefold() == wanted:
            return share
    return None


def get_share_by_project(project: str) -> dict[str, Any] | None:
    """Return the existing share for ``project`` or None."""
    return get_share_by_scope(SCOPE_PROJECT, project)


def create_share(
    name: str,
    ttl_days: int | None = None,
    scope_type: str = SCOPE_PROJECT,
) -> dict[str, Any]:
    """Create (or return existing) share for a project or context.

    Idempotent: calling twice for the same scope returns the same entry —
    the ``ttl_days`` argument is only honored when a *new* entry is created.
    To change the TTL of an existing share, revoke it first.

    ``ttl_days`` must be one of :data:`ALLOWED_TTL_DAYS` or ``None`` (no
    expiration). Other values raise :class:`ValueError`.
    """
    if scope_type not in SCOPE_TYPES:
        raise ValueError(f"scope_type must be one of {list(SCOPE_TYPES)}")
    name = (name or '').strip()
    if not name:
        raise ValueError(f"{scope_type} must not be empty")
    if ttl_days is not None and ttl_days not in ALLOWED_TTL_DAYS:
        raise ValueError(f"ttl_days must be one of {sorted(ALLOWED_TTL_DAYS)} or None")

    existing = get_share_by_scope(scope_type, name)
    if existing:
        return existing

    created = datetime.now(timezone.utc)
    entry: dict[str, Any] = {
        'token': secrets.token_urlsafe(16),
        scope_type: name,
        'created': created.isoformat(),
    }
    if ttl_days is not None:
        entry['expires_at'] = (created + timedelta(days=ttl_days)).isoformat()
    shares = _load_shares()
    shares.append(entry)
    _save_shares(shares)
    return entry


def purge_expired_shares() -> int:
    """Remove all expired shares from storage. Returns the number removed.

    Called from the owner-facing API routes so the public ``/s/<token>``
    path stays read-only.
    """
    now = datetime.now(timezone.utc)
    shares = _load_shares()
    kept = [s for s in shares if not _is_expired(s, now)]
    removed = len(shares) - len(kept)
    if removed:
        _save_shares(kept)
    return removed


def delete_share(token: str) -> bool:
    """Remove the share identified by ``token``. Returns True on success."""
    if not token:
        return False
    shares = _load_shares()
    new_shares = [s for s in shares if not hmac.compare_digest(s.get('token', ''), token)]
    if len(new_shares) == len(shares):
        return False
    _save_shares(new_shares)
    return True


def delete_share_by_scope(scope_type: str, name: str) -> bool:
    """Remove the share for the given project/context. True on success."""
    share = get_share_by_scope(scope_type, name)
    if not share:
        return False
    return delete_share(share['token'])


def delete_share_by_project(project: str) -> bool:
    """Remove the share for the given project. Returns True on success."""
    return delete_share_by_scope(SCOPE_PROJECT, project)
