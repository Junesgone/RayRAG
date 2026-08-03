//! Tenant-scoped provider instances and model capability bindings.
//!
//! Mirrors RAGFlow's tenant_model_provider / tenant_model_instance / tenant_model
//! split while keeping the local JSON store compact and atomic.

use crate::api::features::ProviderStore;
use crate::embed::{SharedEmbedder, openai_compatible_embedder};
use crate::llm::{LlmClient, LlmConfig};
use crate::persistence::atomic_write;
use crate::rerank::{RemoteReranker, Reranker};
use crate::server::{AppState, AuthContext};
use anyhow::Context;
use axum::{
    Json,
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::{
    collections::{HashMap, HashSet},
    path::{Path as FsPath, PathBuf},
    sync::{Mutex, RwLock},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelCapability {
    #[serde(rename = "chat")]
    Chat,
    #[serde(rename = "embedding")]
    Embedding,
    #[serde(rename = "rerank")]
    Rerank,
    #[serde(rename = "image2text", alias = "image_to_text")]
    ImageToText,
    #[serde(rename = "speech2text", alias = "speech_to_text")]
    SpeechToText,
    #[serde(rename = "tts", alias = "text_to_speech")]
    TextToSpeech,
}

impl ModelCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Embedding => "embedding",
            Self::Rerank => "rerank",
            Self::ImageToText => "image2text",
            Self::SpeechToText => "speech2text",
            Self::TextToSpeech => "tts",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantModelSpec {
    pub name: String,
    pub model_types: Vec<ModelCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantModelInstance {
    pub tenant_id: String,
    pub provider_id: String,
    pub instance_id: String,
    pub instance_name: String,
    pub api_base: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    pub models: Vec<TenantModelSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicTenantModelInstance {
    pub tenant_id: String,
    pub provider_id: String,
    pub instance_id: String,
    pub instance_name: String,
    pub api_base: String,
    pub api_key_configured: bool,
    pub models: Vec<TenantModelSpec>,
}

impl From<&TenantModelInstance> for PublicTenantModelInstance {
    fn from(instance: &TenantModelInstance) -> Self {
        Self {
            tenant_id: instance.tenant_id.clone(),
            provider_id: instance.provider_id.clone(),
            instance_id: instance.instance_id.clone(),
            instance_name: instance.instance_name.clone(),
            api_base: instance.api_base.clone(),
            api_key_configured: instance.api_key.as_ref().is_some_and(|key| !key.is_empty()),
            models: instance.models.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct TenantModelInstanceUpdate {
    pub tenant_id: Option<String>,
    pub instance_name: String,
    #[serde(default)]
    pub api_base: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::model_meta::deserialize_optional_api_key"
    )]
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_api_key: bool,
    pub models: Vec<TenantModelSpec>,
}

pub struct TenantModelStore {
    instances: RwLock<Vec<TenantModelInstance>>,
    /// Explicit tenant-scoped default chat selectors. Missing entries mean None.
    default_chat_models: RwLock<HashMap<String, String>>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TenantModelSnapshot {
    instances: Vec<TenantModelInstance>,
    #[serde(default)]
    default_chat_models: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTenantModel {
    pub tenant_id: String,
    pub provider_id: String,
    pub provider_name: String,
    pub instance_id: String,
    pub model_name: String,
    pub api_base: String,
    pub api_key: Option<String>,
    pub max_tokens: Option<u64>,
    pub capability: ModelCapability,
}

impl ResolvedTenantModel {
    pub fn id(&self) -> String {
        format!(
            "{}/{}/{}",
            self.provider_id, self.instance_id, self.model_name
        )
    }

    pub fn llm_client(&self) -> LlmClient {
        let decoded_api_key = decode_tenant_api_key(self.api_key.as_deref());
        let api_key = crate::model_meta::normalize_provider_api_key(
            &self.provider_id,
            &self.provider_name,
            decoded_api_key.as_deref(),
        )
        .unwrap_or_default();
        let api_base = crate::model_meta::normalize_inference_base(
            &self.provider_id,
            &self.provider_name,
            &self.api_base,
        );
        LlmClient::new(LlmConfig {
            api_base,
            api_key,
            model: self.model_name.clone(),
            generation: self
                .max_tokens
                .and_then(|value| u32::try_from(value).ok())
                .map(|value| {
                    crate::generation_params::GenerationParams::default().with_max_tokens(value)
                })
                .unwrap_or_default(),
            ..LlmConfig::default()
        })
    }

    pub fn embedder(&self) -> SharedEmbedder {
        let decoded_api_key = decode_tenant_api_key(self.api_key.as_deref());
        let api_key = crate::model_meta::normalize_provider_api_key(
            &self.provider_id,
            &self.provider_name,
            decoded_api_key.as_deref(),
        )
        .unwrap_or_default();
        let api_base = crate::model_meta::normalize_inference_base(
            &self.provider_id,
            &self.provider_name,
            &self.api_base,
        );
        Arc::new(openai_compatible_embedder(
            &api_base,
            &api_key,
            &self.model_name,
        ))
    }

    pub fn reranker(&self) -> Arc<dyn Reranker> {
        let decoded_api_key = decode_tenant_api_key(self.api_key.as_deref());
        let api_key = crate::model_meta::normalize_provider_api_key(
            &self.provider_id,
            &self.provider_name,
            decoded_api_key.as_deref(),
        );
        let api_base = crate::model_meta::normalize_inference_base(
            &self.provider_id,
            &self.provider_name,
            &self.api_base,
        );
        let reranker = RemoteReranker::new(&api_base);
        let reranker = match api_key.as_deref().filter(|key| !key.is_empty()) {
            Some(key) => reranker.with_api_key(key),
            None => reranker,
        };
        Arc::new(reranker)
    }
}

impl TenantModelStore {
    pub fn new(path: impl AsRef<FsPath>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        crate::persistence::restore_if_missing(&path)?;
        let (instances, default_chat_models) = if path.exists() {
            let data = std::fs::read(&path)?;
            if data
                .iter()
                .copied()
                .find(|byte| !byte.is_ascii_whitespace())
                == Some(b'[')
            {
                // v0.1 array snapshots remain valid and intentionally have no selector.
                (serde_json::from_slice(&data)?, HashMap::new())
            } else {
                let snapshot: TenantModelSnapshot = serde_json::from_slice(&data)?;
                (snapshot.instances, snapshot.default_chat_models)
            }
        } else {
            (Vec::new(), HashMap::new())
        };
        validate_instance_set(&instances)?;
        validate_default_chat_models(&instances, &default_chat_models)?;
        let store = Self {
            instances: RwLock::new(instances),
            default_chat_models: RwLock::new(default_chat_models),
            path: Some(path),
            save_lock: Mutex::new(()),
        };
        store.persist_current()?;
        Ok(store)
    }

    pub fn in_memory() -> Self {
        Self {
            instances: RwLock::new(Vec::new()),
            default_chat_models: RwLock::new(HashMap::new()),
            path: None,
            save_lock: Mutex::new(()),
        }
    }

    pub fn list(&self, tenant_id: &str) -> Vec<PublicTenantModelInstance> {
        let mut instances: Vec<_> = self
            .instances
            .read()
            .unwrap()
            .iter()
            .filter(|instance| instance.tenant_id == tenant_id)
            .map(PublicTenantModelInstance::from)
            .collect();
        instances.sort_by(|left, right| {
            left.provider_id
                .cmp(&right.provider_id)
                .then_with(|| left.instance_id.cmp(&right.instance_id))
        });
        instances
    }

    pub fn default_chat_model(&self, tenant_id: &str) -> Option<String> {
        self.default_chat_models
            .read()
            .unwrap()
            .get(tenant_id)
            .cloned()
    }

    pub fn set_default_chat_model(
        &self,
        providers: &ProviderStore,
        tenant_id: &str,
        selector: Option<&str>,
    ) -> anyhow::Result<Option<String>> {
        let normalized = selector.map(str::trim).filter(|value| !value.is_empty());
        if let Some(selector) = normalized {
            self.resolve(providers, tenant_id, ModelCapability::Chat, Some(selector))?
                .ok_or_else(|| anyhow::anyhow!("Chat model is not configured: {selector}"))?;
        }
        self.mutate_with_defaults(|instances, defaults| {
            if let Some(selector) = normalized {
                defaults.insert(tenant_id.to_string(), selector.to_string());
            } else {
                defaults.remove(tenant_id);
            }
            validate_default_chat_models(instances, defaults)?;
            Ok(normalized.map(str::to_string))
        })
    }

    pub fn list_configured(&self, tenant_id: &str) -> Vec<TenantModelInstance> {
        self.instances
            .read()
            .unwrap()
            .iter()
            .filter(|instance| instance.tenant_id == tenant_id)
            .cloned()
            .collect()
    }

    pub fn resolve(
        &self,
        providers: &ProviderStore,
        tenant_id: &str,
        capability: ModelCapability,
        selector: Option<&str>,
    ) -> anyhow::Result<Option<ResolvedTenantModel>> {
        let selector = selector.map(str::trim).filter(|value| !value.is_empty());
        let providers = providers.list_configured();
        let instances = self.list_configured(tenant_id);
        if let Some(composite) = selector.and_then(split_composite_model_selector) {
            let provider = providers
                .iter()
                .filter(|provider| provider.name == composite.provider_name)
                // A user-created enabled provider wins over a disabled
                // catalog seed that shares the display name.
                .max_by_key(|provider| provider.enabled)
                .with_context(|| {
                    format!(
                        "Provider {} not found for model {}",
                        composite.provider_name,
                        selector.expect("composite selectors are present")
                    )
                })?;
            if !provider.enabled {
                anyhow::bail!("Provider is disabled");
            }
            let provider_instances: Vec<_> = instances
                .iter()
                .filter(|instance| instance.provider_id == provider.id)
                .collect();
            let instance = provider_instances
                .iter()
                .copied()
                .find(|instance| instance.instance_name == composite.instance_name)
                .or_else(|| {
                    (composite.instance_name == "default" && provider_instances.len() == 1)
                        .then(|| provider_instances[0])
                })
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Instance {} not found for model {}",
                        composite.instance_name,
                        selector.expect("composite selectors are present")
                    )
                })?;
            let model = instance
                .models
                .iter()
                .find(|model| {
                    model.name == composite.model_name
                        && model.enabled
                        && model.model_types.contains(&capability)
                })
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Requested {} model is not configured for tenant: {}",
                        capability.as_str(),
                        selector.expect("composite selectors are present")
                    )
                })?;
            return Ok(Some(resolved_tenant_model(
                tenant_id, provider, instance, model, capability,
            )));
        }
        let mut candidates = Vec::new();
        for instance in instances {
            for model in instance
                .models
                .iter()
                .filter(|model| model.enabled && model.model_types.contains(&capability))
            {
                let id = format!(
                    "{}/{}/{}",
                    instance.provider_id, instance.instance_id, model.name
                );
                if selector.is_some_and(|selector| selector != id && selector != model.name) {
                    continue;
                }
                candidates.push((id, instance.clone(), model.clone()));
            }
        }
        candidates.sort_by(|left, right| left.0.cmp(&right.0));
        if let Some(selector) = selector {
            let exact: Vec<_> = candidates
                .iter()
                .filter(|candidate| candidate.0 == selector)
                .cloned()
                .collect();
            if !exact.is_empty() {
                candidates = exact;
            } else if candidates.len() > 1 {
                anyhow::bail!(
                    "Requested {} model is ambiguous for tenant: {}",
                    capability.as_str(),
                    selector
                );
            }
        }
        let Some((_, instance, model)) = candidates.into_iter().next() else {
            if let Some(selector) = selector {
                anyhow::bail!(
                    "Requested {} model is not configured for tenant: {}",
                    capability.as_str(),
                    selector
                );
            }
            return Ok(None);
        };
        let provider = providers
            .into_iter()
            .find(|provider| provider.id == instance.provider_id)
            .context("Provider not found")?;
        if !provider.enabled {
            anyhow::bail!("Provider is disabled");
        }
        Ok(Some(resolved_tenant_model(
            tenant_id, &provider, &instance, &model, capability,
        )))
    }

    pub fn provider_in_use(&self, provider_id: &str) -> bool {
        self.instances
            .read()
            .unwrap()
            .iter()
            .any(|instance| instance.provider_id == provider_id)
    }

    pub fn validate_providers(&self, providers: &ProviderStore) -> anyhow::Result<()> {
        let provider_ids: HashSet<_> = providers
            .list_configured()
            .into_iter()
            .map(|provider| provider.id)
            .collect();
        for instance in self.instances.read().unwrap().iter() {
            if !provider_ids.contains(&instance.provider_id) {
                anyhow::bail!(
                    "Tenant model instance references missing provider: {}",
                    instance.provider_id
                );
            }
        }
        Ok(())
    }

    pub fn upsert(
        &self,
        providers: &ProviderStore,
        tenant_id: &str,
        provider_id: &str,
        instance_id: &str,
        update: TenantModelInstanceUpdate,
    ) -> anyhow::Result<PublicTenantModelInstance> {
        validate_identifier(instance_id, "Instance id")?;
        let provider = providers
            .list_configured()
            .into_iter()
            .find(|provider| provider.id == provider_id)
            .context("Provider not found")?;
        if !provider.enabled {
            anyhow::bail!("Provider is disabled");
        }
        self.mutate(|instances| {
            let current = instances.iter().position(|instance| {
                instance.tenant_id == tenant_id
                    && instance.provider_id == provider_id
                    && instance.instance_id == instance_id
            });
            let current_key = current.and_then(|index| instances[index].api_key.clone());
            let api_key = if update.clear_api_key {
                None
            } else {
                update
                    .api_key
                    .filter(|key| !key.trim().is_empty())
                    .or(current_key)
            };
            let api_base = update
                .api_base
                .as_deref()
                .filter(|base| !base.trim().is_empty())
                .unwrap_or(&provider.api_base)
                .trim()
                .trim_end_matches('/')
                .to_string();
            let instance = TenantModelInstance {
                tenant_id: tenant_id.to_string(),
                provider_id: provider_id.to_string(),
                instance_id: instance_id.to_string(),
                instance_name: update.instance_name.trim().to_string(),
                api_base,
                api_key,
                models: update.models,
            };
            validate_instance(&instance)?;
            let public = PublicTenantModelInstance::from(&instance);
            if let Some(index) = current {
                instances[index] = instance;
            } else {
                instances.push(instance);
            }
            Ok(public)
        })
    }

    pub fn delete(
        &self,
        tenant_id: &str,
        provider_id: &str,
        instance_id: &str,
    ) -> anyhow::Result<bool> {
        self.mutate(|instances| {
            let previous_len = instances.len();
            instances.retain(|instance| {
                instance.tenant_id != tenant_id
                    || instance.provider_id != provider_id
                    || instance.instance_id != instance_id
            });
            Ok(previous_len != instances.len())
        })
    }

    /// Diff-sync the capability types of one model inside an instance.
    /// Mirrors RAGFlow `TenantModelService.upsert_model_type`
    /// (tenant_model_service.py): for each type in `add`, insert it when the
    /// model does not have it yet; for each type in `delete`, remove it when
    /// present. RAGFlow keeps a UNSUPPORTED tombstone record per deleted
    /// type; RayRAG models carry a per-spec `enabled` flag, so a removed
    /// capability is simply no longer resolvable — equivalent semantics.
    ///
    /// Returns the number of type operations applied (add + delete).
    pub fn sync_model_types(
        &self,
        tenant_id: &str,
        provider_id: &str,
        instance_id: &str,
        model_name: &str,
        add: &[ModelCapability],
        delete: &[ModelCapability],
    ) -> anyhow::Result<usize> {
        let mut operated = 0usize;
        self.mutate(|instances| {
            let Some(instance) = instances.iter_mut().find(|instance| {
                instance.tenant_id == tenant_id
                    && instance.provider_id == provider_id
                    && instance.instance_id == instance_id
            }) else {
                anyhow::bail!("Cannot find tenant model instance {instance_id}.");
            };
            let Some(model) = instance
                .models
                .iter_mut()
                .find(|model| model.name == model_name)
            else {
                anyhow::bail!(
                    "Cannot find model {model_name} on instance {instance_id}."
                );
            };
            for capability in add {
                if !model.model_types.contains(capability) {
                    model.model_types.push(*capability);
                    model.enabled = true;
                }
                operated += 1;
            }
            for capability in delete {
                if let Some(index) = model
                    .model_types
                    .iter()
                    .position(|existing| existing == capability)
                {
                    model.model_types.remove(index);
                }
                operated += 1;
            }
            Ok(())
        })?;
        Ok(operated)
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut Vec<TenantModelInstance>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.mutate_with_defaults(|instances, _| mutation(instances))
    }

    fn mutate_with_defaults<T>(
        &self,
        mutation: impl FnOnce(
            &mut Vec<TenantModelInstance>,
            &mut HashMap<String, String>,
        ) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut instances = self.instances.write().unwrap();
        let mut defaults = self.default_chat_models.write().unwrap();
        let previous = instances.clone();
        let previous_defaults = defaults.clone();
        let result = mutation(&mut instances, &mut defaults).and_then(|value| {
            validate_instance_set(&instances)?;
            validate_default_chat_models(&instances, &defaults)?;
            self.persist(&instances, &defaults)?;
            Ok(value)
        });
        if result.is_err() {
            *instances = previous;
            *defaults = previous_defaults;
        }
        result
    }

    fn persist_current(&self) -> anyhow::Result<()> {
        let _save_guard = self.save_lock.lock().unwrap();
        self.persist(
            &self.instances.read().unwrap(),
            &self.default_chat_models.read().unwrap(),
        )
    }

    fn persist(
        &self,
        instances: &[TenantModelInstance],
        defaults: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let snapshot = TenantModelSnapshot {
            instances: instances.to_vec(),
            default_chat_models: defaults.clone(),
        };
        atomic_write(path, &serde_json::to_vec_pretty(&snapshot)?)?;
        restrict_permissions(path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompositeModelSelector<'a> {
    model_name: &'a str,
    instance_name: &'a str,
    provider_name: &'a str,
}

/// Parse RAGFlow's `{model}@{provider}` and
/// `{model}@{instance}@{provider}` identifiers from the right. Model names may
/// themselves contain `@`, so a left-to-right split would silently select the
/// wrong credential scope.
fn split_composite_model_selector(selector: &str) -> Option<CompositeModelSelector<'_>> {
    let mut parts = selector.rsplitn(3, '@');
    let provider_name = parts.next()?;
    let second = parts.next()?;
    let third = parts.next();
    let (model_name, instance_name) = match third {
        Some(model_name) => (model_name, second),
        None => (second, "default"),
    };
    (!model_name.is_empty() && !instance_name.is_empty() && !provider_name.is_empty()).then_some(
        CompositeModelSelector {
            model_name,
            instance_name,
            provider_name,
        },
    )
}

fn resolved_tenant_model(
    tenant_id: &str,
    provider: &crate::api::features::Provider,
    instance: &TenantModelInstance,
    model: &TenantModelSpec,
    capability: ModelCapability,
) -> ResolvedTenantModel {
    ResolvedTenantModel {
        tenant_id: tenant_id.to_string(),
        provider_id: instance.provider_id.clone(),
        provider_name: provider.name.clone(),
        instance_id: instance.instance_id.clone(),
        model_name: model.name.clone(),
        api_base: instance.api_base.clone(),
        api_key: instance.api_key.clone(),
        max_tokens: model.max_tokens,
        capability,
    }
}

/// RAGFlow provider instances can persist either a plain key or a JSON key
/// envelope. Decode only the documented string field and fail closed for a
/// malformed envelope instead of forwarding serialized credentials as a
/// bearer token.
fn decode_tenant_api_key(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    let Ok(serde_json::Value::Object(values)) = serde_json::from_str::<serde_json::Value>(raw)
    else {
        return Some(raw.to_owned());
    };
    match values.get("api_key") {
        Some(serde_json::Value::String(api_key)) => Some(api_key.clone()),
        Some(_) => Some(String::new()),
        None if values
            .keys()
            .all(|key| matches!(key.as_str(), "api_key" | "is_tools")) =>
        {
            Some(String::new())
        }
        None => Some(raw.to_owned()),
    }
}

fn validate_instance_set(instances: &[TenantModelInstance]) -> anyhow::Result<()> {
    let mut keys = HashSet::new();
    for instance in instances {
        validate_instance(instance)?;
        if !keys.insert((
            instance.tenant_id.as_str(),
            instance.provider_id.as_str(),
            instance.instance_id.as_str(),
        )) {
            anyhow::bail!("Duplicate tenant provider instance");
        }
    }
    Ok(())
}

fn validate_default_chat_models(
    instances: &[TenantModelInstance],
    defaults: &HashMap<String, String>,
) -> anyhow::Result<()> {
    for (tenant_id, selector) in defaults {
        if tenant_id.trim().is_empty() || selector.trim().is_empty() {
            anyhow::bail!("Tenant default chat selector must not be empty");
        }
        let matches = instances
            .iter()
            .flat_map(|instance| {
                instance.models.iter().filter(move |model| {
                    instance.tenant_id == *tenant_id
                        && model.enabled
                        && model.model_types.contains(&ModelCapability::Chat)
                        && (selector == &model.name
                            || selector
                                == &format!(
                                    "{}/{}/{}",
                                    instance.provider_id, instance.instance_id, model.name
                                ))
                })
            })
            .count();
        if matches != 1 {
            anyhow::bail!(
                "Tenant default chat selector is not a unique enabled chat model: {selector}"
            );
        }
    }
    Ok(())
}

fn validate_instance(instance: &TenantModelInstance) -> anyhow::Result<()> {
    if instance.tenant_id.trim().is_empty() {
        anyhow::bail!("Tenant id is required");
    }
    validate_identifier(&instance.provider_id, "Provider id")?;
    validate_identifier(&instance.instance_id, "Instance id")?;
    if instance.instance_name.is_empty() {
        anyhow::bail!("Instance name is required");
    }
    let url = reqwest::Url::parse(&instance.api_base)?;
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!("Instance API base must use http or https");
    }
    if instance.models.is_empty() {
        anyhow::bail!("At least one tenant model is required");
    }
    let mut names = HashSet::new();
    for model in &instance.models {
        if model.name.trim().is_empty() {
            anyhow::bail!("Model name is required");
        }
        if !names.insert(model.name.as_str()) {
            anyhow::bail!("Tenant model names must be unique per instance");
        }
        if model.model_types.is_empty() {
            anyhow::bail!("At least one model type is required");
        }
        let mut types = HashSet::new();
        if model
            .model_types
            .iter()
            .any(|model_type| !types.insert(model_type))
        {
            anyhow::bail!("Model types must be unique");
        }
        if model.max_tokens == Some(0) {
            anyhow::bail!("Model max_tokens must be greater than zero");
        }
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        anyhow::bail!("{label} must contain letters, digits, hyphens, or underscores");
    }
    Ok(())
}

fn enabled_by_default() -> bool {
    true
}

#[cfg(unix)]
fn restrict_permissions(path: &FsPath) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &FsPath) -> anyhow::Result<()> {
    Ok(())
}

#[derive(Debug, Default, Deserialize)]
pub struct TenantModelQuery {
    pub tenant_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DefaultChatModelUpdate {
    pub tenant_id: Option<String>,
    pub selector: Option<String>,
}

fn selected_tenant<'a>(auth: &'a AuthContext, tenant_id: Option<&'a str>) -> &'a str {
    tenant_id.unwrap_or(&auth.user_id)
}

fn can_read_tenant(state: &AppState, auth: &AuthContext, tenant_id: &str) -> bool {
    state.tenants.is_member(tenant_id, &auth.user_id)
}

fn forbidden(message: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "code": 403, "message": message })),
    )
        .into_response()
}

fn tenant_model_mutation_error(error: anyhow::Error) -> Response {
    let message = error.to_string();
    let (status, code) = if message.starts_with("Tenant default chat selector") {
        (StatusCode::CONFLICT, 409)
    } else {
        (StatusCode::BAD_REQUEST, 400)
    };
    (
        status,
        Json(serde_json::json!({ "code": code, "message": message })),
    )
        .into_response()
}

pub async fn get_default_chat_model(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<TenantModelQuery>,
) -> Response {
    let tenant_id = selected_tenant(&auth, query.tenant_id.as_deref());
    if !can_read_tenant(&state, &auth, tenant_id) {
        return forbidden("Tenant membership required");
    }
    Json(serde_json::json!({ "code": 0, "data": { "selector": state.tenant_models.default_chat_model(tenant_id) } })).into_response()
}

pub async fn set_default_chat_model(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(update): Json<DefaultChatModelUpdate>,
) -> Response {
    let tenant_id = selected_tenant(&auth, update.tenant_id.as_deref()).to_string();
    if !state.tenants.can_manage(&tenant_id, &auth.user_id) {
        return forbidden("Tenant administrator access required");
    }
    match state.tenant_models.set_default_chat_model(
        &state.providers,
        &tenant_id,
        update.selector.as_deref(),
    ) {
        Ok(selector) => {
            Json(serde_json::json!({ "code": 0, "data": { "selector": selector } })).into_response()
        }
        Err(error) => tenant_model_mutation_error(error),
    }
}

pub async fn list_tenant_models(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<TenantModelQuery>,
) -> Response {
    let tenant_id = selected_tenant(&auth, query.tenant_id.as_deref());
    if !can_read_tenant(&state, &auth, tenant_id) {
        return forbidden("Tenant membership required");
    }
    Json(serde_json::json!({ "code": 0, "data": state.tenant_models.list(tenant_id) }))
        .into_response()
}

pub async fn upsert_tenant_model_instance(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((provider_id, instance_id)): Path<(String, String)>,
    Json(update): Json<TenantModelInstanceUpdate>,
) -> Response {
    let tenant_id = selected_tenant(&auth, update.tenant_id.as_deref()).to_string();
    if !state.tenants.can_manage(&tenant_id, &auth.user_id) {
        return forbidden("Tenant administrator access required");
    }
    match state.tenant_models.upsert(
        &state.providers,
        &tenant_id,
        &provider_id,
        &instance_id,
        update,
    ) {
        Ok(instance) => Json(serde_json::json!({ "code": 0, "data": instance })).into_response(),
        Err(error) => tenant_model_mutation_error(error),
    }
}

pub async fn delete_tenant_model_instance(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((provider_id, instance_id)): Path<(String, String)>,
    Query(query): Query<TenantModelQuery>,
) -> Response {
    let tenant_id = selected_tenant(&auth, query.tenant_id.as_deref());
    if !state.tenants.can_manage(tenant_id, &auth.user_id) {
        return forbidden("Tenant administrator access required");
    }
    match state
        .tenant_models
        .delete(tenant_id, &provider_id, &instance_id)
    {
        Ok(true) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Model instance not found" })),
        )
            .into_response(),
        Err(error) => tenant_model_mutation_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::features::ProviderUpdate;

    #[test]
    fn sync_model_types_adds_removes_and_counts_ops() {
        let store = TenantModelStore::in_memory();
        store
            .upsert(
                &providers(),
                "tenant-a",
                "local",
                "inst-1",
                update(Some("secret-key")),
            )
            .unwrap();
        // Add a missing capability, keep an existing one, remove another.
        let operated = store
            .sync_model_types(
                "tenant-a",
                "local",
                "inst-1",
                "model-a",
                &[ModelCapability::Rerank, ModelCapability::Chat],
                &[ModelCapability::Embedding],
            )
            .unwrap();
        assert_eq!(operated, 3);
        let model = &store.list("tenant-a")[0].models[0];
        assert!(model.model_types.contains(&ModelCapability::Rerank));
        assert!(model.model_types.contains(&ModelCapability::Chat));
        assert!(!model.model_types.contains(&ModelCapability::Embedding));
        assert!(model.enabled);
    }

    #[test]
    fn sync_model_types_is_idempotent_for_repeated_add() {
        let store = TenantModelStore::in_memory();
        store
            .upsert(
                &providers(),
                "tenant-a",
                "local",
                "inst-1",
                update(Some("secret-key")),
            )
            .unwrap();
        store
            .sync_model_types(
                "tenant-a",
                "local",
                "inst-1",
                "model-a",
                &[ModelCapability::Rerank],
                &[],
            )
            .unwrap();
        store
            .sync_model_types(
                "tenant-a",
                "local",
                "inst-1",
                "model-a",
                &[ModelCapability::Rerank],
                &[],
            )
            .unwrap();
        let model = &store.list("tenant-a")[0].models[0];
        let rerank_count = model
            .model_types
            .iter()
            .filter(|t| **t == ModelCapability::Rerank)
            .count();
        assert_eq!(rerank_count, 1, "no duplicate capability entries");
    }

    #[test]
    fn sync_model_types_errors_on_unknown_instance_or_model() {
        let store = TenantModelStore::in_memory();
        store
            .upsert(
                &providers(),
                "tenant-a",
                "local",
                "inst-1",
                update(Some("secret-key")),
            )
            .unwrap();
        assert!(store
            .sync_model_types("tenant-a", "local", "nope", "model-a", &[], &[])
            .is_err());
        assert!(store
            .sync_model_types("tenant-a", "local", "inst-1", "missing", &[], &[])
            .is_err());
    }

    fn update(api_key: Option<&str>) -> TenantModelInstanceUpdate {
        TenantModelInstanceUpdate {
            tenant_id: None,
            instance_name: "Primary".into(),
            api_base: None,
            api_key: api_key.map(str::to_string),
            clear_api_key: false,
            models: vec![TenantModelSpec {
                name: "model-a".into(),
                model_types: vec![ModelCapability::Chat, ModelCapability::Embedding],
                max_tokens: Some(8192),
                enabled: true,
            }],
        }
    }

    fn providers() -> ProviderStore {
        let providers = ProviderStore::in_memory();
        providers
            .create(
                "local",
                ProviderUpdate {
                    name: "Local".into(),
                    api_base: "http://127.0.0.1:8080/v1".into(),
                    models: vec!["model-a".into()],
                    enabled: true,
                    api_key: None,
                    clear_api_key: false,
                },
            )
            .unwrap();
        providers
    }

    #[test]
    fn tenant_models_survive_restart_without_exposing_secret() {
        let root =
            std::env::temp_dir().join(format!("rayrag-tenant-models-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tenant_models.json");
        let store = TenantModelStore::new(&path).unwrap();
        let public = store
            .upsert(
                &providers(),
                "tenant-a",
                "local",
                "primary",
                update(Some("secret")),
            )
            .unwrap();
        assert!(public.api_key_configured);
        assert!(
            serde_json::to_value(&public)
                .unwrap()
                .get("api_key")
                .is_none()
        );
        drop(store);

        let restored = TenantModelStore::new(&path).unwrap();
        assert_eq!(
            restored.list_configured("tenant-a")[0].api_key.as_deref(),
            Some("secret")
        );
        assert_eq!(restored.list("tenant-a")[0].models[0].model_types.len(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn failed_tenant_model_persistence_rolls_back_memory() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-tenant-models-rollback-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tenant_models.json");
        let store = TenantModelStore::new(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(
            store
                .upsert(&providers(), "tenant-a", "local", "primary", update(None))
                .is_err()
        );
        assert!(store.list("tenant-a").is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn tenant_model_update_preserves_or_clears_secret_explicitly() {
        let store = TenantModelStore::in_memory();
        let providers = providers();
        store
            .upsert(
                &providers,
                "tenant-a",
                "local",
                "primary",
                update(Some("secret")),
            )
            .unwrap();

        store
            .upsert(&providers, "tenant-a", "local", "primary", update(Some("")))
            .unwrap();
        assert_eq!(
            store.list_configured("tenant-a")[0].api_key.as_deref(),
            Some("secret")
        );

        let mut clear = update(None);
        clear.clear_api_key = true;
        store
            .upsert(&providers, "tenant-a", "local", "primary", clear)
            .unwrap();
        assert_eq!(store.list_configured("tenant-a")[0].api_key, None);
    }

    #[test]
    fn default_chat_selector_migrates_clears_survives_restart_and_blocks_model_removal() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-default-chat-model-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tenant_models.json");
        let provider_store = providers();

        // Legacy array snapshots migrate to the object envelope without inventing a default.
        std::fs::write(&path, "[]").unwrap();
        let store = TenantModelStore::new(&path).unwrap();
        assert_eq!(store.default_chat_model("tenant-a"), None);
        assert!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap())
                .unwrap()
                .is_object()
        );

        store
            .upsert(
                &provider_store,
                "tenant-a",
                "local",
                "primary",
                update(None),
            )
            .unwrap();
        let selector = "local/primary/model-a";
        assert_eq!(
            store
                .set_default_chat_model(&provider_store, "tenant-a", Some(selector))
                .unwrap()
                .as_deref(),
            Some(selector)
        );

        let mut disabled = update(None);
        disabled.models[0].enabled = false;
        let error = store
            .upsert(&provider_store, "tenant-a", "local", "primary", disabled)
            .unwrap_err();
        assert!(error.to_string().contains("Tenant default chat selector"));
        assert!(store.list_configured("tenant-a")[0].models[0].enabled);

        let error = store.delete("tenant-a", "local", "primary").unwrap_err();
        assert!(error.to_string().contains("Tenant default chat selector"));
        assert_eq!(store.list_configured("tenant-a").len(), 1);
        drop(store);

        let restored = TenantModelStore::new(&path).unwrap();
        assert_eq!(
            restored.default_chat_model("tenant-a").as_deref(),
            Some(selector)
        );
        assert_eq!(
            restored
                .set_default_chat_model(&provider_store, "tenant-a", None)
                .unwrap(),
            None
        );
        assert!(restored.delete("tenant-a", "local", "primary").unwrap());
        drop(restored);
        let restored = TenantModelStore::new(&path).unwrap();
        assert_eq!(restored.default_chat_model("tenant-a"), None);
        assert!(restored.list_configured("tenant-a").is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn composite_model_selectors_are_right_anchored_and_default_to_the_sole_instance() {
        let provider_store = ProviderStore::in_memory();
        provider_store
            .create(
                "siliconflow-test",
                ProviderUpdate {
                    name: "SILICONFLOW".into(),
                    api_base: "https://api.siliconflow.example/v1".into(),
                    models: vec!["text-embedding@nomic".into(), "chat-model".into()],
                    enabled: true,
                    api_key: None,
                    clear_api_key: false,
                },
            )
            .unwrap();
        let store = TenantModelStore::in_memory();
        store
            .upsert(
                &provider_store,
                "tenant-a",
                "siliconflow-test",
                "instance-east",
                TenantModelInstanceUpdate {
                    tenant_id: None,
                    instance_name: "prod-east".into(),
                    api_base: Some("https://tenant.example/v1".into()),
                    api_key: Some("tenant-secret".into()),
                    clear_api_key: false,
                    models: vec![
                        TenantModelSpec {
                            name: "text-embedding@nomic".into(),
                            model_types: vec![ModelCapability::Chat],
                            max_tokens: Some(4096),
                            enabled: true,
                        },
                        TenantModelSpec {
                            name: "chat-model".into(),
                            model_types: vec![ModelCapability::Chat],
                            max_tokens: Some(4096),
                            enabled: true,
                        },
                    ],
                },
            )
            .unwrap();

        let explicit = store
            .resolve(
                &provider_store,
                "tenant-a",
                ModelCapability::Chat,
                Some("text-embedding@nomic@prod-east@SILICONFLOW"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(explicit.model_name, "text-embedding@nomic");
        assert_eq!(explicit.instance_id, "instance-east");

        let legacy_default = store
            .resolve(
                &provider_store,
                "tenant-a",
                ModelCapability::Chat,
                Some("chat-model@SILICONFLOW"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(legacy_default.instance_id, "instance-east");

        store
            .upsert(
                &provider_store,
                "tenant-a",
                "siliconflow-test",
                "instance-west",
                TenantModelInstanceUpdate {
                    tenant_id: None,
                    instance_name: "prod-west".into(),
                    api_base: Some("https://tenant-west.example/v1".into()),
                    api_key: Some("tenant-west-secret".into()),
                    clear_api_key: false,
                    models: vec![
                        TenantModelSpec {
                            name: "text-embedding@nomic".into(),
                            model_types: vec![ModelCapability::Chat],
                            max_tokens: Some(4096),
                            enabled: true,
                        },
                        TenantModelSpec {
                            name: "chat-model".into(),
                            model_types: vec![ModelCapability::Chat],
                            max_tokens: Some(4096),
                            enabled: true,
                        },
                    ],
                },
            )
            .unwrap();
        let error = store
            .resolve(
                &provider_store,
                "tenant-a",
                ModelCapability::Chat,
                Some("chat-model@SILICONFLOW"),
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Instance default not found for model")
        );
    }

    #[test]
    fn tenant_api_key_envelopes_are_decoded_without_accepting_non_string_secrets() {
        assert_eq!(
            decode_tenant_api_key(Some(r#"{"api_key":"secret","is_tools":true}"#)).as_deref(),
            Some("secret")
        );
        assert_eq!(
            decode_tenant_api_key(Some(r#"{"api_key":7,"is_tools":true}"#)).as_deref(),
            Some("")
        );
        assert_eq!(
            decode_tenant_api_key(Some(r#"{"is_tools":true}"#)).as_deref(),
            Some("")
        );
        assert_eq!(
            decode_tenant_api_key(Some("plain-secret")).as_deref(),
            Some("plain-secret")
        );
        assert_eq!(decode_tenant_api_key(None), None);
    }
}
