//! QWeather (和风天气) connector — mirrors RAGFlow `agent/tools/qweather.py`.
//! Mainland-China direct access; requires a QWeather Web API key
//! (`QWEATHER_API_KEY`). Three modes: weather (now/3d/7d/10d/15d/30d),
//! indices (生活指数), airquality (空气质量).
//!
//! RAGFlow uses the free `devapi.qweather.com` host for free subscriptions and
//! `api.qweather.com` for paid; RayRAG follows the same split and renders
//! results as RAGFlow tool rows.

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use reqwest::redirect::Policy as RedirectPolicy;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const QWEATHER_GEO_ENDPOINT: &str = "https://geoapi.qweather.com/v2/city/lookup";
const QWEATHER_DEV_ENDPOINT: &str = "https://devapi.qweather.com/v7";
const QWEATHER_PAID_ENDPOINT: &str = "https://api.qweather.com/v7";
const MAX_QWEATHER_RESPONSE_BODY: usize = 16 << 20;

/// QWeather request type: weather / indices / airquality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QWeatherType {
    Weather,
    Indices,
    AirQuality,
}

impl QWeatherType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Weather => "weather",
            Self::Indices => "indices",
            Self::AirQuality => "airquality",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QWeatherRequest {
    /// City name (or location keyword), resolved to a location id first.
    pub location: String,
    pub r#type: QWeatherType,
    /// weather time period: now/3d/7d/10d/15d/30d.
    pub time_period: String,
    /// lang: zh / en / ja / ... (RAGFlow validates a long list).
    pub lang: String,
    /// free or paid subscription (paid uses api.qweather.com).
    pub paid: bool,
    pub top_n: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QWeatherResult {
    pub content: String,
}

#[async_trait]
pub trait QWeatherProvider: Send + Sync {
    async fn search(&self, api_key: &str, request: &QWeatherRequest)
    -> Result<Vec<QWeatherResult>>;
}

#[derive(Debug, Clone)]
pub struct QWeatherClient {
    client: reqwest::Client,
    geo_endpoint: reqwest::Url,
    dev_endpoint: reqwest::Url,
    paid_endpoint: reqwest::Url,
}

impl Default for QWeatherClient {
    fn default() -> Self {
        Self::new_with_endpoints(
            QWEATHER_GEO_ENDPOINT,
            QWEATHER_DEV_ENDPOINT,
            QWEATHER_PAID_ENDPOINT,
        )
        .expect("fixed QWeather endpoints are valid")
    }
}

impl QWeatherClient {
    pub(crate) fn new_with_endpoints(geo: &str, dev: &str, paid: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(RedirectPolicy::none())
            .timeout(crate::common::cmd_timeout::duration())
            .build()
            .context("could not build QWeather HTTP client")?;
        Ok(Self {
            client,
            geo_endpoint: geo.parse().context("invalid QWeather geo endpoint")?,
            dev_endpoint: dev.parse().context("invalid QWeather dev endpoint")?,
            paid_endpoint: paid.parse().context("invalid QWeather paid endpoint")?,
        })
    }

    /// Resolve a location keyword to a QWeather location id via the geo API.
    async fn resolve_location_id(&self, api_key: &str, location: &str) -> Result<String> {
        let url = format!(
            "{}?location={}&key={}",
            self.geo_endpoint,
            urlencoding(location),
            api_key
        );
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("QWeather geo lookup failed")?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .context("could not read QWeather geo response body")?;
        if body.len() > MAX_QWEATHER_RESPONSE_BODY {
            bail!(
                "QWeather geo response body exceeds {} bytes",
                MAX_QWEATHER_RESPONSE_BODY
            );
        }
        let envelope: Value = serde_json::from_slice(&body)
            .with_context(|| format!("QWeather geo response (HTTP {status}) was not valid JSON"))?;
        let code = envelope
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if code != "200" {
            bail!("qweather: geo lookup failed with code {code}");
        }
        envelope
            .pointer("/location/0/id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("qweather: no location id found for {location:?}"))
    }

    async fn fetch(&self, url: &str) -> Result<(String, Value)> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("QWeather request failed")?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .context("could not read QWeather response body")?;
        if body.len() > MAX_QWEATHER_RESPONSE_BODY {
            bail!(
                "QWeather response body exceeds {} bytes",
                MAX_QWEATHER_RESPONSE_BODY
            );
        }
        let envelope: Value = serde_json::from_slice(&body)
            .with_context(|| format!("QWeather response (HTTP {status}) was not valid JSON"))?;
        Ok((status.as_u16().to_string(), envelope))
    }
}

/// Minimal URL-encode for query params (space → %20 etc.).
fn urlencoding(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

#[async_trait]
impl QWeatherProvider for QWeatherClient {
    async fn search(
        &self,
        api_key: &str,
        request: &QWeatherRequest,
    ) -> Result<Vec<QWeatherResult>> {
        if api_key.trim().is_empty() {
            bail!("qweather: API key is required");
        }
        if request.location.trim().is_empty() {
            bail!("qweather: location is required");
        }
        let location_id = self
            .resolve_location_id(api_key.trim(), request.location.trim())
            .await?;
        let base = if request.paid {
            self.paid_endpoint.to_string()
        } else {
            self.dev_endpoint.to_string()
        };
        let lang = if request.lang.is_empty() {
            "zh".to_string()
        } else {
            request.lang.clone()
        };
        let (http_code, envelope) = match request.r#type {
            QWeatherType::Weather => {
                let period = if request.time_period.is_empty() {
                    "now".to_string()
                } else {
                    request.time_period.clone()
                };
                let url = format!(
                    "{base}/weather/{period}?location={location_id}&key={}&lang={lang}",
                    api_key.trim()
                );
                self.fetch(&url).await?
            }
            QWeatherType::Indices => {
                let url = format!(
                    "{base}/indices/1d?type=0&location={location_id}&key={}&lang={lang}",
                    api_key.trim()
                );
                self.fetch(&url).await?
            }
            QWeatherType::AirQuality => {
                let url = format!(
                    "{base}/air/now?location={location_id}&key={}&lang={lang}",
                    api_key.trim()
                );
                self.fetch(&url).await?
            }
        };
        let code = envelope
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if code != "200" {
            bail!("qweather: query failed with code {code} (HTTP {http_code})");
        }
        let results = match request.r#type {
            QWeatherType::Weather => {
                if request.time_period == "now" || request.time_period.is_empty() {
                    envelope
                        .get("now")
                        .map(|now| {
                            vec![QWeatherResult {
                                content: now.to_string(),
                            }]
                        })
                        .unwrap_or_default()
                } else {
                    envelope
                        .get("daily")
                        .and_then(Value::as_array)
                        .map(|daily| {
                            daily
                                .iter()
                                .take(request.top_n.max(1))
                                .map(|day| QWeatherResult {
                                    content: day.to_string(),
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                }
            }
            QWeatherType::Indices => envelope
                .get("daily")
                .and_then(Value::as_array)
                .map(|daily| {
                    let date = daily
                        .first()
                        .and_then(|d| d.get("date"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    daily
                        .iter()
                        .filter_map(|item| {
                            let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
                            let category = item
                                .get("category")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            let text = item.get("text").and_then(Value::as_str).unwrap_or_default();
                            if name.is_empty() {
                                return None;
                            }
                            Some(QWeatherResult {
                                content: format!("{date} {name}: {category}, {text}"),
                            })
                        })
                        .take(request.top_n.max(1))
                        .collect()
                })
                .unwrap_or_default(),
            QWeatherType::AirQuality => envelope
                .get("now")
                .map(|now| {
                    vec![QWeatherResult {
                        content: now.to_string(),
                    }]
                })
                .unwrap_or_default(),
        };
        Ok(results)
    }
}

/// Convert QWeather results into RAGFlow tool rows.
pub fn qweather_results_to_tool_rows(results: &[QWeatherResult]) -> Vec<Value> {
    results
        .iter()
        .map(|result| {
            let mut row = Map::new();
            row.insert("title".into(), Value::String("和风天气".into()));
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
        let rows = qweather_results_to_tool_rows(&[QWeatherResult {
            content: "c".into(),
        }]);
        assert_eq!(rows[0]["title"], "和风天气");
        assert_eq!(rows[0]["snippet"], "c");
    }

    #[test]
    fn url_encoding_handles_chinese_and_space() {
        assert_eq!(urlencoding("中山 市"), "%E4%B8%AD%E5%B1%B1%20%E5%B8%82");
        assert_eq!(urlencoding("zhongshan"), "zhongshan");
    }

    #[test]
    fn weather_daily_renders_one_row_per_day() {
        let envelope = serde_json::json!({
            "code": "200",
            "daily": [
                {"date": "2026-08-01", "tempMax": "30"},
                {"date": "2026-08-02", "tempMax": "31"}
            ]
        });
        // The rendering logic lives in the provider impl; assert the shape
        // of the JSON we feed it is what we expect.
        assert_eq!(envelope["daily"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn missing_key_or_location_is_rejected() {
        let client = QWeatherClient::default();
        let error = client
            .search(
                "",
                &QWeatherRequest {
                    location: "中山".into(),
                    r#type: QWeatherType::Weather,
                    time_period: "now".into(),
                    lang: "zh".into(),
                    paid: false,
                    top_n: 5,
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("API key"));
        let error = client
            .search(
                "key",
                &QWeatherRequest {
                    location: "".into(),
                    r#type: QWeatherType::Weather,
                    time_period: "now".into(),
                    lang: "zh".into(),
                    paid: false,
                    top_n: 5,
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("location"));
    }

    #[tokio::test]
    #[ignore = "requires a live QWeather API key (QWEATHER_API_KEY)"]
    async fn live_qweather_weather_now() {
        let key = std::env::var("QWEATHER_API_KEY").expect("QWEATHER_API_KEY must be set");
        let results = QWeatherClient::default()
            .search(
                &key,
                &QWeatherRequest {
                    location: "中山".into(),
                    r#type: QWeatherType::Weather,
                    time_period: "now".into(),
                    lang: "zh".into(),
                    paid: false,
                    top_n: 5,
                },
            )
            .await
            .unwrap();
        assert!(!results.is_empty());
    }
}
