//! Jin10 (金十数据) finance connector — mirrors RAGFlow `agent/tools/jin10.py`.
//! Mainland-China direct access; requires a Jin10 open-data secret key
//! (`JIN10_SECRET_KEY`, default "xxx" like RAGFlow). Four modes: flash
//! (快讯), calendar (财经日历), symbols (行情/品种), news (新闻).
//!
//! RAGFlow renders rows with pandas `to_markdown()`; RayRAG returns the
//! RAGFlow tool rows (title/link/snippet) plus a `json` payload so the
//! domestic-search output path can be shared.

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use reqwest::redirect::Policy as RedirectPolicy;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const JIN10_ENDPOINT: &str = "https://open-data-api.jin10.com/data-api";
const MAX_JIN10_RESPONSE_BODY: usize = 16 << 20;

/// Jin10 request type: flash / calendar / symbols / news.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jin10Type {
    Flash,
    Calendar,
    Symbols,
    News,
}

impl Jin10Type {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Flash => "flash",
            Self::Calendar => "calendar",
            Self::Symbols => "symbols",
            Self::News => "news",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jin10Request {
    pub r#type: Jin10Type,
    /// flash category: 1..=5 (RAGFlow validates 1-5).
    pub flash_type: u8,
    /// calendar category: cj(财经)/qh(期货)/hk(港股)/us(美股).
    pub calendar_type: String,
    /// calendar datatype: data/event/holiday.
    pub calendar_datatype: String,
    /// symbols type: GOODS/FOREX/FUTURE/CRYPTO.
    pub symbols_type: String,
    /// symbols datatype: symbols/quotes.
    pub symbols_datatype: String,
    /// keyword filter (flash/news).
    pub contain: String,
    /// exclude filter (flash/news).
    pub filter: String,
    pub top_n: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Jin10Result {
    /// A single-row item carrying the rendered content (RAGFlow's DataFrame
    /// `content` column); for news/calendar/symbols it is a markdown-ish
    /// rendering of the records.
    pub content: String,
}

#[async_trait]
pub trait Jin10Provider: Send + Sync {
    async fn search(&self, secret_key: &str, request: &Jin10Request) -> Result<Vec<Jin10Result>>;
}

#[derive(Debug, Clone)]
pub struct Jin10Client {
    client: reqwest::Client,
    endpoint: reqwest::Url,
}

impl Default for Jin10Client {
    fn default() -> Self {
        Self::new_with_endpoints(JIN10_ENDPOINT).expect("fixed Jin10 endpoint is valid")
    }
}

impl Jin10Client {
    pub(crate) fn new_with_endpoints(endpoint: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(RedirectPolicy::none())
            .timeout(crate::common::cmd_timeout::duration())
            .build()
            .context("could not build Jin10 HTTP client")?;
        Ok(Self {
            client,
            endpoint: endpoint.parse().context("invalid Jin10 endpoint")?,
        })
    }

    /// Mirror RAGFlow's flash item rendering: `{"content": data[i].data.content}`.
    fn render_flash(data: &Value) -> Vec<Jin10Result> {
        data.as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let content = item
                            .pointer("/data/content")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if content.is_empty() {
                            None
                        } else {
                            Some(Jin10Result { content })
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Render arbitrary record rows as one content item per record (approximates
    /// RAGFlow's pandas `to_markdown()` per row without the table header).
    fn render_rows(data: &Value) -> Vec<Jin10Result> {
        data.as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let row = item.as_object()?;
                        if row.is_empty() {
                            return None;
                        }
                        let content = row
                            .iter()
                            .map(|(k, v)| {
                                let value = match v {
                                    Value::String(s) => s.clone(),
                                    other => other.to_string(),
                                };
                                format!("{k}: {value}")
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        Some(Jin10Result { content })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Symbols 行情 quotes: RAGFlow renames short keys to long Chinese names.
    fn render_quotes(data: &Value) -> Vec<Jin10Result> {
        const RENAMES: &[(&str, &str)] = &[
            ("a", "Selling Price"),
            ("b", "Buying Price"),
            ("c", "Commodity Code"),
            ("e", "Stock Exchange"),
            ("h", "Highest Price"),
            ("hc", "Yesterday's Closing Price"),
            ("l", "Lowest Price"),
            ("o", "Opening Price"),
            ("p", "Latest Price"),
            ("t", "Market Quote Time"),
        ];
        data.as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let obj = item.as_object()?;
                        if obj.is_empty() {
                            return None;
                        }
                        let mut lines = Vec::new();
                        for (k, v) in obj {
                            let key = RENAMES
                                .iter()
                                .find(|(short, _)| short == k)
                                .map(|(_, long)| *long)
                                .unwrap_or(k.as_str());
                            let value = match v {
                                Value::String(s) => s.clone(),
                                other => other.to_string(),
                            };
                            lines.push(format!("{key}: {value}"));
                        }
                        Some(Jin10Result {
                            content: lines.join("\n"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[async_trait]
impl Jin10Provider for Jin10Client {
    async fn search(&self, secret_key: &str, request: &Jin10Request) -> Result<Vec<Jin10Result>> {
        if secret_key.trim().is_empty() {
            bail!("jin10: secret key is required");
        }
        let mut url = self.endpoint.clone();
        match request.r#type {
            Jin10Type::Flash => {
                let category = request.flash_type.clamp(1, 5);
                url = format!("{}/flash?category={}", url, category)
                    .parse()
                    .context("invalid Jin10 flash URL")?;
            }
            Jin10Type::Calendar => {
                let category = if request.calendar_type.is_empty() {
                    "cj".to_string()
                } else {
                    request.calendar_type.clone()
                };
                let datatype = if request.calendar_datatype.is_empty() {
                    "data".to_string()
                } else {
                    request.calendar_datatype.clone()
                };
                url = format!("{}/calendar/{}?category={}", url, datatype, category)
                    .parse()
                    .context("invalid Jin10 calendar URL")?;
            }
            Jin10Type::Symbols => {
                let stype = if request.symbols_type.is_empty() {
                    "GOODS".to_string()
                } else {
                    request.symbols_type.clone()
                };
                let datatype = if request.symbols_datatype.is_empty() {
                    "symbols".to_string()
                } else {
                    request.symbols_datatype.clone()
                };
                url = format!("{}/{}?type={}", url, datatype, stype)
                    .parse()
                    .context("invalid Jin10 symbols URL")?;
            }
            Jin10Type::News => {
                url = format!("{}/news", url)
                    .parse()
                    .context("invalid Jin10 news URL")?;
            }
        }
        let mut body = Map::new();
        if request.r#type == Jin10Type::Flash || request.r#type == Jin10Type::News {
            body.insert("contain".into(), Value::String(request.contain.clone()));
            body.insert("filter".into(), Value::String(request.filter.clone()));
        }
        let response = self
            .client
            .get(url)
            .header("secret-key", secret_key.trim())
            .header("Content-Type", "application/json")
            .json(&Value::Object(body))
            .send()
            .await
            .context("Jin10 request failed")?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .context("could not read Jin10 response body")?;
        if body.len() > MAX_JIN10_RESPONSE_BODY {
            bail!(
                "Jin10 response body exceeds {} bytes",
                MAX_JIN10_RESPONSE_BODY
            );
        }
        let envelope: Value = serde_json::from_slice(&body)
            .with_context(|| format!("Jin10 response (HTTP {status}) was not valid JSON"))?;
        if !status.is_success() {
            let message = envelope
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("upstream error");
            bail!("jin10: upstream returned {}: {message}", status.as_u16());
        }
        let data = envelope
            .get("data")
            .cloned()
            .ok_or_else(|| anyhow!("jin10: missing data in response"))?;
        let mut results = match request.r#type {
            Jin10Type::Flash => Self::render_flash(&data),
            Jin10Type::Symbols if request.symbols_datatype == "quotes" => {
                Self::render_quotes(&data)
            }
            Jin10Type::Calendar | Jin10Type::Symbols | Jin10Type::News => Self::render_rows(&data),
        };
        if request.top_n > 0 && results.len() > request.top_n {
            results.truncate(request.top_n);
        }
        Ok(results)
    }
}

/// Convert Jin10 results into RAGFlow tool rows.
pub fn jin10_results_to_tool_rows(results: &[Jin10Result]) -> Vec<Value> {
    results
        .iter()
        .map(|result| {
            let mut row = Map::new();
            row.insert("title".into(), Value::String("金十数据".into()));
            row.insert("link".into(), Value::String(String::new()));
            row.insert("snippet".into(), Value::String(result.content.clone()));
            Value::Object(row)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_ragflow_shaped() {
        let rows = jin10_results_to_tool_rows(&[Jin10Result {
            content: "c".into(),
        }]);
        assert_eq!(rows[0]["title"], "金十数据");
        assert_eq!(rows[0]["snippet"], "c");
    }

    #[test]
    fn flash_render_extracts_data_content() {
        let data = serde_json::json!({"data": [
            {"data": {"content": "快讯一"}},
            {"data": {"content": "快讯二"}},
            {"data": {"content": ""}}
        ]});
        let results = Jin10Client::render_flash(&data["data"]);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].content, "快讯一");
        assert_eq!(results[1].content, "快讯二");
    }

    #[test]
    fn rows_render_key_value_lines() {
        let data = serde_json::json!({"data": [
            {"title": "标题", "time": "2026-08-01"},
            {"title": "标题2"}
        ]});
        let results = Jin10Client::render_rows(&data["data"]);
        assert_eq!(results.len(), 2);
        assert!(results[0].content.contains("标题"));
        assert!(results[0].content.contains("2026-08-01"));
    }

    #[test]
    fn quotes_render_long_names() {
        let data = serde_json::json!({"data": [
            {"a": "100.1", "c": "BTCUSD", "p": "100.5"}
        ]});
        let results = Jin10Client::render_quotes(&data["data"]);
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Selling Price: 100.1"));
        assert!(results[0].content.contains("Commodity Code: BTCUSD"));
        assert!(results[0].content.contains("Latest Price: 100.5"));
        assert!(!results[0].content.contains("\na: "));
        assert!(!results[0].content.contains("\nc: "));
    }

    #[tokio::test]
    async fn missing_key_is_rejected() {
        let client = Jin10Client::default();
        let error = client
            .search(
                "",
                &Jin10Request {
                    r#type: Jin10Type::Flash,
                    flash_type: 1,
                    calendar_type: "cj".into(),
                    calendar_datatype: "data".into(),
                    symbols_type: "GOODS".into(),
                    symbols_datatype: "symbols".into(),
                    contain: String::new(),
                    filter: String::new(),
                    top_n: 5,
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("secret key"));
    }

    #[tokio::test]
    #[ignore = "requires a live Jin10 secret key (JIN10_SECRET_KEY)"]
    async fn live_jin10_flash() {
        let key = std::env::var("JIN10_SECRET_KEY").expect("JIN10_SECRET_KEY must be set");
        let results = Jin10Client::default()
            .search(
                &key,
                &Jin10Request {
                    r#type: Jin10Type::Flash,
                    flash_type: 1,
                    calendar_type: "cj".into(),
                    calendar_datatype: "data".into(),
                    symbols_type: "GOODS".into(),
                    symbols_datatype: "symbols".into(),
                    contain: String::new(),
                    filter: String::new(),
                    top_n: 5,
                },
            )
            .await
            .unwrap();
        assert!(!results.is_empty());
    }
}
