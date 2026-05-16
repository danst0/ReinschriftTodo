"""Share service: manages public bearer-token links to a single +project scope.

Shares are stored as a list under the ``shares`` key in the application
settings file (see :func:`load_settings`/:func:`save_settings`). Each entry::

    {"token": "<22-char base64url>", "project": "<name>", "created": "<iso>"}

The URL path component ``/s/<token>`` *is* the capability — anyone with the
link gets access to the share's scope without authentication.
"""

from __future__ import annotations

import hmac
import logging
import secrets
from datetime import datetime, timezone
from typing import Any

from app.services.storage import load_settings, save_settings

logger = logging.getLogger(__name__)


def _load_shares() -> list[dict[str, Any]]:
    settings = load_settings()
    shares = settings.get('shares')
    return list(shares) if isinstance(shares, list) else []


def _save_shares(shares: list[dict[str, Any]]) -> None:
    settings = load_settings()
    settings['shares'] = shares
    save_settings(settings)


def list_shares() -> list[dict[str, Any]]:
    """Return all shares (copy)."""
    return _load_shares()


def get_share_by_token(token: str) -> dict[str, Any] | None:
    """Look up a share by its token using constant-time comparison.

    Constant-time on the token side prevents timing attacks that would
    otherwise allow incremental enumeration of valid tokens.
    """
    if not token or not isinstance(token, str):
        return None
    for share in _load_shares():
        stored = share.get('token', '')
        if isinstance(stored, str) and hmac.compare_digest(stored, token):
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


def create_share(project: str) -> dict[str, Any]:
    """Create (or return existing) share for the given project.

    Idempotent: calling twice for the same project returns the same entry.
    """
    project = (project or '').strip()
    if not project:
        raise ValueError("project must not be empty")

    existing = get_share_by_project(project)
    if existing:
        return existing

    entry = {
        'token': secrets.token_urlsafe(16),
        'project': project,
        'created': datetime.now(timezone.utc).isoformat(),
    }
    shares = _load_shares()
    shares.append(entry)
    _save_shares(shares)
    return entry


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
