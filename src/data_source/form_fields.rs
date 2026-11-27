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
    /// Chinese label for the schema-driven detail form; omitted when the
    /// upstream field has no localized label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_zh: Option<String>,
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
    /// Upstream `allowCustomValue`: the select accepts a value outside its list (the S3
    /// region selector, so a region newer than the embedded AWS list can still be typed).
    #[serde(default)]
    pub allow_custom: bool,
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

/// The source card manifest serialized for the browser (name/description/
/// icon per upstream `dataSourceInfo`).
pub fn source_info_for_ui() -> serde_json::Value {
    serde_json::to_value(source_info()).expect("source info manifest is serializable")
}




