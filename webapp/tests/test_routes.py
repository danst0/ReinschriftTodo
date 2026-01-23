"""Integration tests for routes."""

import pytest

import sys
import os
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


class TestAuthRoutes:
    """Tests for authentication routes."""

    def test_login_page_loads(self, client):
        """Test login page loads successfully."""
        response = client.get('/login')
        assert response.status_code == 200

    def test_unauthenticated_redirect(self, client):
        """Test unauthenticated requests redirect to login."""
        response = client.get('/')
        assert response.status_code == 302
        assert '/login' in response.headers['Location']

    def test_logout(self, client):
        """Test logout redirects to login."""
        response = client.get('/logout')
        assert response.status_code == 302


class TestMainRoutes:
    """Tests for main routes."""

    def test_set_language(self, client):
        """Test language setting."""
        with client.session_transaction() as sess:
            sess['logged_in'] = True

        response = client.get('/set_language/en')
        assert response.status_code == 302

        with client.session_transaction() as sess:
            assert sess.get('lang') == 'en'

    def test_set_invalid_language(self, client):
        """Test setting invalid language doesn't change session."""
        with client.session_transaction() as sess:
            sess['logged_in'] = True
            sess['lang'] = 'de'

        response = client.get('/set_language/invalid')
        assert response.status_code == 302

        with client.session_transaction() as sess:
            assert sess.get('lang') == 'de'


class TestApiRoutes:
    """Tests for API routes."""

    def test_api_unauthorized(self, client):
        """Test API returns 401 when not logged in."""
        response = client.get('/api/todo/0')
        assert response.status_code == 401

    def test_api_parse_unauthorized(self, client):
        """Test parse API returns 401 when not logged in."""
        response = client.post('/api/parse',
                               json={'text': 'test'},
                               content_type='application/json')
        assert response.status_code == 401
