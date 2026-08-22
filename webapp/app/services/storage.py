"""Storage service for reading/writing todo files."""

import json
import logging
import os
import time
from typing import Any

import requests
from requests.auth import HTTPBasicAuth

# Retry settings for WebDAV locked files (423 status)
MAX_RETRIES = 3
RETRY_DELAY = 0.5  # seconds, will be multiplied for exponential backoff

from flask import current_app

from app.exceptions import ConflictError, StorageError

logger = logging.getLogger(__name__)


def _get_config():
    """Get storage configuration from Flask app or environment."""
    if current_app:
        return {
            'use_webdav': current_app.config.get('USE_WEBDAV', False),
            'webdav_url': current_app.config.get('WEBDAV_URL'),
            'webdav_username': current_app.config.get('WEBDAV_USERNAME'),
            'webdav_password': current_app.config.get('WEBDAV_PASSWORD'),
            'todo_path': current_app.config.get('TODO_PATH', 'TodosDatenbank.md'),
            'config_path': current_app.config.get('CONFIG_PATH', '/config/settings.json'),
        }
    # Fallback to environment variables
    return {
        'use_webdav': os.environ.get('USE_WEBDAV', 'false').lower() == 'true',
        'webdav_url': os.environ.get('WEBDAV_URL'),
        'webdav_username': os.environ.get('WEBDAV_USERNAME'),
        'webdav_password': os.environ.get('WEBDAV_PASSWORD'),
        'todo_path': os.environ.get('TODOS_DB_PATH', 'TodosDatenbank.md'),
        'config_path': os.environ.get('CONFIG_PATH', '/config/settings.json'),
    }


def _fingerprint_slot() -> dict[str, str] | None:
    """Per-request store for the fingerprint of the last read.

    Outside a request (CLI use, tests) there is nothing to scope it to, so
    tracking is off and writes stay unconditional.
    """
    try:
        from flask import g, has_request_context
    except ImportError:  # pragma: no cover - Flask is a hard dependency
        return None
    if not has_request_context():
        return None
    if not hasattr(g, '_todo_fingerprint'):
        g._todo_fingerprint = {}
    slot: dict[str, str] = g._todo_fingerprint
    return slot


def _remember_fingerprint(value: str) -> None:
    slot = _fingerprint_slot()
    if slot is not None:
        slot['value'] = value


def _expected_fingerprint() -> str:
    slot = _fingerprint_slot()
    return slot.get('value', '') if slot else ''


def _forget_fingerprint() -> None:
    slot = _fingerprint_slot()
    if slot is not None:
        slot.pop('value', None)


def _stamp_undo(content: str) -> None:
    """Tell the undo stack which state this write produced.

    Every mutation funnels through here, so this is the one place that knows
    the post-mutation content without touching each route.
    """
    try:
        from app.services.undo_service import stamp_last_result
        stamp_last_result(content)
    except (ImportError, RuntimeError):
        # No session (CLI use, tests) — nothing to stamp.
        pass


def _fingerprint_from_headers(headers) -> str:
    """Build the fingerprint from a response's validators."""
    return f"{headers.get('etag', '')}-{headers.get('last-modified', '')}"


def read_content() -> str:
    """Read todo file content from local filesystem or WebDAV.

    Also records the fingerprint of exactly this content for the current
    request, so a later :func:`write_content` can refuse to overwrite somebody
    else's newer state.

    Returns:
        File contents as string.

    Raises:
        StorageError: If reading fails.
    """
    content, fingerprint = _read_content_and_fingerprint()
    _remember_fingerprint(fingerprint)
    return content


def _read_content_and_fingerprint() -> tuple[str, str]:
    """Read content and a fingerprint that provably belongs to it.

    Fetching the validator in a separate request pairs stale content with a
    fresh ETag: the guard on the way out then passes and the other writer's
    changes are overwritten without any conflict being reported.
    """
    config = _get_config()

    if config['use_webdav']:
        if not config['webdav_url']:
            return "", ""
        try:
            auth = None
            if config['webdav_username'] and config['webdav_password']:
                auth = HTTPBasicAuth(config['webdav_username'], config['webdav_password'])

            response = requests.get(config['webdav_url'], auth=auth, timeout=10)
            response.raise_for_status()
            return response.text, _fingerprint_from_headers(response.headers)
        except requests.RequestException as e:
            logger.error("WebDAV read error: %s", e)
            raise StorageError(f"WebDAV read error: {e}") from e
    else:
        todo_path = config['todo_path']
        if not os.path.exists(todo_path):
            return "", ""
        try:
            # Read between two stats and retry while they disagree, so the
            # content belongs to the mtime we report.
            for _ in range(3):
                before = str(os.path.getmtime(todo_path))
                with open(todo_path, 'r', encoding='utf-8') as f:
                    content = f.read()
                after = str(os.path.getmtime(todo_path))
                if before == after:
                    return content, after
            raise StorageError("The file keeps changing while it is being read.")
        except IOError as e:
            logger.error("Local file read error: %s", e)
            raise StorageError(f"Local file read error: {e}") from e


def write_content(content: str) -> None:
    """Write todo file content to local filesystem or WebDAV.

    If this request read the file first, the write is guarded by that read's
    fingerprint: a concurrent write from another client is reported instead of
    being silently overwritten.

    Args:
        content: Content to write.

    Raises:
        StorageError: If writing fails.
        ConflictError: If the file changed since this request read it.
    """
    _write(content, _expected_fingerprint())


def force_write_content(content: str) -> None:
    """Write unconditionally, bypassing conflict detection.

    Only for an explicit "overwrite" after the user was shown a conflict.
    """
    _write(content, '')


def _write(content: str, expected_fingerprint: str) -> None:
    config = _get_config()

    if config['use_webdav']:
        if not config['webdav_url']:
            return

        auth = None
        if config['webdav_username'] and config['webdav_password']:
            auth = HTTPBasicAuth(config['webdav_username'], config['webdav_password'])

        headers = {}
        if expected_fingerprint:
            # The fingerprint is "<etag>-<last-modified>"; If-Match wants the ETag.
            etag = expected_fingerprint.split('-', 1)[0]
            if etag:
                headers['If-Match'] = etag

        last_error = None
        for attempt in range(MAX_RETRIES):
            try:
                response = requests.put(
                    config['webdav_url'],
                    data=content.encode('utf-8'),
                    auth=auth,
                    headers=headers,
                    timeout=10
                )
                response.raise_for_status()
                # The stored validator is spent; a follow-up write in the same
                # request must not be checked against it.
                _forget_fingerprint()
                _stamp_undo(content)
                return  # Success
            except requests.HTTPError as e:
                last_error = e
                status = e.response.status_code if e.response is not None else None
                if status == 412:
                    raise ConflictError(
                        expected_fingerprint,
                        e.response.headers.get('etag', 'unknown'),
                    ) from e
                # Retry on 423 Locked (Nextcloud file locking)
                if status == 423:
                    if attempt < MAX_RETRIES - 1:
                        delay = RETRY_DELAY * (2 ** attempt)
                        logger.warning("File locked (423), retrying in %.1fs (attempt %d/%d)",
                                     delay, attempt + 1, MAX_RETRIES)
                        time.sleep(delay)
                        continue
                raise StorageError(f"WebDAV write error: {e}") from e
            except requests.RequestException as e:
                logger.error("WebDAV write error: %s", e)
                raise StorageError(f"WebDAV write error: {e}") from e

        # All retries exhausted
        logger.error("WebDAV write failed after %d retries: %s", MAX_RETRIES, last_error)
        raise StorageError(f"WebDAV write error (file locked): {last_error}") from last_error
    else:
        todo_path = config['todo_path']
        if expected_fingerprint and os.path.exists(todo_path):
            current = str(os.path.getmtime(todo_path))
            if current != expected_fingerprint:
                raise ConflictError(expected_fingerprint, current)
        try:
            with open(todo_path, 'w', encoding='utf-8') as f:
                f.write(content)
            _forget_fingerprint()
            _stamp_undo(content)
        except IOError as e:
            logger.error("Local file write error: %s", e)
            raise StorageError(f"Local file write error: {e}") from e


def get_fingerprint() -> str:
    """Get a fingerprint for the current file (for conflict detection).

    Returns:
        A string fingerprint (mtime for local, ETag for WebDAV).
    """
    config = _get_config()

    if config['use_webdav']:
        if not config['webdav_url']:
            return ""
        try:
            auth = None
            if config['webdav_username'] and config['webdav_password']:
                auth = HTTPBasicAuth(config['webdav_username'], config['webdav_password'])
            response = requests.head(config['webdav_url'], auth=auth, timeout=5)
            response.raise_for_status()
            etag = response.headers.get('etag', '')
            last_mod = response.headers.get('last-modified', '')
            return f"{etag}-{last_mod}"
        except requests.RequestException as e:
            logger.warning("Failed to get WebDAV fingerprint: %s", e)
            return ""
    else:
        todo_path = config['todo_path']
        if not os.path.exists(todo_path):
            return ""
        return str(os.path.getmtime(todo_path))


def read_content_with_fingerprint() -> tuple[str, str]:
    """Read content together with a fingerprint that belongs to it.

    Returns:
        Tuple of (content, fingerprint).
    """
    content, fingerprint = _read_content_and_fingerprint()
    _remember_fingerprint(fingerprint)
    return content, fingerprint


def write_content_checked(content: str, expected_fingerprint: str) -> None:
    """Write content only if the fingerprint has not changed.

    The check rides on the write itself (``If-Match``) rather than a separate
    request, so nothing can slip in between checking and writing.

    Args:
        content: Content to write.
        expected_fingerprint: Expected fingerprint from when content was read.

    Raises:
        ConflictError: If the file was modified since it was read.
    """
    _write(content, expected_fingerprint)


def load_settings() -> dict[str, Any]:
    """Load application settings from JSON file.

    Returns:
        Settings dictionary.
    """
    config = _get_config()
    config_path = config['config_path']

    if not os.path.exists(config_path):
        return {}
    try:
        with open(config_path, 'r', encoding='utf-8') as f:
            settings = json.load(f)
        return settings if isinstance(settings, dict) else {}
    except (IOError, json.JSONDecodeError) as e:
        logger.warning("Error loading settings: %s", e)
        return {}


def save_settings(settings: dict[str, Any]) -> None:
    """Save application settings to JSON file.

    Args:
        settings: Settings dictionary to save.
    """
    config = _get_config()
    config_path = config['config_path']

    try:
        os.makedirs(os.path.dirname(config_path), exist_ok=True)
        with open(config_path, 'w', encoding='utf-8') as f:
            json.dump(settings, f)
    except (IOError, OSError) as e:
        logger.error("Error saving settings: %s", e)
