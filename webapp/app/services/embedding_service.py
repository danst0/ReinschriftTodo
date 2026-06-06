"""Semantic embeddings via Ollama's /api/embed endpoint.

Deliberate parallel port of the Rust core module (core/src/embeddings.rs)
— keep canonicalization, thresholds and the cache schema in sync. The
sidecar cache lives next to CONFIG_PATH (persistent /config volume in
Docker), never next to the markdown database (WebDAV has no local
"next to"). Caches are per-stack: the GUI keeps its own.

Everything degrades gracefully: if the feature is disabled or Ollama is
unreachable, the public helpers return None / (False, None) and callers
fall back to plain substring behavior.
"""

import hashlib
import json
import logging
import math
import os
import tempfile
from typing import Any, Optional

import requests
from flask import current_app

from app.services.storage import get_fingerprint
from app.services.todo_service import load_todos

logger = logging.getLogger(__name__)

SCHEMA_VERSION = 1

# Number of texts sent per /api/embed request when (re)building the index.
EMBED_BATCH_SIZE = 64

# Thresholds and result limits per feature (cosine similarity).
# Keep in sync with core/src/embeddings.rs.
TAG_SUGGEST_THRESHOLD = 0.55
TAG_SUGGEST_TOP_K = 5
SEARCH_THRESHOLD = 0.50
SEARCH_TOP_K = 10
DUPLICATE_THRESHOLD = 0.85

# In-memory memo of the last loaded cache, keyed by (model, fingerprint),
# so we don't re-read the JSON sidecar on every request.
_cache_memo: dict[str, Any] = {}


class EmbeddingError(Exception):
    """Embedding backend unavailable or returned an invalid response."""


def is_semantic_enabled() -> bool:
    """Whether semantic suggestions are enabled (SEMANTIC_ENABLED)."""
    try:
        return bool(current_app.config.get('SEMANTIC_ENABLED', False))
    except RuntimeError:
        return False


def canonical_text(title: str) -> str:
    """Canonical form of a title for embedding: trimmed, whitespace
    collapsed. Must match the Rust core's ``canonical_text`` exactly."""
    return ' '.join((title or '').split())


def text_hash(canonical: str) -> str:
    """Stable per-stack hash of the canonical text (sha256 hex).

    Does not need to match the Rust side (FNV-1a) — caches are separate.
    """
    return hashlib.sha256(canonical.encode('utf-8')).hexdigest()


def _get_embedding_config() -> dict[str, Any]:
    """Resolve the embed endpoint from the existing OLLAMA_URL resolution."""
    # Reuse the LLM URL resolution (Flask config → config.json → env →
    # default) and swap the endpoint: .../api/chat → .../api/embed.
    from app.services.ai_service import _get_llm_config

    llm = _get_llm_config()
    chat_url = llm['chat_url']
    base = chat_url[: -len('/api/chat')] if chat_url.endswith('/api/chat') else chat_url.rstrip('/')
    return {
        'embed_url': base.rstrip('/') + '/api/embed',
        'model': current_app.config.get('EMBEDDING_MODEL', 'bge-m3'),
        'timeout': int(current_app.config.get('EMBEDDING_TIMEOUT', 30)),
        'cache_path': current_app.config.get('EMBEDDING_CACHE_PATH', 'embeddings.json'),
    }


def embed_texts(inputs: list[str]) -> list[list[float]]:
    """Fetch embeddings for the given texts from Ollama.

    Raises EmbeddingError on any failure (404 = model not pulled).
    """
    if not inputs:
        return []
    cfg = _get_embedding_config()
    try:
        response = requests.post(
            cfg['embed_url'],
            json={'model': cfg['model'], 'input': inputs},
            timeout=cfg['timeout'],
        )
    except requests.RequestException as exc:
        raise EmbeddingError(f"embed request failed: {exc}") from exc
    if response.status_code == 404:
        raise EmbeddingError(
            f"embedding model '{cfg['model']}' not available (404) — "
            f"run `ollama pull {cfg['model']}`"
        )
    if response.status_code != 200:
        raise EmbeddingError(f"embed request failed with status {response.status_code}")
    try:
        embeddings = response.json().get('embeddings')
    except ValueError as exc:
        raise EmbeddingError(f"invalid embed response: {exc}") from exc
    if not isinstance(embeddings, list) or len(embeddings) != len(inputs):
        raise EmbeddingError(
            f"embed response count mismatch: {len(inputs)} inputs, "
            f"{len(embeddings) if isinstance(embeddings, list) else 'no'} vectors"
        )
    return embeddings


def cosine_similarity(a: list[float], b: list[float]) -> float:
    """Cosine similarity between two vectors (0.0 for mismatched/zero)."""
    if len(a) != len(b) or not a:
        return 0.0
    dot = 0.0
    norm_a = 0.0
    norm_b = 0.0
    for x, y in zip(a, b):
        dot += x * y
        norm_a += x * x
        norm_b += y * y
    if norm_a == 0.0 or norm_b == 0.0:
        return 0.0
    return dot / (math.sqrt(norm_a) * math.sqrt(norm_b))


def _empty_cache(model: str) -> dict[str, Any]:
    return {
        'schema_version': SCHEMA_VERSION,
        'model': model,
        'dim': 0,
        'db_fingerprint': '',
        'items': {},
    }


def _load_cache(path: str, model: str) -> dict[str, Any]:
    """Load the sidecar cache; empty cache on any error or schema/model
    mismatch (different model ⇒ incompatible vector space ⇒ full reset)."""
    try:
        with open(path, 'r', encoding='utf-8') as f:
            cache = json.load(f)
        if (
            isinstance(cache, dict)
            and cache.get('schema_version') == SCHEMA_VERSION
            and cache.get('model') == model
            and isinstance(cache.get('items'), dict)
        ):
            return cache
    except (IOError, ValueError):
        pass
    return _empty_cache(model)


def _save_cache(path: str, cache: dict[str, Any]) -> None:
    """Persist the cache atomically (tmp file + rename)."""
    directory = os.path.dirname(path) or '.'
    os.makedirs(directory, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=directory, suffix='.tmp')
    try:
        with os.fdopen(fd, 'w', encoding='utf-8') as f:
            json.dump(cache, f)
        os.replace(tmp, path)
    except Exception:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def ensure_index() -> dict[str, Any]:
    """Ensure the cache covers the current todos: embed new/changed titles
    (batched), evict deleted markers, update the fingerprint and persist.

    Items without a marker or with an empty title are ignored.
    Raises EmbeddingError if the backend is unavailable.
    """
    cfg = _get_embedding_config()
    fingerprint = get_fingerprint()

    memo_key = f"{cfg['model']}@{fingerprint}"
    memoized = _cache_memo.get(memo_key)
    if memoized is not None:
        return memoized  # type: ignore[no-any-return]

    cache = _load_cache(cfg['cache_path'], cfg['model'])

    # Canonical view of the current items: marker → (canonical, hash).
    current: dict[str, tuple[str, str]] = {}
    for item in load_todos():
        marker = item.marker
        if not marker:
            continue
        canonical = canonical_text(item.title)
        if not canonical:
            continue
        current[marker] = (canonical, text_hash(canonical))

    items: dict[str, Any] = cache['items']
    covered = all(
        marker in items and items[marker].get('text_hash') == hashed
        for marker, (_, hashed) in current.items()
    )
    if cache.get('db_fingerprint') == fingerprint and covered and len(items) == len(current):
        _cache_memo.clear()
        _cache_memo[memo_key] = cache
        return cache

    # Evict deleted markers.
    cache['items'] = {m: e for m, e in items.items() if m in current}
    items = cache['items']

    # Collect new or changed items and embed them in batches.
    to_embed = [
        (marker, canonical, hashed)
        for marker, (canonical, hashed) in current.items()
        if marker not in items or items[marker].get('text_hash') != hashed
    ]
    for start in range(0, len(to_embed), EMBED_BATCH_SIZE):
        chunk = to_embed[start:start + EMBED_BATCH_SIZE]
        vectors = embed_texts([canonical for _, canonical, _ in chunk])
        for (marker, _, hashed), vector in zip(chunk, vectors):
            if not cache['dim']:
                cache['dim'] = len(vector)
            elif len(vector) != cache['dim']:
                # Same model name but different dimension (re-tagged model):
                # vector space changed, start over next time.
                raise EmbeddingError(
                    f"embedding dimension changed ({cache['dim']} → {len(vector)})"
                )
            items[marker] = {'text_hash': hashed, 'vector': vector}

    cache['db_fingerprint'] = fingerprint
    try:
        _save_cache(cfg['cache_path'], cache)
    except OSError as exc:
        # Persisting is best-effort; serving from memory still works.
        logger.warning("could not persist embedding cache: %s", exc)
    _cache_memo.clear()
    _cache_memo[memo_key] = cache
    return cache


def similar_markers(
    cache: dict[str, Any],
    query_vec: list[float],
    top_k: int,
    threshold: float,
    allowed_markers: Optional[set[str]] = None,
) -> list[tuple[str, float]]:
    """Markers of the most similar cached items, descending by score."""
    scored = []
    for marker, entry in cache['items'].items():
        if allowed_markers is not None and marker not in allowed_markers:
            continue
        score = cosine_similarity(query_vec, entry['vector'])
        if score >= threshold:
            scored.append((marker, score))
    scored.sort(key=lambda pair: -pair[1])
    return scored[:top_k]


def query_similar(query: str, mode: str = 'tags') -> Optional[list[dict[str, Any]]]:
    """Find semantically similar todos for the given query text.

    Returns None when the feature is disabled or the backend is
    unavailable (callers fall back to substring behavior); otherwise a
    list of {marker, title, projects, contexts, done, score} dicts.
    """
    if not is_semantic_enabled():
        return None
    canonical = canonical_text(query)
    if not canonical:
        return []
    if mode == 'search':
        top_k, threshold = SEARCH_TOP_K, SEARCH_THRESHOLD
    else:
        top_k, threshold = TAG_SUGGEST_TOP_K, TAG_SUGGEST_THRESHOLD
    try:
        cache = ensure_index()
        query_vec = embed_texts([canonical])[0]
    except EmbeddingError as exc:
        logger.warning("semantic suggestions unavailable: %s", exc)
        return None
    by_marker = {item.marker: item for item in load_todos() if item.marker}
    results = []
    for marker, score in similar_markers(cache, query_vec, top_k, threshold):
        item = by_marker.get(marker)
        if item is None:
            continue
        # Don't suggest the literally identical title back.
        if canonical_text(item.title) == canonical:
            continue
        results.append({
            'marker': marker,
            'title': item.title,
            'projects': item.projects,
            'contexts': item.contexts,
            'done': item.done,
            'score': round(score, 4),
        })
    return results


def find_duplicate(title: str) -> tuple[bool, Optional[dict[str, Any]]]:
    """Check whether a very similar OPEN task already exists.

    Returns (available, match): available is False when the feature is
    disabled or the backend is unavailable; match is the best open todo
    above DUPLICATE_THRESHOLD or None.
    """
    if not is_semantic_enabled():
        return False, None
    canonical = canonical_text(title)
    if not canonical:
        return True, None
    try:
        cache = ensure_index()
        query_vec = embed_texts([canonical])[0]
    except EmbeddingError as exc:
        logger.warning("duplicate check unavailable: %s", exc)
        return False, None
    open_items = {item.marker: item for item in load_todos() if item.marker and not item.done}
    hits = similar_markers(
        cache, query_vec, top_k=1, threshold=DUPLICATE_THRESHOLD,
        allowed_markers=set(open_items.keys()),
    )
    if not hits:
        return True, None
    marker, score = hits[0]
    item = open_items[marker]
    return True, {'marker': marker, 'title': item.title, 'score': round(score, 4)}
