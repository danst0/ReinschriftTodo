//! Semantic embeddings via Ollama's `/api/embed` endpoint.
//!
//! Provides text canonicalization, cosine similarity and a JSON sidecar
//! cache so that GUI and webapp can offer semantic suggestions, semantic
//! search and duplicate detection. The cache lives in the user cache
//! directory (never next to the database — WebDAV has no local "next to")
//! and is invalidated via the storage fingerprint plus per-item text hashes.
//!
//! The webapp (`webapp/app/services/embedding_service.py`) is a deliberate
//! parallel port of this module — keep canonicalization and thresholds in sync.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::types::TodoItem;

/// Default Ollama embedding model (multilingual, 1024 dimensions).
pub const DEFAULT_EMBEDDING_MODEL: &str = "bge-m3";

/// Number of texts sent per `/api/embed` request when (re)building the index.
pub const EMBED_BATCH_SIZE: usize = 64;

/// Thresholds and result limits per feature (cosine similarity).
/// Keep in sync with the webapp's `embedding_service.py`.
pub const TAG_SUGGEST_THRESHOLD: f32 = 0.55;
pub const TAG_SUGGEST_TOP_K: usize = 5;
pub const SEARCH_THRESHOLD: f32 = 0.50;
pub const SEARCH_TOP_K: usize = 10;
pub const DUPLICATE_THRESHOLD: f32 = 0.85;

const SCHEMA_VERSION: u32 = 1;

/// Connection settings for the Ollama embed endpoint.
#[derive(Clone, Debug)]
pub struct EmbeddingConfig {
    pub ollama_url: String,
    pub model: String,
    pub timeout_secs: u64,
}

/// Anything that can turn texts into embedding vectors.
/// Allows tests to inject a fake implementation without network access.
pub trait Embedder {
    fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>>;
}

/// Embedder backed by Ollama's `/api/embed` endpoint (blocking reqwest).
pub struct OllamaEmbedder {
    pub config: EmbeddingConfig,
}

#[derive(Debug, Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

impl Embedder for OllamaEmbedder {
    fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!(
            "{}/api/embed",
            self.config.ollama_url.trim_end_matches('/')
        );
        let payload = serde_json::json!({
            "model": self.config.model,
            "input": inputs,
        });
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(self.config.timeout_secs))
            .build()
            .context("failed to build HTTP client")?;
        let response = client
            .post(&url)
            .json(&payload)
            .send()
            .with_context(|| format!("embed request to {} failed", url))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            bail!(
                "embedding model '{}' not available (404) — run `ollama pull {}`",
                self.config.model,
                self.config.model
            );
        }
        if !response.status().is_success() {
            bail!("embed request failed with status {}", response.status());
        }
        let parsed: EmbedResponse = response
            .json()
            .context("failed to parse embed response")?;
        if parsed.embeddings.len() != inputs.len() {
            bail!(
                "embed response count mismatch: {} inputs, {} vectors",
                inputs.len(),
                parsed.embeddings.len()
            );
        }
        Ok(parsed.embeddings)
    }
}

/// Fetch embeddings for the given texts from Ollama.
pub fn embed_texts(cfg: &EmbeddingConfig, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
    OllamaEmbedder {
        config: cfg.clone(),
    }
    .embed(inputs)
}

/// Canonical form of a title for embedding: trimmed, whitespace collapsed.
/// Must match the webapp's `canonical_text` exactly.
pub fn canonical_text(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// FNV-1a 64-bit hash over the canonical text (stable, dependency-free).
pub fn text_hash(canonical: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for byte in canonical.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Cosine similarity between two vectors (0.0 for mismatched/zero vectors).
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

/// A cached embedding for one todo item (keyed by marker).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedEmbedding {
    pub text_hash: u64,
    pub vector: Vec<f32>,
}

/// Sidecar cache of item embeddings, persisted as JSON.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmbeddingCache {
    pub schema_version: u32,
    pub model: String,
    pub dim: usize,
    pub db_fingerprint: String,
    pub items: HashMap<String, CachedEmbedding>,
}

impl EmbeddingCache {
    pub fn empty(model: &str) -> Self {
        EmbeddingCache {
            schema_version: SCHEMA_VERSION,
            model: model.to_string(),
            dim: 0,
            db_fingerprint: String::new(),
            items: HashMap::new(),
        }
    }

    /// Markers of the most similar cached items, descending by score.
    pub fn similar_markers(
        &self,
        query_vec: &[f32],
        top_k: usize,
        threshold: f32,
    ) -> Vec<(String, f32)> {
        let mut scored: Vec<(String, f32)> = self
            .items
            .iter()
            .map(|(marker, entry)| (marker.clone(), cosine_similarity(query_vec, &entry.vector)))
            .filter(|(_, score)| *score >= threshold)
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        scored
    }
}

/// Load the cache from disk; returns an empty cache on any error or
/// schema/model mismatch (callers then rebuild incrementally).
pub fn load_cache(path: &Path, model: &str) -> EmbeddingCache {
    let Ok(data) = fs::read_to_string(path) else {
        return EmbeddingCache::empty(model);
    };
    match serde_json::from_str::<EmbeddingCache>(&data) {
        Ok(cache) if cache.schema_version == SCHEMA_VERSION && cache.model == model => cache,
        // Different model or dim ⇒ incompatible vector space: full reset.
        _ => EmbeddingCache::empty(model),
    }
}

/// Persist the cache atomically (tmp file + rename).
pub fn save_cache(path: &Path, cache: &EmbeddingCache) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let serialized = serde_json::to_string(cache).context("failed to serialize cache")?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serialized)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Ensure the cache covers the given items: embed new/changed titles
/// (batched), evict deleted markers, update the fingerprint and persist.
/// Items without a marker or with an empty title are ignored.
pub fn ensure_index_with(
    path: &Path,
    embedder: &dyn Embedder,
    model: &str,
    items: &[TodoItem],
    fingerprint: &str,
) -> Result<EmbeddingCache> {
    let mut cache = load_cache(path, model);

    // Canonical view of the current items: marker → (canonical text, hash).
    let mut current: HashMap<String, (String, u64)> = HashMap::new();
    for item in items {
        let Some(marker) = item.key.marker.as_ref() else {
            continue;
        };
        let canonical = canonical_text(&item.title);
        if canonical.is_empty() {
            continue;
        }
        let hash = text_hash(&canonical);
        current.insert(marker.clone(), (canonical, hash));
    }

    // Fast path: fingerprint unchanged and every item covered with same hash.
    let covered = current.iter().all(|(marker, (_, hash))| {
        cache
            .items
            .get(marker)
            .map(|e| e.text_hash == *hash)
            .unwrap_or(false)
    });
    if cache.db_fingerprint == fingerprint && covered && cache.items.len() == current.len() {
        return Ok(cache);
    }

    // Evict deleted markers.
    cache.items.retain(|marker, _| current.contains_key(marker));

    // Collect new or changed items.
    let mut to_embed: Vec<(String, String, u64)> = Vec::new();
    for (marker, (canonical, hash)) in &current {
        let needs_embed = cache
            .items
            .get(marker)
            .map(|e| e.text_hash != *hash)
            .unwrap_or(true);
        if needs_embed {
            to_embed.push((marker.clone(), canonical.clone(), *hash));
        }
    }

    // Embed in batches.
    for chunk in to_embed.chunks(EMBED_BATCH_SIZE) {
        let inputs: Vec<String> = chunk.iter().map(|(_, text, _)| text.clone()).collect();
        let vectors = embedder.embed(&inputs)?;
        for ((marker, _, hash), vector) in chunk.iter().zip(vectors.into_iter()) {
            if cache.dim == 0 {
                cache.dim = vector.len();
            } else if vector.len() != cache.dim {
                // Same model name but different dimension (e.g. re-tagged
                // model): vector space changed, start over.
                bail!(
                    "embedding dimension changed ({} → {}), cache reset required",
                    cache.dim,
                    vector.len()
                );
            }
            cache.items.insert(
                marker.clone(),
                CachedEmbedding {
                    text_hash: *hash,
                    vector,
                },
            );
        }
    }

    cache.db_fingerprint = fingerprint.to_string();
    save_cache(path, &cache)?;
    Ok(cache)
}

/// Convenience wrapper using the Ollama embedder.
pub fn ensure_index(
    path: &Path,
    cfg: &EmbeddingConfig,
    items: &[TodoItem],
    fingerprint: &str,
) -> Result<EmbeddingCache> {
    let embedder = OllamaEmbedder {
        config: cfg.clone(),
    };
    ensure_index_with(path, &embedder, &cfg.model, items, fingerprint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TodoKey;
    use std::cell::RefCell;

    struct FakeEmbedder {
        calls: RefCell<Vec<Vec<String>>>,
    }

    impl FakeEmbedder {
        fn new() -> Self {
            FakeEmbedder {
                calls: RefCell::new(Vec::new()),
            }
        }

        fn embedded_count(&self) -> usize {
            self.calls.borrow().iter().map(|c| c.len()).sum()
        }
    }

    impl Embedder for FakeEmbedder {
        fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
            self.calls.borrow_mut().push(inputs.to_vec());
            // Deterministic 3-dim vector derived from the text hash.
            Ok(inputs
                .iter()
                .map(|text| {
                    let h = text_hash(text);
                    vec![
                        (h % 100) as f32 / 100.0,
                        ((h >> 8) % 100) as f32 / 100.0,
                        ((h >> 16) % 100) as f32 / 100.0,
                    ]
                })
                .collect())
        }
    }

    fn item(marker: &str, title: &str) -> TodoItem {
        TodoItem {
            key: TodoKey {
                line_index: 0,
                marker: Some(marker.to_string()),
            },
            title: title.to_string(),
            projects: vec![],
            contexts: vec![],
            due: None,
            myday: None,
            reference: None,
            recurrence: None,
            note: None,
            done: false,
        }
    }

    fn temp_cache_path(name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("reinschrift_embed_test_{}_{}.json", name, std::process::id()));
        let _ = fs::remove_file(&path);
        path
    }

    #[test]
    fn test_cosine_identical_and_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
        assert_eq!(cosine_similarity(&a, &b), cosine_similarity(&b, &a));
        assert_eq!(cosine_similarity(&a, &[1.0, 0.0]), 0.0); // length mismatch
        assert_eq!(cosine_similarity(&a, &[0.0, 0.0, 0.0]), 0.0); // zero vector
    }

    #[test]
    fn test_canonical_text() {
        assert_eq!(canonical_text("  Milch   kaufen \n"), "Milch kaufen");
        assert_eq!(canonical_text("Milch kaufen"), "Milch kaufen");
        assert_eq!(canonical_text("   "), "");
    }

    #[test]
    fn test_text_hash_stability() {
        assert_eq!(text_hash("Milch kaufen"), text_hash("Milch kaufen"));
        assert_ne!(text_hash("Milch kaufen"), text_hash("Brot kaufen"));
        assert_ne!(text_hash(""), text_hash(" "));
    }

    #[test]
    fn test_similar_markers_sorting_threshold_topk() {
        let mut cache = EmbeddingCache::empty("test");
        cache.items.insert(
            "A".into(),
            CachedEmbedding {
                text_hash: 1,
                vector: vec![1.0, 0.0],
            },
        );
        cache.items.insert(
            "B".into(),
            CachedEmbedding {
                text_hash: 2,
                vector: vec![0.9, 0.1],
            },
        );
        cache.items.insert(
            "C".into(),
            CachedEmbedding {
                text_hash: 3,
                vector: vec![0.0, 1.0],
            },
        );
        let query = vec![1.0, 0.0];
        let hits = cache.similar_markers(&query, 10, 0.5);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, "A");
        assert_eq!(hits[1].0, "B");
        assert!(hits[0].1 > hits[1].1);
        // top_k limit
        let hits = cache.similar_markers(&query, 1, 0.5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, "A");
    }

    #[test]
    fn test_cache_roundtrip() {
        let path = temp_cache_path("roundtrip");
        let mut cache = EmbeddingCache::empty("bge-m3");
        cache.dim = 3;
        cache.db_fingerprint = "fp1".into();
        cache.items.insert(
            "M1".into(),
            CachedEmbedding {
                text_hash: 42,
                vector: vec![0.1, 0.2, 0.3],
            },
        );
        save_cache(&path, &cache).unwrap();
        let loaded = load_cache(&path, "bge-m3");
        assert_eq!(loaded.items.len(), 1);
        assert_eq!(loaded.items["M1"].text_hash, 42);
        assert_eq!(loaded.db_fingerprint, "fp1");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_model_change_resets_cache() {
        let path = temp_cache_path("modelchange");
        let mut cache = EmbeddingCache::empty("bge-m3");
        cache.items.insert(
            "M1".into(),
            CachedEmbedding {
                text_hash: 42,
                vector: vec![0.1],
            },
        );
        save_cache(&path, &cache).unwrap();
        let loaded = load_cache(&path, "nomic-embed-text");
        assert!(loaded.items.is_empty());
        assert_eq!(loaded.model, "nomic-embed-text");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_ensure_index_incremental_diff_and_eviction() {
        let path = temp_cache_path("incremental");
        let embedder = FakeEmbedder::new();
        let items = vec![item("A", "Milch kaufen"), item("B", "Brot kaufen")];

        let cache = ensure_index_with(&path, &embedder, "test", &items, "fp1").unwrap();
        assert_eq!(cache.items.len(), 2);
        assert_eq!(embedder.embedded_count(), 2);

        // Unchanged fingerprint + items ⇒ no new embed calls.
        let cache = ensure_index_with(&path, &embedder, "test", &items, "fp1").unwrap();
        assert_eq!(cache.items.len(), 2);
        assert_eq!(embedder.embedded_count(), 2);

        // One title changed, one deleted, one added ⇒ only 2 new embeds.
        let items2 = vec![item("A", "Hafermilch kaufen"), item("C", "Wäsche waschen")];
        let cache = ensure_index_with(&path, &embedder, "test", &items2, "fp2").unwrap();
        assert_eq!(cache.items.len(), 2);
        assert!(cache.items.contains_key("A"));
        assert!(cache.items.contains_key("C"));
        assert!(!cache.items.contains_key("B")); // evicted
        assert_eq!(embedder.embedded_count(), 4);
        assert_eq!(cache.db_fingerprint, "fp2");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_ensure_index_skips_unmarked_and_empty() {
        let path = temp_cache_path("skips");
        let embedder = FakeEmbedder::new();
        let mut unmarked = item("X", "Ohne Marker");
        unmarked.key.marker = None;
        let items = vec![unmarked, item("E", "   ")];
        let cache = ensure_index_with(&path, &embedder, "test", &items, "fp1").unwrap();
        assert!(cache.items.is_empty());
        assert_eq!(embedder.embedded_count(), 0);
        let _ = fs::remove_file(&path);
    }
}
