"""Tests for Web Push reminders (app/services/push_service.py and /api/push/*)."""

import json
import os
import sys
from datetime import datetime
from unittest.mock import patch

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app import create_app
from app.services import push_service
from app.services.parser import parse_line

TODOS_CONTENT = """- [ ] Zahnarzt due:2026-05-16T14:00 ^timed001
- [ ] Müll rausbringen due:2026-05-16 ^allday01
- [ ] Irgendwann due:9999-12-31T00:00 ^someday1
- [x] Schon erledigt due:2026-05-16T14:00 ✅ 2026-05-16 ^donedone
- [ ] Ohne Datum ^nodate01
- [ ] Ohne Marker due:2026-05-16T14:00
"""

SUBSCRIPTION = {
    'endpoint': 'https://push.example.com/send/abc',
    'keys': {'p256dh': 'BPUBKEY', 'auth': 'AUTHSECRET'},
}


@pytest.fixture
def push_app(tmp_path):
    todo_path = tmp_path / 'todos.md'
    todo_path.write_text(TODOS_CONTENT, encoding='utf-8')
    app = create_app('testing')
    app.config.update({
        'SECRET_KEY': 'test-secret-key',
        'TODO_PATH': str(todo_path),
        'CONFIG_PATH': str(tmp_path / 'config' / 'settings.json'),
        'USE_WEBDAV': False,
        'PUSH_ENABLED': True,
        'PUSH_LEAD_MINUTES': 15,
        'PUSH_ALL_DAY_TIME': '08:00',
    })
    push_service._vapid_cache.clear()
    yield app
    push_service._vapid_cache.clear()


@pytest.fixture
def ctx(push_app):
    with push_app.app_context():
        yield


@pytest.fixture
def client(push_app):
    client = push_app.test_client()
    with client.session_transaction() as sess:
        sess['logged_in'] = True
        sess['lang'] = 'de'
    return client


def items():
    return [item for i, line in enumerate(TODOS_CONTENT.splitlines())
            if (item := parse_line(line, i))]


def by_marker(marker):
    return next(item for item in items() if item.marker == marker)


class TestReminderTime:
    def test_timed_todo_is_announced_lead_minutes_ahead(self, ctx):
        assert push_service.reminder_time(by_marker('timed001')) == datetime(2026, 5, 16, 13, 45)

    def test_date_only_todo_is_announced_in_the_morning(self, ctx):
        assert push_service.reminder_time(by_marker('allday01')) == datetime(2026, 5, 16, 8, 0)

    def test_no_reminder_for_sometime_done_or_undated(self, ctx):
        for marker in ('someday1', 'donedone', 'nodate01'):
            assert push_service.reminder_time(by_marker(marker)) is None

    def test_invalid_all_day_time_falls_back(self, push_app, ctx):
        push_app.config['PUSH_ALL_DAY_TIME'] = '25:00'
        assert push_service.reminder_time(by_marker('allday01')) == datetime(2026, 5, 16, 8, 0)


class TestDueReminders:
    def test_before_reminder_time_nothing_is_due(self, ctx):
        assert push_service.due_reminders(items(), datetime(2026, 5, 16, 7, 59), {}) == []

    def test_due_inside_window(self, ctx):
        due = push_service.due_reminders(items(), datetime(2026, 5, 16, 13, 50), {})
        assert {item.title for item in due} == {'Zahnarzt', 'Müll rausbringen', 'Ohne Marker'}

    def test_stale_reminders_are_dropped(self, ctx):
        assert push_service.due_reminders(items(), datetime(2026, 5, 17, 9, 0), {}) == []

    def test_already_sent_is_skipped(self, ctx):
        sent = {push_service.reminder_key(by_marker('allday01')): '2026-05-16T08:00:00'}
        due = push_service.due_reminders(items(), datetime(2026, 5, 16, 8, 1), sent)
        assert due == []

    def test_postponed_todo_gets_a_new_reminder(self, ctx):
        old = by_marker('allday01')
        sent = {push_service.reminder_key(old): '2026-05-15T08:00:00'}
        moved = parse_line('- [ ] Müll rausbringen due:2026-05-17 ^allday01', 0)
        assert push_service.due_reminders([moved], datetime(2026, 5, 17, 8, 0), sent) == [moved]


class TestPayloads:
    def test_single_reminder_has_actions_and_localised_body(self, ctx):
        [payload] = push_service.build_payloads([by_marker('timed001')], 'de')
        assert payload['title'] == 'Zahnarzt'
        assert payload['body'] == 'Fällig um 14:00'
        assert payload['marker'] == 'timed001'
        assert [a['action'] for a in payload['actions']] == ['done', 'tomorrow']

    def test_all_day_body(self, ctx):
        [payload] = push_service.build_payloads([by_marker('allday01')], 'en')
        assert payload['body'] == 'Due today'

    def test_todo_without_marker_gets_no_actions(self, ctx):
        unmarked = next(item for item in items() if item.marker is None)
        [payload] = push_service.build_payloads([unmarked], 'de')
        assert 'actions' not in payload and 'marker' not in payload

    def test_many_reminders_collapse_into_summary(self, ctx):
        many = [parse_line(f'- [ ] Task {n} due:2026-05-16 ^task000{n}', n) for n in range(5)]
        [payload] = push_service.build_payloads(many, 'de')
        assert payload['title'] == '5 Aufgaben fällig'
        assert 'Task 4' in payload['body']


class TestTick:
    def test_no_subscriptions_does_not_read_the_file(self, ctx):
        runner = push_service.ReminderRunner()
        with patch('app.services.todo_service.load_todos') as load:
            assert runner.tick(datetime(2026, 5, 16, 13, 50)) == 0
        load.assert_not_called()

    def test_sends_once_and_remembers(self, ctx):
        push_service.add_subscription(SUBSCRIPTION, 'de', 'https://t.example')
        runner = push_service.ReminderRunner()
        with patch.object(push_service, 'send') as send:
            assert runner.tick(datetime(2026, 5, 16, 13, 50)) == 3
            assert runner.tick(datetime(2026, 5, 16, 13, 51)) == 0
        assert send.call_count == 3
        assert len(push_service.load_sent()) == 3

    def test_expired_subscription_is_removed(self, ctx):
        push_service.add_subscription(SUBSCRIPTION, 'de', 'https://t.example')
        runner = push_service.ReminderRunner()
        gone = push_service.SubscriptionGone(SUBSCRIPTION['endpoint'])
        with patch.object(push_service, 'send', side_effect=gone):
            runner.tick(datetime(2026, 5, 16, 13, 50))
        assert push_service.list_subscriptions() == []

    def test_total_failure_is_retried_next_tick(self, ctx):
        push_service.add_subscription(SUBSCRIPTION, 'de', 'https://t.example')
        runner = push_service.ReminderRunner()
        with patch.object(push_service, 'send', side_effect=RuntimeError('down')):
            runner.tick(datetime(2026, 5, 16, 13, 50))
        assert push_service.load_sent() == {}
        with patch.object(push_service, 'send') as send:
            runner.tick(datetime(2026, 5, 16, 13, 51))
        assert send.call_count == 3


class TestVapid:
    def test_key_is_created_once_and_reused(self, push_app, ctx):
        first = push_service.public_key()
        push_service._vapid_cache.clear()
        assert push_service.public_key() == first
        key_file = os.path.join(os.path.dirname(push_app.config['CONFIG_PATH']), 'vapid_private.pem')
        assert oct(os.stat(key_file).st_mode & 0o777) == '0o600'


class TestRoutes:
    def test_requires_login(self, push_app):
        response = push_app.test_client().get('/api/push/public-key')
        assert response.status_code == 401

    def test_public_key(self, client):
        response = client.get('/api/push/public-key')
        assert response.status_code == 200
        # Uncompressed P-256 point: 65 bytes → 87 base64url chars
        assert len(response.get_json()['publicKey']) == 87

    def test_subscribe_stores_language_and_replaces_same_endpoint(self, client, ctx):
        for _ in range(2):
            response = client.post('/api/push/subscribe', json=SUBSCRIPTION)
            assert response.status_code == 200
        [stored] = push_service.list_subscriptions()
        assert stored['lang'] == 'de'
        assert stored['subject'] == 'http://localhost'

    def test_subscribe_rejects_garbage(self, client):
        bad = {'endpoint': 'http://insecure.example', 'keys': {}}
        assert client.post('/api/push/subscribe', json=bad).status_code == 400

    def test_unsubscribe(self, client, ctx):
        client.post('/api/push/subscribe', json=SUBSCRIPTION)
        response = client.post('/api/push/unsubscribe', json={'endpoint': SUBSCRIPTION['endpoint']})
        assert response.get_json() == {'ok': True, 'removed': True}
        assert push_service.list_subscriptions() == []

    def test_test_push(self, client):
        client.post('/api/push/subscribe', json=SUBSCRIPTION)
        with patch.object(push_service, 'send') as send:
            response = client.post('/api/push/test', json={'endpoint': SUBSCRIPTION['endpoint']})
        assert response.status_code == 200
        assert send.call_args[0][1]['body'] == 'Erinnerungen sind eingerichtet.'

    def test_disabled_server_says_so(self, push_app, client):
        push_app.config['PUSH_ENABLED'] = False
        assert client.get('/api/push/public-key').status_code == 503

    def test_notification_action_completes_by_marker(self, client, push_app):
        response = client.post('/api/toggle-batch', json={
            'line_indexes': [-1], 'markers': ['timed001'], 'done': True,
        })
        assert response.get_json()['updated'] == 1
        with open(push_app.config['TODO_PATH'], encoding='utf-8') as f:
            assert '- [x] Zahnarzt' in f.read()


def test_payload_serialises(ctx):
    json.dumps(push_service.build_payloads(items(), 'ja'))
