//! Provider catalog — mirrors RAGFlow `conf/llm_factories.json` + `rag/llm/*.py`
//! factories, adapted for mainland-China network reality.
//!
//! Every RAGFlow factory is listed with:
//!   - a default `api_base` (domestic endpoints preferred; empty-string entries
//!     in RAGFlow's JSON are filled from the provider's official docs)
//!   - a recommended API-key environment variable
//!   - a discovery/inference dialect (reuses [`crate::model_meta::ProviderDialect`])
//!   - capability flags: chat / embedding / rerank / image2text / tts / asr / ocr
//!   - a `domestic` flag (directly reachable from mainland China without a proxy)
//!
//! The catalog is pure data + lookups: no network, no side effects. Runtime
//! wiring (tenant model resolution, discovery, inference clients) consults it
//! through [`provider_base`], [`provider_api_key_env`] and [`provider_dialect`].

use crate::model_meta::ProviderDialect;

/// Provider capability kinds, aligned with RAGFlow `ModelType` and the RayRAG
/// `ModelCapability` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Chat,
    Embedding,
    Rerank,
    ImageToText,
    TextToSpeech,
    SpeechToText,
    Ocr,
}

impl ProviderKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Embedding => "embedding",
            Self::Rerank => "rerank",
            Self::ImageToText => "image2text",
            Self::TextToSpeech => "tts",
            Self::SpeechToText => "asr",
            Self::Ocr => "ocr",
        }
    }
}

/// Upstream `un-add-model.tsx::mapModelKey`: short display label per model type.
/// `vision` and `speech2text` share labels with their canonical counterparts.
pub const fn capability_short_label(capability: ProviderKind) -> &'static str {
    match capability {
        ProviderKind::Chat => "LLM",
        ProviderKind::Embedding => "Embedding",
        ProviderKind::Rerank => "Rerank",
        ProviderKind::ImageToText => "VLM",
        ProviderKind::TextToSpeech => "TTS",
        ProviderKind::SpeechToText => "ASR",
        ProviderKind::Ocr => "OCR",
    }
}

/// Resolve the upstream model-type string (`"image2text"`/`"vision"`,
/// `"asr"`/`"speech2text"`, …) to the same short label the React filter pills
/// use. Returns `None` for unknown keys so the caller can fall back to the key
/// verbatim (matching the upstream `|| tag.trim()` fallback).
pub fn capability_short_label_from_str(key: &str) -> Option<&'static str> {
    match key.trim() {
        "chat" => Some("LLM"),
        "embedding" => Some("Embedding"),
        "rerank" => Some("Rerank"),
        "tts" => Some("TTS"),
        "asr" | "speech2text" => Some("ASR"),
        "ocr" => Some("OCR"),
        "image2text" | "vision" => Some("VLM"),
        _ => None,
    }
}

/// Upstream `un-add-model.tsx::orderMap`: chat=1, embedding=2, rerank=3,
/// tts=4, asr/speech2text=5, image2text/vision=6, ocr=7. Unknown keys sink
/// to 999 so newly-added capabilities don't accidentally sort first.
/// Not `const`: `str` comparison/matching is not const-stable on rustc 1.97.
pub fn model_type_sort_order(key: &str) -> u32 {
    match key.trim() {
        "chat" => 1,
        "embedding" => 2,
        "rerank" => 3,
        "tts" => 4,
        "asr" | "speech2text" => 5,
        "image2text" | "vision" => 6,
        "ocr" => 7,
        _ => 999,
    }
}

/// Upstream `un-add-model.tsx::sortModelTypes`: stable sort by `orderMap`.
pub fn sort_model_types<T: AsRef<str>>(model_types: &mut [T]) {
    model_types.sort_by_key(|t| model_type_sort_order(t.as_ref()));
}

/// One entry of the provider catalog.
#[derive(Debug, Clone, Copy)]
pub struct ProviderPreset {
    /// RAGFlow factory id, e.g. `"ZHIPU-AI"`.
    pub id: &'static str,
    /// Display name used by RayRAG providers UI (lowercased by the store).
    pub name: &'static str,
    /// Default API base. Domestic-first: Chinese providers point at their
    /// mainland endpoints; self-hosted engines point at loopback.
    pub base_url: &'static str,
    /// Recommended environment variable holding the API key, e.g. `"ZHIPU_API_KEY"`.
    pub api_key_env: &'static str,
    /// Inference/discovery dialect.
    pub dialect: ProviderDialect,
    /// Directly reachable from mainland China.
    pub domestic: bool,
    /// Capability flags.
    pub kinds: &'static [ProviderKind],
    /// Representative model names (for seeding the provider models list).
    pub models: &'static [&'static str],
    /// Short annotation: endpoint flavour / caveats.
    pub note: &'static str,
}

/// All 63 RAGFlow factories plus the local engines. `domestic` marks providers
/// reachable without a proxy from mainland China; the rest remain available but
/// are annotated as requiring an international network.
pub static PROVIDER_PRESETS: &[ProviderPreset] = &[
    // ── 国内直连 (domestic, China-accessible) ──────────────────────────
    ProviderPreset {
        id: "ZHIPU-AI",
        name: "ZHIPU-AI",
        base_url: "https://open.bigmodel.cn/api/paas/v4",
        api_key_env: "ZHIPU_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::Rerank,
            ProviderKind::ImageToText,
        ],
        models: &["glm-4.5", "glm-4.7", "embedding-3", "rerank", "glm-4v-plus"],
        note: "智谱AI: GLM 系列 chat/embedding/rerank/视觉统一走 bigmodel.cn",
    },
    ProviderPreset {
        id: "DeepSeek",
        name: "DeepSeek",
        base_url: "https://api.deepseek.com/v1",
        api_key_env: "DEEPSEEK_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Chat],
        models: &["deepseek-chat", "deepseek-reasoner"],
        note: "深度求索: deepseek-chat / deepseek-reasoner",
    },
    ProviderPreset {
        id: "SILICONFLOW",
        name: "SILICONFLOW",
        base_url: "https://api.siliconflow.cn/v1",
        api_key_env: "SILICONFLOW_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::Rerank,
            ProviderKind::ImageToText,
        ],
        models: &[
            "Qwen/Qwen3-32B",
            "BAAI/bge-m3",
            "BAAI/bge-reranker-v2-m3",
            "Qwen/Qwen2.5-VL-72B",
        ],
        note: "硅基流动: 聚合开源模型, chat/embedding/rerank/视觉全支持, 国内直连",
    },
    ProviderPreset {
        id: "Moonshot",
        name: "Moonshot",
        base_url: "https://api.moonshot.cn/v1",
        api_key_env: "MOONSHOT_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Chat],
        models: &["kimi-k2-0711-preview", "moonshot-v1-32k"],
        note: "月之暗面 Kimi: moonshot.cn 国内端点",
    },
    ProviderPreset {
        id: "MiniMax",
        name: "MiniMax",
        base_url: "https://api.minimaxi.com/v1",
        api_key_env: "MINIMAX_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::ImageToText,
            ProviderKind::TextToSpeech,
        ],
        models: &[
            "MiniMax-M3",
            "MiniMax-M2",
            "MiniMax-Embedding",
            "abab6.5s-chat",
        ],
        note: "MiniMax: M3/M2 chat、Embedding、TTS, 国内直连; RayRAG 默认提供商",
    },
    ProviderPreset {
        id: "Tongyi-Qianwen",
        name: "Tongyi-Qianwen",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        api_key_env: "DASHSCOPE_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::ImageToText,
        ],
        models: &["qwen-max", "qwen-plus", "text-embedding-v4", "qwen-vl-max"],
        note: "阿里云百炼/通义千问: DashScope compatible-mode OpenAI 兼容",
    },
    ProviderPreset {
        id: "VolcEngine",
        name: "VolcEngine",
        base_url: "https://ark.cn-beijing.volces.com/api/v3",
        api_key_env: "ARK_API_KEY",
        dialect: ProviderDialect::VolcEngine,
        domestic: true,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::ImageToText,
        ],
        models: &[
            "doubao-1-5-pro-32k-250115",
            "doubao-embedding",
            "doubao-1-5-vision-pro",
        ],
        note: "火山方舟: 豆包系列, 密钥字段为 ark_api_key; 发现走 /api/v3/models",
    },
    ProviderPreset {
        id: "BaiduYiyan",
        name: "BaiduYiyan",
        base_url: "https://qianfan.baidubce.com/v2",
        api_key_env: "QIANFAN_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::ImageToText,
        ],
        models: &[
            "ernie-4.5-turbo-128k",
            "ernie-3.5-8k",
            "embedding-v1",
            "ernie-4.5-vl-128k",
        ],
        note: "百度千帆: ERNIE 系列, v2 OpenAI 兼容端点",
    },
    ProviderPreset {
        id: "Tencent Hunyuan",
        name: "Tencent Hunyuan",
        base_url: "https://api.hunyuan.cloud.tencent.com/v1",
        api_key_env: "HUNYUAN_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Chat],
        models: &["hunyuan-turbos-latest", "hunyuan-standard"],
        note: "腾讯混元: 国内直连, OpenAI 兼容",
    },
    ProviderPreset {
        id: "XunFei Spark",
        name: "XunFei Spark",
        base_url: "https://spark-api-open.xf-yun.com/v1",
        api_key_env: "SPARK_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Chat, ProviderKind::ImageToText],
        models: &["generalv3.5", "lite"],
        note: "讯飞星火: spark-api-open OpenAI 兼容",
    },
    ProviderPreset {
        id: "ModelScope",
        name: "ModelScope",
        base_url: "https://api-inference.modelscope.cn/v1",
        api_key_env: "MODELSCOPE_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::ImageToText,
        ],
        models: &["Qwen/Qwen3-32B-Instruct", "AI-ModelScope/bge-m3"],
        note: "魔搭社区: 国内直连聚合推理",
    },
    ProviderPreset {
        id: "GiteeAI",
        name: "GiteeAI",
        base_url: "https://ai.gitee.com/v1",
        api_key_env: "GITEE_AI_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::Rerank,
        ],
        models: &["Qwen3-235B-A22B", "bge-m3"],
        note: "Gitee AI: 国内直连, chat/embedding/rerank",
    },
    ProviderPreset {
        id: "BaiChuan",
        name: "BaiChuan",
        base_url: "https://api.baichuan-ai.com/v1",
        api_key_env: "BAICHUAN_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Chat],
        models: &["Baichuan4", "Baichuan3-Turbo"],
        note: "百川智能",
    },
    ProviderPreset {
        id: "TokenPony",
        name: "TokenPony",
        base_url: "https://ragflow.vip-api.tokenpony.cn/v1",
        api_key_env: "TOKENPONY_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Chat],
        models: &["gpt-4o-mini"],
        note: "国内中转聚合 API",
    },
    ProviderPreset {
        id: "n1n",
        name: "n1n",
        base_url: "https://api.n1n.ai/v1",
        api_key_env: "N1N_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Chat],
        models: &["claude-3-5-sonnet-20241022"],
        note: "国内中转聚合 API",
    },
    ProviderPreset {
        id: "Astraflow-CN",
        name: "Astraflow-CN",
        base_url: "https://api.modelverse.cn/v1",
        api_key_env: "ASTRAFLOW_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Chat],
        models: &["claude-sonnet-4-20250514"],
        note: "国内中转聚合 API (国际版为 Astraflow)",
    },
    ProviderPreset {
        id: "LongCat",
        name: "LongCat",
        base_url: "https://api.longcat.chat/openai",
        api_key_env: "LONGCAT_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Chat],
        models: &["gpt-4o"],
        note: "国内中转聚合 API",
    },
    ProviderPreset {
        id: "OpenAI-API-Compatible",
        name: "OpenAI-API-Compatible",
        base_url: "http://127.0.0.1:8088/v1",
        api_key_env: "OPENAI_COMPATIBLE_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::Rerank,
            ProviderKind::ImageToText,
        ],
        models: &["Qwen3.5-9B", "Qwen3-Embedding-4B", "mxbai-rerank-large-v2"],
        note: "任意 OpenAI 兼容端点 (llama.cpp/vLLM/LiteLLM); 默认指向本机 GPU llama-server 8088",
    },
    ProviderPreset {
        id: "VLLM",
        name: "VLLM",
        base_url: "http://127.0.0.1:8000/v1",
        api_key_env: "VLLM_API_KEY",
        dialect: ProviderDialect::Vllm,
        domestic: true,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::ImageToText,
        ],
        models: &["Qwen/Qwen3-32B-Instruct"],
        note: "vLLM 自托管; 发现走 /v1/models",
    },
    ProviderPreset {
        id: "Ollama",
        name: "Ollama",
        base_url: "http://127.0.0.1:11434",
        api_key_env: "OLLAMA_API_KEY",
        dialect: ProviderDialect::Ollama,
        domestic: true,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::ImageToText,
        ],
        models: &["qwen3:32b", "bge-m3"],
        note: "Ollama 本地; 发现走 /api/tags + /api/show",
    },
    ProviderPreset {
        id: "Xinference",
        name: "Xinference",
        base_url: "http://127.0.0.1:9997/v1",
        api_key_env: "XINFERENCE_API_KEY",
        dialect: ProviderDialect::Xinference,
        domestic: true,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::Rerank,
            ProviderKind::ImageToText,
            ProviderKind::TextToSpeech,
            ProviderKind::SpeechToText,
        ],
        models: &["qwen3", "bge-m3", "bge-reranker-v2-m3"],
        note: "Xinference 本地推理平台, 六类能力全支持",
    },
    ProviderPreset {
        id: "LocalAI",
        name: "LocalAI",
        base_url: "http://127.0.0.1:8080/v1",
        api_key_env: "LOCALAI_API_KEY",
        dialect: ProviderDialect::LocalAi,
        domestic: true,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::ImageToText,
        ],
        models: &["gpt-4"],
        note: "LocalAI 本地 OpenAI 兼容",
    },
    ProviderPreset {
        id: "LM-Studio",
        name: "LM-Studio",
        base_url: "http://127.0.0.1:1234/v1",
        api_key_env: "LMSTUDIO_API_KEY",
        dialect: ProviderDialect::LmStudio,
        domestic: true,
        kinds: &[ProviderKind::Chat, ProviderKind::Embedding],
        models: &["qwen3-32b"],
        note: "LM Studio 本地",
    },
    ProviderPreset {
        id: "GPUStack",
        name: "GPUStack",
        base_url: "http://127.0.0.1:8080/v1",
        api_key_env: "GPUSTACK_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::ImageToText,
        ],
        models: &["qwen3"],
        note: "GPUStack 本地 GPU 集群",
    },
    ProviderPreset {
        id: "Builtin",
        name: "Builtin",
        base_url: "builtin://candle",
        api_key_env: "",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Embedding],
        models: &["all-MiniLM-L6-v2"],
        note: "RayRAG 内置 Candle 本地 embedding (离线)",
    },
    ProviderPreset {
        id: "PaddleOCR",
        name: "PaddleOCR",
        base_url: "https://paddleocr.aistudio-app.com",
        api_key_env: "PADDLEOCR_ACCESS_TOKEN",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Ocr],
        models: &["PaddleOCR-VL"],
        note: "PaddleOCR 云端/本地 GGUF 8090 (解析器已接入)",
    },
    ProviderPreset {
        id: "MinerU",
        name: "MinerU",
        base_url: "",
        api_key_env: "MINERU_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Ocr],
        models: &["mineru"],
        note: "MinerU 文档解析 (解析器已接入)",
    },
    ProviderPreset {
        id: "SoMark",
        name: "SoMark",
        base_url: "https://somark.tech/api/v1",
        api_key_env: "SOMARK_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Ocr],
        models: &["somark"],
        note: "SoMark 版面识别 SaaS (解析器已接入)",
    },
    ProviderPreset {
        id: "OpenDataLoader",
        name: "OpenDataLoader",
        base_url: "",
        api_key_env: "OPENDATALOADER_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Ocr],
        models: &["opendataloader"],
        note: "OpenDataLoader 远程 PDF 解析 (解析器已接入)",
    },
    // ── 国际端点 (require an international network) ────────────────────
    ProviderPreset {
        id: "OpenAI",
        name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        api_key_env: "OPENAI_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::ImageToText,
            ProviderKind::TextToSpeech,
            ProviderKind::SpeechToText,
        ],
        models: &[
            "gpt-4o",
            "gpt-4o-mini",
            "text-embedding-3-large",
            "gpt-4o-mini-tts",
        ],
        note: "OpenAI 官方",
    },
    ProviderPreset {
        id: "Azure-OpenAI",
        name: "Azure-OpenAI",
        base_url: "https://<resource>.openai.azure.com/openai/deployments",
        api_key_env: "AZURE_OPENAI_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::ImageToText,
        ],
        models: &["gpt-4o"],
        note: "Azure OpenAI, 需替换 resource 与 deployment",
    },
    ProviderPreset {
        id: "Anthropic",
        name: "Anthropic",
        base_url: "https://api.anthropic.com/",
        api_key_env: "ANTHROPIC_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat, ProviderKind::ImageToText],
        models: &["claude-sonnet-4", "claude-3-5-haiku"],
        note: "Anthropic Claude; 经 RayRAG OpenAI 兼容适配层转发",
    },
    ProviderPreset {
        id: "Gemini",
        name: "Gemini",
        base_url: "https://generativelanguage.googleapis.com/v1beta",
        api_key_env: "GEMINI_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::ImageToText,
        ],
        models: &["gemini-2.5-pro", "gemini-2.5-flash", "text-embedding-004"],
        note: "Google Gemini",
    },
    ProviderPreset {
        id: "xAI",
        name: "xAI",
        base_url: "https://api.x.ai/v1",
        api_key_env: "XAI_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat, ProviderKind::ImageToText],
        models: &["grok-4", "grok-3"],
        note: "xAI Grok",
    },
    ProviderPreset {
        id: "OpenRouter",
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        api_key_env: "OPENROUTER_API_KEY",
        dialect: ProviderDialect::OpenRouter,
        domestic: false,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::ImageToText,
        ],
        models: &["anthropic/claude-sonnet-4", "openai/gpt-4o"],
        note: "OpenRouter 聚合; 发现走 /api/v1/models?output_modalities=all",
    },
    ProviderPreset {
        id: "Groq",
        name: "Groq",
        base_url: "https://api.groq.com/openai/v1",
        api_key_env: "GROQ_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat],
        models: &["llama-3.3-70b-versatile"],
        note: "Groq 快速推理",
    },
    ProviderPreset {
        id: "Mistral",
        name: "Mistral",
        base_url: "https://api.mistral.ai/v1",
        api_key_env: "MISTRAL_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat, ProviderKind::Embedding],
        models: &["mistral-large-latest", "mistral-embed"],
        note: "Mistral",
    },
    ProviderPreset {
        id: "Cohere",
        name: "Cohere",
        base_url: "https://api.cohere.com/v1",
        api_key_env: "COHERE_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Rerank,
            ProviderKind::Embedding,
        ],
        models: &[
            "command-r-plus",
            "rerank-multilingual-v3.0",
            "embed-english-v3.0",
        ],
        note: "Cohere chat/rerank/embedding",
    },
    ProviderPreset {
        id: "Jina",
        name: "Jina",
        base_url: "https://api.jina.ai/v1",
        api_key_env: "JINA_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::Rerank,
        ],
        models: &[
            "jina-chat",
            "jina-embeddings-v3",
            "jina-reranker-v2-base-multilingual",
        ],
        note: "Jina embedding/rerank/chat",
    },
    ProviderPreset {
        id: "Voyage AI",
        name: "Voyage AI",
        base_url: "https://api.voyageai.com/v1",
        api_key_env: "VOYAGE_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Embedding, ProviderKind::Rerank],
        models: &["voyage-3-large", "voyage-multilingual-2", "rerank-2"],
        note: "Voyage embedding/rerank",
    },
    ProviderPreset {
        id: "StepFun",
        name: "StepFun",
        base_url: "https://api.stepfun.com/v1",
        api_key_env: "STEPFUN_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat, ProviderKind::ImageToText],
        models: &["step-3-8k", "step-1v-8k"],
        note: "阶跃星辰 (国际端点, 国内可尝试 api.stepfun.com 直连)",
    },
    ProviderPreset {
        id: "NVIDIA",
        name: "NVIDIA",
        base_url: "https://integrate.api.nvidia.com/v1",
        api_key_env: "NVIDIA_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::ImageToText,
        ],
        models: &["meta/llama-3.3-70b-instruct", "nvidia/embed-qa-4"],
        note: "NVIDIA NIM 聚合",
    },
    ProviderPreset {
        id: "TogetherAI",
        name: "TogetherAI",
        base_url: "https://api.together.xyz/v1",
        api_key_env: "TOGETHER_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat, ProviderKind::Embedding],
        models: &["meta-llama/Llama-3.3-70B-Instruct-Turbo"],
        note: "Together AI",
    },
    ProviderPreset {
        id: "Upstage",
        name: "Upstage",
        base_url: "https://api.upstage.ai/v1/solar",
        api_key_env: "UPSTAGE_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat, ProviderKind::ImageToText],
        models: &["solar-pro"],
        note: "Upstage Solar",
    },
    ProviderPreset {
        id: "NovitaAI",
        name: "NovitaAI",
        base_url: "https://api.novita.ai/v3/openai",
        api_key_env: "NOVITA_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat, ProviderKind::ImageToText],
        models: &["deepseek/deepseek-v3"],
        note: "Novita AI",
    },
    ProviderPreset {
        id: "PPIO",
        name: "PPIO",
        base_url: "https://api.ppio.ai/v1",
        api_key_env: "PPIO_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::ImageToText,
        ],
        models: &["gpt-4o"],
        note: "PPIO",
    },
    ProviderPreset {
        id: "Replicate",
        name: "Replicate",
        base_url: "https://api.replicate.com/v1",
        api_key_env: "REPLICATE_API_KEY",
        dialect: ProviderDialect::Replicate,
        domestic: false,
        kinds: &[ProviderKind::Chat, ProviderKind::ImageToText],
        models: &["meta/meta-llama-3-70b-instruct"],
        note: "Replicate; 模型发现不适用, 使用静态模型列表",
    },
    ProviderPreset {
        id: "siliconflow_intl",
        name: "siliconflow_intl",
        base_url: "https://api.siliconflow.com/v1",
        api_key_env: "SILICONFLOW_INTL_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::Rerank,
        ],
        models: &["Qwen/Qwen3-32B"],
        note: "硅基流动国际端点",
    },
    ProviderPreset {
        id: "DeepInfra",
        name: "DeepInfra",
        base_url: "https://api.deepinfra.com/v1",
        api_key_env: "DEEPINFRA_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::ImageToText,
        ],
        models: &["meta-llama/Llama-3.3-70B-Instruct"],
        note: "DeepInfra",
    },
    ProviderPreset {
        id: "Perplexity",
        name: "Perplexity",
        base_url: "https://api.perplexity.ai",
        api_key_env: "PERPLEXITY_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat],
        models: &["sonar-pro", "sonar"],
        note: "Perplexity Sonar 联网搜索",
    },
    ProviderPreset {
        id: "302.AI",
        name: "302.AI",
        base_url: "https://api.302.ai/v1",
        api_key_env: "API302_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::Embedding,
            ProviderKind::Rerank,
            ProviderKind::ImageToText,
        ],
        models: &["gpt-4o", "text-embedding-3-small"],
        note: "302.AI 聚合",
    },
    ProviderPreset {
        id: "CometAPI",
        name: "CometAPI",
        base_url: "https://api.cometapi.com/v1",
        api_key_env: "COMET_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat],
        models: &["gpt-4o"],
        note: "CometAPI 聚合",
    },
    ProviderPreset {
        id: "DeerAPI",
        name: "DeerAPI",
        base_url: "https://api.deerapi.com/v1",
        api_key_env: "DEER_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat],
        models: &["gpt-4o"],
        note: "DeerAPI 聚合",
    },
    ProviderPreset {
        id: "Jiekou.AI",
        name: "Jiekou.AI",
        base_url: "https://api.jiekou.ai/v1",
        api_key_env: "JIEKOU_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat],
        models: &["gpt-4o"],
        note: "接口AI 聚合",
    },
    ProviderPreset {
        id: "Astraflow",
        name: "Astraflow",
        base_url: "https://api-us-ca.umodelverse.ai/v1",
        api_key_env: "ASTRAFLOW_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat],
        models: &["claude-sonnet-4-20250514"],
        note: "Astraflow 国际端点",
    },
    ProviderPreset {
        id: "FuturMix",
        name: "FuturMix",
        base_url: "https://futurmix.ai/v1",
        api_key_env: "FUTURMIX_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat],
        models: &["gpt-4o"],
        note: "FuturMix 聚合",
    },
    ProviderPreset {
        id: "Avian",
        name: "Avian",
        base_url: "https://api.avian.io/v1",
        api_key_env: "AVIAN_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat],
        models: &["gpt-4o"],
        note: "Avian 聚合",
    },
    ProviderPreset {
        id: "RAGcon",
        name: "RAGcon",
        base_url: "https://api.ragcon.ai/v1",
        api_key_env: "RAGCON_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat],
        models: &["gpt-4o"],
        note: "RAGcon 聚合",
    },
    ProviderPreset {
        id: "Bedrock",
        name: "Bedrock",
        base_url: "https://bedrock-runtime.us-east-1.amazonaws.com",
        api_key_env: "AWS_ACCESS_KEY_ID",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat],
        models: &["anthropic.claude-sonnet-4-20250514"],
        note: "AWS Bedrock; 需 AWS 签名, 建议经 OpenAI 兼容网关接入",
    },
    ProviderPreset {
        id: "Google Cloud",
        name: "Google Cloud",
        base_url: "https://us-central1-aiplatform.googleapis.com/v1",
        api_key_env: "GOOGLE_CLOUD_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat, ProviderKind::Embedding],
        models: &["gemini-2.5-pro"],
        note: "Google Vertex AI",
    },
    ProviderPreset {
        id: "HuggingFace",
        name: "HuggingFace",
        base_url: "https://api-inference.huggingface.co/v1",
        api_key_env: "HF_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat, ProviderKind::ImageToText],
        models: &["meta-llama/Llama-3.3-70B-Instruct"],
        note: "HuggingFace Inference",
    },
    ProviderPreset {
        id: "FastEmbed",
        name: "FastEmbed",
        base_url: "http://127.0.0.1:8094",
        api_key_env: "",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Embedding],
        models: &["BAAI/bge-small-en-v1.5"],
        note: "Qdrant FastEmbed 本地服务",
    },
    ProviderPreset {
        id: "Fish Audio",
        name: "Fish Audio",
        base_url: "https://api.fish.audio",
        api_key_env: "FISH_AUDIO_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::TextToSpeech, ProviderKind::SpeechToText],
        models: &["fish-speech-1.5"],
        note: "Fish Audio TTS/ASR",
    },
    ProviderPreset {
        id: "Tencent Cloud",
        name: "Tencent Cloud",
        base_url: "https://api.hunyuan.cloud.tencent.com/v1",
        api_key_env: "TENCENT_CLOUD_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Chat, ProviderKind::ImageToText],
        models: &["hunyuan-turbos-latest"],
        note: "腾讯云 (与混元共用端点)",
    },
    ProviderPreset {
        id: "Xiaomi",
        name: "Xiaomi",
        base_url: "https://api.xiaomimimo.com/v1",
        api_key_env: "XIAOMI_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[
            ProviderKind::Chat,
            ProviderKind::SpeechToText,
            ProviderKind::TextToSpeech,
        ],
        models: &[
            "mimo-v2.5-pro",
            "mimo-v2.5",
            "mimo-v2.5-asr",
            "mimo-v2.5-tts",
        ],
        note: "小米 MiMo (国内直连)",
    },
    ProviderPreset {
        id: "Qiniu",
        name: "Qiniu",
        base_url: "https://api.qnaigc.com/v1",
        api_key_env: "QINIU_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Chat],
        models: &[
            "deepseek/deepseek-v4-flash",
            "deepseek/deepseek-v4-pro",
            "moonshotai/kimi-k2.6",
        ],
        note: "七牛云 AI (国内直连)",
    },
    ProviderPreset {
        id: "HuaweiCloud",
        name: "HuaweiCloud",
        base_url: "https://api.modelarts-maas.com",
        api_key_env: "HUAWEICLOUD_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Chat, ProviderKind::Embedding],
        models: &["deepseek-v4-pro", "deepseek-v4-flash", "deepseek-v3.2"],
        note: "华为云 ModelArts MaaS (国内直连)",
    },
    ProviderPreset {
        id: "TokenHub",
        name: "TokenHub",
        base_url: "https://aitok.cc/v1",
        api_key_env: "TOKENHUB_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: true,
        kinds: &[ProviderKind::Chat, ProviderKind::Embedding],
        models: &["gpt-4o-mini", "gpt-4o", "gpt-4"],
        note: "聚合中转站 (国内直连)",
    },
    ProviderPreset {
        id: "OrcaRouter",
        name: "OrcaRouter",
        base_url: "https://api.orcarouter.ai",
        api_key_env: "ORCAROUTER_API_KEY",
        dialect: ProviderDialect::OpenAiCompatible,
        domestic: false,
        kinds: &[ProviderKind::Chat, ProviderKind::TextToSpeech],
        models: &["orcarouter/auto", "openai/tts-1"],
        note: "模型路由 (需国际网络)",
    },
];

/// Normalize a provider id/name for lookup: lowercase, strip spaces and hyphens.
fn normalize_provider_name(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Model-catalog fixture key → preset id. The provider model catalog
/// (`src/api/fixtures/models/<key>.json` filenames, mirrored verbatim from
/// RAGFlow `conf/models/`) uses keys that differ from the RAGFlow factory
/// ids in `PROVIDER_PRESETS` for several factories (e.g. fixture `aliyun`
/// is factory `Tongyi-Qianwen`). This table bridges the two so that
/// `provider_preset("aliyun")`, `provider_is_domestic("aliyun")` and
/// `factory_endpoint("aliyun")` resolve correctly.
const FIXTURE_TO_PRESET: &[(&str, &str)] = &[
    ("302ai", "302.AI"),
    ("aliyun", "Tongyi-Qianwen"),
    ("astraflow", "Astraflow"),
    ("avian", "Avian"),
    ("baidu", "BaiduYiyan"),
    ("fishaudio", "Fish Audio"),
    ("futurmix", "FuturMix"),
    ("gitee", "GiteeAI"),
    ("google", "Google Cloud"),
    ("huaweicloud", "HuaweiCloud"),
    ("hunyuan", "Tencent Hunyuan"),
    ("jiekouai", "Jiekou.AI"),
    ("lmstudio", "LM-Studio"),
    ("mineru_local", "MinerU"),
    ("n1n", "n1n"),
    ("novita", "NovitaAI"),
    ("orcarouter", "OrcaRouter"),
    ("paddleocr_local", "PaddleOCR"),
    ("qiniu", "Qiniu"),
    ("tokenhub", "TokenHub"),
    ("tokenpony", "TokenPony"),
    ("voyage", "Voyage AI"),
    ("xiaomi", "Xiaomi"),
    ("xunfei", "XunFei Spark"),
];

/// Slugify a provider id for the providers store: lowercase, spaces and
/// underscores become hyphens, other punctuation is dropped. The result only
/// contains lowercase letters, digits and hyphens (validated by the store).
pub fn provider_id_slug(provider_id: &str) -> String {
    let mut slug = String::with_capacity(provider_id.len());
    for c in provider_id.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

/// Look up a preset by provider id (case/hyphen insensitive).
///
/// Falls back through `FIXTURE_TO_PRESET` so model-catalog fixture keys
/// (e.g. `aliyun`, `hunyuan`) resolve onto their RAGFlow factory preset.
pub fn provider_preset(provider_id: &str) -> Option<&'static ProviderPreset> {
    let needle = normalize_provider_name(provider_id);
    let direct = PROVIDER_PRESETS
        .iter()
        .find(|preset| normalize_provider_name(preset.id) == needle);
    direct.or_else(|| {
        let factory = FIXTURE_TO_PRESET
            .iter()
            .find(|(key, _)| normalize_provider_name(key) == needle)
            .map(|(_, id)| *id)?;
        let needle = normalize_provider_name(factory);
        PROVIDER_PRESETS
            .iter()
            .find(|preset| normalize_provider_name(preset.id) == needle)
    })
}

/// Look up a preset by display name (case/hyphen insensitive).
pub fn provider_preset_by_name(provider_name: &str) -> Option<&'static ProviderPreset> {
    let needle = normalize_provider_name(provider_name);
    PROVIDER_PRESETS.iter().find(|preset| {
        normalize_provider_name(preset.name) == needle
            || normalize_provider_name(preset.id) == needle
    })
}

/// Resolve a persisted/provider-store identity to the canonical RAGFlow
/// factory name. Older RayRAG data may use the `conf/models` fixture key or
/// display name (for example `aliyun` / `Aliyun`) while the v0.26.4 wire UI
/// uses `Tongyi-Qianwen` consistently.
pub fn canonical_provider_name(provider_id: &str, provider_name: &str) -> String {
    provider_preset(provider_id)
        .or_else(|| provider_preset_by_name(provider_name))
        .map(|preset| preset.name.to_string())
        .unwrap_or_else(|| provider_name.trim().to_string())
}

/// Resolve the default API base for a provider (id or name), if a preset exists.
pub fn provider_default_base(provider_id: &str, provider_name: &str) -> Option<&'static str> {
    provider_preset(provider_id)
        .or_else(|| provider_preset_by_name(provider_name))
        .map(|preset| preset.base_url)
        .filter(|base| !base.is_empty() && !base.starts_with("builtin://"))
}

/// Resolve the recommended API-key environment variable for a provider.
pub fn provider_api_key_env(provider_id: &str, provider_name: &str) -> Option<&'static str> {
    provider_preset(provider_id)
        .or_else(|| provider_preset_by_name(provider_name))
        .map(|preset| preset.api_key_env)
        .filter(|env| !env.is_empty())
}

/// Whether a provider is directly reachable from mainland China.
pub fn provider_is_domestic(provider_id: &str, provider_name: &str) -> bool {
    provider_preset(provider_id)
        .or_else(|| provider_preset_by_name(provider_name))
        .map(|preset| preset.domestic)
        .unwrap_or(false)
}

/// Whether a provider advertises the given capability.
pub fn provider_has_kind(provider_id: &str, provider_name: &str, kind: ProviderKind) -> bool {
    provider_preset(provider_id)
        .or_else(|| provider_preset_by_name(provider_name))
        .map(|preset| preset.kinds.contains(&kind))
        .unwrap_or(false)
}

/// Dialect for a provider, falling back to OpenAI-compatible.
pub fn provider_dialect(provider_id: &str, provider_name: &str) -> ProviderDialect {
    provider_preset(provider_id)
        .or_else(|| provider_preset_by_name(provider_name))
        .map(|preset| preset.dialect)
        .unwrap_or(ProviderDialect::OpenAiCompatible)
}

/// Representative model names for a provider preset.
pub fn provider_default_models(provider_id: &str, provider_name: &str) -> Vec<String> {
    provider_preset(provider_id)
        .or_else(|| provider_preset_by_name(provider_name))
        .map(|preset| {
            preset
                .models
                .iter()
                .map(|model| (*model).to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// All presets supporting a capability, sorted domestic-first then by id.
pub fn presets_for(kind: ProviderKind) -> Vec<&'static ProviderPreset> {
    let mut presets: Vec<&'static ProviderPreset> = PROVIDER_PRESETS
        .iter()
        .filter(|preset| preset.kinds.contains(&kind))
        .collect();
    presets.sort_by_key(|preset| (std::cmp::Reverse(preset.domestic), preset.id));
    presets
}

/// Count of catalog entries (63 RAGFlow factories + local engines).
pub const fn preset_count() -> usize {
    PROVIDER_PRESETS.len()
}

// ── Full factory model catalog (mirrors RAGFlow conf/llm_factories.json) ──
//
// The embedded catalog carries the *complete* per-factory model list (943
// models across the 63 RAGFlow factories: chat / embedding / rerank /
// image2text / speech2text / tts), generated from the upstream JSON so the
// model-discovery layer can seed every model a factory offers — not just the
// representative names in `ProviderPreset::models`.

/// One model entry inside the embedded factory catalog.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub struct FactoryModel {
    /// Model name as presented by the provider, e.g. `"deepseek-chat"`.
    pub n: &'static str,
    /// RAGFlow model type: chat / embedding / rerank / image2text /
    /// speech2text / tts (the upstream JSON uses these literals).
    pub t: &'static str,
    /// Max context tokens (0 when the upstream entry omits it).
    pub mx: u64,
    /// Whether the model supports tool calling.
    pub tools: bool,
}

/// The full factory catalog embedded at compile time.
static FACTORY_CATALOG_JSON: &str = include_str!("data/providers/llm_factories.json");

/// Lazily parsed factory catalog: `normalized factory name → entries`.
fn factory_catalog() -> &'static std::collections::HashMap<String, Vec<FactoryModel>> {
    use std::sync::OnceLock;
    static CATALOG: OnceLock<std::collections::HashMap<String, Vec<FactoryModel>>> =
        OnceLock::new();
    CATALOG.get_or_init(|| {
        let v: serde_json::Value = serde_json::from_str(FACTORY_CATALOG_JSON).unwrap_or_default();
        let mut map = std::collections::HashMap::new();
        if let Some(obj) = v.as_object() {
            for (name, entry) in obj {
                let mut models = Vec::new();
                let mut seen_names = std::collections::HashSet::new();
                if let Some(list) = entry.get("models").and_then(|m| m.as_array()) {
                    for m in list {
                        let n = m
                            .get("n")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string();
                        // The providers store requires unique model names; the
                        // upstream JSON occasionally lists the same model twice
                        // (e.g. FuturMix gpt-4o), so deduplicate defensively.
                        if n.is_empty() || !seen_names.insert(n.clone()) {
                            continue;
                        }
                        let t = m
                            .get("t")
                            .and_then(|x| x.as_str())
                            .unwrap_or("chat")
                            .to_string();
                        let mx = m.get("mx").and_then(|x| x.as_u64()).unwrap_or(0);
                        let tools = m.get("tools").and_then(|x| x.as_bool()).unwrap_or(false);
                        models.push(FactoryModel {
                            n: Box::leak(n.into_boxed_str()),
                            t: Box::leak(t.into_boxed_str()),
                            mx,
                            tools,
                        });
                    }
                }
                map.insert(normalize_provider_name(name), models);
            }
        }
        map
    })
}

/// All models a factory offers, straight from the embedded full catalog
/// (not just the representative `ProviderPreset::models` list).
pub fn factory_full_models(provider_id: &str, provider_name: &str) -> Vec<FactoryModel> {
    let needle = normalize_provider_name(provider_id);
    let catalog = factory_catalog();
    if let Some(models) = catalog.get(&needle) {
        return models.clone();
    }
    let by_name = normalize_provider_name(provider_name);
    if !by_name.is_empty()
        && let Some(models) = catalog.get(&by_name) {
            return models.clone();
        }
    Vec::new()
}

/// Full model names (strings) for a factory, for seeding provider rows.
pub fn factory_full_model_names(provider_id: &str, provider_name: &str) -> Vec<String> {
    factory_full_models(provider_id, provider_name)
        .iter()
        .map(|m| m.n.to_string())
        .collect()
}

/// Whether the full catalog knows this factory at all.
pub fn factory_known(provider_id: &str, provider_name: &str) -> bool {
    let needle = normalize_provider_name(provider_id);
    factory_catalog().contains_key(&needle)
        || (!normalize_provider_name(provider_name).is_empty()
            && factory_catalog().contains_key(&normalize_provider_name(provider_name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_all_ragflow_factories() {
        // RAGFlow v0.25.2 conf/llm_factories.json lists 63 factories.
        assert!(preset_count() >= 63);
        // Spot-check the factory ids that RayRAG must mirror.
        for id in [
            "OpenAI",
            "Tongyi-Qianwen",
            "ZHIPU-AI",
            "Moonshot",
            "DeepSeek",
            "MiniMax",
            "SILICONFLOW",
            "Tencent Hunyuan",
            "BaiduYiyan",
            "VolcEngine",
            "Ollama",
            "Xinference",
            "Anthropic",
            "Gemini",
        ] {
            assert!(provider_preset(id).is_some(), "missing preset for {id}");
        }
    }

    #[test]
    fn fixture_keys_resolve_to_presets_and_keep_domestic_flags() {
        // Every model-catalog fixture key either matches a preset directly or
        // resolves through FIXTURE_TO_PRESET, and the domestic classification
        // follows the RAGFlow factory (CN providers stay domestic).
        for (fixture, factory) in FIXTURE_TO_PRESET {
            let preset = provider_preset(fixture)
                .unwrap_or_else(|| panic!("fixture {fixture} must resolve to a preset"));
            assert_eq!(
                normalize_provider_name(preset.id),
                normalize_provider_name(factory),
                "fixture {fixture} resolved to wrong preset {}",
                preset.id
            );
        }
        // Domestic CN providers must stay marked reachable without a proxy.
        for id in [
            "aliyun",
            "baidu",
            "hunyuan",
            "xunfei",
            "gitee",
            "xiaomi",
            "qiniu",
            "huaweicloud",
        ] {
            assert!(
                provider_is_domestic(id, id),
                "{id} must be classified domestic (CN provider)"
            );
        }
        // International providers keep their annotation.
        assert!(!provider_is_domestic("voyage", "voyage"));
        assert!(!provider_is_domestic("orcarouter", "orcarouter"));
        // Newly added presets carry a default endpoint.
        for id in ["Xiaomi", "Qiniu", "HuaweiCloud", "TokenHub", "OrcaRouter"] {
            let preset = provider_preset(id).unwrap();
            assert!(!preset.base_url.is_empty(), "{id} preset needs a base_url");
        }
    }

    #[test]
    fn canonical_names_bridge_legacy_fixture_identities() {
        assert_eq!(
            canonical_provider_name("aliyun", "Aliyun"),
            "Tongyi-Qianwen"
        );
        assert_eq!(canonical_provider_name("google", "Google"), "Google Cloud");
        assert_eq!(
            canonical_provider_name("hunyuan", "HunYuan"),
            "Tencent Hunyuan"
        );
        assert_eq!(canonical_provider_name("nvidia", "Nvidia"), "NVIDIA");
        assert_eq!(canonical_provider_name("custom", "My Custom"), "My Custom");
    }

    #[test]
    fn lookups_are_case_and_hyphen_insensitive() {
        assert!(provider_preset("zhipu-ai").is_some());
        assert!(provider_preset("ZHIPU AI").is_some());
        assert!(provider_preset_by_name("zhipu ai").is_some());
        assert_eq!(
            provider_preset("deepseek").map(|p| p.dialect),
            Some(ProviderDialect::OpenAiCompatible)
        );
    }

    #[test]
    fn domestic_flags_are_sane() {
        assert!(provider_is_domestic("ZHIPU-AI", "ZHIPU-AI"));
        assert!(provider_is_domestic("DeepSeek", "DeepSeek"));
        assert!(provider_is_domestic("SILICONFLOW", "SILICONFLOW"));
        assert!(provider_is_domestic("MiniMax", "MiniMax"));
        assert!(provider_is_domestic(
            "OpenAI-API-Compatible",
            "OpenAI-API-Compatible"
        ));
        assert!(!provider_is_domestic("OpenAI", "OpenAI"));
        assert!(!provider_is_domestic("Anthropic", "Anthropic"));
        assert!(!provider_is_domestic("", ""));
    }

    #[test]
    fn base_urls_point_at_domestic_endpoints() {
        assert_eq!(
            provider_default_base("DeepSeek", "DeepSeek"),
            Some("https://api.deepseek.com/v1")
        );
        assert_eq!(
            provider_default_base("SILICONFLOW", "SILICONFLOW"),
            Some("https://api.siliconflow.cn/v1")
        );
        assert_eq!(
            provider_default_base("Tongyi-Qianwen", "Tongyi-Qianwen"),
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1")
        );
        // Local engines default to loopback.
        assert_eq!(
            provider_default_base("Ollama", "Ollama"),
            Some("http://127.0.0.1:11434")
        );
        assert_eq!(
            provider_default_base("OpenAI-API-Compatible", "OpenAI-API-Compatible"),
            Some("http://127.0.0.1:8088/v1")
        );
    }

    #[test]
    fn capability_routing() {
        assert!(provider_has_kind(
            "ZHIPU-AI",
            "ZHIPU-AI",
            ProviderKind::Rerank
        ));
        assert!(provider_has_kind(
            "SILICONFLOW",
            "SILICONFLOW",
            ProviderKind::Embedding
        ));
        assert!(!provider_has_kind(
            "DeepSeek",
            "DeepSeek",
            ProviderKind::Rerank
        ));
        assert!(provider_has_kind(
            "Xinference",
            "Xinference",
            ProviderKind::SpeechToText
        ));
        assert!(provider_has_kind(
            "PaddleOCR",
            "PaddleOCR",
            ProviderKind::Ocr
        ));
        assert!(!provider_has_kind("OpenAI", "OpenAI", ProviderKind::Ocr));
    }

    #[test]
    fn key_env_conventions() {
        assert_eq!(provider_api_key_env("ZHIPU-AI", ""), Some("ZHIPU_API_KEY"));
        assert_eq!(
            provider_api_key_env("DeepSeek", ""),
            Some("DEEPSEEK_API_KEY")
        );
        assert_eq!(provider_api_key_env("VolcEngine", ""), Some("ARK_API_KEY"));
        assert_eq!(provider_api_key_env("Ollama", ""), Some("OLLAMA_API_KEY"));
    }

    #[test]
    fn domestic_providers_are_sorted_first() {
        let chats = presets_for(ProviderKind::Chat);
        let first_non_domestic = chats
            .iter()
            .position(|preset| !preset.domestic)
            .unwrap_or(chats.len());
        assert!(
            chats[..first_non_domestic]
                .iter()
                .all(|preset| preset.domestic),
            "all domestic presets must come before international ones"
        );
    }

    #[test]
    fn no_duplicate_normalized_ids() {
        let mut seen = std::collections::HashSet::new();
        for preset in PROVIDER_PRESETS {
            let key = normalize_provider_name(preset.id);
            assert!(
                seen.insert(key),
                "duplicate normalized provider id: {}",
                preset.id
            );
        }
    }

    #[test]
    fn full_catalog_covers_all_presets() {
        // Every factory preset that exists upstream should be present in the
        // embedded full catalog. Factories that ship an empty model list in
        // the upstream JSON rely on dynamic model discovery (e.g. VolcEngine
        // /api/v3/models, Ollama /api/tags) — that is legitimate. Only
        // completely unknown factories (or local engines without an upstream
        // entry) may be absent.
        for preset in PROVIDER_PRESETS {
            let models = factory_full_models(preset.id, preset.name);
            if !models.is_empty() {
                continue;
            }
            let known = factory_known(preset.id, preset.name);
            assert!(
                known
                    || preset.id == "OpenAI-API-Compatible"
                    || preset.id == "SoMark"
                    || preset.id == "Builtin"
                    // CN providers shipped as conf/models fixtures but absent
                    // from the upstream llm_factories factory list (2026-08-06).
                    || preset.id == "Qiniu"
                    || preset.id == "HuaweiCloud"
                    || preset.id == "TokenHub"
                    || preset.id == "OrcaRouter",
                "factory {} ({}) missing from embedded catalog",
                preset.id,
                preset.name
            );
        }
    }

    #[test]
    fn full_catalog_model_types_are_valid() {
        let models = factory_full_models("SILICONFLOW", "SILICONFLOW");
        assert!(!models.is_empty());
        for m in &models {
            assert!(
                matches!(
                    m.t,
                    "chat"
                        | "embedding"
                        | "rerank"
                        | "reranker"
                        | "image2text"
                        | "speech2text"
                        | "tts"
                        | "moderation"
                ),
                "unexpected model type {} for {}",
                m.t,
                m.n
            );
        }
    }

    #[test]
    fn full_catalog_matches_upstream_counts() {
        // DeepSeek upstream ships deepseek-chat + deepseek-reasoner (v0.26.x);
        let ds = factory_full_models("DeepSeek", "DeepSeek");
        assert!(!ds.is_empty(), "DeepSeek catalog must not be empty");
        assert!(
            ds.iter().any(|m| m.n == "deepseek-chat"
                || m.n == "deepseek-v4-chat"
                || m.n.contains("deepseek-v4")),
            "DeepSeek catalog missing expected models: {:?}",
            ds.iter().map(|m| m.n).collect::<Vec<_>>()
        );
        // Embedding models exist for at least one domestic provider.
        let emb: Vec<_> = factory_full_models("Tongyi-Qianwen", "Tongyi-Qianwen")
            .into_iter()
            .filter(|m| m.t == "embedding")
            .collect();
        assert!(!emb.is_empty(), "Tongyi embedding models missing");
        // Rerank models exist for at least one provider.
        let rr: Vec<_> = factory_full_models("SILICONFLOW", "SILICONFLOW")
            .into_iter()
            .filter(|m| m.t == "rerank" || m.t == "reranker")
            .collect();
        assert!(!rr.is_empty(), "SILICONFLOW rerank models missing");
    }

    /// `un-add-model.tsx::mapModelKey` — every alias maps to the upstream pill
    /// label, surrounding whitespace is trimmed, and unknown keys yield `None`
    /// so the caller can reproduce the `|| tag.trim()` fallback.
    #[test]
    fn capability_short_label_matches_upstream_map_model_key() {
        for (key, label) in [
            ("chat", "LLM"),
            ("embedding", "Embedding"),
            ("rerank", "Rerank"),
            ("tts", "TTS"),
            ("asr", "ASR"),
            ("speech2text", "ASR"),
            ("image2text", "VLM"),
            ("vision", "VLM"),
            ("ocr", "OCR"),
        ] {
            assert_eq!(capability_short_label_from_str(key), Some(label), "{key}");
        }
        assert_eq!(
            capability_short_label_from_str("  embedding  "),
            Some("Embedding")
        );
        assert_eq!(capability_short_label_from_str("audio"), None);
        assert_eq!(capability_short_label(ProviderKind::ImageToText), "VLM");
        assert_eq!(capability_short_label(ProviderKind::SpeechToText), "ASR");
        assert_eq!(capability_short_label(ProviderKind::Chat), "LLM");
    }

    /// `un-add-model.tsx::orderMap` + `sortModelTypes` — canonical order is
    /// chat, embedding, rerank, tts, asr/speech2text, image2text/vision, ocr,
    /// and an unranked capability sinks behind all known ones.
    #[test]
    fn sort_model_types_follows_upstream_order_map() {
        for (key, order) in [
            ("chat", 1),
            ("embedding", 2),
            ("rerank", 3),
            ("tts", 4),
            ("asr", 5),
            ("speech2text", 5),
            ("image2text", 6),
            ("vision", 6),
            ("ocr", 7),
        ] {
            assert_eq!(model_type_sort_order(key), order, "{key}");
        }
        assert_eq!(model_type_sort_order("audio"), 999);

        let mut tags = ["ocr", "vision", "chat", "audio", "rerank", "speech2text"];
        sort_model_types(&mut tags);
        assert_eq!(
            tags,
            ["chat", "rerank", "speech2text", "vision", "ocr", "audio"]
        );
    }
}
