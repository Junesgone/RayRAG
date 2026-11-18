//! RAGFlow v0.27.2 Go-backend ingestion pipeline catalog API.
//!
//! Go handler: internal/handler/pipeline.go.
//!   GET /api/v1/pipelines       public static catalog
//!   GET /api/v1/pipelines/:id   a single template with its DSL
//!
//! Envelopes follow common.SuccessWithData / ErrorWithCode: HTTP 200 with
//! {code,data,message}. The catalog is public static data, so no auth is
//! required, exactly like the Go router (apiNoAuth group).

use axum::{
    Json,
    extract::Path,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

/// GET /api/v1/pipelines — built-in pipeline catalog.
pub async fn list_pipelines() -> Json<Value> {
    let canvas: Vec<Value> = crate::pipeline::builtin::list()
        .iter()
        .map(|pipeline| {
            json!({
                "id": pipeline.id,
                "title": pipeline.title,
                "description": pipeline.description,
                "filename": pipeline.filename,
            })
        })
        .collect();
    let total = canvas.len();
    Json(json!({
        "code": 0,
        "message": "success",
        "data": { "canvas": canvas, "total": total },
    }))
}

/// GET /api/v1/pipelines/:id — one built-in template with its DSL.
pub async fn get_pipeline(Path(reference): Path<String>) -> Response {
    match crate::pipeline::builtin::get(&reference) {
        Some(pipeline) => Json(json!({
            "code": 0,
            "message": "success",
            "data": { "dsl": pipeline.dsl },
        }))
        .into_response(),
        None => Json(json!({
            "code": 102,
            "message": "pipeline not found",
        }))
        .into_response(),
    }
}


