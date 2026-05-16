"""Tests for the share service (token persistence in settings.json)."""

import os
import sys
import tempfile

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app import create_app
from app.services import share_service


@pytest.fixture
def isolated_app():
    """App with isolated temp settings file so share state doesn't leak."""
    cfg_fd, cfg_path = tempfile.mkstemp(suffix='.json')
    os.close(cfg_fd)
    os.unlink(cfg_path)  # let load_settings see "no file" initially

    app = create_app('testing')
    app.config.update({
        'TESTING': True,
        'WTF_CSRF_ENABLED': False,
        'SECRET_KEY': 'test-secret-key',
        'CONFIG_PATH': cfg_path,
        'USE_WEBDAV': False,
    })

    with app.app_context():
        yield app

    if os.path.exists(cfg_path):
        os.unlink(cfg_path)


class TestShareService:
    def test_create_share_returns_token_and_project(self, isolated_app):
        share = share_service.create_share('einkauf')
        assert share['project'] == 'einkauf'
        assert isinstance(share['token'], str)
        assert len(share['token']) >= 20

    def test_create_share_is_idempotent(self, isolated_app):
        first = share_service.create_share('einkauf')
        second = share_service.create_share('einkauf')
        assert first['token'] == second['token']

    def test_get_by_token_returns_share(self, isolated_app):
        created = share_service.create_share('shopping')
        found = share_service.get_share_by_token(created['token'])
        assert found is not None
        assert found['project'] == 'shopping'

    def test_get_by_token_returns_none_for_unknown(self, isolated_app):
        share_service.create_share('einkauf')
        assert share_service.get_share_by_token('not-a-real-token') is None
        assert share_service.get_share_by_token('') is None
        assert share_service.get_share_by_token(None) is None  # type: ignore[arg-type]

    def test_get_by_project_returns_share(self, isolated_app):
        share_service.create_share('travel')
        found = share_service.get_share_by_project('travel')
        assert found is not None and found['project'] == 'travel'

    def test_delete_share_removes_it(self, isolated_app):
        created = share_service.create_share('einkauf')
        assert share_service.delete_share(created['token']) is True
        assert share_service.get_share_by_token(created['token']) is None
        assert share_service.delete_share(created['token']) is False  # no-op on second call

    def test_delete_share_by_project(self, isolated_app):
        share_service.create_share('einkauf')
        assert share_service.delete_share_by_project('einkauf') is True
        assert share_service.get_share_by_project('einkauf') is None

    def test_create_share_rejects_empty_project(self, isolated_app):
        with pytest.raises(ValueError):
            share_service.create_share('')

    def test_multiple_projects_get_independent_tokens(self, isolated_app):
        a = share_service.create_share('a')
        b = share_service.create_share('b')
        assert a['token'] != b['token']
        assert share_service.get_share_by_token(a['token'])['project'] == 'a'
        assert share_service.get_share_by_token(b['token'])['project'] == 'b'
