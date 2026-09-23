//! The public model catalogue (`https://models.agent-one.dev/list`).
//!
//! Adding a model by hand means guessing: is `deepseek-vl2` vision-capable, does it
//! support tool calls, how large is its context, what does it cost? The catalogue on
//! `models.agent-one.dev` answers those questions for ~75 providers, so the
//! provider dialog can look a name up instead of asking the operator to know.
//!
//! The page is a Next.js app that streams its data as a React Server Component
//! payload, so the catalogue is extracted from that payload rather than from an API:
//! the `self.__next_f.push([1, "…"])` chunks are concatenated and the `directory`
//! object inside them is read as JSON. The extraction is deliberately strict — if the
//! page changes shape the parse fails with what it looked for — and a cached copy is
//! kept so a deployment behind a slow or blocked route still has the catalogue and
//! never has to wait for it.

use crate::Result;
use serde::{Deserialize, Serialize};

/// Where the catalogue lives.
pub const CATALOG_URL: &str = "https://models.agent-one.dev/list";

/// Largest page accepted for parsing. The page is ~4.5 MB today; the cap keeps a
/// surprise (or a redirect to something enormous) from becoming memory.
pub const MAX_CATALOG_PAGE_BYTES: usize = 32 << 20;

/// How long a fetched catalogue is considered fresh.
pub const CATALOG_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// One model in the catalogue.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CatalogModel {
    /// Catalogue id (`provider-model`, e.g. `302ai-yi-lightning`).
    #[serde(default)]
    pub id: String,
    /// Name as the API expects it.
    #[serde(default)]
    pub name: String,
    /// Context window in tokens, when the catalogue knows it.
    #[serde(default)]
    pub context: Option<u64>,
    #[serde(default)]
    pub tool_call: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub attachment: bool,
    #[serde(default)]
    pub structured_output: bool,
    /// Price per million input tokens, when published.
    #[serde(default)]
    pub input_price: Option<f64>,
    /// Price per million output tokens, when published.
    #[serde(default)]
    pub output_price: Option<f64>,
    #[serde(default)]
    pub input_modalities: Vec<String>,
    #[serde(default)]
    pub output_modalities: Vec<String>,
}

impl CatalogModel {
    /// Does this model accept images on the way in?
    pub fn accepts_images(&self) -> bool {
        self.input_modalities
            .iter()
            .any(|value| value.eq_ignore_ascii_case("image"))
    }

    /// The RAGFlow model types this catalogue entry implies.
    ///
    /// The vocabulary is the provider dialog's (`PI_MODEL_TYPE_OPTIONS`): chat,
    /// embedding, rerank, tts, image2text, ocr, speech2text. A catalogue entry
    /// describes a chat-style model, so the image-capable ones also offer VLM.
    pub fn suggested_model_types(&self) -> Vec<&'static str> {
        let mut types = vec!["chat"];
        if self.accepts_images() {
            types.push("image2text");
        }
        types
    }

    /// One-line summary for the dialog.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(context) = self.context {
            parts.push(format!("context {context}"));
        }
        if self.tool_call {
            parts.push("tool calls".to_string());
        }
        if self.reasoning {
            parts.push("reasoning".to_string());
        }
        if self.accepts_images() {
            parts.push("images".to_string());
        }
        match (self.input_price, self.output_price) {
            (Some(input), Some(output)) => {
                parts.push(format!("${input}/M in · ${output}/M out"));
            }
            _ => {}
        }
        parts.join(" · ")
    }
}

/// One provider (a service the catalogue knows an endpoint for).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CatalogProvider {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub website: String,
    #[serde(default)]
    pub api_base_url: String,
    #[serde(default)]
    pub models: Vec<CatalogModel>,
}

/// The whole catalogue.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelCatalog {
    pub providers: Vec<CatalogProvider>,
    /// When this copy was fetched (ms since the epoch); 0 for a parsed-but-unsaved
    /// copy.
    #[serde(default)]
    pub fetched_at: u64,
    /// Where it came from (URL or cache path), for the status endpoint.
    #[serde(default)]
    pub source: String,
}

/// `models.agent-one.dev` shapes the entry as
/// `features {attachment, reasoning, tool_call, structured_output}`,
/// `pricing {input, output}`, `limit {context}` and
/// `modalities {input: [], output: []}`.
#[derive(Debug, Deserialize)]
struct RawModel {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    features: RawFeatures,
    #[serde(default)]
    pricing: RawPricing,
    #[serde(default)]
    limit: RawLimit,
    #[serde(default)]
    modalities: RawModalities,
}

#[derive(Debug, Default, Deserialize)]
struct RawFeatures {
    #[serde(default)]
    attachment: bool,
    #[serde(default)]
    reasoning: bool,
    #[serde(default)]
    tool_call: bool,
    #[serde(default)]
    structured_output: bool,
}

#[derive(Debug, Default, Deserialize)]
struct RawPricing {
    #[serde(default)]
    input: Option<f64>,
    #[serde(default)]
    output: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
struct RawLimit {
    #[serde(default)]
    context: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct RawModalities {
    #[serde(default)]
    input: Vec<String>,
    #[serde(default)]
    output: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawProvider {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    website: String,
    #[serde(default, alias = "apiBaseUrl")]
    api_base_url: String,
    #[serde(default)]
    models: std::collections::BTreeMap<String, RawModel>,
}

impl ModelCatalog {
    /// Parse the catalogue out of the catalogue page.
    pub fn parse_page(html: &str) -> Result<Self> {
        let payload = rsc_payload(html)
            .ok_or_else(|| anyhow::anyhow!("catalogue page carried no RSC payload"))?;
        let directory = directory_object(&payload)
            .ok_or_else(|| anyhow::anyhow!("catalogue payload had no 'directory' object"))?;
        let raw: std::collections::BTreeMap<String, RawProvider> = serde_json::from_str(directory)
            .map_err(|error| {
                anyhow::anyhow!("catalogue directory is not the expected JSON: {error}")
            })?;
        let mut providers: Vec<CatalogProvider> = raw
            .into_values()
            .map(|provider| CatalogProvider {
                id: provider.id,
                name: provider.name,
                website: provider.website,
                api_base_url: provider.api_base_url,
                models: provider
                    .models
                    .into_values()
                    .map(|model| CatalogModel {
                        id: model.id,
                        name: model.name,
                        context: model.limit.context,
                        tool_call: model.features.tool_call,
                        reasoning: model.features.reasoning,
                        attachment: model.features.attachment,
                        structured_output: model.features.structured_output,
                        input_price: model.pricing.input,
                        output_price: model.pricing.output,
                        input_modalities: model.modalities.input,
                        output_modalities: model.modalities.output,
                    })
                    .collect(),
            })
            .filter(|provider: &CatalogProvider| !provider.models.is_empty())
            .collect();
        if providers.is_empty() {
            anyhow::bail!("catalogue contained no providers with models");
        }
        providers.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(Self {
            providers,
            fetched_at: 0,
            source: CATALOG_URL.to_string(),
        })
    }

    /// Fetch and parse the live catalogue.
    pub async fn fetch() -> Result<Self> {
        let client = crate::common::cmd_timeout::model_client();
        let response = client
            .get(CATALOG_URL)
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("could not reach {CATALOG_URL}: {error}"))?;
        if !response.status().is_success() {
            anyhow::bail!("{CATALOG_URL} answered HTTP {}", response.status());
        }
        let body = crate::common::cmd_timeout::read_text_limited(
            response,
            MAX_CATALOG_PAGE_BYTES,
            "model catalogue",
        )
        .await?;
        let mut catalog = Self::parse_page(&body)?;
        catalog.fetched_at = now_ms();
        catalog.source = CATALOG_URL.to_string();
        Ok(catalog)
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    pub fn model_count(&self) -> usize {
        self.providers
            .iter()
            .map(|provider| provider.models.len())
            .sum()
    }

    /// The provider whose endpoint matches `base_url`, comparing hosts so
    /// `https://api.deepseek.com/v1` and `https://api.deepseek.com` agree.
    pub fn provider_for_base_url(&self, base_url: &str) -> Option<&CatalogProvider> {
        let host = host_of(base_url)?;
        self.providers.iter().find(|provider| {
            host_of(&provider.api_base_url).is_some_and(|candidate| candidate == host)
        })
    }

    /// Look a model name up, preferring the provider that matches `base_url`.
    ///
    /// Matching is intentionally forgiving (case-insensitive, and a name that
    /// contains the query or vice versa) because operators type `deepseek-chat`
    /// where the catalogue says `DeepSeek-Chat`.
    pub fn lookup(&self, base_url: Option<&str>, model_name: &str) -> Vec<CatalogMatch<'_>> {
        let query = model_name.trim().to_ascii_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let preferred = base_url.and_then(|url| self.provider_for_base_url(url));
        let mut matches = Vec::new();
        for provider in &self.providers {
            let preferred_provider = preferred.is_some_and(|candidate| candidate.id == provider.id);
            for model in &provider.models {
                let name = model.name.to_ascii_lowercase();
                let id = model.id.to_ascii_lowercase();
                let exact = name == query || id == query;
                let contains = name.contains(&query) || query.contains(&name);
                if !exact && !contains {
                    continue;
                }
                // Exact matches first, the endpoint being configured before other
                // endpoints, and a name that merely contains the query last.
                let score = match (exact, preferred_provider) {
                    (true, true) => 0,
                    (true, false) => 2,
                    (false, true) => 4,
                    (false, false) => 6,
                };
                matches.push(CatalogMatch {
                    provider,
                    model,
                    score,
                    rank: 0,
                });
            }
        }
        // An endpoint the catalogue knows but whose own models none of the typed name
        // matched — `api.deepseek.com` when the operator typed a name DeepSeek itself
        // does not serve — would otherwise answer with other providers' models only.
        // What that endpoint actually serves is the more useful answer, so it is
        // ranked ahead of a match on a *different* endpoint, closest name first: a near
        // miss (`deepseek-vl2` for `deepseek-chat`) is a likelier intent than an
        // alphabetical neighbour.
        let served_by_preferred = preferred
            .is_some_and(|provider| matches.iter().any(|hit| hit.provider.id == provider.id));
        if let Some(provider) = preferred
            && !served_by_preferred
        {
            let mut own: Vec<&CatalogModel> = provider.models.iter().collect();
            own.sort_by(|left, right| {
                let left_prefix = common_prefix_len(&left.name.to_ascii_lowercase(), &query);
                let right_prefix = common_prefix_len(&right.name.to_ascii_lowercase(), &query);
                right_prefix
                    .cmp(&left_prefix)
                    .then_with(|| left.name.cmp(&right.name))
            });
            for (index, model) in own.into_iter().take(3).enumerate() {
                matches.push(CatalogMatch {
                    provider,
                    model,
                    score: FALLBACK_SCORE,
                    // Keeps the closest-name-first order: the shared name-length
                    // tie-break below would otherwise reshuffle these entries.
                    rank: index as u8,
                });
            }
        }
        matches.sort_by(|left, right| {
            left.score
                .cmp(&right.score)
                .then_with(|| left.rank.cmp(&right.rank))
                .then_with(|| left.model.name.len().cmp(&right.model.name.len()))
                .then_with(|| left.provider.name.cmp(&right.provider.name))
        });
        matches.truncate(5);
        matches
    }
}

/// Rank of a model that the configured endpoint serves, added because none of its own
/// models matched the typed name. It sorts after an exact match on the endpoint and
/// before an exact match on a *different* endpoint, which is the useful order when the
/// operator typed a name their endpoint does not serve.
const FALLBACK_SCORE: u8 = 1;

/// Length of the shared prefix of two already-lowercased names.
fn common_prefix_len(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

/// One lookup hit.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogMatch<'a> {
    #[serde(flatten)]
    pub provider: &'a CatalogProvider,
    #[serde(skip)]
    pub model: &'a CatalogModel,
    #[serde(skip)]
    pub score: u8,
    /// Position within one score group (the endpoint's own models keep their
    /// closest-name-first order).
    #[serde(skip)]
    pub rank: u8,
}

/// Serialisable lookup hit for the API.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogLookupEntry {
    pub provider_id: String,
    pub provider_name: String,
    pub provider_website: String,
    pub provider_api_base_url: String,
    pub model: CatalogModel,
    pub summary: String,
    pub suggested_model_types: Vec<&'static str>,
}

impl CatalogLookupEntry {
    pub fn from_match(hit: &CatalogMatch<'_>) -> Self {
        Self {
            provider_id: hit.provider.id.clone(),
            provider_name: hit.provider.name.clone(),
            provider_website: hit.provider.website.clone(),
            provider_api_base_url: hit.provider.api_base_url.clone(),
            model: hit.model.clone(),
            summary: hit.model.summary(),
            suggested_model_types: hit.model.suggested_model_types(),
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0)
}

impl ModelCatalog {
    /// Is this copy young enough to answer lookups without going to the network?
    pub fn is_fresh(&self, now: u64) -> bool {
        self.fetched_at > 0 && now.saturating_sub(self.fetched_at) < CATALOG_TTL_MS
    }

    /// Read a previously saved copy.
    pub fn load_from(path: &std::path::Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| anyhow::anyhow!("could not read {}: {error}", path.display()))?;
        let mut catalog: Self = serde_json::from_str(&text).map_err(|error| {
            anyhow::anyhow!("{} is not a saved catalogue: {error}", path.display())
        })?;
        if catalog.source.is_empty() {
            catalog.source = path.display().to_string();
        }
        Ok(catalog)
    }

    /// Save a copy for the next boot.
    ///
    /// Written through a temporary file and renamed, so a crash mid-write leaves the
    /// previous copy intact instead of a truncated one that would fail to parse.
    pub fn save_to(&self, path: &std::path::Path) -> Result<()> {
        let temporary = path.with_extension("json.tmp");
        let text = serde_json::to_string(self)?;
        std::fs::write(&temporary, text)
            .map_err(|error| anyhow::anyhow!("could not write {}: {error}", temporary.display()))?;
        std::fs::rename(&temporary, path)
            .map_err(|error| anyhow::anyhow!("could not replace {}: {error}", path.display()))?;
        Ok(())
    }
}

/// File name of the saved copy, beside the other state files.
pub const CACHE_FILE: &str = "model_catalog.json";

/// Where the saved copy lives for a deployment whose web root is `static_dir`.
///
/// The state root is the parent of the web root — the same convention the user,
/// knowledge-base and provider stores use, so a container needs one volume.
pub fn cache_path(static_dir: &str) -> std::path::PathBuf {
    std::path::Path::new(static_dir).join("..").join(CACHE_FILE)
}

/// In-process copy plus what happened to it.
#[derive(Debug, Default)]
struct Cached {
    catalog: Option<ModelCatalog>,
    /// "memory" | "disk" | the catalogue URL.
    source: String,
    last_error: Option<String>,
}

static CACHE: std::sync::OnceLock<tokio::sync::Mutex<Cached>> = std::sync::OnceLock::new();

fn cache() -> &'static tokio::sync::Mutex<Cached> {
    CACHE.get_or_init(|| tokio::sync::Mutex::new(Cached::default()))
}

/// What the deployment knows about the catalogue right now, without fetching it.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogStatus {
    pub url: String,
    pub providers: usize,
    pub models: usize,
    /// When the cached copy was fetched (ms since the epoch); 0 when nothing is
    /// cached yet.
    pub fetched_at: u64,
    pub age_ms: u64,
    pub fresh: bool,
    /// Where the in-memory copy came from.
    pub source: String,
    pub cache_path: String,
    /// Why the last refresh failed, when one did. A stale copy keeps serving.
    pub last_error: Option<String>,
}

/// Replace the in-memory copy — used after a fetch and by tests.
pub async fn put(catalog: ModelCatalog, source: &str) {
    let mut guard = cache().lock().await;
    guard.source = source.to_string();
    guard.last_error = None;
    guard.catalog = Some(catalog);
}

/// The catalogue for lookups: memory, then the saved copy, then the network.
///
/// A refresh that fails keeps whatever was already known and records why, because a
/// deployment on a network that cannot reach the catalogue should still be able to
/// look a model up from the last copy it had.
pub async fn catalog(static_dir: &str) -> Result<ModelCatalog> {
    let path = cache_path(static_dir);
    let mut guard = cache().lock().await;
    let now = now_ms();
    if guard
        .catalog
        .as_ref()
        .is_some_and(|existing| existing.is_fresh(now))
    {
        return Ok(guard.catalog.clone().unwrap_or_default());
    }
    if guard.catalog.is_none()
        && let Ok(saved) = ModelCatalog::load_from(&path)
        && !saved.is_empty()
    {
        guard.source = path.display().to_string();
        guard.catalog = Some(saved);
    }
    if guard
        .catalog
        .as_ref()
        .is_some_and(|existing| existing.is_fresh(now))
    {
        return Ok(guard.catalog.clone().unwrap_or_default());
    }
    match ModelCatalog::fetch().await {
        Ok(fresh) => {
            // A copy that cannot be saved is still usable; say so without failing.
            let _ = fresh.save_to(&path);
            guard.source = CATALOG_URL.to_string();
            guard.last_error = None;
            guard.catalog = Some(fresh.clone());
            Ok(fresh)
        }
        Err(error) => {
            guard.last_error = Some(error.to_string());
            match guard.catalog.clone() {
                Some(stale) => Ok(stale),
                None => Err(error),
            }
        }
    }
}

/// Fetch the catalogue unconditionally, replacing the cached copy.
pub async fn refresh(static_dir: &str) -> Result<ModelCatalog> {
    let path = cache_path(static_dir);
    let fresh = ModelCatalog::fetch().await?;
    let _ = fresh.save_to(&path);
    let mut guard = cache().lock().await;
    guard.source = CATALOG_URL.to_string();
    guard.last_error = None;
    guard.catalog = Some(fresh.clone());
    Ok(fresh)
}

/// Report the cache without fetching; a copy on disk is loaded once so a fresh
/// process does not claim to know nothing.
pub async fn status(static_dir: &str) -> CatalogStatus {
    let path = cache_path(static_dir);
    let mut guard = cache().lock().await;
    if guard.catalog.is_none()
        && let Ok(saved) = ModelCatalog::load_from(&path)
        && !saved.is_empty()
    {
        guard.source = path.display().to_string();
        guard.catalog = Some(saved);
    }
    let now = now_ms();
    let catalog = guard.catalog.as_ref();
    CatalogStatus {
        url: CATALOG_URL.to_string(),
        providers: catalog.map_or(0, ModelCatalog::provider_count),
        models: catalog.map_or(0, ModelCatalog::model_count),
        fetched_at: catalog.map_or(0, |value| value.fetched_at),
        age_ms: catalog.map_or(0, |value| now.saturating_sub(value.fetched_at)),
        fresh: catalog.is_some_and(|value| value.is_fresh(now)),
        source: if guard.catalog.is_some() {
            guard.source.clone()
        } else {
            String::new()
        },
        cache_path: path.display().to_string(),
        last_error: guard.last_error.clone(),
    }
}

/// The catalogue without touching the disk or the network — for handlers that must
/// not block on I/O.
pub async fn cached() -> Option<ModelCatalog> {
    cache().lock().await.catalog.clone()
}

/// Host (lowercased) of a URL, ignoring scheme, path and port.
pub fn host_of(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    let without_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    let host = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .split('@')
        .next_back()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Concatenate the React Server Component payload a Next.js page streams in
/// `self.__next_f.push([1, "…"])` calls.
fn rsc_payload(html: &str) -> Option<String> {
    const MARKER: &str = "self.__next_f.push([1,";
    let mut payload = String::new();
    let mut rest = html;
    while let Some(index) = rest.find(MARKER) {
        let after = &rest[index + MARKER.len()..];
        let Some(start) = after.find('"') else {
            break;
        };
        let quoted = &after[start..];
        // The chunk is a JSON string literal; let serde decode the escapes rather
        // than hand-rolling them.
        let mut end = 1usize;
        let bytes = quoted.as_bytes();
        while end < bytes.len() {
            match bytes[end] {
                b'\\' => end += 2,
                b'"' => break,
                _ => end += 1,
            }
        }
        if end >= bytes.len() {
            break;
        }
        if let Ok(decoded) = serde_json::from_str::<String>(&quoted[..=end]) {
            payload.push_str(&decoded);
        }
        rest = &quoted[end..];
    }
    (!payload.is_empty()).then_some(payload)
}

/// Slice the `"directory":{…}` object out of the payload.
fn directory_object(payload: &str) -> Option<&str> {
    const KEY: &str = "\"directory\":";
    let start = payload.find(KEY)? + KEY.len();
    let bytes = payload.as_bytes();
    let mut index = start;
    while index < bytes.len() && bytes[index] != b'{' {
        index += 1;
    }
    if index >= bytes.len() {
        return None;
    }
    let open = index;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&payload[open..=index]);
                    }
                }
                _ => {}
            }
        }
        index += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A miniature of the real page: the RSC chunks carry a `directory` object whose
    /// providers hold a `models` map.
    fn sample_page() -> String {
        page_with_directory(serde_json::json!({
            "deepseek": {
                "id": "deepseek",
                "name": "DeepSeek",
                "website": "https://deepseek.com",
                "apiBaseUrl": "https://api.deepseek.com/v1",
                "models": {
                    "deepseek-chat": {
                        "id": "deepseek-chat",
                        "name": "DeepSeek-Chat",
                        "features": {"attachment": false, "reasoning": false, "tool_call": true, "structured_output": true},
                        "pricing": {"input": 0.27, "output": 1.1},
                        "limit": {"context": 128000},
                        "modalities": {"input": ["text"], "output": ["text"]}
                    },
                    "deepseek-vl2": {
                        "id": "deepseek-vl2",
                        "name": "deepseek-vl2",
                        "features": {"attachment": true, "reasoning": false, "tool_call": false, "structured_output": false},
                        "pricing": {"input": 0.1, "output": 0.2},
                        "limit": {"context": 32000},
                        "modalities": {"input": ["text", "image"], "output": ["text"]}
                    }
                }
            }
        }))
    }

    /// Wrap a `directory` object in the RSC payload a Next.js page streams.
    fn page_with_directory(directory: serde_json::Value) -> String {
        let payload = format!(
            "1:null\n2:[\"$\",\"main\",null,{{\"children\":[\"$\",\"$L1\",null,{{\"directory\":{directory}}}]}}]\n"
        );
        let escaped = serde_json::to_string(&payload).unwrap();
        format!(
            "<!DOCTYPE html><html><body><script>self.__next_f.push([1,{escaped}])</script></body></html>"
        )
    }

    #[test]
    fn parses_providers_models_capabilities_and_pricing() {
        let catalog = ModelCatalog::parse_page(&sample_page()).unwrap();
        assert_eq!(catalog.provider_count(), 1);
        assert_eq!(catalog.model_count(), 2);
        let provider = &catalog.providers[0];
        assert_eq!(provider.name, "DeepSeek");
        assert_eq!(provider.api_base_url, "https://api.deepseek.com/v1");
        let chat = provider
            .models
            .iter()
            .find(|model| model.name == "DeepSeek-Chat")
            .unwrap();
        assert_eq!(chat.context, Some(128_000));
        assert!(chat.tool_call);
        assert_eq!(chat.input_price, Some(0.27));
        assert_eq!(chat.suggested_model_types(), vec!["chat"]);
        assert!(chat.summary().contains("context 128000"));
        assert!(chat.summary().contains("tool calls"));

        let vision = provider
            .models
            .iter()
            .find(|model| model.name == "deepseek-vl2")
            .unwrap();
        assert!(vision.accepts_images());
        assert_eq!(vision.suggested_model_types(), vec!["chat", "image2text"]);
    }

    #[test]
    fn lookup_prefers_the_provider_matching_the_base_url() {
        let catalog = ModelCatalog::parse_page(&sample_page()).unwrap();
        // Same name offered by a matching and a non-matching provider would be ranked
        // by the base URL; here the single provider must still be found by host.
        assert_eq!(
            catalog
                .provider_for_base_url("https://api.deepseek.com/v1/chat")
                .map(|provider| provider.id.as_str()),
            Some("deepseek")
        );
        assert!(
            catalog
                .provider_for_base_url("https://example.invalid/v1")
                .is_none()
        );

        let hits = catalog.lookup(Some("https://api.deepseek.com/v1"), "deepseek-chat");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].model.name, "DeepSeek-Chat");

        // Forgiving matching: case and partial names both work.
        assert_eq!(
            catalog
                .lookup(None, "DEEPSEEK-VL2")
                .first()
                .map(|hit| hit.model.name.clone()),
            Some("deepseek-vl2".to_string())
        );
        assert!(
            !catalog
                .lookup(None, "gpt-4o")
                .iter()
                .any(|hit| hit.model.name.contains("deepseek"))
        );
        assert!(catalog.lookup(None, "").is_empty());
    }

    #[test]
    fn a_known_endpoint_offers_its_own_models_when_the_name_does_not_match() {
        let catalog = ModelCatalog::parse_page(&page_with_directory(serde_json::json!({
            "deepseek": {
                "id": "deepseek",
                "name": "DeepSeek",
                "apiBaseUrl": "https://api.deepseek.com/v1",
                "models": {"deepseek-chat": {"id": "deepseek-chat", "name": "deepseek-chat"}}
            },
            "gateway": {
                "id": "gateway",
                "name": "Gateway",
                "apiBaseUrl": "https://gateway.example.com/v1",
                "models": {
                    "gateway-a": {"id": "gateway-a", "name": "gateway-a"},
                    "gateway-b": {"id": "gateway-b", "name": "gateway-b", "limit": {"context": 200000}},
                    "deepseek-mini": {"id": "deepseek-mini", "name": "deepseek-mini"}
                }
            }
        })))
        .unwrap();
        // `deepseek-chat` is DeepSeek's, but the dialog is configuring Gateway. The
        // endpoint being configured answers first — starting with the closest name it
        // serves, a near miss of what was typed — and the other endpoint's exact match
        // follows, so a careless click cannot apply the wrong provider's numbers.
        let hits = catalog.lookup(Some("https://gateway.example.com/v1"), "deepseek-chat");
        let order: Vec<&str> = hits.iter().map(|hit| hit.model.name.as_str()).collect();
        assert_eq!(
            order,
            vec!["deepseek-mini", "gateway-a", "gateway-b", "deepseek-chat"]
        );
        assert_eq!(hits[0].provider.id, "gateway");
        assert_eq!(hits[3].provider.id, "deepseek");

        // When the endpoint's own model does match, nothing extra is appended.
        let hits = catalog.lookup(Some("https://gateway.example.com/v1"), "gateway-a");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].model.name, "gateway-a");

        // No endpoint at all means no fallback either: only the name matches.
        let hits = catalog.lookup(None, "gateway-a");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].model.name, "gateway-a");
    }

    #[test]
    fn a_page_without_the_expected_shape_fails_loudly() {
        let error = ModelCatalog::parse_page("<html><body>nothing here</body></html>")
            .unwrap_err()
            .to_string();
        assert!(error.contains("RSC payload"), "{error}");

        // Payload present but no directory: the failure names what was missing.
        let payload = "1:[\"$\",\"main\",null,{}]";
        let escaped = serde_json::to_string(payload).unwrap();
        let page = format!("<script>self.__next_f.push([1,{escaped}])</script>");
        let error = ModelCatalog::parse_page(&page).unwrap_err().to_string();
        assert!(error.contains("directory"), "{error}");
    }

    #[test]
    fn host_extraction_ignores_scheme_path_port_and_userinfo() {
        assert_eq!(
            host_of("https://api.deepseek.com/v1").as_deref(),
            Some("api.deepseek.com")
        );
        assert_eq!(
            host_of("http://API.Example.com:8080/x").as_deref(),
            Some("api.example.com")
        );
        assert_eq!(host_of("user:pw@host.tld/v1").as_deref(), Some("host.tld"));
        assert_eq!(host_of("   "), None);
    }

    #[test]
    fn a_saved_copy_round_trips_and_only_a_fetched_one_is_fresh() {
        let directory = std::env::temp_dir().join(format!(
            "rayrag-model-catalog-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = cache_path(directory.to_str().unwrap());
        assert_eq!(path.file_name().unwrap(), CACHE_FILE);
        assert!(
            path.parent()
                .is_some_and(|parent| parent.ends_with("..") || parent.exists())
        );

        let mut catalog = ModelCatalog::parse_page(&sample_page()).unwrap();
        // A parsed copy has never been fetched, so it is never "fresh": the first
        // lookup after a restart must be allowed to refresh it.
        assert!(!catalog.is_fresh(now_ms()));
        catalog.fetched_at = now_ms();
        assert!(catalog.is_fresh(now_ms()));
        assert!(!catalog.is_fresh(catalog.fetched_at + CATALOG_TTL_MS + 1));

        catalog.save_to(&path).unwrap();
        // No stray temporary file is left behind by the rename.
        assert!(!path.with_extension("json.tmp").exists());
        let restored = ModelCatalog::load_from(&path).unwrap();
        assert_eq!(restored.provider_count(), 1);
        assert_eq!(restored.model_count(), 2);
        assert_eq!(restored.fetched_at, catalog.fetched_at);
        assert_eq!(
            restored.providers[0].models[0].name,
            catalog.providers[0].models[0].name
        );

        // A file that is not a catalogue is rejected with its path named.
        let broken = directory.join("broken.json");
        std::fs::write(&broken, "{not json").unwrap();
        let error = ModelCatalog::load_from(&broken).unwrap_err().to_string();
        assert!(error.contains("broken.json"), "{error}");
        let _ = std::fs::remove_dir_all(&directory);
    }
}
