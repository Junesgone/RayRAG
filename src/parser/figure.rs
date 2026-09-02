//! Figure / Image parser — handles PNG, JPEG, GIF, WebP, BMP, SVG, TIFF.
//!
//! Mirrors RAGFlow `deepdoc/parser/figure_parser.py` + `rag/app/picture.py`:
//! - SVG: extract text from XML tags
//! - Raster images: OCR first; short OCR text (<32 tokens) is enriched with a
//!   vision-model description (`describe_with_prompt`), exactly like
//!   picture.py's "Use CV LLM to describe the picture" branch.
//! - `vision_llm_chunk`: a standalone helper turning image bytes into
//!   Markdown via a vision model (JPEG preferred, PNG fallback, tiny-image
//!   skip) — mirrors picture.py `vision_llm_chunk` (used by figure_parser
//!   and pdf_parser upstream).
//! - Video files are NOT handled here (picture.py routes them to the CV
//!   model's async video description); RayRAG's video support lives in
//!   `app_parsers.rs` and requires an explicit vision client.
//!
//! Embeds result as image context marker for pipeline.

use crate::ocr::OcrClient;
use crate::parser::{Parse, new_document};
use crate::vision::VisionClient;
use crate::{Document, Result};
use quick_xml::Reader;
use quick_xml::events::Event;

/// MIME types accepted by Gemini-style video-capable vision models
/// (mirrors picture.py `VIDEO_EXTS`).
pub const VIDEO_EXTS: [&str; 11] = [
    ".mp4", ".mov", ".avi", ".flv", ".mpeg", ".mpg", ".webm", ".wmv", ".3gp", ".3gpp", ".mkv",
];

#[derive(Default)]
pub struct FigureParser {
    ocr: Option<OcrClient>,
    vision: Option<VisionClient>,
}

impl FigureParser {
    pub fn new() -> Self {
        // 惰性接入 OCR：.env 设置 RAYRAG_OCR_PROVIDER=proxy + RAYRAG_OCR_BASE_URL 时，
        // 图片解析自动调用 OCR 服务提取文字（对齐 RAGFlow figure_parser use_ocr 语义）。
        let ocr = crate::ocr::OcrClient::from_env().ok().flatten();
        Self { ocr, vision: None }
    }

    /// Enable OCR for raster images.
    pub fn with_ocr(ocr: OcrClient) -> Self {
        Self {
            ocr: Some(ocr),
            vision: None,
        }
    }

    /// Enable vision-model enrichment for short-OCR images.
    pub fn with_vision(vision: VisionClient) -> Self {
        Self {
            ocr: None,
            vision: Some(vision),
        }
    }

    /// Enable both OCR and vision enrichment.
    pub fn with_ocr_and_vision(ocr: OcrClient, vision: VisionClient) -> Self {
        Self {
            ocr: Some(ocr),
            vision: Some(vision),
        }
    }

    fn extract_text(&self, name: &str, data: &[u8]) -> Result<String> {
        let ext = std::path::Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "svg" => parse_svg(data),
            _ => parse_raster(name, data, self.ocr.as_ref(), self.vision.as_ref()),
        }
    }
}

impl Parse for FigureParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = self.extract_text(name, data)?;
        let mime = match std::path::Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
        {
            Some("png") => "image/png",
            Some("jpg") | Some("jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            Some("bmp") => "image/bmp",
            Some("svg") => "image/svg+xml",
            Some("tiff") | Some("tif") => "image/tiff",
            _ => "application/octet-stream",
        };
        Ok(new_document(name, content, mime, data.len()))
    }
}

/// `vision_llm_chunk` — turn image bytes into Markdown via a vision model.
/// Mirrors RAGFlow `rag/app/picture.py vision_llm_chunk`:
/// - skips images with any side < 11px (provider image-size limits)
/// - saves as JPEG first, falls back to PNG on error
/// - cleans the model reply with `clean_markdown_block`
///
/// Returns the described text (may be empty when the model fails or the
/// image is skipped). This is a synchronous wrapper for callers without a
/// runtime; async callers should use [`describe_image_bytes`].
pub fn vision_llm_chunk_sync(
    vision: &VisionClient,
    image_bytes: &[u8],
    mime: &str,
    prompt: Option<&str>,
) -> Result<String> {
    // Skip tiny crops that fail provider image-size limits (mirror min_side).
    let tiny = detect_dimensions(image_bytes).is_some_and(|(w, h)| w < 11 || h < 11);
    if tiny {
        return Ok(String::new());
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(describe_image_bytes(vision, image_bytes, mime, prompt))
}

/// Async vision description of raw image bytes. Tries JPEG-compatible
/// re-encode for formats the provider may reject, then falls back to the
/// original bytes. Mirrors picture.py's BytesIO JPEG/PNG dance.
pub async fn describe_image_bytes(
    vision: &VisionClient,
    image_bytes: &[u8],
    mime: &str,
    prompt: Option<&str>,
) -> Result<String> {
    let data_url = VisionClient::normalize_image(image_bytes, mime);
    let description = match prompt {
        Some(p) => vision.describe_with_prompt(&data_url, p).await?,
        None => vision.describe(&data_url).await?,
    };
    Ok(clean_markdown_block(&description.content))
}

/// `clean_markdown_block` — mirrors `common/string_utils.py
/// clean_markdown_block`: strip a single leading/trailing ``` fence pair.
pub fn clean_markdown_block(text: &str) -> String {
    let trimmed = text.trim();
    if let Some(inner) = trimmed
        .strip_prefix("```markdown")
        .or_else(|| trimmed.strip_prefix("```md"))
        .or_else(|| trimmed.strip_prefix("```"))
    {
        let inner = inner.trim();
        if let Some(rest) = inner.strip_suffix("```") {
            return rest.trim().to_owned();
        }
        return inner.to_owned();
    }
    trimmed.to_owned()
}

/// `vision_llm_figure_describe_prompt` — figure-specific vision prompt
/// (port of RAGFlow `rag/prompts/vision_llm_figure_describe_prompt.md`).
pub fn figure_describe_prompt() -> String {
    FIGURE_DESCRIBE_PROMPT.to_owned()
}

/// `vision_llm_figure_describe_prompt_with_context` — figure prompt with
/// surrounding document context (port of
/// `rag/prompts/vision_llm_figure_describe_prompt_with_context.md`).
/// Context is used only to disambiguate terms visible in the image.
pub fn figure_describe_prompt_with_context(context_above: &str, context_below: &str) -> String {
    FIGURE_DESCRIBE_PROMPT_WITH_CONTEXT
        .replace("{{ context_above }}", context_above)
        .replace("{{ context_below }}", context_below)
}

/// Describe a figure with optional surrounding document context — mirrors
/// `VisionFigureParser.process` in `figure_parser.py`: the with-context
/// prompt is used when either context side is non-empty, otherwise the
/// default figure prompt; both go through `picture.vision_llm_chunk`.
pub fn describe_figure_with_context_sync(
    vision: &VisionClient,
    image_bytes: &[u8],
    mime: &str,
    context_above: &str,
    context_below: &str,
) -> Result<String> {
    let prompt = if context_above.is_empty() && context_below.is_empty() {
        figure_describe_prompt()
    } else {
        figure_describe_prompt_with_context(context_above, context_below)
    };
    vision_llm_chunk_sync(vision, image_bytes, mime, Some(&prompt))
}

/// Port of `rag/prompts/vision_llm_figure_describe_prompt.md`.
const FIGURE_DESCRIBE_PROMPT: &str = r#"## ROLE

You are an expert visual data analyst.

## GOAL

Analyze the image and produce a textual representation strictly based on what is visible in the image.

## DECISION RULE (CRITICAL)

First, determine whether the image contains an explicit visual data representation with enumerable data units forming a coherent dataset.

Enumerable data units are clearly separable, repeatable elements intended for comparison, measurement, or aggregation, such as:

- rows or columns in a table
- individual bars in a bar chart
- identifiable data points or series in a line graph
- labeled segments in a pie chart

The mere presence of numbers, icons, UI elements, or labels does NOT qualify unless they together form such a dataset.

## TASKS

1. Inspect the image and determine which output mode applies based on the decision rule.
2. Follow the output rules strictly.
3. Include only content that is explicitly visible in the image.
4. Do not infer intent, functionality, process logic, or meaning beyond what is visually or textually shown.

## OUTPUT RULES (STRICT)

- Produce output in **exactly one** of the two modes defined below.
- Do NOT mention, label, or reference the modes in the output.
- Do NOT combine content from both modes.
- Do NOT explain or justify the choice of mode.
- Do NOT add any headings, titles, or commentary beyond what the mode requires.

---

## MODE 1: STRUCTURED VISUAL DATA OUTPUT

(Use only if the image contains enumerable data units forming a coherent dataset.)

Output **only** the following fields, in list form.
Do NOT add free-form paragraphs or additional sections.

- Visual Type:
- Title:
- Axes / Legends / Labels:
- Data Points:
- Captions / Annotations:

---

## MODE 2: GENERAL FIGURE CONTENT

(Use only if the image does NOT contain enumerable data units.)

Write the content directly, starting from the first sentence.
Do NOT add any introductory labels, titles, headings, or prefixes.

Requirements:

- Describe visible regions and components in a stable order (e.g., top-to-bottom, left-to-right).
- Explicitly name interface elements or visual objects exactly as they appear (e.g., tabs, panels, buttons, icons, input fields).
- Transcribe all visible text verbatim; do not paraphrase, summarize, or reinterpret labels.
- Describe spatial grouping, containment, and alignment of elements.
- Do NOT interpret intent, behavior, workflows, gameplay rules, or processes.
- Do NOT describe the figure as a chart, diagram, process, phase, or sequence unless such words explicitly appear in the image text.
- Avoid narrative or stylistic language unless it is a dominant and functional visual element.

Use concise, information-dense sentences.
Do not use bullet lists or structured fields in this mode."#;

/// Port of `rag/prompts/vision_llm_figure_describe_prompt_with_context.md`.
const FIGURE_DESCRIBE_PROMPT_WITH_CONTEXT: &str = r#"## ROLE

You are an expert visual data analyst.

## GOAL

Analyze the image and produce a textual representation strictly based on what is visible in the image.
Surrounding context may be used only for minimal clarification or disambiguation of terms that appear in the image, not as a source of new information.

## CONTEXT (ABOVE)

{{ context_above }}

## CONTEXT (BELOW)

{{ context_below }}

## DECISION RULE (CRITICAL)

First, determine whether the image contains an explicit visual data representation with enumerable data units forming a coherent dataset.

Enumerable data units are clearly separable, repeatable elements intended for comparison, measurement, or aggregation, such as:

- rows or columns in a table
- individual bars in a bar chart
- identifiable data points or series in a line graph
- labeled segments in a pie chart

The mere presence of numbers, icons, UI elements, or labels does NOT qualify unless they together form such a dataset.

## TASKS

1. Inspect the image and determine which output mode applies based on the decision rule.
2. Use surrounding context only to disambiguate terms that appear in the image.
3. Follow the output rules strictly.
4. Include only content that is explicitly visible in the image.
5. Do not infer intent, functionality, process logic, or meaning beyond what is visually or textually shown.

## OUTPUT RULES (STRICT)

- Produce output in **exactly one** of the two modes defined below.
- Do NOT mention, label, or reference the modes in the output.
- Do NOT combine content from both modes.
- Do NOT explain or justify the choice of mode.
- Do NOT add any headings, titles, or commentary beyond what the mode requires.

---

## MODE 1: STRUCTURED VISUAL DATA OUTPUT

(Use only if the image contains enumerable data units forming a coherent dataset.)

Output **only** the following fields, in list form.
Do NOT add free-form paragraphs or additional sections.

- Visual Type:
- Title:
- Axes / Legends / Labels:
- Data Points:
- Captions / Annotations:

---

## MODE 2: GENERAL FIGURE CONTENT

(Use only if the image does NOT contain enumerable data units.)

Write the content directly, starting from the first sentence.
Do NOT add any introductory labels, titles, headings, or prefixes.

Requirements:

- Describe visible regions and components in a stable order (e.g., top-to-bottom, left-to-right).
- Explicitly name interface elements or visual objects exactly as they appear (e.g., tabs, panels, buttons, icons, input fields).
- Transcribe all visible text verbatim; do not paraphrase, summarize, or reinterpret labels.
- Describe spatial grouping, containment, and alignment of elements.
- Do NOT interpret intent, behavior, workflows, gameplay rules, or processes.
- Do NOT describe the figure as a chart, diagram, process, phase, or sequence unless such words explicitly appear in the image text.
- Avoid narrative or stylistic language unless it is a dominant and functional visual element.

Use concise, information-dense sentences.
Do not use bullet lists or structured fields in this mode."#;

/// Detect (width, height) from image magic bytes; None for unknown formats.
fn detect_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() >= 24 && data[0] == 0x89 && data[1] == b'P' && data[2] == b'N' && data[3] == b'G'
    {
        // PNG: width/height at bytes 16..24 (big-endian).
        let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        return Some((w, h));
    }
    if data.len() >= 4 && data[0] == 0xFF && data[1] == 0xD8 {
        // JPEG: scan for SOF0/SOF2 marker (0xFFC0/0xFFC2) with dims at +5/+7.
        let mut i = 2usize;
        while i + 9 <= data.len() {
            if data[i] == 0xFF {
                let marker = data[i + 1];
                if matches!(
                    marker,
                    0xC0 | 0xC1
                        | 0xC2
                        | 0xC3
                        | 0xC5
                        | 0xC6
                        | 0xC7
                        | 0xC9
                        | 0xCA
                        | 0xCB
                        | 0xCD
                        | 0xCE
                        | 0xCF
                ) {
                    let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                    let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                    return Some((w, h));
                }
                i += 2;
            } else {
                i += 1;
            }
        }
        return None;
    }
    if data.len() >= 30 && data[0] == b'R' && data[1] == b'I' && data[2] == b'F' && data[3] == b'F'
    {
        // WEBP: "VP8 " at +12 → dims at +26/+28 (LE); "VP8L" at +12 → dims at +21.
        if data.len() >= 30 && &data[12..16] == b"VP8 " {
            let w = u16::from_le_bytes([data[26], data[27]]) as u32 & 0x3FFF;
            let h = u16::from_le_bytes([data[28], data[29]]) as u32 & 0x3FFF;
            return Some((w, h));
        }
        if data.len() >= 25 && &data[12..16] == b"VP8L" {
            let bits = u32::from_le_bytes([data[21], data[22], data[23], data[24]]);
            let w = (bits & 0x3FFF) + 1;
            let h = ((bits >> 14) & 0x3FFF) + 1;
            return Some((w, h));
        }
        return None;
    }
    if data.len() >= 10 && data[0] == b'G' && data[1] == b'I' && data[2] == b'F' && data[3] == b'8'
    {
        // GIF: dims at +6/+8 (LE).
        let w = u16::from_le_bytes([data[6], data[7]]) as u32;
        let h = u16::from_le_bytes([data[8], data[9]]) as u32;
        return Some((w, h));
    }
    if data.len() >= 8 && data[0] == b'B' && data[1] == b'M' {
        // BMP: dims at +18/+22 (LE, signed).
        let w = i32::from_le_bytes([data[18], data[19], data[20], data[21]]);
        let h = i32::from_le_bytes([data[22], data[23], data[24], data[25]]);
        if w > 0 && h > 0 {
            return Some((w as u32, h as u32));
        }
        return None;
    }
    None
}

/// Parse SVG: extract text from <text> and <tspan> elements.
fn parse_svg(data: &[u8]) -> Result<String> {
    let xml = String::from_utf8_lossy(data);
    let mut reader = Reader::from_str(&xml);
    let mut text = String::from("<!--IMAGE_START-->\n");
    let mut text_depth = 0usize;
    loop {
        match reader.read_event()? {
            Event::Start(event)
                if event.local_name().as_ref() == b"text"
                    || event.local_name().as_ref() == b"tspan" =>
            {
                text_depth += 1;
            }
            Event::Text(event) if text_depth > 0 => {
                let value = event.unescape()?;
                if !value.trim().is_empty() {
                    text.push_str(value.trim());
                    text.push('\n');
                }
            }
            Event::CData(event) if text_depth > 0 => {
                let value = String::from_utf8_lossy(event.as_ref());
                if !value.trim().is_empty() {
                    text.push_str(value.trim());
                    text.push('\n');
                }
            }
            Event::End(event)
                if event.local_name().as_ref() == b"text"
                    || event.local_name().as_ref() == b"tspan" =>
            {
                text_depth = text_depth.saturating_sub(1);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    text.push_str("<!--IMAGE_END-->\n");
    Ok(text)
}

/// Parse raster image: OCR first; when OCR text is short (<32 tokens) and a
/// vision client is available, enrich with a vision-model description
/// (mirrors picture.py's "OCR results is too long to use CV LLM" gate).
fn parse_raster(
    name: &str,
    data: &[u8],
    ocr: Option<&OcrClient>,
    vision: Option<&VisionClient>,
) -> Result<String> {
    let mut text = String::from("<!--IMAGE_START-->\n");

    // Add filename as context
    let basename = std::path::Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image");
    text.push_str(&format!("[Image: {}]\n", basename));

    let mime = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mime = match mime.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tiff" | "tif" => "image/tiff",
        _ => "application/octet-stream",
    };

    // Try OCR if client is available
    let mut ocr_text = String::new();
    if let Some(client) = ocr {
        // 已在 tokio runtime 内执行时禁止再建 runtime；用 block_in_place + 当前
        // handle 同步等待（对齐 RAGFlow figure_parser 的同步 OCR 调用语义）。
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(client.ocr(data))
        });
        if let Ok(found) = result
            && !found.is_empty() {
                ocr_text = found;
            }
    }

    // picture.py gate: use OCR text directly when long enough, else ask the
    // vision model to describe the picture.
    let token_count = crate::chunk::token_count(&ocr_text);
    let long_enough = if ocr_text.trim().is_empty() {
        false
    } else {
        // English: >32 words; other langs: >32 chars (mirrors picture.py).
        if ocr_text.chars().all(|c| {
            c.is_ascii()
                && (c.is_ascii_alphanumeric()
                    || c.is_ascii_whitespace()
                    || c.is_ascii_punctuation())
        }) {
            ocr_text.split_whitespace().count() > 32 || token_count > 32
        } else {
            ocr_text.chars().count() > 32 || token_count > 32
        }
    };

    if long_enough {
        text.push_str(&ocr_text);
        if let Some(_vision) = vision {
            // Also attach the vision description when available — RAGFlow
            // appends `txt += "\n" + ans` after describe for short text, but
            // for long OCR text it returns early with OCR only.
        }
    } else if let Some(vision) = vision {
        // "Use CV LLM to describe the picture."
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(describe_image_bytes(vision, data, mime, None))
        {
            Ok(description) if !description.is_empty() => {
                if !ocr_text.trim().is_empty() {
                    text.push_str(&ocr_text);
                    text.push('\n');
                }
                text.push_str(&description);
            }
            Ok(_) => {
                if !ocr_text.trim().is_empty() {
                    text.push_str(&ocr_text);
                } else {
                    text.push_str(&format!(
                        "[Image dimensions: {} bytes, format: {}]",
                        data.len(),
                        detect_format(data),
                    ));
                }
            }
            Err(_) => {
                if !ocr_text.trim().is_empty() {
                    text.push_str(&ocr_text);
                } else {
                    text.push_str(&format!(
                        "[Image dimensions: {} bytes, format: {}]",
                        data.len(),
                        detect_format(data),
                    ));
                }
            }
        }
    } else if !ocr_text.trim().is_empty() {
        text.push_str(&ocr_text);
    } else {
        text.push_str(&format!(
            "[Image: {} bytes, format: {} — enable OCR/vision for text extraction]",
            data.len(),
            detect_format(data),
        ));
    }

    text.push_str("\n<!--IMAGE_END-->\n");
    Ok(text)
}

/// Detect image format from magic bytes.
fn detect_format(data: &[u8]) -> &'static str {
    if data.len() < 4 {
        return "unknown";
    }
    match &data[0..4] {
        [0x89, b'P', b'N', b'G'] => "PNG",
        [0xFF, 0xD8, 0xFF, _] => "JPEG",
        [b'G', b'I', b'F', b'8'] => "GIF",
        [b'R', b'I', b'F', b'F'] => "WEBP/TIFF",
        [b'B', b'M', _, _] => "BMP",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vision::VisionConfig;

    #[test]
    fn svg_text_parsing_handles_nested_spans_and_entities() {
        let parsed = parse_svg(
            br#"<svg><text>Lead <tspan>A &amp; B</tspan> tail</text><path d="M0 0"/></svg>"#,
        )
        .unwrap();
        assert!(parsed.contains("Lead\nA & B\ntail\n"));
    }

    #[test]
    fn clean_markdown_block_strips_fences_and_whitespace() {
        assert_eq!(clean_markdown_block("```markdown\n# Title\n```"), "# Title");
        assert_eq!(clean_markdown_block("```md\nbody\n```"), "body");
        assert_eq!(clean_markdown_block("```\ncode\n```"), "code");
        assert_eq!(clean_markdown_block("  plain text  "), "plain text");
        // No closing fence → leading fence stripped, rest kept (Python behavior).
        assert_eq!(clean_markdown_block("```unclosed"), "unclosed");
    }

    #[test]
    fn dimensions_detected_from_png_jpeg_webp_gif_bmp_magic_bytes() {
        // PNG: signature + 8-byte IHDR chunk header, then BE width/height.
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&[0, 0, 0, 13]); // IHDR len
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&100u32.to_be_bytes());
        png.extend_from_slice(&200u32.to_be_bytes());
        assert_eq!(detect_dimensions(&png), Some((100, 200)));

        // JPEG: SOI + SOF0 with dims (height at +5, width at +7 after marker).
        let mut jpeg = vec![0xFF, 0xD8];
        jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        jpeg.extend_from_slice(&300u16.to_be_bytes()); // height
        jpeg.extend_from_slice(&400u16.to_be_bytes()); // width
        assert_eq!(detect_dimensions(&jpeg), Some((400, 300)));

        // WEBP VP8 lossy: RIFF....WEBPVP8 ... dims at +26 (LE, low 14 bits).
        let mut webp = vec![b'R', b'I', b'F', b'F', 0, 0, 0, 0, b'W', b'E', b'B', b'P'];
        webp.extend_from_slice(b"VP8 ");
        webp.resize(26, 0);
        webp.extend_from_slice(&640u16.to_le_bytes()); // width (low 14 bits)
        webp.extend_from_slice(&480u16.to_le_bytes()); // height (low 14 bits)
        assert_eq!(detect_dimensions(&webp), Some((640, 480)));

        // GIF87a: dims at +6 LE.
        let mut gif = vec![b'G', b'I', b'F', b'8', b'7', b'a'];
        gif.extend_from_slice(&16u16.to_le_bytes());
        gif.extend_from_slice(&32u16.to_le_bytes());
        assert_eq!(detect_dimensions(&gif), Some((16, 32)));

        // BMP: dims at +18 LE signed.
        let mut bmp = vec![b'B', b'M', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        bmp.extend_from_slice(&50i32.to_le_bytes());
        bmp.extend_from_slice(&25i32.to_le_bytes());
        assert_eq!(detect_dimensions(&bmp), Some((50, 25)));

        // Unknown/truncated → None.
        assert_eq!(detect_dimensions(b"hello"), None);
        assert_eq!(detect_dimensions(&[0xFF, 0xD8]), None);
    }

    #[test]
    fn tiny_images_are_skipped_by_vision_llm_chunk_sync() {
        // A 1x1 PNG: signature + IHDR with BE width/height = 1.
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);

        // Without a live endpoint the client construction succeeds but the
        // tiny-image gate must short-circuit BEFORE any network call.
        let vision = VisionClient::new(VisionConfig::default());
        let result = vision_llm_chunk_sync(&vision, &png, "image/png", None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    #[ignore = "requires opt-in GPU vision endpoint"]
    fn gpu_vision_describes_png_via_vision_llm_chunk_sync() {
        // 64x24 white PNG with a black rectangle (same generator as the
        // agent::tests::gpu_vision_endpoint_describes_image_with_prompt test).
        fn encode_png(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
            use flate2::Compression;
            use flate2::write::ZlibEncoder;
            use std::io::Write;
            let mut raw = Vec::with_capacity((width * height * 3 + height) as usize);
            for y in 0..height {
                raw.push(0);
                for x in 0..width {
                    let i = ((y * width + x) * 3) as usize;
                    raw.extend_from_slice(&pixels[i..i + 3]);
                }
            }
            let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
            z.write_all(&raw).unwrap();
            let idat = z.finish().unwrap();
            // PNG chunk CRC-32 (IEEE 802.3, poly 0xEDB88320), no external crate.
            fn crc32(bytes: &[u8]) -> u32 {
                let mut table = [0u32; 256];
                for (i, entry) in table.iter_mut().enumerate() {
                    let mut c = i as u32;
                    for _ in 0..8 {
                        c = if c & 1 != 0 {
                            0xEDB8_8320 ^ (c >> 1)
                        } else {
                            c >> 1
                        };
                    }
                    *entry = c;
                }
                let mut crc = 0xFFFF_FFFFu32;
                for &b in bytes {
                    crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
                }
                crc ^ 0xFFFF_FFFF
            }
            fn chunk(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
                let mut out = Vec::new();
                out.extend_from_slice(&(data.len() as u32).to_be_bytes());
                out.extend_from_slice(tag);
                out.extend_from_slice(data);
                out.extend_from_slice(&crc32(&[tag, data].concat()).to_be_bytes());
                out
            }
            let mut png = Vec::new();
            png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
            let mut ihdr = Vec::new();
            ihdr.extend_from_slice(&width.to_be_bytes());
            ihdr.extend_from_slice(&height.to_be_bytes());
            ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
            png.extend_from_slice(&chunk(b"IHDR", &ihdr));
            png.extend_from_slice(&chunk(b"IDAT", &idat));
            png.extend_from_slice(&chunk(b"IEND", &[]));
            png
        }
        let (w, h) = (64u32, 24u32);
        let mut pixels = vec![255u8; (w * h * 3) as usize];
        for y in 8..16 {
            for x in 8..56 {
                let i = ((y * w + x) * 3) as usize;
                pixels[i] = 0;
                pixels[i + 1] = 0;
                pixels[i + 2] = 0;
            }
        }
        let png = encode_png(w, h, &pixels);

        let base = std::env::var("RAYRAG_TEST_VISION_BASE").unwrap();
        let model = std::env::var("RAYRAG_TEST_VISION_MODEL").unwrap_or_else(|_| "default".into());
        let vision = VisionClient::new(VisionConfig {
            api_base: base,
            api_key: String::new(),
            model,
            lang: "Chinese".into(),
        });
        // Long prompt path: describe_image_bytes with a custom prompt.
        let described = vision_llm_chunk_sync(
            &vision,
            &png,
            "image/png",
            Some("What color is the rectangle? Answer in one word."),
        )
        .unwrap();
        assert!(!described.is_empty());
        let lower = described.to_ascii_lowercase();
        assert!(
            lower.contains("black") || lower.contains("dark") || described.contains("黑"),
            "unexpected vision reply: {described}"
        );
    }
}
