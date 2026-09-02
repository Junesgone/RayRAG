//! Parsing pipeline — orchestration of parse → chunk → embed → store.
//!
//! Ported from RAGFlow's `rag/app/naive.py` `chunk()` function.
//! This is the main entry point for document processing.

use crate::chunk::{self, ChunkStrategy};
use crate::embed::Embedder;
use crate::parser;
use crate::store::ZvecStore;
use crate::{Chunk, ParserConfig, Result};

/// `rag/flow` component data contracts — ProcessBase output map, the
/// chunker/extractor/tokenizer upstream payload schema (+ `_check_payloads`),
/// the pipeline progress trace, and the REST chunk contract (`ChunkDoc`).
pub mod flow;

struct SharedEmbedderAdapter(crate::embed::SharedEmbedder);

#[async_trait::async_trait]
impl Embedder for SharedEmbedderAdapter {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.0.embed(texts).await
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
    mineru: Option<crate::parser::mineru::MinerUClient>,
    somark: Option<crate::parser::somark::SoMarkClient>,
    stt: Option<crate::audio::AsrClient>,
    vision: Option<crate::vision::VisionClient>,
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
            mineru: None,
            somark: None,
            stt: None,
            vision: None,
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
        self
    }

    /// Resolve all optional HTTP-backed document providers from environment.
    pub fn with_document_parsers_from_env(self) -> Result<Self> {
        self.with_ocr_from_env()?
            .with_opendataloader_from_env()?
            .with_mineru_from_env()?
            .with_somark_from_env()?
            .with_asr_from_env()
    }

    /// Run the full pipeline on a file: parse → chunk → embed → store.
    pub async fn process(&self, file_path: &str) -> Result<Vec<Chunk>> {
        let name = std::path::Path::new(file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(file_path);

        // 1. Detect MIME type and parse
        let mime = parser::mime_from_extension(name)
            .ok_or_else(|| anyhow::anyhow!("Unsupported file type: {}", name))?;

        let data = std::fs::read(file_path)?;
        let doc = self.parse_document(name, mime, &data).await?;
        tracing::info!("Parsed: {} ({} bytes)", doc.name, doc.size);

        // 2. Chunk
        let mut chunks = self.chunker.chunk(&doc, &self.config)?;
        tracing::info!("Chunked: {} chunks", chunks.len());

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
            embedder.embed_chunks(&mut chunks).await?;
            tracing::info!("Embedded: {} vectors", chunks.len());
        }

        // 4. Store (if store is configured)
        if let Some(ref store) = self.store {
            store.insert(&chunks)?;
            tracing::info!("Stored: {} chunks", chunks.len());
        }

        Ok(chunks)
    }

    async fn parse_document(&self, name: &str, mime: &str, data: &[u8]) -> Result<crate::Document> {
        // RAGFlow `app_parsers.py`: chunk_method (parser_id) selects the
        // domain parser (paper/book/laws/qa/table/tag/presentation/picture/
        // audio/email) — wired in v0.3.3aq, previously dead code.
        if let Some(parser) = crate::app_parsers::parser_for_method(&self.config.chunk_method) {
            match parser.parse(name, data) {
                Ok(document) if !document.content.trim().is_empty() => return Ok(document),
                Ok(_) => {
                    tracing::warn!(
                        %name,
                        method = %self.config.chunk_method,
                        "Domain parser returned empty content; falling back to MIME parser"
                    );
                }
                Err(error) => {
                    tracing::warn!(%error, %name, "Domain parser failed; falling back to MIME parser");
                }
            }
        }

        if is_somark_layout(&self.config.layout_recognize) && mime == "application/pdf" {
            let client = self.somark.as_ref().ok_or_else(|| {
                anyhow::anyhow!("SoMark layout requested but SOMARK_BASE_URL is not configured")
            })?;
            client.check_installation().await?;
            let options =
                crate::parser::somark::SoMarkRequestOptions::from_parser_config(&self.config);
            let output = client
                .parse_pdf(name, data, Some(&options))
                .await
                .map_err(|error| anyhow::anyhow!("SoMark parsing failed for {name}: {error}"))?;
            let mut document = parser::new_document(name, output.content, mime, data.len());
            document.metadata.insert(
                crate::chunk::RAGFLOW_POSITION_TAGS_METADATA.to_string(),
                "true".to_string(),
            );
            return Ok(document);
        }

        if is_mineru_layout(&self.config.layout_recognize) && mime == "application/pdf" {
            let client = self.mineru.as_ref().ok_or_else(|| {
                anyhow::anyhow!("MinerU layout requested but MINERU_APISERVER is not configured")
            })?;
            client.check_installation().await?;
            let options =
                crate::parser::mineru::MinerURequestOptions::from_parser_config(&self.config)?;
            let output = client
                .parse_pdf(name, data, &options)
                .await
                .map_err(|error| anyhow::anyhow!("MinerU parsing failed for {name}: {error}"))?;
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
                    "OpenDataLoader service is not accessible at its configured /health endpoint"
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

        if is_paddleocr_layout(&self.config.layout_recognize)
            && (crate::ocr::supports_ocr(mime) || mime == "application/pdf")
        {
            let ocr = self.ocr.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "PaddleOCR layout requested but RAYRAG_OCR_PROVIDER=paddleocr is not configured"
                )
            })?;
            let text = ocr
                .ocr_file(name, data)
                .await
                .map_err(|error| anyhow::anyhow!("PaddleOCR parsing failed for {name}: {error}"))?;
            if text.trim().is_empty() {
                anyhow::bail!(
                    "PaddleOCR returned no text for {name}; a local OCR fallback is not configured"
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
        // `picture.py vision_llm_chunk`：JPEG 优先、PNG 回退、小图跳过，用
        // `vision_llm_figure_describe_prompt.md` 提示词让多模态模型直接输出
        // Markdown）。视觉模型未配置时静默落到下方 FigureParser（无 OCR 时
        // 输出元数据占位，与上游 naive figure 行为一致）。
        if crate::ocr::supports_ocr(mime)
            && let Some(vision) = &self.vision
                && data.len() >= 100 {
                    let prompt = crate::parser::figure::figure_describe_prompt();
                    let image = crate::vision::VisionClient::normalize_image(data, mime);
                    match vision.describe_with_prompt(&image, &prompt).await {
                        Ok(description) if !description.content.trim().is_empty() => {
                            return Ok(parser::new_document(
                                name,
                                format!(
                                    "<!--IMAGE_START-->\n[DOCUMENT_LAYOUT: VisionLLM]\n{}\n<!--IMAGE_END-->\n",
                                    description.content.trim()
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
                        }
                        Err(error) => {
                            tracing::warn!(%error, %name, "Vision description failed; falling back to figure parser");
                        }
                    }
                }
        // 音频：配置 ASR 时转录（OpenAI 兼容 /audio/transcriptions），否则落到元数据占位
        if (mime.starts_with("audio/") || mime == "application/ogg") && self.stt.is_some()
            && let Some(stt) = &self.stt {
                match stt.transcribe(data, name).await {
                    Ok(transcription) => {
                        if !transcription.text.trim().is_empty() {
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
                    }
                    Err(error) => {
                        tracing::warn!(%error, "ASR transcription failed; falling back to metadata");
                    }
                }
            }
        let parser = parser::get_parser(mime)
            .ok_or_else(|| anyhow::anyhow!("No parser for MIME: {}", mime))?;
        parser.parse(name, data)
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        http::StatusCode,
        routing::{get, post},
    };
    use serde_json::json;

    #[test]
    fn paddleocr_layout_accepts_fixed_provider_instance_suffix() {
        assert!(is_paddleocr_layout("PaddleOCR"));
        assert!(is_paddleocr_layout(
            "PaddleOCR-VL@primary-instance@PaddleOCR"
        ));
        assert!(!is_paddleocr_layout("DeepDOC"));
        assert!(!is_paddleocr_layout("PaddleOCR-VL"));
    }

    #[test]
    fn opendataloader_layout_accepts_fixed_provider_instance_suffix() {
        assert!(is_opendataloader_layout("OpenDataLoader"));
        assert!(is_opendataloader_layout(
            "docling-fast@primary-instance@OpenDataLoader"
        ));
        assert!(!is_opendataloader_layout("DeepDOC"));
        assert!(!is_opendataloader_layout("docling-fast"));
    }

    #[test]
    fn normalize_first_chunk_metadata_keeps_outline_on_first_only() {
        fn chunk_with_outline() -> crate::Chunk {
            let mut c = crate::Chunk {
                id: "c".into(),
                content: "text".into(),
                content_type: "text".into(),
                doc_id: uuid::Uuid::new_v4(),
                position: 0,
                token_count: 1,
                embedding: None,
                metadata: std::collections::HashMap::new(),
            };
            c.metadata
                .insert("__outline__".into(), "[{\"title\":\"ch1\"}]".into());
            c.metadata.insert("file_name".into(), "a.pdf".into());
            c
        }

        let mut chunks = vec![
            chunk_with_outline(),
            chunk_with_outline(),
            chunk_with_outline(),
        ];
        Pipeline::normalize_first_chunk_metadata(&mut chunks);
        assert!(chunks[0].metadata.contains_key("__outline__"));
        assert!(!chunks[1].metadata.contains_key("__outline__"));
        assert!(!chunks[2].metadata.contains_key("__outline__"));
        // Non-transient metadata is preserved on every chunk.
        assert!(chunks[1].metadata.contains_key("file_name"));
        assert!(chunks[2].metadata.contains_key("file_name"));
    }

    #[test]
    fn normalize_first_chunk_metadata_handles_empty() {
        let mut chunks: Vec<crate::Chunk> = vec![];
        Pipeline::normalize_first_chunk_metadata(&mut chunks);
        assert!(chunks.is_empty());
    }

    #[test]
    fn mineru_layout_accepts_fixed_provider_instance_suffix() {
        assert!(is_mineru_layout("MinerU"));
        assert!(is_mineru_layout("pipeline@primary-instance@MinerU"));
        assert!(!is_mineru_layout("DeepDOC"));
        assert!(!is_mineru_layout("pipeline"));
    }

    #[test]
    fn somark_layout_accepts_fixed_provider_instance_suffix() {
        assert!(is_somark_layout("SoMark"));
        assert!(is_somark_layout("somark-model@primary-instance@SoMark"));
        assert!(!is_somark_layout("DeepDOC"));
        assert!(!is_somark_layout("somark-model"));
    }

    #[tokio::test]
    async fn configured_paddleocr_is_used_by_the_image_pipeline() {
        let app = Router::new().route(
            "/ocr",
            post(|| async {
                Json(json!({
                    "texts": ["invoice", "total"],
                    "full_text": "invoice\ntotal",
                    "count": 2
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let pipeline = Pipeline::new(ParserConfig {
            layout_recognize: "PaddleOCR".into(),
            ..ParserConfig::default()
        })
        .with_ocr_client(crate::ocr::OcrClient::new(&format!("http://{address}")));

        let document = pipeline
            .parse_document("invoice.png", "image/png", b"fake-png")
            .await
            .unwrap();
        server.abort();

        assert!(document.content.contains("[DOCUMENT_LAYOUT: PaddleOCR]"));
        assert!(document.content.contains("invoice\ntotal"));
    }

    #[tokio::test]
    async fn configured_opendataloader_is_used_by_the_pdf_pipeline() {
        let app = Router::new()
            .route("/health", get(|| async { StatusCode::OK }))
            .route(
                "/file_parse",
                post(|| async {
                    Json(json!({
                        "json_doc": {
                            "type": "document",
                            "children": [
                                {"type": "title", "content": "ODL title"},
                                {"type": "table", "html": "<table><tr><td>1</td></tr></table>"}
                            ]
                        },
                        "md_text": null
                    }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let config = crate::parser::opendataloader::OpenDataLoaderConfig::from_ragflow_key(
            &format!(r#"{{"opendataloader_apiserver":"http://{address}"}}"#),
            None,
        )
        .unwrap();
        let client = crate::parser::opendataloader::OpenDataLoaderClient::new(config).unwrap();
        let pipeline = Pipeline::new(ParserConfig {
            layout_recognize: "docling@primary@OpenDataLoader".into(),
            ..ParserConfig::default()
        })
        .with_opendataloader_client(client);

        let document = pipeline
            .parse_document("sample.pdf", "application/pdf", b"%PDF-1.4\nmock")
            .await
            .unwrap();
        server.abort();

        assert!(document.content.contains("ODL title"));
        assert!(document.content.contains("<!--TABLE_START-->"));
        assert!(document.content.contains("<table>"));
    }

    #[tokio::test]
    async fn configured_mineru_is_used_by_the_pdf_pipeline() {
        let app = Router::new()
            .route(
                "/openapi.json",
                axum::routing::head(|| async { StatusCode::OK }),
            )
            .route(
                "/file_parse",
                post(|| async {
                    (
                        StatusCode::ACCEPTED,
                        Json(json!({"data":{"task_id":"pipeline-task"}})),
                    )
                }),
            )
            .route(
                "/tasks/pipeline-task/result",
                get(|| async {
                    Json(json!({
                        "results": {
                            "doc": {"md_content":"# MinerU pipeline\n\nParsed body.\n"}
                        }
                    }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let config = crate::parser::mineru::MinerUConfig::from_ragflow_key(
            &format!(r#"{{"mineru_apiserver":"http://{address}"}}"#),
            None,
        )
        .unwrap();
        let client = crate::parser::mineru::MinerUClient::new(config).unwrap();
        let pipeline = Pipeline::new(ParserConfig {
            layout_recognize: "pipeline@primary@MinerU".into(),
            ..ParserConfig::default()
        })
        .with_mineru_client(client);

        let document = pipeline
            .parse_document("sample.pdf", "application/pdf", b"%PDF-1.4\nmock")
            .await
            .unwrap();
        server.abort();

        assert!(document.content.contains("# MinerU pipeline"));
        assert!(document.content.contains("Parsed body."));
    }

    #[tokio::test]
    async fn configured_somark_is_used_by_the_pdf_pipeline() {
        let app = Router::new()
            .route("/", axum::routing::head(|| async { StatusCode::NOT_FOUND }))
            .route(
                "/parse/async",
                post(|| async { Json(json!({"code":0,"data":{"task_id":"pipeline-task"}})) }),
            )
            .route(
                "/parse/async_check",
                post(|| async {
                    Json(json!({
                        "code":0,
                        "data":{
                            "status":"SUCCESS",
                            "result":{
                                "outputs":{
                                    "json":{
                                        "pages":[{
                                            "page_num":0,
                                            "page_size":{"w":600,"h":800},
                                            "blocks":[
                                                {
                                                    "type":"title",
                                                    "content":"SoMark pipeline",
                                                    "title_level":1,
                                                    "bbox":[1,2,3,4]
                                                },
                                                {
                                                    "type":"table",
                                                    "content":"<table><tr><td>ok</td></tr></table>",
                                                    "bbox":[1,2,3,4]
                                                }
                                            ]
                                        }]
                                    }
                                }
                            }
                        }
                    }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let config = crate::parser::somark::SoMarkConfig::from_ragflow_key(
            &format!(
                r#"{{
                    "somark_base_url":"http://{address}",
                    "somark_poll_interval_base_ms":1,
                    "somark_poll_interval_max_ms":1
                }}"#
            ),
            None,
        )
        .unwrap();
        let client = crate::parser::somark::SoMarkClient::new(config).unwrap();
        let pipeline = Pipeline::new(ParserConfig {
            layout_recognize: "somark-model@primary@SoMark".into(),
            ..ParserConfig::default()
        })
        .with_somark_client(client);

        let document = pipeline
            .parse_document("sample.pdf", "application/pdf", b"%PDF-1.4\nmock")
            .await
            .unwrap();
        server.abort();

        assert!(document.content.contains("# SoMark pipeline"));
        assert!(document.content.contains("<!--TABLE_START-->"));
        assert!(document.content.contains("<table>"));
        assert!(document.content.contains("@@1\t1.0\t3.0\t2.0\t4.0##"));

        let mut chunks = pipeline.chunker.chunk(&document, &pipeline.config).unwrap();
        Pipeline::annotate_chunks(&mut chunks, &pipeline.config);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content_type, "table");
        assert!(!chunks[0].content.contains("@@"));
        assert_eq!(
            chunks[0].metadata.get("position_int").map(String::as_str),
            Some("[[1,1,3,2,4],[1,1,3,2,4]]")
        );
        assert!(
            !chunks[0]
                .metadata
                .contains_key(crate::chunk::RAGFLOW_POSITION_TAGS_METADATA)
        );
    }

    #[tokio::test]
    async fn vision_client_describes_raster_images_when_configured() {
        use axum::routing::post;
        let app = Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                Json(json!({
                    "choices": [{
                        "message": {
                            "content": "溶解氧低于3毫克每升时鱼类会浮头。"
                        }
                    }]
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let vision = crate::vision::VisionClient::new(crate::vision::VisionConfig {
            api_base: format!("http://{address}/v1"),
            api_key: String::new(),
            model: "mock-vision".into(),
            lang: "Chinese".into(),
        });
        let pipeline = Pipeline::new(ParserConfig::default()).with_vision_client(vision);
        // 1x1 PNG (68 bytes) padded to 128 with trailing zeros — the vision
        // branch only checks `data.len() >= 100`; the mock server ignores bytes.
        use base64::Engine;
        let mut png = base64::engine::general_purpose::STANDARD
            .decode(
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
            )
            .unwrap();
        png.resize(128, 0);
        let document = pipeline
            .parse_document("sample.png", "image/png", &png)
            .await
            .unwrap();
        server.abort();
        assert!(document.content.contains("[DOCUMENT_LAYOUT: VisionLLM]"));
        assert!(document.content.contains("溶解氧低于3毫克"));
    }
}
