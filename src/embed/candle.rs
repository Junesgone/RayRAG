//! Pure Rust embedding with Candle — no HTTP, no Python.
//!
//! ⚠️ MODEL DOWNLOADS ARE UI-TRIGGERED ONLY.
//! Models are NOT downloaded automatically at build time, runtime,
//! or on first use. The user must explicitly trigger a download
//! from the RayRAG UI (by selecting "candle" mode) before a local
//! model is available.
//!
//! To download a model explicitly (for UI integration), call
//! `CandleEmbedder::download_model_to(config, target_dir).await`.
//!
//! To load a pre-downloaded model, call
//! `CandleEmbedder::load(CandleConfig::from_local("/path/to/model")).await`.
//!
//! Default model: all-MiniLM-L6-v2 (~23 MB, 384-dim).

use super::Embedder;
use crate::Result;
use hf_hub::{Repo, RepoType};

/// Container for downloaded model files.
struct DownloadedFiles {
    model: std::path::PathBuf,
    config: std::path::PathBuf,
    tokenizer: std::path::PathBuf,
}

/// Candle-based embedding model.
/// Loads BERT-style models via candle-transformers.
///
/// ## Model download policy
/// Models are NOT auto-downloaded. Use `download_model_to()` to
/// explicitly download a model to a local directory, then load it
/// with `CandleConfig::from_local(path)`.
///
/// Auto-download via `load(CandleConfig::default())` is supported
/// but should ONLY be called from the UI download trigger, never
/// from background/pipeline code.
pub struct CandleEmbedder {
    /// Thread-local computation device (CPU by default)
    device: candle_core::Device,
    /// Candle BERT model
    model: candle_transformers::models::bert::BertModel,
    /// HuggingFace tokenizer
    tokenizer: tokenizers::Tokenizer,
    /// Embedding dimension
    pub dimension: usize,
    /// Max sequence length
    max_length: usize,
    /// Mean pooling or CLS token
    pooling: PoolingStrategy,
}

#[derive(Debug, Clone, Copy)]
pub enum PoolingStrategy {
    /// Mean of all token embeddings (default for sentence embeddings)
    Mean,
    /// Use CLS token only
    Cls,
    /// Mean of last layer hidden states (with attention mask)
    MeanWithMask,
}

/// Embedder configuration for model loading.
pub struct CandleConfig {
    /// Model ID on HuggingFace (for download)
    pub model_id: String,
    /// Local model directory (skip HF download).
    /// Directory must contain: model.safetensors, config.json, tokenizer.json
    pub local_path: Option<String>,
    /// HuggingFace revision (optional)
    pub revision: Option<String>,
    /// Max sequence length (default: 256)
    pub max_length: usize,
    /// Pooling strategy (default: MeanWithMask)
    pub pooling: PoolingStrategy,
    /// Use GPU if available
    pub use_cuda: bool,
}

impl Default for CandleConfig {
    fn default() -> Self {
        Self {
            model_id: "sentence-transformers/all-MiniLM-L6-v2".to_string(),
            local_path: None,
            revision: None,
            max_length: 256,
            pooling: PoolingStrategy::MeanWithMask,
            use_cuda: false,
        }
    }
}

/// Small model configs for different use cases.
impl CandleConfig {
    /// Fast, small model (384-dim, ~23 MB)
    pub fn mini() -> Self {
        Self {
            model_id: "sentence-transformers/all-MiniLM-L6-v2".to_string(),
            ..Default::default()
        }
    }

    /// Medium model (768-dim, ~110 MB)
    pub fn base() -> Self {
        Self {
            model_id: "BAAI/bge-base-en-v1.5".to_string(),
            ..Default::default()
        }
    }

    /// Chinese-optimized (512-dim)
    pub fn chinese() -> Self {
        Self {
            model_id: "BAAI/bge-small-zh-v1.5".to_string(),
            ..Default::default()
        }
    }

    /// Multilingual (768-dim, ~470 MB)
    pub fn multilingual() -> Self {
        Self {
            model_id: "intfloat/multilingual-e5-base".to_string(),
            ..Default::default()
        }
    }

    /// Load from a local model directory (no download).
    /// Directory must contain: model.safetensors, config.json, tokenizer.json
    pub fn from_local(path: &str) -> Self {
        Self {
            model_id: String::new(),
            local_path: Some(path.to_string()),
            ..Default::default()
        }
    }
}

/// HuggingFace endpoints to try in order.
const HF_ENDPOINTS: &[&str] = &[
    "https://huggingface.co", // Primary
    "https://hf-mirror.com",  // Mirror (China)
];

impl CandleEmbedder {
    /// Download a model explicitly to a target directory (UI-triggered only).
    ///
    /// This is the ONLY function that should trigger model downloads.
    /// It downloads model.safetensors, config.json, tokenizer.json from
    /// HuggingFace (with hf-mirror.com fallback) into `target_dir`.
    ///
    /// After download, the model can be loaded with:
    ///   CandleEmbedder::load(CandleConfig::from_local("/path/to/model"))
    pub async fn download_model_to(config: &CandleConfig, target_dir: &str) -> Result<()> {
        let target = std::path::Path::new(target_dir);
        std::fs::create_dir_all(target)?;

        let mut last_err = None;
        for (i, endpoint) in HF_ENDPOINTS.iter().enumerate() {
            let label = if i == 0 { "primary" } else { "mirror" };
            tracing::info!("  Downloading model via {}: {}...", label, endpoint);

            match Self::download_model(endpoint, config).await {
                Ok(files) => {
                    // Copy downloaded files to target directory
                    std::fs::copy(&files.model, target.join("model.safetensors"))?;
                    std::fs::copy(&files.config, target.join("config.json"))?;
                    std::fs::copy(&files.tokenizer, target.join("tokenizer.json"))?;

                    if i > 0 {
                        tracing::warn!("  Mirror fallback succeeded (primary failed)");
                    }
                    tracing::info!(
                        "Model downloaded to: {} ({:.1} MB)",
                        target_dir,
                        std::fs::metadata(&files.model)?.len() as f64 / 1_048_576.0
                    );
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!("  ❌ {} failed: {:?}", label, e);
                    last_err = Some(e);
                    continue;
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            anyhow::anyhow!("All HF endpoints failed for model: {}", config.model_id)
        }))
    }

    /// Load an embedding model.
    /// If `local_path` is set, loads directly from the directory.
    /// Otherwise downloads from HuggingFace (with mirror fallback).
    ///
    /// ⚠️  When `local_path` is None, this WILL download the model.
    ///     Only call this path from the UI download trigger, never from
    ///     background/pipeline code — use `from_local()` instead.
    pub async fn load(config: CandleConfig) -> Result<Self> {
        let device = if config.use_cuda && candle_core::utils::cuda_is_available() {
            candle_core::Device::new_cuda(0)?
        } else {
            candle_core::Device::Cpu
        };

        // If local path is set, load directly without download
        if let Some(ref local) = config.local_path {
            tracing::info!("Loading embedding model from local: {}", local);
            let local_dir = std::path::Path::new(local);
            let files = DownloadedFiles {
                model: local_dir.join("model.safetensors"),
                config: local_dir.join("config.json"),
                tokenizer: local_dir.join("tokenizer.json"),
            };
            return Self::build_from_files(files, &config, &device);
        }

        tracing::info!(
            "Loading embedding model: {} (device: {:?})",
            config.model_id,
            device
        );

        let mut last_err = None;

        for (i, endpoint) in HF_ENDPOINTS.iter().enumerate() {
            let label = if i == 0 { "primary" } else { "mirror" };
            tracing::info!("  Trying {}: {}...", label, endpoint);

            match Self::download_model(endpoint, &config).await {
                Ok(files) => {
                    if i > 0 {
                        tracing::warn!("  ✅ Mirror fallback succeeded (primary failed)");
                    }
                    return Self::build_from_files(files, &config, &device);
                }
                Err(e) => {
                    tracing::warn!("  ❌ {} failed: {:?}", label, e);
                    last_err = Some(e);
                    continue;
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            anyhow::anyhow!("All HF endpoints failed for model: {}", config.model_id)
        }))
    }

    /// Download model files from a specific endpoint.
    async fn download_model(endpoint: &str, config: &CandleConfig) -> Result<DownloadedFiles> {
        // hf-hub reads HF_ENDPOINT env var for the endpoint URL.
        // Use tokio::sync::Mutex to avoid blocking the async runtime.
        static HF_ENDPOINT_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
            std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));
        let _guard = HF_ENDPOINT_LOCK.lock().await;
        // SAFETY: set_var is called under a mutex lock, no concurrent access
        // to the HF_ENDPOINT env var from other tasks.
        unsafe { std::env::set_var("HF_ENDPOINT", endpoint) };
        let api = hf_hub::api::tokio::Api::new()?;
        drop(_guard); // Release lock after Api is created (it captures endpoint at build time)

        // hf-hub 0.4: use Repo::with_revision for revision, Repo::new for default
        let repo = if let Some(ref rev) = config.revision {
            api.repo(Repo::with_revision(
                config.model_id.clone(),
                RepoType::Model,
                rev.clone(),
            ))
        } else {
            api.repo(Repo::new(config.model_id.clone(), RepoType::Model))
        };

        let model_file = repo.get("model.safetensors").await?;
        let config_file = repo.get("config.json").await?;
        let tokenizer_file = repo.get("tokenizer.json").await?;

        Ok(DownloadedFiles {
            model: model_file,
            config: config_file,
            tokenizer: tokenizer_file,
        })
    }

    /// Build the embedder from downloaded files.
    fn build_from_files(
        files: DownloadedFiles,
        config: &CandleConfig,
        device: &candle_core::Device,
    ) -> Result<Self> {
        // Load the model config
        let model_config: candle_transformers::models::bert::Config =
            serde_json::from_reader(std::fs::File::open(&files.config)?)?;

        // candle 0.8+: VarBuilder::from_mmaped_safetensors is now unsafe
        // (inherited from memmap2::MmapOptions)
        let vb = unsafe {
            candle_nn::VarBuilder::from_mmaped_safetensors(
                &[files.model],
                candle_core::DType::F32,
                device,
            )?
        };

        let model = candle_transformers::models::bert::BertModel::load(vb, &model_config)?;

        // Load tokenizer
        let tokenizer = tokenizers::Tokenizer::from_file(&files.tokenizer)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        let dimension = model_config.hidden_size;

        tracing::info!(
            "Embedding model loaded: dim={}, max_len={}",
            dimension,
            config.max_length
        );

        Ok(Self {
            device: device.clone(),
            model,
            tokenizer,
            dimension,
            max_length: config.max_length,
            pooling: config.pooling,
        })
    }

    /// Compute embeddings for a batch of texts.
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut embeddings = Vec::with_capacity(texts.len());

        for text in texts {
            let embedding = self.embed_single(text)?;
            embeddings.push(embedding);
        }

        Ok(embeddings)
    }

    /// Compute embedding for a single text.
    fn embed_single(&self, text: &str) -> Result<Vec<f32>> {
        // Tokenize
        let tokens = self
            .tokenizer
            .encode(text, false)
            .map_err(|e| anyhow::anyhow!("Tokenization error: {}", e))?;

        let token_ids: Vec<u32> = tokens.get_ids().to_vec();
        let attention_mask: Vec<f32> = tokens
            .get_attention_mask()
            .iter()
            .map(|&x| x as f32)
            .collect();

        // Truncate to max_length
        let len = token_ids.len().min(self.max_length);
        let token_ids = &token_ids[..len];
        let attention_mask = &attention_mask[..len];

        // Convert to Candle tensors
        let input_ids = candle_core::Tensor::new(token_ids, &self.device)?.unsqueeze(0)?;
        let attention_mask =
            candle_core::Tensor::new(attention_mask, &self.device)?.unsqueeze(0)?;
        let token_type_ids = input_ids.zeros_like()?;

        // Forward pass
        let output = self
            .model
            .forward(&input_ids, &token_type_ids, Some(&attention_mask))?;

        // Pool
        let pooled = match self.pooling {
            PoolingStrategy::Cls => {
                // CLS token (first token)
                let cls = output.narrow(1, 0, 1)?;
                cls.squeeze(0)?.squeeze(0)?
            }
            PoolingStrategy::Mean => {
                // Simple mean of all token embeddings
                let (_b, _s, _h) = output.dims3()?;
                output.mean(1)?
            }
            PoolingStrategy::MeanWithMask => {
                // Weighted mean with attention mask
                let mask_expanded = attention_mask.unsqueeze(2)?; // (1, seq, 1)
                let masked = output.mul(&mask_expanded)?;
                let sum = masked.sum(1)?;
                let mask_sum = mask_expanded.sum(1)?;
                let mask_sum = mask_sum.broadcast_div(&mask_sum)?;
                sum.div(&mask_sum.clamp(1e-9, f32::MAX)?)?
            }
        };

        // Normalize to unit length
        let norm = pooled.sqr()?.sum_all()?.sqrt()?.to_scalar::<f32>()?;
        let normalized = if norm > 0.0 {
            // candle 0.8+: affine expects f64
            pooled.affine(1.0f64 / norm as f64, 0.0)?
        } else {
            pooled
        };

        // Convert to Vec<f32>
        let data = normalized.to_vec1()?;
        Ok(data)
    }
}

#[async_trait::async_trait]
impl Embedder for CandleEmbedder {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.embed_batch(texts)
    }
}
