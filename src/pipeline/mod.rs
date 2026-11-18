//! Parsing pipeline — orchestration of parse → chunk → embed → store.
//!
//! Ported from RAGFlow's `rag/app/naive.py` `chunk()` function.
//! This is the main entry point for document processing.

use crate::chunk::{self, ChunkStrategy};
use crate::embed::Embedder;
use crate::parser;
use crate::parser::Parse;
use crate::store::ZvecStore;
use crate::{Chunk, ParserConfig, Result};

/// `rag/flow` component data contracts — ProcessBase output map, the
/// chunker/extractor/tokenizer upstream payload schema (+ `_check_payloads`),
/// the pipeline progress trace, and the REST chunk contract (`ChunkDoc`).
pub mod flow;

/// v0.27.2 Go backend built-in ingestion pipeline registry.
pub mod builtin;

struct SharedEmbedderAdapter(crate::embed::SharedEmbedder);

#[async_trait::async_trait]
impl Embedder for SharedEmbedderAdapter {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.0.embed(texts).await
    }
}

/// One executed stage of a parse run, reported for the ingestion log
/// (`api/apps/services/dataset_api_service.py::get_ingestion_log` serves the
/// `PipelineOperationLog` row whose `dsl` carries one component per executed
/// step, and `pages/dataflow-result` rebuilds its timeline from that DSL).
///
/// RayRAG's parsing is a fixed sequence rather than a canvas, so the timeline is
/// synthesized from the stages this run actually executed together with their
/// measured elapsed time and the outputs they produced.
#[derive(Debug, Clone, PartialEq)]
pub struct StageReport {
    /// Component key inside the synthesized DSL (`parser`, `tokenChunker`, …).
    pub id: &'static str,
    /// RAGFlow component name (`Parser`, `TokenChunker`, …).
    pub component: &'static str,
    /// Timeline title (`graph.nodes[].data.name` upstream).
    pub title: &'static str,
    /// Wall-clock time of the stage in seconds.
    pub elapsed_seconds: f64,
    /// `obj.params.outputs`-shaped payload for the dataflow-result right panel.
    pub outputs: serde_json::Value,
}

/// Result of one parse run: the chunks plus the executed stages.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineReport {
    pub stages: Vec<StageReport>,
}

impl PipelineReport {
    /// The `_elapsed_time` of a stage, defaulting to zero like upstream's
    /// `node.params?.outputs?._elapsed_time?.value || 0`.
    pub fn stage(&self, id: &str) -> Option<&StageReport> {
        self.stages.iter().find(|stage| stage.id == id)
    }

    /// Total wall-clock time of every reported stage.
    pub fn total_seconds(&self) -> f64 {
        self.stages.iter().map(|stage| stage.elapsed_seconds).sum()
    }
}

/// The full RayRAG parsing pipeline.
pub struct Pipeline {
    config: ParserConfig,
    chunker: Box<dyn ChunkStrategy>,
    embedder: Option<Box<dyn Embedder>>,
    store: Option<ZvecStore>,
    ocr: Option<crate::ocr::OcrClient>,
    opendataloader: Option<crate::parser::opendataloader::OpenDataLoaderClient>,
    docling: Option<crate::parser::docling::DoclingClient>,
    oxr: Option<crate::parser::oxr::OxrClient>,
    mineru: Option<crate::parser::mineru::MinerUClient>,
    somark: Option<crate::parser::somark::SoMarkClient>,
    monkeyocrv2: Option<crate::parser::monkeyocrv2::MonkeyOcrV2Client>,
    mistral_ocr: Option<crate::parser::mistral_ocr::MistralOcrClient>,
    stt: Option<crate::audio::AsrClient>,
    vision: Option<crate::vision::VisionClient>,
    vision_config_error: Option<String>,
}

impl Pipeline {
    /// Create a new pipeline, selecting the chunking strategy from
    /// `config.chunk_method` (naive | token | title).
    pub fn new(config: ParserConfig) -> Self {
        Self {
            chunker: chunk::chunker_for(&config),
            config,
            embedder: None,
            store: None,
            ocr: None,
            opendataloader: None,
            docling: None,
            oxr: None,
            mineru: None,
            somark: None,
            monkeyocrv2: None,
            mistral_ocr: None,
            stt: None,
            vision: None,
            vision_config_error: None,
        }
    }

    /// Set a custom chunking strategy.
    pub fn with_chunker(mut self, chunker: Box<dyn ChunkStrategy>) -> Self {
        self.chunker = chunker;
        self
    }

    /// Set an embedder for generating embeddings.
    pub fn with_embedder(mut self, embedder: Box<dyn Embedder>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    /// Set a shared runtime embedder without introducing a second model instance.
    pub fn with_shared_embedder(mut self, embedder: crate::embed::SharedEmbedder) -> Self {
        self.embedder = Some(Box::new(SharedEmbedderAdapter(embedder)));
        self
    }

    /// Set a zvec store for persisting chunks.
    pub fn with_store(mut self, store: ZvecStore) -> Self {
        self.store = Some(store);
        self
    }

    /// Set an OCR provider used when `layout_recognize` selects PaddleOCR.
    pub fn with_ocr_client(mut self, ocr: crate::ocr::OcrClient) -> Self {
        self.ocr = Some(ocr);
        self
    }

    /// Resolve the optional Linux/Docker OCR provider from environment.
    pub fn with_ocr_from_env(mut self) -> Result<Self> {
        self.ocr = crate::ocr::OcrClient::from_env()?;
        Ok(self)
    }

    /// Set the remote PDF provider used by an OpenDataLoader layout selector.
    pub fn with_opendataloader_client(
        mut self,
        client: crate::parser::opendataloader::OpenDataLoaderClient,
    ) -> Self {
        self.opendataloader = Some(client);
        self
    }

    /// Resolve the optional OpenDataLoader PDF provider from environment.
    pub fn with_opendataloader_from_env(mut self) -> Result<Self> {
        self.opendataloader = crate::parser::opendataloader::OpenDataLoaderClient::from_env()?;
        Ok(self)
    }

    /// Set the remote Docling provider used by a Docling layout selector.
    pub fn with_docling_client(mut self, client: crate::parser::docling::DoclingClient) -> Self {
        self.docling = Some(client);
        self
    }

    /// Resolve the optional Docling service from environment.
    pub fn with_docling_from_env(mut self) -> Result<Self> {
        self.docling = crate::parser::docling::DoclingClient::from_env()?;
        Ok(self)
    }

    /// Set the remote OXR provider used by an OXR layout selector.
    pub fn with_oxr_client(mut self, client: crate::parser::oxr::OxrClient) -> Self {
        self.oxr = Some(client);
        self
    }

    /// Resolve the optional OXR service from environment.
    pub fn with_oxr_from_env(mut self) -> Result<Self> {
        self.oxr = crate::parser::oxr::OxrClient::from_env()?;
        Ok(self)
    }

    /// Set the remote PDF provider used by a MinerU layout selector.
    pub fn with_mineru_client(mut self, client: crate::parser::mineru::MinerUClient) -> Self {
        self.mineru = Some(client);
        self
    }

    /// Resolve the optional MinerU PDF provider from environment.
    pub fn with_mineru_from_env(mut self) -> Result<Self> {
        self.mineru = crate::parser::mineru::MinerUClient::from_env()?;
        Ok(self)
    }

    /// Set the remote PDF provider used by a SoMark layout selector.
    pub fn with_somark_client(mut self, client: crate::parser::somark::SoMarkClient) -> Self {
        self.somark = Some(client);
        self
    }

    /// Resolve the optional SoMark PDF provider from environment.
    pub fn with_somark_from_env(mut self) -> Result<Self> {
        self.somark = crate::parser::somark::SoMarkClient::from_env()?;
        Ok(self)
    }

    /// Set the remote PDF provider used by a MonkeyOCRv2 layout selector.
    pub fn with_monkeyocrv2_client(
        mut self,
        client: crate::parser::monkeyocrv2::MonkeyOcrV2Client,
    ) -> Self {
        self.monkeyocrv2 = Some(client);
        self
    }

    /// Resolve the optional MonkeyOCRv2 PDF provider from environment.
    pub fn with_monkeyocrv2_from_env(mut self) -> Result<Self> {
        self.monkeyocrv2 = crate::parser::monkeyocrv2::MonkeyOcrV2Client::from_env()?;
        Ok(self)
    }

    /// Set the remote PDF provider used by a Mistral OCR layout selector.
    pub fn with_mistral_ocr_client(
        mut self,
        client: crate::parser::mistral_ocr::MistralOcrClient,
    ) -> Self {
        self.mistral_ocr = Some(client);
        self
    }

    /// Resolve the optional Mistral OCR PDF provider from environment.
    pub fn with_mistral_ocr_from_env(mut self) -> Result<Self> {
        self.mistral_ocr = crate::parser::mistral_ocr::MistralOcrClient::from_env()?;
        Ok(self)
    }

    /// Resolve the optional ASR (speech-to-text) client from environment.
    pub fn with_asr_from_env(mut self) -> Result<Self> {
        self.stt = crate::audio::asr_from_env()?;
        Ok(self)
    }

    /// Attach an OpenAI-compatible vision (image-to-text) client for raster
    /// image parsing — mirrors RAGFlow `picture.py` / `naive.py` resolving the
    /// tenant default IMAGE2TEXT model and handing it to `VisionFigureParser`.
    pub fn with_vision_client(mut self, vision: crate::vision::VisionClient) -> Self {
        self.vision = Some(vision);
        self.vision_config_error = None;
        self
    }

    pub(crate) fn with_vision_config_error(mut self, error: String) -> Self {
        self.vision_config_error = Some(error);
        self
    }

    /// Resolve all optional HTTP-backed document providers from environment.
    pub fn with_document_parsers_from_env(self) -> Result<Self> {
        self.with_ocr_from_env()?
            .with_opendataloader_from_env()?
            .with_docling_from_env()?
            .with_oxr_from_env()?
            .with_mineru_from_env()?
            .with_somark_from_env()?
            .with_monkeyocrv2_from_env()?
            .with_mistral_ocr_from_env()?
            .with_asr_from_env()
    }

    /// Run the full pipeline on a file: parse → chunk → embed → store.
    pub async fn process(&self, file_path: &str) -> Result<Vec<Chunk>> {
        Ok(self.process_with_report(file_path).await?.0)
    }

    /// `process` plus the per-stage report the ingestion log stores.
    pub async fn process_with_report(
        &self,
        file_path: &str,
    ) -> Result<(Vec<Chunk>, PipelineReport)> {
        let mut stages: Vec<StageReport> = Vec::new();
        let name = std::path::Path::new(file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(file_path);

        // 1. Detect MIME type and parse
        let mime = parser::mime_from_extension(name)
            .ok_or_else(|| anyhow::anyhow!("Unsupported file type: {name}. Convert legacy Word .doc to .docx or Excel .xls to .xlsx; upload a supported document format. 旧版 Word/Excel 请先另存为 DOCX/XLSX。"))?;

        let data = std::fs::read(file_path)?;
        let parse_started = std::time::Instant::now();
        let mut doc = self.parse_document(name, mime, &data).await.map_err(|error| {
            match mime {
                "application/pdf" => anyhow::anyhow!(
                    "Cannot parse {name}: {error}. Open the PDF to check for damage or password protection, export an unencrypted PDF and upload it again. If an OCR layout is selected, check its client URL, credentials and model configuration. PDF 解析失败，请检查文件是否损坏或加密，重新导出后上传；使用 OCR 时请检查解析客户端地址、密钥和模型配置。"
                ),
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                | "application/vnd.openxmlformats-officedocument.presentationml.presentation" => anyhow::anyhow!(
                    "Cannot parse {name}: {error}. Open the document in Office or LibreOffice, repair it if necessary, save an unencrypted DOCX/XLSX/PPTX file and upload it again. Office 文档解析失败，请修复文件并另存为未加密的 DOCX/XLSX/PPTX 后重新上传。"
                ),
                _ => error,
            }
        })?;
        if doc.content.trim().is_empty() {
            anyhow::bail!(
                "No text extracted from {name}. For a scanned PDF, select and configure an OCR document layout backend (for example PaddleOCR) with a reachable API URL and valid credentials; the built-in PDF parser reads the text layer only. For other documents check whether the file is empty, damaged or encrypted. 未提取到内容：扫描 PDF 请配置 OCR 解析客户端，其他文档请检查是否为空、损坏或加密。"
            );
        }
        // Domain parsers organize extracted text, never compressed PDF/Office bytes.
        let method = self.config.chunk_method.to_ascii_lowercase();
        if matches!(
            method.as_str(),
            "paper" | "book" | "laws" | "table" | "qa" | "tag" | "presentation"
        ) && !crate::ocr::supports_ocr(mime)
            && !mime.starts_with("audio/")
            && let Some(domain_parser) = crate::app_parsers::parser_for_method(&method)
        {
            let organized = domain_parser
                .parse(name, doc.content.as_bytes())
                .map_err(|error| {
                    anyhow::anyhow!("Domain parser {method} failed for {name}: {error}")
                })?;
            doc.content = organized.content;
        }
        tracing::info!("Parsed: {} ({} bytes)", doc.name, doc.size);
        stages.push(StageReport {
            id: "parser",
            component: "Parser",
            title: "Parser",
            elapsed_seconds: parse_started.elapsed().as_secs_f64(),
            outputs: serde_json::json!({
                "output_format": {"type": "string", "value": "text"},
                "text": {"type": "string", "value": doc.content},
                "_elapsed_time": {
                    "type": "number",
                    "value": parse_started.elapsed().as_secs_f64()
                }
            }),
        });

        // 2. Chunk
        let chunk_started = std::time::Instant::now();
        let mut chunks = self.chunker.chunk(&doc, &self.config)?;
        tracing::info!("Chunked: {} chunks", chunks.len());
        let chunk_outputs = serde_json::json!({
            "output_format": {"type": "string", "value": "chunks"},
            "chunks": {
                "type": "array",
                "value": chunks
                    .iter()
                    .map(|chunk| serde_json::json!({
                        "content": chunk.content,
                        "docnm_kwd": chunk.metadata.get("docnm_kwd").cloned().unwrap_or_default(),
                        "important_kwd": chunk
                            .metadata
                            .get("important_kwd")
                            .cloned()
                            .unwrap_or_default(),
                    }))
                    .collect::<Vec<_>>()
            },
            "chunk_count": {"type": "number", "value": chunks.len()},
            "token_count": {
                "type": "number",
                "value": chunks.iter().map(|chunk| chunk.token_count).sum::<usize>()
            },
            "_elapsed_time": {
                "type": "number",
                "value": chunk_started.elapsed().as_secs_f64()
            }
        });
        stages.push(StageReport {
            id: "tokenChunker",
            component: "TokenChunker",
            title: "Token chunker",
            elapsed_seconds: chunk_started.elapsed().as_secs_f64(),
            outputs: chunk_outputs,
        });

        // 2.5 Annotate content type + add context for tables/images
        Self::annotate_chunks(&mut chunks, &self.config);

        // 2.6 Normalize transient document metadata onto the first chunk
        // only. Mirrors RAGFlow naive.py: the PDF outline travels as a
        // transient `__outline__` on chunks[0] and is popped by
        // chunk_builder.extract_outline before persistence.
        Self::normalize_first_chunk_metadata(&mut chunks);

        // 2.7 Optional key-term extraction (RAGFlow auto_keywords): runs
        // after chunking, before embedding, and stores per-chunk keywords in
        // the `important_kwd` metadata consumed by hybrid retrieval.
        if self.config.auto_keywords > 0 {
            crate::extractor::Extractor::with_top_n(self.config.auto_keywords)
                .extract_chunks(&mut chunks);
            tracing::info!("Keywords: {} chunks annotated", chunks.len());
        }

        // 3. Embed (if embedder is configured)
        if let Some(ref embedder) = self.embedder {
            let embed_started = std::time::Instant::now();
            embedder.embed_chunks(&mut chunks).await?;
            tracing::info!("Embedded: {} vectors", chunks.len());
            stages.push(StageReport {
                id: "tokenizer",
                component: "Tokenizer",
                title: "Tokenizer",
                elapsed_seconds: embed_started.elapsed().as_secs_f64(),
                outputs: serde_json::json!({
                    "output_format": {"type": "string", "value": "json"},
                    "json": {
                        "type": "object",
                        "value": {
                            "vector_count": chunks.len(),
                            "dimension": chunks
                                .iter()
                                .find_map(|chunk| chunk.embedding.as_ref().map(Vec::len))
                                .unwrap_or_default(),
                        }
                    },
                    "_elapsed_time": {
                        "type": "number",
                        "value": embed_started.elapsed().as_secs_f64()
                    }
                }),
            });
        }

        // 4. Store (if store is configured)
        if let Some(ref store) = self.store {
            store.insert(&chunks)?;
            tracing::info!("Stored: {} chunks", chunks.len());
        }

        Ok((chunks, PipelineReport { stages }))
    }

    async fn parse_document(&self, name: &str, mime: &str, data: &[u8]) -> Result<crate::Document> {
        // Email has its own wire format; other domain transforms run after extraction.
        if mime == "message/rfc822" {
            return crate::app_parsers::EmailParser::new().parse(name, data);
        }

        if is_somark_layout(&self.config.layout_recognize) && mime == "application/pdf" {
            let guidance = "Check the SoMark provider's somark_base_url and somark_api_key or SOMARK_BASE_URL and SOMARK_API_KEY (if authentication is required), verify the parsing service and its models are available, then reparse. 请检查 SoMark 客户端地址、认证和模型服务，修复后重新解析。";
            let client = self.somark.as_ref().ok_or_else(|| {
                anyhow::anyhow!("SoMark layout requested but no client is configured. {guidance}")
            })?;
            client.check_installation().await.map_err(|error| {
                anyhow::anyhow!("SoMark availability check failed for {name}: {error}. {guidance}")
            })?;
            let options =
                crate::parser::somark::SoMarkRequestOptions::from_parser_config(&self.config);
            let output = client
                .parse_pdf(name, data, Some(&options))
                .await
                .map_err(|error| {
                    anyhow::anyhow!("SoMark parsing failed for {name}: {error}. {guidance}")
                })?;
            let mut document = parser::new_document(name, output.content, mime, data.len());
            document.metadata.insert(
                crate::chunk::RAGFLOW_POSITION_TAGS_METADATA.to_string(),
                "true".to_string(),
            );
            return Ok(document);
        }

        if is_mistral_ocr_layout(&self.config.layout_recognize) && mime == "application/pdf" {
            let guidance = "Check the Mistral OCR provider's API key, mistral_ocr_base_url and mistral_ocr_model or MISTRAL_OCR_API_KEY, MISTRAL_OCR_BASE_URL and MISTRAL_OCR_MODEL, verify access to /models and /ocr and model availability, then reparse. 请检查 Mistral OCR 客户端地址、密钥和 OCR 模型权限，修复后重新解析。";
            let client = self.mistral_ocr.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "Mistral OCR layout requested but no client is configured. {guidance}"
                )
            })?;
            client.check_installation().await.map_err(|error| {
                anyhow::anyhow!("Mistral OCR availability check failed for {name}: {error}. {guidance}")
            })?;
            let output = client
                .parse_pdf(
                    name,
                    data,
                    0,
                    crate::parser::mistral_ocr::MAXIMUM_PAGE_NUMBER,
                )
                .await
                .map_err(|error| {
                    anyhow::anyhow!("Mistral OCR parsing failed for {name}: {error}. {guidance}")
                })?;
            let mut document = parser::new_document(name, output.content, mime, data.len());
            document.metadata.insert(
                crate::chunk::RAGFLOW_POSITION_TAGS_METADATA.to_string(),
                "true".to_string(),
            );
            return Ok(document);
        }

        if is_monkeyocrv2_layout(&self.config.layout_recognize) && mime == "application/pdf" {
            let override_url = self
                .config
                .monkeyocrv2_server_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let client = match (override_url, self.monkeyocrv2.as_ref()) {
                (Some(url), _) => crate::parser::monkeyocrv2::MonkeyOcrV2Client::new(
                    crate::parser::monkeyocrv2::MonkeyOcrV2Config::new(url)?,
                )?,
                (None, Some(client)) => client.clone(),
                (None, None) => anyhow::bail!(
                    "MonkeyOCRv2 layout requested but MONKEYOCRV2_SERVER_URL is not configured"
                ),
            };
            if !client.check_installation().await? {
                anyhow::bail!(
                    "MonkeyOCRv2 health check failed. Check parser_config.monkeyocrv2_server_url or MONKEYOCRV2_SERVER_URL and verify the service's /health endpoint is reachable from RayRAG, then reparse. 请检查 MonkeyOCRv2 客户端地址和服务连接，修复后重新解析。"
                );
            }
            let output = client
                .parse_pdf(name, data, 0, 99999)
                .await
                .map_err(|error| {
                    anyhow::anyhow!("MonkeyOCRv2 parsing failed for {name}: {error}")
                })?;
            let mut document = parser::new_document(name, output.content, mime, data.len());
            document.metadata.insert(
                crate::chunk::RAGFLOW_POSITION_TAGS_METADATA.to_string(),
                "true".to_string(),
            );
            return Ok(document);
        }

        if is_mineru_layout(&self.config.layout_recognize) && mime == "application/pdf" {
            let guidance = "Check the MinerU provider's mineru_apiserver, mineru_api_key and mineru_backend or MINERU_APISERVER, MINERU_API_KEY (if authentication is required) and MINERU_BACKEND. Verify access to /openapi.json and /file_parse and that the selected backend's models are loaded; set MINERU_SERVER_URL for an HTTP model backend, then reparse. 请检查 MinerU 客户端地址、认证和解析后端，确认模型服务已加载，修复后重新解析。";
            let client = self.mineru.as_ref().ok_or_else(|| {
                anyhow::anyhow!("MinerU layout requested but no client is configured. {guidance}")
            })?;
            client.check_installation().await.map_err(|error| {
                anyhow::anyhow!("MinerU availability check failed for {name}: {error}. {guidance}")
            })?;
            let options = crate::parser::mineru::MinerURequestOptions::from_parser_config(&self.config)
                .map_err(|error| {
                    anyhow::anyhow!(
                        "MinerU parser settings are invalid for {name}: {error}. {guidance}"
                    )
                })?;
            let output = client
                .parse_pdf(name, data, &options)
                .await
                .map_err(|error| {
                    anyhow::anyhow!("MinerU parsing failed for {name}: {error}. {guidance}")
                })?;
            return Ok(parser::new_document(name, output.content, mime, data.len()));
        }

        if is_opendataloader_layout(&self.config.layout_recognize) && mime == "application/pdf" {
            let client = self.opendataloader.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "OpenDataLoader layout requested but OPENDATALOADER_APISERVER is not configured"
                )
            })?;
            if !client.check_installation().await {
                anyhow::bail!(
                    "OpenDataLoader health check failed. Check OPENDATALOADER_APISERVER, OPENDATALOADER_API_KEY (if authentication is required), and access to the service's /health endpoint from RayRAG, then reparse. 请检查 OpenDataLoader 客户端地址、密钥及服务连接，修复后重新解析。"
                );
            }
            let output = client
                .parse_pdf(
                    name,
                    data,
                    &crate::parser::opendataloader::OpenDataLoaderOptions {
                        hybrid: self.config.opendataloader_hybrid.clone(),
                        image_output: self.config.opendataloader_image_output.clone(),
                        sanitize: self.config.opendataloader_sanitize,
                    },
                )
                .await
                .map_err(|error| {
                    anyhow::anyhow!("OpenDataLoader parsing failed for {name}: {error}")
                })?;
            return Ok(parser::new_document(name, output.content, mime, data.len()));
        }

        if is_docling_layout(&self.config.layout_recognize) && mime == "application/pdf" {
            if let Some(client) = self.docling.as_ref() {
                if !client.check_installation().await {
                    anyhow::bail!(
                        "Docling availability check failed. Check DOCLING_SERVER_URL, DOCLING_API_KEY (if authentication is required), and access to /health, /openapi.json, /docs or /v1/convert/source from RayRAG, then reparse. 请检查 Docling 客户端地址、密钥及服务连接，修复后重新解析。"
                    );
                }
                let output = client.parse_pdf(name, data).await.map_err(|error| {
                    anyhow::anyhow!("Docling parsing failed for {name}: {error}")
                })?;
                return Ok(parser::new_document(name, output.content, mime, data.len()));
            }
            tracing::warn!(
                %name,
                "Docling layout requested without DOCLING_SERVER_URL; using the native fallback"
            );
            return crate::parser::docling::DoclingParser::new().parse(name, data);
        }

        if is_oxr_layout(&self.config.layout_recognize)
            && (mime == "application/pdf" || crate::ocr::supports_ocr(mime))
        {
            let guidance = "Configure OXR_BASE_URL and OXR_API_KEY (if required), verify the OXR parsing service is reachable and its model is loaded, then reparse. 请配置 OXR 客户端地址和密钥，检查解析服务连接与模型加载状态后重新解析。";
            let client = self.oxr.as_ref().ok_or_else(|| {
                anyhow::anyhow!("OXR layout requested but OXR_BASE_URL is not configured. {guidance}")
            })?;
            let output = client
                .parse_file(name, data)
                .await
                .map_err(|error| anyhow::anyhow!("OXR parsing failed for {name}: {error}. {guidance}"))?;
            let mut document = parser::new_document(name, output.content, mime, data.len());
            document.metadata.insert(
                crate::chunk::RAGFLOW_POSITION_TAGS_METADATA.to_string(),
                "true".to_string(),
            );
            return Ok(document);
        }

        if is_paddleocr_layout(&self.config.layout_recognize)
            && (crate::ocr::supports_ocr(mime) || mime == "application/pdf")
        {
            let guidance = "Configure RAYRAG_OCR_PROVIDER=paddleocr, PADDLEOCR_BASE_URL, PADDLEOCR_ACCESS_TOKEN (if required) and PADDLEOCR_ALGORITHM, verify the client connection and model availability, then reparse. 请配置 PaddleOCR 客户端地址、访问令牌和算法模型，检查服务连接后重新解析。";
            let ocr = self.ocr.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "PaddleOCR layout requested but RAYRAG_OCR_PROVIDER=paddleocr is not configured. {guidance}"
                )
            })?;
            let text = ocr
                .ocr_file(name, data)
                .await
                .map_err(|error| anyhow::anyhow!("PaddleOCR parsing failed for {name}: {error}. {guidance}"))?;
            if text.trim().is_empty() {
                anyhow::bail!(
                    "PaddleOCR returned no text for {name}. {guidance}"
                );
            }
            let content = if crate::ocr::supports_ocr(mime) {
                format!(
                    "<!--IMAGE_START-->\n[DOCUMENT_LAYOUT: PaddleOCR]\n{}\n<!--IMAGE_END-->\n",
                    text.trim()
                )
            } else {
                text
            };
            return Ok(parser::new_document(name, content, mime, data.len()));
        }

        // 图片 + 租户默认 image2text 模型：走视觉描述（对齐 RAGFlow
        // BMP/TIFF 转为 PNG；不按压缩字节数跳过有效小图。使用
        // `vision_llm_figure_describe_prompt.md` 提示词让多模态模型直接输出
        // Markdown）。视觉模型未配置时尝试 FigureParser 的 OCR；没有可用
        // 提取结果时必须报错，不能将元数据占位当成正文入库。
        let mut vision_failure = if crate::ocr::supports_ocr(mime) {
            self.vision_config_error.clone()
        } else {
            None
        };
        if crate::ocr::supports_ocr(mime)
            && let Some(vision) = &self.vision
        {
            let prompt = crate::parser::figure::figure_describe_prompt();
            match crate::parser::figure::describe_image_bytes(vision, data, mime, Some(&prompt))
                .await
            {
                Ok(description) if !description.trim().is_empty() => {
                    return Ok(parser::new_document(
                        name,
                        format!(
                            "<!--IMAGE_START-->\n[DOCUMENT_LAYOUT: VisionLLM]\n{}\n<!--IMAGE_END-->\n",
                            description.trim()
                        ),
                        mime,
                        data.len(),
                    ));
                }
                Ok(_) => {
                    tracing::warn!(
                        %name,
                        "Vision model returned empty description; falling back to figure parser"
                    );
                    vision_failure = Some("image-to-text model returned no content".to_string());
                }
                Err(error) => {
                    tracing::warn!(%error, %name, "Vision description failed; falling back to figure parser");
                    vision_failure = Some(format!("image-to-text request failed: {error}"));
                }
            }
        }
        if crate::ocr::supports_ocr(mime)
            && let Some(ocr) = &self.ocr
        {
            let text = ocr.ocr(data).await.map_err(|error| {
                let vision_detail = vision_failure.as_deref().map_or(String::new(), |failure| {
                    format!(" Vision model failure: {failure}.")
                });
                anyhow::anyhow!("Image {name}: OCR request failed: {error}.{vision_detail} Check RAYRAG_OCR_PROVIDER, RAYRAG_OCR_BASE_URL and provider credentials, or configure a working tenant image-to-text model. 图片解析失败，请检查 OCR 客户端地址、密钥和服务状态，或配置图像转文本模型。")
            })?;
            if text.trim().is_empty() {
                let vision_detail = vision_failure.as_deref().map_or(String::new(), |failure| {
                    format!(" Vision model failure: {failure}.")
                });
                anyhow::bail!(
                    "Image {name}: OCR returned no text.{vision_detail} Check the OCR client/model configuration or select a vision-capable image-to-text model. OCR 未返回文字，请检查客户端或图像转文本模型配置。"
                );
            }
            return Ok(parser::new_document(
                name,
                format!("<!--IMAGE_START-->\n{}\n<!--IMAGE_END-->\n", text.trim()),
                mime,
                data.len(),
            ));
        }
        // Audio must yield a transcription before it can enter the text index.
        if mime.starts_with("audio/") || mime == "application/ogg" {
            let guidance = "Configure RAYRAG_ASR_API_BASE, RAYRAG_ASR_MODEL and RAYRAG_ASR_API_KEY (if required), verify the client can reach /audio/transcriptions and that the model supports speech-to-text; then reparse. 请配置语音转文本客户端地址、模型和密钥，检查服务连接后重新解析。";
            let stt = self.stt.as_ref().ok_or_else(|| {
                anyhow::anyhow!("Audio {name}: no ASR client configured. {guidance}")
            })?;
            let transcription = stt.transcribe(data, name).await.map_err(|error| {
                anyhow::anyhow!("Audio {name}: ASR request failed: {error:#}. {guidance}")
            })?;
            if transcription.text.trim().is_empty() {
                anyhow::bail!(
                    "Audio {name}: ASR returned no text. Check whether the recording contains audible speech. {guidance}"
                );
            }
            return Ok(parser::new_document(
                name,
                format!(
                    "<!--AUDIO_START-->\n{}\n<!--AUDIO_END-->\n",
                    transcription.text.trim()
                ),
                mime,
                data.len(),
            ));
        }
        let parser = parser::get_parser(mime)
            .ok_or_else(|| anyhow::anyhow!("No parser for MIME: {}", mime))?;
        parser.parse(name, data).map_err(|error| {
            if let Some(failure) = vision_failure {
                anyhow::anyhow!("{failure}. Check the tenant image-to-text model's vision support, API base URL and API key; OCR fallback also failed: {error}")
            } else {
                error
            }
        })
    }

    /// Normalize transient document metadata onto the first chunk only.
    ///
    /// Mirrors RAGFlow `naive.py` + `chunk_builder.py`: the PDF outline is
    /// attached to the parsed document as transient `__outline__` and must
    /// travel on `chunks[0]` only (where `extract_outline` pops it before
    /// persistence). The naive chunker clones document metadata onto every
    /// chunk, so strip the transient key from all but the first.
    fn normalize_first_chunk_metadata(chunks: &mut [Chunk]) {
        let transient_keys = ["__outline__"];
        let mut first = true;
        for chunk in chunks.iter_mut() {
            if first {
                first = false;
                continue;
            }
            for key in transient_keys {
                chunk.metadata.remove(key);
            }
        }
    }

    /// Scan chunks for content-type markers and add surrounding context.
    fn annotate_chunks(chunks: &mut [Chunk], config: &ParserConfig) {
        for chunk in chunks.iter_mut() {
            if chunk.content.contains("<!--TABLE_START-->") {
                chunk.content_type = "table".into();
                // Clean up markers
                chunk.content = chunk
                    .content
                    .replace("<!--TABLE_START-->", "")
                    .replace("<!--TABLE_END-->", "");
            } else if chunk.content.contains("<!--IMAGE_START-->") {
                chunk.content_type = "image".into();
                chunk.content = chunk
                    .content
                    .replace("<!--IMAGE_START-->", "")
                    .replace("<!--IMAGE_END-->", "");
            } else {
                chunk.content_type = "text".into();
            }
        }

        // Pass 2: add surrounding context for table/image chunks
        let chunk_count = chunks.len();
        for i in 0..chunk_count {
            if chunks[i].content_type == "table" || chunks[i].content_type == "image" {
                let context_size = if chunks[i].content_type == "table" {
                    config.table_context_size
                } else {
                    config.image_context_size
                };

                let mut context = String::new();

                // Pre-context: text from previous chunks
                let pre_start = i.saturating_sub(3);
                for chunk in &chunks[pre_start..i] {
                    if chunk.content_type == "text" {
                        let snippet = &chunk.content;
                        let start = snippet.len().saturating_sub(context_size);
                        context.push_str(&snippet[start..]);
                        context.push(' ');
                    }
                }

                // Post-context: text from next chunks
                let before = if context.trim().is_empty() {
                    "(no preceding text)".to_string()
                } else {
                    context.clone()
                };

                let mut after = String::new();
                for chunk in &chunks[(i + 1)..(i + 4).min(chunk_count)] {
                    if chunk.content_type == "text" {
                        let snippet = &chunk.content;
                        let end = snippet.len().min(context_size);
                        after.push_str(&snippet[..end]);
                        after.push(' ');
                        break;
                    }
                }

                if after.trim().is_empty() {
                    after = "(no following text)".into();
                }

                chunks[i]
                    .metadata
                    .insert("context_before".into(), before.trim().to_string());
                chunks[i]
                    .metadata
                    .insert("context_after".into(), after.trim().to_string());
            }
        }
    }
}

fn is_docling_layout(layout_recognize: &str) -> bool {
    let normalized = layout_recognize.trim().to_ascii_lowercase();
    normalized == "docling" || normalized.ends_with("@docling")
}

fn is_oxr_layout(layout_recognize: &str) -> bool {
    let normalized = layout_recognize.trim().to_ascii_lowercase();
    normalized == "oxr" || normalized.ends_with("@oxr")
}

fn is_paddleocr_layout(layout_recognize: &str) -> bool {
    let normalized = layout_recognize.trim().to_ascii_lowercase();
    normalized == "paddleocr" || normalized.ends_with("@paddleocr")
}

fn is_opendataloader_layout(layout_recognize: &str) -> bool {
    let normalized = layout_recognize.trim().to_ascii_lowercase();
    normalized == "opendataloader" || normalized.ends_with("@opendataloader")
}

fn is_mineru_layout(layout_recognize: &str) -> bool {
    let normalized = layout_recognize.trim().to_ascii_lowercase();
    normalized == "mineru" || normalized.ends_with("@mineru")
}

fn is_somark_layout(layout_recognize: &str) -> bool {
    let normalized = layout_recognize.trim().to_ascii_lowercase();
    normalized == "somark" || normalized.ends_with("@somark")
}

fn is_monkeyocrv2_layout(layout_recognize: &str) -> bool {
    let normalized = layout_recognize.trim().to_ascii_lowercase();
    normalized == "monkeyocrv2" || normalized.ends_with("@monkeyocrv2")
}

fn is_mistral_ocr_layout(layout_recognize: &str) -> bool {
    let normalized = layout_recognize.trim().to_ascii_lowercase();
    normalized == "mistral ocr" || normalized.ends_with("@mistral ocr")
}




