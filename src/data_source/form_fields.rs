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
    #[serde(default)]
    pub equals: String,
    /// Mirrors upstream predicates that accept several values
    /// (`scope === 'library' || scope === 'directory'`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub any_of: Vec<String>,
    /// True when at least one of the named fields is filled — upstream's
    /// `Boolean(credentials.aws_access_key_id || credentials.aws_secret_access_key)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub any_filled: Vec<String>,
    /// Value to assume when the field was never set, mirroring upstream's `?? 'account'`
    /// style defaults (`seafile-constant.tsx`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Mirrors upstream `!==` predicates (e.g. Jira `is_cloud !== false`).
    #[serde(default)]
    pub negate: bool,
}

impl ShowWhen {
    pub fn matches(&self, values: &HashMap<String, String>) -> bool {
        if !self.any_filled.is_empty() {
            let filled = self.any_filled.iter().any(|name| {
                values
                    .get(name)
                    .is_some_and(|value| !value.trim().is_empty())
            });
            return self.negate != filled;
        }
        let current = values
            .get(&self.field)
            .map(String::as_str)
            .or(self.default.as_deref());
        if !self.any_of.is_empty() {
            let listed = current.is_some_and(|value| self.any_of.iter().any(|one| one == value));
            return self.negate != listed;
        }
        let equals = current == Some(self.equals.as_str());
        self.negate != equals
    }
}

/// One `customValidate` rule from an upstream connector constant.
///
/// The connectors validate across fields, not just per input: the SeaFile account token
/// is required only for the `account` scope, and `library`/`directory` accept *either* a
/// token or a library token. Expressing those rules as data lets the dialog and the API
/// enforce the same thing with the same message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormValidation {
    /// All conditions must hold for the rule to apply.
    #[serde(default)]
    pub when: Vec<ShowWhen>,
    /// Alternatives, each AND-ed inside and OR-ed between: upstream writes rules like
    /// `authMode === 'access_key' || bucketType === 's3_compatible'`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when_any: Vec<Vec<ShowWhen>>,
    /// `nonempty` (the named field must be filled) or `any_of` (at least one of
    /// [`Self::targets`] must be filled).
    pub require: String,
    /// Field the message belongs to; for `any_of` the first target carries it.
    pub field: String,
    /// Fields `any_of` accepts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<String>,
    pub message_en: String,
    /// Upstream has no zh entry for most of these connector strings, so the English
    /// sentence is what the Chinese UI shows too (i18next `fallbackLng: en`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_zh: Option<String>,
}

impl FormValidation {
    fn applies(&self, values: &HashMap<String, String>) -> bool {
        let all = self.when.iter().all(|condition| condition.matches(values));
        let any = self.when_any.is_empty()
            || self
                .when_any
                .iter()
                .any(|group| group.iter().all(|condition| condition.matches(values)));
        all && any
    }

    /// The message for this rule, or `None` when the configuration satisfies it.
    pub fn check(&self, values: &HashMap<String, String>) -> Option<String> {
        if !self.applies(values) {
            return None;
        }
        let filled = |name: &str| {
            values
                .get(name)
                .is_some_and(|value| !value.trim().is_empty())
        };
        let satisfied = if self.require == "any_of" {
            self.targets.iter().any(|target| filled(target))
        } else {
            filled(&self.field)
        };
        (!satisfied).then(|| self.message_en.clone())
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
    /// Upstream `tooltip` i18n strings resolved from locales/en.ts / zh.ts; the dialogs
    /// show them in a hover popup (upstream renders a `Tooltip` around a `?` trigger).
    #[serde(default)]
    pub tooltip_en: Option<String>,
    #[serde(default)]
    pub tooltip_zh: Option<String>,
    /// Upstream `customValidate` rules for this source's form.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub validate: Vec<FormValidation>,
    /// `Custom` fields are informational panels, not inputs: upstream renders them from
    /// a `render()` function that switches on other values. The text travels as data
    /// (first line is the heading, the rest are bullets) so the browser draws the panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_en: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_zh: Option<String>,
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

/// Run a source's cross-field rules against a configuration.
///
/// Returns `(field, message)` pairs in schema order. The dialog shows the first one next
/// to its field; the create/update endpoints refuse the request so a client that never
/// ran the JavaScript cannot store a configuration the connector cannot use.
pub fn validate_config(source: &str, config: &serde_json::Value) -> Vec<(String, String)> {
    let mut values: HashMap<String, String> = HashMap::new();
    flatten_config("config", config, &mut values);
    fields_for(source)
        .into_iter()
        .flat_map(|field| field.validate)
        .filter_map(|rule| {
            rule.check(&values)
                .map(|message| (rule.field.clone(), message))
        })
        .collect()
}

/// Flatten a connector configuration into the dot-paths the rules speak
/// (`config.credentials.seafile_token`), trimming strings and stringifying the rest so a
/// number and a string compare the same way.
fn flatten_config(prefix: &str, node: &serde_json::Value, into: &mut HashMap<String, String>) {
    let Some(object) = node.as_object() else {
        return;
    };
    for (key, value) in object {
        let path = format!("{prefix}.{key}");
        match value {
            serde_json::Value::Object(_) => flatten_config(&path, value, into),
            serde_json::Value::Bool(flag) => {
                into.insert(path, flag.to_string());
            }
            serde_json::Value::Null => {
                into.insert(path, String::new());
            }
            serde_json::Value::String(text) => {
                into.insert(path, text.trim().to_string());
            }
            other => {
                into.insert(path, other.to_string());
            }
        }
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
                // `Custom` entries are informational panels drawn in place, not inputs:
                // upstream names them `filter.*` because they are not part of the config.
                if field.field_type == "Custom" {
                    assert!(
                        field.name.starts_with("filter."),
                        "{source} panel {} must use the filter.* prefix",
                        field.name
                    );
                    assert!(
                        field.panel_en.is_some(),
                        "{source} panel {} must carry its text",
                        field.name
                    );
                    continue;
                }
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
            find("jira", "config.include_comments")
                .default_value
                .as_deref(),
            Some("true")
        );
        assert_eq!(
            find("jira", "config.include_attachments")
                .default_value
                .as_deref(),
            Some("false")
        );
        assert_eq!(
            find("jira", "config.is_cloud").default_value.as_deref(),
            Some("true")
        );
    }

    /// Upstream's connector constants validate across fields, not only per input. These
    /// are the SeaFile rules, whose messages come from `setting.seafileValidation*`.
    #[test]
    fn seafile_cross_field_rules_follow_the_sync_scope() {
        use serde_json::json;
        let account_without_token = json!({"sync_scope": "account"});
        let failures = validate_config("SEAFILE", &account_without_token);
        assert_eq!(
            failures,
            vec![(
                "config.credentials.seafile_token".to_string(),
                "Account API Token is required for Entire Account scope".to_string()
            )]
        );
        // Same scope, token present: nothing to report.
        assert!(
            validate_config(
                "SEAFILE",
                &json!({"sync_scope": "account", "credentials": {"seafile_token": "t"}})
            )
            .is_empty()
        );
        // `library` accepts either token, but needs the library id.
        let library = json!({"sync_scope": "library"});
        let failures = validate_config("SEAFILE", &library);
        assert_eq!(failures.len(), 2, "{failures:?}");
        assert_eq!(
            failures[0].1,
            "Provide either an Account API Token or a Library Token"
        );
        assert_eq!(
            failures[1],
            (
                "config.repo_id".to_string(),
                "Library ID is required".to_string()
            )
        );
        // The library token alone satisfies the token rule.
        let failures = validate_config(
            "SEAFILE",
            &json!({"sync_scope": "library", "credentials": {"repo_token": "r"},
                    "repo_id": "7a9e"}),
        );
        assert!(failures.is_empty(), "{failures:?}");
        // `directory` additionally needs the path.
        let failures = validate_config(
            "SEAFILE",
            &json!({"sync_scope": "directory", "credentials": {"repo_token": "r"}, "repo_id": "x"}),
        );
        assert_eq!(
            failures,
            vec![(
                "config.sync_path".to_string(),
                "Directory Path is required".to_string()
            )]
        );
        // A configuration with no scope at all behaves like the upstream default
        // (`account`), which is what the segmented control starts on.
        let failures = validate_config("SEAFILE", &json!({}));
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].1.contains("Entire Account"));
    }

    /// S3, Confluence, Bitbucket and Jira carry rules of their own; each is checked here
    /// with the condition that turns it on and the value that satisfies it.
    #[test]
    fn the_companion_connectors_validate_their_conditional_fields() {
        use serde_json::json;
        // S3: an access key needs a region; s3_compatible needs both key halves.
        // s3_compatible with an access key: the key halves are required, the region is
        // not (upstream only demands it for `bucket_type === 's3'`).
        let failures = validate_config(
            "S3",
            &json!({"bucket_name": "b", "bucket_type": "s3_compatible",
                    "credentials": {"authentication_method": "access_key"}}),
        );
        let messages: Vec<&str> = failures
            .iter()
            .map(|(_, message)| message.as_str())
            .collect();
        assert!(
            !messages.contains(&"Region is required when using access key"),
            "the region rule must not fire for s3_compatible: {messages:?}"
        );
        // Plain S3 with a key present does need the region.
        let failures = validate_config(
            "S3",
            &json!({"bucket_name": "b", "bucket_type": "s3",
                    "credentials": {"authentication_method": "access_key",
                                    "aws_access_key_id": "AKIA"}}),
        );
        let messages: Vec<&str> = failures
            .iter()
            .map(|(_, message)| message.as_str())
            .collect();
        assert!(
            messages.contains(&"Region is required when using access key"),
            "{messages:?}"
        );
        // Only the secret half is missing here, and that is what the rule says.
        assert!(
            messages.contains(&"\"AWS Secret Access Key\" is required"),
            "{messages:?}"
        );
        assert!(
            !messages.contains(&"AWS Access Key ID is required"),
            "a filled access key id must satisfy its own rule: {messages:?}"
        );
        // The IAM role branch names the field it wants (upstream's message says
        // "AWS Secret Access Key", a copy-paste RayRAG does not reproduce).
        let failures = validate_config(
            "S3",
            &json!({"bucket_name": "b", "bucket_type": "s3",
                    "credentials": {"authentication_method": "iam_role"}}),
        );
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert_eq!(failures[0].0, "config.credentials.aws_role_arn");
        assert!(failures[0].1.contains("Role ARN"));
        // The default (plain s3, no credentials) needs nothing beyond the bucket.
        assert!(
            validate_config("S3", &json!({"bucket_name": "b", "bucket_type": "s3"})).is_empty()
        );

        // Confluence and Bitbucket follow their index mode.
        assert_eq!(
            validate_config("CONFLUENCE", &json!({"index_mode": "page"}))[0].1,
            "Page ID is required"
        );
        assert_eq!(
            validate_config("CONFLUENCE", &json!({"index_mode": "space"}))[0].1,
            "Space Key is required"
        );
        assert_eq!(
            validate_config("BITBUCKET", &json!({"index_mode": "repositories"}))[0].1,
            "Repository Slugs is required"
        );
        assert_eq!(
            validate_config("BITBUCKET", &json!({"index_mode": "projects"}))[0].1,
            "Projects is required"
        );

        // Jira: cloud uses email + API token, server uses username + password.
        let cloud = validate_config("JIRA", &json!({"is_cloud": true}));
        let messages: Vec<&str> = cloud.iter().map(|(_, message)| message.as_str()).collect();
        assert!(
            messages.contains(&"Jira User Email is required"),
            "{messages:?}"
        );
        assert!(
            messages.contains(&"Jira API Token is required"),
            "{messages:?}"
        );
        assert!(
            !messages.iter().any(|message| message.contains("Password")),
            "{messages:?}"
        );
        let server = validate_config("JIRA", &json!({"is_cloud": false}));
        let messages: Vec<&str> = server.iter().map(|(_, message)| message.as_str()).collect();
        assert!(
            messages.contains(&"Jira Username is required"),
            "{messages:?}"
        );
        assert!(
            messages.contains(&"Jira Password is required"),
            "{messages:?}"
        );
    }

    /// The informational panels upstream renders with `render()` travel as schema data:
    /// the page draws them, and they follow the same conditions as the fields around them.
    #[test]
    fn informational_panels_are_ordered_and_conditional() {
        let seafile = fields_for("SEAFILE");
        let names: Vec<&str> = seafile.iter().map(|field| field.name.as_str()).collect();
        let account_panel = names
            .iter()
            .position(|name| *name == "filter.account-tip")
            .unwrap();
        let token_panel = names
            .iter()
            .position(|name| *name == "filter.token-tip")
            .unwrap();
        let token = names
            .iter()
            .position(|name| *name == "config.credentials.seafile_token")
            .unwrap();
        let repo_token = names
            .iter()
            .position(|name| *name == "config.credentials.repo_token")
            .unwrap();
        assert!(
            account_panel < token,
            "the account panel precedes the account token"
        );
        assert!(
            token_panel < repo_token,
            "the token panel precedes the library token"
        );

        let panel = &seafile[account_panel];
        assert_eq!(panel.field_type, "Custom");
        assert!(panel.panel_en.is_some());
        assert!(panel.visible(&values(&[("config.sync_scope", "account")])));
        assert!(!panel.visible(&values(&[("config.sync_scope", "library")])));
        let panel = &seafile[token_panel];
        assert!(panel.visible(&values(&[("config.sync_scope", "library")])));
        assert!(panel.visible(&values(&[("config.sync_scope", "directory")])));
        assert!(!panel.visible(&values(&[("config.sync_scope", "account")])));
        assert!(
            panel
                .panel_en
                .as_deref()
                .is_some_and(|text| text.contains("Library Token"))
        );

        // The other three panels follow their own switches.
        let s3_panel = fields_for("S3")
            .into_iter()
            .find(|field| field.name == "filter.tip")
            .unwrap();
        assert!(s3_panel.visible(&values(&[
            ("config.credentials.authentication_method", "assume_role"),
            ("config.bucket_type", "s3"),
        ])));
        assert!(!s3_panel.visible(&values(&[
            ("config.credentials.authentication_method", "access_key"),
            ("config.bucket_type", "s3"),
        ])));
        let confluence_panel = fields_for("CONFLUENCE")
            .into_iter()
            .find(|field| field.name == "filter.tip")
            .unwrap();
        assert!(confluence_panel.visible(&values(&[("config.index_mode", "everything")])));
        let bitbucket_panel = fields_for("BITBUCKET")
            .into_iter()
            .find(|field| field.name == "filter.tip")
            .unwrap();
        assert!(bitbucket_panel.visible(&values(&[("config.index_mode", "workspace")])));
        assert!(
            bitbucket_panel
                .panel_en
                .as_deref()
                .is_some_and(|text| text.contains("all repositories"))
        );
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
            find("seafile", "config.include_shared")
                .default_value
                .as_deref(),
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
            field
                .tooltip_en
                .as_deref()
                .unwrap_or("")
                .contains("tenant ID"),
            "tooltip_en merged from upstream locale: {:?}",
            field.tooltip_en
        );
        assert!(
            field.tooltip_zh.is_some(),
            "zh tooltip present for CN users"
        );
    }
}
