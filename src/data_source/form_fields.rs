//! Per-source dynamic form schema for the data-source management UI.
//!
//! Mirrors the fixed RAGFlow v0.26.4 frontend contract in
//! `web/src/pages/user-setting/data-source/constant/index.tsx` plus the
//! `s3-constant.tsx` / `confluence-constant.tsx` / `jira-constant.tsx` /
//! `seafile-constant.tsx` / `bitbucket-constant.tsx` companions. The schema
//! is a compile-time embedded JSON asset generated from those files at
//! commit `cb93883f3f8c975eecb2fed81210effeb3bdb06f`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

const SCHEMA_JSON: &str = include_str!("form_schema.json");
const SOURCE_INFO_JSON: &str = include_str!("source_info.json");

/// The fixed AWS region list used by both the Bedrock provider modal and the
/// S3 connector's region selector (RAGFlow derives it from `BedrockRegionList`).
pub const AWS_REGIONS: &[(&str, &str)] = &[
    ("us-east-2", "US East (Ohio)"),
    ("us-east-1", "US East (N. Virginia)"),
    ("us-west-1", "US West (N. California)"),
    ("us-west-2", "US West (Oregon)"),
    ("af-south-1", "Africa (Cape Town)"),
    ("ap-east-1", "Asia Pacific (Hong Kong)"),
    ("ap-south-2", "Asia Pacific (Hyderabad)"),
    ("ap-southeast-3", "Asia Pacific (Jakarta)"),
    ("ap-southeast-5", "Asia Pacific (Malaysia)"),
    ("ap-southeast-4", "Asia Pacific (Melbourne)"),
    ("ap-south-1", "Asia Pacific (Mumbai)"),
    ("ap-northeast-3", "Asia Pacific (Osaka)"),
    ("ap-northeast-2", "Asia Pacific (Seoul)"),
    ("ap-southeast-1", "Asia Pacific (Singapore)"),
    ("ap-southeast-2", "Asia Pacific (Sydney)"),
    ("ap-east-2", "Asia Pacific (Taipei)"),
    ("ap-southeast-7", "Asia Pacific (Thailand)"),
    ("ap-northeast-1", "Asia Pacific (Tokyo)"),
    ("ca-central-1", "Canada (Central)"),
    ("ca-west-1", "Canada West (Calgary)"),
    ("eu-central-1", "Europe (Frankfurt)"),
    ("eu-west-1", "Europe (Ireland)"),
    ("eu-west-2", "Europe (London)"),
    ("eu-south-1", "Europe (Milan)"),
    ("eu-west-3", "Europe (Paris)"),
    ("eu-south-2", "Europe (Spain)"),
    ("eu-north-1", "Europe (Stockholm)"),
    ("eu-central-2", "Europe (Zurich)"),
    ("il-central-1", "Israel (Tel Aviv)"),
    ("mx-central-1", "Mexico (Central)"),
    ("me-south-1", "Middle East (Bahrain)"),
    ("me-central-1", "Middle East (UAE)"),
    ("sa-east-1", "South America (São Paulo)"),
    ("us-gov-east-1", "AWS GovCloud (US-East)"),
    ("us-gov-west-1", "AWS GovCloud (US-West)"),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormOption {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShowWhen {
    /// Dot-path of another field in the same form (e.g. `config.auth_mode`).
    pub field: String,
    /// Value that makes this field visible; `show_when` conditions are
    /// OR-ed, `show_when_all` conditions are AND-ed.
    pub equals: String,
    /// Mirrors upstream `!==` predicates (e.g. Jira `is_cloud !== false`).
    #[serde(default)]
    pub negate: bool,
}

impl ShowWhen {
    fn matches(&self, values: &HashMap<String, String>) -> bool {
        let equals = values.get(&self.field).map(|value| value.as_str())
            == Some(self.equals.as_str());
        self.negate != equals
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormFieldSpec {
    pub label: String,
    /// Dot-path into the connector `config` object, e.g.
    /// `config.credentials.client_id`.
    pub name: String,
    /// `Text` | `Password` | `Number` | `Checkbox` | `Select` | `Segmented` |
    /// `Textarea` | `Email` | `Switch`.
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub default_value: Option<String>,
    #[serde(default)]
    pub options: Vec<FormOption>,
    /// OR-ed visibility predicates; absent means always visible.
    #[serde(default)]
    pub show_when: Vec<ShowWhen>,
    /// AND-ed visibility predicates; absent/empty means no restriction.
    #[serde(default)]
    pub show_when_all: Vec<ShowWhen>,
    /// Upstream `tooltip` i18n strings resolved from locales/en.ts / zh.ts;
    /// rendered as hint lines under the field label in the add/detail modals.
    #[serde(default)]
    pub tooltip_en: Option<String>,
    #[serde(default)]
    pub tooltip_zh: Option<String>,
}

impl FormFieldSpec {
    /// Whether this field is visible for the supplied form values, where keys
    /// are dot-paths and values are the current string form. `show_when` is
    /// OR-ed, `show_when_all` is AND-ed; empty lists mean no restriction.
    pub fn visible(&self, values: &HashMap<String, String>) -> bool {
        let any_ok = self.show_when.is_empty()
            || self
                .show_when
                .iter()
                .any(|condition| condition.matches(values));
        let all_ok = self.show_when_all.is_empty()
            || self
                .show_when_all
                .iter()
                .all(|condition| condition.matches(values));
        any_ok && all_ok
    }
}

fn schema() -> &'static HashMap<String, Vec<FormFieldSpec>> {
    static SCHEMA: OnceLock<HashMap<String, Vec<FormFieldSpec>>> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let mut parsed: HashMap<String, Vec<FormFieldSpec>> =
            serde_json::from_str(SCHEMA_JSON).expect("embedded data-source form schema");
        // The S3 region selector reuses RAGFlow's Bedrock region list; the
        // upstream frontend derives it from the same constant.
        if let Some(fields) = parsed.get_mut("S3") {
            for field in fields {
                if field.name == "config.credentials.region" {
                    field.options = AWS_REGIONS
                        .iter()
                        .map(|(value, label)| FormOption {
                            label: (*label).to_string(),
                            value: (*value).to_string(),
                        })
                        .collect();
                }
            }
        }
        parsed
    })
}

/// The per-source field list; empty for unknown sources (upstream also
/// renders no extra fields when a source has no entry).
pub fn fields_for(source: &str) -> Vec<FormFieldSpec> {
    schema()
        .get(&source.to_ascii_uppercase())
        .cloned()
        .unwrap_or_default()
}

/// The full schema serialized for the browser-driven detail form. Includes the
/// runtime-injected S3 region options.
pub fn schema_for_ui() -> serde_json::Value {
    serde_json::to_value(schema()).expect("data-source form schema is serializable")
}

/// The number of sources with a defined field set.
pub fn schema_source_count() -> usize {
    schema().len()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    pub name: String,
    pub description: String,
    /// Static asset path under `/assets/svg/...` or an emoji fallback for the
    /// four upstream sources rendered with lucide icons.
    pub icon: String,
}

fn source_info() -> &'static HashMap<String, SourceInfo> {
    static SOURCE_INFO: OnceLock<HashMap<String, SourceInfo>> = OnceLock::new();
    SOURCE_INFO.get_or_init(|| {
        serde_json::from_str(SOURCE_INFO_JSON).expect("embedded data-source info manifest")
    })
}

pub fn source_info_for(key: &str) -> Option<&'static SourceInfo> {
    source_info().get(&key.to_ascii_lowercase())
}

/// The 35-source card manifest serialized for the browser (name/description/
/// icon per upstream `dataSourceInfo`).
pub fn source_info_for_ui() -> serde_json::Value {
    serde_json::to_value(source_info()).expect("source info manifest is serializable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_schema_covers_all_upstream_sources_with_valid_paths() {
        let parsed: HashMap<String, Vec<FormFieldSpec>> =
            serde_json::from_str(SCHEMA_JSON).expect("schema parses");
        assert_eq!(parsed.len(), 35, "all fixed upstream sources are mapped");
        for (source, fields) in &parsed {
            assert!(!fields.is_empty(), "{source} has fields");
            for field in fields {
                assert!(
                    field.name.starts_with("config."),
                    "{source} field {} must be a config dot-path",
                    field.name
                );
                for condition in &field.show_when {
                    assert!(
                        condition.field.starts_with("config."),
                        "{source} show_when must reference a config path"
                    );
                }
                if matches!(field.field_type.as_str(), "Select" | "Segmented")
                    && !(source == "S3" && field.name == "config.credentials.region")
                {
                    assert!(
                        !field.options.is_empty(),
                        "{source}.{} has select options",
                        field.name
                    );
                }
            }
        }
    }

    #[test]
    fn gitlab_schema_keeps_the_fixed_seven_fields() {
        let fields = fields_for("gitlab");
        assert_eq!(fields.len(), 7);
        let names: Vec<&str> = fields.iter().map(|field| field.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "config.project_owner",
                "config.project_name",
                "config.credentials.gitlab_access_token",
                "config.gitlab_url",
                "config.include_mrs",
                "config.include_issues",
                "config.include_code_files",
            ]
        );
        for field in fields.iter().take(4) {
            assert!(field.required, "{} is required", field.name);
        }
    }

    #[test]
    fn unknown_source_has_no_fields() {
        assert!(fields_for("not-a-source").is_empty());
    }

    #[test]
    fn s3_region_options_inherit_the_bedrock_region_list() {
        let fields = fields_for("s3");
        let region = fields
            .iter()
            .find(|field| field.name == "config.credentials.region")
            .expect("S3 has a region field");
        assert_eq!(region.options.len(), AWS_REGIONS.len());
        assert_eq!(region.options[0].value, "us-east-2");
    }

    fn find(source: &str, name: &str) -> FormFieldSpec {
        fields_for(source)
            .into_iter()
            .find(|field| field.name == name)
            .unwrap_or_else(|| panic!("{source} has {name}"))
    }

    fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn s3_access_key_fields_use_the_or_condition() {
        // Upstream: authMode === 'access_key' || bucketType === 's3_compatible'
        let field = find("s3", "config.credentials.aws_access_key_id");
        assert!(field.visible(&values(&[
            ("config.credentials.authentication_method", "access_key"),
            ("config.bucket_type", "s3"),
        ])));
        // s3_compatible bucket shows the keys even without access_key auth.
        assert!(field.visible(&values(&[
            ("config.credentials.authentication_method", "iam_role"),
            ("config.bucket_type", "s3_compatible"),
        ])));
        // Neither condition holds: hidden.
        assert!(!field.visible(&values(&[
            ("config.credentials.authentication_method", "iam_role"),
            ("config.bucket_type", "s3"),
        ])));
    }

    #[test]
    fn s3_role_arn_uses_the_and_condition() {
        // Upstream: authMode === 'iam_role' && bucketType === 's3'
        let field = find("s3", "config.credentials.aws_role_arn");
        assert!(field.visible(&values(&[
            ("config.credentials.authentication_method", "iam_role"),
            ("config.bucket_type", "s3"),
        ])));
        assert!(!field.visible(&values(&[
            ("config.credentials.authentication_method", "access_key"),
            ("config.bucket_type", "s3"),
        ])));
        assert!(!field.visible(&values(&[
            ("config.credentials.authentication_method", "iam_role"),
            ("config.bucket_type", "s3_compatible"),
        ])));
    }

    #[test]
    fn jira_cloud_server_conditionals_match_upstream() {
        // `is_cloud !== false` → cloud fields; `=== false` → server fields.
        let email = find("jira", "config.credentials.jira_user_email");
        let token = find("jira", "config.credentials.jira_api_token");
        let scoped = find("jira", "config.scoped_token");
        let username = find("jira", "config.credentials.jira_username");
        let password = find("jira", "config.credentials.jira_password");
        for field in [&email, &token, &scoped] {
            assert!(field.visible(&values(&[("config.is_cloud", "true")])));
            assert!(field.visible(&values(&[])), "absent is_cloud is cloud");
            assert!(!field.visible(&values(&[("config.is_cloud", "false")])));
        }
        for field in [&username, &password] {
            assert!(!field.visible(&values(&[("config.is_cloud", "true")])));
            assert!(field.visible(&values(&[("config.is_cloud", "false")])));
        }
    }

    #[test]
    fn jira_tag_fields_and_defaults_match_upstream() {
        let labels = find("jira", "config.labels_to_skip");
        assert_eq!(labels.field_type, "Tag");
        let blacklist = find("jira", "config.comment_email_blacklist");
        assert_eq!(blacklist.field_type, "Tag");
        assert_eq!(
            find("jira", "config.include_comments").default_value.as_deref(),
            Some("true")
        );
        assert_eq!(
            find("jira", "config.include_attachments").default_value.as_deref(),
            Some("false")
        );
        assert_eq!(find("jira", "config.is_cloud").default_value.as_deref(), Some("true"));
    }

    #[test]
    fn seafile_token_is_visible_in_every_sync_scope() {
        let token = find("seafile", "config.credentials.seafile_token");
        for scope in ["account", "library", "directory"] {
            assert!(
                token.visible(&values(&[("config.sync_scope", scope)])),
                "token visible for {scope}"
            );
        }
        assert_eq!(
            find("seafile", "config.include_shared").default_value.as_deref(),
            Some("true")
        );
    }

    #[test]
    fn show_when_predicates_are_or_ed() {
        let mut values = HashMap::new();
        values.insert("config.auth_mode".to_string(), "account_key".to_string());
        let fields = fields_for("azure_blob");
        let account_name = fields
            .iter()
            .find(|field| field.name == "config.credentials.account_name")
            .unwrap();
        assert!(account_name.visible(&values));
        values.insert("config.auth_mode".to_string(), "sas_token".to_string());
        assert!(!account_name.visible(&values));
    }

    #[test]
    fn source_info_manifest_covers_all_35_upstream_sources() {
        let parsed: HashMap<String, SourceInfo> =
            serde_json::from_str(SOURCE_INFO_JSON).expect("info manifest parses");
        assert_eq!(
            parsed.len(),
            35,
            "all upstream DataSourceKey entries mapped"
        );
        for (key, info) in &parsed {
            assert!(!info.name.is_empty(), "{key} has a display name");
            assert!(
                info.icon.ends_with(".svg") || info.icon.chars().count() <= 4,
                "{key} icon is a static asset or emoji fallback"
            );
        }
        let s3 = source_info_for("S3").expect("S3 entry present");
        assert_eq!(s3.name, "S3");
        assert_eq!(s3.icon, "assets/svg/data-source/s3.svg");
        assert!(s3.description.contains("AWS S3"));
        assert!(source_info_for("not-a-source").is_none());
    }
    #[test]
    fn onedrive_tenant_id_field_carries_upstream_tooltip() {
        let field = find("onedrive", "config.credentials.tenant_id");
        assert!(
            field.tooltip_en.as_deref().unwrap_or("").contains("tenant ID"),
            "tooltip_en merged from upstream locale: {:?}",
            field.tooltip_en
        );
        assert!(field.tooltip_zh.is_some(), "zh tooltip present for CN users");
    }

}
