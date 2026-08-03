//! Retrieval feedback that adjusts cited chunks' pagerank feature.
//!
//! This mirrors RAGFlow's chunk feedback service. The feature is disabled by
//! default and can be enabled with `CHUNK_FEEDBACK_ENABLED=true`.

use axum::{
    Json,
    extract::{Extension, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

use crate::server::{AppState, AuthContext, all_kbs_accessible};

const FEEDBACK_BUDGET: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackWeighting {
    Relevance,
    Uniform,
}

#[derive(Debug, Deserialize)]
pub struct MessageFeedbackRequest {
    pub thumbup: bool,
    #[serde(default)]
    pub feedback: Option<String>,
}

pub fn feedback_weighting_from_env() -> FeedbackWeighting {
    match std::env::var("CHUNK_FEEDBACK_WEIGHTING") {
        Ok(value) if value.eq_ignore_ascii_case("uniform") => FeedbackWeighting::Uniform,
        _ => FeedbackWeighting::Relevance,
    }
}

#[derive(Debug, Deserialize)]
pub struct ChunkFeedbackRequest {
    pub positive: bool,
    #[serde(default)]
    pub chunks: Vec<FeedbackChunk>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FeedbackChunk {
    #[serde(alias = "chunk_id")]
    pub id: String,
    #[serde(alias = "dataset_id")]
    pub kb_id: String,
    pub similarity: Option<f32>,
    pub vector_similarity: Option<f32>,
    pub term_similarity: Option<f32>,
}

impl FeedbackChunk {
    fn retrieval_signal(&self) -> f32 {
        [
            self.similarity,
            self.vector_similarity,
            self.term_similarity,
        ]
        .into_iter()
        .flatten()
        .filter(|score| score.is_finite() && *score > 0.0)
        .fold(0.0, f32::max)
    }
}

#[derive(Debug, Serialize)]
pub struct ChunkFeedbackResult {
    pub success_count: usize,
    pub fail_count: usize,
    pub chunk_ids: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
}

pub async fn apply_chunk_feedback(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<ChunkFeedbackRequest>,
) -> axum::response::Response {
    if !state.chunk_feedback_enabled {
        return Json(serde_json::json!({
            "code": 0,
            "data": ChunkFeedbackResult {
                success_count: 0,
                fail_count: 0,
                chunk_ids: Vec::new(),
                disabled: true,
            }
        }))
        .into_response();
    }

    let chunks = deduplicate_chunks(request.chunks);
    let kb_ids: Vec<String> = chunks
        .iter()
        .map(|chunk| chunk.kb_id.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if chunks.is_empty() || !all_kbs_accessible(&state, &kb_ids, &auth) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 400,
                "message": "At least one accessible chunk reference is required"
            })),
        )
            .into_response();
    }

    let signed_budget = if request.positive {
        FEEDBACK_BUDGET
    } else {
        -FEEDBACK_BUDGET
    };
    let deltas = allocate_deltas(&chunks, signed_budget, state.chunk_feedback_weighting);
    let chunk_ids = chunks.iter().map(|chunk| chunk.id.clone()).collect();
    let _commit_guard = state.document_commit_lock.lock().unwrap();
    let mut engine = state.engine.write().unwrap();
    let (success_count, fail_count) = match apply_deltas_and_save(
        &mut engine,
        &state.index_path,
        &state.vector_mirror,
        &chunks,
        &deltas,
    ) {
        Ok(counts) => counts,
        Err(error) => {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
            )
                .into_response();
        }
    };

    Json(serde_json::json!({
        "code": 0,
        "data": ChunkFeedbackResult {
            success_count,
            fail_count,
            chunk_ids,
            disabled: false,
        }
    }))
    .into_response()
}

pub async fn update_message_feedback(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path((_chat_id, session_id, message_id)): axum::extract::Path<(
        String,
        String,
        String,
    )>,
    Json(request): Json<MessageFeedbackRequest>,
) -> axum::response::Response {
    update_message_feedback_inner(state, auth, session_id, message_id, request)
}

pub async fn update_session_message_feedback(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path((session_id, message_id)): axum::extract::Path<(String, String)>,
    Json(request): Json<MessageFeedbackRequest>,
) -> axum::response::Response {
    update_message_feedback_inner(state, auth, session_id, message_id, request)
}

fn update_message_feedback_inner(
    state: Arc<AppState>,
    auth: AuthContext,
    session_id: String,
    message_id: String,
    request: MessageFeedbackRequest,
) -> axum::response::Response {
    let Some(target) = state
        .conversations
        .feedback_target(&session_id, &auth.user_id, &message_id)
    else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Message not found" })),
        )
            .into_response();
    };

    let chunks = deduplicate_chunks(
        target
            .references
            .iter()
            .cloned()
            .map(FeedbackChunk::from)
            .collect(),
    );
    let kb_ids: Vec<String> = chunks
        .iter()
        .map(|chunk| chunk.kb_id.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if !kb_ids.is_empty() && !all_kbs_accessible(&state, &kb_ids, &auth) {
        return (
            axum::http::StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "code": 403, "message": "Reference access denied" })),
        )
            .into_response();
    }

    let _commit_guard = state.document_commit_lock.lock().unwrap();
    let mut engine = state.engine.write().unwrap();
    let previous_index = engine.to_vec();
    let index_changed = state.chunk_feedback_enabled
        && target.prior_thumb != Some(request.thumbup)
        && !chunks.is_empty();
    let (success_count, fail_count) = if index_changed {
        match apply_feedback_transition(
            &mut engine,
            &state.index_path,
            &state.vector_mirror,
            &chunks,
            target.prior_thumb,
            request.thumbup,
            state.chunk_feedback_weighting,
        ) {
            Ok(counts) => counts,
            Err(error) => {
                return (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
                )
                    .into_response();
            }
        }
    } else {
        (0, 0)
    };

    let feedback = request
        .feedback
        .as_deref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    match state.conversations.update_message_feedback(
        &session_id,
        &auth.user_id,
        &message_id,
        target.prior_thumb,
        request.thumbup,
        feedback.clone(),
    ) {
        Ok(true) => Json(serde_json::json!({
            "code": 0,
            "data": {
                "session_id": session_id,
                "message_id": message_id,
                "thumbup": request.thumbup,
                "feedback": if request.thumbup { None::<String> } else { feedback },
                "success_count": success_count,
                "fail_count": fail_count,
                "disabled": !state.chunk_feedback_enabled,
            }
        }))
        .into_response(),
        Ok(false) => {
            if index_changed {
                rollback_index(
                    &mut engine,
                    &state.index_path,
                    &state.vector_mirror,
                    previous_index,
                );
            }
            (
                axum::http::StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "code": 404, "message": "Message not found" })),
            )
                .into_response()
        }
        Err(error) => {
            if index_changed {
                rollback_index(
                    &mut engine,
                    &state.index_path,
                    &state.vector_mirror,
                    previous_index,
                );
            }
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
            )
                .into_response()
        }
    }
}

pub fn feedback_enabled_from_env() -> bool {
    std::env::var("CHUNK_FEEDBACK_ENABLED")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

impl From<crate::llm::ChunkReference> for FeedbackChunk {
    fn from(reference: crate::llm::ChunkReference) -> Self {
        Self {
            id: reference.id,
            kb_id: reference.kb_id,
            similarity: reference.similarity,
            vector_similarity: reference.vector_similarity,
            term_similarity: reference.term_similarity,
        }
    }
}

fn deduplicate_chunks(chunks: Vec<FeedbackChunk>) -> Vec<FeedbackChunk> {
    let mut seen = HashSet::new();
    chunks
        .into_iter()
        .filter(|chunk| {
            !chunk.id.trim().is_empty()
                && !chunk.kb_id.trim().is_empty()
                && seen.insert((chunk.id.clone(), chunk.kb_id.clone()))
        })
        .collect()
}

fn allocate_deltas(
    chunks: &[FeedbackChunk],
    signed_budget: i32,
    mode: FeedbackWeighting,
) -> Vec<i32> {
    if chunks.is_empty() || signed_budget == 0 {
        return vec![0; chunks.len()];
    }
    if mode == FeedbackWeighting::Uniform {
        return vec![signed_budget.signum(); chunks.len()];
    }

    let magnitudes: Vec<f32> = chunks
        .iter()
        .map(|chunk| {
            let signal = chunk.retrieval_signal();
            if signal > 0.0 { signal } else { 1.0 }
        })
        .collect();
    split_integer_budget(&magnitudes, signed_budget)
}

fn split_integer_budget(magnitudes: &[f32], signed_budget: i32) -> Vec<i32> {
    let budget = signed_budget.unsigned_abs() as usize;
    if magnitudes.is_empty() || budget == 0 {
        return vec![0; magnitudes.len()];
    }
    let total: f32 = magnitudes.iter().sum();
    let raw: Vec<f32> = magnitudes
        .iter()
        .map(|magnitude| budget as f32 * magnitude / total)
        .collect();
    let mut parts: Vec<i32> = raw.iter().map(|value| value.floor() as i32).collect();
    let assigned = parts.iter().sum::<i32>() as usize;
    let mut order: Vec<usize> = (0..raw.len()).collect();
    order.sort_by(|left, right| {
        let left_remainder = raw[*left] - parts[*left] as f32;
        let right_remainder = raw[*right] - parts[*right] as f32;
        right_remainder
            .total_cmp(&left_remainder)
            .then_with(|| left.cmp(right))
    });
    for index in order.into_iter().take(budget.saturating_sub(assigned)) {
        parts[index] += 1;
    }
    let sign = signed_budget.signum();
    parts.iter_mut().for_each(|part| *part *= sign);
    parts
}

fn apply_deltas_and_save(
    engine: &mut crate::search::SearchEngine,
    index_path: &str,
    vector_mirror: &crate::store::OnlineVectorMirror,
    chunks: &[FeedbackChunk],
    deltas: &[i32],
) -> anyhow::Result<(usize, usize)> {
    let previous = engine.to_vec();
    let mut success_count = 0;
    let mut fail_count = 0;
    for (chunk, delta) in chunks.iter().zip(deltas.iter().copied()) {
        if delta == 0 {
            continue;
        }
        if engine
            .adjust_chunk_pagerank(&chunk.id, &chunk.kb_id, delta)
            .is_some()
        {
            success_count += 1;
        } else {
            fail_count += 1;
        }
    }
    if success_count > 0 {
        crate::store::persist_online_index(engine, index_path, vector_mirror, previous)?;
    }
    Ok((success_count, fail_count))
}

fn apply_feedback_transition(
    engine: &mut crate::search::SearchEngine,
    index_path: &str,
    vector_mirror: &crate::store::OnlineVectorMirror,
    chunks: &[FeedbackChunk],
    prior_thumb: Option<bool>,
    thumbup: bool,
    mode: FeedbackWeighting,
) -> anyhow::Result<(usize, usize)> {
    let previous = engine.to_vec();
    let mut deltas = vec![0; chunks.len()];
    if let Some(prior_thumb) = prior_thumb {
        let undo_budget = if prior_thumb {
            -FEEDBACK_BUDGET
        } else {
            FEEDBACK_BUDGET
        };
        for (combined, delta) in deltas
            .iter_mut()
            .zip(allocate_deltas(chunks, undo_budget, mode))
        {
            *combined += delta;
        }
    }
    let new_budget = if thumbup {
        FEEDBACK_BUDGET
    } else {
        -FEEDBACK_BUDGET
    };
    for (combined, delta) in deltas
        .iter_mut()
        .zip(allocate_deltas(chunks, new_budget, mode))
    {
        *combined += delta;
    }
    apply_deltas_and_save(engine, index_path, vector_mirror, chunks, &deltas).inspect_err(|_| {
        *engine = crate::search::SearchEngine::from_chunks(previous);
    })
}

fn rollback_index(
    engine: &mut crate::search::SearchEngine,
    index_path: &str,
    vector_mirror: &crate::store::OnlineVectorMirror,
    previous: Vec<crate::search::IndexedChunk>,
) {
    crate::store::rollback_online_index(
        engine,
        index_path,
        vector_mirror,
        previous,
        "Feedback index rollback",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{IndexedChunk, SearchEngine};
    use std::collections::HashMap;

    fn chunk(id: &str, score: Option<f32>) -> FeedbackChunk {
        FeedbackChunk {
            id: id.into(),
            kb_id: "kb-a".into(),
            similarity: score,
            vector_similarity: None,
            term_similarity: None,
        }
    }

    #[test]
    fn relevance_mode_spends_one_unit_on_the_strongest_reference() {
        let chunks = vec![chunk("strong", Some(0.9)), chunk("weak", Some(0.1))];
        assert_eq!(
            allocate_deltas(&chunks, 1, FeedbackWeighting::Relevance),
            vec![1, 0]
        );
        assert_eq!(
            allocate_deltas(&chunks, -1, FeedbackWeighting::Relevance),
            vec![-1, 0]
        );
    }

    #[test]
    fn uniform_mode_updates_every_reference() {
        let chunks = vec![chunk("a", None), chunk("b", None)];
        assert_eq!(
            allocate_deltas(&chunks, -1, FeedbackWeighting::Uniform),
            vec![-1, -1]
        );
    }

    #[test]
    fn duplicate_chunk_references_are_applied_once() {
        let chunks = deduplicate_chunks(vec![chunk("same", Some(0.8)), chunk("same", Some(0.2))]);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].similarity, Some(0.8));
    }

    #[test]
    fn failed_index_persistence_rolls_back_feedback() {
        let root =
            std::env::temp_dir().join(format!("rayrag-feedback-rollback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let invalid_index_path = root.join("index.json");
        std::fs::create_dir(&invalid_index_path).unwrap();
        let mut engine = SearchEngine::from_chunks(vec![IndexedChunk {
            id: "chunk-a".into(),
            doc_name: "document.txt".into(),
            content: "content".into(),
            embedding: vec![],
            token_count: 1,
            position: 0,
            metadata: HashMap::from([("kb_id".into(), "kb-a".into())]),
        }]);

        let error = apply_deltas_and_save(
            &mut engine,
            invalid_index_path.to_str().unwrap(),
            &crate::store::OnlineVectorMirror::disabled(),
            &[chunk("chunk-a", Some(1.0))],
            &[1],
        )
        .unwrap_err();

        assert!(!error.to_string().is_empty());
        assert_eq!(engine.to_vec()[0].metadata.get("pagerank_fea"), None);
    }
}
