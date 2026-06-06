"""Tests for the embedding service (semantic suggestions).

All embed calls are mocked — no network access. Mirrors the Rust core
tests (core/src/embeddings.rs) where applicable.
"""

import os
import sys

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.models.todo import TodoItem
from app.services import embedding_service
from app.services.embedding_service import (
    EmbeddingError,
    canonical_text,
    cosine_similarity,
    find_duplicate,
    query_similar,
    similar_markers,
    text_hash,
)


# Deterministic 2-dim "embeddings" per canonical text.
VECTORS = {
    'Milch kaufen': [1.0, 0.0],
    'Milch besorgen': [1.0, 0.0],
    'Milch holen': [1.0, 0.0],
    'Hafermilch kaufen': [0.96, 0.28],
    'Einkauf planen': [0.6, 0.8],
    'Zahnarzt anrufen': [0.0, 1.0],
    'Wäsche waschen': [0.0, 1.0],
    # Below DUPLICATE_THRESHOLD (0.85) to every other vector.
    'Fenster putzen': [0.7, 0.7141],
}


def fake_embed(inputs):
    return [VECTORS[text] for text in inputs]


@pytest.fixture
def semantic_env(app, monkeypatch, tmp_path):
    """App context with the feature enabled and all I/O mocked."""
    app.config['SEMANTIC_ENABLED'] = True
    monkeypatch.setattr(embedding_service, '_get_embedding_config', lambda: {
        'embed_url': 'http://test/api/embed',
        'model': 'test-model',
        'timeout': 5,
        'cache_path': str(tmp_path / 'embeddings.json'),
    })
    monkeypatch.setattr(embedding_service, 'get_fingerprint', lambda: 'fp1')
    monkeypatch.setattr(embedding_service, 'embed_texts', fake_embed)
    embedding_service._cache_memo.clear()
    with app.app_context():
        yield app


class TestCanonicalText:
    def test_collapses_whitespace(self):
        assert canonical_text('  Milch   kaufen \n') == 'Milch kaufen'
        assert canonical_text('Milch kaufen') == 'Milch kaufen'

    def test_empty(self):
        assert canonical_text('   ') == ''
        assert canonical_text('') == ''


class TestTextHash:
    def test_stable_and_distinct(self):
        assert text_hash('Milch kaufen') == text_hash('Milch kaufen')
        assert text_hash('Milch kaufen') != text_hash('Brot kaufen')


class TestCosineSimilarity:
    def test_identical_orthogonal_commutative(self):
        a, b = [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]
        assert cosine_similarity(a, a) == pytest.approx(1.0)
        assert cosine_similarity(a, b) == pytest.approx(0.0)
        assert cosine_similarity(a, b) == cosine_similarity(b, a)

    def test_degenerate_inputs(self):
        assert cosine_similarity([1.0], [1.0, 0.0]) == 0.0
        assert cosine_similarity([1.0, 0.0], [0.0, 0.0]) == 0.0
        assert cosine_similarity([], []) == 0.0


class TestSimilarMarkers:
    CACHE = {'items': {
        'A': {'text_hash': 'h1', 'vector': [1.0, 0.0]},
        'B': {'text_hash': 'h2', 'vector': [0.9, 0.1]},
        'C': {'text_hash': 'h3', 'vector': [0.0, 1.0]},
    }}

    def test_sorting_and_threshold(self):
        hits = similar_markers(self.CACHE, [1.0, 0.0], top_k=10, threshold=0.5)
        assert [marker for marker, _ in hits] == ['A', 'B']
        assert hits[0][1] > hits[1][1]

    def test_top_k(self):
        hits = similar_markers(self.CACHE, [1.0, 0.0], top_k=1, threshold=0.5)
        assert [marker for marker, _ in hits] == ['A']

    def test_allowed_markers(self):
        hits = similar_markers(self.CACHE, [1.0, 0.0], top_k=10, threshold=0.5,
                               allowed_markers={'B'})
        assert [marker for marker, _ in hits] == ['B']


class TestEnsureIndex:
    def _items(self):
        return [
            TodoItem(line_index=0, marker='A', title='Milch kaufen'),
            TodoItem(line_index=1, marker='B', title='Zahnarzt anrufen'),
        ]

    def test_initial_build_and_persist(self, semantic_env, monkeypatch, tmp_path):
        monkeypatch.setattr(embedding_service, 'load_todos', self._items)
        cache = embedding_service.ensure_index()
        assert set(cache['items'].keys()) == {'A', 'B'}
        assert cache['dim'] == 2
        assert cache['db_fingerprint'] == 'fp1'
        assert (tmp_path / 'embeddings.json').exists()

    def test_unchanged_skips_embedding(self, semantic_env, monkeypatch):
        monkeypatch.setattr(embedding_service, 'load_todos', self._items)
        calls = []

        def counting_embed(inputs):
            calls.append(inputs)
            return fake_embed(inputs)

        monkeypatch.setattr(embedding_service, 'embed_texts', counting_embed)
        embedding_service.ensure_index()
        first = sum(len(c) for c in calls)
        embedding_service._cache_memo.clear()  # force re-read from disk
        embedding_service.ensure_index()
        assert sum(len(c) for c in calls) == first == 2

    def test_incremental_diff_and_eviction(self, semantic_env, monkeypatch):
        monkeypatch.setattr(embedding_service, 'load_todos', self._items)
        embedding_service.ensure_index()

        calls = []

        def counting_embed(inputs):
            calls.append(inputs)
            return fake_embed(inputs)

        # A changes title, B is deleted, C is new.
        changed = [
            TodoItem(line_index=0, marker='A', title='Hafermilch kaufen'),
            TodoItem(line_index=1, marker='C', title='Wäsche waschen'),
        ]
        monkeypatch.setattr(embedding_service, 'load_todos', lambda: changed)
        monkeypatch.setattr(embedding_service, 'get_fingerprint', lambda: 'fp2')
        monkeypatch.setattr(embedding_service, 'embed_texts', counting_embed)
        cache = embedding_service.ensure_index()
        assert set(cache['items'].keys()) == {'A', 'C'}
        assert sum(len(c) for c in calls) == 2
        assert cache['db_fingerprint'] == 'fp2'

    def test_skips_unmarked_and_empty(self, semantic_env, monkeypatch):
        items = [
            TodoItem(line_index=0, marker=None, title='Ohne Marker'),
            TodoItem(line_index=1, marker='E', title='   '),
        ]
        monkeypatch.setattr(embedding_service, 'load_todos', lambda: items)
        cache = embedding_service.ensure_index()
        assert cache['items'] == {}

    def test_model_change_resets_cache(self, tmp_path):
        path = str(tmp_path / 'cache.json')
        cache = embedding_service._empty_cache('old-model')
        cache['items']['A'] = {'text_hash': 'h', 'vector': [1.0]}
        embedding_service._save_cache(path, cache)
        loaded = embedding_service._load_cache(path, 'new-model')
        assert loaded['items'] == {}
        assert loaded['model'] == 'new-model'


class TestQuerySimilar:
    def _items(self):
        return [
            TodoItem(line_index=0, marker='A', title='Milch kaufen',
                     projects=['haushalt'], contexts=['laden']),
            TodoItem(line_index=1, marker='B', title='Einkauf planen'),
            TodoItem(line_index=2, marker='C', title='Zahnarzt anrufen'),
        ]

    def test_finds_similar_with_tags(self, semantic_env, monkeypatch):
        monkeypatch.setattr(embedding_service, 'load_todos', self._items)
        results = query_similar('Milch holen', mode='tags')
        assert results is not None
        markers = [r['marker'] for r in results]
        assert markers == ['A', 'B']  # 1.0 and 0.6, both >= 0.55
        assert results[0]['projects'] == ['haushalt']
        assert results[0]['contexts'] == ['laden']

    def test_excludes_identical_title(self, semantic_env, monkeypatch):
        items = self._items() + [
            TodoItem(line_index=3, marker='D', title='Milch  holen'),
        ]
        monkeypatch.setattr(embedding_service, 'load_todos', lambda: items)
        results = query_similar('Milch holen', mode='tags')
        assert 'D' not in [r['marker'] for r in results]

    def test_disabled_returns_none(self, app):
        app.config['SEMANTIC_ENABLED'] = False
        with app.app_context():
            assert query_similar('Milch holen') is None

    def test_backend_error_returns_none(self, semantic_env, monkeypatch):
        monkeypatch.setattr(embedding_service, 'load_todos', self._items)

        def failing_embed(inputs):
            raise EmbeddingError('model not pulled (404)')

        monkeypatch.setattr(embedding_service, 'embed_texts', failing_embed)
        assert query_similar('Milch holen') is None


class TestFindDuplicate:
    def _items(self):
        return [
            TodoItem(line_index=0, marker='A', title='Milch kaufen', done=False),
            TodoItem(line_index=1, marker='B', title='Milch besorgen', done=True),
            TodoItem(line_index=2, marker='C', title='Zahnarzt anrufen', done=False),
        ]

    def test_finds_open_duplicate(self, semantic_env, monkeypatch):
        monkeypatch.setattr(embedding_service, 'load_todos', self._items)
        available, match = find_duplicate('Milch holen')
        assert available is True
        assert match is not None
        assert match['marker'] == 'A'  # B is just as similar but done

    def test_no_match_below_threshold(self, semantic_env, monkeypatch):
        monkeypatch.setattr(embedding_service, 'load_todos', self._items)
        available, match = find_duplicate('Fenster putzen')
        assert available is True
        assert match is None

    def test_disabled(self, app):
        app.config['SEMANTIC_ENABLED'] = False
        with app.app_context():
            assert find_duplicate('Milch holen') == (False, None)

    def test_backend_error(self, semantic_env, monkeypatch):
        monkeypatch.setattr(embedding_service, 'load_todos', self._items)

        def failing_embed(inputs):
            raise EmbeddingError('unreachable')

        monkeypatch.setattr(embedding_service, 'embed_texts', failing_embed)
        assert find_duplicate('Milch holen') == (False, None)
