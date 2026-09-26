"""Web Push reminders for due todos.

Browsers register a push subscription with :func:`add_subscription`; a
background loop (:func:`start_reminder_loop`) re-reads the todo file every
minute and sends one notification per todo when its reminder time arrives.

Where things live, all next to ``CONFIG_PATH`` on the persistent volume:

- ``settings.json`` → ``push_subscriptions``: one entry per device, keyed by
  its push endpoint, plus the language its notifications are written in.
- ``vapid_private.pem``: the server's VAPID key, generated on first use unless
  ``VAPID_PRIVATE_KEY`` supplies one. Replacing it invalidates every existing
  subscription, so it must survive container rebuilds.
- ``push_sent.json``: which reminders went out, so a restart does not repeat
  them.
- ``push_reminders.lock``: gunicorn runs several workers, each starting the
  loop; only the one holding this lock sends, the others keep trying in case
  it dies.

Reminder times follow the server's local clock, like every other date in the
webapp — set ``TZ`` on the container to the timezone the due dates mean.
"""

from __future__ import annotations

import base64
import fcntl
import hashlib
import json
import logging
import os
import tempfile
import threading
import time
from datetime import datetime, timedelta
from typing import Any, Optional

from flask import Flask, current_app

from app.models.todo import DEFAULT_DUE_TIME, TodoItem
from app.services.storage import get_fingerprint, load_settings, save_settings

logger = logging.getLogger(__name__)

#: How often the loop looks for due reminders.
TICK_SECONDS = 60
#: A reminder that could not go out in time (server down) is still sent this
#: late; anything older is dropped rather than arriving as a surprise.
GRACE = timedelta(hours=6)
#: More reminders than this in one tick collapse into a single summary, so the
#: morning batch of all-day todos does not bury the lock screen.
SUMMARY_THRESHOLD = 3
#: Push services keep an undelivered message this long (device offline).
PUSH_TTL_SECONDS = 6 * 3600
#: Sent-markers older than this are forgotten; their reminders are past GRACE.
SENT_RETENTION = timedelta(days=3)


# ---------------------------------------------------------------------------
# Paths and settings
# ---------------------------------------------------------------------------

def _config_dir() -> str:
    return os.path.dirname(current_app.config.get('CONFIG_PATH', '')) or '.'


def _lead_time() -> timedelta:
    return timedelta(minutes=int(current_app.config.get('PUSH_LEAD_MINUTES', 15)))


def _all_day_time() -> tuple[int, int]:
    raw = str(current_app.config.get('PUSH_ALL_DAY_TIME', '08:00'))
    try:
        hour, minute = (int(part) for part in raw.split(':', 1))
        if 0 <= hour < 24 and 0 <= minute < 60:
            return hour, minute
    except ValueError:
        pass
    logger.warning("Invalid PUSH_ALL_DAY_TIME %r, using 08:00", raw)
    return 8, 0


# ---------------------------------------------------------------------------
# VAPID keys
# ---------------------------------------------------------------------------

_vapid_lock = threading.Lock()
_vapid_cache: dict[str, Any] = {}


def _vapid():
    """Return the server's VAPID key, creating and storing it on first use."""
    from py_vapid import Vapid

    with _vapid_lock:
        cached = _vapid_cache.get('key')
        if cached is not None:
            return cached

        configured = current_app.config.get('VAPID_PRIVATE_KEY')
        if configured:
            key = Vapid.from_string(configured)
        else:
            path = os.path.join(_config_dir(), 'vapid_private.pem')
            key = _load_or_create_key_file(path)

        _vapid_cache['key'] = key
        return key


def _load_or_create_key_file(path: str):
    from py_vapid import Vapid

    if os.path.exists(path):
        return Vapid.from_file(path)

    key = Vapid()
    key.generate_keys()
    os.makedirs(os.path.dirname(path) or '.', exist_ok=True)
    # O_EXCL: two workers racing here must not end up with different keys.
    try:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        return Vapid.from_file(path)
    with os.fdopen(fd, 'wb') as handle:
        handle.write(key.private_pem())
    logger.info("Generated new VAPID key at %s", path)
    return key


def public_key() -> str:
    """The VAPID public key in the form ``PushManager.subscribe`` expects."""
    from cryptography.hazmat.primitives import serialization

    raw = _vapid().public_key.public_bytes(
        serialization.Encoding.X962,
        serialization.PublicFormat.UncompressedPoint,
    )
    return base64.urlsafe_b64encode(raw).rstrip(b'=').decode('ascii')


# ---------------------------------------------------------------------------
# Subscriptions
# ---------------------------------------------------------------------------

def list_subscriptions() -> list[dict[str, Any]]:
    subs = load_settings().get('push_subscriptions')
    return [s for s in subs if isinstance(s, dict)] if isinstance(subs, list) else []


def _save_subscriptions(subs: list[dict[str, Any]]) -> None:
    settings = load_settings()
    settings['push_subscriptions'] = subs
    save_settings(settings)


def valid_subscription(data: Any) -> bool:
    """Whether ``data`` looks like a ``PushSubscription.toJSON()`` result."""
    if not isinstance(data, dict):
        return False
    endpoint = data.get('endpoint')
    keys = data.get('keys')
    return (
        isinstance(endpoint, str) and endpoint.startswith('https://')
        and isinstance(keys, dict)
        and isinstance(keys.get('p256dh'), str) and bool(keys.get('p256dh'))
        and isinstance(keys.get('auth'), str) and bool(keys.get('auth'))
    )


def add_subscription(subscription: dict[str, Any], lang: str, subject: str) -> None:
    """Store a device's subscription, replacing an older one for its endpoint.

    ``subject`` is the site's own URL; push services want a contact in the
    VAPID claims, and Apple rejects placeholder addresses.
    """
    entry = {
        'endpoint': subscription['endpoint'],
        'keys': {
            'p256dh': subscription['keys']['p256dh'],
            'auth': subscription['keys']['auth'],
        },
        'lang': lang,
        'subject': subject,
        'created': datetime.now().isoformat(timespec='seconds'),
    }
    subs = [s for s in list_subscriptions() if s.get('endpoint') != entry['endpoint']]
    subs.append(entry)
    _save_subscriptions(subs)


def remove_subscription(endpoint: str) -> bool:
    subs = list_subscriptions()
    remaining = [s for s in subs if s.get('endpoint') != endpoint]
    if len(remaining) == len(subs):
        return False
    _save_subscriptions(remaining)
    return True


# ---------------------------------------------------------------------------
# Sending
# ---------------------------------------------------------------------------

class SubscriptionGone(Exception):
    """The push service no longer knows this subscription (404/410)."""


def send(subscription: dict[str, Any], payload: dict[str, Any]) -> None:
    """Deliver one push message. Raises :class:`SubscriptionGone` if expired."""
    from pywebpush import WebPushException, webpush

    subject = current_app.config.get('VAPID_SUBJECT') or subscription.get('subject')
    try:
        webpush(
            subscription_info={'endpoint': subscription['endpoint'],
                               'keys': subscription['keys']},
            data=json.dumps(payload),
            vapid_private_key=_vapid(),
            # webpush() writes aud/exp into this dict, so it must be fresh
            # every call — a reused one would carry an expired token.
            vapid_claims={'sub': subject or 'mailto:reinschrift@localhost'},
            ttl=PUSH_TTL_SECONDS,
            timeout=10,
        )
    except WebPushException as e:
        status = e.response.status_code if e.response is not None else None
        if status in (404, 410):
            raise SubscriptionGone(subscription['endpoint']) from e
        raise


def _translations(lang: str) -> dict[str, str]:
    from translations import TRANSLATIONS
    return TRANSLATIONS.get(lang) or TRANSLATIONS['en']


def test_payload(lang: str) -> dict[str, Any]:
    t = _translations(lang)
    return {'title': t['push_test_title'], 'body': t['push_test_body'], 'tag': 'test'}


# ---------------------------------------------------------------------------
# Reminder calculation
# ---------------------------------------------------------------------------

def reminder_time(item: TodoItem) -> Optional[datetime]:
    """When to remind about ``item``, or None if it gets no reminder.

    A todo with a time is announced ``PUSH_LEAD_MINUTES`` ahead. One with only
    a date is stored at midnight; waking someone then would be absurd, so it is
    announced on its day at ``PUSH_ALL_DAY_TIME`` instead.
    """
    if item.done or item.due is None or item.due_is_sometime:
        return None
    if item.due.time() == DEFAULT_DUE_TIME:
        hour, minute = _all_day_time()
        return item.due.replace(hour=hour, minute=minute)
    return item.due - _lead_time()


def reminder_key(item: TodoItem) -> str:
    """Identify one reminder: postponing the todo makes it a new one."""
    ident = item.marker or hashlib.sha1(item.title.encode('utf-8')).hexdigest()[:12]
    return f"{ident}|{item.due.isoformat() if item.due else ''}"


def due_reminders(items: list[TodoItem], now: datetime,
                  sent: dict[str, str]) -> list[TodoItem]:
    """Todos whose reminder is due at ``now`` and has not been sent yet."""
    result = []
    for item in items:
        at = reminder_time(item)
        if at is None or not (at <= now < at + GRACE):
            continue
        if reminder_key(item) in sent:
            continue
        result.append(item)
    return result


def build_payloads(items: list[TodoItem], lang: str) -> list[dict[str, Any]]:
    """Notification payloads for one device, in its language."""
    t = _translations(lang)
    actions = [
        {'action': 'done', 'title': t['push_action_done']},
        {'action': 'tomorrow', 'title': t['push_action_tomorrow']},
    ]

    def body(item: TodoItem) -> str:
        assert item.due is not None
        if item.due.time() == DEFAULT_DUE_TIME:
            return t['push_due_today']
        return t['push_due_at'].format(time=item.due.strftime('%H:%M'))

    if len(items) > SUMMARY_THRESHOLD:
        return [{
            'title': t['push_summary_title'].format(count=len(items)),
            'body': '\n'.join(f"• {item.title}" for item in items),
            'tag': 'summary',
        }]

    payloads = []
    for item in items:
        payload: dict[str, Any] = {
            'title': item.title,
            'body': body(item),
            'tag': item.marker or reminder_key(item),
        }
        # Actions address the todo by marker; without one there is nothing
        # safe to point them at, and a tap simply opens the app.
        if item.marker:
            payload['marker'] = item.marker
            payload['actions'] = actions
        payloads.append(payload)
    return payloads


# ---------------------------------------------------------------------------
# Sent-state persistence
# ---------------------------------------------------------------------------

def _sent_path() -> str:
    return os.path.join(_config_dir(), 'push_sent.json')


def load_sent() -> dict[str, str]:
    try:
        with open(_sent_path(), encoding='utf-8') as handle:
            data = json.load(handle)
        return data if isinstance(data, dict) else {}
    except (OSError, json.JSONDecodeError):
        return {}


def save_sent(sent: dict[str, str], now: datetime) -> None:
    cutoff = (now - SENT_RETENTION).isoformat()
    kept = {key: stamp for key, stamp in sent.items() if stamp >= cutoff}
    path = _sent_path()
    os.makedirs(os.path.dirname(path) or '.', exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path) or '.', prefix='.push_sent.')
    with os.fdopen(fd, 'w', encoding='utf-8') as handle:
        json.dump(kept, handle)
    os.replace(tmp, path)


# ---------------------------------------------------------------------------
# The loop
# ---------------------------------------------------------------------------

class ReminderRunner:
    """One tick of reminder work; keeps the parsed file between ticks."""

    def __init__(self) -> None:
        self._fingerprint: Optional[str] = None
        self._items: list[TodoItem] = []

    def _todos(self) -> list[TodoItem]:
        from app.services.todo_service import load_todos

        # A HEAD is much cheaper than fetching the file every minute. An empty
        # fingerprint means the check failed, so read to be safe.
        fingerprint = get_fingerprint()
        if not fingerprint or fingerprint != self._fingerprint:
            self._items = load_todos()
            self._fingerprint = fingerprint
        return self._items

    def tick(self, now: Optional[datetime] = None) -> int:
        """Send what is due. Returns the number of notifications sent."""
        subs = list_subscriptions()
        if not subs:
            return 0

        now = now or datetime.now()
        sent = load_sent()
        items = due_reminders(self._todos(), now, sent)
        if not items:
            return 0

        delivered = 0
        failed = 0
        gone: list[str] = []
        for sub in subs:
            for payload in build_payloads(items, sub.get('lang') or 'en'):
                try:
                    send(sub, payload)
                    delivered += 1
                except SubscriptionGone:
                    gone.append(sub['endpoint'])
                    break
                except Exception as e:  # noqa: BLE001 - one device must not stop the rest
                    failed += 1
                    logger.warning("Push to %s failed: %s",
                                   sub['endpoint'].split('/')[2], e)

        for endpoint in gone:
            logger.info("Removing expired push subscription for %s",
                        endpoint.split('/')[2])
            remove_subscription(endpoint)

        # Nothing arrived anywhere (push service or network down): try again
        # next tick. Once one device got it, retrying would repeat it there.
        if failed and not delivered:
            return 0

        stamp = now.isoformat(timespec='seconds')
        for item in items:
            sent[reminder_key(item)] = stamp
        save_sent(sent, now)
        logger.info("Sent %d reminder(s) for %d todo(s)", delivered, len(items))
        return delivered


def _try_lock(path: str):
    """Take the sender lock without waiting; returns the open file or None."""
    os.makedirs(os.path.dirname(path) or '.', exist_ok=True)
    handle = open(path, 'w')  # noqa: SIM115 - held for the process lifetime
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        handle.close()
        return None
    return handle


def start_reminder_loop(app: Flask) -> None:
    """Run reminder ticks in a daemon thread for the lifetime of the process."""

    def run() -> None:
        runner = ReminderRunner()
        lock = None
        while True:
            try:
                with app.app_context():
                    if lock is None:
                        lock = _try_lock(os.path.join(_config_dir(), 'push_reminders.lock'))
                    if lock is not None:
                        runner.tick()
            except Exception:  # noqa: BLE001 - keep the loop alive
                logger.exception("Reminder tick failed")
            time.sleep(TICK_SECONDS)

    threading.Thread(target=run, name='push-reminders', daemon=True).start()
