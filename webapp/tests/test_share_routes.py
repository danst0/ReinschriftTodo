"""Tests for the public share routes /s/<token>/... and owner /api/shares/..."""

import json
import os
import sys
import tempfile
from datetime import datetime, timedelta, timezone

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app import create_app
from app.services import share_service
from app.services.storage import load_settings, save_settings


TODOS_CONTENT = """- [ ] Eier kaufen +einkauf @supermarkt due:2026-05-16T12:00 ^aaaaaa11
- [ ] Brot kaufen +einkauf @supermarkt due:2026-05-16T12:00 ^aaaaaa22
- [x] Milch kaufen +einkauf @supermarkt due:2026-05-15T12:00 ✅ 2026-05-15 ^aaaaaa33
- [ ] Steuer machen +arbeit @buero due:2026-05-20T12:00 ^bbbbbb11
- [ ] Schrauben holen +heimwerken @Baumarkt due:2026-05-18T12:00 ^cccccc11
- [ ] Farbe holen +heimwerken @baumarkt due:2026-05-19T12:00 ^cccccc22
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


class TestContextShare:
    """Location/context shares (@tag) behave like project shares (+tag)."""

    def _context_share(self, share_app, name='Baumarkt'):
        with share_app.app_context():
            return share_service.create_share(
                name, scope_type=share_service.SCOPE_CONTEXT)

    def test_view_shows_open_todos_with_context(self, share_app, client):
        share = self._context_share(share_app)
        resp = client.get(f"/s/{share['token']}")
        assert resp.status_code == 200
        body = resp.get_data(as_text=True)
        assert 'Schrauben holen' in body
        # Casing variant of the same location belongs to the same list
        assert 'Farbe holen' in body
        # Other locations must not appear
        assert 'Eier kaufen' not in body
        assert 'Steuer machen' not in body
        # Header shows the @ sigil, not +
        assert '@Baumarkt' in body

    def test_toggle_within_context_scope(self, share_app, client):
        share = self._context_share(share_app)
        resp = client.post(f"/s/{share['token']}/toggle/cccccc11")
        assert resp.status_code == 200
        assert '- [x] Schrauben holen' in _read_todo(share_app)

    def test_toggle_outside_context_scope_is_forbidden(self, share_app, client):
        share = self._context_share(share_app)
        resp = client.post(f"/s/{share['token']}/toggle/aaaaaa11")
        assert resp.status_code == 403
        assert '- [ ] Eier kaufen' in _read_todo(share_app)

    def test_add_forces_context_tag_and_keeps_project(self, share_app, client):
        share = self._context_share(share_app)
        resp = client.post(
            f"/s/{share['token']}/add",
            json={'title': 'Dübel kaufen +heimwerken @woanders'}
        )
        assert resp.status_code == 200
        new_line = [l for l in _read_todo(share_app).splitlines() if 'Dübel' in l][0]
        assert '@Baumarkt' in new_line
        # User-provided context must have been stripped, project preserved
        assert '@woanders' not in new_line
        assert '+heimwerken' in new_line

    def test_add_multiword_context_is_quoted(self, share_app, client):
        share = self._context_share(share_app, 'Grüner Markt')
        resp = client.post(f"/s/{share['token']}/add", json={'title': 'Äpfel'})
        assert resp.status_code == 200
        new_line = [l for l in _read_todo(share_app).splitlines() if 'Äpfel' in l][0]
        assert '@"Grüner Markt"' in new_line

    def test_project_share_does_not_match_same_named_context(self, share_app, client):
        """A +einkauf share must not expose @einkauf todos (and vice versa)."""
        with share_app.app_context():
            proj = share_service.create_share('supermarkt')
        resp = client.get(f"/s/{proj['token']}")
        assert resp.status_code == 200
        # No todo carries +supermarkt — only @supermarkt
        assert 'Eier kaufen' not in resp.get_data(as_text=True)

    def test_owner_api_context_flow(self, share_app, client):
        _login(client)
        resp = client.get('/api/shares/Baumarkt?type=context')
        assert resp.status_code == 200
        assert resp.get_json()['token'] is None

        resp = client.post('/api/shares/Baumarkt?type=context')
        assert resp.status_code == 200
        data = resp.get_json()
        assert data['token'] and data['url']
        assert data['scope'] == 'context'
        assert data['name'] == 'Baumarkt'
        assert data['project'] is None

        # A project share of the same name is a separate link
        resp = client.get('/api/shares/Baumarkt')
        assert resp.get_json()['token'] is None

        resp = client.get('/api/shares/Baumarkt?type=context')
        assert resp.get_json()['token'] == data['token']

        resp = client.delete('/api/shares/Baumarkt?type=context')
        assert resp.get_json()['ok'] is True
        resp = client.get('/api/shares/Baumarkt?type=context')
        assert resp.get_json()['token'] is None

    def test_owner_api_rejects_unknown_scope(self, share_app, client):
        _login(client)
        resp = client.get('/api/shares/Baumarkt?type=bogus')
        assert resp.status_code == 400
        assert resp.get_json()['error'] == 'scope_required'


class TestShareButtonRendering:
    """The group header offers sharing in both grouping modes."""

    @pytest.mark.parametrize('sort_mode,scope', [('topic', 'project'), ('location', 'context')])
    def test_group_header_has_share_button(self, share_app, client, sort_mode, scope):
        _login(client)
        resp = client.get(f'/?sort_mode={sort_mode}&partial=1')
        assert resp.status_code == 200
        body = resp.get_data(as_text=True)
        assert f'data-share-scope="{scope}"' in body

    def test_date_mode_has_no_share_button(self, share_app, client):
        _login(client)
        resp = client.get('/?sort_mode=date&partial=1')
        assert resp.status_code == 200
        assert 'data-share-scope' not in resp.get_data(as_text=True)


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


class TestShareTtlApi:
    def test_create_with_ttl_30_sets_expires_at(self, share_app, client):
        _login(client)
        resp = client.post('/api/shares/einkauf', json={'ttl_days': 30})
        assert resp.status_code == 200
        data = resp.get_json()
        assert data['expires_at'] is not None
        expires = datetime.fromisoformat(data['expires_at'])
        now = datetime.now(timezone.utc)
        assert timedelta(days=29) < expires - now < timedelta(days=31)

    def test_create_with_ttl_none_has_no_expiration(self, share_app, client):
        _login(client)
        resp = client.post('/api/shares/einkauf', json={'ttl_days': None})
        assert resp.status_code == 200
        assert resp.get_json()['expires_at'] is None

    def test_create_without_body_defaults_to_30_days(self, share_app, client):
        _login(client)
        resp = client.post('/api/shares/einkauf')
        assert resp.status_code == 200
        data = resp.get_json()
        assert data['expires_at'] is not None
        expires = datetime.fromisoformat(data['expires_at'])
        now = datetime.now(timezone.utc)
        assert timedelta(days=29) < expires - now < timedelta(days=31)

    @pytest.mark.parametrize('bad_ttl', [5, 1, 365, 0, -7, 'thirty', True])
    def test_create_with_invalid_ttl_returns_400(self, share_app, client, bad_ttl):
        _login(client)
        resp = client.post('/api/shares/einkauf', json={'ttl_days': bad_ttl})
        assert resp.status_code == 400
        assert resp.get_json()['error'] == 'invalid_ttl'

    def test_expired_share_view_returns_404(self, share_app, client):
        with share_app.app_context():
            share = share_service.create_share('einkauf', ttl_days=7)
            settings = load_settings()
            past = (datetime.now(timezone.utc) - timedelta(hours=1)).isoformat()
            for s in settings['shares']:
                if s['token'] == share['token']:
                    s['expires_at'] = past
            save_settings(settings)

        resp = client.get(f"/s/{share['token']}")
        assert resp.status_code == 404

    def test_owner_get_after_expiration_purges_and_returns_null(self, share_app, client):
        _login(client)
        with share_app.app_context():
            share = share_service.create_share('einkauf', ttl_days=7)
            settings = load_settings()
            past = (datetime.now(timezone.utc) - timedelta(hours=1)).isoformat()
            for s in settings['shares']:
                if s['token'] == share['token']:
                    s['expires_at'] = past
            save_settings(settings)

        resp = client.get('/api/shares/einkauf')
        assert resp.status_code == 200
        assert resp.get_json()['token'] is None

        with share_app.app_context():
            shares = load_settings().get('shares', [])
            assert shares == []
