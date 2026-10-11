//! Runtime attachments: the upload endpoint that creates them, the two that stream them back, and
//! the agent-scoped upload that stores one against a canvas.
//!
//! Upstream keeps these blobs in its object store and serves them from
//! `api/apps/restful_apis/agent_api.py::_stream_agent_attachment`, with metadata produced by
//! `FileService.upload_info`. RayRAG keeps the blob in the uploads directory and the metadata in
//! `attachments.json` (restored at boot, mirrored to PostgreSQL like every other store).
//!
//! Two behaviours are worth naming because they are easy to get wrong:
//!
//! * **`url` uploads are fetched by the server**, so the fetch is checked before it happens: only
//!   `http`/`https`, no loopback or private address, at most ten redirects, and the response is
//!   capped at the configured upload limit. Upstream validates its crawl URLs the same way; a
//!   document upload is not a proxy anyone should be able to point at their own network.
//! * **`ext`/`mime_type` only choose the response headers.** The bytes are stored once and served
//!   as they were uploaded; the query parameters exist so a client can ask for a different
//!   presentation of the same attachment, which is how upstream's `ext` parameter behaves.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use axum::Extension;
use axum::Json;
use axum::extract::{FromRequest, Multipart, Path, Query, State};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::server::{AppState, AuthContext, api_error_code, code};

/// One stored attachment.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub size: usize,
    #[serde(default)]
    pub extension: String,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub owner_id: String,
    /// The file name inside the attachments directory.
    pub storage_name: String,
    pub created_at: u64,
    /// Where it was crawled from, when it was uploaded by URL.
    #[serde(default)]
    pub source_url: Option<String>,
    /// The canvas it was uploaded for, when uploaded through the agent route.
    #[serde(default)]
    pub agent_id: Option<String>,
}

pub struct AttachmentStore {
    items: RwLock<HashMap<String, Attachment>>,
    pub(crate) data_dir: String,
    metadata_path: String,
    save_lock: Mutex<()>,
}

impl AttachmentStore {
    pub fn new(data_dir: &str) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let metadata_path = std::path::Path::new(data_dir).join("attachments.json");
        crate::persistence::restore_if_missing(&metadata_path)?;
        let items: Vec<Attachment> = if metadata_path.exists() {
            let data = std::fs::read_to_string(&metadata_path)?;
            serde_json::from_str(&data).map_err(|error| {
                anyhow::anyhow!(
                    "Failed to parse attachment metadata '{}': {error}",
                    metadata_path.display()
                )
            })?
        } else {
            Vec::new()
        };
        Ok(Self {
            items: RwLock::new(
                items
                    .into_iter()
                    .map(|item| (item.id.clone(), item))
                    .collect(),
            ),
            data_dir: data_dir.into(),
            metadata_path: metadata_path.to_string_lossy().into_owned(),
            save_lock: Mutex::new(()),
        })
    }

    fn persist(&self) -> anyhow::Result<()> {
        let _guard = self.save_lock.lock().unwrap();
        let items: Vec<Attachment> = self.items.read().unwrap().values().cloned().collect();
        crate::persistence::save_json(std::path::Path::new(&self.metadata_path), &items)
    }

    pub fn insert(&self, attachment: Attachment) -> anyhow::Result<()> {
        self.items
            .write()
            .unwrap()
            .insert(attachment.id.clone(), attachment);
        self.persist()
    }

    pub fn get(&self, id: &str) -> Option<Attachment> {
        self.items.read().unwrap().get(id).cloned()
    }

    pub fn list_for(&self, owner_id: &str, is_admin: bool) -> Vec<Attachment> {
        let mut items: Vec<Attachment> = self
            .items
            .read()
            .unwrap()
            .values()
            .filter(|item| item.owner_id == owner_id || is_admin)
            .cloned()
            .collect();
        items.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        items
    }

    /// The blob's path on disk.
    pub(crate) fn blob_path(&self, attachment: &Attachment) -> std::path::PathBuf {
        std::path::Path::new(&self.data_dir).join(&attachment.storage_name)
    }
}

/// Attachment names keep their extension — unlike documents that feed the parser, an attachment may
/// be any file type — but must stay a single, harmless path component.
pub(crate) fn safe_attachment_name(name: &str) -> anyhow::Result<String> {
    let name = name.trim().replace('\\', "/");
    let name = name.rsplit('/').next().unwrap_or("").trim();
    if name.is_empty() || name.len() > 255 || name.contains('\0') || name == "." || name == ".." {
        anyhow::bail!("Invalid filename");
    }
    Ok(name.to_string())
}

fn now_ms() -> u64 {
    crate::api::utils::datetime::now_ms()
}

/// The content type and file extension a client asked for, falling back to what was uploaded.
fn requested_content_type(
    query: &HashMap<String, String>,
    attachment: &Attachment,
) -> (String, String) {
    let extension = query
        .get("ext")
        .map(|value| value.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    // `ext` and `mime_type` are hints: a client that asks for `pdf` gets the PDF content type for
    // the stored bytes, which is what upstream's resolver does.
    let declared = query
        .get("mime_type")
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    let resolved = extension
        .as_deref()
        .and_then(|name| crate::parser::mime_from_extension(&format!("attachment.{name}")))
        .map(str::to_string)
        .or_else(|| (!attachment.mime_type.is_empty()).then(|| attachment.mime_type.clone()))
        .or_else(|| declared.clone())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let final_extension = extension.unwrap_or_else(|| {
        if attachment.extension.is_empty() {
            "bin".to_string()
        } else {
            attachment.extension.clone()
        }
    });
    (resolved, final_extension)
}

fn attachment_json(attachment: &Attachment) -> serde_json::Value {
    serde_json::json!({
        "id": attachment.id,
        "name": attachment.name,
        "size": attachment.size,
        "extension": attachment.extension,
        "mime_type": attachment.mime_type,
        "created_by": attachment.owner_id,
        "created_at": attachment.created_at,
        "preview_url": serde_json::Value::Null,
    })
}

/// Reject anything that is not a public http(s) endpoint, so a `?url=` upload cannot be pointed at
/// the host's own network.
pub(crate) fn url_is_fetchable(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw).map_err(|error| format!("Invalid url '{raw}': {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "Only http and https urls are supported, not '{}'",
            url.scheme()
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| format!("Invalid url '{raw}': no host"))?
        .to_ascii_lowercase();
    let blocked = host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| match address {
                std::net::IpAddr::V4(v4) => {
                    v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
                }
                std::net::IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
            });
    if blocked {
        return Err(format!("Refusing to fetch the internal address '{host}'"));
    }
    Ok(url)
}

/// Fetch a url into memory, following at most ten redirects and stopping at the upload limit.
async fn fetch_url(url: &str, max_bytes: usize) -> Result<(Vec<u8>, String, String), String> {
    let mut current = url_is_fetchable(url)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("Could not build a client: {error}"))?;
    for _ in 0..10 {
        let response = client
            .get(current.clone())
            .send()
            .await
            .map_err(|error| format!("Failed to fetch '{current}': {error}"))?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| format!("Redirect from '{current}' without a Location header"))?;
            let next = current
                .join(location)
                .map_err(|error| format!("Invalid redirect target '{location}': {error}"))?;
            current = url_is_fetchable(next.as_str())?;
            continue;
        }
        if !response.status().is_success() {
            return Err(format!("'{current}' answered HTTP {}", response.status()));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.split(';').next().unwrap_or(value).trim().to_string())
            .unwrap_or_default();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| format!("Failed to read '{current}': {error}"))?;
        if bytes.len() > max_bytes {
            return Err(format!(
                "'{current}' is larger than the {max_bytes} byte upload limit"
            ));
        }
        let name = current
            .path_segments()
            .and_then(|segments| segments.filter(|part| !part.is_empty()).next_back())
            .map(str::to_string)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("{}.html", current.host_str().unwrap_or("page")));
        return Ok((bytes.to_vec(), name, content_type));
    }
    Err(format!("Exceeded ten redirects fetching '{url}'"))
}

/// Store one uploaded attachment.
async fn store_attachment(
    state: &AppState,
    owner_id: &str,
    name: &str,
    bytes: &[u8],
    declared_mime: &str,
    source_url: Option<String>,
    agent_id: Option<String>,
) -> anyhow::Result<Attachment> {
    let id = uuid::Uuid::new_v4().to_string();
    let name = safe_attachment_name(name)?;
    let extension = std::path::Path::new(&name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let storage_name = if extension.is_empty() {
        id.clone()
    } else {
        format!("{id}.{extension}")
    };
    let path = std::path::Path::new(&state.attachments.data_dir).join(&storage_name);
    std::fs::write(&path, bytes)?;
    let mime_type = if !declared_mime.is_empty() {
        declared_mime.to_string()
    } else {
        crate::parser::mime_from_extension(&name)
            .unwrap_or("application/octet-stream")
            .to_string()
    };
    let attachment = Attachment {
        id,
        name,
        size: bytes.len(),
        extension,
        mime_type,
        owner_id: owner_id.to_string(),
        storage_name,
        created_at: now_ms(),
        source_url,
        agent_id,
    };
    state.attachments.insert(attachment.clone())?;
    Ok(attachment)
}

fn argument_error(message: &str) -> Response {
    api_error_code(
        axum::http::StatusCode::BAD_REQUEST,
        code::INVALID_ARGUMENT,
        message,
    )
}

fn not_found(message: &str) -> Response {
    api_error_code(
        axum::http::StatusCode::NOT_FOUND,
        code::INVALID_OR_MISSING_DATA,
        message,
    )
}

/// `POST /api/v1/documents/upload` — multipart `file`(s) or `?url=`, and not both.
///
/// The body is taken as a raw request rather than as a `Multipart` extractor: axum rejects a
/// request without a multipart content type before the handler runs, and the guide answers that
/// case with `101 Missing input: provide multipart file(s) or url` — a business code, not a 415.
pub async fn upload_document(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<HashMap<String, String>>,
    request: axum::extract::Request,
) -> Response {
    let url = query
        .get("url")
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty());
    match Multipart::from_request(request, &state).await {
        Ok(multipart) => upload_parts(&state, &auth, None, url, multipart).await,
        Err(_) => match url {
            Some(url) => upload_by_url(&state, &auth, url).await,
            None => argument_error("Missing input: provide multipart file(s) or url"),
        },
    }
}

async fn upload_by_url(state: &AppState, auth: &AuthContext, url: &str) -> Response {
    match fetch_url(url, state.max_upload_bytes).await {
        Ok((bytes, name, content_type)) => {
            match store_attachment(
                state,
                &auth.user_id,
                &name,
                &bytes,
                &content_type,
                Some(url.to_string()),
                None,
            )
            .await
            {
                Ok(attachment) => Json(serde_json::json!({
                    "code": 0,
                    "data": attachment_json(&attachment),
                    "message": "success",
                }))
                .into_response(),
                Err(error) => api_error_code(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    code::OPERATION_ERROR,
                    &error.to_string(),
                ),
            }
        }
        Err(message) => argument_error(&message),
    }
}

/// The shared body of both upload routes: a multipart `file`(s), or `?url=` for the document route.
///
/// The parts are read before anything is stored, so the "not both" case leaves no attachment behind.
pub(crate) async fn upload_parts(
    state: &AppState,
    auth: &AuthContext,
    agent_id: Option<&str>,
    url: Option<&str>,
    multipart: Multipart,
) -> Response {
    let mut files: Vec<(String, String, Vec<u8>)> = Vec::new();
    let mut multipart = multipart;
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                if field.name() != Some("file") {
                    continue;
                }
                let name = field
                    .file_name()
                    .map(str::to_string)
                    .unwrap_or_else(|| "uploaded".to_string());
                let declared = field.content_type().map(str::to_string).unwrap_or_default();
                match field.bytes().await {
                    Ok(bytes) => {
                        if bytes.len() > state.max_upload_bytes {
                            return argument_error(&format!(
                                "The uploaded file is larger than the {} byte limit",
                                state.max_upload_bytes
                            ));
                        }
                        files.push((name, declared, bytes.to_vec()));
                    }
                    Err(error) => {
                        return argument_error(&format!(
                            "Could not read the uploaded file: {error}"
                        ));
                    }
                }
            }
            Ok(None) => break,
            Err(error) => return argument_error(&format!("Malformed multipart body: {error}")),
        }
    }

    if !files.is_empty() && url.is_some() {
        return argument_error("Provide either multipart file(s) or ?url=..., not both.");
    }
    if files.is_empty() {
        return match url {
            Some(url) => upload_by_url(state, auth, url).await,
            None => argument_error("Missing input: provide multipart file(s) or url"),
        };
    }

    let mut uploads = Vec::new();
    for (name, declared, bytes) in files {
        match store_attachment(
            state,
            &auth.user_id,
            &name,
            &bytes,
            &declared,
            None,
            agent_id.map(str::to_string),
        )
        .await
        {
            Ok(attachment) => uploads.push(attachment_json(&attachment)),
            Err(error) => {
                return api_error_code(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    code::OPERATION_ERROR,
                    &error.to_string(),
                );
            }
        }
    }
    let data = if uploads.len() == 1 {
        uploads.pop().unwrap()
    } else {
        serde_json::Value::Array(uploads)
    };
    Json(serde_json::json!({ "code": 0, "data": data, "message": "success" })).into_response()
}

/// `POST /api/v1/agents/{agent_id}/upload` — an attachment for a canvas.
pub async fn upload_agent_file(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    multipart: Multipart,
) -> Response {
    if !state.agents.list().iter().any(|agent| agent.id == agent_id) {
        return not_found("canvas not found.");
    }
    let url = query
        .get("url")
        .map(String::as_str)
        .filter(|value| !value.is_empty());
    upload_parts(&state, &auth, Some(&agent_id), url, multipart).await
}

async fn stream_attachment(
    state: &AppState,
    auth: &AuthContext,
    attachment_id: &str,
    query: &HashMap<String, String>,
    default_inline: bool,
) -> Response {
    let attachment = match state.attachments.get(attachment_id) {
        Some(attachment) => attachment,
        None => return not_found("document not found"),
    };
    if attachment.owner_id != auth.user_id && !auth.is_admin {
        // The id is a UUID, but a reader must still be the owner: the attachment is the uploader's.
        return not_found("document not found");
    }
    let (content_type, extension) = requested_content_type(query, &attachment);
    let inline = default_inline
        || query
            .get("disposition")
            .is_some_and(|value| value.eq_ignore_ascii_case("inline"));
    let filename = query
        .get("filename")
        .filter(|value| !value.is_empty())
        .cloned()
        .unwrap_or_else(|| attachment.name.clone());
    let disposition_name = if inline { "inline" } else { "attachment" };
    let _ = extension;
    let path = state.attachments.blob_path(&attachment);
    if !path.exists() {
        return not_found("document not found");
    }
    let safe_filename = filename.replace(['"', '\n', '\r'], "");
    let disposition = format!("{disposition_name}; filename=\"{safe_filename}\"");
    crate::api::common::stream_stored_file(
        &path,
        &content_type,
        &disposition,
        "This attachment is empty.",
    )
    .await
}

/// `GET /api/v1/agents/attachments/{attachment_id}/download`.
pub async fn download_attachment(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(attachment_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    stream_attachment(&state, &auth, &attachment_id, &query, false).await
}

/// `GET /api/v1/agents/attachments/{attachment_id}/preview`.
pub async fn preview_attachment(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(attachment_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    stream_attachment(&state, &auth, &attachment_id, &query, true).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_internal_url_is_refused_before_anything_is_fetched() {
        for blocked in [
            "http://127.0.0.1:9390/api/v1/version",
            "http://localhost/admin",
            "http://10.0.0.5/secret",
            "http://192.168.1.10/router",
            "http://172.16.4.4/",
            "file:///etc/passwd",
            "http://nas.local/",
        ] {
            assert!(
                url_is_fetchable(blocked).is_err(),
                "{blocked} must be refused"
            );
        }
        for allowed in ["https://example.com/page", "http://example.com/a/b.html"] {
            assert!(
                url_is_fetchable(allowed).is_ok(),
                "{allowed} must be allowed"
            );
        }
    }

    #[test]
    fn the_requested_extension_chooses_the_content_type() {
        let attachment = Attachment {
            id: "a1".into(),
            name: "notes.txt".into(),
            size: 3,
            extension: "txt".into(),
            mime_type: "text/plain".into(),
            owner_id: "u".into(),
            storage_name: "a1.txt".into(),
            created_at: 1,
            source_url: None,
            agent_id: None,
        };
        let mut query = HashMap::new();
        query.insert("ext".to_string(), ".pdf".to_string());
        let (content_type, extension) = requested_content_type(&query, &attachment);
        assert_eq!(content_type, "application/pdf");
        assert_eq!(extension, "pdf");
        // Without a hint the uploaded type is used.
        let (content_type, extension) = requested_content_type(&HashMap::new(), &attachment);
        assert_eq!(content_type, "text/plain");
        assert_eq!(extension, "txt");
    }
}
