//! Compilation templates aligned with RAGFlow v0.27.2.
//!
//! User-created groups are tenant-scoped and persistent. The builtin palette and
//! wiki presets are immutable data compiled from the fixed v0.27.2 YAML files
//! (api/db/init_data/compilation_templates/, commit a024bea0cd93).

use axum::{
    Json,
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

use crate::server::{AppState, AuthContext};

const NAME_MAX_BYTES: usize = 128;
const DESCRIPTION_MAX_CHARS: usize = 1024;
const GLOBAL_RULES_MAX_CHARS: usize = 4096;
const BUILTIN_DATA: &str = include_str!("fixtures/compilation_template_builtins.v0.27.2.json");
const WIKI_PRESET_DATA: &str =
    include_str!("fixtures/compilation_template_wiki_presets.v0.27.2.json");
type ReferenceData = (Arc<[BuiltinCompilationTemplate]>, Arc<[WikiPreset]>);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BuiltinCompilationTemplate {
    pub id: String,
    pub kind: String,
    pub display_name: String,
    pub description: String,
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WikiPreset {
    pub id: String,
    pub topic: String,
    pub instruction: String,
    pub example: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompilationTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub config: serde_json::Value,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompilationTemplateGroup {
    pub id: String,
    pub tenant_id: String,
    pub name: String,
    pub description: String,
    pub scope: String,
    pub templates: Vec<CompilationTemplate>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TemplateRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub kind: String,
    pub config: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct GroupCreateRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub templates: Vec<TemplateRequest>,
}

#[derive(Debug, Deserialize, Default)]
pub struct GroupUpdateRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub templates: Option<Vec<TemplateRequest>>,
}

#[derive(Debug, Deserialize, Default)]
pub struct GroupListQuery {
    #[serde(default)]
    pub keywords: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub page: usize,
    #[serde(default)]
    pub page_size: usize,
    #[serde(default = "default_orderby")]
    pub orderby: String,
    #[serde(default = "default_desc")]
    pub desc: bool,
}

#[derive(Debug, Deserialize, Default)]
pub struct BuiltinListQuery {
    pub tenant_id: Option<String>,
}

fn default_orderby() -> String {
    "create_time".into()
}

fn default_desc() -> bool {
    true
}

pub struct CompilationTemplateStore {
    groups: RwLock<Vec<CompilationTemplateGroup>>,
    builtins: Arc<[BuiltinCompilationTemplate]>,
    wiki_presets: Arc<[WikiPreset]>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
}

impl CompilationTemplateStore {
    pub fn new(path: impl AsRef<FsPath>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        crate::persistence::restore_if_missing(&path)?;
        let mut groups: Vec<CompilationTemplateGroup> = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            Vec::new()
        };
        normalize_legacy_kinds(&mut groups);
        validate_saved_groups(&groups)?;
        let (builtins, wiki_presets) = load_reference_data()?;
        let store = Self {
            groups: RwLock::new(groups),
            builtins,
            wiki_presets,
            path: Some(path),
            save_lock: Mutex::new(()),
        };
        store.persist_current()?;
        Ok(store)
    }

    pub fn in_memory() -> Self {
        Self::try_in_memory().expect("embedded v0.27.2 compilation template data is valid")
    }

    fn try_in_memory() -> anyhow::Result<Self> {
        let (builtins, wiki_presets) = load_reference_data()?;
        Ok(Self {
            groups: RwLock::new(Vec::new()),
            builtins,
            wiki_presets,
            path: None,
            save_lock: Mutex::new(()),
        })
    }

    pub fn list_builtins_for(
        &self,
        tenant_id: &str,
        tenant_models: &crate::api::tenant_models::TenantModelStore,
        providers: &crate::api::features::ProviderStore,
    ) -> Vec<BuiltinCompilationTemplate> {
        let selector = tenant_models.default_chat_model(tenant_id);
        self.builtins
            .iter()
            .cloned()
            .map(|mut builtin| {
                if let Some(selector) = selector.as_deref()
                    && builtin.config.get("llm_id").is_none()
                    && tenant_models
                        .resolve(
                            providers,
                            tenant_id,
                            crate::api::tenant_models::ModelCapability::Chat,
                            Some(selector),
                        )
                        .ok()
                        .flatten()
                        .is_some()
                    && let Some(config) = builtin.config.as_object_mut()
                {
                    config.insert("llm_id".into(), serde_json::Value::String(selector.into()));
                }
                builtin
            })
            .collect()
    }

    pub fn list_builtins(&self) -> Vec<BuiltinCompilationTemplate> {
        self.builtins.to_vec()
    }

    pub fn list_wiki_presets(&self) -> Vec<WikiPreset> {
        self.wiki_presets.to_vec()
    }

    pub fn create(
        &self,
        tenant_id: &str,
        mut request: GroupCreateRequest,
    ) -> anyhow::Result<CompilationTemplateGroup> {
        normalize_legacy_requests(&mut request.templates);
        let name = request.name.trim().to_string();
        validate_group_fields(&name, &request.description, &request.templates)?;
        let scope = derive_scope(&request.templates)?;
        let now = now_ms();
        let group_id = uuid::Uuid::new_v4().to_string();
        let templates = materialize_templates(&group_id, request.templates, now);
        let group = CompilationTemplateGroup {
            id: group_id,
            tenant_id: tenant_id.into(),
            name,
            description: request.description,
            scope: scope.into(),
            templates,
            created_at: now,
            updated_at: now,
        };
        self.mutate(|groups| {
            ensure_unique_name(groups, tenant_id, &group.name, None)?;
            groups.push(group.clone());
            Ok(group)
        })
    }

    pub fn list(
        &self,
        tenant_id: &str,
        query: &GroupListQuery,
    ) -> (Vec<CompilationTemplateGroup>, usize) {
        let keywords = query.keywords.to_lowercase();
        let mut groups: Vec<_> = self
            .groups
            .read()
            .unwrap()
            .iter()
            .filter(|group| group.tenant_id == tenant_id)
            .filter(|group| keywords.is_empty() || group.name.to_lowercase().contains(&keywords))
            .filter(|group| query.scope.is_empty() || group.scope == query.scope)
            .cloned()
            .collect();
        match query.orderby.as_str() {
            "name" => groups.sort_by(|left, right| left.name.cmp(&right.name)),
            "update_time" | "updated_at" => groups.sort_by_key(|group| group.updated_at),
            _ => groups.sort_by_key(|group| group.created_at),
        }
        if query.desc {
            groups.reverse();
        }
        let total = groups.len();
        if query.page > 0 && query.page_size > 0 {
            let start = (query.page - 1).saturating_mul(query.page_size);
            groups = groups
                .into_iter()
                .skip(start)
                .take(query.page_size.min(1000))
                .collect();
        }
        (groups, total)
    }

    pub fn get(&self, tenant_id: &str, id: &str) -> Option<CompilationTemplateGroup> {
        self.groups
            .read()
            .unwrap()
            .iter()
            .find(|group| group.id == id && group.tenant_id == tenant_id)
            .cloned()
    }

    pub fn update(
        &self,
        tenant_id: &str,
        id: &str,
        request: GroupUpdateRequest,
    ) -> anyhow::Result<Option<CompilationTemplateGroup>> {
        self.mutate(|groups| {
            let Some(index) = groups
                .iter()
                .position(|group| group.id == id && group.tenant_id == tenant_id)
            else {
                return Ok(None);
            };
            let current = groups[index].clone();
            let name = request
                .name
                .as_deref()
                .map(str::trim)
                .unwrap_or(&current.name)
                .to_string();
            let description = request.description.unwrap_or(current.description);
            let now = now_ms();
            let (templates, scope) = match request.templates {
                Some(mut templates) => {
                    normalize_legacy_requests(&mut templates);
                    validate_group_fields(&name, &description, &templates)?;
                    let scope = derive_scope(&templates)?.to_string();
                    (materialize_templates(id, templates, now), scope)
                }
                None => {
                    validate_name(&name)?;
                    validate_description(&description)?;
                    (current.templates, current.scope)
                }
            };
            ensure_unique_name(groups, tenant_id, &name, Some(id))?;
            let updated = CompilationTemplateGroup {
                id: current.id,
                tenant_id: current.tenant_id,
                name,
                description,
                scope,
                templates,
                created_at: current.created_at,
                updated_at: now,
            };
            groups[index] = updated.clone();
            Ok(Some(updated))
        })
    }

    pub fn delete(&self, tenant_id: &str, id: &str) -> anyhow::Result<bool> {
        self.mutate(|groups| {
            let before = groups.len();
            groups.retain(|group| !(group.id == id && group.tenant_id == tenant_id));
            Ok(groups.len() != before)
        })
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut Vec<CompilationTemplateGroup>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut groups = self.groups.write().unwrap();
        let previous = groups.clone();
        let result = mutation(&mut groups)?;
        validate_saved_groups(&groups)?;
        if let Err(error) = self.persist(&groups) {
            *groups = previous;
            return Err(error);
        }
        Ok(result)
    }

    fn persist_current(&self) -> anyhow::Result<()> {
        let _save_guard = self.save_lock.lock().unwrap();
        self.persist(&self.groups.read().unwrap())
    }

    fn persist(&self, groups: &[CompilationTemplateGroup]) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        crate::persistence::atomic_write(path, &serde_json::to_vec_pretty(groups)?)
    }
}

impl Default for CompilationTemplateStore {
    fn default() -> Self {
        Self::in_memory()
    }
}

fn load_reference_data() -> anyhow::Result<ReferenceData> {
    let builtins: Vec<BuiltinCompilationTemplate> = serde_json::from_str(BUILTIN_DATA)?;
    let wiki_presets: Vec<WikiPreset> = serde_json::from_str(WIKI_PRESET_DATA)?;
    validate_reference_data(&builtins, &wiki_presets)?;
    Ok((builtins.into(), wiki_presets.into()))
}

fn validate_reference_data(
    builtins: &[BuiltinCompilationTemplate],
    wiki_presets: &[WikiPreset],
) -> anyhow::Result<()> {
    let mut ids = HashSet::new();
    for builtin in builtins {
        if builtin.id.trim().is_empty()
            || builtin.kind.trim().is_empty()
            || builtin.display_name.trim().is_empty()
            || !builtin.config.is_object()
            || !ids.insert(builtin.id.as_str())
        {
            anyhow::bail!("Invalid or duplicated builtin compilation template");
        }
    }
    // Upstream _sort_builtins: the (unseeded) empty kind would sort first and
    // everything else by display name; the embedded palette must already be in
    // that order so the editor kind list is byte-for-byte the upstream one.
    let mut ordered = builtins.to_vec();
    ordered.sort_by(|left, right| {
        let left_empty = left.kind == "empty" || left.id == "empty";
        let right_empty = right.kind == "empty" || right.id == "empty";
        left_empty
            .cmp(&right_empty)
            .then_with(|| left.display_name.cmp(&right.display_name))
    });
    if ordered
        .iter()
        .map(|template| template.id.as_str())
        .ne(builtins.iter().map(|template| template.id.as_str()))
    {
        anyhow::bail!("Builtin compilation templates must follow the upstream sort order");
    }
    let mut preset_ids = HashSet::new();
    for preset in wiki_presets {
        if preset.id.trim().is_empty() || !preset_ids.insert(preset.id.as_str()) {
            anyhow::bail!("Invalid or duplicated wiki preset");
        }
    }
    Ok(())
}

/// v0.26.4 spelled the graph-based-wiki template kind "artifacts"; v0.27.2
/// renamed it "wiki" (web/src/constants/compilation.ts). Stored pre-upgrade
/// groups are migrated in place so the v0.27.2 validator and editor see the
/// canonical vocabulary.
fn normalize_legacy_kinds(groups: &mut [CompilationTemplateGroup]) {
    for group in groups.iter_mut() {
        for template in group.templates.iter_mut() {
            if template.kind != "artifacts" {
                continue;
            }
            template.kind = "wiki".into();
            if let Some(config) = template.config.as_object_mut() {
                if config.get("kind").and_then(|value| value.as_str()) == Some("artifacts") {
                    config.insert("kind".into(), serde_json::Value::String("wiki".into()));
                }
                if !config.contains_key("mode") {
                    config.insert("mode".into(), serde_json::Value::String("entity".into()));
                }
            }
        }
    }
}

/// Write-path twin of [`normalize_legacy_kinds`]: a v0.26.4 client (cached
/// editor bundle) still posts kind `artifacts`; accept it as the v0.27.2
/// `wiki` template with its default `entity` mode instead of storing a kind
/// the palette no longer offers.
fn normalize_legacy_requests(templates: &mut [TemplateRequest]) {
    for template in templates.iter_mut() {
        let legacy = template.kind == "artifacts";
        if legacy {
            template.kind = "wiki".into();
        }
        if let Some(config) = template.config.as_object_mut() {
            if config.get("kind").and_then(|value| value.as_str()) == Some("artifacts") {
                config.insert("kind".into(), serde_json::Value::String("wiki".into()));
            }
            if template.kind == "wiki" && !config.contains_key("mode") {
                config.insert("mode".into(), serde_json::Value::String("entity".into()));
            }
        }
    }
}

fn materialize_templates(
    _group_id: &str,
    templates: Vec<TemplateRequest>,
    now: u64,
) -> Vec<CompilationTemplate> {
    templates
        .into_iter()
        .map(|template| CompilationTemplate {
            id: uuid::Uuid::new_v4().to_string(),
            name: template.name.trim().into(),
            description: template.description,
            kind: template.kind.trim().into(),
            config: template.config,
            created_at: now,
            updated_at: now,
        })
        .collect()
}

fn validate_group_fields(
    name: &str,
    description: &str,
    templates: &[TemplateRequest],
) -> anyhow::Result<()> {
    validate_name(name)?;
    validate_description(description)?;
    if templates.is_empty() {
        anyhow::bail!("A template group must contain at least one template.");
    }
    for template in templates {
        validate_template(template)?;
    }
    Ok(())
}

fn validate_name(name: &str) -> anyhow::Result<()> {
    if name.trim().is_empty() {
        anyhow::bail!("Invalid template group name.");
    }
    if name.len() > NAME_MAX_BYTES {
        anyhow::bail!("Template group name is too long.");
    }
    Ok(())
}

fn validate_description(description: &str) -> anyhow::Result<()> {
    if description.chars().count() > DESCRIPTION_MAX_CHARS {
        anyhow::bail!("Invalid template group description.");
    }
    Ok(())
}

fn validate_template(template: &TemplateRequest) -> anyhow::Result<()> {
    let name = template.name.trim();
    if name.is_empty() || name.len() > NAME_MAX_BYTES {
        anyhow::bail!("Invalid template name.");
    }
    if template.description.chars().count() > DESCRIPTION_MAX_CHARS {
        anyhow::bail!("Invalid template description.");
    }
    if template.kind.trim().is_empty() {
        anyhow::bail!("Invalid template kind.");
    }
    let config = template
        .config
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Invalid template config."))?;
    if config
        .get("global_rules")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .chars()
        .count()
        > GLOBAL_RULES_MAX_CHARS
    {
        anyhow::bail!("Global compilation rules is too long.");
    }
    for section in ["entity", "relation"] {
        let fields = config
            .get(section)
            .and_then(|value| value.get("fields"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut seen = HashSet::new();
        for field in fields {
            let field_type = field
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim();
            if field_type.is_empty() {
                anyhow::bail!("{} type is required.", capitalize(section));
            }
            if !seen.insert(field_type.to_string()) {
                anyhow::bail!("{} type can not be duplicated.", capitalize(section));
            }
            let description = field
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if description.trim().is_empty() {
                anyhow::bail!("{} field description is required.", capitalize(section));
            }
            if description.chars().count() > DESCRIPTION_MAX_CHARS {
                anyhow::bail!("{} field description is too long.", capitalize(section));
            }
            if field
                .get("rule")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .chars()
                .count()
                > DESCRIPTION_MAX_CHARS
            {
                anyhow::bail!("{} field rule is too long.", capitalize(section));
            }
        }
    }
    if template.kind.trim() == "wiki" || config.get("kind").and_then(|v| v.as_str()) == Some("wiki")
    {
        // Upstream validate_template_payload: wiki mode first, then the claim
        // and concept field sets of the graph-based wiki recipe.
        let mode = config
            .get("mode")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if mode != "entity" && mode != "topic" {
            anyhow::bail!("Wiki mode must be either 'entity' or 'topic'.");
        }
        validate_wiki_fields(
            config,
            "claim",
            &[
                ("statement", "Claim statement"),
                ("subject", "Claim subject"),
            ],
        )?;
        validate_wiki_fields(
            config,
            "concept",
            &[
                ("term", "Concept term"),
                ("definition_excerpt", "Concept definition excerpt"),
            ],
        )?;
    }
    Ok(())
}

/// Wiki recipe fields (upstream validate_template_payload): the label is part of
/// the upstream message verbatim, e.g. "Claim statement is required.".
fn validate_wiki_fields(
    config: &serde_json::Map<String, serde_json::Value>,
    section: &str,
    keys: &[(&str, &str)],
) -> anyhow::Result<()> {
    let fields = config
        .get(section)
        .and_then(|value| value.get("fields"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    for field in fields {
        for (key, label) in keys {
            let value = field
                .get(*key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if value.trim().is_empty() {
                anyhow::bail!("{label} is required.");
            }
            if value.chars().count() > DESCRIPTION_MAX_CHARS {
                anyhow::bail!("{label} is too long.");
            }
        }
    }
    Ok(())
}

fn derive_scope(templates: &[TemplateRequest]) -> anyhow::Result<&'static str> {
    if templates.is_empty() {
        anyhow::bail!("A template group must contain at least one template.");
    }
    let wiki_templates = templates
        .iter()
        .filter(|template| template.kind.trim() == "wiki")
        .count();
    if wiki_templates > 0 {
        if wiki_templates != 1 || templates.len() != 1 {
            anyhow::bail!(
                "A wiki template cannot be combined with other templates in the same group."
            );
        }
        return Ok("dataset");
    }
    let rechunk_trees = templates
        .iter()
        .filter(|template| template.kind.trim() == "tree")
        .filter(|template| {
            template
                .config
                .get("raptor")
                .and_then(|value| value.get("rechunk"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        })
        .count();
    if rechunk_trees > 1 {
        anyhow::bail!("Only one tree template in a group may enable re-chunking.");
    }
    Ok("file")
}

fn ensure_unique_name(
    groups: &[CompilationTemplateGroup],
    tenant_id: &str,
    name: &str,
    exclude_id: Option<&str>,
) -> anyhow::Result<()> {
    if groups.iter().any(|group| {
        group.tenant_id == tenant_id && group.name == name && Some(group.id.as_str()) != exclude_id
    }) {
        anyhow::bail!("Duplicated compilation template group name.");
    }
    Ok(())
}

fn validate_saved_groups(groups: &[CompilationTemplateGroup]) -> anyhow::Result<()> {
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    let mut template_ids = HashSet::new();
    for group in groups {
        if group.id.is_empty() || group.tenant_id.is_empty() || !ids.insert(group.id.clone()) {
            anyhow::bail!("Invalid or duplicated compilation template group id");
        }
        validate_name(&group.name)?;
        validate_description(&group.description)?;
        if !names.insert((group.tenant_id.clone(), group.name.clone())) {
            anyhow::bail!("Duplicated compilation template group name.");
        }
        if group.scope != "file" && group.scope != "dataset" {
            anyhow::bail!("Invalid compilation template group scope");
        }
        if group.templates.is_empty() {
            anyhow::bail!("A template group must contain at least one template.");
        }
        for template in &group.templates {
            if template.id.is_empty() || !template_ids.insert(template.id.clone()) {
                anyhow::bail!("Invalid or duplicated compilation template id");
            }
        }
    }
    Ok(())
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn json_result(code: i64, message: &str, data: serde_json::Value) -> Response {
    (
        StatusCode::OK,
        Json(serde_json::json!({ "code": code, "message": message, "data": data })),
    )
        .into_response()
}

/// Upstream `get_data_error_result(...)` in the compilation-template blueprint is a
/// plain jsonify answer: HTTP 200, code 102, message only, no data key.
fn data_error(message: &str) -> Response {
    (
        StatusCode::OK,
        Json(serde_json::json!({ "code": 102, "message": message })),
    )
        .into_response()
}

/// Upstream `validate_request("name", "templates")`: a missing key answers
/// `get_json_result(code=101, message=..., data=null)` on HTTP 200.
fn missing_arguments_message(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    let missing: Vec<&str> = keys
        .iter()
        .copied()
        .filter(|key| value.get(*key).is_none())
        .collect();
    if missing.is_empty() {
        None
    } else {
        Some(format!(
            "required argument are missing: {}; ",
            missing.join(",")
        ))
    }
}

/// `_validate_group_payload` scalar half: name/description types and lengths.
fn group_scalars_from_value(
    value: &serde_json::Value,
) -> Result<(Option<String>, Option<String>), String> {
    let name = match value.get("name") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(raw)) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err("Invalid template group name.".into());
            }
            if trimmed.len() > 128 {
                return Err("Template group name is too long.".into());
            }
            Some(trimmed.to_string())
        }
        Some(_) => return Err("Invalid template group name.".into()),
    };
    let description = match value.get("description") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(raw)) => {
            if raw.chars().count() > 1024 {
                return Err("Invalid template group description.".into());
            }
            Some(raw.clone())
        }
        Some(_) => return Err("Invalid template group description.".into()),
    };
    Ok((name, description))
}

/// `_validate_group_payload` template half plus the child required-field check
/// from `validate_template_payload(require_all=True)`.
fn templates_from_value(value: &serde_json::Value) -> Result<Option<Vec<TemplateRequest>>, String> {
    let Some(raw) = value.get("templates") else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    let Some(array) = raw.as_array() else {
        return Err("A template group must contain at least one template.".into());
    };
    if array.is_empty() {
        return Err("A template group must contain at least one template.".into());
    }
    let mut templates = Vec::with_capacity(array.len());
    for child in array {
        let Some(object) = child.as_object() else {
            return Err("Invalid template entry in group.".into());
        };
        templates.push(template_from_object(object)?);
    }
    Ok(Some(templates))
}

fn template_from_object(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Result<TemplateRequest, String> {
    for key in ["name", "kind", "config"] {
        if !object.contains_key(key) {
            return Err(format!("Missing required field: {key}."));
        }
    }
    let name = object
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if name.trim().is_empty() || name.len() > 128 {
        return Err("Invalid template name.".into());
    }
    let description = match object.get("description") {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(raw)) => {
            if raw.chars().count() > 1024 {
                return Err("Invalid template description.".into());
            }
            raw.clone()
        }
        Some(_) => return Err("Invalid template description.".into()),
    };
    let kind = object
        .get("kind")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if kind.is_empty() {
        return Err("Invalid template kind.".into());
    }
    let config = object
        .get("config")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    if !config.is_object() {
        return Err("Invalid template config.".into());
    }
    Ok(TemplateRequest {
        name: name.to_string(),
        description,
        kind: kind.to_string(),
        config,
    })
}

pub async fn list_groups(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<GroupListQuery>,
) -> Response {
    let (groups, total) = state.compilation_templates.list(&auth.user_id, &query);
    json_result(
        0,
        "success",
        serde_json::json!({ "groups": groups, "total": total }),
    )
}

pub async fn list_builtin_templates(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<BuiltinListQuery>,
) -> Response {
    let tenant_id = query.tenant_id.as_deref().unwrap_or(&auth.user_id);
    if !state.tenants.is_member(tenant_id, &auth.user_id) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "code": 403, "message": "Tenant membership required" })),
        )
            .into_response();
    }
    json_result(
        0,
        "success",
        serde_json::json!(state.compilation_templates.list_builtins_for(
            tenant_id,
            &state.tenant_models,
            &state.providers
        )),
    )
}

pub async fn list_wiki_presets(
    State(state): State<Arc<AppState>>,
    Extension(_auth): Extension<AuthContext>,
) -> Response {
    json_result(
        0,
        "success",
        serde_json::json!(state.compilation_templates.list_wiki_presets()),
    )
}

pub async fn create_group(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    request: Option<Json<serde_json::Value>>,
) -> Response {
    let Some(Json(value)) = request else {
        return json_result(
            101,
            "required argument are missing: name,templates; ",
            serde_json::Value::Null,
        );
    };
    if !value.is_object() {
        return json_result(
            101,
            "required argument are missing: name,templates; ",
            serde_json::Value::Null,
        );
    }
    if let Some(message) = missing_arguments_message(&value, &["name", "templates"]) {
        return json_result(101, &message, serde_json::Value::Null);
    }
    let (name, description) = match group_scalars_from_value(&value) {
        Ok(fields) => fields,
        Err(message) => return data_error(&message),
    };
    let Some(name) = name else {
        return data_error("Missing required field: name.");
    };
    let templates = match templates_from_value(&value) {
        Ok(Some(templates)) => templates,
        Ok(None) => {
            return data_error("A template group must contain at least one template.");
        }
        Err(message) => return data_error(&message),
    };
    let group_request = GroupCreateRequest {
        name,
        description: description.unwrap_or_default(),
        templates,
    };
    match state
        .compilation_templates
        .create(&auth.user_id, group_request)
    {
        Ok(group) => json_result(
            0,
            "success",
            serde_json::to_value(group).unwrap_or(serde_json::Value::Null),
        ),
        Err(error) => data_error(&error.to_string()),
    }
}

pub async fn get_group(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Response {
    match state.compilation_templates.get(&auth.user_id, &id) {
        Some(group) => json_result(
            0,
            "success",
            serde_json::to_value(group).unwrap_or(serde_json::Value::Null),
        ),
        None => data_error(&format!("Cannot find compilation template group {id}.")),
    }
}

/// Request body for the tree-template execution endpoint.
#[derive(Debug, Deserialize)]
pub struct ExecuteTreeTemplatesRequest {
    /// Chunks to build trees over (text + embedding).
    pub chunks: Vec<ExecuteChunkInput>,
}

#[derive(Debug, Deserialize)]
pub struct ExecuteChunkInput {
    pub content: String,
    #[serde(default)]
    pub embedding: Vec<f32>,
}

/// Optional query params: when both `doc_id` and `kb_id` are supplied, each
/// built tree is persisted as a `raptor_tree` graph checkpoint.
#[derive(Debug, Deserialize, Default)]
pub struct ExecuteTreeTemplatesQuery {
    pub doc_id: Option<String>,
    pub kb_id: Option<String>,
}

/// Run every `tree`-kind template in the group over the supplied chunks.
///
/// Mirrors RAGFlow `chunk_post_processor.run_tree_templates`: each tree
/// template builds a RAPTOR tree with the tenant chat model as summarizer.
/// Requires the server LLM client to be configured; returns 400 otherwise.
/// With `?doc_id=..&kb_id=..` the resulting tree graphs are persisted into
/// the document graph store (RAGFlow `_struct_upsert_graph_json` equivalent).
pub async fn execute_group_templates(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(query): Query<ExecuteTreeTemplatesQuery>,
    Json(request): Json<ExecuteTreeTemplatesRequest>,
) -> Response {
    let Some(group) = state.compilation_templates.get(&auth.user_id, &id) else {
        return data_error(&format!("Cannot find compilation template group {id}."));
    };
    let tree_templates: Vec<(String, serde_json::Value)> = group
        .templates
        .iter()
        .filter(|t| t.kind.eq_ignore_ascii_case("tree"))
        .map(|t| (t.id.clone(), t.config.clone()))
        .collect();
    let hypergraph_templates: Vec<(String, serde_json::Value)> = group
        .templates
        .iter()
        .filter(|t| !t.kind.eq_ignore_ascii_case("tree"))
        .map(|t| (t.id.clone(), t.config.clone()))
        .collect();
    if tree_templates.is_empty() && hypergraph_templates.is_empty() {
        return Json(serde_json::json!({
            "code": 0,
            "data": { "results": [], "templates": 0 }
        }))
        .into_response();
    }
    let Some(llm) = state.llm.as_ref() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 400,
                "message": "Server LLM client is not configured; cannot run compilation templates"
            })),
        )
            .into_response();
    };
    let chunks: Vec<(String, Vec<f32>)> = request
        .chunks
        .into_iter()
        .map(|c| (c.content, c.embedding))
        .filter(|(content, embedding)| !content.is_empty() && !embedding.is_empty())
        .collect();
    if chunks.is_empty() {
        return Json(serde_json::json!({
            "code": 400,
            "message": "No usable chunks supplied (need non-empty content + embedding)"
        }))
        .into_response();
    }
    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut persisted_entities: Vec<crate::graphrag_enhanced::NerEntity> = Vec::new();
    let mut persisted_relations: Vec<(String, String)> = Vec::new();

    // Tree-kind templates: RAPTOR build (run_tree_templates).
    if !tree_templates.is_empty() {
        let summarizer = crate::raptor::LlmClusterSummarizer::new((**llm).clone(), 8192);
        let tree_results =
            crate::raptor::run_tree_templates(&chunks, &tree_templates, &summarizer).await;
        for result in &tree_results {
            for entity in &result.graph.entities {
                persisted_entities.push(crate::graphrag_enhanced::NerEntity {
                    name: entity.name.clone(),
                    entity_type: crate::graphrag_enhanced::EntityType::Concept,
                    start: 0,
                    end: 0,
                    confidence: 1.0,
                });
            }
            for relation in &result.graph.relations {
                persisted_relations.push((relation.from.clone(), relation.to.clone()));
            }
        }
        results.extend(
            tree_results
                .into_iter()
                .map(|r| serde_json::to_value(r).unwrap_or(serde_json::Value::Null)),
        );
    }

    // Non-tree templates (hypergraph/list/set): LLM extraction + dedup merge.
    if !hypergraph_templates.is_empty() {
        let text_chunks: Vec<(String, String)> = chunks
            .iter()
            .enumerate()
            .map(|(index, (content, _))| (format!("chunk-{index}"), content.clone()))
            .collect();
        for (template_id, config) in &hypergraph_templates {
            let (mut entities, relations) =
                match crate::hypergraph::compile_hypergraph(llm, &text_chunks, config).await {
                    Ok(ok) => ok,
                    Err(error) => {
                        tracing::warn!(
                            "hypergraph-template {template_id}: extraction failed: {error}"
                        );
                        continue;
                    }
                };
            let entity_count = entities.len();
            let deduped = crate::merge::deduplicate_items(
                entities.clone(),
                llm,
                crate::merge::DEFAULT_MERGE_THRESHOLD,
            )
            .await;
            entities = match deduped {
                Ok(items) => items,
                Err(error) => {
                    tracing::warn!(
                        "hypergraph-template {template_id}: dedup failed, keeping raw: {error}"
                    );
                    entities
                }
            };
            for entity in &entities {
                if let Some(name) = entity.get("name").and_then(serde_json::Value::as_str) {
                    persisted_entities.push(crate::graphrag_enhanced::NerEntity {
                        name: name.to_string(),
                        entity_type: crate::graphrag_enhanced::EntityType::Concept,
                        start: 0,
                        end: 0,
                        confidence: 1.0,
                    });
                }
            }
            for relation in &relations {
                if let (Some(from), Some(to)) = (
                    relation.get("source").and_then(serde_json::Value::as_str),
                    relation.get("target").and_then(serde_json::Value::as_str),
                ) {
                    persisted_relations.push((from.to_string(), to.to_string()));
                }
            }
            results.push(serde_json::json!({
                "template_id": template_id,
                "entities": entities,
                "relations": relations,
                "entity_count": entity_count,
                "relation_count": relations.len(),
            }));
        }
    }

    // Persist the compiled graph as a document checkpoint when the caller
    // scopes the run to a document (RAGFlow `_struct_upsert_graph_json`).
    if let (Some(doc_id), Some(kb_id)) = (query.doc_id, query.kb_id) {
        let mut graph = crate::graphrag_enhanced::EntityGraph::new();
        graph.add_entities(&persisted_entities);
        for (from, to) in &persisted_relations {
            graph.add_relation(from, to, 1.0);
        }
        let checkpoint = crate::graph_store::GraphCheckpoint {
            doc_id: doc_id.clone(),
            kb_id: kb_id.clone(),
            content_hash: "knowledge_compile".into(),
            method: "knowledge_compile".into(),
            entity_types: vec!["tree_node".into(), "entity".into()],
            graph,
        };
        if let Err(error) = state.graphs.replace_document(&doc_id, Some(checkpoint)) {
            tracing::warn!("compile-template: failed to persist graph for {doc_id}: {error}");
        }
    }

    Json(serde_json::json!({
        "code": 0,
        "data": {
            "results": results,
            "templates": tree_templates.len() + hypergraph_templates.len()
        }
    }))
    .into_response()
}

pub async fn update_group(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    request: Option<Json<serde_json::Value>>,
) -> Response {
    let value = request
        .map(|Json(value)| value)
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    if !value.is_object() {
        return data_error("Invalid request payload: expected object.");
    }
    let (name, description) = match group_scalars_from_value(&value) {
        Ok(fields) => fields,
        Err(message) => return data_error(&message),
    };
    let templates = match templates_from_value(&value) {
        Ok(templates) => templates,
        Err(message) => return data_error(&message),
    };
    let group_request = GroupUpdateRequest {
        name,
        description,
        templates,
    };
    match state
        .compilation_templates
        .update(&auth.user_id, &id, group_request)
    {
        Ok(Some(group)) => json_result(
            0,
            "success",
            serde_json::to_value(group).unwrap_or(serde_json::Value::Null),
        ),
        Ok(None) => data_error(&format!("Cannot find compilation template group {id}.")),
        Err(error) => data_error(&error.to_string()),
    }
}

pub async fn delete_group(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Response {
    match state.compilation_templates.delete(&auth.user_id, &id) {
        Ok(true) => json_result(0, "success", serde_json::Value::Bool(true)),
        Ok(false) => data_error(&format!("Cannot find compilation template group {id}.")),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A graph-based-wiki config that passes the v0.27.2 template validator.
    fn wiki_config() -> serde_json::Value {
        serde_json::json!({
            "kind": "wiki",
            "mode": "entity",
            "claim": {"fields": [{"statement": "A is B", "subject": "A"}]},
            "concept": {"fields": [{"term": "A", "definition_excerpt": "the A"}]}
        })
    }

    fn template(name: &str, kind: &str, config: serde_json::Value) -> TemplateRequest {
        TemplateRequest {
            name: name.into(),
            description: String::new(),
            kind: kind.into(),
            config,
        }
    }

    fn request(name: &str, templates: Vec<TemplateRequest>) -> GroupCreateRequest {
        GroupCreateRequest {
            name: name.into(),
            description: String::new(),
            templates,
        }
    }

    #[test]
    fn scope_and_group_invariants_match_ragflow() {
        let store = CompilationTemplateStore::in_memory();
        let group = store
            .create(
                "tenant-a",
                request(
                    "File",
                    vec![template(
                        "Tree",
                        "tree",
                        serde_json::json!({"raptor":{"rechunk":true}}),
                    )],
                ),
            )
            .unwrap();
        assert_eq!(group.scope, "file");
        let artifact = store
            .create(
                "tenant-a",
                request("Dataset", vec![template("Wiki", "wiki", wiki_config())]),
            )
            .unwrap();
        assert_eq!(artifact.scope, "dataset");
        assert!(
            store
                .create(
                    "tenant-a",
                    request(
                        "Mixed",
                        vec![
                            template("Wiki", "wiki", wiki_config()),
                            template("Other", "graph", serde_json::json!({}))
                        ]
                    )
                )
                .is_err()
        );
        assert!(
            store
                .create(
                    "tenant-a",
                    request(
                        "Two",
                        vec![
                            template("A", "tree", serde_json::json!({"raptor":{"rechunk":true}})),
                            template("B", "tree", serde_json::json!({"raptor":{"rechunk":true}}))
                        ]
                    )
                )
                .is_err()
        );
    }

    #[test]
    fn tenant_scope_update_and_name_uniqueness_are_atomic() {
        let store = CompilationTemplateStore::in_memory();
        let first = store
            .create(
                "tenant-a",
                request("One", vec![template("A", "tree", serde_json::json!({}))]),
            )
            .unwrap();
        assert!(store.get("tenant-b", &first.id).is_none());
        store
            .create(
                "tenant-a",
                request("Two", vec![template("B", "tree", serde_json::json!({}))]),
            )
            .unwrap();
        assert!(
            store
                .update(
                    "tenant-a",
                    &first.id,
                    GroupUpdateRequest {
                        name: Some("Two".into()),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert_eq!(store.get("tenant-a", &first.id).unwrap().name, "One");
        let updated = store
            .update(
                "tenant-a",
                &first.id,
                GroupUpdateRequest {
                    templates: Some(vec![template("Artifact", "wiki", wiki_config())]),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(updated.scope, "dataset");
        assert_ne!(updated.templates[0].id, first.templates[0].id);
    }

    #[test]
    fn validation_checks_nested_schema_rules() {
        let store = CompilationTemplateStore::in_memory();
        let duplicate = serde_json::json!({"entity":{"fields":[{"type":"person","description":"a"},{"type":"person","description":"b"}]}});
        assert!(
            store
                .create(
                    "tenant",
                    request("Bad", vec![template("Bad", "tree", duplicate)])
                )
                .is_err()
        );
        let missing_claim = serde_json::json!({
            "kind": "wiki",
            "mode": "entity",
            "claim": {"fields": [{"statement": "", "subject": "fish"}]}
        });
        let error = store
            .create(
                "tenant",
                request("Bad wiki", vec![template("Bad", "wiki", missing_claim)]),
            )
            .unwrap_err();
        assert_eq!(error.to_string(), "Claim statement is required.");
    }

    #[test]
    fn persistent_store_recovers_complete_snapshot() {
        let root =
            std::env::temp_dir().join(format!("rayrag-template-groups-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("compilation_templates.json");
        let store = CompilationTemplateStore::new(&path).unwrap();
        let group = store
            .create(
                "tenant",
                request(
                    "Saved",
                    vec![template("Tree", "tree", serde_json::json!({}))],
                ),
            )
            .unwrap();
        drop(store);
        let restored = CompilationTemplateStore::new(&path).unwrap();
        assert_eq!(restored.get("tenant", &group.id).unwrap(), group);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn reference_data_is_identical_after_persistent_store_restart() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-template-reference-restart-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("compilation_templates.json");
        let first = CompilationTemplateStore::new(&path).unwrap();
        let builtins = first.list_builtins();
        let presets = first.list_wiki_presets();
        drop(first);
        let restarted = CompilationTemplateStore::new(&path).unwrap();
        assert_eq!(restarted.list_builtins(), builtins);
        assert_eq!(restarted.list_wiki_presets(), presets);
        assert_eq!(std::fs::read_to_string(path).unwrap().trim(), "[]");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn reference_palette_matches_v0272_seed_set() {
        let store = CompilationTemplateStore::in_memory();
        let builtins = store.list_builtins();
        assert_eq!(builtins.len(), 8);
        for builtin in &builtins {
            assert_eq!(builtin.id, builtin.kind);
            assert!(!builtin.display_name.is_empty());
            assert!(builtin.config.is_object());
        }
        let wiki = builtins.iter().find(|b| b.id == "wiki").unwrap();
        assert_eq!(wiki.kind, "wiki");
        assert_eq!(wiki.config["kind"], "wiki");
        assert_eq!(wiki.config["mode"], "entity");
        assert!(
            wiki.config["instruction"]
                .as_str()
                .unwrap()
                .contains("wikilinks")
        );
        assert!(wiki.config["example"].as_str().is_some());
        for section in ["entity", "relation", "claim", "concept"] {
            assert!(
                wiki.config[section]["fields"].as_array().is_some(),
                "missing wiki section {section}"
            );
        }
        let tree = builtins.iter().find(|b| b.id == "tree").unwrap();
        assert_eq!(tree.config["raptor"]["max_token"], 512);
        assert_eq!(tree.config["raptor"]["clustering_threshold"], 0.3);
        let presets = store.list_wiki_presets();
        assert_eq!(presets.len(), 6);
        assert!(
            presets
                .iter()
                .all(|preset| !preset.example.trim().is_empty())
        );
    }

    #[test]
    fn wiki_validation_messages_match_upstream() {
        let store = CompilationTemplateStore::in_memory();
        let bad_mode = serde_json::json!({"kind": "wiki", "mode": "neither"});
        let error = store
            .create(
                "tenant",
                request("Wiki", vec![template("Wiki", "wiki", bad_mode)]),
            )
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Wiki mode must be either 'entity' or 'topic'."
        );
        let missing_concept = serde_json::json!({
            "kind": "wiki",
            "mode": "topic",
            "concept": {"fields": [{"term": "", "definition_excerpt": "d"}]}
        });
        let error = store
            .create(
                "tenant",
                request("Wiki 2", vec![template("Wiki", "wiki", missing_concept)]),
            )
            .unwrap_err();
        assert_eq!(error.to_string(), "Concept term is required.");
    }

    #[test]
    fn wiki_scope_error_message_matches_upstream() {
        let store = CompilationTemplateStore::in_memory();
        let error = store
            .create(
                "tenant",
                request(
                    "Mixed",
                    vec![
                        template("Wiki", "wiki", wiki_config()),
                        template("Tree", "tree", serde_json::json!({})),
                    ],
                ),
            )
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "A wiki template cannot be combined with other templates in the same group."
        );
    }

    #[test]
    fn legacy_artifacts_kind_migrates_to_wiki_on_load() {
        let root =
            std::env::temp_dir().join(format!("rayrag-template-legacy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("compilation_templates.json");
        let legacy = serde_json::json!([{
            "id": "g1",
            "tenant_id": "tenant",
            "name": "Legacy",
            "description": "",
            "scope": "dataset",
            "templates": [{
                "id": "t1",
                "name": "Wiki",
                "description": "",
                "kind": "artifacts",
                "config": {"kind": "artifacts"},
                "created_at": 1,
                "updated_at": 1
            }],
            "created_at": 1,
            "updated_at": 1
        }]);
        std::fs::write(&path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();
        let store = CompilationTemplateStore::new(&path).unwrap();
        let group = store.get("tenant", "g1").unwrap();
        assert_eq!(group.templates[0].kind, "wiki");
        assert_eq!(group.templates[0].config["kind"], "wiki");
        assert_eq!(group.templates[0].config["mode"], "entity");
        // The migration is persisted, so a restart sees the canonical record too.
        let persisted = std::fs::read_to_string(&path).unwrap();
        assert!(persisted.contains("\"kind\": \"wiki\""));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn legacy_artifacts_write_is_normalized_to_wiki() {
        let store = CompilationTemplateStore::in_memory();
        let group = store
            .create(
                "tenant",
                request(
                    "Legacy client",
                    vec![template(
                        "Wiki",
                        "artifacts",
                        serde_json::json!({"kind": "artifacts"}),
                    )],
                ),
            )
            .unwrap();
        assert_eq!(group.scope, "dataset");
        assert_eq!(group.templates[0].kind, "wiki");
        assert_eq!(group.templates[0].config["kind"], "wiki");
        assert_eq!(group.templates[0].config["mode"], "entity");
    }

    #[test]
    fn embedded_builtin_seed_is_complete_stable_and_read_only() {
        let store = CompilationTemplateStore::in_memory();
        // Upstream _sort_builtins: kind == empty first, then display_name; the
        // v0.27.2 seed set has no empty template.
        let expected = [
            "knowledge_graph",
            "timeline",
            "mind_map",
            "page_index",
            "session_essence",
            "session_graph",
            "tree",
            "wiki",
        ];
        assert_eq!(
            store
                .list_builtins()
                .iter()
                .map(|template| template.id.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        let mut changed = store.list_builtins();
        changed[0].display_name = "changed by caller".into();
        assert_eq!(store.list_builtins()[0].display_name, "Knowledge graph");
        assert_eq!(
            store
                .list_builtins()
                .iter()
                .find(|template| template.id == "wiki")
                .map(|template| template.display_name.as_str()),
            Some("Wiki — Graph-based wiki")
        );
        assert_eq!(store.list("tenant-a", &GroupListQuery::default()).1, 0);
        assert_eq!(store.list("tenant-b", &GroupListQuery::default()).1, 0);
    }

    #[test]
    fn embedded_wiki_presets_match_fixed_filename_order_and_defaults() {
        let presets = CompilationTemplateStore::in_memory().list_wiki_presets();
        assert_eq!(
            presets
                .iter()
                .map(|preset| preset.id.as_str())
                .collect::<Vec<_>>(),
            [
                "brand",
                "engineering",
                "general",
                "market",
                "product",
                "user_interview"
            ]
        );
        assert_eq!(presets[0].topic, "marketing");
        assert!(presets[2].instruction.contains("See also"));
        assert_eq!(presets[5].topic, "product");
        assert!(presets[5].example.contains("# User interview"));
    }

    #[test]
    fn reference_data_validation_rejects_duplicate_or_misordered_seeds() {
        let store = CompilationTemplateStore::in_memory();
        let mut duplicated = store.list_builtins();
        duplicated[1].id = duplicated[0].id.clone();
        assert!(validate_reference_data(&duplicated, &store.list_wiki_presets()).is_err());
        let mut misordered = store.list_builtins();
        misordered.swap(0, 7);
        assert!(validate_reference_data(&misordered, &store.list_wiki_presets()).is_err());
    }
}
