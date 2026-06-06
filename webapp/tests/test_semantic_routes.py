"""Tests for the semantic suggestion API routes.

The embedding service is mocked — the routes must never 500, and must
report enabled=false so the frontend silently falls back to substring
behavior when the feature is off or the backend is unavailable.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def _login(client):
    with client.session_transaction() as sess:
        sess['logged_in'] = True


class TestSemanticSuggestionsApi:
    def test_unauthorized(self, client):
        assert client.get('/api/semantic-suggestions?q=Milch').status_code == 401

    def test_empty_query(self, client):
        _login(client)
        response = client.get('/api/semantic-suggestions')
        assert response.status_code == 200
        # SEMANTIC_ENABLED defaults to false in the testing config.
        assert response.get_json() == {'enabled': False, 'items': []}

    def test_unavailable_backend(self, client, monkeypatch):
        _login(client)
        monkeypatch.setattr(
            'app.services.embedding_service.query_similar',
            lambda q, mode='tags': None,
        )
        response = client.get('/api/semantic-suggestions?q=Milch')
        assert response.status_code == 200
        assert response.get_json() == {'enabled': False, 'items': []}

    def test_returns_items(self, client, monkeypatch):
        _login(client)
        items = [{
            'marker': 'A', 'title': 'Milch kaufen', 'projects': ['haushalt'],
            'contexts': [], 'done': False, 'score': 0.91,
        }]
        captured = {}

        def fake_query(q, mode='tags'):
            captured['q'] = q
            captured['mode'] = mode
            return items

        monkeypatch.setattr(
            'app.services.embedding_service.query_similar', fake_query)
        response = client.get('/api/semantic-suggestions?q=Milch%20holen&mode=search')
        assert response.status_code == 200
        assert response.get_json() == {'enabled': True, 'items': items}
        assert captured == {'q': 'Milch holen', 'mode': 'search'}


class TestCheckDuplicateApi:
    def test_unauthorized(self, client):
        assert client.get('/api/check-duplicate?title=Milch').status_code == 401

    def test_empty_title(self, client):
        _login(client)
        response = client.get('/api/check-duplicate')
        assert response.status_code == 200
        assert response.get_json() == {'enabled': False, 'match': None}

    def test_unavailable_backend(self, client, monkeypatch):
        _login(client)
        monkeypatch.setattr(
            'app.services.embedding_service.find_duplicate',
            lambda title: (False, None),
        )
        response = client.get('/api/check-duplicate?title=Milch')
        assert response.status_code == 200
        assert response.get_json() == {'enabled': False, 'match': None}

    def test_no_match(self, client, monkeypatch):
        _login(client)
        monkeypatch.setattr(
            'app.services.embedding_service.find_duplicate',
            lambda title: (True, None),
        )
        response = client.get('/api/check-duplicate?title=Milch')
        assert response.get_json() == {'enabled': True, 'match': None}

    def test_match(self, client, monkeypatch):
        _login(client)
        match = {'marker': 'A', 'title': 'Milch kaufen', 'score': 0.93}
        monkeypatch.setattr(
            'app.services.embedding_service.find_duplicate',
            lambda title: (True, match),
        )
        response = client.get('/api/check-duplicate?title=Milch%20holen')
        assert response.get_json() == {'enabled': True, 'match': match}
