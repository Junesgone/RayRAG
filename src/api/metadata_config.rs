//! Auto-metadata configuration: the field schema a dataset defines for its documents, and the
//! per-document override.
//!
//! Upstream keeps this in `dataset_api_service.get_auto_metadata` / `update_auto_metadata` and
//! `document_api.update_metadata_config`, validated by `validation_utils.AutoMetadataConfig`:
//!
//! ```text
//! { "metadata": [ {"key": …, "type": "string"|"list"|"time"|"number",
//!                  "description": …, "enum": […] } ],
//!   "built_in_metadata": [ … same shape … ] }
//! ```
//!
//! The difference between the two lists is kept exactly as upstream has it: `metadata` is what the
//! user configured, `built_in_metadata` is what the deployment offers on top (here: the keys the
//! extraction pipeline can fill in), and a client shows both while only the first is editable.
//!
//! Validation is real: a key must be non-empty and at most 255 characters after trimming, `type` must
//! be one of the four names, and every entry of `enum` must be a string. A rejected body is a `101`
//! with the field named — silently storing a malformed schema would break every later filter.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::server::{AppState, AuthContext, api_error_code, code};

/// One auto-metadata field.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MetadataField {
    pub key: String,
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "enum")]
    pub enum_values: Option<Vec<String>>,
}

/// The configuration a dataset carries.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct AutoMetadataConfig {
    #[serde(default)]
    pub metadata: Vec<MetadataField>,
    #[serde(default)]
    pub built_in_metadata: Vec<MetadataField>,
}

/// A per-document override of the dataset's schema.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct DocumentMetadataConfig {
    #[serde(default)]
    pub metadata: Vec<MetadataField>,
    #[serde(default)]
    pub built_in_metadata: Vec<MetadataField>,
}

#[derive(Serialize, Deserialize, Default)]
struct ConfigFile {
    datasets: BTreeMap<String, AutoMetadataConfig>,
    documents: BTreeMap<String, DocumentMetadataConfig>,
}

pub struct MetadataConfigStore {
    datasets: RwLock<BTreeMap<String, AutoMetadataConfig>>,
    documents: RwLock<BTreeMap<String, DocumentMetadataConfig>>,
    metadata_path: String,
    save_lock: Mutex<()>,
}

/// The keys this deployment's extraction pipeline can fill in, offered as `built_in_metadata`.
///
/// They are named after the fields the parser already writes, so a client that shows the built-ins is
/// showing something that will really appear on a parsed document.
pub(crate) fn built_in_fields() -> Vec<MetadataField> {
    [
        ("author", "string", "Author recorded in the document"),
        ("tags", "list", "Tags extracted or assigned to the document"),
        (
            "url",
            "string",
            "Source URL, for documents fetched from the web",
        ),
        ("language", "string", "Language the document is written in"),
        (
            "published_at",
            "time",
            "Publication time, when the document states one",
        ),
    ]
    .into_iter()
    .map(|(key, field_type, description)| MetadataField {
        key: key.to_string(),
        field_type: field_type.to_string(),
        description: Some(description.to_string()),
        enum_values: None,
    })
    .collect()
}

impl MetadataConfigStore {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::persistence::restore_if_missing(std::path::Path::new(path))?;
        let file: ConfigFile = if std::path::Path::new(path).exists() {
            let data = std::fs::read_to_string(path)?;
            serde_json::from_str(&data).map_err(|error| {
                anyhow::anyhow!("Failed to parse metadata configuration '{path}': {error}")
            })?
        } else {
            ConfigFile::default()
        };
        Ok(Self {
            datasets: RwLock::new(file.datasets),
            documents: RwLock::new(file.documents),
            metadata_path: path.to_string(),
            save_lock: Mutex::new(()),
        })
    }

    pub fn in_memory() -> Self {
        Self {
            datasets: RwLock::new(BTreeMap::new()),
            documents: RwLock::new(BTreeMap::new()),
            metadata_path: String::new(),
            save_lock: Mutex::new(()),
        }
    }

    fn persist(&self) -> anyhow::Result<()> {
        if self.metadata_path.is_empty() {
            return Ok(());
        }
        let _guard = self.save_lock.lock().unwrap();
        let file = ConfigFile {
            datasets: self.datasets.read().unwrap().clone(),
            documents: self.documents.read().unwrap().clone(),
        };
        crate::persistence::save_json(std::path::Path::new(&self.metadata_path), &file)
    }

    /// The dataset's configuration with `built_in_metadata` filled in.
    pub fn get(&self, kb_id: &str) -> AutoMetadataConfig {
        let mut config = self
            .datasets
            .read()
            .unwrap()
            .get(kb_id)
            .cloned()
            .unwrap_or_default();
        config.built_in_metadata = built_in_fields();
        config
    }

    pub fn replace(&self, kb_id: &str, mut config: AutoMetadataConfig) -> anyhow::Result<()> {
        // The built-ins are the deployment's, not the client's: whatever the body says is replaced.
        config.built_in_metadata = built_in_fields();
        self.datasets
            .write()
            .unwrap()
            .insert(kb_id.to_string(), config);
        self.persist()
    }

    pub fn get_document(&self, doc_id: &str) -> Option<DocumentMetadataConfig> {
        self.documents.read().unwrap().get(doc_id).cloned()
    }

    pub fn replace_document(
        &self,
        doc_id: &str,
        config: DocumentMetadataConfig,
    ) -> anyhow::Result<()> {
        self.documents
            .write()
            .unwrap()
            .insert(doc_id.to_string(), config);
        self.persist()
    }

    /// Remove everything recorded for a dataset (its own schema and its documents').
    pub fn drop_dataset(&self, kb_id: &str, document_ids: &[String]) -> anyhow::Result<()> {
        self.datasets.write().unwrap().remove(kb_id);
        if !document_ids.is_empty() {
            let mut documents = self.documents.write().unwrap();
            for doc_id in document_ids {
                documents.remove(doc_id);
            }
        }
        self.persist()
    }
}

/// Validate one field list, returning the first problem in the guide's wording.
pub(crate) fn validate_fields(fields: &[MetadataField]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for field in fields {
        let key = field.key.trim();
        if key.is_empty() {
            return Err("Field: <key> - Message: <String should have at least 1 character>".into());
        }
        if key.chars().count() > 255 {
            return Err(format!(
                "Field: <key> - Message: <String should have at most 255 characters> - Value: <{key}>"
            ));
        }
        if !matches!(
            field.field_type.as_str(),
            "string" | "list" | "time" | "number"
        ) {
            return Err(format!(
                "Field: <type> - Message: <Input should be 'string', 'list', 'time' or 'number'> - Value: <{}>",
                field.field_type
            ));
        }
        if !seen.insert(key.to_string()) {
            return Err(format!(
                "Field: <key> - Message: <Duplicated metadata key> - Value: <{key}>"
            ));
        }
        if let Some(values) = &field.enum_values {
            if values.iter().any(|value| value.is_empty()) {
                return Err(format!(
                    "Field: <enum> - Message: <Enum values must not be empty> - Value: <{key}>"
                ));
            }
        }
    }
    Ok(())
}

/// A dataset a reader may see, or `None`.
fn visible_kb(state: &AppState, auth: &AuthContext, kb_id: &str) -> Option<()> {
    let kb = state.kbs.get(kb_id)?;
    if kb.owner_id == auth.user_id || auth.is_admin || kb.owner_id.is_empty() {
        Some(())
    } else {
        None
    }
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

fn config_json(config: &AutoMetadataConfig) -> serde_json::Value {
    serde_json::json!({
        "metadata": config.metadata,
        "built_in_metadata": config.built_in_metadata,
    })
}

/// `GET /api/v1/datasets/{dataset_id}/metadata/config`.
pub async fn get_dataset_metadata_config(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
) -> Response {
    if visible_kb(&state, &auth, &kb_id).is_none() {
        return not_found("The dataset doesn't exist");
    }
    let config = state.metadata_config.get(&kb_id);
    Json(serde_json::json!({ "code": 0, "data": config_json(&config), "message": "success" }))
        .into_response()
}

/// `PUT /api/v1/datasets/{dataset_id}/metadata/config`.
pub async fn update_dataset_metadata_config(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
    Json(body): Json<AutoMetadataConfig>,
) -> Response {
    if visible_kb(&state, &auth, &kb_id).is_none() {
        return not_found("The dataset doesn't exist");
    }
    // Only the user-configured list is validated; the built-ins are this deployment's.
    if let Err(message) = validate_fields(&body.metadata) {
        return argument_error(&message);
    }
    if let Err(error) = state.metadata_config.replace(&kb_id, body) {
        return api_error_code(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            code::OPERATION_ERROR,
            &error.to_string(),
        );
    }
    let config = state.metadata_config.get(&kb_id);
    Json(serde_json::json!({ "code": 0, "data": config_json(&config), "message": "success" }))
        .into_response()
}

/// `PUT /api/v1/datasets/{dataset_id}/documents/{document_id}/metadata/config`.
pub async fn update_document_metadata_config(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((kb_id, doc_id)): Path<(String, String)>,
    Json(body): Json<DocumentMetadataConfig>,
) -> Response {
    if visible_kb(&state, &auth, &kb_id).is_none() {
        return not_found("The dataset doesn't exist");
    }
    match state.docs.get(&doc_id) {
        Some(doc) if doc.kb_id == kb_id => {}
        _ => return not_found("The dataset does not have the document."),
    }
    if let Err(message) = validate_fields(&body.metadata) {
        return argument_error(&message);
    }
    if let Err(error) = state.metadata_config.replace_document(&doc_id, body) {
        return api_error_code(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            code::OPERATION_ERROR,
            &error.to_string(),
        );
    }
    let stored = state
        .metadata_config
        .get_document(&doc_id)
        .unwrap_or_default();
    Json(serde_json::json!({
        "code": 0,
        "data": { "metadata": stored.metadata, "built_in_metadata": stored.built_in_metadata },
        "message": "success",
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(key: &str, field_type: &str) -> MetadataField {
        MetadataField {
            key: key.into(),
            field_type: field_type.into(),
            description: None,
            enum_values: None,
        }
    }

    #[test]
    fn a_valid_schema_passes_and_every_rule_has_a_message() {
        assert!(validate_fields(&[field("author", "string"), field("tags", "list")]).is_ok());
        // An empty key.
        let error = validate_fields(&[field("   ", "string")]).unwrap_err();
        assert!(error.contains("at least 1 character"), "{error}");
        // An unknown type, named as the guide names it.
        let error = validate_fields(&[field("author", "text")]).unwrap_err();
        assert!(
            error.contains("'string', 'list', 'time' or 'number'"),
            "{error}"
        );
        // A duplicate key would make every later filter ambiguous.
        let error =
            validate_fields(&[field("author", "string"), field("author", "list")]).unwrap_err();
        assert!(error.contains("Duplicated metadata key"), "{error}");
        // A key past the documented limit.
        let long = "k".repeat(256);
        assert!(
            validate_fields(&[field(&long, "string")])
                .unwrap_err()
                .contains("255")
        );
    }

    #[test]
    fn the_built_ins_are_the_deployments_and_the_user_list_is_the_clients() {
        let store = MetadataConfigStore::in_memory();
        store
            .replace(
                "kb-1",
                AutoMetadataConfig {
                    metadata: vec![field("author", "string")],
                    // A body that tries to set the built-ins is overridden.
                    built_in_metadata: vec![field("mine", "string")],
                },
            )
            .unwrap();
        let config = store.get("kb-1");
        assert_eq!(config.metadata.len(), 1);
        assert_eq!(config.metadata[0].key, "author");
        assert!(
            config.built_in_metadata.iter().any(|f| f.key == "author"),
            "the deployment's built-ins are offered: {config:?}"
        );
        assert!(
            !config.built_in_metadata.iter().any(|f| f.key == "mine"),
            "a client cannot define the built-ins"
        );
    }

    #[test]
    fn a_document_override_is_stored_per_document() {
        let store = MetadataConfigStore::in_memory();
        store
            .replace_document(
                "doc-1",
                DocumentMetadataConfig {
                    metadata: vec![field("author", "string")],
                    built_in_metadata: Vec::new(),
                },
            )
            .unwrap();
        assert_eq!(
            store.get_document("doc-1").unwrap().metadata[0].key,
            "author"
        );
        assert!(store.get_document("doc-2").is_none());
    }
}
