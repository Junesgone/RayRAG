//! MWS GPT Model Hub helpers — RAGFlow v0.27.2 `rag/llm/mws_utils.py`.
//!
//! The MWS provider is addressed by a project-root URL of the form
//! `https://gpt.mwsapis.ru/projects/<project>`; these helpers validate and
//! normalize that root, build endpoint URLs relative to it and normalize the
//! bearer token.

use crate::Result;

/// Validate and normalize an MWS GPT Model Hub project-root URL.
pub fn normalize_mws_project_url(base_url: Option<&str>) -> Result<String> {
    let value = base_url.unwrap_or_default().trim().trim_end_matches('/');
    let parsed = reqwest::Url::parse(value).ok();
    let Some(parsed) = parsed else {
        anyhow::bail!(
            "MWS API URL must be a project root in the form https://gpt.mwsapis.ru/projects/<project>"
        );
    };
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none_or(str::is_empty)
        || parsed.host().is_none()
    {
        anyhow::bail!(
            "MWS API URL must be a project root in the form https://gpt.mwsapis.ru/projects/<project>"
        );
    }
    let path_parts: Vec<&str> = parsed.path().trim_matches('/').split('/').collect();
    if path_parts.len() != 2 || path_parts[0] != "projects" || path_parts[1].is_empty() {
        anyhow::bail!(
            "MWS API URL must be a project root in the form https://gpt.mwsapis.ru/projects/<project>"
        );
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        anyhow::bail!(
            "MWS API URL must not contain credentials, parameters, a query string, or a fragment"
        );
    }
    let project = path_parts[1].to_owned();
    let mut normalized = parsed;
    normalized.set_path(&format!("/projects/{project}"));
    Ok(normalized.to_string())
}

/// Build an MWS API endpoint relative to a validated project root.
pub fn mws_api_url(base_url: Option<&str>, endpoint: &str) -> Result<String> {
    Ok(format!(
        "{}/{}",
        normalize_mws_project_url(base_url)?,
        endpoint.trim_matches('/')
    ))
}

/// Return a normalized MWS bearer token or reject an empty value.
pub fn require_mws_token(token: Option<&str>) -> Result<String> {
    let value = token.unwrap_or_default().trim();
    if value.is_empty() {
        anyhow::bail!("MWS Token is required");
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_project_root_urls() {
        assert_eq!(
            normalize_mws_project_url(Some("https://gpt.mwsapis.ru/projects/demo/")).unwrap(),
            "https://gpt.mwsapis.ru/projects/demo"
        );
        assert_eq!(
            normalize_mws_project_url(Some(" http://host:8080/projects/p ")).unwrap(),
            "http://host:8080/projects/p"
        );
    }

    #[test]
    fn rejects_invalid_project_root_urls() {
        for value in [
            None,
            Some(""),
            Some("gpt.mwsapis.ru/projects/demo"),
            Some("ftp://gpt.mwsapis.ru/projects/demo"),
            Some("https://gpt.mwsapis.ru"),
            Some("https://gpt.mwsapis.ru/projects"),
            Some("https://gpt.mwsapis.ru/projects/demo/chat"),
            Some("https://user:pass@gpt.mwsapis.ru/projects/demo"),
            Some("https://gpt.mwsapis.ru/projects/demo?x=1"),
            Some("https://gpt.mwsapis.ru/projects/demo#frag"),
        ] {
            assert!(normalize_mws_project_url(value).is_err(), "{value:?}");
        }
    }

    #[test]
    fn builds_endpoints_and_requires_token() {
        assert_eq!(
            mws_api_url(
                Some("https://gpt.mwsapis.ru/projects/demo"),
                "/v1/chat/completions"
            )
            .unwrap(),
            "https://gpt.mwsapis.ru/projects/demo/v1/chat/completions"
        );
        assert_eq!(require_mws_token(Some("  tok ")).unwrap(), "tok");
        assert!(require_mws_token(None).is_err());
        assert!(require_mws_token(Some("   ")).is_err());
    }
}
