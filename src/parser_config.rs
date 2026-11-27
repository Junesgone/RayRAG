//! Parser configuration helpers — mirrors `common/parser_config_utils.py`
//! normalize_layout_recognizer (used by naive/one/manual/book/laws apps).

/// Normalize a layout_recognize value (parser_config_utils.py:20-36).
///
/// Strings ending with `@mineru` / `@paddleocr` / `@opendataloader` /
/// `@monkeyocrv2` / `@mistral ocr` are split: the part before `@` becomes
/// the parser model name and the recognizer is set to "MinerU" /
/// "PaddleOCR" / "OpenDataLoader" / "MonkeyOCRv2" / "Mistral OCR".
/// Anything else passes through unchanged with a None model name.
pub fn normalize_layout_recognizer(raw: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(layout_recognizer_raw) = raw else {
        return (None, None);
    };
    let lowered = layout_recognizer_raw.to_lowercase();
    let (parser_model_name, layout_recognizer) = if lowered.ends_with("@mineru") {
        (
            Some(
                layout_recognizer_raw
                    .rsplit_once('@')
                    .unwrap()
                    .0
                    .to_string(),
            ),
            "MinerU",
        )
    } else if lowered.ends_with("@paddleocr") {
        (
            Some(
                layout_recognizer_raw
                    .rsplit_once('@')
                    .unwrap()
                    .0
                    .to_string(),
            ),
            "PaddleOCR",
        )
    } else if lowered.ends_with("@mistral ocr") {
        (
            Some(
                layout_recognizer_raw
                    .rsplit_once('@')
                    .unwrap()
                    .0
                    .to_string(),
            ),
            "Mistral OCR",
        )
    } else if lowered.ends_with("@monkeyocrv2") {
        (
            Some(
                layout_recognizer_raw
                    .rsplit_once('@')
                    .unwrap()
                    .0
                    .to_string(),
            ),
            "MonkeyOCRv2",
        )
    } else if lowered.ends_with("@opendataloader") {
        (
            Some(
                layout_recognizer_raw
                    .rsplit_once('@')
                    .unwrap()
                    .0
                    .to_string(),
            ),
            "OpenDataLoader",
        )
    } else {
        (None, layout_recognizer_raw)
    };
    (Some(layout_recognizer.to_string()), parser_model_name)
}

/// Resolve a boolean layout_recognize to the canonical name
/// (one.py:92-93 / naive.py): true → "DeepDOC", false → "Plain Text".
pub fn layout_recognizer_name(layout_recognizer: Option<String>) -> String {
    match layout_recognizer.as_deref() {
        Some("true") | Some("True") => "DeepDOC".to_string(),
        Some("false") | Some("False") => "Plain Text".to_string(),
        Some(other) => other.trim().to_lowercase(),
        // RayRAG defaults to Docling for new knowledge bases; an explicit
        // `true`/`false` still follows RAGFlow's boolean compatibility.
        None => "docling".to_string(),
    }
}


