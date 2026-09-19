//! RAGFlow 0.26.4 agent-log parity — `api/apps/restful_apis/agent_api.py`
//! (`list_agent_sessions`, `get_agent_session`) plus the web-API alias the
//! browser actually calls.
//!
//! The page at `/agent-log-page/:id` (`web/src/pages/agents/agent-log-page.tsx`)
//! reads its table from `api.ts::fetchAgentLogs`, which the browser resolves to
//! `GET /v1/canvas/{canvas_id}/sessions`. In this pinned RAGFlow release that
//! alias no longer exists on the server — the route moved to
//! `/api/v1/agents/{agent_id}/sessions` when the `sdk` blueprints were folded
//! into `restful_apis`, while `api.ts` kept the old `webAPI` path — so upstream
//! ships a log page whose request 404s. RayRAG serves both spellings from one
//! handler so the ported page works and the REST surface stays aligned.
//!
//! Query semantics mirror `API4ConversationService.get_list`:
//! `id` / `user_id` / `exp_user_id` equality filters, a case-insensitive
//! `keywords` match against the serialized `message` column, `from_date` /
//! `to_date` compared against `update_date` when `orderby` starts with
//! `update_` and against `create_date` otherwise, `orderby` + `desc` ordering,
//! `page` / `page_size` pagination with `REST_API_MAX_PAGE_SIZE = 100`, and
//! `dsl=false` to drop the DSL snapshot from every row. Rows are shaped by
//! `_normalize_agent_session`: `dialog_id` becomes `agent_id`, per-message
//! `prompt` fields are dropped, and the session-level `reference` array is
//! folded into each assistant message's `reference` list.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::llm::{ChatMessage, ChunkReference, Conversation};
use crate::server::{AppState, AuthContext};

/// `REST_API_MAX_PAGE_SIZE` (`api/utils/pagination_utils.py`).
pub const REST_API_MAX_PAGE_SIZE: usize = 100;
/// `request.args.get("page_size", 30)` in `list_agent_sessions`.
pub const DEFAULT_AGENT_LOG_PAGE_SIZE: usize = 30;
/// `request.args.get("orderby", "update_time")`.
pub const DEFAULT_AGENT_LOG_ORDERBY: &str = "update_time";

/// Parsed `list_agent_sessions` query string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionQuery {
    pub session_id: String,
    pub user_id: String,
    pub exp_user_id: String,
    pub page: usize,
    pub page_size: usize,
    pub keywords: String,
    pub from_date: String,
    pub to_date: String,
    pub orderby: String,
    pub desc: bool,
    /// `dsl` query flag (upstream `include_dsl`).
    pub include_dsl: bool,
}

impl Default for SessionQuery {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            user_id: String::new(),
            exp_user_id: String::new(),
            page: 1,
            page_size: DEFAULT_AGENT_LOG_PAGE_SIZE,
            keywords: String::new(),
            from_date: String::new(),
            to_date: String::new(),
            orderby: DEFAULT_AGENT_LOG_ORDERBY.to_string(),
            desc: true,
            include_dsl: true,
        }
    }
}

/// `_is_false_flag`: upstream compares the raw argument against the two
/// spellings `False` / `false` (`not in {"False", "false"}`), so every other
/// value — including an empty string — is truthy.
fn is_false_flag(value: &str) -> bool {
    value == "False" || value == "false"
}

impl SessionQuery {
    /// Parse the request query string with upstream defaults. Python's `int()`
    /// raises on garbage and upstream turns that into a 500; RayRAG falls back
    /// to the documented default instead of failing the page.
    pub fn parse(params: &HashMap<String, String>) -> Result<Self, String> {
        let get = |key: &str| params.get(key).cloned().unwrap_or_default();
        let page = get("page").trim().parse::<usize>().unwrap_or(1).max(1);
        let raw_page_size = match get("page_size").trim() {
            value if value.is_empty() => DEFAULT_AGENT_LOG_PAGE_SIZE,
            value => value
                .parse::<usize>()
                .unwrap_or(DEFAULT_AGENT_LOG_PAGE_SIZE),
        };
        if raw_page_size > REST_API_MAX_PAGE_SIZE {
            return Err(format!(
                "page_size must be less than or equal to {REST_API_MAX_PAGE_SIZE}"
            ));
        }
        let orderby = match get("orderby") {
            value if value.trim().is_empty() => DEFAULT_AGENT_LOG_ORDERBY.to_string(),
            value => value,
        };
        Ok(Self {
            session_id: get("id"),
            user_id: get("user_id"),
            exp_user_id: get("exp_user_id"),
            page,
            page_size: raw_page_size,
            keywords: get("keywords"),
            from_date: get("from_date"),
            to_date: get("to_date"),
            orderby,
            desc: !is_false_flag(&get("desc")),
            include_dsl: !is_false_flag(&get("dsl")),
        })
    }
}

/// `API4ConversationService._normalize_query_date`: an ISO-8601 value is
/// converted to local wall-clock `%Y-%m-%d %H:%M:%S`, a bare `YYYY-MM-DD` is
/// widened to the day's first or last second, anything else passes through.
pub fn normalize_query_date(value: &str, is_end: bool) -> String {
    if value.contains('T') {
        if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(value) {
            return parsed
                .with_timezone(&chrono::Local)
                .naive_local()
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();
        }
        return value.to_string();
    }
    if value.len() == 10 {
        return format!("{value} {}", if is_end { "23:59:59" } else { "00:00:00" });
    }
    value.to_string()
}

/// `json_dumps(keywords)[1:-1]` — the JSON escaping of the keyword with the
/// surrounding quotes removed. Plain ASCII keywords escape to themselves, so
/// upstream only adds a second `contains()` branch for exotic input.
fn json_escaped_keyword(keywords: &str) -> String {
    let dumped = serde_json::Value::String(keywords.to_string()).to_string();
    dumped
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(keywords)
        .to_string()
}

/// RAGFlow persists naive **server-local** datetimes
/// (`DataBaseModel.create_date = DateTimeField(default=datetime.now)`) and
/// `CustomJSONEncoder` writes them back unchanged, so the log rows and the
/// `from_date`/`to_date` comparisons use the process timezone. Deployments pin
/// that timezone with `TZ` (the compose file defaults to `Asia/Shanghai`, like
/// RAGFlow's `TIMEZONE`), which keeps the browser's picked day aligned with the
/// stored day.
pub fn format_local_datetime(timestamp_ms: u64) -> String {
    let seconds = (timestamp_ms / 1000) as i64;
    let nanos = ((timestamp_ms % 1000) * 1_000_000) as u32;
    match chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, nanos) {
        Some(dt) => dt
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        None => String::new(),
    }
}

/// Session-level dates compared by the filters and used by the sort.
fn session_dates(session: &Conversation) -> (String, String) {
    (
        format_local_datetime(session.created_at),
        format_local_datetime(session.updated_at),
    )
}

/// `conv.round`: upstream increments the column once per appended exchange, so
/// the number of user turns is the same count.
pub fn session_round(session: &Conversation) -> usize {
    session
        .messages
        .iter()
        .filter(|message| message.role == "user")
        .count()
}

/// `conv.thumb_up`: upstream keeps a counter column; RayRAG stores the thumb on
/// the message, so the positive votes are counted here.
pub fn session_thumb_up(session: &Conversation) -> i64 {
    session
        .messages
        .iter()
        .filter(|message| message.thumbup == Some(true))
        .count() as i64
}

/// Chunk identity resolved from the live index, standing in for the retrieval
/// aggregation fields (`docnm_kwd`, `doc_id`, `kb_id`) upstream stores on every
/// reference chunk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChunkIdentity {
    pub document_id: String,
    pub document_name: String,
    pub dataset_id: String,
    pub positions: Vec<i64>,
}

/// `_normalize_agent_reference_chunk`: the upstream field fallbacks
/// (`chunk_id`/`id`, `content_with_weight`/`content`, `doc_id`/`document_id`,
/// `docnm_kwd`/`document_name`, `kb_id`/`dataset_id`, `image_id`/`img_id`,
/// `positions`/`position_int`).
pub fn normalize_reference_chunk(
    reference: &ChunkReference,
    identity: Option<&ChunkIdentity>,
) -> serde_json::Value {
    let identity = identity.cloned().unwrap_or_default();
    let document_id = if identity.document_id.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(identity.document_id)
    };
    let document_name = if identity.document_name.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(identity.document_name)
    };
    let dataset_id = if identity.dataset_id.is_empty() {
        serde_json::Value::String(reference.kb_id.clone())
    } else {
        serde_json::Value::String(identity.dataset_id)
    };
    serde_json::json!({
        "id": reference.id,
        "content": reference.content,
        "document_id": document_id,
        "document_name": document_name,
        "dataset_id": dataset_id,
        "image_id": serde_json::Value::Null,
        "positions": if identity.positions.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::json!(identity.positions)
        },
        "similarity": reference.similarity,
        "vector_similarity": reference.vector_similarity,
        "term_similarity": reference.term_similarity,
    })
}

/// One `message` entry of `_normalize_agent_session`. Assistant messages carry
/// the flow's reference chunks, exactly like upstream attaches the session
/// `reference` entries to the non-user messages after the prologue.
pub fn normalize_message(
    message: &ChatMessage,
    identity: &dyn Fn(&str) -> Option<ChunkIdentity>,
) -> serde_json::Value {
    let mut value = serde_json::json!({
        "id": message.id,
        "role": message.role,
        "content": message.content,
        "created_at": message.created_at,
    });
    if message.role != "user" && !message.references.is_empty() {
        let chunks: Vec<serde_json::Value> = message
            .references
            .iter()
            .map(|reference| normalize_reference_chunk(reference, identity(&reference.id).as_ref()))
            .collect();
        value["reference"] = serde_json::json!(chunks);
    }
    value
}

/// `_normalize_agent_session` for one session row: `message` is normalized in
/// place (dropping per-message `prompt`), `dialog_id` is renamed to `agent_id`,
/// the session-level `reference` is folded into the assistant messages and then
/// removed, and the remaining columns are emitted with RAGFlow's
/// `%Y-%m-%d %H:%M:%S` datetime encoding.
pub fn normalize_agent_session(
    session: &Conversation,
    query: &SessionQuery,
    identity: &dyn Fn(&str) -> Option<ChunkIdentity>,
) -> serde_json::Value {
    let (create_date, update_date) = session_dates(session);
    let messages: Vec<serde_json::Value> = session
        .messages
        .iter()
        .map(|message| normalize_message(message, identity))
        .collect();
    let mut row = serde_json::json!({
        "id": session.id,
        "name": session.name,
        "user_id": session.owner_id,
        "exp_user_id": session.owner_id,
        "message": messages,
        "source": session.source,
        "duration": session.duration_ms as f64 / 1000.0,
        "round": session_round(session),
        "thumb_up": session_thumb_up(session),
        "errors": session.errors.clone(),
        "version_title": session.version_title.clone(),
        "create_time": session.created_at,
        "create_date": create_date,
        "update_time": session.updated_at,
        "update_date": update_date,
        "agent_id": session.canvas_id.clone(),
    });
    if query.include_dsl {
        row["dsl"] = session.dsl.clone().unwrap_or(serde_json::Value::Null);
    }
    row
}

/// `API4ConversationService.get_list` filtering + ordering + pagination.
/// Returns the total row count before pagination plus the requested page.
pub fn select_sessions(
    sessions: Vec<Conversation>,
    query: &SessionQuery,
) -> (usize, Vec<Conversation>) {
    let keywords = query.keywords.to_lowercase();
    let escaped = json_escaped_keyword(&keywords);
    let from_date = normalize_query_date(&query.from_date, false);
    let to_date = normalize_query_date(&query.to_date, true);
    let date_is_update = query.orderby.starts_with("update_");
    let mut rows: Vec<Conversation> = sessions
        .into_iter()
        .filter(|session| query.session_id.is_empty() || session.id == query.session_id)
        .filter(|session| query.user_id.is_empty() || session.owner_id == query.user_id)
        .filter(|session| query.exp_user_id.is_empty() || session.owner_id == query.exp_user_id)
        .filter(|session| {
            if keywords.is_empty() {
                return true;
            }
            let serialized = serde_json::to_string(&session.messages)
                .unwrap_or_default()
                .to_lowercase();
            serialized.contains(&keywords) || (escaped != keywords && serialized.contains(&escaped))
        })
        .filter(|session| {
            if query.from_date.is_empty() && query.to_date.is_empty() {
                return true;
            }
            let (create_date, update_date) = session_dates(session);
            let field = if date_is_update {
                update_date
            } else {
                create_date
            };
            (query.from_date.is_empty() || field >= from_date)
                && (query.to_date.is_empty() || field <= to_date)
        })
        .collect();
    let order_key = |session: &Conversation| -> String {
        let (create_date, update_date) = session_dates(session);
        match query.orderby.as_str() {
            "id" => session.id.clone(),
            "name" => session.name.clone(),
            "user_id" | "exp_user_id" => session.owner_id.clone(),
            "round" => format!("{:020}", session_round(session)),
            "thumb_up" => format!("{:020}", session_thumb_up(session)),
            "tokens" => format!("{:020}", session_token_count(session)),
            "duration" => format!("{:020}", session.duration_ms),
            "create_date" => create_date,
            "update_date" => update_date,
            "create_time" => format!("{:020}", session.created_at),
            // `update_time` is the documented default; an unknown column falls
            // back to the same date field the filters use instead of raising
            // the way `getter_by` does upstream.
            _ => format!("{:020}", session.updated_at),
        }
    };
    rows.sort_by(|left, right| {
        let (left_key, right_key) = (order_key(left), order_key(right));
        if query.desc {
            right_key
                .cmp(&left_key)
                .then_with(|| left.id.cmp(&right.id))
        } else {
            left_key
                .cmp(&right_key)
                .then_with(|| left.id.cmp(&right.id))
        }
    });
    let total = rows.len();
    let start = query.page.saturating_sub(1).saturating_mul(query.page_size);
    let page: Vec<Conversation> = rows.into_iter().skip(start).take(query.page_size).collect();
    (total, page)
}

/// `conv.tokens`: upstream keeps a token column; RayRAG sums the per-message
/// usage it already stores.
pub fn session_token_count(session: &Conversation) -> usize {
    session
        .messages
        .iter()
        .filter_map(|message| message.usage.as_ref())
        .map(|usage| usage.total_tokens as usize)
        .sum()
}

/// The web-API alias `GET /v1/canvas/{canvas_id}/sessions` requested by
/// `api.ts::fetchAgentLogs` and the REST route
/// `GET /api/v1/agents/{agent_id}/sessions` from `list_agent_sessions`.
pub async fn list_canvas_sessions(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path(canvas_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let query = match SessionQuery::parse(&params) {
        Ok(query) => query,
        Err(message) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "code": 400, "message": message })),
            )
                .into_response();
        }
    };
    if state
        .agents
        .get_accessible(
            &canvas_id,
            &auth.user_id,
            auth.is_admin,
            |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
        )
        .is_none()
    {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Agent not found" })),
        )
            .into_response();
    }
    let sessions = state.conversations.list_agent_sessions(&canvas_id);
    // `exp_user_id` switches the handler to `API4ConversationService.get_names`,
    // which answers with `{id, name}` rows for an end-user's channel sessions.
    if !query.exp_user_id.is_empty() {
        let rows: Vec<serde_json::Value> = sessions
            .iter()
            .filter(|session| session.owner_id == query.exp_user_id)
            .map(|session| serde_json::json!({ "id": session.id, "name": session.name }))
            .collect();
        let total = rows.len();
        return Json(serde_json::json!({
            "code": 0,
            "message": "success",
            "data": rows,
            "total": total,
        }))
        .into_response();
    }
    let (total, page) = select_sessions(sessions, &query);
    let identity = chunk_index(&state);
    let data: Vec<serde_json::Value> = page
        .iter()
        .map(|session| normalize_agent_session(session, &query, &identity))
        .collect();
    Json(serde_json::json!({
        "code": 0,
        "message": "success",
        "data": data,
        "total": total,
    }))
    .into_response()
}

/// `GET /api/v1/agents/{agent_id}/sessions` — the REST spelling of the same
/// list endpoint.
pub async fn list_agent_sessions(
    state: State<Arc<AppState>>,
    auth: axum::extract::Extension<AuthContext>,
    Path(agent_id): Path<String>,
    query: Query<HashMap<String, String>>,
) -> Response {
    list_canvas_sessions(state, auth, Path(agent_id), query).await
}

/// `GET /api/v1/agents/{agent_id}/sessions/{session_id}` — `get_agent_session`
/// answers the stored conversation (`conv.to_dict()`), guarding that the
/// session belongs to the requested canvas.
pub async fn get_agent_session(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path((agent_id, session_id)): Path<(String, String)>,
) -> Response {
    if state
        .agents
        .get_accessible(
            &agent_id,
            &auth.user_id,
            auth.is_admin,
            |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
        )
        .is_none()
    {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Agent not found" })),
        )
            .into_response();
    }
    match state
        .conversations
        .get(&session_id)
        .filter(|session| session.canvas_id.as_deref() == Some(agent_id.as_str()))
    {
        Some(session) => {
            let query = SessionQuery::default();
            let identity = chunk_index(&state);
            Json(serde_json::json!({
                "code": 0,
                "message": "success",
                "data": normalize_agent_session(&session, &query, &identity),
            }))
            .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "code": 404,
                "message": "Session not found!",
            })),
        )
            .into_response(),
    }
}

/// `DELETE /api/v1/agents/{agent_id}/sessions/{session_id}` —
/// `delete_agent_session_item`: the session must exist, belong to the canvas
/// and be owned by the caller.
pub async fn delete_agent_session(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path((agent_id, session_id)): Path<(String, String)>,
) -> Response {
    if state
        .agents
        .get_accessible(
            &agent_id,
            &auth.user_id,
            auth.is_admin,
            |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
        )
        .is_none()
    {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Agent not found" })),
        )
            .into_response();
    }
    let exists = state
        .conversations
        .get(&session_id)
        .is_some_and(|session| session.canvas_id.as_deref() == Some(agent_id.as_str()));
    if !exists {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "code": 404,
                "message": "Session not found!",
            })),
        )
            .into_response();
    }
    match state.conversations.delete_for(&session_id, &auth.user_id) {
        Ok(deleted) => Json(serde_json::json!({ "code": 0, "data": deleted })).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
        )
            .into_response(),
    }
}

/// Chunk identity resolver backed by the live index: reference chunks only
/// store the chunk id, so the document id/name and dataset id are read back
/// from the indexed chunk and its document record.
fn chunk_index(state: &AppState) -> impl Fn(&str) -> Option<ChunkIdentity> + use<> {
    let docs: HashMap<String, (String, String)> = state
        .docs
        .list_all()
        .into_iter()
        .map(|doc| (doc.id, (doc.kb_id, doc.name)))
        .collect();
    let engine = state.engine.clone();
    move |chunk_id: &str| {
        let engine = engine.read().unwrap();
        let chunk = engine
            .to_vec()
            .into_iter()
            .find(|chunk| chunk.id == chunk_id)?;
        let doc_id = chunk.metadata.get("doc_id").cloned().unwrap_or_default();
        let (dataset_id, document_name) = docs.get(&doc_id).cloned().unwrap_or_default();
        Some(ChunkIdentity {
            document_id: doc_id,
            document_name,
            dataset_id,
            positions: vec![chunk.position as i64],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ChatMessage;

    fn session(id: &str, owner: &str, created_at: u64, updated_at: u64) -> Conversation {
        let mut user = ChatMessage::new("user", "what is ragflow?");
        user.created_at = created_at;
        let mut assistant = ChatMessage::new("assistant", "a retrieval engine");
        assistant.created_at = created_at;
        assistant.thumbup = Some(true);
        assistant.references = vec![ChunkReference {
            id: "chunk-1".into(),
            kb_id: "kb-1".into(),
            content: "retrieval augmented generation".into(),
            similarity: Some(0.9),
            vector_similarity: None,
            term_similarity: None,
        }];
        Conversation {
            id: id.into(),
            name: "session".into(),
            owner_id: owner.into(),
            tenant_id: owner.into(),
            source: "agent".into(),
            canvas_id: Some("canvas-1".into()),
            app_id: None,
            kb_ids: Vec::new(),
            chat_model: None,
            embedding_model: None,
            messages: vec![user, assistant],
            duration_ms: 1500,
            dsl: Some(serde_json::json!({"components": {}})),
            errors: None,
            version_title: Some("v1".into()),
            created_at,
            updated_at,
        }
    }

    #[test]
    fn query_defaults_match_upstream_argument_defaults() {
        let query = SessionQuery::parse(&HashMap::new()).unwrap();
        assert_eq!(query.page, 1);
        assert_eq!(query.page_size, 30);
        assert_eq!(query.orderby, "update_time");
        assert!(query.desc);
        assert!(query.include_dsl);
        assert!(query.keywords.is_empty());
    }

    #[test]
    fn query_reads_upstream_flags_and_rejects_oversize_pages() {
        let params = HashMap::from([
            ("page".to_string(), "3".to_string()),
            ("page_size".to_string(), "50".to_string()),
            ("orderby".to_string(), "create_date".to_string()),
            ("desc".to_string(), "false".to_string()),
            ("dsl".to_string(), "False".to_string()),
            ("keywords".to_string(), "rag".to_string()),
        ]);
        let query = SessionQuery::parse(&params).unwrap();
        assert_eq!(query.page, 3);
        assert_eq!(query.page_size, 50);
        assert_eq!(query.orderby, "create_date");
        assert!(!query.desc);
        assert!(!query.include_dsl);
        assert_eq!(query.keywords, "rag");

        let oversize = HashMap::from([("page_size".to_string(), "101".to_string())]);
        assert_eq!(
            SessionQuery::parse(&oversize).unwrap_err(),
            "page_size must be less than or equal to 100"
        );
    }

    #[test]
    fn local_datetimes_parse_back_as_ragflow_wall_clock() {
        let rendered = format_local_datetime(1_700_086_400_000);
        assert_eq!(rendered.len(), 19);
        assert!(
            chrono::NaiveDateTime::parse_from_str(&rendered, "%Y-%m-%d %H:%M:%S").is_ok(),
            "{rendered}"
        );
        // The rendering is the server-local wall clock, i.e. the UTC instant
        // shifted by the process offset.
        let offset = chrono::Local::now().offset().local_minus_utc();
        let expected = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_086_400, 0)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        assert_eq!(rendered, expected);
        let _ = offset;
    }

    #[test]
    fn normalize_query_date_widens_bare_days_and_iso_values() {
        assert_eq!(
            normalize_query_date("2026-07-18", false),
            "2026-07-18 00:00:00"
        );
        assert_eq!(
            normalize_query_date("2026-07-18", true),
            "2026-07-18 23:59:59"
        );
        assert_eq!(
            normalize_query_date("2026-07-18 10:00:00", false),
            "2026-07-18 10:00:00"
        );
        let iso = normalize_query_date("2026-07-18T10:00:00+00:00", false);
        assert!(iso.starts_with("2026-07-18 "), "{iso}");
    }

    #[test]
    fn filters_keywords_dates_and_orders_like_get_list() {
        let sessions = vec![
            session("a", "alice", 1_700_000_000_000, 1_700_000_100_000),
            session("b", "bob", 1_700_086_400_000, 1_700_086_500_000),
        ];
        let identity = |_: &str| None;
        let mut query = SessionQuery::default();
        query.orderby = "create_time".into();
        query.desc = false;
        let (total, page) = select_sessions(sessions.clone(), &query);
        assert_eq!(total, 2);
        assert_eq!(page[0].id, "a");
        assert_eq!(page[1].id, "b");

        query.desc = true;
        let (_, page) = select_sessions(sessions.clone(), &query);
        assert_eq!(page[0].id, "b");

        query.keywords = "RAGFLOW".into();
        let (total, _) = select_sessions(sessions.clone(), &query);
        assert_eq!(total, 2);
        query.keywords = "missing".into();
        let (total, _) = select_sessions(sessions.clone(), &query);
        assert_eq!(total, 0);

        // The range is expressed in the same server-local clock the rows use,
        // so the assertion holds in every process timezone.
        let exact = format_local_datetime(1_700_086_400_000);
        let mut dated = SessionQuery::default();
        dated.orderby = "create_date".into();
        dated.from_date = exact.clone();
        dated.to_date = exact;
        let (total, page) = select_sessions(sessions.clone(), &dated);
        assert_eq!(total, 1);
        assert_eq!(page[0].id, "b");

        let mut by_user = SessionQuery::default();
        by_user.user_id = "alice".into();
        let (total, _) = select_sessions(sessions.clone(), &by_user);
        assert_eq!(total, 1);

        let mut paged = SessionQuery::default();
        paged.page = 2;
        paged.page_size = 1;
        let (total, page) = select_sessions(sessions, &paged);
        assert_eq!(total, 2);
        assert_eq!(page.len(), 1);

        let row = normalize_agent_session(&page[0], &paged, &identity);
        assert_eq!(row["agent_id"], "canvas-1");
        assert!(row.get("dsl").is_some());
        assert_eq!(row["round"], 1);
        assert_eq!(row["thumb_up"], 1);
        assert_eq!(
            row["message"][1]["reference"][0]["content"],
            "retrieval augmented generation"
        );
        assert_eq!(row["message"][1]["reference"][0]["dataset_id"], "kb-1");
        assert!(row["create_date"].as_str().unwrap().len() == 19);

        paged.include_dsl = false;
        let row = normalize_agent_session(&page[0], &paged, &identity);
        assert!(row.get("dsl").is_none());
    }
}
