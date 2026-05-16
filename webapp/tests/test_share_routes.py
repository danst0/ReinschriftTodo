"""Tests for the public share routes /s/<token>/... and owner /api/shares/..."""

import json
import os
import sys
import tempfile

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app import create_app
from app.services import share_service


TODOS_CONTENT = """- [ ] Eier kaufen +einkauf due:2026-05-16T12:00 ^aaaaaa11
- [ ] Brot kaufen +einkauf due:2026-05-16T12:00 ^aaaaaa22
- [x] Milch kaufen +einkauf due:2026-05-15T12:00 ✅ 2026-05-15 ^aaaaaa33
- [ ] Steuer machen +arbeit due:2026-05-20T12:00 ^bbbbbb11
"""


@pytest.fixture
def share_app():
    todo_fd, todo_path = tempfile.mkstemp(suffix='.md')
    with os.fdopen(todo_fd, 'w', encoding='utf-8') as f:
        f.write(TODOS_CONTENT)

    cfg_fd, cfg_path = tempfile.mkstemp(suffix='.json')
    os.close(cfg_fd)
    os.unlink(cfg_path)

    app = create_app('testing')
    app.config.update({
        'TESTING': True,
        'WTF_CSRF_ENABLED': False,
        'SECRET_KEY': 'test-secret-key',
        'TODO_PATH': todo_path,
        'CONFIG_PATH': cfg_path,
        'USE_WEBDAV': False,
        'SERVER_NAME': 'localhost.test',
    })

    yield app

    if os.path.exists(todo_path):
        os.unlink(todo_path)
    if os.path.exists(cfg_path):
        os.unlink(cfg_path)


@pytest.fixture
def client(share_app):
    return share_app.test_client()


def _login(client):
    with client.session_transaction() as sess:
        sess['logged_in'] = True


def _read_todo(share_app):
    with open(share_app.config['TODO_PATH'], 'r', encoding='utf-8') as f:
        return f.read()


class TestPublicShareView:
    def test_view_shows_open_todos_with_project(self, share_app, client):
        with share_app.app_context():
            share = share_service.create_share('einkauf')
        resp = client.get(f"/s/{share['token']}")
        assert resp.status_code == 200
        body = resp.get_data(as_text=True)
        assert 'Eier kaufen' in body
        assert 'Brot kaufen' in body
        # Done todo must not appear
        assert 'Milch kaufen' not in body
        # Other project's todo must not appear
        assert 'Steuer machen' not in body

    def test_view_empty_list_returns_200(self, share_app, client):
        """Link stays valid even when no open todos match — core requirement."""
        with share_app.app_context():
            share = share_service.create_share('nonexistent-project')
        resp = client.get(f"/s/{share['token']}")
        assert resp.status_code == 200
        body = resp.get_data(as_text=True)
        # No todos rendered, but the page should load successfully.
        assert 'Eier kaufen' not in body

    def test_view_unknown_token_returns_404(self, share_app, client):
        resp = client.get('/s/totally-fake-token-1234567890')
        assert resp.status_code == 404


class TestPublicToggle:
    def test_toggle_within_scope(self, share_app, client):
        with share_app.app_context():
            share = share_service.create_share('einkauf')
        resp = client.post(f"/s/{share['token']}/toggle/aaaaaa11")
        assert resp.status_code == 200
        content = _read_todo(share_app)
        # The toggled line should now be done
        assert '- [x] Eier kaufen' in content

    def test_toggle_outside_scope_is_forbidden(self, share_app, client):
        with share_app.app_context():
            share = share_service.create_share('einkauf')
        # Marker bbbbbb11 belongs to +arbeit, not the share's +einkauf
        resp = client.post(f"/s/{share['token']}/toggle/bbbbbb11")
        assert resp.status_code == 403
        # Ensure the line wasn't touched
        content = _read_todo(share_app)
        assert '- [ ] Steuer machen' in content

    def test_toggle_unknown_marker_returns_404(self, share_app, client):
        with share_app.app_context():
            share = share_service.create_share('einkauf')
        resp = client.post(f"/s/{share['token']}/toggle/zzzzzz99")
        assert resp.status_code == 404

    def test_toggle_already_done_returns_409(self, share_app, client):
        with share_app.app_context():
            share = share_service.create_share('einkauf')
        # aaaaaa33 is already done
        resp = client.post(f"/s/{share['token']}/toggle/aaaaaa33")
        assert resp.status_code == 409


class TestPublicAdd:
    def test_add_forces_project_tag(self, share_app, client):
        with share_app.app_context():
            share = share_service.create_share('einkauf')
        resp = client.post(
            f"/s/{share['token']}/add",
            json={'title': 'Käse +sonst @market'}
        )
        assert resp.status_code == 200
        content = _read_todo(share_app)
        assert 'Käse' in content
        # New todo must have +einkauf
        new_line = [l for l in content.splitlines() if 'Käse' in l][0]
        assert '+einkauf' in new_line
        # User-provided +sonst must have been stripped
        assert '+sonst' not in new_line
        # Context should be preserved
        assert '@market' in new_line

    def test_add_empty_title_rejected(self, share_app, client):
        with share_app.app_context():
            share = share_service.create_share('einkauf')
        resp = client.post(f"/s/{share['token']}/add", json={'title': '   '})
        assert resp.status_code == 400

    def test_add_unknown_token_404(self, share_app, client):
        resp = client.post('/s/totally-fake/add', json={'title': 'x'})
        assert resp.status_code == 404

    def test_add_multiword_project_is_quoted(self, share_app, client):
        with share_app.app_context():
            share = share_service.create_share('Geburtstag Mama')
        resp = client.post(
            f"/s/{share['token']}/add",
            json={'title': 'Karte schreiben'}
        )
        assert resp.status_code == 200
        content = _read_todo(share_app)
        new_line = [l for l in content.splitlines() if 'Karte schreiben' in l][0]
        assert '+"Geburtstag Mama"' in new_line


class TestOwnerShareApi:
    def test_get_share_requires_login(self, client):
        resp = client.get('/api/shares/einkauf')
        assert resp.status_code == 401

    def test_create_share_requires_login(self, client):
        resp = client.post('/api/shares/einkauf')
        assert resp.status_code == 401

    def test_delete_share_requires_login(self, client):
        resp = client.delete('/api/shares/einkauf')
        assert resp.status_code == 401

    def test_create_and_get_share_flow(self, share_app, client):
        _login(client)
        # Initially no share
        resp = client.get('/api/shares/einkauf')
        assert resp.status_code == 200
        assert resp.get_json()['token'] is None

        # Create
        resp = client.post('/api/shares/einkauf')
        assert resp.status_code == 200
        data = resp.get_json()
        assert data['token']
        assert data['url']
        assert '/s/' in data['url']

        # GET returns same token
        resp2 = client.get('/api/shares/einkauf')
        assert resp2.get_json()['token'] == data['token']

        # DELETE removes it
        resp = client.delete('/api/shares/einkauf')
        assert resp.status_code == 200
        assert resp.get_json()['ok'] is True

        # GET now returns null again
        resp = client.get('/api/shares/einkauf')
        assert resp.get_json()['token'] is None
