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
    /// RAGFlow `Tenant` per-capability default columns beyond chat
    /// (`embd_id` / `rerank_id` / `asr_id` / `img2txt_id` / `tts_id` /
    /// `ocr_id`). Outer key = tenant id, inner key = API model-type tag
    /// (`embedding` / `rerank` / `asr` / `vision` / `tts` / `ocr`), value =
    /// the composite `{model_name}@{instance_name}@{provider_name}` selector
    /// exactly as RAGFlow persists it in the Tenant table.
    default_capability_models: RwLock<HashMap<String, HashMap<String, String>>>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TenantModelSnapshot {
    instances: Vec<TenantModelInstance>,
    #[serde(default)]
    default_chat_models: HashMap<String, String>,
    #[serde(default)]
    default_capability_models: HashMap<String, HashMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTenantModel {
    pub tenant_id: String,
    pub provider_id: String,
    pub provider_name: String,
    pub instance_id: String,
    pub instance_name: String,
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

    /// RAGFlow's browser/API selector wire. Splitters must parse it from the
    /// right because model names themselves are allowed to contain `@`.
    pub fn ragflow_selector(&self) -> String {
        format!(
            "{}@{}@{}",
            self.model_name, self.instance_name, self.provider_name
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
        let (instances, default_chat_models, default_capability_models) = if path.exists() {
            let data = std::fs::read(&path)?;
            if data
                .iter()
                .copied()
                .find(|byte| !byte.is_ascii_whitespace())
                == Some(b'[')
            {
                // v0.1 array snapshots remain valid and intentionally have no selector.
                (serde_json::from_slice(&data)?, HashMap::new(), HashMap::new())
            } else {
                let snapshot: TenantModelSnapshot = serde_json::from_slice(&data)?;
                (
                    snapshot.instances,
                    snapshot.default_chat_models,
                    snapshot.default_capability_models,
                )
            }
        } else {
            (Vec::new(), HashMap::new(), HashMap::new())
        };
        validate_instance_set(&instances)?;
        validate_default_chat_models(&instances, &default_chat_models)?;
        let store = Self {
            instances: RwLock::new(instances),
            default_chat_models: RwLock::new(default_chat_models),
            default_capability_models: RwLock::new(default_capability_models),
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
            default_capability_models: RwLock::new(HashMap::new()),
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
        self.mutate_with_defaults(|instances, defaults, _capability_defaults| {
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
                anyhow::bail!("Cannot find model {model_name} on instance {instance_id}.");
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
        self.mutate_with_defaults(|instances, _, _| mutation(instances))
    }

    fn mutate_with_defaults<T>(
        &self,
        mutation: impl FnOnce(
            &mut Vec<TenantModelInstance>,
            &mut HashMap<String, String>,
            &mut HashMap<String, HashMap<String, String>>,
        ) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut instances = self.instances.write().unwrap();
        let mut defaults = self.default_chat_models.write().unwrap();
        let mut capability_defaults = self.default_capability_models.write().unwrap();
        let previous = instances.clone();
        let previous_defaults = defaults.clone();
        let previous_capability_defaults = capability_defaults.clone();
        let result =
            mutation(&mut instances, &mut defaults, &mut capability_defaults).and_then(|value| {
                validate_instance_set(&instances)?;
                validate_default_chat_models(&instances, &defaults)?;
                self.persist(&instances, &defaults, &capability_defaults)?;
                Ok(value)
            });
        if result.is_err() {
            *instances = previous;
            *defaults = previous_defaults;
            *capability_defaults = previous_capability_defaults;
        }
        result
    }

    fn persist_current(&self) -> anyhow::Result<()> {
        let _save_guard = self.save_lock.lock().unwrap();
        self.persist(
            &self.instances.read().unwrap(),
            &self.default_chat_models.read().unwrap(),
            &self.default_capability_models.read().unwrap(),
        )
    }

    fn persist(
        &self,
        instances: &[TenantModelInstance],
        defaults: &HashMap<String, String>,
        capability_defaults: &HashMap<String, HashMap<String, String>>,
    ) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let snapshot = TenantModelSnapshot {
            instances: instances.to_vec(),
            default_chat_models: defaults.clone(),
            default_capability_models: capability_defaults.clone(),
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
        instance_name: instance.instance_name.clone(),
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
const RAGFLOW_LLM_FACTORIES_JSON: &str = r#"[{"name":"OpenAI","rank":"999","llm":[{"n":"gpt-5.5","t":["chat"],"mx":400000},{"n":"gpt-5.4","t":["chat"],"mx":400000},{"n":"gpt-5.4-mini","t":["chat"],"mx":400000},{"n":"gpt-5.4-nano","t":["chat"],"mx":400000},{"n":"gpt-5.2-pro","t":["chat"],"mx":400000},{"n":"gpt-5.2","t":["chat"],"mx":400000},{"n":"gpt-5.1","t":["chat"],"mx":400000},{"n":"gpt-5.1-chat-latest","t":["chat"],"mx":400000},{"n":"gpt-5","t":["chat"],"mx":400000},{"n":"gpt-5-mini","t":["chat"],"mx":400000},{"n":"gpt-5-nano","t":["chat"],"mx":400000},{"n":"gpt-5-chat-latest","t":["chat"],"mx":400000},{"n":"gpt-4.1","t":["chat"],"mx":1047576},{"n":"gpt-4.1-mini","t":["chat"],"mx":1047576},{"n":"gpt-4.1-nano","t":["chat"],"mx":1047576},{"n":"gpt-4.5-preview","t":["chat"],"mx":128000},{"n":"o3","t":["chat"],"mx":200000},{"n":"o4-mini","t":["chat"],"mx":200000},{"n":"o4-mini-high","t":["chat"],"mx":200000},{"n":"gpt-4o-mini","t":["chat"],"mx":128000},{"n":"gpt-4o","t":["chat"],"mx":128000},{"n":"gpt-3.5-turbo","t":["chat"],"mx":4096},{"n":"gpt-3.5-turbo-16k-0613","t":["chat"],"mx":16385},{"n":"text-embedding-ada-002","t":["embedding"],"mx":8191},{"n":"text-embedding-3-small","t":["embedding"],"mx":8191},{"n":"text-embedding-3-large","t":["embedding"],"mx":8191},{"n":"whisper-1","t":["speech2text"],"mx":26214400},{"n":"gpt-4","t":["chat"],"mx":8191},{"n":"gpt-4-turbo","t":["chat"],"mx":8191},{"n":"gpt-4-32k","t":["chat"],"mx":32768},{"n":"tts-1","t":["tts"],"mx":2048}]},{"name":"xAI","rank":"992","llm":[{"n":"grok-4","t":["chat"],"mx":256000},{"n":"grok-3","t":["chat"],"mx":131072},{"n":"grok-3-fast","t":["chat"],"mx":131072},{"n":"grok-3-mini","t":["chat"],"mx":131072},{"n":"grok-3-mini-mini-fast","t":["chat"],"mx":131072},{"n":"grok-2-vision","t":["image2text","chat"],"mx":32768}]},{"name":"TokenPony","rank":null,"llm":[{"n":"qwen3-8b","t":["chat"],"mx":128000},{"n":"deepseek-v3-0324","t":["chat"],"mx":128000},{"n":"qwen3-32b","t":["chat"],"mx":128000},{"n":"kimi-k2-instruct-0905","t":["chat"],"mx":256000},{"n":"deepseek-r1-0528","t":["chat"],"mx":164000},{"n":"qwen3-coder-480b","t":["chat"],"mx":1024000},{"n":"hunyuan-a13b-instruct","t":["chat"],"mx":256000},{"n":"qwen3-next-80b-a3b-instruct","t":["chat"],"mx":1024000},{"n":"deepseek-v3.2-exp","t":["chat"],"mx":128000},{"n":"deepseek-v3.1-terminus","t":["chat"],"mx":128000},{"n":"qwen3-vl-235b-a22b-instruct","t":["chat"],"mx":262000},{"n":"qwen3-vl-30b-a3b-instruct","t":["chat"],"mx":262000},{"n":"deepseek-ocr","t":["chat"],"mx":8000},{"n":"qwen3-235b-a22b-instruct-2507","t":["chat"],"mx":256000},{"n":"glm-4.6","t":["chat"],"mx":200000},{"n":"minimax-m2","t":["chat"],"mx":200000}]},{"name":"Tongyi-Qianwen","rank":"994","llm":[{"n":"qwen3-max-2026-01-23","t":["chat"],"mx":262144},{"n":"qwen3-tts-instruct-flash","t":["tts"],"mx":0},{"n":"kimi-k2.7-code","t":["chat","image2text"],"mx":262144},{"n":"qwen3-asr-flash-realtime-2025-10-27","t":["speech2text"],"mx":0},{"n":"qwen-mt-flash","t":["chat"],"mx":8192},{"n":"qwen3-vl-flash-2025-10-15","t":["chat","image2text"],"mx":32768},{"n":"qwen3.5-35b-a3b","t":["chat","image2text"],"mx":262144},{"n":"qwen-mt-plus","t":["chat"],"mx":16384},{"n":"qwen-mt-turbo","t":["chat"],"mx":8192},{"n":"glm-5.2","t":["chat"],"mx":1048576},{"n":"qwen3.7-max-preview","t":["chat"],"mx":1000000},{"n":"qwen3-vl-flash-2026-01-22","t":["chat","image2text"],"mx":262144},{"n":"qwen3-coder-next","t":["chat"],"mx":262144},{"n":"deepseek-v3.1","t":["chat"],"mx":163840},{"n":"qwen3-tts-instruct-flash-realtime","t":["tts"],"mx":0},{"n":"qwen3-omni-flash-2025-09-15","t":["chat","speech2text","image2text"],"mx":49152},{"n":"qvq-max","t":["chat","image2text"],"mx":128000},{"n":"qwen3-tts-vc-2026-01-22","t":["tts"],"mx":0},{"n":"deepseek-r1-distill-qwen-7b","t":["chat"],"mx":131072},{"n":"qwen3-livetranslate-flash-2025-12-01","t":["speech2text"],"mx":49152},{"n":"kimi-k2-thinking","t":["chat"],"mx":262144},{"n":"qwen-math-plus","t":["chat"],"mx":3072},{"n":"qwen-plus-2025-12-01","t":["chat"],"mx":1000000},{"n":"qwen3-tts-vd-realtime-2026-01-15","t":["tts"],"mx":0},{"n":"qwen3.6-flash-2026-04-16","t":["chat","image2text"],"mx":1000000},{"n":"qwen3-max-preview","t":["chat"],"mx":262144},{"n":"deepseek-v4-flash","t":["chat"],"mx":1048576},{"n":"qwen-math-plus-latest","t":["chat"],"mx":3072},{"n":"qvq-plus","t":["chat"],"mx":128000},{"n":"qwen3-tts-flash-2025-11-27","t":["tts"],"mx":0},{"n":"qwen3-vl-flash","t":["chat","image2text"],"mx":262144},{"n":"qwen3-tts-instruct-flash-realtime-2026-01-22","t":["tts"],"mx":0},{"n":"qwen3.7-max","t":["chat"],"mx":1000000},{"n":"qwen-math-turbo","t":["chat"],"mx":3072},{"n":"ZHIPU/GLM-5.1","t":["chat"],"mx":202752},{"n":"qwen3-tts-flash-2025-09-18","t":["tts"],"mx":0},{"n":"qwen-vl-ocr-2025-11-20","t":["ocr","image2text"],"mx":30720},{"n":"qwen3.5-397b-a17b","t":["chat","image2text"],"mx":262144},{"n":"qwen3.6-plus-2026-04-02","t":["chat","image2text"],"mx":1000000},{"n":"qwen3.6-35b-a3b","t":["chat","image2text"],"mx":262144},{"n":"kimi/kimi-k2.5","t":["chat","image2text"],"mx":262144},{"n":"qwen3-tts-flash-realtime-2025-11-27","t":["tts"],"mx":0},{"n":"qwen3-coder-plus-2025-07-22","t":["chat"],"mx":1000000},{"n":"qwen3-vl-plus-2025-12-19","t":["chat","image2text"],"mx":262144},{"n":"qwen3-omni-flash-realtime-2025-09-15","t":["chat","speech2text","image2text"],"mx":65536},{"n":"qwen3-omni-flash","t":["chat","speech2text","image2text"],"mx":65536},{"n":"qwen3-livetranslate-flash-realtime","t":["speech2text"],"mx":49152},{"n":"qwen-vl-ocr-latest","t":["ocr","image2text"],"mx":30720},{"n":"qwen3.5-omni-flash-2026-03-15","t":["chat","speech2text","image2text"],"mx":262144},{"n":"qwen3-asr-flash-realtime","t":["speech2text"],"mx":0},{"n":"qwen3.7-plus-2026-05-26","t":["chat","image2text"],"mx":1000000},{"n":"qwen3-tts-vc-realtime-2025-11-27","t":["tts"],"mx":0},{"n":"qwen-plus-2025-09-11","t":["chat"],"mx":1048576},{"n":"qwen3-coder-plus","t":["chat"],"mx":1000000},{"n":"qwen3.7-max-2026-05-20","t":["chat"],"mx":1000000},{"n":"qwen3-max-2025-09-23","t":["chat"],"mx":262144},{"n":"qwen3-tts-instruct-flash-2026-01-26","t":["tts"],"mx":0},{"n":"qwen3-tts-vd-realtime-2025-12-16","t":["tts"],"mx":0},{"n":"qwen-mt-lite","t":["chat"],"mx":8192},{"n":"qwen3-omni-flash-realtime","t":["chat","speech2text","image2text"],"mx":65536},{"n":"qwen3-tts-vd-2026-01-26","t":["tts"],"mx":0},{"n":"kimi/kimi-k2.7-code","t":["chat","image2text"],"mx":262144},{"n":"qwen-plus-2025-01-25","t":["chat"],"mx":131072},{"n":"qwen3.5-27b","t":["chat","image2text"],"mx":262144},{"n":"qwen-coder-turbo","t":["chat"],"mx":131072},{"n":"qwen3-omni-flash-2025-12-01","t":["chat","speech2text","image2text"],"mx":65536},{"n":"qwen3.6-max-preview","t":["chat"],"mx":262144},{"n":"qwen3-omni-flash-realtime-2025-12-01","t":["chat","speech2text","image2text"],"mx":65536},{"n":"kimi-k2.5","t":["chat","image2text"],"mx":262144},{"n":"qwen-coder-plus","t":["chat"],"mx":131072},{"n":"qwen3-vl-plus-2025-09-23","t":["chat","image2text"],"mx":262144},{"n":"qwen3.7-plus","t":["chat","image2text"],"mx":1000000},{"n":"qwen-flash-character","t":["chat"],"mx":8192},{"n":"qwen3.5-omni-plus","t":["chat","speech2text","image2text"],"mx":262144},{"n":"qwen3-coder-flash","t":["chat"],"mx":1000000},{"n":"kimi/kimi-k2.6","t":["chat","image2text"],"mx":262144},{"n":"qwen-math-plus-0919","t":["chat"],"mx":3072},{"n":"qwen3.6-flash","t":["chat","image2text"],"mx":1000000},{"n":"qwen3.6-plus","t":["chat","ocr","image2text"],"mx":1000000},{"n":"qwen3-tts-flash-realtime-2025-09-18","t":["tts"],"mx":0},{"n":"qwen3.5-omni-plus-2026-03-15","t":["chat","speech2text","image2text"],"mx":262144},{"n":"qwen3-tts-flash-realtime","t":["tts"],"mx":0},{"n":"qwen-omni-turbo","t":["chat","speech2text","image2text"],"mx":32768},{"n":"qwen3-livetranslate-flash-realtime-2025-09-22","t":["speech2text"],"mx":49152},{"n":"qwen3-tts-flash","t":["tts"],"mx":0},{"n":"deepseek-v4-pro","t":["chat"],"mx":1048576},{"n":"qwen3.6-27b","t":["chat","image2text"],"mx":262144},{"n":"qwen3-asr-flash-2026-02-10","t":["speech2text"],"mx":0},{"n":"qwen3-asr-flash-realtime-2026-02-10","t":["speech2text"],"mx":0},{"n":"ZHIPU/GLM-5","t":["chat"],"mx":202752},{"n":"qwen-tts-2025-05-22","t":["tts"],"mx":0},{"n":"qwen3-coder-plus-2025-09-23","t":["chat"],"mx":1000000},{"n":"qwen3-livetranslate-flash","t":["speech2text"],"mx":49152},{"n":"qwen3.7-max-2026-06-08","t":["chat","image2text"],"mx":1000000},{"n":"kimi-k2.6","t":["chat","image2text"],"mx":262144},{"n":"qwen3-tts-vc-realtime-2026-01-15","t":["tts"],"mx":0},{"n":"qwen3.5-omni-flash","t":["chat","speech2text","image2text"],"mx":262144},{"n":"qwen-vl-ocr","t":["ocr","image2text"],"mx":30720},{"n":"qwen3.5-122b-a10b","t":["chat"],"mx":128000},{"n":"deepseek-v3.2","t":["chat"],"mx":128000},{"n":"deepseek-r1","t":["chat"],"mx":65792},{"n":"deepseek-v3","t":["chat"],"mx":65792},{"n":"deepseek-r1-distill-qwen-1.5b","t":["chat"],"mx":32768},{"n":"deepseek-r1-distill-qwen-14b","t":["chat"],"mx":32768},{"n":"deepseek-r1-distill-qwen-32b","t":["chat"],"mx":32768},{"n":"deepseek-r1-distill-llama-8b","t":["chat"],"mx":32768},{"n":"deepseek-r1-distill-llama-70b","t":["chat"],"mx":32768},{"n":"qwq-plus","t":["chat"],"mx":131072},{"n":"qwen-plus-2025-07-28","t":["chat"],"mx":131072},{"n":"qwen-plus-2025-07-14","t":["chat"],"mx":131072},{"n":"qwen-flash","t":["chat"],"mx":1000000},{"n":"qwen-flash-2025-07-28","t":["chat"],"mx":1000000},{"n":"qwen3.5-plus","t":["chat"],"mx":1000000},{"n":"qwen3.5-plus-2026-02-15","t":["chat"],"mx":1000000},{"n":"qwen3.5-flash","t":["chat"],"mx":1000000},{"n":"qwen3.5-flash-2026-02-23","t":["chat"],"mx":1000000},{"n":"qwen3-max","t":["chat"],"mx":256000},{"n":"qwen3-coder-480b-a35b-instruct","t":["chat"],"mx":256000},{"n":"qwen3-30b-a3b-instruct-2507","t":["chat"],"mx":128000},{"n":"qwen3-30b-a3b-thinking-2507","t":["chat"],"mx":128000},{"n":"qwen3-30b-a3b","t":["chat"],"mx":128000},{"n":"qwen3-vl-plus","t":["image2text","chat"],"mx":256000},{"n":"qwen3-vl-235b-a22b-instruct","t":["image2text","chat"],"mx":128000},{"n":"qwen3-vl-235b-a22b-thinking","t":["image2text","chat"],"mx":128000},{"n":"qwen3-235b-a22b-instruct-2507","t":["chat"],"mx":128000},{"n":"qwen3-235b-a22b-thinking-2507","t":["chat"],"mx":128000},{"n":"qwen3-235b-a22b","t":["chat"],"mx":128000},{"n":"qwen3-next-80b-a3b-instruct","t":["chat"],"mx":128000},{"n":"qwen3-next-80b-a3b-thinking","t":["chat"],"mx":128000},{"n":"qwen3-8b","t":["chat"],"mx":128000},{"n":"qwen3-14b","t":["chat"],"mx":128000},{"n":"qwen3-32b","t":["chat"],"mx":128000},{"n":"qwen-long","t":["chat"],"mx":1000000},{"n":"qwen-turbo","t":["chat"],"mx":1000000},{"n":"qwen-max","t":["chat"],"mx":32768},{"n":"qwen-plus","t":["chat"],"mx":131072},{"n":"qwen-plus-2025-04-28","t":["chat"],"mx":128000},{"n":"qwen-plus-latest","t":["chat"],"mx":131072},{"n":"text-embedding-v2","t":["embedding"],"mx":2048},{"n":"sambert-zhide-v1","t":["tts"],"mx":2048},{"n":"sambert-zhiru-v1","t":["tts"],"mx":2048},{"n":"text-embedding-v3","t":["embedding"],"mx":8192},{"n":"text-embedding-v4","t":["embedding"],"mx":8192},{"n":"qwen-vl-max","t":["image2text","chat"],"mx":765},{"n":"qwen-vl-plus","t":["image2text","chat"],"mx":765},{"n":"gte-rerank","t":["rerank"],"mx":4000},{"n":"qwen3-asr-flash","t":["speech2text"],"mx":8000},{"n":"qwen3-asr-flash-2025-09-08","t":["speech2text"],"mx":8000},{"n":"gte-rerank-v2","t":["rerank"],"mx":4000},{"n":"qwen3-rerank","t":["rerank"],"mx":4000}]},{"name":"ZHIPU-AI","rank":"993","llm":[{"n":"glm-4.7","t":["chat"],"mx":128000},{"n":"glm-4.5","t":["chat"],"mx":128000},{"n":"glm-4.5-x","t":["chat"],"mx":128000},{"n":"glm-4.5-air","t":["chat"],"mx":128000},{"n":"glm-4.5-airx","t":["chat"],"mx":128000},{"n":"glm-4.5-flash","t":["chat"],"mx":128000},{"n":"glm-4.5v","t":["image2text","chat"],"mx":64000},{"n":"glm-4-plus","t":["chat"],"mx":128000},{"n":"glm-4-0520","t":["chat"],"mx":128000},{"n":"glm-4","t":["chat"],"mx":128000},{"n":"glm-4-airx","t":["chat"],"mx":8000},{"n":"glm-4-air","t":["chat"],"mx":128000},{"n":"glm-4-flash","t":["chat"],"mx":128000},{"n":"glm-4-flashx","t":["chat"],"mx":128000},{"n":"glm-4-long","t":["chat"],"mx":1000000},{"n":"glm-3-turbo","t":["chat"],"mx":128000},{"n":"glm-4v","t":["image2text","chat"],"mx":2000},{"n":"glm-4-9b","t":["chat"],"mx":8192},{"n":"embedding-2","t":["embedding"],"mx":512},{"n":"embedding-3","t":["embedding"],"mx":512},{"n":"glm-asr","t":["speech2text"],"mx":4096}]},{"name":"Ollama","rank":"830","llm":[]},{"name":"ModelScope","rank":null,"llm":[]},{"name":"LocalAI","rank":null,"llm":[]},{"name":"OpenAI-API-Compatible","rank":"985","llm":[]},{"name":"VLLM","rank":null,"llm":[]},{"name":"Moonshot","rank":"995","llm":[{"n":"kimi-thinking-preview","t":["chat"],"mx":131072},{"n":"kimi-k2-0711-preview","t":["chat"],"mx":131072},{"n":"kimi-k2-0905-preview","t":["chat"],"mx":262144},{"n":"kimi-k2-thinking","t":["chat"],"mx":262144},{"n":"kimi-k2-thinking-turbo","t":["chat"],"mx":262144},{"n":"kimi-k2-turbo-preview","t":["chat"],"mx":262144},{"n":"kimi-k2.5","t":["chat"],"mx":256000},{"n":"kimi-latest","t":["chat"],"mx":131072},{"n":"moonshot-v1-8k","t":["chat"],"mx":8192},{"n":"moonshot-v1-32k","t":["chat"],"mx":32768},{"n":"moonshot-v1-128k","t":["chat"],"mx":131072},{"n":"moonshot-v1-8k-vision-preview","t":["image2text","chat"],"mx":8192},{"n":"moonshot-v1-32k-vision-preview","t":["image2text","chat"],"mx":32768},{"n":"moonshot-v1-128k-vision-preview","t":["image2text","chat"],"mx":131072},{"n":"moonshot-v1-auto","t":["chat"],"mx":128000}]},{"name":"FastEmbed","rank":null,"llm":[]},{"name":"Xinference","rank":null,"llm":[]},{"name":"DeepSeek","rank":"996","llm":[{"n":"deepseek-v4-flash","t":["chat"],"mx":1000000},{"n":"deepseek-v4-pro","t":["chat"],"mx":1000000}]},{"name":"VolcEngine","rank":null,"llm":[]},{"name":"BaiChuan","rank":null,"llm":[{"n":"Baichuan2-Turbo","t":["chat"],"mx":32768},{"n":"Baichuan2-Turbo-192k","t":["chat"],"mx":196608},{"n":"Baichuan3-Turbo","t":["chat"],"mx":32768},{"n":"Baichuan3-Turbo-128k","t":["chat"],"mx":131072},{"n":"Baichuan4","t":["chat"],"mx":131072},{"n":"Baichuan-Text-Embedding","t":["embedding"],"mx":512}]},{"name":"Jina","rank":null,"llm":[{"n":"jina-reranker-v1-base-en","t":["rerank"],"mx":8196},{"n":"jina-reranker-v1-turbo-en","t":["rerank"],"mx":8196},{"n":"jina-reranker-v1-tiny-en","t":["rerank"],"mx":8196},{"n":"jina-colbert-v1-en","t":["rerank"],"mx":8196},{"n":"jina-embeddings-v2-base-en","t":["embedding"],"mx":8196},{"n":"jina-embeddings-v2-base-de","t":["embedding"],"mx":8196},{"n":"jina-embeddings-v2-base-es","t":["embedding"],"mx":8196},{"n":"jina-embeddings-v2-base-code","t":["embedding"],"mx":8196},{"n":"jina-embeddings-v2-base-zh","t":["embedding"],"mx":8196},{"n":"jina-reranker-v2-base-multilingual","t":["rerank"],"mx":8196},{"n":"jina-embeddings-v3","t":["embedding"],"mx":8196},{"n":"jina-embeddings-v4","t":["embedding"],"mx":32768}]},{"name":"Builtin","rank":null,"llm":[{"n":"BAAI/bge-small-en-v1.5","t":["embedding"],"mx":512},{"n":"BAAI/bge-m3","t":["embedding"],"mx":8192},{"n":"Qwen/Qwen3-Embedding-0.6B","t":["embedding"],"mx":32768}]},{"name":"MiniMax","rank":"987","llm":[{"n":"MiniMax-M3","t":["chat"],"mx":1000000},{"n":"MiniMax-M2.7","t":["chat"],"mx":204800},{"n":"MiniMax-M2.7-highspeed","t":["chat"],"mx":204800},{"n":"MiniMax-M2.5","t":["chat"],"mx":204800},{"n":"MiniMax-M2.5-highspeed","t":["chat"],"mx":204800},{"n":"MiniMax-M2.1","t":["chat"],"mx":200000},{"n":"MiniMax-M2","t":["chat"],"mx":200000}]},{"name":"Mistral","rank":null,"llm":[{"n":"codestral-latest","t":["chat"],"mx":256000},{"n":"mistral-large-latest","t":["chat"],"mx":131000},{"n":"mistral-saba-latest","t":["chat"],"mx":32000},{"n":"pixtral-large-latest","t":["image2text","chat"],"mx":131000},{"n":"ministral-3b-latest","t":["chat"],"mx":131000},{"n":"ministral-8b-latest","t":["chat"],"mx":131000},{"n":"mistral-embed","t":["embedding"],"mx":8192},{"n":"mistral-moderation-latest","t":["chat"],"mx":8192},{"n":"mistral-small-latest","t":["chat"],"mx":32000},{"n":"pixtral-12b-2409","t":["image2text","chat"],"mx":131000},{"n":"mistral-ocr-latest","t":["image2text","chat"],"mx":131000},{"n":"open-mistral-nemo","t":["chat"],"mx":131000},{"n":"open-codestral-mamba","t":["chat"],"mx":256000}]},{"name":"Azure-OpenAI","rank":null,"llm":[{"n":"gpt-4o-mini","t":["image2text","chat"],"mx":128000},{"n":"gpt-4o","t":["image2text","chat"],"mx":128000},{"n":"gpt-3.5-turbo","t":["chat"],"mx":4096},{"n":"gpt-3.5-turbo-16k","t":["chat"],"mx":16385},{"n":"text-embedding-ada-002","t":["embedding"],"mx":8191},{"n":"text-embedding-3-small","t":["embedding"],"mx":8191},{"n":"text-embedding-3-large","t":["embedding"],"mx":8191},{"n":"whisper-1","t":["speech2text"],"mx":26214400},{"n":"gpt-4","t":["chat"],"mx":8191},{"n":"gpt-4-turbo","t":["chat"],"mx":8191},{"n":"gpt-4-32k","t":["chat"],"mx":32768},{"n":"gpt-4-vision-preview","t":["image2text","chat"],"mx":765}]},{"name":"Bedrock","rank":null,"llm":[]},{"name":"Gemini","rank":"997","llm":[{"n":"gemini-3-pro-preview","t":["image2text","chat"],"mx":1048576},{"n":"gemini-2.5-flash","t":["image2text","chat"],"mx":1048576},{"n":"gemini-2.5-pro","t":["image2text","chat"],"mx":1048576},{"n":"gemini-2.5-flash-lite","t":["image2text","chat"],"mx":1048576},{"n":"gemini-2.0-flash","t":["image2text","chat"],"mx":1048576},{"n":"gemini-2.0-flash-lite","t":["image2text","chat"],"mx":1048576},{"n":"gemini-embedding-001","t":["embedding"],"mx":2048}]},{"name":"Groq","rank":null,"llm":[{"n":"gemma2-9b-it","t":["chat"],"mx":8192},{"n":"llama3-70b-8192","t":["chat"],"mx":8192},{"n":"llama3-8b-8192","t":["chat"],"mx":8192},{"n":"llama-3.1-70b-versatile","t":["chat"],"mx":131072},{"n":"llama-3.1-8b-instant","t":["chat"],"mx":131072},{"n":"llama-3.3-70b-versatile","t":["chat"],"mx":128000},{"n":"llama-3.3-70b-specdec","t":["chat"],"mx":8192},{"n":"mixtral-8x7b-32768","t":["chat"],"mx":32768}]},{"name":"OpenRouter","rank":"989","llm":[]},{"name":"StepFun","rank":null,"llm":[{"n":"step-3","t":["image2text","chat"],"mx":65536},{"n":"step-2-mini","t":["chat"],"mx":32768},{"n":"step-2-16k","t":["chat"],"mx":16384},{"n":"step-1-8k","t":["chat"],"mx":8192},{"n":"step-1-32k","t":["chat"],"mx":32768},{"n":"step-1-256k","t":["chat"],"mx":262144},{"n":"step-r1-v-mini","t":["image2text","chat"],"mx":102400},{"n":"step-1v-8k","t":["image2text","chat"],"mx":8192},{"n":"step-1v-32k","t":["image2text","chat"],"mx":32768},{"n":"step-1o-vision-32k","t":["image2text","chat"],"mx":32768},{"n":"step-1o-turbo-vision","t":["image2text","chat"],"mx":32768},{"n":"step-tts-mini","t":["tts"],"mx":1000},{"n":"step-tts-vivid","t":["tts"],"mx":1000},{"n":"step-asr","t":["speech2text"],"mx":32768}]},{"name":"NVIDIA","rank":null,"llm":[{"n":"01-ai/yi-large","t":["chat"],"mx":32768},{"n":"abacusai/dracarys-llama-3.1-70b-instruct","t":["chat"],"mx":131072},{"n":"ai21labs/jamba-1.5-large-instruct","t":["chat"],"mx":256000},{"n":"ai21labs/jamba-1.5-mini-instruct","t":["chat"],"mx":256000},{"n":"aisingapore/sea-lion-7b-instruct","t":["chat"],"mx":131072},{"n":"baichuan-inc/baichuan2-13b-chat","t":["chat"],"mx":196608},{"n":"bigcode/starcoder2-7b","t":["chat"],"mx":16384},{"n":"bigcode/starcoder2-15b","t":["chat"],"mx":16384},{"n":"databricks/dbrx-instruct","t":["chat"],"mx":32768},{"n":"deepseek-ai/deepseek-r1","t":["chat"],"mx":131072},{"n":"google/gemma-2b","t":["chat"],"mx":8192},{"n":"google/gemma-7b","t":["chat"],"mx":8192},{"n":"google/gemma-2-2b-it","t":["chat"],"mx":4096},{"n":"google/gemma-2-9b-it","t":["chat"],"mx":4096},{"n":"google/gemma-2-27b-it","t":["chat"],"mx":4096},{"n":"google/codegemma-1.1-7b","t":["chat"],"mx":8192},{"n":"google/codegemma-7b","t":["chat"],"mx":8192},{"n":"google/recurrentgemma-2b","t":["chat"],"mx":8192},{"n":"google/shieldgemma-9b","t":["chat"],"mx":8192},{"n":"ibm/granite-3.0-3b-a800m-instruct","t":["chat"],"mx":4096},{"n":"ibm/granite-3.0-8b-instruct","t":["chat"],"mx":4096},{"n":"ibm/granite-34b-code-instruct","t":["chat"],"mx":8192},{"n":"ibm/granite-8b-code-instruct","t":["chat"],"mx":131072},{"n":"ibm/granite-guardian-3.0-8b","t":["chat"],"mx":131072},{"n":"igenius / colosseum-355b_instruct_16k","t":["chat"],"mx":16384},{"n":"igenius / italia_10b_instruct_16k","t":["chat"],"mx":16384},{"n":"institute-of-science-tokyo/llama-3.1-swallow-70b-instruct-v01","t":["chat"],"mx":8192},{"n":"institute-of-science-tokyo/llama-3.1-swallow-8b-instruct-v0.1","t":["chat"],"mx":8192},{"n":"mediatek/breeze-7b-instruct","t":["chat"],"mx":8192},{"n":"meta/codellama-70b","t":["chat"],"mx":100000},{"n":"meta/llama2-70b","t":["chat"],"mx":4096},{"n":"meta/llama3-8b","t":["chat"],"mx":8192},{"n":"meta/llama3-70b","t":["chat"],"mx":8192},{"n":"meta/llama-3.1-8b-instruct","t":["chat"],"mx":131072},{"n":"meta/llama-3.1-70b-instruct","t":["chat"],"mx":131072},{"n":"meta/llama-3.1-405b-instruct","t":["chat"],"mx":131072},{"n":"meta/llama-3.2-1b-instruct","t":["chat"],"mx":131072},{"n":"meta/llama-3.2-3b-instruct","t":["chat"],"mx":131072},{"n":"meta/llama-3.3-70b-instruct","t":["chat"],"mx":131072},{"n":"microsoft/phi-3-medium-128k-instruct","t":["chat"],"mx":131072},{"n":"microsoft/phi-3-medium-4k-instruct","t":["chat"],"mx":4096},{"n":"microsoft/phi-3-mini-128k-instruct","t":["chat"],"mx":131072},{"n":"microsoft/phi-3-mini-4k-instruct","t":["chat"],"mx":4096},{"n":"microsoft/phi-3-small-128k-instruct","t":["chat"],"mx":131072},{"n":"microsoft/phi-3-small-8k-instruct","t":["chat"],"mx":8192},{"n":"microsoft/phi-3.5-mini","t":["chat"],"mx":131072},{"n":"microsoft/phi-3.5-moe-instruct","t":["chat"],"mx":131072},{"n":"mistralai/codestral-22b-instruct-v0.1","t":["chat"],"mx":32768},{"n":"mistralai/mamba-codestral-7b-v0.1","t":["chat"],"mx":4096},{"n":"mistralai/mistral-2-large-instruct","t":["chat"],"mx":131072},{"n":"mistralai/mathstral-7b-v01","t":["chat"],"mx":4096},{"n":"mistralai/mistral-7b-instruct","t":["chat"],"mx":32768},{"n":"mistralai/mistral-7b-instruct-v0.3","t":["chat"],"mx":32768},{"n":"mistralai/mixtral-8x7b-instruct","t":["chat"],"mx":32768},{"n":"mistralai/mixtral-8x22b-instruct","t":["chat"],"mx":65536},{"n":"mistralai/mistral-large","t":["chat"],"mx":32768},{"n":"mistralai/mistral-small-24b-instruct","t":["chat"],"mx":32768},{"n":"nvidia/llama3-chatqa-1.5-8b","t":["chat"],"mx":8192},{"n":"nvidia/llama-3.1-nemoguard-8b-content-safety","t":["chat"],"mx":131072},{"n":"nvidia/llama-3.1-nemoguard-8b-topic-control","t":["chat"],"mx":131072},{"n":"nvidia/llama-3.1-nemotron-51b-instruct","t":["chat"],"mx":131072},{"n":"nvidia/llama-3.1-nemotron-70b-instruct","t":["chat"],"mx":131072},{"n":"nvidia/llama-3.1-nemotron-70b-reward","t":["chat"],"mx":128000},{"n":"nvidia/llama3-chatqa-1.5-70b","t":["chat"],"mx":131072},{"n":"nvidia/mistral-nemo-minitron-8b-base","t":["chat"],"mx":8192},{"n":"nvidia/mistral-nemo-minitron-8b-8k-instruct","t":["chat"],"mx":8192},{"n":"nvidia/nemotron-4-340b-instruct","t":["chat"],"mx":4096},{"n":"nvidia/nemotron-4-340b-reward","t":["chat"],"mx":4096},{"n":"nvidia/nemotron-4-mini-hindi-4b-instruct","t":["chat"],"mx":4096},{"n":"nvidia/nemotron-mini-4b-instruct","t":["chat"],"mx":4096},{"n":"nv-mistralai/mistral-nemo-12b-instruct","t":["chat"],"mx":131072},{"n":"qwen/qwen2-7b-instruct","t":["chat"],"mx":131072},{"n":"qwen/qwen2.5-7b-instruct","t":["chat"],"mx":131072},{"n":"qwen/qwen2.5-coder-7b-instruct","t":["chat"],"mx":32768},{"n":"rakuten/rakutenai-7b-chat","t":["chat"],"mx":4096},{"n":"rakuten/rakutenai-7b-instruct","t":["chat"],"mx":32768},{"n":"seallms/seallm-7b-v2.5","t":["chat"],"mx":8192},{"n":"snowflake/arctic","t":["chat"],"mx":8192},{"n":"tokyotech-llm/llama-3-swallow-70b-instruct-v01","t":["chat"],"mx":8192},{"n":"thudm/chatglm3-6b","t":["chat"],"mx":131072},{"n":"tiiuae/falcon3-7b-instruct","t":["chat"],"mx":32768},{"n":"upstage/solar-10.7b-instruct","t":["chat"],"mx":8192},{"n":"writer/palmyra-creative-122b","t":["chat"],"mx":131072},{"n":"writer/palmyra-fin-70b-32k","t":["chat"],"mx":32768},{"n":"writer/palmyra-med-70b-32k","t":["chat"],"mx":32768},{"n":"writer/palmyra-med-70b","t":["chat"],"mx":8192},{"n":"yentinglin/llama-3-taiwan-70b-instruct","t":["chat"],"mx":8192},{"n":"zyphra/zamba2-7b-instruct","t":["chat"],"mx":4096},{"n":"BAAI/bge-m3","t":["embedding"],"mx":8192},{"n":"BAAI/bge-m3-unsupervised","t":["embedding"],"mx":8192},{"n":"BAAI/bge-m3-retromae","t":["embedding"],"mx":8129},{"n":"BAAI/bge-large-en-v1.5","t":["embedding"],"mx":512},{"n":"BAAI/bge-base-en-v1.5","t":["embedding"],"mx":512},{"n":"BAAI/bge-small-en-v1.5","t":["embedding"],"mx":512},{"n":"nvidia/embed-qa-4","t":["embedding"],"mx":512},{"n":"nvidia/llama-3.2-nv-embedqa-1b-v1","t":["embedding"],"mx":512},{"n":"nvidia/llama-3.2-nv-embedqa-1b-v2","t":["embedding"],"mx":8192},{"n":"nvidia/llama-3.2-nv-rerankqa-1b-v1","t":["rerank"],"mx":512},{"n":"nvidia/llama-3.2-nv-rerankqa-1b-v2","t":["rerank"],"mx":8192},{"n":"nvidia/nvclip","t":["embedding"],"mx":1024},{"n":"nvidia/nv-embed-v1","t":["embedding"],"mx":4096},{"n":"nvidia/nv-embedqa-e5-v5","t":["embedding"],"mx":1024},{"n":"nvidia/nv-embedqa-mistral-7b-v2","t":["embedding"],"mx":4096},{"n":"nvidia/nv-rerankqa-mistral-4b-v3","t":["rerank"],"mx":512},{"n":"nvidia/rerank-qa-mistral-4b","t":["embedding"],"mx":512},{"n":"snowflake-arctic-embed-xs","t":["embedding"],"mx":512},{"n":"snowflake-arctic-embed-s","t":["embedding"],"mx":512},{"n":"snowflake-arctic-embed-m","t":["embedding"],"mx":512},{"n":"snowflake-arctic-embed-m-long","t":["embedding"],"mx":512},{"n":"snowflake-arctic-embed-l","t":["embedding"],"mx":512},{"n":"adept/fuyu-8b","t":["image2text","chat"],"mx":1024},{"n":"google/deplot","t":["image2text","chat"],"mx":8192},{"n":"google/paligemma","t":["image2text","chat"],"mx":256000},{"n":"meta/llama-3.2-11b-vision-instruct","t":["image2text","chat"],"mx":131072},{"n":"meta/llama-3.2-90b-vision-instruct","t":["image2text","chat"],"mx":131072},{"n":"microsoft/florence-2","t":["image2text","chat"],"mx":1024},{"n":"microsoft/kosmos-2","t":["image2text","chat"],"mx":4096},{"n":"microsoft/phi-3-vision-128k-instruct","t":["image2text","chat"],"mx":131072},{"n":"microsoft/phi-3.5-vision-instruct","t":["image2text","chat"],"mx":131072},{"n":"nvidia/neva-22b","t":["image2text","chat"],"mx":1024}]},{"name":"LM-Studio","rank":null,"llm":[]},{"name":"Cohere","rank":"990","llm":[{"n":"command-a-plus-05-2026","t":["chat"],"mx":131072},{"n":"command-a-03-2025","t":["chat"],"mx":262144},{"n":"command-r7b-12-2024","t":["chat"],"mx":131072},{"n":"command-a-translate-08-2025","t":["chat"],"mx":8192},{"n":"command-a-reasoning-08-2025","t":["chat"],"mx":262144},{"n":"command-a-vision-07-2025","t":["chat"],"mx":131072},{"n":"command-r-plus-08-2024","t":["chat"],"mx":131072},{"n":"command-r-08-2024","t":["chat"],"mx":131072},{"n":"embed-v4.0","t":["embedding"],"mx":131072},{"n":"embed-english-v3.0","t":["embedding"],"mx":512},{"n":"embed-english-light-v3.0","t":["embedding"],"mx":512},{"n":"embed-multilingual-v3.0","t":["embedding"],"mx":512},{"n":"embed-multilingual-light-v3.0","t":["embedding"],"mx":512},{"n":"rerank-v4.0-pro","t":["rerank"],"mx":32768},{"n":"rerank-v4.0-fast","t":["rerank"],"mx":32768},{"n":"rerank-v3.5","t":["rerank"],"mx":4096},{"n":"rerank-english-v3.0","t":["rerank"],"mx":4096},{"n":"rerank-multilingual-v3.0","t":["rerank"],"mx":4096},{"n":"cohere-transcribe-03-2026","t":["speech2text"],"mx":8192}]},{"name":"TogetherAI","rank":null,"llm":[]},{"name":"Upstage","rank":null,"llm":[{"n":"solar-1-mini-chat","t":["chat"],"mx":32768},{"n":"solar-1-mini-chat-ja","t":["chat"],"mx":32768},{"n":"solar-embedding-1-large-query","t":["embedding"],"mx":4000},{"n":"solar-embedding-1-large-passage","t":["embedding"],"mx":4000}]},{"name":"NovitaAI","rank":null,"llm":[{"n":"qwen/qwen2.5-7b-instruct","t":["chat"],"mx":32000},{"n":"meta-llama/llama-3.2-1b-instruct","t":["chat"],"mx":131000},{"n":"meta-llama/llama-3.2-3b-instruct","t":["chat"],"mx":32768},{"n":"thudm/glm-4-9b-0414","t":["chat"],"mx":32000},{"n":"thudm/glm-z1-9b-0414","t":["chat"],"mx":32000},{"n":"meta-llama/llama-3.1-8b-instruct-bf16","t":["chat"],"mx":8192},{"n":"meta-llama/llama-3.1-8b-instruct","t":["chat"],"mx":16384},{"n":"deepseek/deepseek-v3-0324","t":["chat"],"mx":128000},{"n":"deepseek/deepseek-r1-turbo","t":["chat"],"mx":64000},{"n":"Sao10K/L3-8B-Stheno-v3.2","t":["chat"],"mx":8192},{"n":"meta-llama/llama-3.3-70b-instruct","t":["chat"],"mx":131072},{"n":"deepseek/deepseek-r1-distill-llama-8b","t":["chat"],"mx":32000},{"n":"mistralai/mistral-nemo","t":["chat"],"mx":131072},{"n":"meta-llama/llama-3-8b-instruct","t":["chat"],"mx":8192},{"n":"deepseek/deepseek-v3-turbo","t":["chat"],"mx":64000},{"n":"mistralai/mistral-7b-instruct","t":["chat"],"mx":32768},{"n":"deepseek/deepseek-r1","t":["chat"],"mx":64000},{"n":"deepseek/deepseek-r1-distill-qwen-14b","t":["chat"],"mx":64000},{"n":"baai/bge-m3","t":["embedding"],"mx":8192}]},{"name":"SILICONFLOW","rank":"986","llm":[{"n":"deepseek-ai/DeepSeek-V4-Pro","t":["chat"],"mx":1000000},{"n":"deepseek-ai/DeepSeek-V4-Flash","t":["chat"],"mx":1000000},{"n":"Pro/moonshotai/Kimi-K2.6","t":["image2text","chat"],"mx":262000},{"n":"Pro/zai-org/GLM-5.1","t":["chat"],"mx":205000},{"n":"nex-agi/Nex-N2-Pro","t":["image2text","chat"],"mx":32000},{"n":"MiniMaxAI/MiniMax-M2.5","t":["chat"],"mx":197000},{"n":"Pro/MiniMaxAI/MiniMax-M2.5","t":["chat"],"mx":197000},{"n":"deepseek-ai/DeepSeek-V3.2","t":["chat"],"mx":164000},{"n":"Pro/deepseek-ai/DeepSeek-V3.2","t":["chat"],"mx":164000},{"n":"deepseek-ai/DeepSeek-V3.1-Terminus","t":["chat"],"mx":164000},{"n":"Pro/deepseek-ai/DeepSeek-V3.1-Terminus","t":["chat"],"mx":164000},{"n":"Qwen/Qwen3.6-35B-A3B","t":["image2text","chat"],"mx":256000},{"n":"Qwen/Qwen3.6-27B","t":["image2text","chat"],"mx":256000},{"n":"Qwen/Qwen3.5-397B-A17B","t":["image2text","chat"],"mx":256000},{"n":"Qwen/Qwen3.5-122B-A10B","t":["image2text","chat"],"mx":256000},{"n":"Qwen/Qwen3.5-35B-A3B","t":["image2text","chat"],"mx":256000},{"n":"Qwen/Qwen3.5-27B","t":["image2text","chat"],"mx":256000},{"n":"Qwen/Qwen3.5-9B","t":["image2text","chat"],"mx":256000},{"n":"Qwen/Qwen3.5-4B","t":["image2text","chat"],"mx":256000},{"n":"deepseek-ai/DeepSeek-R1","t":["chat"],"mx":160000},{"n":"Pro/deepseek-ai/DeepSeek-R1","t":["chat"],"mx":160000},{"n":"deepseek-ai/DeepSeek-V3","t":["chat"],"mx":160000},{"n":"Pro/deepseek-ai/DeepSeek-V3","t":["chat"],"mx":160000},{"n":"stepfun-ai/Step-3.5-Flash","t":["chat"],"mx":256000},{"n":"Qwen/Qwen3-VL-32B-Instruct","t":["image2text","chat"],"mx":256000},{"n":"Qwen/Qwen3-VL-32B-Thinking","t":["image2text","chat"],"mx":256000},{"n":"Qwen/Qwen3-VL-8B-Instruct","t":["image2text","chat"],"mx":256000},{"n":"Qwen/Qwen3-VL-8B-Thinking","t":["image2text","chat"],"mx":256000},{"n":"Qwen/Qwen3-VL-30B-A3B-Instruct","t":["image2text","chat"],"mx":256000},{"n":"Qwen/Qwen3-VL-30B-A3B-Thinking","t":["image2text","chat"],"mx":256000},{"n":"Qwen/Qwen3-Omni-30B-A3B-Instruct","t":["image2text","chat"],"mx":256000},{"n":"Qwen/Qwen3-Omni-30B-A3B-Thinking","t":["image2text","chat"],"mx":64000},{"n":"Qwen/Qwen3-Omni-30B-A3B-Captioner","t":["image2text","chat"],"mx":64000},{"n":"inclusionAI/Ling-flash-2.0","t":["chat"],"mx":128000},{"n":"inclusionAI/Ling-mini-2.0","t":["chat"],"mx":128000},{"n":"tencent/Hunyuan-MT-7B","t":["chat"],"mx":32000},{"n":"ByteDance-Seed/Seed-OSS-36B-Instruct","t":["chat"],"mx":256000},{"n":"zai-org/GLM-4.5V","t":["image2text","chat"],"mx":64000},{"n":"zai-org/GLM-4.5-Air","t":["chat"],"mx":128000},{"n":"Qwen/Qwen3-Coder-30B-A3B-Instruct","t":["chat"],"mx":256000},{"n":"Qwen/Qwen3-30B-A3B-Instruct-2507","t":["chat"],"mx":256000},{"n":"tencent/Hunyuan-A13B-Instruct","t":["chat"],"mx":128000},{"n":"deepseek-ai/DeepSeek-R1-0528-Qwen3-8B","t":["chat"],"mx":128000},{"n":"Qwen/Qwen3-32B","t":["chat"],"mx":128000},{"n":"Qwen/Qwen3-14B","t":["chat"],"mx":128000},{"n":"Qwen/Qwen3-8B","t":["chat"],"mx":128000},{"n":"THUDM/GLM-4-32B-0414","t":["chat"],"mx":32000},{"n":"THUDM/GLM-Z1-9B-0414","t":["chat"],"mx":128000},{"n":"THUDM/GLM-4-9B-0414","t":["chat"],"mx":32000},{"n":"Qwen/Qwen2.5-72B-Instruct-128K","t":["chat"],"mx":128000},{"n":"Qwen/Qwen2.5-72B-Instruct","t":["chat"],"mx":32000},{"n":"Qwen/Qwen2.5-32B-Instruct","t":["chat"],"mx":32000},{"n":"Qwen/Qwen2.5-14B-Instruct","t":["chat"],"mx":32000},{"n":"Qwen/Qwen2.5-7B-Instruct","t":["chat"],"mx":32000},{"n":"Pro/Qwen/Qwen2.5-7B-Instruct","t":["chat"],"mx":32000},{"n":"Qwen/Qwen3-VL-Embedding-8B","t":["embedding"],"mx":32000},{"n":"Qwen/Qwen3-Embedding-8B","t":["embedding"],"mx":32000},{"n":"Qwen/Qwen3-Embedding-4B","t":["embedding"],"mx":32000},{"n":"Qwen/Qwen3-Embedding-0.6B","t":["embedding"],"mx":32000},{"n":"BAAI/bge-m3","t":["embedding"],"mx":8192},{"n":"BAAI/bge-large-en-v1.5","t":["embedding"],"mx":512},{"n":"BAAI/bge-large-zh-v1.5","t":["embedding"],"mx":512},{"n":"Pro/BAAI/bge-m3","t":["embedding"],"mx":8192},{"n":"Qwen/Qwen3-VL-Reranker-8B","t":["rerank"],"mx":32000},{"n":"Qwen/Qwen3-Reranker-8B","t":["rerank"],"mx":32000},{"n":"Qwen/Qwen3-Reranker-4B","t":["rerank"],"mx":32000},{"n":"Qwen/Qwen3-Reranker-0.6B","t":["rerank"],"mx":32000},{"n":"BAAI/bge-reranker-v2-m3","t":["rerank"],"mx":8192},{"n":"Pro/BAAI/bge-reranker-v2-m3","t":["rerank"],"mx":8192},{"n":"fnlp/MOSS-TTSD-v0.5","t":["tts"],"mx":26214400},{"n":"FunAudioLLM/CosyVoice2-0.5B","t":["tts"],"mx":26214400}]},{"name":"siliconflow_intl","rank":null,"llm":[{"n":"meta-llama/Meta-Llama-3.1-8B-Instruct","t":["chat"],"mx":33000},{"n":"MiniMaxAI/MiniMax-M2.5","t":["chat"],"mx":197000},{"n":"zai-org/GLM-5","t":["chat"],"mx":205000},{"n":"stepfun-ai/Step-3.5-Flash","t":["chat"],"mx":262000},{"n":"moonshotai/Kimi-K2.5","t":["chat"],"mx":262000},{"n":"MiniMaxAI/MiniMax-M2.1","t":["chat"],"mx":197000},{"n":"zai-org/GLM-4.7","t":["chat"],"mx":205000},{"n":"deepseek-ai/DeepSeek-V3.2","t":["chat"],"mx":164000},{"n":"deepseek-ai/DeepSeek-V3.2-Exp","t":["chat"],"mx":164000},{"n":"zai-org/GLM-4.6V","t":["chat"],"mx":131000},{"n":"deepseek-ai/DeepSeek-V3.1-Terminus","t":["chat"],"mx":164000},{"n":"deepseek-ai/DeepSeek-V3.1","t":["chat"],"mx":164000},{"n":"deepseek-ai/DeepSeek-V3","t":["chat"],"mx":164000},{"n":"deepseek-ai/DeepSeek-R1","t":["chat"],"mx":154000},{"n":"nex-agi/DeepSeek-V3.1-Nex-N1","t":["chat"],"mx":164000},{"n":"Qwen/Qwen3-VL-32B-Instruct","t":["chat"],"mx":262000},{"n":"Qwen/Qwen3-VL-32B-Thinking","t":["chat"],"mx":262000},{"n":"zai-org/GLM-4.5V","t":["chat"],"mx":66000},{"n":"inclusionAI/Ling-mini-2.0","t":["chat"],"mx":131000},{"n":"inclusionAI/Ring-flash-2.0","t":["chat"],"mx":131000},{"n":"inclusionAI/Ling-flash-2.0","t":["chat"],"mx":131000},{"n":"tencent/Hunyuan-MT-7B","t":["chat"],"mx":32000},{"n":"Qwen/Qwen3-Omni-30B-A3B-Captioner","t":["chat"],"mx":131000},{"n":"Qwen/Qwen3-Omni-30B-A3B-Thinking","t":["chat"],"mx":131000},{"n":"Qwen/Qwen3-Omni-30B-A3B-Instruct","t":["chat"],"mx":65000},{"n":"Qwen/Qwen3-Next-80B-A3B-Thinking","t":["chat"],"mx":262000},{"n":"Qwen/Qwen3-Next-80B-A3B-Instruct","t":["chat"],"mx":262000},{"n":"Qwen/Qwen3-Coder-480B-A35B-Instruct","t":["chat"],"mx":262000},{"n":"Qwen/Qwen3-Coder-30B-A3B-Instruct","t":["chat"],"mx":262000},{"n":"Qwen/Qwen3-30B-A3B-Thinking-2507","t":["chat"],"mx":262000},{"n":"Qwen/Qwen3-30B-A3B-Instruct-2507","t":["chat"],"mx":262000},{"n":"Qwen/Qwen3-235B-A22B-Instruct-2507","t":["chat"],"mx":262000},{"n":"Qwen/Qwen3-235B-A22B-Thinking-2507","t":["chat"],"mx":262000},{"n":"ByteDance-Seed/Seed-OSS-36B-Instruct","t":["chat"],"mx":262000},{"n":"baidu/ERNIE-4.5-300B-A47B","t":["chat"],"mx":131000},{"n":"tencent/Hunyuan-A13B-Instruct","t":["chat"],"mx":131000},{"n":"moonshotai/Kimi-K2-Instruct","t":["chat"],"mx":131000},{"n":"Qwen/Qwen3-32B","t":["chat"],"mx":131000},{"n":"Qwen/Qwen3-14B","t":["chat"],"mx":131000},{"n":"Qwen/Qwen3-8B","t":["chat"],"mx":131000},{"n":"Qwen/Qwen3-Reranker-8B","t":["rerank"],"mx":33000},{"n":"Qwen/Qwen3-Embedding-8B","t":["embedding"],"mx":33000},{"n":"Qwen/Qwen3-Reranker-4B","t":["rerank"],"mx":33000},{"n":"Qwen/Qwen3-Embedding-4B","t":["embedding"],"mx":33000},{"n":"Qwen/Qwen3-Reranker-0.6B","t":["rerank"],"mx":33000},{"n":"Qwen/Qwen3-Embedding-0.6B","t":["embedding"],"mx":33000},{"n":"THUDM/GLM-Z1-32B-0414","t":["chat"],"mx":131000},{"n":"THUDM/GLM-4-32B-0414","t":["chat"],"mx":33000},{"n":"THUDM/GLM-Z1-9B-0414","t":["chat"],"mx":131000},{"n":"THUDM/GLM-4-9B-0414","t":["chat"],"mx":33000},{"n":"Qwen/QwQ-32B","t":["chat"],"mx":131000},{"n":"deepseek-ai/DeepSeek-R1-Distill-Qwen-32B","t":["chat"],"mx":131000},{"n":"deepseek-ai/DeepSeek-R1-Distill-Qwen-14B","t":["chat"],"mx":131000},{"n":"Qwen/Qwen2.5-Coder-32B-Instruct","t":["chat"],"mx":33000},{"n":"Qwen/Qwen2.5-72B-Instruct-128K","t":["chat"],"mx":131000},{"n":"deepseek-ai/deepseek-vl2","t":["chat"],"mx":4000},{"n":"Qwen/Qwen2.5-72B-Instruct","t":["chat"],"mx":33000},{"n":"Qwen/Qwen2.5-32B-Instruct","t":["chat"],"mx":33000},{"n":"Qwen/Qwen2.5-14B-Instruct","t":["chat"],"mx":33000},{"n":"Qwen/Qwen2.5-7B-Instruct","t":["chat"],"mx":33000},{"n":"IndexTeam/IndexTTS-2","t":["tts"],"mx":1000}]},{"name":"PPIO","rank":null,"llm":[{"n":"deepseek/deepseek-r1/community","t":["chat"],"mx":64000},{"n":"deepseek/deepseek-v3/community","t":["chat"],"mx":64000},{"n":"deepseek/deepseek-r1","t":["chat"],"mx":64000},{"n":"deepseek/deepseek-v3","t":["chat"],"mx":64000},{"n":"deepseek/deepseek-r1-distill-llama-70b","t":["chat"],"mx":32000},{"n":"deepseek/deepseek-r1-distill-qwen-32b","t":["chat"],"mx":64000},{"n":"deepseek/deepseek-r1-distill-qwen-14b","t":["chat"],"mx":64000},{"n":"deepseek/deepseek-r1-distill-llama-8b","t":["chat"],"mx":32000},{"n":"qwen/qwen-2.5-72b-instruct","t":["chat"],"mx":32768},{"n":"qwen/qwen-2-vl-72b-instruct","t":["chat"],"mx":32768},{"n":"meta-llama/llama-3.2-3b-instruct","t":["chat"],"mx":32768},{"n":"qwen/qwen2.5-32b-instruct","t":["chat"],"mx":32000},{"n":"baichuan/baichuan2-13b-chat","t":["chat"],"mx":14336},{"n":"meta-llama/llama-3.1-70b-instruct","t":["chat"],"mx":32768},{"n":"meta-llama/llama-3.1-8b-instruct","t":["chat"],"mx":32768},{"n":"01-ai/yi-1.5-34b-chat","t":["chat"],"mx":16384},{"n":"01-ai/yi-1.5-9b-chat","t":["chat"],"mx":16384},{"n":"thudm/glm-4-9b-chat","t":["chat"],"mx":32768},{"n":"qwen/qwen-2-7b-instruct","t":["chat"],"mx":32768}]},{"name":"Replicate","rank":"987","llm":[{"n":"meta/llama-4-maverick-instruct","t":["chat"],"mx":8192},{"n":"meta/llama-4-scout-instruct","t":["chat"],"mx":8192},{"n":"meta/meta-llama-3-70b-instruct","t":["chat"],"mx":8192},{"n":"meta/meta-llama-3-8b-instruct","t":["chat"],"mx":8192},{"n":"replicate/all-mpnet-base-v2:b6b7585c9640cd7a9572c6e129c9549d79c9c31f0d3fdce7baac7c67ca38f305","t":["embedding"],"mx":384},{"n":"ibm-granite/granite-embedding-278m-multilingual:1f76d42a05f120e12272746d5a2d86b525c13420773f795a4cbef9117d8685f1","t":["embedding"],"mx":512}]},{"name":"Tencent Hunyuan","rank":null,"llm":[{"n":"hunyuan-pro","t":["chat"],"mx":32768},{"n":"hunyuan-standard","t":["chat"],"mx":32768},{"n":"hunyuan-standard-256K","t":["chat"],"mx":262144},{"n":"hunyuan-lite","t":["chat"],"mx":262144},{"n":"hunyuan-vision","t":["image2text","chat"],"mx":8192}]},{"name":"XunFei Spark","rank":null,"llm":[{"n":"Spark-Max","t":["chat"],"mx":8192},{"n":"Spark-Max-32K","t":["chat"],"mx":32768},{"n":"Spark-Lite","t":["chat"],"mx":8192},{"n":"Spark-Pro","t":["chat"],"mx":8192},{"n":"Spark-Pro-128K","t":["chat"],"mx":131072},{"n":"Spark-4.0-Ultra","t":["chat"],"mx":131072}]},{"name":"BaiduYiyan","rank":null,"llm":[]},{"name":"Fish Audio","rank":null,"llm":[]},{"name":"Tencent Cloud","rank":null,"llm":[]},{"name":"Anthropic","rank":"998","llm":[{"n":"claude-opus-4-8","t":["chat"],"mx":204800},{"n":"claude-opus-4-7","t":["chat"],"mx":204800},{"n":"claude-opus-4-6","t":["chat"],"mx":204800},{"n":"claude-opus-4-5-20251101","t":["chat"],"mx":204800},{"n":"claude-opus-4-1-20250805","t":["chat"],"mx":204800},{"n":"claude-opus-4-20250514","t":["chat"],"mx":204800},{"n":"claude-sonnet-4-6","t":["chat"],"mx":204800},{"n":"claude-sonnet-4-5-20250929","t":["chat"],"mx":204800},{"n":"claude-sonnet-4-20250514","t":["chat"],"mx":204800},{"n":"claude-3-7-sonnet-20250219","t":["chat"],"mx":204800},{"n":"claude-3-5-sonnet-20241022","t":["chat"],"mx":204800},{"n":"claude-3-5-haiku-20241022","t":["chat"],"mx":204800},{"n":"claude-3-haiku-20240307","t":["chat"],"mx":204800}]},{"name":"Voyage AI","rank":null,"llm":[{"n":"voyage-4-large","t":["embedding"],"mx":32000},{"n":"voyage-4","t":["embedding"],"mx":32000},{"n":"voyage-4-lite","t":["embedding"],"mx":32000},{"n":"voyage-3-large","t":["embedding"],"mx":32000},{"n":"voyage-3.5","t":["embedding"],"mx":32000},{"n":"voyage-3.5-lite","t":["embedding"],"mx":32000},{"n":"voyage-code-3","t":["embedding"],"mx":32000},{"n":"voyage-multimodal-3","t":["embedding"],"mx":32000},{"n":"voyage-large-2-instruct","t":["embedding"],"mx":16000},{"n":"voyage-finance-2","t":["embedding"],"mx":32000},{"n":"voyage-multilingual-2","t":["embedding"],"mx":32000},{"n":"voyage-law-2","t":["embedding"],"mx":16000},{"n":"voyage-code-2","t":["embedding"],"mx":16000},{"n":"voyage-large-2","t":["embedding"],"mx":16000},{"n":"voyage-2","t":["embedding"],"mx":4000},{"n":"voyage-3","t":["embedding"],"mx":32000},{"n":"voyage-3-lite","t":["embedding"],"mx":32000},{"n":"rerank-1","t":["rerank"],"mx":8000},{"n":"rerank-lite-1","t":["rerank"],"mx":4000},{"n":"rerank-2.5","t":["rerank"],"mx":32000},{"n":"rerank-2.5-lite","t":["rerank"],"mx":32000},{"n":"rerank-2","t":["rerank"],"mx":16000},{"n":"rerank-2-lite","t":["rerank"],"mx":8000}]},{"name":"GiteeAI","rank":null,"llm":[{"n":"ERNIE-4.5-Turbo","t":["chat"],"mx":32768},{"n":"ERNIE-X1-Turbo","t":["chat"],"mx":4096},{"n":"DeepSeek-R1","t":["chat"],"mx":65792},{"n":"DeepSeek-V3","t":["chat"],"mx":65792},{"n":"Qwen3-235B-A22B","t":["chat"],"mx":128000},{"n":"Qwen3-30B-A3B","t":["chat"],"mx":128000},{"n":"Qwen3-32B","t":["chat"],"mx":128000},{"n":"Qwen3-8B","t":["chat"],"mx":128000},{"n":"Qwen3-4B","t":["chat"],"mx":128000},{"n":"Qwen3-0.6B","t":["chat"],"mx":32000},{"n":"QwQ-32B","t":["chat"],"mx":131072},{"n":"DeepSeek-R1-Distill-Qwen-32B","t":["chat"],"mx":65792},{"n":"DeepSeek-R1-Distill-Qwen-14B","t":["chat"],"mx":65792},{"n":"DeepSeek-R1-Distill-Qwen-1.5B","t":["chat"],"mx":65792},{"n":"Qwen2.5-72B-Instruct","t":["chat"],"mx":4096},{"n":"Qwen2.5-32B-Instruct","t":["chat"],"mx":4096},{"n":"Qwen2.5-14B-Instruct","t":["chat"],"mx":4096},{"n":"Qwen2.5-7B-Instruct","t":["chat"],"mx":131072},{"n":"Qwen2-72B-Instruct","t":["chat"],"mx":131072},{"n":"Qwen2-7B-Instruct","t":["chat"],"mx":131072},{"n":"GLM-4-32B","t":["chat"],"mx":128000},{"n":"GLM-4-9B-0414","t":["chat"],"mx":128000},{"n":"glm-4-9b-chat","t":["chat"],"mx":128000},{"n":"internlm3-8b-instruct","t":["chat"],"mx":4096},{"n":"Yi-34B-Chat","t":["chat"],"mx":32768},{"n":"ERNIE-4.5-Turbo-VL","t":["image2text","chat"],"mx":4096},{"n":"Qwen2.5-VL-32B-Instruct","t":["image2text","chat"],"mx":32768},{"n":"Qwen2-VL-72B","t":["image2text","chat"],"mx":4096},{"n":"Align-DS-V","t":["image2text","chat"],"mx":4096},{"n":"InternVL3-78B","t":["image2text","chat"],"mx":32768},{"n":"InternVL3-38B","t":["image2text","chat"],"mx":32768},{"n":"InternVL2.5-78B","t":["image2text","chat"],"mx":32768},{"n":"InternVL2.5-26B","t":["image2text","chat"],"mx":16384},{"n":"InternVL2-8B","t":["image2text","chat"],"mx":8192},{"n":"Qwen2-Audio-7B-Instruct","t":["speech2text"],"mx":8192},{"n":"whisper-base","t":["speech2text"],"mx":512},{"n":"whisper-large","t":["speech2text"],"mx":512},{"n":"whisper-large-v3-turbo","t":["speech2text"],"mx":512},{"n":"whisper-large-v3","t":["speech2text"],"mx":512},{"n":"SenseVoiceSmall","t":["speech2text"],"mx":512},{"n":"Qwen3-Reranker-8B","t":["rerank"],"mx":32768},{"n":"Qwen3-Reranker-4B","t":["rerank"],"mx":32768},{"n":"Qwen3-Reranker-0.6B","t":["rerank"],"mx":32768},{"n":"Qwen3-Embedding-8B","t":["embedding"],"mx":8192},{"n":"Qwen3-Embedding-4B","t":["embedding"],"mx":4096},{"n":"Qwen3-Embedding-0.6B","t":["embedding"],"mx":4096},{"n":"jina-clip-v1","t":["embedding"],"mx":512},{"n":"jina-clip-v2","t":["embedding"],"mx":8192},{"n":"jina-reranker-m0","t":["rerank"],"mx":10240},{"n":"bce-embedding-base_v1","t":["embedding"],"mx":512},{"n":"bce-reranker-base_v1","t":["rerank"],"mx":512},{"n":"bge-m3","t":["embedding"],"mx":8192},{"n":"bge-reranker-v2-m3","t":["rerank"],"mx":8192},{"n":"bge-large-zh-v1.5","t":["embedding"],"mx":1024},{"n":"bge-small-zh-v1.5","t":["embedding"],"mx":512},{"n":"nomic-embed-code","t":["embedding"],"mx":512},{"n":"all-mpnet-base-v2","t":["embedding"],"mx":512}]},{"name":"Google Cloud","rank":null,"llm":[]},{"name":"HuggingFace","rank":"991","llm":[]},{"name":"GPUStack","rank":null,"llm":[]},{"name":"DeepInfra","rank":null,"llm":[{"n":"moonshotai/Kimi-K2-Instruct","t":["chat"],"mx":0},{"n":"mistralai/Voxtral-Small-24B-2507","t":["speech2text"],"mx":0},{"n":"mistralai/Voxtral-Mini-3B-2507","t":["speech2text"],"mx":0},{"n":"deepseek-ai/DeepSeek-R1-0528-Turbo","t":["chat"],"mx":0},{"n":"Qwen/Qwen3-235B-A22B","t":["chat"],"mx":0},{"n":"Qwen/Qwen3-30B-A3B","t":["chat"],"mx":0},{"n":"Qwen/Qwen3-32B","t":["chat"],"mx":0},{"n":"Qwen/Qwen3-14B","t":["chat"],"mx":0},{"n":"deepseek-ai/DeepSeek-V3-0324-Turbo","t":["chat"],"mx":0},{"n":"meta-llama/Llama-4-Maverick-17B-128E-Instruct-Turbo","t":["chat"],"mx":0},{"n":"meta-llama/Llama-4-Maverick-17B-128E-Instruct-FP8","t":["chat"],"mx":0},{"n":"meta-llama/Llama-4-Scout-17B-16E-Instruct","t":["chat"],"mx":0},{"n":"deepseek-ai/DeepSeek-R1-0528","t":["chat"],"mx":0},{"n":"deepseek-ai/DeepSeek-V3-0324","t":["chat"],"mx":0},{"n":"mistralai/Devstral-Small-2507","t":["chat"],"mx":0},{"n":"mistralai/Mistral-Small-3.2-24B-Instruct-2506","t":["chat"],"mx":0},{"n":"meta-llama/Llama-Guard-4-12B","t":["chat"],"mx":0},{"n":"Qwen/QwQ-32B","t":["chat"],"mx":0},{"n":"anthropic/claude-4-opus","t":["chat"],"mx":0},{"n":"anthropic/claude-4-sonnet","t":["chat"],"mx":0},{"n":"google/gemini-2.5-flash","t":["chat"],"mx":0},{"n":"google/gemini-2.5-pro","t":["chat"],"mx":0},{"n":"google/gemma-3-27b-it","t":["chat"],"mx":0},{"n":"google/gemma-3-12b-it","t":["chat"],"mx":0},{"n":"google/gemma-3-4b-it","t":["chat"],"mx":0},{"n":"hexgrad/Kokoro-82M","t":["tts"],"mx":0},{"n":"canopylabs/orpheus-3b-0.1-ft","t":["tts"],"mx":0},{"n":"sesame/csm-1b","t":["tts"],"mx":0},{"n":"microsoft/Phi-4-multimodal-instruct","t":["chat"],"mx":0},{"n":"deepseek-ai/DeepSeek-R1-Distill-Llama-70B","t":["chat"],"mx":0},{"n":"deepseek-ai/DeepSeek-V3","t":["chat"],"mx":0},{"n":"meta-llama/Llama-3.3-70B-Instruct-Turbo","t":["chat"],"mx":0},{"n":"meta-llama/Llama-3.3-70B-Instruct","t":["chat"],"mx":0},{"n":"microsoft/phi-4","t":["chat"],"mx":0},{"n":"openai/whisper-large-v3-turbo","t":["speech2text"],"mx":0},{"n":"BAAI/bge-base-en-v1.5","t":["embedding"],"mx":0},{"n":"BAAI/bge-en-icl","t":["embedding"],"mx":0},{"n":"BAAI/bge-large-en-v1.5","t":["embedding"],"mx":0},{"n":"BAAI/bge-m3","t":["embedding"],"mx":0},{"n":"BAAI/bge-m3-multi","t":["embedding"],"mx":0},{"n":"Qwen/Qwen3-Embedding-0.6B","t":["embedding"],"mx":0},{"n":"Qwen/Qwen3-Embedding-4B","t":["embedding"],"mx":0},{"n":"Qwen/Qwen3-Embedding-8B","t":["embedding"],"mx":0},{"n":"intfloat/e5-base-v2","t":["embedding"],"mx":0},{"n":"intfloat/e5-large-v2","t":["embedding"],"mx":0},{"n":"intfloat/multilingual-e5-large","t":["embedding"],"mx":0},{"n":"intfloat/multilingual-e5-large-instruct","t":["embedding"],"mx":0},{"n":"sentence-transformers/all-MiniLM-L12-v2","t":["embedding"],"mx":0},{"n":"sentence-transformers/all-MiniLM-L6-v2","t":["embedding"],"mx":0},{"n":"sentence-transformers/all-mpnet-base-v2","t":["embedding"],"mx":0},{"n":"sentence-transformers/clip-ViT-B-32","t":["embedding"],"mx":0},{"n":"sentence-transformers/clip-ViT-B-32-multilingual-v1","t":["embedding"],"mx":0},{"n":"sentence-transformers/multi-qa-mpnet-base-dot-v1","t":["embedding"],"mx":0},{"n":"sentence-transformers/paraphrase-MiniLM-L6-v2","t":["embedding"],"mx":0},{"n":"shibing624/text2vec-base-chinese","t":["embedding"],"mx":0},{"n":"thenlper/gte-base","t":["embedding"],"mx":0},{"n":"thenlper/gte-large","t":["embedding"],"mx":0}]},{"name":"302.AI","rank":null,"llm":[{"n":"deepseek-chat","t":["chat"],"mx":32000},{"n":"gpt-4o","t":["chat"],"mx":128000},{"n":"chatgpt-4o-latest","t":["chat"],"mx":128000},{"n":"llama3.3-70b","t":["chat"],"mx":128000},{"n":"deepseek-reasoner","t":["chat"],"mx":64000},{"n":"gemini-2.0-flash","t":["image2text","chat"],"mx":1000000},{"n":"claude-3-7-sonnet-20250219","t":["chat"],"mx":200000},{"n":"claude-3-7-sonnet-latest","t":["chat"],"mx":200000},{"n":"grok-3-beta","t":["chat"],"mx":131072},{"n":"grok-3-mini-beta","t":["chat"],"mx":131072},{"n":"gpt-4.1","t":["chat"],"mx":1000000},{"n":"o3","t":["chat"],"mx":200000},{"n":"o4-mini","t":["chat"],"mx":200000},{"n":"qwen3-235b-a22b","t":["chat"],"mx":128000},{"n":"qwen3-32b","t":["chat"],"mx":128000},{"n":"gemini-2.5-pro-preview-05-06","t":["chat"],"mx":1000000},{"n":"llama-4-maverick","t":["chat"],"mx":128000},{"n":"gemini-2.5-flash","t":["chat"],"mx":1000000},{"n":"claude-sonnet-4-20250514","t":["chat"],"mx":200000},{"n":"claude-opus-4-20250514","t":["image2text","chat"],"mx":200000},{"n":"gemini-2.5-pro","t":["image2text","chat"],"mx":1000000},{"n":"jina-clip-v2","t":["embedding"],"mx":8192},{"n":"jina-reranker-m0","t":["rerank"],"mx":10240}]},{"name":"CometAPI","rank":null,"llm":[{"n":"gpt-5-chat-latest","t":["chat"],"mx":400000},{"n":"chatgpt-4o-latest","t":["chat"],"mx":128000},{"n":"gpt-5-mini","t":["chat"],"mx":400000},{"n":"gpt-5-nano","t":["chat"],"mx":400000},{"n":"gpt-5","t":["chat"],"mx":400000},{"n":"gpt-4.1-mini","t":["chat"],"mx":1047576},{"n":"gpt-4.1-nano","t":["chat"],"mx":1047576},{"n":"gpt-4.1","t":["chat"],"mx":1047576},{"n":"gpt-4o-mini","t":["chat"],"mx":128000},{"n":"o4-mini-2025-04-16","t":["chat"],"mx":200000},{"n":"o3-pro-2025-06-10","t":["chat"],"mx":200000},{"n":"claude-opus-4-1-20250805","t":["image2text","chat"],"mx":200000},{"n":"claude-opus-4-1-20250805-thinking","t":["image2text","chat"],"mx":200000},{"n":"claude-sonnet-4-20250514","t":["image2text","chat"],"mx":200000},{"n":"claude-sonnet-4-20250514-thinking","t":["image2text","chat"],"mx":200000},{"n":"claude-3-7-sonnet-latest","t":["chat"],"mx":200000},{"n":"claude-3-5-haiku-latest","t":["chat"],"mx":200000},{"n":"gemini-2.5-pro","t":["image2text","chat"],"mx":1000000},{"n":"gemini-2.5-flash","t":["image2text","chat"],"mx":1000000},{"n":"gemini-2.5-flash-lite","t":["image2text","chat"],"mx":1000000},{"n":"gemini-2.0-flash","t":["image2text","chat"],"mx":1000000},{"n":"grok-4-0709","t":["chat"],"mx":131072},{"n":"grok-3","t":["chat"],"mx":131072},{"n":"grok-3-mini","t":["chat"],"mx":131072},{"n":"grok-2-image-1212","t":["image2text","chat"],"mx":32768},{"n":"deepseek-v3.1","t":["chat"],"mx":64000},{"n":"deepseek-v3","t":["chat"],"mx":64000},{"n":"deepseek-r1-0528","t":["chat"],"mx":164000},{"n":"deepseek-chat","t":["chat"],"mx":32000},{"n":"deepseek-reasoner","t":["chat"],"mx":64000},{"n":"qwen3-30b-a3b","t":["chat"],"mx":128000},{"n":"qwen3-coder-plus-2025-07-22","t":["chat"],"mx":128000},{"n":"text-embedding-ada-002","t":["embedding"],"mx":8191},{"n":"text-embedding-3-small","t":["embedding"],"mx":8191},{"n":"text-embedding-3-large","t":["embedding"],"mx":8191},{"n":"whisper-1","t":["speech2text"],"mx":26214400},{"n":"tts-1","t":["tts"],"mx":2048}]},{"name":"LongCat","rank":null,"llm":[{"n":"LongCat-Flash-Chat","t":["chat"],"mx":8000},{"n":"LongCat-Flash-Thinking","t":["chat"],"mx":8000}]},{"name":"DeerAPI","rank":null,"llm":[{"n":"gpt-5-chat-latest","t":["chat"],"mx":400000},{"n":"chatgpt-4o-latest","t":["chat"],"mx":128000},{"n":"gpt-5-mini","t":["chat"],"mx":400000},{"n":"gpt-5-nano","t":["chat"],"mx":400000},{"n":"gpt-5","t":["chat"],"mx":400000},{"n":"gpt-4.1-mini","t":["chat"],"mx":1047576},{"n":"gpt-4.1-nano","t":["chat"],"mx":1047576},{"n":"gpt-4.1","t":["chat"],"mx":1047576},{"n":"gpt-4o-mini","t":["chat"],"mx":128000},{"n":"o4-mini-2025-04-16","t":["chat"],"mx":200000},{"n":"o3-pro-2025-06-10","t":["chat"],"mx":200000},{"n":"claude-opus-4-1-20250805","t":["image2text","chat"],"mx":200000},{"n":"claude-opus-4-1-20250805-thinking","t":["image2text","chat"],"mx":200000},{"n":"claude-sonnet-4-20250514","t":["image2text","chat"],"mx":200000},{"n":"claude-sonnet-4-20250514-thinking","t":["image2text","chat"],"mx":200000},{"n":"claude-3-7-sonnet-latest","t":["chat"],"mx":200000},{"n":"claude-3-5-haiku-latest","t":["chat"],"mx":200000},{"n":"gemini-2.5-pro","t":["image2text","chat"],"mx":1000000},{"n":"gemini-2.5-flash","t":["image2text","chat"],"mx":1000000},{"n":"gemini-2.5-flash-lite","t":["image2text","chat"],"mx":1000000},{"n":"gemini-2.0-flash","t":["image2text","chat"],"mx":1000000},{"n":"grok-4-0709","t":["chat"],"mx":131072},{"n":"grok-3","t":["chat"],"mx":131072},{"n":"grok-3-mini","t":["chat"],"mx":131072},{"n":"grok-2-image-1212","t":["image2text","chat"],"mx":32768},{"n":"deepseek-v3.1","t":["chat"],"mx":64000},{"n":"deepseek-v3","t":["chat"],"mx":64000},{"n":"deepseek-r1-0528","t":["chat"],"mx":164000},{"n":"deepseek-chat","t":["chat"],"mx":32000},{"n":"deepseek-reasoner","t":["chat"],"mx":64000},{"n":"qwen3-30b-a3b","t":["chat"],"mx":128000},{"n":"qwen3-coder-plus-2025-07-22","t":["chat"],"mx":128000},{"n":"text-embedding-ada-002","t":["embedding"],"mx":8191},{"n":"text-embedding-3-small","t":["embedding"],"mx":8191},{"n":"text-embedding-3-large","t":["embedding"],"mx":8191},{"n":"whisper-1","t":["speech2text"],"mx":26214400},{"n":"tts-1","t":["tts"],"mx":2048}]},{"name":"Jiekou.AI","rank":null,"llm":[{"n":"Sao10K/L3-8B-Stheno-v3.2","t":["chat"],"mx":8192},{"n":"baichuan/baichuan-m2-32b","t":["chat"],"mx":131072},{"n":"baidu/ernie-4.5-300b-a47b-paddle","t":["chat"],"mx":123000},{"n":"baidu/ernie-4.5-vl-424b-a47b","t":["chat"],"mx":123000},{"n":"claude-3-5-haiku-20241022","t":["chat"],"mx":200000},{"n":"claude-3-5-sonnet-20241022","t":["chat"],"mx":200000},{"n":"claude-3-7-sonnet-20250219","t":["chat"],"mx":200000},{"n":"claude-3-haiku-20240307","t":["chat"],"mx":200000},{"n":"claude-haiku-4-5-20251001","t":["image2text","chat"],"mx":20000},{"n":"claude-opus-4-1-20250805","t":["chat"],"mx":200000},{"n":"claude-opus-4-20250514","t":["chat"],"mx":200000},{"n":"claude-sonnet-4-20250514","t":["chat"],"mx":200000},{"n":"claude-sonnet-4-5-20250929","t":["image2text","chat"],"mx":200000},{"n":"deepseek/deepseek-r1-0528","t":["chat"],"mx":163840},{"n":"deepseek/deepseek-v3-0324","t":["chat"],"mx":163840},{"n":"deepseek/deepseek-v3.1","t":["chat"],"mx":163840},{"n":"doubao-1-5-pro-32k-250115","t":["chat"],"mx":128000},{"n":"doubao-1.5-pro-32k-character-250715","t":["chat"],"mx":200000},{"n":"gemini-2.0-flash-20250609","t":["chat"],"mx":1048576},{"n":"gemini-2.0-flash-lite","t":["chat"],"mx":1048576},{"n":"gemini-2.5-flash","t":["chat"],"mx":1048576},{"n":"gemini-2.5-flash-lite","t":["chat"],"mx":1048576},{"n":"gemini-2.5-flash-lite-preview-06-17","t":["chat"],"mx":1048576},{"n":"gemini-2.5-flash-lite-preview-09-2025","t":["image2text","chat"],"mx":1048576},{"n":"gemini-2.5-flash-preview-05-20","t":["chat"],"mx":1048576},{"n":"gemini-2.5-pro","t":["chat"],"mx":1048576},{"n":"gemini-2.5-pro-preview-06-05","t":["chat"],"mx":1048576},{"n":"google/gemma-3-12b-it","t":["chat"],"mx":131072},{"n":"google/gemma-3-27b-it","t":["chat"],"mx":32768},{"n":"gpt-4.1","t":["chat"],"mx":1047576},{"n":"gpt-4.1-mini","t":["chat"],"mx":1047576},{"n":"gpt-4.1-nano","t":["chat"],"mx":1047576},{"n":"gpt-4o","t":["chat"],"mx":131072},{"n":"gpt-4o-mini","t":["chat"],"mx":131072},{"n":"gpt-5","t":["chat"],"mx":400000},{"n":"gpt-5-chat-latest","t":["chat"],"mx":400000},{"n":"gpt-5-codex","t":["image2text","chat"],"mx":400000},{"n":"gpt-5-mini","t":["chat"],"mx":400000},{"n":"gpt-5-nano","t":["chat"],"mx":400000},{"n":"gpt-5-pro","t":["image2text","chat"],"mx":400000},{"n":"gpt-5.1","t":["chat"],"mx":400000},{"n":"gpt-5.1-chat-latest","t":["chat"],"mx":128000},{"n":"gpt-5.1-codex","t":["chat"],"mx":400000},{"n":"grok-3","t":["chat"],"mx":131072},{"n":"grok-3-mini","t":["chat"],"mx":131072},{"n":"grok-4-0709","t":["chat"],"mx":256000},{"n":"grok-4-fast-non-reasoning","t":["image2text","chat"],"mx":2000000},{"n":"grok-4-fast-reasoning","t":["image2text","chat"],"mx":2000000},{"n":"grok-code-fast-1","t":["chat"],"mx":256000},{"n":"gryphe/mythomax-l2-13b","t":["chat"],"mx":4096},{"n":"meta-llama/llama-3.1-8b-instruct","t":["chat"],"mx":16384},{"n":"meta-llama/llama-3.2-3b-instruct","t":["chat"],"mx":32768},{"n":"meta-llama/llama-3.3-70b-instruct","t":["chat"],"mx":131072},{"n":"meta-llama/llama-4-maverick-17b-128e-instruct-fp8","t":["chat"],"mx":1048576},{"n":"meta-llama/llama-4-scout-17b-16e-instruct","t":["chat"],"mx":131072},{"n":"minimaxai/minimax-m1-80k","t":["chat"],"mx":1000000},{"n":"mistralai/mistral-7b-instruct","t":["chat"],"mx":32768},{"n":"mistralai/mistral-nemo","t":["chat"],"mx":60288},{"n":"moonshotai/kimi-k2-0905","t":["chat"],"mx":262144},{"n":"moonshotai/kimi-k2-instruct","t":["chat"],"mx":131072},{"n":"o1","t":["chat"],"mx":131072},{"n":"o1-mini","t":["chat"],"mx":131072},{"n":"o3","t":["chat"],"mx":131072},{"n":"o3-mini","t":["chat"],"mx":131072},{"n":"openai/gpt-oss-120b","t":["chat"],"mx":131072},{"n":"openai/gpt-oss-20b","t":["chat"],"mx":131072},{"n":"qwen/qwen-2.5-72b-instruct","t":["chat"],"mx":32000},{"n":"qwen/qwen-mt-plus","t":["chat"],"mx":4096},{"n":"qwen/qwen2.5-7b-instruct","t":["chat"],"mx":32000},{"n":"qwen/qwen2.5-vl-72b-instruct","t":["chat"],"mx":32768},{"n":"qwen/qwen3-235b-a22b-fp8","t":["chat"],"mx":40960},{"n":"qwen/qwen3-235b-a22b-instruct-2507","t":["chat"],"mx":131072},{"n":"qwen/qwen3-235b-a22b-thinking-2507","t":["chat"],"mx":131072},{"n":"qwen/qwen3-30b-a3b-fp8","t":["chat"],"mx":40960},{"n":"qwen/qwen3-32b-fp8","t":["chat"],"mx":40960},{"n":"qwen/qwen3-8b-fp8","t":["chat"],"mx":128000},{"n":"qwen/qwen3-coder-480b-a35b-instruct","t":["chat"],"mx":262144},{"n":"qwen/qwen3-next-80b-a3b-instruct","t":["chat"],"mx":65536},{"n":"qwen/qwen3-next-80b-a3b-thinking","t":["chat"],"mx":65536},{"n":"sao10k/l3-70b-euryale-v2.1","t":["chat"],"mx":8192},{"n":"sao10k/l3-8b-lunaris","t":["chat"],"mx":8192},{"n":"sao10k/l31-70b-euryale-v2.2","t":["chat"],"mx":8192},{"n":"thudm/glm-4.1v-9b-thinking","t":["chat"],"mx":65536},{"n":"zai-org/glm-4.5","t":["chat"],"mx":131072},{"n":"zai-org/glm-4.5v","t":["chat"],"mx":65536},{"n":"baai/bge-m3","t":["embedding"],"mx":8192},{"n":"qwen/qwen3-embedding-0.6b","t":["embedding"],"mx":32768},{"n":"qwen/qwen3-embedding-8b","t":["embedding"],"mx":32768},{"n":"baai/bge-reranker-v2-m3","t":["rerank"],"mx":8000},{"n":"qwen/qwen3-reranker-8b","t":["rerank"],"mx":32768}]},{"name":"MinerU","rank":null,"llm":[]},{"name":"PaddleOCR","rank":null,"llm":[]},{"name":"OpenDataLoader","rank":null,"llm":[]},{"name":"SoMark","rank":"930","llm":[]},{"name":"n1n","rank":null,"llm":[{"n":"gpt-4o-mini","t":["chat"],"mx":128000},{"n":"gpt-4o","t":["chat"],"mx":128000},{"n":"gpt-3.5-turbo","t":["chat"],"mx":4096},{"n":"deepseek-chat","t":["chat"],"mx":128000}]},{"name":"Astraflow","rank":null,"llm":[{"n":"claude-opus-4-7","t":["chat"],"mx":200000},{"n":"claude-opus-4-6","t":["chat"],"mx":200000},{"n":"claude-sonnet-4-5-20250929","t":["chat"],"mx":200000},{"n":"claude-haiku-4-5-20251001","t":["chat"],"mx":200000},{"n":"gpt-5.4","t":["chat"],"mx":400000},{"n":"gpt-5.4-mini","t":["chat"],"mx":400000},{"n":"gpt-5.4-nano","t":["chat"],"mx":400000},{"n":"gpt-4o-mini","t":["chat"],"mx":128000},{"n":"Qwen/Qwen3-Max","t":["chat"],"mx":131072},{"n":"Qwen/Qwen3-Coder","t":["chat"],"mx":131072},{"n":"Qwen/Qwen3-32B","t":["chat"],"mx":131072},{"n":"Qwen/Qwen3-VL-235B-A22B-Instruct","t":["chat"],"mx":131072},{"n":"kimi-k2.6","t":["chat"],"mx":200000},{"n":"glm-5.1","t":["chat"],"mx":128000},{"n":"MiniMax-M2.7","t":["chat"],"mx":1000000},{"n":"MiniMax-M2","t":["chat"],"mx":1000000},{"n":"gemini-2.5-pro","t":["chat"],"mx":1000000},{"n":"gemini-2.5-flash","t":["chat"],"mx":1000000},{"n":"qwen3-embedding-8b","t":["embedding"],"mx":8192},{"n":"text-embedding-3-large","t":["embedding"],"mx":8191},{"n":"text-embedding-ada-002","t":["embedding"],"mx":8191}]},{"name":"FuturMix","rank":null,"llm":[{"n":"claude-sonnet-4-20250514","t":["chat"],"mx":200000},{"n":"claude-3.5-haiku","t":["chat"],"mx":200000},{"n":"gpt-4o","t":["chat"],"mx":128000},{"n":"gpt-4o-mini","t":["chat"],"mx":128000},{"n":"gemini-2.5-flash","t":["chat"],"mx":1000000},{"n":"gemini-2.0-flash","t":["chat"],"mx":1000000},{"n":"deepseek-chat","t":["chat"],"mx":65536},{"n":"deepseek-reasoner","t":["chat"],"mx":65536},{"n":"gpt-4o","t":["image2text","chat"],"mx":128000},{"n":"text-embedding-3-small","t":["embedding"],"mx":8191},{"n":"text-embedding-3-large","t":["embedding"],"mx":8191},{"n":"tts-1","t":["tts"],"mx":4096},{"n":"tts-1-hd","t":["tts"],"mx":4096},{"n":"whisper-1","t":["speech2text"],"mx":25000000},{"n":"jina-reranker-v2-base-multilingual","t":["rerank"],"mx":8192}]},{"name":"Astraflow-CN","rank":null,"llm":[{"n":"claude-opus-4-7","t":["chat"],"mx":200000},{"n":"claude-opus-4-6","t":["chat"],"mx":200000},{"n":"claude-sonnet-4-5-20250929","t":["chat"],"mx":200000},{"n":"claude-haiku-4-5-20251001","t":["chat"],"mx":200000},{"n":"gpt-5.4","t":["chat"],"mx":400000},{"n":"gpt-5.4-mini","t":["chat"],"mx":400000},{"n":"gpt-5.4-nano","t":["chat"],"mx":400000},{"n":"gpt-4o-mini","t":["chat"],"mx":128000},{"n":"Qwen/Qwen3-Max","t":["chat"],"mx":131072},{"n":"Qwen/Qwen3-Coder","t":["chat"],"mx":131072},{"n":"Qwen/Qwen3-32B","t":["chat"],"mx":131072},{"n":"Qwen/Qwen3-VL-235B-A22B-Instruct","t":["chat"],"mx":131072},{"n":"kimi-k2.6","t":["chat"],"mx":200000},{"n":"glm-5.1","t":["chat"],"mx":128000},{"n":"MiniMax-M2.7","t":["chat"],"mx":1000000},{"n":"MiniMax-M2","t":["chat"],"mx":1000000},{"n":"gemini-2.5-pro","t":["chat"],"mx":1000000},{"n":"gemini-2.5-flash","t":["chat"],"mx":1000000},{"n":"qwen3-embedding-8b","t":["embedding"],"mx":8192},{"n":"text-embedding-3-large","t":["embedding"],"mx":8191},{"n":"text-embedding-ada-002","t":["embedding"],"mx":8191}]},{"name":"Avian","rank":null,"llm":[{"n":"deepseek/deepseek-v3.2","t":["chat"],"mx":164000},{"n":"moonshotai/kimi-k2.5","t":["chat"],"mx":131000},{"n":"z-ai/glm-5","t":["chat"],"mx":131000},{"n":"minimax/minimax-m2.5","t":["chat"],"mx":1000000}]},{"name":"RAGcon","rank":null,"llm":[]},{"name":"Xiaomi","rank":null,"llm":[{"n":"mimo-v2.5-pro","t":["chat"],"mx":1048576},{"n":"mimo-v2.5","t":["chat"],"mx":1048576},{"n":"mimo-v2-flash","t":["chat"],"mx":262144}]},{"name":"Perplexity","rank":null,"llm":[{"n":"pplx-embed-v1-0.6b","t":["embedding"],"mx":32000},{"n":"pplx-embed-v1-4b","t":["embedding"],"mx":32000},{"n":"pplx-embed-context-v1-0.6b","t":["embedding"],"mx":32000},{"n":"pplx-embed-context-v1-4b","t":["embedding"],"mx":32000}]},{"name":"New API","rank":"885","llm":[]}]"#;


// ═══════════════════════════════════════════════════════════════════════════
// RAGFlow models_api_service.py port — per-capability default models and the
// added-models ranking/factory fallback.
//
// Reference: api/apps/services/models_api_service.py (RAGFlow v0.26.4,
// commit cb93883f3f8c975eecb2fed81210effeb3bdb06f). Python semantics notes:
//   * `MODEL_TYPE_TO_FIELD` — API tag → Tenant column; the dict insertion
//     order (chat, embedding, rerank, asr, vision, tts, ocr) drives the
//     default-model listing order.
//   * `MODEL_TAG_TO_TYPE` — API tag → raw capability tag
//     (vision→image2text, asr→speech2text); stored composites and factory
//     JSON use raw tags.
//   * `_to_int(v, 500)` — rank strings parse to int; anything unparseable
//     (missing / non-int) falls back to 500; the ranking key is NEGATED so
//     higher ranks sort first.
//   * `list_tenant_added_models` sorts by
//     `(factory_rank_mapping.get(name), provider_name, instance_name)`.
//     Upstream raises TypeError when a provider name is missing from the
//     factory map (None vs int), so RayRAG ranks unknown names with the
//     default rank (-500, sorts last) — a deterministic superset of upstream.
//   * `ensure_mineru/paddleocr/opendataloader_from_env` calls belong to the
//     joint-services layer (`api::joint_services`, tenant_model_service.py);
//     they are intentionally not part of this port.
// ═══════════════════════════════════════════════════════════════════════════

/// `models_api_service.py::MODEL_TYPE_TO_FIELD` — API model-type tag → RAGFlow
/// Tenant default-model column, in upstream dict insertion order.
pub const MODEL_TYPE_TO_FIELD: &[(&str, &str)] = &[
    ("chat", "llm_id"),
    ("embedding", "embd_id"),
    ("rerank", "rerank_id"),
    ("asr", "asr_id"),
    ("vision", "img2txt_id"),
    ("tts", "tts_id"),
    ("ocr", "ocr_id"),
];

/// Tenant default-model column for an API model-type tag.
pub fn model_type_field(tag: &str) -> Option<&'static str> {
    MODEL_TYPE_TO_FIELD
        .iter()
        .find(|(candidate, _)| *candidate == tag)
        .map(|(_, field)| *field)
}

/// `models_api_service.py::MODEL_TAG_TO_TYPE` — API tag → raw capability tag;
/// unknown tags pass through unchanged (dict `.get(key, key)` semantics).
pub fn model_tag_type(tag: &str) -> &str {
    match tag {
        "asr" => "speech2text",
        "vision" => "image2text",
        other => other,
    }
}

/// `models_api_service.py::_to_int` — parse an integer with a fallback.
fn to_int(value: Option<&str>, default: i64) -> i64 {
    value
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(default)
}

/// One `llm` row of `FACTORY_LLM_INFOS` (conf/llm_factories.json), keeping the
/// FULL model-type list (`model_type` may be a list upstream; the compiled
/// providers catalog only carries the first type).
#[derive(Debug, Clone)]
pub struct FactoryLlmRow {
    pub llm_name: String,
    pub model_types: Vec<String>,
    pub max_tokens: u64,
}

/// One factory row of `FACTORY_LLM_INFOS` (conf/llm_factories.json).
#[derive(Debug, Clone)]
pub struct FactoryLlmEntry {
    pub name: String,
    /// Upstream `rank` field (string, e.g. `"999"`); absent for unranked
    /// factories (e.g. `Builtin`).
    pub rank: Option<String>,
    pub llm: Vec<FactoryLlmRow>,
}

/// Parse the embedded `conf/llm_factories.json` catalog (66 factories, 1064
/// model rows) — the data source for factory fallback and ranking.
pub fn factory_llm_entries() -> &'static Vec<FactoryLlmEntry> {
    use std::sync::OnceLock;
    static ENTRIES: OnceLock<Vec<FactoryLlmEntry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let values: Vec<serde_json::Value> =
            serde_json::from_str(RAGFLOW_LLM_FACTORIES_JSON).unwrap_or_default();
        values
            .into_iter()
            .filter_map(|factory| {
                let name = factory.get("name")?.as_str()?.to_string();
                let rank = factory
                    .get("rank")
                    .and_then(|rank| rank.as_str())
                    .map(str::to_string);
                let llm = factory
                    .get("llm")
                    .and_then(|llm| llm.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|row| {
                        let llm_name = row.get("n")?.as_str()?.to_string();
                        let model_types = row
                            .get("t")
                            .and_then(|types| types.as_array())
                            .into_iter()
                            .flatten()
                            .filter_map(|value| value.as_str().map(str::to_string))
                            .collect();
                        let max_tokens = row.get("mx").and_then(|mx| mx.as_u64()).unwrap_or(0);
                        Some(FactoryLlmRow {
                            llm_name,
                            model_types,
                            max_tokens,
                        })
                    })
                    .collect();
                Some(FactoryLlmEntry { name, rank, llm })
            })
            .collect()
    })
}

/// `factory_rank_mapping` sort key: `-to_int(rank, 500)`; unknown provider
/// names rank with the default (sorts last).
fn factory_rank_key(provider_name: &str) -> i64 {
    let rank = factory_llm_entries()
        .iter()
        .find(|factory| factory.name == provider_name)
        .and_then(|factory| factory.rank.as_deref());
    -to_int(rank, 500)
}

/// Wire descriptor for `GET /models/default` (Python `_get_model_info`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DefaultModelDescriptor {
    pub model_provider: String,
    pub model_instance: String,
    pub model_name: String,
    /// Mapped capability tag (`MODEL_TAG_TO_TYPE`), e.g. `image2text` for a
    /// `vision` default.
    pub model_type: String,
    /// RayRAG wire extension: composite `{model_name}@{instance_name}@{provider_name}`.
    pub selector: String,
    pub enable: bool,
}

fn default_descriptor(
    provider: &str,
    instance: &str,
    model: &str,
    model_type: &str,
    selector: &str,
    enable: bool,
) -> DefaultModelDescriptor {
    DefaultModelDescriptor {
        model_provider: provider.to_string(),
        model_instance: instance.to_string(),
        model_name: model.to_string(),
        model_type: model_type.to_string(),
        selector: selector.to_string(),
        enable,
    }
}

/// Split the right-anchored composite `model@instance@provider` /
/// `model@provider` exactly like `models_api_service._get_model_info`
/// (`rsplit("@", 2)`). Unlike [`split_composite_model_selector`], an empty
/// provider is representable (1-part selectors), because the TEI builtin
/// branch accepts a missing provider.
fn split_model_default(selector: &str) -> (&str, &str, &str) {
    let mut parts = selector.rsplitn(3, '@');
    let provider = parts.next().unwrap_or_default();
    let second = parts.next();
    let third = parts.next();
    match (second, third) {
        (Some(instance), Some(model)) => (model, instance, provider),
        (Some(model), None) => (model, "default", provider),
        (None, _) => (provider, "default", ""),
    }
}

/// `models_api_service._get_model_info` — resolve a persisted composite
/// default-model selector into a descriptor, or `None` when the provider,
/// instance, or model is unavailable. Pure over injected inputs so the TEI
/// env-gated branch is testable without process env mutation.
pub fn get_model_info(
    providers: &[crate::api::features::Provider],
    instances: &[TenantModelInstance],
    factory: &[FactoryLlmEntry],
    default_model: &str,
    model_type: &str,
    compose_profiles: &str,
    tei_model: &str,
) -> Option<DefaultModelDescriptor> {
    if default_model.is_empty() {
        return None;
    }
    let (model_name, instance_name, provider_name) = split_model_default(default_model);
    let mapped = model_tag_type(model_type);
    if mapped == "ocr"
        && provider_name == "infiniflow"
        && instance_name == "default"
        && model_name == "deepdoc"
    {
        return Some(default_descriptor(
            provider_name,
            instance_name,
            model_name,
            mapped,
            default_model,
            true,
        ));
    }
    if mapped == "embedding"
        && compose_profiles.contains("tei-")
        && !tei_model.is_empty()
        && model_name == tei_model
        && (provider_name.is_empty() || provider_name == "Builtin")
    {
        return Some(default_descriptor(
            "Builtin",
            "default",
            model_name,
            mapped,
            default_model,
            true,
        ));
    }
    let provider = providers
        .iter()
        .filter(|provider| provider.name == provider_name)
        .max_by_key(|provider| provider.enabled)?;
    let instance = instances.iter().find(|instance| {
        instance.provider_id == provider.id && instance.instance_name == instance_name
    })?;
    let entity = instance.models.iter().find(|model| {
        model.name == model_name
            && model
                .model_types
                .iter()
                .any(|capability| capability.as_str() == mapped)
    });
    if let Some(entity) = entity {
        if !entity.enabled {
            return None;
        }
        return Some(default_descriptor(
            provider_name,
            instance_name,
            model_name,
            mapped,
            default_model,
            true,
        ));
    }
    let row = factory
        .iter()
        .find(|entry| entry.name == provider_name)
        .and_then(|entry| entry.llm.iter().find(|row| row.llm_name == model_name))?;
    if !row.model_types.iter().any(|capability| capability == mapped) {
        return None;
    }
    Some(default_descriptor(
        provider_name,
        instance_name,
        model_name,
        mapped,
        default_model,
        true,
    ))
}

/// `models_api_service._check_model_available` — validate that a provider /
/// instance / model triple is usable as a tenant default. Error strings match
/// upstream verbatim.
pub fn check_model_available(
    providers: &[crate::api::features::Provider],
    instances: &[TenantModelInstance],
    factory: &[FactoryLlmEntry],
    provider_name: &str,
    instance_name: &str,
    model_name: &str,
    model_type: &str,
    compose_profiles: &str,
    tei_model: &str,
) -> Result<(), String> {
    if provider_name == "infiniflow" && instance_name == "default" && model_name == "deepdoc" {
        return Ok(());
    }
    if model_type == "ocr"
        && provider_name == "infiniflow"
        && instance_name == "default"
        && model_name == "deepdoc"
    {
        return Ok(());
    }
    if model_type == "embedding"
        && compose_profiles.contains("tei-")
        && model_name == tei_model
        && (provider_name == "Builtin" || provider_name.is_empty())
    {
        return Ok(());
    }
    let provider = providers
        .iter()
        .filter(|provider| provider.name == provider_name)
        .max_by_key(|provider| provider.enabled);
    let Some(provider) = provider else {
        return Err(format!("Provider '{provider_name}' not found"));
    };
    let instance = instances.iter().find(|instance| {
        instance.provider_id == provider.id && instance.instance_name == instance_name
    });
    let Some(instance) = instance else {
        return Err(format!(
            "Instance '{instance_name}' not found for provider '{provider_name}'"
        ));
    };
    let Some(factory_entry) = factory.iter().find(|entry| entry.name == provider_name) else {
        return Err(format!("Provider '{provider_name}' not found in factory info"));
    };
    let mapped = model_tag_type(model_type);
    let entity = instance.models.iter().find(|model| {
        model.name == model_name
            && model
                .model_types
                .iter()
                .any(|capability| capability.as_str() == mapped)
    });
    if let Some(entity) = entity {
        if !entity.enabled {
            return Err(format!("Model '{model_name}' isn't available"));
        }
        return Ok(());
    }
    let row = factory_entry
        .llm
        .iter()
        .find(|row| row.llm_name == model_name);
    let Some(row) = row else {
        return Err(format!(
            "Model '{model_name}' not found for provider '{provider_name}'"
        ));
    };
    if !row.model_types.iter().any(|capability| capability == mapped) {
        return Err(format!("Model '{model_name}' isn't a {mapped} model"));
    }
    Ok(())
}

/// RAGFlow `models_api_service` store-level port — per-capability default
/// models and the added-model ranking/factory fallback.
impl TenantModelStore {
    /// Composite default selector for a non-chat capability (`None` unset).
    pub fn default_capability_model(&self, tenant_id: &str, tag: &str) -> Option<String> {
        self.default_capability_models
            .read()
            .unwrap()
            .get(tenant_id)
            .and_then(|models| models.get(tag))
            .cloned()
    }

    /// `models_api_service.set_tenant_default_models` — set or clear a tenant
    /// default model for one capability. An all-empty triple clears the
    /// default; a partial triple is an error; a full triple is validated via
    /// [`check_model_available`] before persisting the composite
    /// `{model_name}@{instance_name}@{provider_name}`. The `chat` capability
    /// routes through the pre-existing RayRAG chat-default store.
    pub fn set_tenant_default_model(
        &self,
        providers: &ProviderStore,
        tenant_id: &str,
        model_provider: &str,
        model_instance: &str,
        model_name: &str,
        model_type: &str,
    ) -> anyhow::Result<()> {
        let provider = model_provider.trim();
        let instance = model_instance.trim();
        let name = model_name.trim();
        if model_type_field(model_type).is_none() {
            anyhow::bail!("model type '{model_type}' is invalid");
        }
        if provider.is_empty() && instance.is_empty() && name.is_empty() {
            if model_type == "chat" {
                return self
                    .set_default_chat_model(providers, tenant_id, None)
                    .map(|_| ());
            }
            return self.mutate_with_defaults(|_, _, capability_defaults| {
                if let Some(models) = capability_defaults.get_mut(tenant_id) {
                    models.remove(model_type);
                    if models.is_empty() {
                        capability_defaults.remove(tenant_id);
                    }
                }
                Ok(())
            });
        }
        if provider.is_empty() || instance.is_empty() || name.is_empty() {
            anyhow::bail!(
                "model_provider, model_instance and model_name must be specified together"
            );
        }
        if model_type == "chat" {
            let composite = format!("{name}@{instance}@{provider}");
            let resolved = self
                .resolve(providers, tenant_id, ModelCapability::Chat, Some(&composite))?
                .ok_or_else(|| anyhow::anyhow!("Chat model is not configured: {composite}"))?;
            let selector = resolved.id();
            return self
                .set_default_chat_model(providers, tenant_id, Some(&selector))
                .map(|_| ());
        }
        let providers_list = providers.list_configured();
        let instances = self.list_configured(tenant_id);
        let compose_profiles = std::env::var("COMPOSE_PROFILES").unwrap_or_default();
        let tei_model = std::env::var("TEI_MODEL").unwrap_or_default();
        check_model_available(
            &providers_list,
            &instances,
            factory_llm_entries(),
            provider,
            instance,
            name,
            model_type,
            &compose_profiles,
            &tei_model,
        )
        .map_err(anyhow::Error::msg)?;
        let composite = format!("{name}@{instance}@{provider}");
        self.mutate_with_defaults(|_, _, capability_defaults| {
            capability_defaults
                .entry(tenant_id.to_string())
                .or_default()
                .insert(model_type.to_string(), composite);
            Ok(())
        })
    }

    /// `models_api_service.list_tenant_default_models` — resolve every
    /// configured default model in `MODEL_TYPE_TO_FIELD` order (chat first).
    pub fn list_default_models(
        &self,
        providers: &ProviderStore,
        tenant_id: &str,
    ) -> Vec<DefaultModelDescriptor> {
        let providers_list = providers.list_configured();
        let instances = self.list_configured(tenant_id);
        let compose_profiles = std::env::var("COMPOSE_PROFILES").unwrap_or_default();
        let tei_model = std::env::var("TEI_MODEL").unwrap_or_default();
        let mut models = Vec::new();
        if let Some(selector) = self.default_chat_model(tenant_id)
            && let Ok(Some(resolved)) = self.resolve(
                providers,
                tenant_id,
                ModelCapability::Chat,
                Some(&selector),
            )
        {
            models.push(DefaultModelDescriptor {
                model_provider: resolved.provider_name.clone(),
                model_instance: resolved.instance_name.clone(),
                model_name: resolved.model_name.clone(),
                model_type: "chat".to_string(),
                selector: resolved.ragflow_selector(),
                enable: true,
            });
        }
        let capability_defaults = self.default_capability_models.read().unwrap();
        let tenant_defaults = capability_defaults.get(tenant_id);
        for (tag, _field) in MODEL_TYPE_TO_FIELD {
            if *tag == "chat" {
                continue;
            }
            let Some(composite) = tenant_defaults.and_then(|models| models.get(*tag)) else {
                continue;
            };
            if let Some(info) = get_model_info(
                &providers_list,
                &instances,
                factory_llm_entries(),
                composite,
                tag,
                &compose_profiles,
                &tei_model,
            ) {
                models.push(info);
            }
        }
        models
    }
}

/// One entry of `GET /models` (Python `list_tenant_added_models`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddedModelEntry {
    pub model_type: Vec<String>,
    pub name: String,
    pub provider_id: String,
    pub provider_name: String,
    pub instance_id: String,
    pub instance_name: String,
    /// RayRAG wire extension (factory `max_tokens` / spec override).
    pub max_tokens: Option<u64>,
}

/// `models_api_service.list_tenant_added_models` — factory-catalog expansion
/// with manual status overrides, manual-only models, the TEI builtin
/// embedding row, and the `(rank, provider_name, instance_name)` ordering.
/// Pure over injected inputs (`compose_profiles` / `tei_model` are passed in
/// because the TEI branch is env-gated upstream).
pub fn plan_added_models(
    providers: &[crate::api::features::Provider],
    instances: &[TenantModelInstance],
    factory: &[FactoryLlmEntry],
    filter: &[String],
    compose_profiles: &str,
    tei_model: &str,
) -> Vec<AddedModelEntry> {
    let provider_by_id: HashMap<_, _> = providers
        .iter()
        .map(|provider| (provider.id.as_str(), provider))
        .collect();
    // provider_name -> tenant instances, keyed by name exactly like the
    // upstream `provider_instance_map` (same-name providers merge).
    let mut instances_by_provider: HashMap<&str, Vec<&TenantModelInstance>> = HashMap::new();
    for instance in instances {
        let Some(name) = provider_by_id
            .get(instance.provider_id.as_str())
            .map(|provider| provider.name.as_str())
        else {
            continue;
        };
        instances_by_provider.entry(name).or_default().push(instance);
    }
    // `model_record_map`: "{provider_id}|{instance_id}|{model_name}" -> specs,
    // keeping first-seen order for the manual-keys pass (upstream iterates a
    // set, i.e. arbitrary order; insertion order is the deterministic port).
    let mut record_map: Vec<(String, Vec<&TenantModelSpec>)> = Vec::new();
    let mut record_index: HashMap<String, usize> = HashMap::new();
    for instance in instances {
        for model in &instance.models {
            let key = format!(
                "{}|{}|{}",
                instance.provider_id, instance.instance_id, model.name
            );
            let index = *record_index.entry(key.clone()).or_insert_with(|| {
                record_map.push((key.clone(), Vec::new()));
                record_map.len() - 1
            });
            record_map[index].1.push(model);
        }
    }

    let mut added: Vec<AddedModelEntry> = Vec::new();
    let mut key_in_factory: HashSet<String> = HashSet::new();
    for entry in factory {
        let Some(provider_instances) = instances_by_provider.get(entry.name.as_str()) else {
            continue;
        };
        for row in &entry.llm {
            if !filter.is_empty()
                && !row
                    .model_types
                    .iter()
                    .any(|capability| filter.contains(capability))
            {
                continue;
            }
            for instance in provider_instances {
                let key = format!(
                    "{}|{}|{}",
                    instance.provider_id, instance.instance_id, row.llm_name
                );
                key_in_factory.insert(key.clone());
                let specs = record_index
                    .get(&key)
                    .map(|index| record_map[*index].1.as_slice())
                    .unwrap_or_default();
                // factory ∪ active − inactive − unsupported (RayRAG models
                // carry a per-spec enabled flag; UNSUPPORTED tombstones are
                // folded into `enabled == false`).
                let mut model_types: Vec<String> = row.model_types.clone();
                let mut active: Vec<&str> = Vec::new();
                let mut inactive: Vec<&str> = Vec::new();
                for spec in specs {
                    for capability in &spec.model_types {
                        let raw = capability.as_str();
                        if !filter.is_empty() && !filter.contains(&raw.to_string()) {
                            continue;
                        }
                        if spec.enabled {
                            active.push(raw);
                        } else {
                            inactive.push(raw);
                        }
                    }
                }
                for capability in active {
                    if !model_types.contains(&capability.to_string()) {
                        model_types.push(capability.to_string());
                    }
                }
                model_types.retain(|capability| !inactive.contains(&capability.as_str()));
                if model_types.is_empty() {
                    continue;
                }
                added.push(AddedModelEntry {
                    model_type: model_types,
                    name: row.llm_name.clone(),
                    provider_id: instance.provider_id.clone(),
                    provider_name: entry.name.clone(),
                    instance_id: instance.instance_id.clone(),
                    instance_name: instance.instance_name.clone(),
                    max_tokens: specs
                        .iter()
                        .find_map(|spec| spec.max_tokens)
                        .or((row.max_tokens > 0).then_some(row.max_tokens)),
                });
            }
        }
    }

    // Manual-only models: TenantModel records whose key never appeared in the
    // factory catalog. Only ACTIVE types survive, mirroring upstream.
    for (key, specs) in &record_map {
        if key_in_factory.contains(key) {
            continue;
        }
        let mut parts = key.splitn(3, '|');
        let (Some(provider_id), Some(instance_id), Some(model_name)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let mut model_types: Vec<&str> = Vec::new();
        for spec in specs {
            if !spec.enabled {
                continue;
            }
            for capability in &spec.model_types {
                let raw = capability.as_str();
                if !filter.is_empty() && !filter.contains(&raw.to_string()) {
                    continue;
                }
                if !model_types.contains(&raw) {
                    model_types.push(raw);
                }
            }
        }
        if model_types.is_empty() {
            continue;
        }
        let provider_name = provider_by_id
            .get(provider_id)
            .map(|provider| provider.name.clone())
            .unwrap_or_default();
        let instance_name = instances
            .iter()
            .find(|instance| instance.instance_id == instance_id)
            .map(|instance| instance.instance_name.clone())
            .unwrap_or_default();
        added.push(AddedModelEntry {
            model_type: model_types
                .into_iter()
                .map(str::to_string)
                .collect(),
            name: model_name.to_string(),
            provider_id: provider_id.to_string(),
            provider_name,
            instance_id: instance_id.to_string(),
            instance_name,
            max_tokens: specs.iter().find_map(|spec| spec.max_tokens),
        });
    }

    // TEI builtin embedding row (env-gated upstream).
    if compose_profiles.contains("tei-") && !tei_model.is_empty() {
        if filter.is_empty() || filter.iter().any(|capability| capability == "embedding") {
            if !added
                .iter()
                .any(|model| model.provider_name == "Builtin" && model.name == tei_model)
            {
                added.push(AddedModelEntry {
                    model_type: vec!["embedding".to_string()],
                    name: tei_model.to_string(),
                    provider_id: String::new(),
                    provider_name: "Builtin".to_string(),
                    instance_id: String::new(),
                    instance_name: "default".to_string(),
                    max_tokens: None,
                });
            }
        }
    }

    added.sort_by(|left, right| {
        factory_rank_key(&left.provider_name)
            .cmp(&factory_rank_key(&right.provider_name))
            .then_with(|| left.provider_name.cmp(&right.provider_name))
            .then_with(|| left.instance_name.cmp(&right.instance_name))
    });
    added
}

#[derive(Debug, Default, Deserialize)]
pub struct TenantModelQuery {
    pub tenant_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RagflowModelQuery {
    pub tenant_id: Option<String>,
    #[serde(rename = "type")]
    pub model_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct RagflowTenantModel {
    name: String,
    model_type: Vec<String>,
    provider: String,
    provider_id: String,
    instance_id: String,
    instance_name: String,
    selector: String,
    max_tokens: Option<u64>,
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

/// GET `/api/v1/models` — RAGFlow tenant-added model catalog used by model
/// tree selectors. Mirrors `models_api_service.list_tenant_added_models`:
/// every model of the factory catalog is exposed per tenant instance with
/// manual status overrides, manual-only models are appended, the TEI builtin
/// embedding is synthesized from env config, and entries are ordered by
/// `(factory rank desc, provider name, instance name)`. The OpenAI protocol
/// catalog lives at `/api/v1/openai/models`; these payloads intentionally
/// have different contracts.
pub async fn list_ragflow_models(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<RagflowModelQuery>,
) -> Response {
    let tenant_id = selected_tenant(&auth, query.tenant_id.as_deref());
    if !can_read_tenant(&state, &auth, tenant_id) {
        return forbidden("Tenant membership required");
    }
    let requested_types: Vec<String> = query
        .model_type
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase)
        .collect();
    let providers: Vec<_> = state
        .providers
        .list_configured()
        .into_iter()
        .filter(|provider| provider.enabled)
        .collect();
    let instances = state.tenant_models.list_configured(tenant_id);
    let compose_profiles = std::env::var("COMPOSE_PROFILES").unwrap_or_default();
    let tei_model = std::env::var("TEI_MODEL").unwrap_or_default();
    let entries = plan_added_models(
        &providers,
        &instances,
        factory_llm_entries(),
        &requested_types,
        &compose_profiles,
        &tei_model,
    );
    let models: Vec<RagflowTenantModel> = entries
        .into_iter()
        .map(|entry| RagflowTenantModel {
            selector: format!(
                "{}@{}@{}",
                entry.name, entry.instance_name, entry.provider_name
            ),
            name: entry.name,
            model_type: entry.model_type,
            provider: entry.provider_name.clone(),
            provider_id: entry.provider_id,
            instance_id: entry.instance_id,
            instance_name: entry.instance_name,
            max_tokens: entry.max_tokens,
        })
        .collect();
    Json(serde_json::json!({ "code": 0, "data": models })).into_response()
}

/// GET `/api/v1/models/default` — RAGFlow tenant default models across every
/// capability (`chat`, `embedding`, `rerank`, `asr`, `vision`, `tts`, `ocr`),
/// resolved via `_get_model_info`. RayRAG emits an extra `selector` field on
/// each entry for its own selector UI.
pub async fn get_ragflow_default_models(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<TenantModelQuery>,
) -> Response {
    let tenant_id = selected_tenant(&auth, query.tenant_id.as_deref());
    if !can_read_tenant(&state, &auth, tenant_id) {
        return forbidden("Tenant membership required");
    }
    let models = state
        .tenant_models
        .list_default_models(&state.providers, tenant_id);
    Json(serde_json::json!({ "code": 0, "data": { "models": models } })).into_response()
}

#[derive(Debug, Default, Deserialize)]
pub struct RagflowDefaultModelUpdate {
    pub tenant_id: Option<String>,
    #[serde(default)]
    pub model_provider: String,
    #[serde(default)]
    pub model_instance: String,
    #[serde(default)]
    pub model_name: String,
    #[serde(default)]
    pub model_type: Option<String>,
}

/// PATCH `/api/v1/models/default` — RAGFlow `set_default_models`: set or clear
/// a tenant default model for one capability. Wire codes mirror upstream:
/// `100` for a missing `model_type` argument, `102` for service errors, and
/// `{"code":0,"message":"success"}` on success.
pub async fn set_ragflow_default_models(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(update): Json<RagflowDefaultModelUpdate>,
) -> Response {
    let tenant_id = selected_tenant(&auth, update.tenant_id.as_deref()).to_string();
    if !state.tenants.can_manage(&tenant_id, &auth.user_id) {
        return forbidden("Tenant administrator access required");
    }
    let Some(model_type) = update
        .model_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Json(serde_json::json!({
            "code": 100,
            "message": "model_type is required"
        }))
        .into_response();
    };
    match state.tenant_models.set_tenant_default_model(
        &state.providers,
        &tenant_id,
        &update.model_provider,
        &update.model_instance,
        &update.model_name,
        model_type,
    ) {
        Ok(()) => Json(serde_json::json!({ "code": 0, "message": "success" })).into_response(),
        Err(error) => Json(serde_json::json!({
            "code": 102,
            "message": error.to_string()
        }))
        .into_response(),
    }
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
        assert!(
            store
                .sync_model_types("tenant-a", "local", "nope", "model-a", &[], &[])
                .is_err()
        );
        assert!(
            store
                .sync_model_types("tenant-a", "local", "inst-1", "missing", &[], &[])
                .is_err()
        );
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
        assert_eq!(explicit.instance_name, "prod-east");
        assert_eq!(
            explicit.ragflow_selector(),
            "text-embedding@nomic@prod-east@SILICONFLOW"
        );

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

    // ── models_api_service port tests ─────────────────────────────────

    fn fake_provider(id: &str, name: &str) -> crate::api::features::Provider {
        crate::api::features::Provider {
            id: id.into(),
            name: name.into(),
            api_base: "http://127.0.0.1:8080/v1".into(),
            models: vec![],
            enabled: true,
            api_key: None,
        }
    }

    fn fake_factory(name: &str, rank: Option<&str>, rows: &[(&str, &[&str])]) -> FactoryLlmEntry {
        FactoryLlmEntry {
            name: name.into(),
            rank: rank.map(str::to_string),
            llm: rows
                .iter()
                .map(|(llm_name, types)| FactoryLlmRow {
                    llm_name: (*llm_name).into(),
                    model_types: types.iter().map(|value| value.to_string()).collect(),
                    max_tokens: 8192,
                })
                .collect(),
        }
    }

    fn fake_instance(
        tenant: &str,
        provider_id: &str,
        instance_id: &str,
        instance_name: &str,
        models: Vec<TenantModelSpec>,
    ) -> TenantModelInstance {
        TenantModelInstance {
            tenant_id: tenant.into(),
            provider_id: provider_id.into(),
            instance_id: instance_id.into(),
            instance_name: instance_name.into(),
            api_base: "http://127.0.0.1:8080/v1".into(),
            api_key: None,
            models,
        }
    }

    fn spec(name: &str, types: &[ModelCapability], enabled: bool) -> TenantModelSpec {
        TenantModelSpec {
            name: name.into(),
            model_types: types.to_vec(),
            max_tokens: Some(8192),
            enabled,
        }
    }

    /// Pick a real `(factory name, model name)` pair from the embedded
    /// catalog whose model carries the requested raw capability.
    fn factory_model_for_type(raw_type: &str) -> (String, String) {
        for entry in factory_llm_entries() {
            if let Some(row) = entry
                .llm
                .iter()
                .find(|row| row.model_types.iter().any(|capability| capability == raw_type))
            {
                return (entry.name.clone(), row.llm_name.clone());
            }
        }
        panic!("embedded factory catalog has no {raw_type} model");
    }

    #[test]
    fn model_type_mappings_match_ragflow_constant_tables() {
        assert_eq!(model_type_field("chat"), Some("llm_id"));
        assert_eq!(model_type_field("embedding"), Some("embd_id"));
        assert_eq!(model_type_field("rerank"), Some("rerank_id"));
        assert_eq!(model_type_field("asr"), Some("asr_id"));
        assert_eq!(model_type_field("vision"), Some("img2txt_id"));
        assert_eq!(model_type_field("tts"), Some("tts_id"));
        assert_eq!(model_type_field("ocr"), Some("ocr_id"));
        assert_eq!(model_type_field("unknown"), None);
        assert_eq!(model_tag_type("asr"), "speech2text");
        assert_eq!(model_tag_type("vision"), "image2text");
        assert_eq!(model_tag_type("chat"), "chat");
        assert_eq!(model_tag_type("embedding"), "embedding");
        assert_eq!(model_tag_type("ocr"), "ocr");
        assert_eq!(model_tag_type("something-else"), "something-else");
    }

    #[test]
    fn split_model_default_is_right_anchored() {
        assert_eq!(split_model_default("a@b@c"), ("a", "b", "c"));
        assert_eq!(split_model_default("a@c"), ("a", "default", "c"));
        assert_eq!(split_model_default("a"), ("a", "default", ""));
        // '@' inside the model name stays in the leftmost field.
        assert_eq!(
            split_model_default("text-embedding@q8_0@lmstudio@LM-Studio"),
            ("text-embedding@q8_0", "lmstudio", "LM-Studio")
        );
    }

    #[test]
    fn get_model_info_resolves_entity_factory_fallback_and_specials() {
        let providers = vec![fake_provider("oa", "OpenAI")];
        let instances = vec![fake_instance(
            "tenant-a",
            "oa",
            "inst",
            "default",
            vec![
                spec("gpt-chat", &[ModelCapability::Chat], true),
                spec("gpt-disabled", &[ModelCapability::Chat], false),
            ],
        )];
        let factory = vec![fake_factory(
            "OpenAI",
            Some("999"),
            &[
                ("gpt-chat", &["chat"]),
                ("gpt-factory-only", &["chat"]),
                ("embed-model", &["embedding"]),
            ],
        )];

        let info = get_model_info(
            &providers,
            &instances,
            &factory,
            "gpt-chat@default@OpenAI",
            "chat",
            "",
            "",
        )
        .unwrap();
        assert_eq!(info.model_type, "chat");
        assert_eq!(info.selector, "gpt-chat@default@OpenAI");
        assert!(info.enable);

        // Disabled entity -> None (status != ACTIVE upstream).
        assert!(
            get_model_info(
                &providers,
                &instances,
                &factory,
                "gpt-disabled@default@OpenAI",
                "chat",
                "",
                ""
            )
            .is_none()
        );

        // Factory fallback: model not configured on the instance but present
        // in the factory catalog with the right capability.
        let fallback = get_model_info(
            &providers,
            &instances,
            &factory,
            "gpt-factory-only@default@OpenAI",
            "chat",
            "",
            "",
        )
        .unwrap();
        assert_eq!(fallback.model_name, "gpt-factory-only");
        assert!(fallback.enable);

        // Capability mismatch in the factory -> None.
        assert!(
            get_model_info(
                &providers,
                &instances,
                &factory,
                "embed-model@default@OpenAI",
                "chat",
                "",
                ""
            )
            .is_none()
        );

        // Unknown provider / instance -> None.
        assert!(
            get_model_info(&providers, &instances, &factory, "gpt-chat@default@Nope", "chat", "", "")
                .is_none()
        );
        assert!(
            get_model_info(&providers, &instances, &factory, "gpt-chat@other@OpenAI", "chat", "", "")
                .is_none()
        );

        // Empty selector -> None.
        assert!(get_model_info(&providers, &instances, &factory, "", "chat", "", "").is_none());

        // deepdoc OCR special case is always enabled without any lookup.
        let deepdoc = get_model_info(
            &providers,
            &instances,
            &factory,
            "deepdoc@default@infiniflow",
            "ocr",
            "",
            "",
        )
        .unwrap();
        assert_eq!(deepdoc.model_type, "ocr");
        assert!(deepdoc.enable);

        // TEI builtin embedding special case (env-injected).
        let tei = get_model_info(
            &providers,
            &instances,
            &factory,
            "bge-m3@Builtin",
            "embedding",
            "tei-test",
            "bge-m3",
        )
        .unwrap();
        assert_eq!(tei.model_provider, "Builtin");
        assert_eq!(tei.model_instance, "default");
        assert_eq!(tei.model_type, "embedding");
        assert!(tei.enable);

        // API tag -> mapped tag: a `vision` default resolves with the
        // `image2text` raw capability.
        let vision_factory = vec![fake_factory("OpenAI", Some("999"), &[("gpt-4o", &["chat", "image2text"])])];
        let vision = get_model_info(
            &providers,
            &instances,
            &vision_factory,
            "gpt-4o@default@OpenAI",
            "vision",
            "",
            "",
        )
        .unwrap();
        assert_eq!(vision.model_type, "image2text");
    }

    #[test]
    fn check_model_available_matches_upstream_validation_messages() {
        let providers = vec![fake_provider("oa", "OpenAI"), fake_provider("inf", "infiniflow")];
        let instances = vec![fake_instance(
            "tenant-a",
            "oa",
            "inst",
            "default",
            vec![
                spec("gpt-chat", &[ModelCapability::Chat], true),
                spec("gpt-disabled", &[ModelCapability::Chat], false),
            ],
        )];
        let factory = vec![fake_factory(
            "OpenAI",
            Some("999"),
            &[("gpt-chat", &["chat"]), ("embed-model", &["embedding"])],
        )];
        let check = |provider: &str, instance: &str, model: &str, model_type: &str| {
            check_model_available(&providers, &instances, &factory, provider, instance, model, model_type, "", "")
        };

        // deepdoc OCR is always available (both upstream early returns).
        assert!(
            check_model_available(&providers, &instances, &factory, "infiniflow", "default", "deepdoc", "ocr", "", "")
                .is_ok()
        );
        assert!(
            check_model_available(&providers, &instances, &factory, "infiniflow", "default", "deepdoc", "chat", "", "")
                .is_ok()
        );
        // TEI builtin embedding (env-injected).
        assert!(
            check_model_available(&providers, &instances, &factory, "Builtin", "default", "bge-m3", "embedding", "tei-test", "bge-m3")
                .is_ok()
        );
        // Happy path: configured + enabled entity.
        assert!(check("OpenAI", "default", "gpt-chat", "chat").is_ok());

        assert_eq!(
            check("Missing", "default", "x", "chat").unwrap_err(),
            "Provider 'Missing' not found"
        );
        assert_eq!(
            check("OpenAI", "nope", "x", "chat").unwrap_err(),
            "Instance 'nope' not found for provider 'OpenAI'"
        );
        let empty_factory: Vec<FactoryLlmEntry> = vec![];
        assert_eq!(
            check_model_available(&providers, &instances, &empty_factory, "OpenAI", "default", "gpt-chat", "chat", "", "")
                .unwrap_err(),
            "Provider 'OpenAI' not found in factory info"
        );
        assert_eq!(
            check("OpenAI", "default", "gpt-disabled", "chat").unwrap_err(),
            "Model 'gpt-disabled' isn't available"
        );
        assert_eq!(
            check("OpenAI", "default", "ghost", "chat").unwrap_err(),
            "Model 'ghost' not found for provider 'OpenAI'"
        );
        assert_eq!(
            check("OpenAI", "default", "embed-model", "chat").unwrap_err(),
            "Model 'embed-model' isn't a chat model"
        );
    }

    #[test]
    fn set_tenant_default_model_validates_sets_clears_and_persists() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-capability-defaults-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tenant_models.json");
        let (factory_name, model_name) = factory_model_for_type("embedding");
        let provider_id = factory_name.to_ascii_lowercase();
        let provider_store = ProviderStore::in_memory();
        // `in_memory()` seeds the default provider catalog — remove any
        // pre-seeded entry for this factory so create() is idempotent
        // (stored ids are lowercased, factory catalog names are not).
        let _ = provider_store.delete(&factory_name.to_ascii_lowercase());
        provider_store
            .create(
                &provider_id,
                ProviderUpdate {
                    name: factory_name.clone(),
                    api_base: "http://127.0.0.1:8080/v1".into(),
                    models: vec![model_name.clone()],
                    enabled: true,
                    api_key: None,
                    clear_api_key: false,
                },
            )
            .unwrap();
        let store = TenantModelStore::new(&path).unwrap();
        store
            .upsert(
                &provider_store,
                "tenant-a",
                &provider_id,
                "primary",
                TenantModelInstanceUpdate {
                    tenant_id: None,
                    instance_name: "Primary".into(),
                    api_base: None,
                    api_key: None,
                    clear_api_key: false,
                    models: vec![spec("chat-dummy", &[ModelCapability::Chat], true)],
                },
            )
            .unwrap();

        // Invalid model type (MODEL_TYPE_TO_FIELD miss).
        let error = store
            .set_tenant_default_model(&provider_store, "tenant-a", "", "", "", "bogus")
            .unwrap_err();
        assert!(error.to_string().contains("model type 'bogus' is invalid"));

        // Partial triple.
        let error = store
            .set_tenant_default_model(&provider_store, "tenant-a", &factory_name, "", "", "embedding")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("model_provider, model_instance and model_name must be specified together")
        );

        // Valid set persists the composite `{model}@{instance}@{provider}`.
        store
            .set_tenant_default_model(
                &provider_store,
                "tenant-a",
                &factory_name,
                "Primary",
                &model_name,
                "embedding",
            )
            .unwrap();
        let expected = format!("{model_name}@Primary@{factory_name}");
        assert_eq!(
            store
                .default_capability_model("tenant-a", "embedding")
                .as_deref(),
            Some(expected.as_str())
        );

        // Survives restart.
        drop(store);
        let restored = TenantModelStore::new(&path).unwrap();
        assert_eq!(
            restored
                .default_capability_model("tenant-a", "embedding")
                .as_deref(),
            Some(expected.as_str())
        );

        // All-empty triple clears the default.
        restored
            .set_tenant_default_model(&provider_store, "tenant-a", "", "", "", "embedding")
            .unwrap();
        assert_eq!(
            restored.default_capability_model("tenant-a", "embedding"),
            None
        );

        // The `chat` capability routes through the chat-default store; the
        // stored id-format selector uses the lowercase provider id.
        restored
            .set_tenant_default_model(&provider_store, "tenant-a", &factory_name, "Primary", "chat-dummy", "chat")
            .unwrap();
        assert_eq!(
            restored.default_chat_model("tenant-a").as_deref(),
            Some(format!("{provider_id}/primary/chat-dummy").as_str())
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn list_default_models_emits_chat_then_capabilities_in_field_order() {
        // Seed a capability default that resolves through the REAL embedded
        // catalog; the tenant also needs an instance of that factory.
        let (factory_name, model_name) = factory_model_for_type("rerank");
        let provider_id = factory_name.to_ascii_lowercase();
        let provider_store = ProviderStore::in_memory();
        // `in_memory()` seeds the default provider catalog — remove any
        // pre-seeded entry for this factory so create() is idempotent
        // (stored ids are lowercased, factory catalog names are not).
        let _ = provider_store.delete(&factory_name.to_ascii_lowercase());
        provider_store
            .create(
                &provider_id,
                ProviderUpdate {
                    name: factory_name.clone(),
                    api_base: "http://127.0.0.1:8080/v1".into(),
                    models: vec![model_name.clone(), "chat-dummy".into()],
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
                &provider_id,
                "primary",
                TenantModelInstanceUpdate {
                    tenant_id: None,
                    instance_name: "Primary".into(),
                    api_base: None,
                    api_key: None,
                    clear_api_key: false,
                    models: vec![spec("chat-dummy", &[ModelCapability::Chat], true)],
                },
            )
            .unwrap();
        store
            .set_tenant_default_model(
                &provider_store,
                "tenant-a",
                &factory_name,
                "Primary",
                &model_name,
                "rerank",
            )
            .unwrap();
        // And a chat default for ordering (selector id-segment is the
        // lowercase store id, matching the candidate scan).
        store
            .set_default_chat_model(
                &provider_store,
                "tenant-a",
                Some(&format!("{provider_id}/primary/chat-dummy")),
            )
            .unwrap();
        let models = store.list_default_models(&provider_store, "tenant-a");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].model_type, "chat");
        assert_eq!(models[0].model_name, "chat-dummy");
        assert_eq!(models[1].model_type, "rerank");
        assert_eq!(models[1].model_name, model_name);
        assert_eq!(
            models[1].selector,
            format!("{model_name}@Primary@{factory_name}")
        );
        assert!(models[1].enable);
    }

    #[test]
    fn plan_added_models_expands_catalog_with_overrides_and_ranking() {
        let providers = vec![
            fake_provider("oa", "OpenAI"),
            fake_provider("custom", "MyCustom"),
        ];
        let instances = vec![
            fake_instance(
                "tenant-a",
                "oa",
                "i1",
                "default",
                vec![
                    spec("gpt-a", &[ModelCapability::Chat], true),
                    // Fully disabled -> all factory types removed -> skipped.
                    spec("gpt-b", &[ModelCapability::Chat], false),
                ],
            ),
            fake_instance(
                "tenant-a",
                "custom",
                "i2",
                "default",
                vec![spec("custom-model", &[ModelCapability::Rerank], true)],
            ),
        ];
        let factory = vec![
            fake_factory(
                "OpenAI",
                Some("999"),
                &[
                    ("gpt-a", &["chat"]),
                    ("gpt-b", &["chat"]),
                    ("gpt-extra", &["chat", "image2text"]),
                ],
            ),
            fake_factory("DeepSeek", Some("980"), &[("deepseek-chat", &["chat"])]),
        ];
        let entries = plan_added_models(&providers, &instances, &factory, &[], "", "");
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert!(names.contains(&"gpt-a"));
        assert!(!names.contains(&"gpt-b"), "disabled model is removed");
        assert!(!names.contains(&"deepseek-chat"), "no instance for DeepSeek");

        // Multi-type factory row keeps its full type list.
        let extra = entries
            .iter()
            .find(|entry| entry.name == "gpt-extra")
            .unwrap();
        assert_eq!(extra.model_type, vec!["chat", "image2text"]);

        // Manual-only model appended with active types.
        let manual = entries
            .iter()
            .find(|entry| entry.name == "custom-model")
            .unwrap();
        assert_eq!(manual.provider_name, "MyCustom");
        assert_eq!(manual.model_type, vec!["rerank"]);

        // Ranking: ranked factories first (rank desc), unknown names last.
        assert_eq!(entries.first().unwrap().provider_name, "OpenAI");
        assert_eq!(entries.last().unwrap().provider_name, "MyCustom");
    }

    #[test]
    fn plan_added_models_applies_active_overrides_and_filters() {
        let providers = vec![fake_provider("oa", "OpenAI")];
        let instances = vec![fake_instance(
            "tenant-a",
            "oa",
            "i1",
            "default",
            vec![spec(
                "gpt-a",
                &[ModelCapability::Chat, ModelCapability::Rerank],
                true,
            )],
        )];
        let factory = vec![fake_factory("OpenAI", Some("999"), &[("gpt-a", &["chat"])])];
        let entries = plan_added_models(&providers, &instances, &factory, &[], "", "");
        assert_eq!(
            entries[0].model_type,
            vec!["chat", "rerank"],
            "factory types plus active manual extras"
        );

        // Filtering: only rerank survives and the entry is rebuilt from the
        // filtered manual record (factory row has no rerank type).
        let filtered = plan_added_models(
            &providers,
            &instances,
            &factory,
            &["rerank".to_string()],
            "",
            "",
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "gpt-a");
        assert_eq!(filtered[0].model_type, vec!["rerank"]);
    }

    #[test]
    fn plan_added_models_synthesizes_tei_builtin_embedding() {
        let providers = vec![fake_provider("oa", "OpenAI")];
        let instances = vec![fake_instance(
            "tenant-a",
            "oa",
            "i1",
            "default",
            vec![spec("gpt-a", &[ModelCapability::Chat], true)],
        )];
        let factory = vec![fake_factory("OpenAI", Some("999"), &[("gpt-a", &["chat"])])];
        let entries = plan_added_models(&providers, &instances, &factory, &[], "tei-profile", "bge-m3");
        let tei = entries
            .iter()
            .find(|entry| entry.provider_name == "Builtin")
            .unwrap();
        assert_eq!(tei.name, "bge-m3");
        assert_eq!(tei.model_type, vec!["embedding"]);
        assert_eq!(tei.instance_name, "default");
        assert_eq!(tei.provider_id, "");
        assert_eq!(tei.instance_id, "");

        // Filtered out when the filter excludes embedding.
        let filtered = plan_added_models(
            &providers,
            &instances,
            &factory,
            &["chat".to_string()],
            "tei-profile",
            "bge-m3",
        );
        assert!(!filtered.iter().any(|entry| entry.provider_name == "Builtin"));

        // Not synthesized without the compose profile.
        let none = plan_added_models(&providers, &instances, &factory, &[], "", "bge-m3");
        assert!(!none.iter().any(|entry| entry.provider_name == "Builtin"));
    }

    #[test]
    fn factory_rank_key_negates_rank_and_defaults_unknowns() {
        assert_eq!(to_int(Some("999"), 500), 999);
        assert_eq!(to_int(None, 500), 500);
        assert_eq!(to_int(Some("1.0"), 500), 500);
        assert_eq!(factory_rank_key("OpenAI"), -999);
        assert_eq!(factory_rank_key("Not-A-Factory"), -500);
    }

    #[test]
    fn embedded_factory_catalog_covers_ragflow_factories_with_full_type_lists() {
        let catalog = factory_llm_entries();
        assert_eq!(catalog.len(), 66);
        let openai = catalog.iter().find(|entry| entry.name == "OpenAI").unwrap();
        assert_eq!(openai.rank.as_deref(), Some("999"));
        assert!(!openai.llm.is_empty());
        // Multi-type rows keep the FULL type list (the compiled providers
        // catalog only carries the first type).
        let tongyi = catalog
            .iter()
            .find(|entry| entry.name == "Tongyi-Qianwen")
            .unwrap();
        let omni = tongyi
            .llm
            .iter()
            .find(|row| row.llm_name == "qwen3-omni-flash-2025-09-15")
            .unwrap();
        assert_eq!(omni.model_types, vec!["chat", "speech2text", "image2text"]);
        // Unranked factories carry rank None.
        let builtin = catalog.iter().find(|entry| entry.name == "Builtin").unwrap();
        assert!(builtin.rank.is_none());
    }
}
