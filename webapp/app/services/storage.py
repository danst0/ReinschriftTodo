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


def read_content() -> str:
    """Read todo file content from local filesystem or WebDAV.

    Returns:
        File contents as string.

    Raises:
        StorageError: If reading fails.
    """
    config = _get_config()

    if config['use_webdav']:
        if not config['webdav_url']:
            return ""
        try:
            auth = None
            if config['webdav_username'] and config['webdav_password']:
                auth = HTTPBasicAuth(config['webdav_username'], config['webdav_password'])

            response = requests.get(config['webdav_url'], auth=auth, timeout=10)
            response.raise_for_status()
            return response.text
        except requests.RequestException as e:
            logger.error("WebDAV read error: %s", e)
            raise StorageError(f"WebDAV read error: {e}") from e
    else:
        todo_path = config['todo_path']
        if not os.path.exists(todo_path):
            return ""
        try:
            with open(todo_path, 'r', encoding='utf-8') as f:
                return f.read()
        except IOError as e:
            logger.error("Local file read error: %s", e)
            raise StorageError(f"Local file read error: {e}") from e


def write_content(content: str) -> None:
    """Write todo file content to local filesystem or WebDAV.

    Args:
        content: Content to write.

    Raises:
        StorageError: If writing fails.
    """
    config = _get_config()

    if config['use_webdav']:
        if not config['webdav_url']:
            return

        auth = None
        if config['webdav_username'] and config['webdav_password']:
            auth = HTTPBasicAuth(config['webdav_username'], config['webdav_password'])

        last_error = None
        for attempt in range(MAX_RETRIES):
            try:
                response = requests.put(
                    config['webdav_url'],
                    data=content.encode('utf-8'),
                    auth=auth,
                    timeout=10
                )
                response.raise_for_status()
                return  # Success
            except requests.HTTPError as e:
                last_error = e
                # Retry on 423 Locked (Nextcloud file locking)
                if e.response is not None and e.response.status_code == 423:
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
        try:
            with open(config['todo_path'], 'w', encoding='utf-8') as f:
                f.write(content)
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
    """Read content together with its fingerprint.

    Returns:
        Tuple of (content, fingerprint).
    """
    content = read_content()
    fingerprint = get_fingerprint()
    return content, fingerprint


def write_content_checked(content: str, expected_fingerprint: str) -> None:
    """Write content only if the fingerprint has not changed.

    Args:
        content: Content to write.
        expected_fingerprint: Expected fingerprint from when content was read.

    Raises:
        ConflictError: If the file was modified since it was read.
    """
    current = get_fingerprint()
    if current and expected_fingerprint and current != expected_fingerprint:
        raise ConflictError(expected_fingerprint, current)
    write_content(content)


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
            return json.load(f)
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
