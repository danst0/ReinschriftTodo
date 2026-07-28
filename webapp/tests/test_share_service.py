"""Tests for the share service (token persistence in settings.json)."""

import os
import sys
import tempfile
from datetime import datetime, timedelta, timezone

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app import create_app
from app.services import share_service
from app.services.storage import load_settings, save_settings


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

    def test_lookup_is_case_insensitive(self, isolated_app):
        created = share_service.create_share('Einkauf')
        assert share_service.get_share_by_project('einkauf')['token'] == created['token']
        # Idempotency follows the same rule — no second token for a casing variant.
        assert share_service.create_share('EINKAUF')['token'] == created['token']


class TestContextScope:
    def test_create_context_share_stores_context_key(self, isolated_app):
        share = share_service.create_share('baumarkt', scope_type=share_service.SCOPE_CONTEXT)
        assert share['context'] == 'baumarkt'
        assert 'project' not in share
        assert share_service.share_scope(share) == ('context', 'baumarkt')

    def test_project_and_context_of_same_name_are_separate(self, isolated_app):
        proj = share_service.create_share('umzug')
        ctx = share_service.create_share('umzug', scope_type=share_service.SCOPE_CONTEXT)
        assert proj['token'] != ctx['token']
        assert share_service.get_share_by_scope('project', 'umzug')['token'] == proj['token']
        assert share_service.get_share_by_scope('context', 'umzug')['token'] == ctx['token']

    def test_get_by_scope_rejects_unknown_scope(self, isolated_app):
        share_service.create_share('einkauf')
        assert share_service.get_share_by_scope('bogus', 'einkauf') is None

    def test_create_rejects_unknown_scope(self, isolated_app):
        with pytest.raises(ValueError):
            share_service.create_share('einkauf', scope_type='bogus')

    def test_create_rejects_empty_context(self, isolated_app):
        with pytest.raises(ValueError):
            share_service.create_share('  ', scope_type=share_service.SCOPE_CONTEXT)

    def test_delete_by_scope_only_hits_matching_scope(self, isolated_app):
        proj = share_service.create_share('umzug')
        share_service.create_share('umzug', scope_type=share_service.SCOPE_CONTEXT)
        assert share_service.delete_share_by_scope('context', 'umzug') is True
        assert share_service.get_share_by_scope('context', 'umzug') is None
        assert share_service.get_share_by_token(proj['token']) is not None

    def test_delete_by_project_ignores_context_shares(self, isolated_app):
        share_service.create_share('umzug', scope_type=share_service.SCOPE_CONTEXT)
        assert share_service.delete_share_by_project('umzug') is False

    def test_share_scope_of_malformed_entry_is_none(self, isolated_app):
        assert share_service.share_scope({'token': 'x', 'created': 'y'}) is None


class TestScopeNormalization:
    def test_lookup_does_not_fold_sharp_s(self, isolated_app):
        """casefold() would treat 'Straßenfest' and 'Strassenfest' as one group."""
        a = share_service.create_share('Straßenfest')
        b = share_service.create_share('Strassenfest')
        assert a['token'] != b['token']
        assert share_service.get_share_by_project('straßenfest')['token'] == a['token']
        assert share_service.get_share_by_project('strassenfest')['token'] == b['token']

    def test_recreate_updates_stored_casing(self, isolated_app):
        created = share_service.create_share('Einkauf')
        again = share_service.create_share('EINKAUF')
        assert again['token'] == created['token']
        assert again['project'] == 'EINKAUF'
        assert share_service.get_share_by_token(created['token'])['project'] == 'EINKAUF'

    def test_delete_removes_every_casing_duplicate(self, isolated_app):
        """Legacy installs can hold two entries for one tag — revoke must kill both."""
        settings = load_settings()
        settings['shares'] = [
            {'token': 'tok-upper', 'project': 'Einkauf', 'created': 'x'},
            {'token': 'tok-lower', 'project': 'einkauf', 'created': 'x'},
        ]
        save_settings(settings)
        assert share_service.delete_share_by_project('einkauf') is True
        assert share_service.get_share_by_token('tok-upper') is None
        assert share_service.get_share_by_token('tok-lower') is None

    def test_non_ascii_token_lookup_does_not_raise(self, isolated_app):
        share_service.create_share('einkauf')
        assert share_service.get_share_by_token('töken') is None
        assert share_service.delete_share('töken') is False


def _set_share_expires_at(token: str, expires_at_iso: str | None) -> None:
    """Test helper: patch the stored ``expires_at`` for a share."""
    settings = load_settings()
    for s in settings.get('shares', []):
        if s['token'] == token:
            if expires_at_iso is None:
                s.pop('expires_at', None)
            else:
                s['expires_at'] = expires_at_iso
            break
    save_settings(settings)


class TestShareExpiration:
    def test_ttl_30_sets_expires_at_around_30_days(self, isolated_app):
        before = datetime.now(timezone.utc)
        share = share_service.create_share('foo', ttl_days=30)
        after = datetime.now(timezone.utc)

        assert 'expires_at' in share
        expires = datetime.fromisoformat(share['expires_at'])
        assert before + timedelta(days=30) - timedelta(seconds=2) <= expires
        assert expires <= after + timedelta(days=30) + timedelta(seconds=2)

    def test_ttl_none_omits_expires_at(self, isolated_app):
        share = share_service.create_share('foo', ttl_days=None)
        assert 'expires_at' not in share

    def test_ttl_default_is_no_expiration(self, isolated_app):
        # create_share() without ttl_days argument keeps backward-compatible
        # behavior (no expiration). The default-30 policy lives in the route.
        share = share_service.create_share('foo')
        assert 'expires_at' not in share

    @pytest.mark.parametrize('bad_ttl', [5, 1, 365, 0, -7])
    def test_invalid_ttl_raises(self, isolated_app, bad_ttl):
        with pytest.raises(ValueError):
            share_service.create_share('foo', ttl_days=bad_ttl)

    def test_expired_share_lookup_returns_none(self, isolated_app):
        share = share_service.create_share('foo', ttl_days=7)
        past = (datetime.now(timezone.utc) - timedelta(hours=1)).isoformat()
        _set_share_expires_at(share['token'], past)
        assert share_service.get_share_by_token(share['token']) is None

    def test_legacy_share_without_expires_at_still_works(self, isolated_app):
        # Simulates a v0.21.5 entry written before the TTL feature shipped.
        share = share_service.create_share('legacy', ttl_days=7)
        _set_share_expires_at(share['token'], None)
        found = share_service.get_share_by_token(share['token'])
        assert found is not None
        assert found['project'] == 'legacy'

    def test_malformed_expires_at_is_treated_as_unbounded(self, isolated_app):
        share = share_service.create_share('weird', ttl_days=7)
        _set_share_expires_at(share['token'], 'not-a-real-iso-string')
        assert share_service.get_share_by_token(share['token']) is not None

    def test_purge_removes_expired_keeps_active(self, isolated_app):
        a = share_service.create_share('alpha', ttl_days=7)
        b = share_service.create_share('beta', ttl_days=30)
        c = share_service.create_share('gamma', ttl_days=None)  # forever

        past = (datetime.now(timezone.utc) - timedelta(days=1)).isoformat()
        _set_share_expires_at(a['token'], past)

        removed = share_service.purge_expired_shares()
        assert removed == 1
        assert share_service.get_share_by_token(a['token']) is None
        assert share_service.get_share_by_token(b['token']) is not None
        assert share_service.get_share_by_token(c['token']) is not None

    def test_purge_when_nothing_expired_is_noop(self, isolated_app):
        share_service.create_share('foo', ttl_days=30)
        assert share_service.purge_expired_shares() == 0
