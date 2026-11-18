//! Shared URL helpers — RAGFlow v0.27.2 `rag/utils/url_utils.py`.
//!
//! `ensure_v1` appends `/v1` unless the path already carries a versioned
//! segment (`v` followed by a digit, e.g. `v1`, `v2beta`, `v1alpha1`);
//! `append_api_path` appends one endpoint path exactly once. Upstream uses
//! these while building model-provider base URLs
//! (`rag/llm/embedding_model.py`, `rag/llm/rerank_model.py`).
//!
//! Bounded divergence: Python's `urlparse` accepts anything, while RayRAG
//! returns the input unchanged when the URL cannot be parsed.

use reqwest::Url;

/// True when a path segment starts with `v` + digit (upstream regex `^v\d+`).
fn is_versioned_segment(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    bytes.len() >= 2 && bytes[0] == b'v' && bytes[1].is_ascii_digit()
}

/// Ensure the URL ends with a versioned path segment like `/v1`.
pub fn ensure_v1(url: &str) -> String {
    if url.is_empty() {
        return url.to_owned();
    }
    let Ok(mut parsed) = Url::parse(url) else {
        return url.to_owned();
    };
    let path = parsed.path().trim_end_matches('/').to_owned();
    if path.split('/').any(is_versioned_segment) {
        return url.to_owned();
    }
    let new_path = if path.is_empty() {
        "/v1".to_owned()
    } else {
        format!("{path}/v1")
    };
    parsed.set_path(&new_path);
    parsed.to_string()
}

/// Append an API endpoint path exactly once while preserving the base path.
pub fn append_api_path(url: &str, endpoint: &str) -> String {
    if url.is_empty() {
        return url.to_owned();
    }
    let Ok(mut parsed) = Url::parse(url) else {
        return url.to_owned();
    };
    let path = parsed.path().trim_end_matches('/').to_owned();
    let endpoint_path = format!("/{}", endpoint.trim_matches('/'));
    let new_path = if path.ends_with(&endpoint_path) {
        path
    } else {
        format!("{path}{endpoint_path}")
    };
    parsed.set_path(&new_path);
    parsed.to_string()
}


