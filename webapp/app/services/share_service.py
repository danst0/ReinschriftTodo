"""Share service: manages public bearer-token links to a single +project scope.

Shares are stored as a list under the ``shares`` key in the application
settings file (see :func:`load_settings`/:func:`save_settings`). Each entry::

    {
        "token": "<22-char base64url>",
        "project": "<name>",
        "created": "<iso>",
        "expires_at": "<iso>"  # optional; missing/None means no expiration
    }

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


def _load_shares() -> list[dict[str, Any]]:
    settings = load_settings()
    shares = settings.get('shares')
    return list(shares) if isinstance(shares, list) else []


def _save_shares(shares: list[dict[str, Any]]) -> None:
    settings = load_settings()
    settings['shares'] = shares
    save_settings(settings)


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


def get_share_by_project(project: str) -> dict[str, Any] | None:
    """Return the existing share for ``project`` or None."""
    if not project:
        return None
    for share in _load_shares():
        if share.get('project') == project:
            return share
    return None


def create_share(project: str, ttl_days: int | None = None) -> dict[str, Any]:
    """Create (or return existing) share for the given project.

    Idempotent: calling twice for the same project returns the same entry —
    the ``ttl_days`` argument is only honored when a *new* entry is created.
    To change the TTL of an existing share, revoke it first.

    ``ttl_days`` must be one of :data:`ALLOWED_TTL_DAYS` or ``None`` (no
    expiration). Other values raise :class:`ValueError`.
    """
    project = (project or '').strip()
    if not project:
        raise ValueError("project must not be empty")
    if ttl_days is not None and ttl_days not in ALLOWED_TTL_DAYS:
        raise ValueError(f"ttl_days must be one of {sorted(ALLOWED_TTL_DAYS)} or None")

    existing = get_share_by_project(project)
    if existing:
        return existing

    created = datetime.now(timezone.utc)
    entry: dict[str, Any] = {
        'token': secrets.token_urlsafe(16),
        'project': project,
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


def delete_share_by_project(project: str) -> bool:
    """Remove the share for the given project. Returns True on success."""
    share = get_share_by_project(project)
    if not share:
        return False
    return delete_share(share['token'])
