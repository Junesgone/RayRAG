//! Parser configuration helpers — mirrors `common/parser_config_utils.py`
//! normalize_layout_recognizer (used by naive/one/manual/book/laws apps).

/// Normalize a layout_recognize value (parser_config_utils.py:20-36).
///
/// Strings ending with `@mineru` / `@paddleocr` / `@opendataloader` are
/// split: the part before `@` becomes the parser model name and the
/// recognizer is set to "MinerU" / "PaddleOCR" / "OpenDataLoader".
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
        None => "deepdoc".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_plain_value_passes_through() {
        let (layout, model) = normalize_layout_recognizer(Some("DeepDOC"));
        assert_eq!(layout.as_deref(), Some("DeepDOC"));
        assert!(model.is_none());
    }

    #[test]
    fn normalize_mineru_suffix_splits_model_name() {
        let (layout, model) = normalize_layout_recognizer(Some("Qwen2-VL@mineru"));
        assert_eq!(layout.as_deref(), Some("MinerU"));
        assert_eq!(model.as_deref(), Some("Qwen2-VL"));
    }

    #[test]
    fn normalize_paddleocr_suffix_splits_model_name() {
        let (layout, model) = normalize_layout_recognizer(Some("PaddleOCR-VL@PaddleOCR"));
        assert_eq!(layout.as_deref(), Some("PaddleOCR"));
        assert_eq!(model.as_deref(), Some("PaddleOCR-VL"));
    }

    #[test]
    fn normalize_opendataloader_suffix_splits_model_name() {
        let (layout, model) = normalize_layout_recognizer(Some("my-model@OpenDataLoader"));
        assert_eq!(layout.as_deref(), Some("OpenDataLoader"));
        assert_eq!(model.as_deref(), Some("my-model"));
    }

    #[test]
    fn normalize_none_returns_none() {
        let (layout, model) = normalize_layout_recognizer(None);
        assert!(layout.is_none());
        assert!(model.is_none());
    }

    #[test]
    fn layout_name_from_bool_strings() {
        assert_eq!(layout_recognizer_name(Some("true".to_string())), "DeepDOC");
        assert_eq!(
            layout_recognizer_name(Some("false".to_string())),
            "Plain Text"
        );
        assert_eq!(
            layout_recognizer_name(Some("Plain Text".to_string())),
            "plain text"
        );
        assert_eq!(layout_recognizer_name(None), "deepdoc");
    }
}
