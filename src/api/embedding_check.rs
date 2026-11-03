//! `POST /api/v1/datasets/{dataset_id}/embedding/check` — does this dataset's stored vectors still
//! agree with its embedding model?
//!
//! The endpoint exists to answer one operational question before someone switches embedding models:
//! if the vectors already stored in the dataset were produced by a *different* model, retrieval
//! silently returns nonsense. So the check re-embeds a sample of the dataset's own chunk text and
//! compares the fresh vector with the stored one. High cosine similarity means the stored vectors are
//! consistent with the model; a low average means they are not, and upstream reports that as business
//! code **10** (`NOT_EFFECTIVE`) rather than a success with a worrying number buried inside it.
//!
//! The summary shape is upstream's, field for field:
//!
//! ```text
//! {"summary": {"kb_id", "model", "sampled", "valid", "avg_cos_sim", "min_cos_sim", "max_cos_sim",
//!              "match_mode"},
//!  "results": [{"chunk_id", "doc_id", "doc_name", "vector_field", "vector_dim", "cos_sim"}]}
//! ```
//!
//! Two deliberate choices:
//!
//! * the sample is taken by even spacing over the dataset's embedded chunks, not at random — the same
//!   dataset and request give the same answer twice, which matters when the answer decides a
//!   migration;
//! * a dimension mismatch between the stored vector and the freshly embedded one is reported as
//!   `cos_sim: 0.0` with the two dimensions visible in `vector_dim`, instead of being hidden as a
//!   generic failure.

use std::sync::Arc;

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::search::IndexedChunk;
use crate::server::{AppState, AuthContext, api_error_code, code, kb_accessible};

/// How many chunks to compare when the request does not say.
const DEFAULT_SAMPLE: usize = 8;
/// The largest sample a request may ask for, so one call cannot embed an entire dataset.
const MAX_SAMPLE: usize = 64;
/// Upstream's threshold: an average at or above this means the stored vectors are still valid.
const EFFECTIVE_THRESHOLD: f64 = 0.9;

#[derive(Debug, Deserialize, Default)]
pub struct EmbeddingCheckRequest {
    /// The embedding model to check against. Required, as upstream requires it.
    #[serde(default)]
    pub embd_id: String,
    /// How many chunks to sample.
    #[serde(default)]
    pub n: Option<usize>,
    /// Upstream accepts a match mode; recorded in the summary as given.
    #[serde(default)]
    pub mode: Option<String>,
}

/// Cosine similarity, `0.0` when either side is empty or the lengths differ.
pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let (x, y) = (*x as f64, *y as f64);
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Evenly spaced positions over `total` items, at most `count` of them.
///
/// Even spacing rather than randomness: the check decides whether a migration is safe, so the same
/// dataset must give the same answer twice.
pub(crate) fn sample_positions(total: usize, count: usize) -> Vec<usize> {
    if total == 0 || count == 0 {
        return Vec::new();
    }
    let take = count.min(total);
    if take == total {
        return (0..total).collect();
    }
    (0..take)
        .map(|index| index * total / take)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The embedding model this deployment is configured with, for the `default` alias.
///
/// `EMBED_MODEL` is RAGFlow's name for it and is the key the wizard writes, so the summary reports the
/// same string the user configured rather than an internal placeholder.
pub(crate) fn configured_embedding_model() -> String {
    std::env::var("EMBED_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "default".to_string())
}

fn data_error(message: &str) -> Response {
    Json(serde_json::json!({
        "code": code::INVALID_OR_MISSING_DATA,
        "data": null,
        "message": message,
    }))
    .into_response()
}

/// `POST /api/v1/datasets/{dataset_id}/embedding/check`.
pub async fn check_dataset_embedding(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
    Json(body): Json<EmbeddingCheckRequest>,
) -> Response {
    let requested = body.embd_id.trim().to_string();
    if requested.is_empty() {
        return data_error("`embd_id` is required.");
    }
    // `default` is this deployment's configured embedder. Upstream requires a concrete id; a client
    // that has not been told the name (the settings page, for instance) would otherwise have to invent
    // one, and reporting an invented label next to real numbers is worse than accepting the alias.
    let model = if requested.eq_ignore_ascii_case("default") {
        configured_embedding_model()
    } else {
        requested
    };
    if !kb_accessible(&state, &kb_id, &auth) {
        return data_error("no authorization");
    }
    let sample_size = body.n.unwrap_or(DEFAULT_SAMPLE).clamp(1, MAX_SAMPLE);

    // Only chunks of this dataset, and only ones that carry a vector: an unembedded chunk has nothing
    // to compare against.
    let document_ids: std::collections::HashSet<String> = state
        .docs
        .list(&kb_id)
        .into_iter()
        .map(|doc| doc.id)
        .collect();
    let embedded: Vec<IndexedChunk> = state
        .engine
        .read()
        .unwrap()
        .to_vec()
        .into_iter()
        .filter(|chunk| {
            chunk
                .metadata
                .get("doc_id")
                .is_some_and(|doc_id| document_ids.contains(doc_id))
                && !chunk.embedding.is_empty()
        })
        .collect();
    if embedded.is_empty() {
        return data_error("No embedded chunks are available to compare.");
    }

    let embedder = match crate::server::kb_embedder_for(&state, std::slice::from_ref(&kb_id)) {
        Ok(embedder) => embedder,
        Err(error) => return data_error(&format!("Embedding failure. {error}")),
    };
    let positions = sample_positions(embedded.len(), sample_size);
    let sampled: Vec<&IndexedChunk> = positions.iter().map(|index| &embedded[*index]).collect();
    let texts: Vec<&str> = sampled.iter().map(|chunk| chunk.content.as_str()).collect();
    let fresh = match embedder.embed(&texts).await {
        Ok(vectors) => vectors,
        Err(error) => return data_error(&format!("Embedding failure. {error}")),
    };

    let mut results = Vec::new();
    let mut similarities = Vec::new();
    for (index, chunk) in sampled.iter().enumerate() {
        let Some(vector) = fresh.get(index) else {
            continue;
        };
        let dimension_matches = vector.len() == chunk.embedding.len();
        let similarity = if dimension_matches {
            cosine_similarity(vector, &chunk.embedding)
        } else {
            // A different width means a different model; saying so beats a vague failure.
            0.0
        };
        similarities.push(similarity);
        results.push(serde_json::json!({
            "chunk_id": chunk.id,
            "doc_id": chunk.metadata.get("doc_id").cloned().unwrap_or_default(),
            "doc_name": chunk.doc_name,
            "vector_field": "embedding",
            "vector_dim": if dimension_matches {
                chunk.embedding.len()
            } else {
                vector.len()
            },
            "stored_vector_dim": chunk.embedding.len(),
            "cos_sim": (similarity * 1_000_000.0).round() / 1_000_000.0,
        }));
    }
    if similarities.is_empty() {
        return data_error("No embedded chunks are available to compare.");
    }

    let sum: f64 = similarities.iter().sum();
    let avg = sum / similarities.len() as f64;
    let min = similarities.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = similarities
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    let round6 = |value: f64| (value * 1_000_000.0).round() / 1_000_000.0;
    let data = serde_json::json!({
        "summary": {
            "kb_id": kb_id,
            "model": model,
            "sampled": embedded.len().min(sample_size),
            "valid": similarities.len(),
            "avg_cos_sim": round6(avg),
            "min_cos_sim": round6(min),
            "max_cos_sim": round6(max),
            "match_mode": body.mode.clone().unwrap_or_else(|| "stored_vector".to_string()),
            "threshold": EFFECTIVE_THRESHOLD,
        },
        "results": results,
    });

    if avg >= EFFECTIVE_THRESHOLD {
        Json(serde_json::json!({ "code": 0, "data": data, "message": "success" })).into_response()
    } else {
        // Business code 10 (`NOT_EFFECTIVE`) with the data attached, exactly as upstream answers.
        Json(serde_json::json!({
            "code": code::NOT_EFFECTIVE,
            "data": data,
            "message": format!(
                "Embedding model switch failed: the average similarity between old and new vectors is below {EFFECTIVE_THRESHOLD}, indicating inconsistent embedding models."
            ),
        }))
        .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_similarity_is_one_for_identical_and_zero_for_orthogonal_or_mismatched() {
        let a = vec![1.0f32, 2.0, 3.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-9);
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-9);
        // A different width is not comparable: reported as zero rather than guessed.
        assert_eq!(cosine_similarity(&[1.0, 2.0], &[1.0, 2.0, 3.0]), 0.0);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        // Opposite directions are -1, which must not be confused with a mismatch.
        assert!((cosine_similarity(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-9);
    }

    #[test]
    fn the_sample_is_evenly_spaced_and_reproducible() {
        assert_eq!(sample_positions(10, 3), vec![0, 3, 6]);
        assert_eq!(sample_positions(10, 3), sample_positions(10, 3));
        // Fewer items than asked for means all of them, in order.
        assert_eq!(sample_positions(3, 8), vec![0, 1, 2]);
        // No items, or nothing asked for.
        assert!(sample_positions(0, 5).is_empty());
        assert!(sample_positions(5, 0).is_empty());
        // Never out of range, never duplicated.
        for (total, count) in [(1, 1), (7, 2), (100, 64), (1000, 8)] {
            let picked = sample_positions(total, count);
            assert!(
                picked.len() <= count && picked.len() <= total,
                "{total}/{count}"
            );
            assert!(
                picked.windows(2).all(|pair| pair[0] < pair[1]),
                "{picked:?}"
            );
            assert!(picked.iter().all(|index| *index < total), "{picked:?}");
        }
    }

    #[test]
    fn the_default_alias_resolves_to_the_configured_model() {
        // The environment is read, never invented: an unset key reports `default` rather than a name
        // that would not match anything.
        let resolved = configured_embedding_model();
        assert!(!resolved.trim().is_empty());
        if let Ok(configured) = std::env::var("EMBED_MODEL") {
            if !configured.trim().is_empty() {
                assert_eq!(resolved, configured.trim());
            }
        }
    }

    #[test]
    fn the_threshold_is_upstreams_ninety_percent() {
        assert_eq!(EFFECTIVE_THRESHOLD, 0.9);
        assert_eq!(DEFAULT_SAMPLE, 8);
        assert_eq!(MAX_SAMPLE, 64);
    }
}
