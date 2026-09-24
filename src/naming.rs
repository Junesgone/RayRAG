//! RAGFlow-compatible duplicate-name handling.

use std::path::Path;

/// Return the first available name, appending `(1)`, `(2)`, ... before the extension.
pub fn duplicate_name(
    original: &str,
    mut exists: impl FnMut(&str) -> bool,
) -> anyhow::Result<String> {
    const MAX_RETRIES: usize = 1000;
    let mut current = original.to_string();
    for _ in 0..=MAX_RETRIES {
        if !exists(&current) {
            return Ok(current);
        }
        let path = Path::new(&current);
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or(&current);
        let suffix = path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| format!(".{value}"))
            .unwrap_or_default();
        let (main, counter) = split_counter(stem);
        current = format!("{main}({}){suffix}", counter.unwrap_or(0) + 1);
    }
    anyhow::bail!("Failed to generate a unique name after {MAX_RETRIES} attempts")
}

fn split_counter(stem: &str) -> (&str, Option<usize>) {
    let Some(prefix) = stem.strip_suffix(')') else {
        return (stem, None);
    };
    let Some(open) = prefix.rfind('(') else {
        return (stem, None);
    };
    let number = &prefix[open + 1..];
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return (stem, None);
    }
    match number.parse() {
        Ok(counter) => (prefix[..open].trim_end(), Some(counter)),
        Err(_) => (stem, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increments_existing_counter_before_extension() {
        let existing = ["report.pdf", "report(1).pdf", "report(2).pdf"];
        assert_eq!(
            duplicate_name("report.pdf", |name| existing.contains(&name)).unwrap(),
            "report(3).pdf"
        );
        assert_eq!(
            duplicate_name("report(1).pdf", |name| existing.contains(&name)).unwrap(),
            "report(3).pdf"
        );
    }
}
