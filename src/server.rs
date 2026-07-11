//! RayRAG Web Server — serves the RAGFlow-derived UI + Rust API backend.
//!
//! Architecture:
//!   /           → SPA static files (web/static/)
//!   /api/v1/*   → RayRAG API endpoints (Rust handlers)
//!
//! Startup: `rayrag serve --port 9380`

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Json, Response},
    routing::get,
};
use serde::Serialize;
use std::sync::Arc;
use tower_http::{
    cors::CorsLayer,
    services::ServeDir,
};
use tracing;

// ── API Response types ──────────────────────────────────────────

#[derive(Serialize)]
struct ApiResponse<T: Serialize> {
    code: i32,
    message: String,
    data: Option<T>,
}

#[derive(Serialize)]
struct SystemStatus {
    status: String,
}

#[derive(Serialize)]
struct VersionInfo {
    version: String,
}

#[derive(Serialize)]
struct NewToken {
    token: String,
}

#[derive(Serialize)]
struct TokenData {
    access_token: String,
    user_id: String,
}

// ── App State ───────────────────────────────────────────────────

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// Static file root directory
    pub static_dir: String,
    /// Server port
    pub port: u16,
}

// ── API Handlers ────────────────────────────────────────────────

/// GET /api/v1/system/status
async fn system_status() -> Json<ApiResponse<SystemStatus>> {
    Json(ApiResponse {
        code: 0,
        message: "success".into(),
        data: Some(SystemStatus {
            status: "healthy".into(),
        }),
    })
}

/// GET /api/v1/version — compatibility with RAGFlow frontend
async fn version() -> Json<ApiResponse<VersionInfo>> {
    Json(ApiResponse {
        code: 0,
        message: "success".into(),
        data: Some(VersionInfo {
            version: env!("CARGO_PKG_VERSION").into(),
        }),
    })
}

/// POST /api/v1/auth/login — stub (frontend checks this on load)
async fn login() -> Json<ApiResponse<TokenData>> {
    Json(ApiResponse {
        code: 0,
        message: "success".into(),
        data: Some(TokenData {
            access_token: "rayrag-default-token".into(),
            user_id: "admin".into(),
        }),
    })
}

/// POST /api/v1/user/register — stub
async fn register() -> Json<ApiResponse<()>> {
    Json(ApiResponse {
        code: 0,
        message: "success".into(),
        data: None,
    })
}

/// POST /api/v1/user/setting — stub
async fn user_setting() -> Json<ApiResponse<()>> {
    Json(ApiResponse {
        code: 0,
        message: "success".into(),
        data: None,
    })
}

/// GET /api/v1/new_token
async fn new_token() -> Json<ApiResponse<NewToken>> {
    Json(ApiResponse {
        code: 0,
        message: "success".into(),
        data: Some(NewToken {
            token: "rayrag-default-token".into(),
        }),
    })
}

/// GET /api/v1/datasets — stub for dataset list
async fn list_datasets() -> Json<ApiResponse<Vec<serde_json::Value>>> {
    Json(ApiResponse {
        code: 0,
        message: "success".into(),
        data: Some(vec![]),
    })
}

/// GET /api/v1/knowledge/list — stub
async fn knowledge_list() -> Json<ApiResponse<Vec<serde_json::Value>>> {
    Json(ApiResponse {
        code: 0,
        message: "success".into(),
        data: Some(vec![]),
    })
}

/// POST /api/v1/chunks — stub for chunk retrieval
async fn retrieval_chunks() -> Json<ApiResponse<Vec<serde_json::Value>>> {
    Json(ApiResponse {
        code: 0,
        message: "success".into(),
        data: Some(vec![]),
    })
}

/// POST /api/v1/retrieval — stub for document retrieval
async fn retrieval() -> Json<ApiResponse<Vec<serde_json::Value>>> {
    Json(ApiResponse {
        code: 0,
        message: "success".into(),
        data: Some(vec![]),
    })
}

/// POST /api/v1/admin/* — stub
async fn admin_handler() -> Json<ApiResponse<()>> {
    Json(ApiResponse {
        code: 0,
        message: "success".into(),
        data: None,
    })
}

/// Catch-all API handler for unimplemented endpoints
async fn api_fallback() -> Json<ApiResponse<()>> {
    Json(ApiResponse {
        code: 0,
        message: "RayRAG API — this endpoint is not yet implemented".into(),
        data: None,
    })
}

// ── Static File Serving ─────────────────────────────────────────

/// SPA fallback: serve index.html for any non-file route
async fn spa_fallback(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Response {
    let path = req.uri().path();

    // If it looks like a file (has extension), return 404
    // Otherwise serve index.html for client-side routing
    if std::path::Path::new(path)
        .extension()
        .is_some_and(|ext| ext != "html")
    {
        return StatusCode::NOT_FOUND.into_response();
    }

    match tokio::fs::read(format!("{}/index.html", state.static_dir)).await {
        Ok(content) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(content))
            .unwrap(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

// ── Router Construction ─────────────────────────────────────────

/// Build the Axum router with all routes.
pub fn build_router(state: Arc<AppState>) -> Router {
    let static_dir = state.static_dir.clone();

    // Serve directory for static assets
    let serve_dir = ServeDir::new(&static_dir)
        .precompressed_gzip()
        .precompressed_br();

    // API routes — organized like RAGFlow's nginx config
    let api_routes = Router::new()
        // System / health
        .route("/v1/system/status", get(system_status))
        .route("/v1/version", get(version))
        // Auth
        .route("/v1/auth/login", get(login).post(login))
        .route("/v1/user/register", get(register).post(register))
        .route("/v1/user/setting", get(user_setting).post(user_setting))
        .route("/v1/new_token", get(new_token))
        // Datasets / knowledge bases
        .route("/v1/datasets", get(list_datasets))
        .route("/v1/knowledge/list", get(knowledge_list))
        // Retrieval
        .route("/v1/chunks", get(retrieval_chunks).post(retrieval_chunks))
        .route("/v1/retrieval", get(retrieval).post(retrieval))
        // Admin
        .route("/v1/admin/{*rest}", get(admin_handler).post(admin_handler))
        // Legacy prefix
        .route("/api/v1/{*rest}", get(api_fallback).post(api_fallback));

    Router::new()
        // API first (exact matches)
        .nest("/api", api_routes)
        // Static files (assets, JS, CSS, etc.)
        .nest_service("/", serve_dir)
        // SPA fallback: all unmatched routes → index.html
        .fallback(get(spa_fallback))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

// ── Server Entry Point ──────────────────────────────────────────

/// Run the web server on the given port.
pub async fn run(port: u16, static_dir: &str) -> anyhow::Result<()> {
    let state = Arc::new(AppState {
        static_dir: static_dir.to_string(),
        port,
    });

    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;

    tracing::info!("RayRAG server listening on http://0.0.0.0:{}", port);
    tracing::info!("Serving static files from: {}", static_dir);

    axum::serve(listener, router).await?;
    Ok(())
}
