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


class TestTitleSuggestionsApi:
    """Tests for /api/title-suggestions endpoint."""

    def _login(self, client):
        with client.session_transaction() as sess:
            sess['logged_in'] = True

    def test_unauthorized(self, client):
        response = client.get('/api/title-suggestions')
        assert response.status_code == 401

    def test_empty(self, client, monkeypatch):
        self._login(client)
        monkeypatch.setattr('app.routes.api.load_todos', lambda: [])
        response = client.get('/api/title-suggestions')
        assert response.status_code == 200
        assert response.get_json() == {'titles': [], 'items': []}

    def test_single_title(self, client, monkeypatch):
        from app.models.todo import TodoItem
        self._login(client)
        items = [TodoItem(line_index=0, marker='m1', title='Buy milk')]
        monkeypatch.setattr('app.routes.api.load_todos', lambda: items)
        response = client.get('/api/title-suggestions')
        assert response.status_code == 200
        assert response.get_json() == {
            'titles': ['Buy milk'],
            'items': [{'title': 'Buy milk', 'marker': 'm1'}],
        }

    def test_frequency_sort_with_duplicates(self, client, monkeypatch):
        from app.models.todo import TodoItem
        self._login(client)
        items = [
            TodoItem(line_index=0, title='Apple'),
            TodoItem(line_index=1, title='Banana'),
            TodoItem(line_index=2, title='Apple'),
            TodoItem(line_index=3, title='Cherry'),
            TodoItem(line_index=4, title='Apple'),
            TodoItem(line_index=5, title='Banana'),
        ]
        monkeypatch.setattr('app.routes.api.load_todos', lambda: items)
        response = client.get('/api/title-suggestions')
        assert response.status_code == 200
        # Apple (3) > Banana (2) > Cherry (1); ties alphabetical.
        assert response.get_json()['titles'] == ['Apple', 'Banana', 'Cherry']

    def test_includes_completed_and_skips_empty(self, client, monkeypatch):
        from app.models.todo import TodoItem
        self._login(client)
        items = [
            TodoItem(line_index=0, title='Open task'),
            TodoItem(line_index=1, title='', done=True),
            TodoItem(line_index=2, title='   ', done=False),
            TodoItem(line_index=3, title='Done task', done=True),
        ]
        monkeypatch.setattr('app.routes.api.load_todos', lambda: items)
        response = client.get('/api/title-suggestions')
        assert response.status_code == 200
        # Whitespace-only titles excluded; both open and done included.
        assert response.get_json()['titles'] == ['Done task', 'Open task']
