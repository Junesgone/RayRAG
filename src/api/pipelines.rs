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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn list_serves_the_go_catalog_shape() {
        let Json(value) = list_pipelines().await;
        assert_eq!(value["code"], 0);
        assert_eq!(value["message"], "success");
        let data = &value["data"];
        assert_eq!(data["total"], 12);
        let canvas = data["canvas"].as_array().expect("canvas array");
        assert_eq!(canvas.len(), 12);
        assert_eq!(canvas[0]["id"], "general");
        assert_eq!(canvas[0]["title"], "General");
        assert!(
            canvas[0]["filename"]
                .as_str()
                .unwrap()
                .starts_with("ingestion_pipeline_")
        );
        assert_eq!(canvas[1]["id"], "qa");
        assert_eq!(canvas[11]["id"], "email");
    }

    #[tokio::test]
    async fn detail_returns_the_dsl_and_alias() {
        let response = get_pipeline(Path("general".to_string())).await;
        assert_eq!(response.status(), 200);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["code"], 0);
        assert!(value["data"]["dsl"]["components"].is_object());

        let response = get_pipeline(Path("naive".to_string())).await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["code"], 0, "naive is an alias of general");
    }

    #[tokio::test]
    async fn unknown_refs_answer_the_upstream_error() {
        let response = get_pipeline(Path("nonexistent".to_string())).await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["code"], 102);
        assert_eq!(value["message"], "pipeline not found");
        assert!(value.get("data").is_none() || value["data"].is_null());
    }
}
