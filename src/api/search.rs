//! Search API — weighted / hybrid / threshold search.
//! Replaces RAGFlow's search_api.py + dify_retrieval_api.py.

use crate::rerank::{apply_hybrid_rerank, rerank_window};
use crate::search::{aggregate_documents, highlight_content};
use crate::server::{
    AppState, AuthContext, all_kbs_accessible, kb_accessible, kb_embedder_for, kb_reranker_for,
    validate_kb_embedding_bindings,
};
use axum::{
    Json,
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::{collections::HashMap, sync::Arc};

#[derive(Deserialize)]
pub struct SearchRequest {
    pub question: String,
    pub kb_ids: Option<Vec<String>>,
    pub doc_ids: Option<Vec<String>>,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    #[serde(default)]
    pub similarity_threshold: Option<f32>,
    #[serde(default)]
    pub vector_similarity_weight: Option<f32>,
    #[serde(default)]
    pub rerank: bool,
    #[serde(default)]
    pub rerank_id: Option<String>,
    #[serde(default = "default_page")]
    pub page: usize,
    #[serde(default)]
    pub highlight: bool,
    #[serde(default = "default_true")]
    pub aggs: bool,
    pub rank_feature: Option<HashMap<String, f32>>,
    #[serde(default)]
    pub meta_data_filter: Option<serde_json::Value>,
    #[serde(default)]
    pub reference_metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub include_metadata: Option<bool>,
    #[serde(default)]
    pub metadata_fields: Option<Vec<String>>,
    /// RAGFlow `cross_languages` — translate the question into these languages
    /// (locale codes, e.g. "en", "zh") via the tenant chat model and merge the
    /// extra-language results into the answer set.
    #[serde(default)]
    pub cross_languages: Vec<String>,
    /// RAGFlow `use_kg` — include knowledge-graph context in the response.
    #[serde(default)]
    pub use_kg: bool,
    /// RAGFlow `keyword` — keyword-only search (vector weight forced to 0).
    #[serde(default)]
    pub keyword: bool,
    /// RAGFlow `size` — page size; 0 falls back to `top_k`.
    #[serde(default)]
    pub size: usize,
}

fn default_top_k() -> usize {
    10
}

fn default_page() -> usize {
    1
}

fn default_true() -> bool {
    true
}

/// POST /api/v1/search — weighted vector search with threshold
pub async fn weighted_search(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<SearchRequest>,
) -> Response {
    let kb_ids = body.kb_ids.as_deref().unwrap_or_default();
    if !all_kbs_accessible(&state, kb_ids, &auth) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 400,
                "message": "At least one accessible kb_id is required",
                "data": { "chunks": [] }
            })),
        )
            .into_response();
    }
    if let Err(error) = validate_kb_embedding_bindings(&state, kb_ids) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 400,
                "message": error.to_string(),
                "data": { "chunks": [] }
            })),
        )
            .into_response();
    }
    let threshold = body.similarity_threshold.unwrap_or(0.2).clamp(0.0, 1.0);
    let mut vector_weight = body.vector_similarity_weight.unwrap_or(0.3).clamp(0.0, 1.0);
    // RAGFlow keyword toggle: pure keyword retrieval.
    if body.keyword {
        vector_weight = 0.0;
    }

    let embedding: Option<Vec<f32>> = if vector_weight > 0.0 {
        match kb_embedder_for(&state, kb_ids) {
            Ok(embedder) => match embedder.embed(&[&body.question]).await {
                Ok(embeddings) => embeddings.into_iter().next(),
                Err(_) => None,
            },
            Err(error) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "code": 400,
                        "message": error.to_string(),
                        "data": { "chunks": [] }
                    })),
                )
                    .into_response();
            }
        }
    } else {
        None
    };

    if vector_weight > 0.0 && embedding.is_none() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "code": 500,
                "message": "Embedding failed",
                "data": { "chunks": [] }
            })),
        )
            .into_response();
    }

    let page = body.page.max(1);
    let global_offset = (page - 1).saturating_mul(body.top_k);
    let rerank_id = body
        .rerank_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let rerank = body.rerank || rerank_id.is_some();
    let window = rerank_window(body.top_k, rerank.then_some(1024));
    let block_start = global_offset / window * window;
    let request_payload = serde_json::json!({
        "reference_metadata": body.reference_metadata,
        "include_metadata": body.include_metadata,
        "metadata_fields": body.metadata_fields,
    });
    let (include_metadata, metadata_fields) =
        crate::api::document_metadata::reference_metadata_selection(&request_payload);
    let filtered_doc_ids = match crate::api::document_metadata::resolve_metadata_doc_ids(
        &state,
        kb_ids,
        body.doc_ids.as_deref(),
        body.meta_data_filter.as_ref(),
    ) {
        Ok(doc_ids) => doc_ids,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "code": 400,
                    "message": error.to_string(),
                    "data": { "chunks": [] }
                })),
            )
                .into_response();
        }
    };
    let all_results =
        state
            .engine
            .read()
            .unwrap()
            .hybrid_search_kbs(crate::search::HybridSearchQuery {
                query: &body.question,
                query_embedding: embedding.as_deref(),
                top_k: usize::MAX,
                kb_ids,
                vector_weight,
                doc_ids: filtered_doc_ids.as_deref(),
                rank_feature: body.rank_feature.as_ref(),
            });
    let post_threshold = if vector_weight > 0.0 { threshold } else { 0.0 };
    let aggregate_candidates: Vec<_> = all_results
        .iter()
        .filter(|result| result.score >= post_threshold)
        .cloned()
        .collect();
    let total = aggregate_candidates.len();
    let doc_aggs = if body.aggs {
        aggregate_documents(&aggregate_candidates)
    } else {
        Vec::new()
    };
    let mut results: Vec<_> = all_results
        .into_iter()
        .skip(block_start)
        .take(window)
        .collect();
    if rerank && !results.is_empty() {
        let reranker = match kb_reranker_for(&state, kb_ids, rerank_id) {
            Ok(Some(reranker)) => reranker,
            Ok(None) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "code": 503,
                        "message": "Reranker is not configured",
                        "data": { "chunks": [] }
                    })),
                )
                    .into_response();
            }
            Err(error) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "code": 400,
                        "message": error.to_string(),
                        "data": { "chunks": [] }
                    })),
                )
                    .into_response();
            }
        };
        let documents: Vec<String> = results
            .iter()
            .map(|result| result.chunk.content.clone())
            .collect();
        let model_scores = match reranker
            .rerank(&body.question, &documents, results.len())
            .await
        {
            Ok(scores) => scores,
            Err(error) => {
                tracing::warn!(%error, "Reranker request failed");
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({
                        "code": 502,
                        "message": "Reranker request failed",
                        "data": { "chunks": [] }
                    })),
                )
                    .into_response();
            }
        };
        results = match apply_hybrid_rerank(results, &model_scores, vector_weight) {
            Ok(results) => results,
            Err(error) => {
                tracing::warn!(%error, "Invalid reranker response");
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({
                        "code": 502,
                        "message": "Invalid reranker response",
                        "data": { "chunks": [] }
                    })),
                )
                    .into_response();
            }
        };
    }
    results.retain(|result| result.score >= post_threshold);

    // RAGFlow cross_languages: translate the question via the tenant chat model
    // and merge extra-language results (deduped by chunk id). Translation
    // failure degrades gracefully to the original-language result set.
    if !body.cross_languages.is_empty()
        && let Some(llm) = state.llm.clone()
    {
        let langs = body.cross_languages.join(", ");
        let mut vars: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        vars.insert("query", &body.question);
        vars.insert("languages", langs.as_str());
        let sys = crate::prompts::PromptLibrary::cross_languages_sys()
            .render(&std::collections::HashMap::new());
        let user = crate::prompts::PromptLibrary::cross_languages_user().render(&vars);
        let translated = match llm
            .chat_completion(&[
                crate::llm::ChatMessage::new("system", &sys),
                crate::llm::ChatMessage::new("user", &user),
            ])
            .await
        {
            Ok(completion) => completion.content,
            Err(error) => {
                tracing::warn!(%error, "cross_languages translation failed; original results returned");
                String::new()
            }
        };
        let mut seen: std::collections::HashSet<String> = results
            .iter()
            .map(|result| result.chunk.id.clone())
            .collect();
        for translation in translated
            .split("###")
            .map(str::trim)
            .filter(|text| !text.is_empty() && *text != body.question)
        {
            let embedding = match kb_embedder_for(&state, kb_ids) {
                Ok(embedder) => match embedder.embed(&[translation]).await {
                    Ok(mut embeddings) => embeddings.pop(),
                    Err(_) => None,
                },
                Err(_) => None,
            };
            let extra =
                state
                    .engine
                    .read()
                    .unwrap()
                    .hybrid_search_kbs(crate::search::HybridSearchQuery {
                        query: translation,
                        query_embedding: embedding.as_deref(),
                        top_k: usize::MAX,
                        kb_ids,
                        vector_weight,
                        doc_ids: filtered_doc_ids.as_deref(),
                        rank_feature: body.rank_feature.as_ref(),
                    });
            for candidate in extra.into_iter().filter(|c| c.score >= post_threshold) {
                if seen.insert(candidate.chunk.id.clone()) {
                    results.push(candidate);
                }
            }
        }
    }

    let begin = global_offset % window;

    let weighted: Vec<serde_json::Value> = results
        .into_iter()
        .skip(begin)
        .take(if body.size > 0 { body.size } else { body.top_k })
        .map(|r| {
            let doc_id = r
                .chunk
                .metadata
                .get("doc_id")
                .cloned()
                .unwrap_or_default();
            let kb_id = r
                .chunk
                .metadata
                .get("kb_id")
                .cloned()
                .unwrap_or_default();
            let document_metadata = include_metadata.then(|| {
                crate::api::document_metadata::enrich_metadata(
                    &state,
                    &kb_id,
                    &doc_id,
                    metadata_fields.as_ref(),
                )
            });
            serde_json::json!({
                "chunk_id": r.chunk.id,
                "content": r.chunk.content,
                "doc_name": r.chunk.doc_name,
                "similarity": r.score,
                "weighted_score": r.score,
                "vector_similarity": r.vector_score,
                "term_similarity": r.term_score,
                "doc_id": doc_id,
                "kb_id": kb_id,
                "highlight": body.highlight.then(|| highlight_content(&r.chunk.content, &body.question)),
                "document_metadata": document_metadata.flatten(),
            })
        })
        .collect();
    let graph_context = if body.use_kg {
        state.graphs.context_for_query(kb_ids, &body.question)
    } else {
        Vec::new()
    };

    Json(serde_json::json!({"code":0,"data":{"total":total,"chunks":weighted,"doc_aggs":doc_aggs,"graph_context":graph_context}}))
        .into_response()
}

// ── Dify Protocol ──────────────────────────────────────────────

#[derive(Deserialize)]
pub struct DifyRetrievalRequest {
    pub knowledge_id: String,
    pub query: String,
    pub retrieval_model: Option<DifyRetrievalModel>,
    pub doc_ids: Option<Vec<String>>,
    pub rank_feature: Option<HashMap<String, f32>>,
    #[serde(default)]
    pub meta_data_filter: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct DifyRetrievalModel {
    pub search_method: Option<String>,
    pub top_k: Option<usize>,
    pub score_threshold: Option<f32>,
    pub vector_similarity_weight: Option<f32>,
    pub rerank: Option<bool>,
    pub rerank_id: Option<String>,
}

/// POST /api/v1/dify/retrieval — Dify-compatible retrieval endpoint
pub async fn dify_retrieval(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<DifyRetrievalRequest>,
) -> Response {
    if !kb_accessible(&state, &body.knowledge_id, &auth) {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Knowledge base not found" })),
        )
            .into_response();
    }
    if let Err(error) =
        validate_kb_embedding_bindings(&state, std::slice::from_ref(&body.knowledge_id))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
        )
            .into_response();
    }
    let top_k = body
        .retrieval_model
        .as_ref()
        .and_then(|m| m.top_k)
        .unwrap_or(5);
    let threshold = body
        .retrieval_model
        .as_ref()
        .and_then(|m| m.score_threshold)
        .unwrap_or(0.0);
    let search_method = body
        .retrieval_model
        .as_ref()
        .and_then(|model| model.search_method.as_deref())
        .unwrap_or("hybrid_search");
    let vector_weight = match search_method {
        "full_text_search" | "keyword_search" => 0.0,
        "semantic_search" => 1.0,
        _ => body
            .retrieval_model
            .as_ref()
            .and_then(|model| model.vector_similarity_weight)
            .unwrap_or(0.3)
            .clamp(0.0, 1.0),
    };
    let rerank_id = body
        .retrieval_model
        .as_ref()
        .and_then(|model| model.rerank_id.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let rerank = body
        .retrieval_model
        .as_ref()
        .and_then(|model| model.rerank)
        .unwrap_or(false)
        || rerank_id.is_some();

    let embedding: Option<Vec<f32>> = if vector_weight > 0.0 {
        match kb_embedder_for(&state, std::slice::from_ref(&body.knowledge_id)) {
            Ok(embedder) => match embedder.embed(&[&body.query]).await {
                Ok(embeddings) => embeddings.into_iter().next(),
                Err(_) => None,
            },
            Err(error) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
                )
                    .into_response();
            }
        }
    } else {
        None
    };

    if vector_weight > 0.0 && embedding.is_none() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "code": 500, "message": "Embedding failed" })),
        )
            .into_response();
    }

    let candidate_limit = if rerank {
        rerank_window(top_k, Some(1024))
    } else {
        top_k
    };
    let filtered_doc_ids = match crate::api::document_metadata::resolve_metadata_doc_ids(
        &state,
        std::slice::from_ref(&body.knowledge_id),
        body.doc_ids.as_deref(),
        body.meta_data_filter.as_ref(),
    ) {
        Ok(doc_ids) => doc_ids,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
            )
                .into_response();
        }
    };
    let mut results = state
        .engine
        .read()
        .unwrap()
        .hybrid_search_kbs(crate::search::HybridSearchQuery {
            query: &body.query,
            query_embedding: embedding.as_deref(),
            top_k: candidate_limit,
            kb_ids: std::slice::from_ref(&body.knowledge_id),
            vector_weight,
            doc_ids: filtered_doc_ids.as_deref(),
            rank_feature: body.rank_feature.as_ref(),
        })
        .into_iter()
        .collect::<Vec<_>>();
    if !results.is_empty() && rerank {
        let reranker =
            match kb_reranker_for(&state, std::slice::from_ref(&body.knowledge_id), rerank_id) {
                Ok(Some(reranker)) => reranker,
                Ok(None) => {
                    return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(
                        serde_json::json!({ "code": 503, "message": "Reranker is not configured" }),
                    ),
                )
                    .into_response();
                }
                Err(error) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
                    )
                        .into_response();
                }
            };
        let documents: Vec<String> = results
            .iter()
            .map(|result| result.chunk.content.clone())
            .collect();
        let model_scores = match reranker
            .rerank(&body.query, &documents, results.len())
            .await
        {
            Ok(scores) => scores,
            Err(error) => {
                tracing::warn!(%error, "Reranker request failed");
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "code": 502, "message": "Reranker request failed" })),
                )
                    .into_response();
            }
        };
        results = match apply_hybrid_rerank(results, &model_scores, vector_weight) {
            Ok(results) => results,
            Err(error) => {
                tracing::warn!(%error, "Invalid reranker response");
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(
                        serde_json::json!({ "code": 502, "message": "Invalid reranker response" }),
                    ),
                )
                    .into_response();
            }
        };
    }
    let records: Vec<serde_json::Value> = results
        .into_iter()
        .filter(|result| vector_weight == 0.0 || threshold == 0.0 || result.score >= threshold)
        .take(top_k)
        .map(|result| {
            serde_json::json!({
                "content": result.chunk.content,
                "score": result.score,
                "title": result.chunk.doc_name,
                "metadata": {
                    "source": result.chunk.id,
                    "vector_similarity": result.vector_score,
                    "term_similarity": result.term_score,
                },
            })
        })
        .collect();

    Json(serde_json::json!({
        "records": records,
        "query": {"content": body.query}
    }))
    .into_response()
}

#[cfg(test)]
mod search_request_schema_tests {
    use super::*;

    #[test]
    fn defaults_match_ragflow_single_dataset_request() {
        let request: SearchRequest =
            serde_json::from_value(serde_json::json!({"question": "q"})).unwrap();
        assert_eq!(request.top_k, 10);
        assert_eq!(request.page, 1);
        assert!(request.cross_languages.is_empty());
        assert!(!request.use_kg);
        assert!(!request.keyword);
        assert_eq!(request.size, 0);
        assert!(request.similarity_threshold.is_none());
        assert!(request.vector_similarity_weight.is_none());
    }

    #[test]
    fn new_fields_parse_and_roundtrip() {
        let value = serde_json::json!({
            "question": "水质",
            "cross_languages": ["en", "ja"],
            "use_kg": true,
            "keyword": true,
            "size": 30,
        });
        let request: SearchRequest = serde_json::from_value(value).unwrap();
        assert_eq!(request.cross_languages, vec!["en", "ja"]);
        assert!(request.use_kg);
        assert!(request.keyword);
        assert_eq!(request.size, 30);
    }
}
