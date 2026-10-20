//! The tool catalogue and the capability report.
//!
//! Two endpoints describe what this deployment can do, and both used to answer from invented data:
//! `GET /api/v1/mcp/tools` returned two hard-coded tools that no code could execute, and
//! `GET /api/v1/plugins` returned a fixed list naming `PaddleOCR` and `mxbai-rerank` whatever the
//! deployment was configured with. They now read one source of truth:
//!
//! * the tool catalogue is built from [`crate::mcp_server::ToolRegistry`], the same registry the MCP
//!   surface uses, with the caller's own dataset list appended to the retrieval tool's description —
//!   the tool is told which datasets exist because a caller cannot pass an id it has never seen;
//! * the capability report states, for each capability, whether this deployment has it configured and
//!   which model provides it, read from the running state rather than from a list in a source file.
//!
//! `POST /api/v1/plugins/{id}/toggle` answers `103` with the reason instead of the old
//! `"Plugin X toggled"`, which was a success message for an operation that did nothing: OCR, reranking
//! and the rest are provider-backed and configured through the wizard or the environment, so a runtime
//! toggle would be a claim the deployment cannot honour.

use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};

use crate::server::{AppState, AuthContext, api_error_code, code};

/// One tool offered to an LLM, in the shape upstream's `get_metadata()` produces.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct ToolMetadata {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// The datasets the caller may use, in the shape the registry enriches its retrieval tool with.
pub(crate) fn caller_datasets(
    state: &AppState,
    auth: &AuthContext,
) -> Vec<crate::mcp_server::DatasetInfo> {
    state
        .kbs
        .list_accessible(&auth.user_id, auth.is_admin, |tenant_id, user_id| {
            state.tenants.is_member(tenant_id, user_id)
        })
        .into_iter()
        .map(|dataset| crate::mcp_server::DatasetInfo {
            id: dataset.id,
            description: dataset.name,
        })
        .collect()
}

/// Every tool this deployment offers, from the registry rather than from a literal in a handler.
pub(crate) fn catalogue(state: &AppState, auth: &AuthContext) -> Vec<ToolMetadata> {
    // The registry is the catalogue; enriching it is how the retrieval tool learns which datasets
    // exist, because a caller cannot pass an id it has never seen.
    let registry =
        crate::mcp_server::ToolRegistry::default().enrich_datasets(&caller_datasets(state, auth));
    registry
        .list()
        .iter()
        .map(|tool| ToolMetadata {
            tool_type: "function".to_string(),
            function: ToolFunction {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.input_schema.clone(),
            },
        })
        .collect()
}

/// One line of the capability report.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct Capability {
    pub id: String,
    pub name: String,
    /// Whether this deployment is configured for the capability. It is not a liveness probe.
    pub available: bool,
    /// The model or endpoint behind it, when one is configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Where a deployment changes this, so the report is actionable rather than decorative.
    pub configured_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

fn env_value(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// What this deployment can actually do, read from the running state and the environment.
pub(crate) fn capabilities(state: &AppState) -> Vec<Capability> {
    let mut report = Vec::new();
    // A capability that is absent says why: a bare `false` leaves the reader with nothing to act on.
    let has_llm = state.llm.is_some();
    report.push(Capability {
        id: "llm".into(),
        name: "Chat completion".into(),
        available: has_llm,
        model: env_value("LLM_MODEL").or_else(|| env_value("LLM_ID")),
        configured_by: "LLM_API_BASE / LLM_MODEL".into(),
        detail: (!has_llm).then(|| {
            "No chat model is configured, so answering is unavailable. Set LLM_API_BASE and LLM_MODEL, or use the first-login wizard.".into()
        }),
    });
    let has_embedder = state.embedder.is_some();
    report.push(Capability {
        id: "embedding".into(),
        name: "Embedding".into(),
        available: has_embedder,
        model: env_value("EMBED_MODEL").or_else(|| env_value("EMBEDDING_MODEL")),
        configured_by: "EMBED_API_BASE / EMBED_MODEL".into(),
        detail: (!has_embedder).then(|| {
            "No embedding endpoint is configured, so parsing and retrieval cannot index text. Set EMBED_API_BASE and EMBED_MODEL.".into()
        }),
    });
    // Reranking is provider-backed: report the endpoint rather than guessing whether it answers.
    let rerank_base = env_value("RERANK_API_BASE");
    report.push(Capability {
        id: "rerank".into(),
        name: "Reranking".into(),
        available: rerank_base.is_some(),
        model: env_value("RERANK_MODEL"),
        configured_by: "RERANK_API_BASE / RERANK_MODEL".into(),
        detail: rerank_base.is_none().then(|| {
            "No rerank endpoint is configured; retrieval uses vector and term similarity.".into()
        }),
    });
    let vision = env_value("VISION_API_BASE").or_else(|| env_value("IMAGE2TEXT_API_BASE"));
    report.push(Capability {
        id: "vision".into(),
        name: "Image understanding".into(),
        available: vision.is_some(),
        model: env_value("VISION_MODEL").or_else(|| env_value("IMAGE2TEXT_MODEL")),
        configured_by: "VISION_API_BASE / VISION_MODEL".into(),
        detail: vision.is_none().then(|| {
            "Without a vision model, images are read through OCR and their captions only.".into()
        }),
    });
    let ocr = env_value("OCR_API_BASE").or_else(|| env_value("RAYRAG_OCR_API_BASE"));
    report.push(Capability {
        id: "ocr".into(),
        name: "OCR".into(),
        // The built-in reader always exists, so OCR is available; a remote provider only replaces it.
        available: true,
        model: ocr.clone(),
        configured_by: "OCR_API_BASE (optional; a built-in reader is always present)".into(),
        detail: ocr.is_none().then(|| {
            "Using the built-in reader; set OCR_API_BASE to delegate to a provider.".into()
        }),
    });
    for (id, name, base_key, model_key, what) in [
        (
            "tts",
            "Text to speech",
            "RAYRAG_TTS_API_BASE",
            "RAYRAG_TTS_MODEL",
            "text to speech",
        ),
        (
            "asr",
            "Speech to text",
            "RAYRAG_ASR_API_BASE",
            "RAYRAG_ASR_MODEL",
            "speech to text",
        ),
    ] {
        let base = env_value(base_key);
        report.push(Capability {
            id: id.into(),
            name: name.into(),
            available: base.is_some(),
            model: env_value(model_key),
            configured_by: format!("{base_key} / {model_key}"),
            detail: base.is_none().then(|| {
                format!("No endpoint is configured, so {what} answers that no model is set.")
            }),
        });
    }
    report
}

/// `GET /api/v1/plugin/tools`.
pub async fn plugin_tools(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    let tools = catalogue(&state, &auth);
    Json(serde_json::json!({
        "code": 0,
        "data": tools,
        "tool_count": tools.len(),
        "message": "success",
    }))
    .into_response()
}

/// `GET /api/v1/plugins`.
pub async fn list_plugins(
    State(state): State<Arc<AppState>>,
    Extension(_auth): Extension<AuthContext>,
) -> Response {
    let capabilities = capabilities(&state);
    Json(serde_json::json!({
        "code": 0,
        "data": {
            "capabilities": capabilities,
            "configured": capabilities.iter().filter(|entry| entry.available).count(),
            "total": capabilities.len(),
            // The wording matters: this is a configuration report, not a liveness probe. A reranker
            // whose endpoint is configured but stopped would otherwise read as working.
            "note": "`available` means this deployment is configured for the capability, not that the endpoint is reachable right now. Test a provider with its own check, for example POST /api/v1/datasets/{id}/embedding/check.",
        },
        "message": "success",
    }))
    .into_response()
}

/// `POST /api/v1/plugins/{id}/toggle`.
pub async fn toggle_plugin(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    if let Some(response) = crate::api::connector::require_admin_response(&auth) {
        return response;
    }
    let known: Vec<String> = capabilities(&state)
        .into_iter()
        .map(|entry| entry.id)
        .collect();
    if !known.iter().any(|candidate| candidate == &id) {
        return api_error_code(
            axum::http::StatusCode::NOT_FOUND,
            code::INVALID_OR_MISSING_DATA,
            &format!("Unknown capability '{id}'. Known: {}.", known.join(", ")),
        );
    }
    // A false success would be worse than a refusal: nothing about these capabilities can be switched
    // at runtime, and saying "toggled" would leave the caller believing it had been.
    api_error_code(
        axum::http::StatusCode::OK,
        code::OPERATION_ERROR,
        &format!(
            "'{id}' is provided by this deployment's configuration, not toggled at runtime. Change it in the first-login wizard or the environment file and restart."
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_comes_from_the_registry_and_names_the_datasets_that_exist() {
        // A catalogue built with no datasets still offers the tools; the enrichment only changes the
        // retrieval tool's description, never whether a tool exists.
        let empty = crate::mcp_server::ToolRegistry::default();
        let names: Vec<&str> = empty.list().iter().map(|tool| tool.name.as_str()).collect();
        assert!(names.contains(&"ragflow_retrieval"), "{names:?}");
        assert!(names.contains(&"list_datasets"), "{names:?}");
        // The tools this deployment offers are the registry's, so nothing can drift from it.
        let enriched = crate::mcp_server::ToolRegistry::default().enrich_datasets(&[
            crate::mcp_server::DatasetInfo {
                id: "kb-1".into(),
                description: "Handbook".into(),
            },
        ]);
        let retrieval = enriched
            .list()
            .iter()
            .find(|tool| tool.name == "ragflow_retrieval")
            .expect("the retrieval tool exists");
        assert!(
            retrieval.description.contains("kb-1"),
            "the live dataset listing reaches the tool: {}",
            retrieval.description
        );
    }
}
