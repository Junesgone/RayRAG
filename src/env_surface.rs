//! Guards for the environment surface.
//!
//! RayRAG is configured through environment variables, and three artefacts have to
//! agree about their names: the code that reads them, `.env.example` (the template a
//! new deployment copies and edits), and the first-login wizard that writes values
//! back into the same file. When they drift apart the failure is silent and expensive:
//! an operator sets a variable that nothing reads, or a knob exists that nobody can
//! discover, and the application looks like it ignored the setting.
//!
//! The tests below enforce the two directions that matter, on every `cargo test`:
//!
//! * **Nothing undocumented.** Every key the crate reads is either in `.env.example`
//!   or in [`INTERNAL_ENV`], where each entry carries the reason it is not deployment
//!   configuration (a compile-time macro, an ambient variable, or something `build.rs`
//!   injects).
//! * **Nothing fictitious.** Every key `.env.example` offers is really read by the
//!   crate, or is consumed by the container tooling listed in [`TOOLING_ONLY_ENV`].
//!   A documented variable that nothing reads is a lie told to the operator.
//!
//! Both lists are deliberately explicit: adding an entry forces a human to write down
//! why the key is exempt, which is the review this file exists to trigger.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Keys the crate reads that are **not** deployment configuration.
///
/// Every entry is a (name, reason) pair; the reason is the review artefact.
pub const INTERNAL_ENV: &[(&str, &str)] = &[
    (
        "CARGO_MANIFEST_DIR",
        "compile-time macro, not runtime config",
    ),
    (
        "CARGO_PKG_VERSION",
        "compile-time macro, not runtime config",
    ),
    ("HOME", "ambient process environment"),
    ("PATH", "ambient process environment"),
    ("LANG", "ambient process environment"),
    (
        "RAYRAG_BUILD_GIT_REV",
        "injected by build.rs into the build banner",
    ),
    (
        "RAYRAG_BUILD_GIT_DIRTY",
        "injected by build.rs into the build banner",
    ),
    (
        "RAYRAG_BUILD_TIME",
        "injected by build.rs into the build banner",
    ),
    (
        "COMPOSE_PROFILES",
        "set by docker compose itself; selects optional model profiles",
    ),
    (
        "RAGFLOW_CRYPTO_KEY",
        "upstream-compatible crypto key of the parity tooling only",
    ),
    (
        "RAGFLOW_FIXTURE_REPO",
        "read-only RAGFlow parity fixture path (internal tooling)",
    ),
    (
        "RAGFLOW_SOURCE",
        "read-only RAGFlow parity source path (internal tooling)",
    ),
    (
        "RAGFLOW_SOURCE_DIR",
        "read-only RAGFlow parity source path (internal tooling)",
    ),
    (
        "RAGFLOW_VERSION",
        "upstream version reported by the parity tooling",
    ),
];

/// Template keys consumed by docker-compose / Dockerfile / `docker/*.sh` rather than by
/// the Rust crate. Documented on purpose: the operator edits them, the container tooling
/// reads them.
pub const TOOLING_ONLY_ENV: &[(&str, &str)] = &[
    ("RAYRAG_PORT", "compose publishes this host port"),
    ("RAYRAG_IMAGE", "compose image tag"),
    ("RAYRAG_FEATURES", "Dockerfile cargo feature list"),
    (
        "RAYRAG_MIRROR_PROFILE",
        "docker/mirror-setup.sh selects the mirror profile",
    ),
    (
        "RAYRAG_POSTGRES_PASSWORD",
        "compose builds the postgres DSN from it",
    ),
    ("RAYRAG_TIMEZONE", "compose sets the container TZ"),
    ("POSTGRES_IMAGE", "compose image tag"),
    ("RUST_IMAGE", "Dockerfile build stage image"),
    ("RUNTIME_IMAGE", "Dockerfile runtime stage image"),
    ("DEBIAN_MIRROR", "Dockerfile apt mirror override"),
    (
        "DEBIAN_SECURITY_MIRROR",
        "Dockerfile apt security mirror override",
    ),
    (
        "ZVEC_RELEASE_BASE",
        "Dockerfile downloads the zvec native library from it",
    ),
    (
        "ZVEC_RELEASE_FALLBACK_BASE",
        "Dockerfile fallback mirror for the zvec native library",
    ),
    ("ZVEC_DOWNLOAD_TIMEOUT", "Dockerfile download timeout"),
];

/// This guard file: excluded from the "is it really read?" reverse scan because it
/// contains every name as data.
const GUARD_FILE: &str = "env_surface.rs";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

fn read_sources() -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    rust_sources(&manifest_dir().join("src"), &mut files);
    files.sort();
    files
        .into_iter()
        .filter_map(|path| {
            let text = fs::read_to_string(&path).ok()?;
            Some((path, text))
        })
        .collect()
}

/// Sources the scanner itself may parse.
///
/// This guard file is excluded: it spells out `env::var` and the macro names as data,
/// so a scanner reading its own source would report the needle rather than a call.
fn scanned_sources() -> Vec<(PathBuf, String)> {
    read_sources()
        .into_iter()
        .filter(|(path, _)| path.file_name().and_then(|name| name.to_str()) != Some(GUARD_FILE))
        .collect()
}

/// The identifier starting at `start`, if there is one.
fn ident_at(text: &str, start: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let first = *bytes.get(start)?;
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return None;
    }
    let mut end = start + 1;
    while let Some(byte) = bytes.get(end) {
        if byte.is_ascii_alphanumeric() || *byte == b'_' {
            end += 1;
        } else {
            break;
        }
    }
    Some((text[start..end].to_string(), end))
}

fn skip_ws(text: &str, mut index: usize) -> usize {
    let bytes = text.as_bytes();
    while let Some(byte) = bytes.get(index) {
        if byte.is_ascii_whitespace() {
            index += 1;
        } else {
            break;
        }
    }
    index
}

/// The string literal starting at `index`, if the text there is one.
fn string_literal_at(text: &str, index: usize) -> Option<(String, usize)> {
    if text.as_bytes().get(index) != Some(&b'"') {
        return None;
    }
    let rest = &text[index + 1..];
    let end = rest.find('"')?;
    Some((rest[..end].to_string(), index + 1 + end + 1))
}

/// `const NAME: &str = "VALUE";` / `static NAME: &str = "VALUE";` declarations, so a key
/// the crate reads through a constant is still discovered.
fn string_consts(sources: &[(PathBuf, String)]) -> BTreeMap<String, String> {
    let mut consts = BTreeMap::new();
    for (_, text) in sources {
        for keyword in ["const ", "static "] {
            let mut from = 0;
            while let Some(found) = text[from..].find(keyword) {
                let start = from + found + keyword.len();
                from = start;
                let Some((name, after_name)) = ident_at(text, skip_ws(text, start)) else {
                    continue;
                };
                let index = skip_ws(text, after_name);
                if text.as_bytes().get(index) != Some(&b':') {
                    continue;
                }
                let Some(rel_eq) = text[index..].find('=') else {
                    continue;
                };
                let Some(rel_semi) = text[index..].find(';') else {
                    continue;
                };
                // A statement that ends before the first `=` is not an initialiser.
                if rel_semi < rel_eq {
                    continue;
                }
                let eq_at = index + rel_eq;
                if !text[index + 1..eq_at].contains("str") {
                    continue;
                }
                let Some((value, _)) = string_literal_at(text, skip_ws(text, eq_at + 1)) else {
                    continue;
                };
                consts.insert(name, value);
            }
        }
    }
    consts
}

/// True when the line looks like a dotenv assignment (active or commented out).
fn assignment_name(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let body = trimmed.strip_prefix('#').unwrap_or(trimmed).trim_start();
    let eq = body.find('=')?;
    let name = body[..eq].trim();
    let mut chars = name.chars();
    let first = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_')
        || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    Some(name.to_string())
}

/// Every environment key this crate reads: literals, constant-aliased names, and the
/// compile-time `env!`/`option_env!` macros.
pub fn read_keys() -> BTreeSet<String> {
    let sources = scanned_sources();
    let consts = string_consts(&sources);
    let mut keys = BTreeSet::new();
    for (_, text) in &sources {
        let mut from = 0;
        while let Some(found) = text[from..].find("env::var") {
            let mut index = from + found + "env::var".len();
            from = index;
            if text[index..].starts_with("_os") {
                index += "_os".len();
            }
            index = skip_ws(text, index);
            if text.as_bytes().get(index) != Some(&b'(') {
                continue;
            }
            index = skip_ws(text, index + 1);
            if let Some((key, _)) = string_literal_at(text, index) {
                keys.insert(key);
            } else if let Some((name, _)) = ident_at(text, index)
                && let Some(value) = consts.get(&name)
            {
                keys.insert(value.clone());
            }
        }
        for macro_name in ["env!(", "option_env!("] {
            let mut from = 0;
            while let Some(found) = text[from..].find(macro_name) {
                let index = from + found + macro_name.len();
                from = index;
                if let Some((key, _)) = string_literal_at(text, skip_ws(text, index)) {
                    keys.insert(key);
                }
            }
        }
    }
    keys
}

/// Template keys mapped to their declared value, in file order.
///
/// A commented `#KEY=` line counts as documented: the template comments out values
/// whose absence is meaningful, and the operator only has to remove the `#`.
pub fn template_entries() -> BTreeMap<String, String> {
    let mut entries = BTreeMap::new();
    for line in include_str!("../.env.example").lines() {
        let Some(name) = assignment_name(line) else {
            continue;
        };
        let trimmed = line.trim_start();
        let body = trimmed.strip_prefix('#').unwrap_or(trimmed).trim_start();
        let value = body
            .split_once('=')
            .map(|(_, value)| value.trim().to_string())
            .unwrap_or_default();
        entries.insert(name, value);
    }
    entries
}

/// Keys declared by `.env.example`, active or commented out.
pub fn documented_keys() -> BTreeSet<String> {
    template_entries().into_keys().collect()
}

fn names(list: &[(&str, &str)]) -> BTreeSet<String> {
    list.iter().map(|(name, _)| (*name).to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys read by the crate but absent from `.env.example` and from the exemption
    /// lists. Any name here is an operator-facing setting nobody can discover.
    fn undocumented_reads() -> Vec<String> {
        let documented = documented_keys();
        let internal = names(INTERNAL_ENV);
        read_keys()
            .into_iter()
            .filter(|key| !documented.contains(key) && !internal.contains(key))
            .collect()
    }

    /// Template keys that no source file mentions and no container tool reads.
    fn fictitious_documents() -> Vec<String> {
        let blob: String = scanned_sources()
            .into_iter()
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join("\n");
        let tooling = names(TOOLING_ONLY_ENV);
        documented_keys()
            .into_iter()
            .filter(|key| !tooling.contains(key) && !blob.contains(&format!("\"{key}\"")))
            .collect()
    }

    #[test]
    fn every_env_key_the_crate_reads_is_documented() {
        let missing = undocumented_reads();
        assert!(
            missing.is_empty(),
            "environment keys read by src/ but missing from .env.example \
             (document them there, or add a reasoned entry to env_surface::INTERNAL_ENV): {missing:?}"
        );
    }

    #[test]
    fn every_documented_key_is_read_by_the_crate_or_a_container_tool() {
        let bogus = fictitious_documents();
        assert!(
            bogus.is_empty(),
            ".env.example offers keys that no code and no container tool reads \
             (fix the name, remove the line, or add a reasoned entry to \
             env_surface::TOOLING_ONLY_ENV): {bogus:?}"
        );
    }

    #[test]
    fn env_template_declares_every_key_at_most_once() {
        let mut seen = BTreeSet::new();
        let mut duplicates = Vec::new();
        for line in include_str!("../.env.example").lines() {
            let Some(name) = assignment_name(line) else {
                continue;
            };
            if !seen.insert(name.clone()) {
                duplicates.push(name);
            }
        }
        assert!(
            duplicates.is_empty(),
            ".env.example declares the same key twice; in a dotenv file the last one \
             silently wins: {duplicates:?}"
        );
    }

    #[test]
    fn every_setup_field_is_documented_in_the_template() {
        let documented = documented_keys();
        let missing: Vec<&str> = crate::api::setup::field_keys()
            .into_iter()
            .filter(|key| !documented.contains(*key))
            .collect();
        assert!(
            missing.is_empty(),
            "the first-login wizard writes keys the template does not document: {missing:?}"
        );
    }

    #[test]
    fn internal_exemptions_carry_a_reason() {
        for (name, reason) in INTERNAL_ENV.iter().chain(TOOLING_ONLY_ENV) {
            assert!(!name.is_empty(), "exemption without a name");
            assert!(
                reason.len() >= 10,
                "exemption {name} needs a real reason, got {reason:?}"
            );
        }
    }

    #[test]
    fn guard_scanner_finds_known_reads() {
        // A sanity check on the scanner itself: these keys are read through literals and
        // through constants, so both paths must be discovered.
        let keys = read_keys();
        for expected in [
            "RAYRAG_CMD_TIMEOUT",
            "RAYRAG_MODEL_TIMEOUT",
            "RAYRAG_HTTP_BODY_LIMIT_BYTES",
            "RAYRAG_VECTOR_BACKEND",
            "RAYRAG_ZVEC_DIR",
            "EMBED_API_BASE",
            "LLM_API_KEY",
        ] {
            assert!(keys.contains(expected), "scanner missed {expected}");
        }
        assert!(!keys.contains("NOT_AN_ENV_KEY"));
    }
}
