//! Fixed-domain Yahoo Finance protocols for RAGFlow-compatible tools.
//!
//! RAGFlow's Python Canvas component delegates to `yfinance==0.2.65` and
//! renders several datasets as Markdown. The independent Go Agent tool has a
//! much smaller quote-snapshot contract. This module keeps those two paths
//! separate while preventing Canvas data from selecting an outbound host.

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{ACCEPT, CONTENT_TYPE, COOKIE, SET_COOKIE, USER_AGENT};
use reqwest::redirect::Policy as RedirectPolicy;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use time::{OffsetDateTime, UtcOffset};
use tokio::sync::Mutex;

const MAX_YAHOO_RESPONSE_BODY: usize = 16 << 20;
const PYTHON_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36";
const GO_USER_AGENT: &str = "Mozilla/5.0 (compatible; ragflow/1.0)";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YahooFinanceRequest {
    pub stock_code: String,
    pub info: bool,
    pub history: bool,
    pub count: bool,
    pub financials: bool,
    pub income_stmt: bool,
    pub balance_sheet: bool,
    pub cash_flow_statement: bool,
    pub news: bool,
}

impl YahooFinanceRequest {
    pub fn python_defaults(stock_code: impl Into<String>) -> Self {
        Self {
            stock_code: stock_code.into(),
            info: true,
            history: false,
            count: false,
            financials: false,
            income_stmt: false,
            balance_sheet: false,
            cash_flow_statement: false,
            news: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct YahooFinanceQuote {
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub symbol: String,
    #[serde(
        rename = "regularMarketPrice",
        default,
        deserialize_with = "deserialize_nullable_f64"
    )]
    pub regular_market_price: f64,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub currency: String,
    #[serde(
        rename = "regularMarketChangePercent",
        default,
        deserialize_with = "deserialize_nullable_f64"
    )]
    pub regular_market_change_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct YahooFinanceGoEnvelope {
    pub results: Vec<YahooFinanceQuote>,
}

#[async_trait]
pub trait YahooFinanceProvider: Send + Sync {
    async fn report(&self, request: &YahooFinanceRequest) -> Result<String>;
}

#[derive(Debug, Clone)]
struct YahooEndpoints {
    fc: reqwest::Url,
    crumb: reqwest::Url,
    quote_summary: reqwest::Url,
    quote: reqwest::Url,
    chart: reqwest::Url,
    timeseries_query1: reqwest::Url,
    timeseries_query2: reqwest::Url,
    news: reqwest::Url,
}

impl YahooEndpoints {
    fn production() -> Result<Self> {
        Ok(Self {
            fc: "https://fc.yahoo.com/".parse()?,
            crumb: "https://query1.finance.yahoo.com/v1/test/getcrumb".parse()?,
            quote_summary: "https://query2.finance.yahoo.com/v10/finance/quoteSummary/".parse()?,
            quote: "https://query1.finance.yahoo.com/v7/finance/quote".parse()?,
            chart: "https://query2.finance.yahoo.com/v8/finance/chart/".parse()?,
            timeseries_query1:
                "https://query1.finance.yahoo.com/ws/fundamentals-timeseries/v1/finance/timeseries/"
                    .parse()?,
            timeseries_query2:
                "https://query2.finance.yahoo.com/ws/fundamentals-timeseries/v1/finance/timeseries/"
                    .parse()?,
            news: "https://finance.yahoo.com/xhr/ncp?queryRef=latestNews&serviceKey=ncp_fin"
                .parse()?,
        })
    }

    #[cfg(test)]
    fn local(base: &str) -> Result<Self> {
        let base = base.trim_end_matches('/');
        Ok(Self {
            fc: format!("{base}/fc").parse()?,
            crumb: format!("{base}/crumb").parse()?,
            quote_summary: format!("{base}/quoteSummary/").parse()?,
            quote: format!("{base}/quote").parse()?,
            chart: format!("{base}/chart/").parse()?,
            timeseries_query1: format!("{base}/timeseries1/").parse()?,
            timeseries_query2: format!("{base}/timeseries2/").parse()?,
            news: format!("{base}/news?queryRef=latestNews&serviceKey=ncp_fin").parse()?,
        })
    }
}

#[derive(Debug, Clone)]
struct YahooSession {
    cookie: String,
    crumb: String,
}

#[derive(Debug, Clone)]
pub struct YahooFinanceClient {
    client: reqwest::Client,
    endpoints: YahooEndpoints,
    session: Arc<Mutex<Option<YahooSession>>>,
}

impl Default for YahooFinanceClient {
    fn default() -> Self {
        Self::new(YahooEndpoints::production().expect("fixed Yahoo Finance endpoints are valid"))
            .expect("fixed Yahoo Finance HTTP client configuration is valid")
    }
}

impl YahooFinanceClient {
    fn new(endpoints: YahooEndpoints) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(RedirectPolicy::none())
                .timeout(crate::common::cmd_timeout::duration())
                .build()
                .context("could not build Yahoo Finance HTTP client")?,
            endpoints,
            session: Arc::new(Mutex::new(None)),
        })
    }

    #[cfg(test)]
    pub(crate) fn new_with_base(base: &str) -> Result<Self> {
        Self::new(YahooEndpoints::local(base)?)
    }

    async fn session(&self) -> Result<YahooSession> {
        let mut cached = self.session.lock().await;
        if let Some(session) = cached.as_ref() {
            return Ok(session.clone());
        }
        let response = self
            .client
            .get(self.endpoints.fc.clone())
            .header(USER_AGENT, PYTHON_USER_AGENT)
            .send()
            .await
            .context("Yahoo Finance cookie bootstrap failed")?;
        let cookies = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|value| value.split(';').next())
            .filter(|value| value.contains('='))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let _ = read_bounded_body(response).await?;
        if cookies.is_empty() {
            bail!("Yahoo Finance cookie bootstrap returned no cookies");
        }
        let cookie = cookies.join("; ");
        let response = self
            .client
            .get(self.endpoints.crumb.clone())
            .header(USER_AGENT, PYTHON_USER_AGENT)
            .header(COOKIE, &cookie)
            .send()
            .await
            .context("Yahoo Finance crumb request failed")?;
        let status = response.status();
        let body = read_bounded_body(response).await?;
        if !status.is_success() {
            bail!("Yahoo Finance crumb endpoint returned HTTP {status}");
        }
        let crumb = String::from_utf8(body)
            .context("Yahoo Finance crumb is not UTF-8")?
            .trim()
            .to_owned();
        if crumb.is_empty() {
            bail!("Yahoo Finance crumb endpoint returned an empty crumb");
        }
        let session = YahooSession { cookie, crumb };
        *cached = Some(session.clone());
        Ok(session)
    }

    async fn get_json(
        &self,
        mut url: reqwest::Url,
        params: &[(&str, String)],
        operation: &str,
    ) -> Result<Value> {
        let session = self.session().await?;
        {
            let mut query = url.query_pairs_mut();
            for (key, value) in params {
                query.append_pair(key, value);
            }
            query.append_pair("crumb", &session.crumb);
        }
        let response = self
            .client
            .get(url)
            .header(USER_AGENT, PYTHON_USER_AGENT)
            .header(ACCEPT, "application/json")
            .header(COOKIE, &session.cookie)
            .send()
            .await
            .with_context(|| format!("Yahoo Finance {operation} request failed"))?;
        decode_json_response(response, operation).await
    }

    async fn post_json(
        &self,
        mut url: reqwest::Url,
        payload: &Value,
        operation: &str,
    ) -> Result<Value> {
        let session = self.session().await?;
        url.query_pairs_mut().append_pair("crumb", &session.crumb);
        let response = self
            .client
            .post(url)
            .header(USER_AGENT, PYTHON_USER_AGENT)
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/json")
            .header(COOKIE, &session.cookie)
            .json(payload)
            .send()
            .await
            .with_context(|| format!("Yahoo Finance {operation} request failed"))?;
        decode_json_response(response, operation).await
    }

    fn symbol_url(base: &reqwest::Url, symbol: &str) -> Result<reqwest::Url> {
        let mut url = base.clone();
        url.path_segments_mut()
            .map_err(|_| anyhow!("Yahoo Finance endpoint cannot contain a symbol path"))?
            .push(symbol);
        Ok(url)
    }

    async fn information(&self, symbol: &str) -> Result<Value> {
        let summary = self
            .get_json(
                Self::symbol_url(&self.endpoints.quote_summary, symbol)?,
                &[
                    (
                        "modules",
                        "financialData,quoteType,defaultKeyStatistics,assetProfile,summaryDetail"
                            .into(),
                    ),
                    ("corsDomain", "finance.yahoo.com".into()),
                    ("formatted", "false".into()),
                    ("symbol", symbol.into()),
                ],
                "information summary",
            )
            .await?;
        let quote = self
            .get_json(
                self.endpoints.quote.clone(),
                &[("symbols", symbol.into()), ("formatted", "false".into())],
                "information quote",
            )
            .await?;
        let mut merged = Map::new();
        if let Some(object) = summary
            .pointer("/quoteSummary/result/0")
            .and_then(Value::as_object)
        {
            merge_flattened(&mut merged, object);
        }
        if let Some(object) = quote
            .pointer("/quoteResponse/result/0")
            .and_then(Value::as_object)
        {
            merge_flattened(&mut merged, object);
        }
        merged.insert("symbol".into(), Value::String(symbol.into()));

        let now = unix_seconds()?;
        let trailing = self
            .get_json(
                Self::symbol_url(&self.endpoints.timeseries_query1, symbol)?,
                &[
                    ("symbol", symbol.into()),
                    ("type", "trailingPegRatio".into()),
                    ("period1", now.saturating_sub(365 / 2 * 86_400).to_string()),
                    ("period2", now.saturating_add(86_400).to_string()),
                ],
                "trailing PEG ratio",
            )
            .await?;
        if trailing
            .pointer("/timeseries/error")
            .is_some_and(|error| !error.is_null())
        {
            bail!("Yahoo Finance trailing PEG ratio response contains an error");
        }
        let value = trailing
            .pointer("/timeseries/result/0/trailingPegRatio")
            .and_then(Value::as_array)
            .and_then(|values| values.last())
            .and_then(|value| value.pointer("/reportedValue/raw"))
            .cloned()
            .unwrap_or(Value::Null);
        merged.insert("trailingPegRatio".into(), value);
        Ok(Value::Object(merged))
    }

    async fn history(&self, symbol: &str) -> Result<String> {
        // yfinance first resolves the exchange timezone with a one-day chart.
        let _ = self
            .get_json(
                Self::symbol_url(&self.endpoints.chart, symbol)?,
                &[("range", "1d".into()), ("interval", "1d".into())],
                "history timezone",
            )
            .await?;
        let data = self
            .get_json(
                Self::symbol_url(&self.endpoints.chart, symbol)?,
                &[
                    ("range", "1mo".into()),
                    ("interval", "1d".into()),
                    ("includePrePost", "false".into()),
                    ("events", "div,splits,capitalGains".into()),
                ],
                "history",
            )
            .await?;
        history_markdown(&data)
    }

    async fn calendar(&self, symbol: &str) -> Result<String> {
        let data = self
            .get_json(
                Self::symbol_url(&self.endpoints.quote_summary, symbol)?,
                &[
                    ("modules", "calendarEvents".into()),
                    ("corsDomain", "finance.yahoo.com".into()),
                    ("formatted", "false".into()),
                    ("symbol", symbol.into()),
                ],
                "calendar",
            )
            .await?;
        calendar_markdown(&data)
    }

    async fn fundamentals(&self, symbol: &str, quarterly: bool, keys: &[&str]) -> Result<String> {
        let prefix = if quarterly { "quarterly" } else { "annual" };
        let types = keys
            .iter()
            .map(|key| format!("{prefix}{key}"))
            .collect::<Vec<_>>()
            .join(",");
        let data = self
            .get_json(
                Self::symbol_url(&self.endpoints.timeseries_query2, symbol)?,
                &[
                    ("symbol", symbol.into()),
                    ("type", types),
                    ("period1", "1483142400".into()),
                    (
                        "period2",
                        unix_seconds()?.saturating_add(86_400).to_string(),
                    ),
                ],
                "fundamentals",
            )
            .await?;
        fundamentals_markdown(&data, prefix, keys)
    }

    async fn news(&self, symbol: &str) -> Result<String> {
        let data = self
            .post_json(
                self.endpoints.news.clone(),
                &json!({"serviceConfig": {"snippetCount": 10, "s": [symbol]}}),
                "news",
            )
            .await?;
        let rows = data
            .pointer("/data/tickerStream/stream")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("Yahoo Finance news response is missing ticker stream"))?
            .iter()
            .filter(|article| article.get("ad").is_none_or(is_empty_json_value))
            .cloned()
            .collect::<Vec<_>>();
        Ok(dataframe_markdown(&rows))
    }

    /// Execute the independent Go tool's quote snapshot contract.
    pub async fn quote_go(
        &self,
        symbols: &[String],
        fields: &[String],
    ) -> Result<YahooFinanceGoEnvelope> {
        if symbols.is_empty() {
            bail!("yahoo_finance: symbols is required and must be non-empty");
        }
        let mut url = self.endpoints.quote.clone();
        {
            let mut query = url.query_pairs_mut();
            if !fields.is_empty() {
                query.append_pair("fields", &fields.join(","));
            }
            query.append_pair("symbols", &symbols.join(","));
        }
        let response = self
            .client
            .get(url)
            .header(USER_AGENT, GO_USER_AGENT)
            .header(ACCEPT, "application/json")
            .send()
            .await
            .context("yahoo_finance: request failed")?;
        let data = decode_json_response(response, "Go quote").await?;
        let results = data
            .pointer("/quoteResponse/result")
            .cloned()
            .ok_or_else(|| anyhow!("yahoo_finance: response is missing quoteResponse.result"))?;
        Ok(YahooFinanceGoEnvelope {
            results: serde_json::from_value(results).context("yahoo_finance: decode response")?,
        })
    }
}

#[async_trait]
impl YahooFinanceProvider for YahooFinanceClient {
    async fn report(&self, request: &YahooFinanceRequest) -> Result<String> {
        if request.stock_code.is_empty() {
            return Ok(String::new());
        }
        let mut sections = Vec::new();
        if request.info {
            let information = self.information(&request.stock_code).await?;
            sections.push(format!(
                "# Information:\n{}\n",
                series_markdown(&information)?
            ));
        }
        if request.history {
            sections.push(format!(
                "# History:\n{}\n",
                self.history(&request.stock_code).await?
            ));
        }
        // `financials` is intentionally Calendar in yahoofinance.py.
        if request.financials {
            sections.push(format!(
                "# Calendar:\n{}\n",
                self.calendar(&request.stock_code).await?
            ));
        }
        if request.balance_sheet {
            sections.push(format!(
                "# Balance sheet:\n{}\n",
                self.fundamentals(&request.stock_code, false, BALANCE_SHEET_KEYS)
                    .await?
            ));
            sections.push(format!(
                "# Quarterly balance sheet:\n{}\n",
                self.fundamentals(&request.stock_code, true, BALANCE_SHEET_KEYS)
                    .await?
            ));
        }
        if request.cash_flow_statement {
            sections.push(format!(
                "# Cash flow statement:\n{}\n",
                self.fundamentals(&request.stock_code, false, CASH_FLOW_KEYS)
                    .await?
            ));
            sections.push(format!(
                "# Quarterly cash flow statement:\n{}\n",
                self.fundamentals(&request.stock_code, true, CASH_FLOW_KEYS)
                    .await?
            ));
        }
        if request.news {
            sections.push(format!(
                "# News:\n{}\n",
                self.news(&request.stock_code).await?
            ));
        }
        // count and income_stmt are validated by Python but never read.
        Ok(sections.join("\n\n"))
    }
}

async fn decode_json_response(response: reqwest::Response, operation: &str) -> Result<Value> {
    let status = response.status();
    let body = read_bounded_body(response).await?;
    if !status.is_success() {
        bail!("Yahoo Finance {operation} upstream returned HTTP {status}");
    }
    serde_json::from_slice(&body)
        .with_context(|| format!("Yahoo Finance {operation} response is not JSON"))
}

async fn read_bounded_body(response: reqwest::Response) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("could not read Yahoo Finance response body")?;
        let remaining = MAX_YAHOO_RESPONSE_BODY.saturating_sub(body.len());
        if chunk.len() > remaining {
            bail!("Yahoo Finance response body exceeds {MAX_YAHOO_RESPONSE_BODY} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn unix_seconds() -> Result<i64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock precedes Unix epoch")?
        .as_secs() as i64)
}

fn merge_flattened(target: &mut Map<String, Value>, source: &Map<String, Value>) {
    for (key, value) in source {
        if value.is_null() {
            continue;
        }
        if let Some(object) = value.as_object() {
            for (nested_key, nested_value) in object {
                if !nested_value.is_null() {
                    let normalized = if nested_key == "maxAge" && nested_value.as_i64() == Some(1) {
                        Value::from(86_400)
                    } else {
                        normalize_yahoo_value(nested_key, nested_value)
                    };
                    target.insert(nested_key.clone(), normalized);
                }
            }
        } else {
            target.insert(key.clone(), normalize_yahoo_value(key, value));
        }
    }
}

fn normalize_yahoo_value(key: &str, value: &Value) -> Value {
    if let Some(object) = value.as_object() {
        if object.contains_key("raw") && object.contains_key("fmt") {
            return object
                .get(if matches!(key, "regularMarketTime" | "postMarketTime") {
                    "fmt"
                } else {
                    "raw"
                })
                .cloned()
                .unwrap_or(Value::Null);
        }
        return Value::Object(
            object
                .iter()
                .map(|(nested_key, nested_value)| {
                    (
                        nested_key.clone(),
                        normalize_yahoo_value(nested_key, nested_value),
                    )
                })
                .collect(),
        );
    }
    if let Some(array) = value.as_array() {
        return Value::Array(
            array
                .iter()
                .map(|value| normalize_yahoo_value(key, value))
                .collect(),
        );
    }
    match value {
        Value::String(value) => Value::String(value.replace('\u{a0}', " ")),
        value => value.clone(),
    }
}

fn series_markdown(value: &Value) -> Result<String> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("Yahoo Finance information must be an object"))?;
    let rows = object
        .iter()
        .map(|(key, value)| vec![key.clone(), markdown_value(value, false)])
        .collect::<Vec<_>>();
    Ok(markdown_table(&["", "0"], &rows, &[false, false]))
}

fn dataframe_markdown(rows: &[Value]) -> String {
    let mut columns = Vec::new();
    let mut seen = BTreeSet::new();
    for row in rows {
        if let Some(object) = row.as_object() {
            for key in object.keys() {
                if seen.insert(key.clone()) {
                    columns.push(key.clone());
                }
            }
        }
    }
    let mut headers = vec![String::new()];
    headers.extend(columns.iter().cloned());
    let rendered = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let mut cells = vec![index.to_string()];
            for column in &columns {
                cells.push(
                    row.get(column)
                        .map(|value| markdown_value(value, true))
                        .unwrap_or_else(|| "nan".into()),
                );
            }
            cells
        })
        .collect::<Vec<_>>();
    let mut right = vec![true];
    right.extend(std::iter::repeat_n(false, columns.len()));
    markdown_table_owned(&headers, &rendered, &right)
}

fn markdown_table(headers: &[&str], rows: &[Vec<String>], right: &[bool]) -> String {
    markdown_table_owned(
        &headers
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>(),
        rows,
        right,
    )
}

fn markdown_table_owned(headers: &[String], rows: &[Vec<String>], right: &[bool]) -> String {
    let widths = headers
        .iter()
        .enumerate()
        .map(|(column, header)| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .fold(header.chars().count(), |width, value| {
                    width.max(value.chars().count())
                })
                .max(1)
        })
        .collect::<Vec<_>>();
    let row = |cells: &[String]| {
        format!(
            "| {} |",
            cells
                .iter()
                .enumerate()
                .map(|(index, cell)| format!("{cell:<width$}", width = widths[index]))
                .collect::<Vec<_>>()
                .join(" | ")
        )
    };
    let mut lines = vec![row(headers)];
    lines.push(format!(
        "|{}|",
        widths
            .iter()
            .enumerate()
            .map(|(index, width)| {
                let dashes = "-".repeat((*width).max(1));
                if right.get(index).copied().unwrap_or(false) {
                    format!("-{dashes}:")
                } else {
                    format!(":{dashes}-")
                }
            })
            .collect::<Vec<_>>()
            .join("|")
    ));
    lines.extend(rows.iter().map(|cells| row(cells)));
    lines.join("\n")
}

fn markdown_value(value: &Value, nan_for_null: bool) -> String {
    match value {
        Value::Null => if nan_for_null { "nan" } else { "None" }.into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.replace('|', "\\|"),
        Value::Array(_) | Value::Object(_) => value.to_string().replace('|', "\\|"),
    }
}

fn history_markdown(data: &Value) -> Result<String> {
    let result = data
        .pointer("/chart/result/0")
        .ok_or_else(|| anyhow!("Yahoo Finance history response is missing chart result"))?;
    let timestamps = result
        .get("timestamp")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Yahoo Finance history response is missing timestamps"))?;
    let quote = result
        .pointer("/indicators/quote/0")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("Yahoo Finance history response is missing quote indicators"))?;
    let offset = result
        .pointer("/meta/gmtoffset")
        .and_then(Value::as_i64)
        .and_then(|seconds| i32::try_from(seconds).ok())
        .and_then(|seconds| UtcOffset::from_whole_seconds(seconds).ok())
        .unwrap_or(UtcOffset::UTC);
    let columns = [
        "Date",
        "Open",
        "High",
        "Low",
        "Close",
        "Volume",
        "Dividends",
        "Stock Splits",
    ];
    let rows = timestamps
        .iter()
        .enumerate()
        .map(|(index, timestamp)| {
            let timestamp = timestamp
                .as_i64()
                .ok_or_else(|| anyhow!("Yahoo Finance history timestamp {index} is invalid"))?;
            let date = OffsetDateTime::from_unix_timestamp(timestamp)
                .context("Yahoo Finance history timestamp is out of range")?
                .to_offset(offset)
                .date()
                .to_string();
            let indicator_value = |name: &str| {
                quote
                    .get(name)
                    .and_then(Value::as_array)
                    .and_then(|values| values.get(index))
                    .cloned()
                    .unwrap_or(Value::Null)
            };
            let close = indicator_value("close");
            let adjusted_close = result
                .pointer("/indicators/adjclose/0/adjclose")
                .and_then(Value::as_array)
                .and_then(|values| values.get(index))
                .cloned()
                .unwrap_or_else(|| close.clone());
            let adjustment = close
                .as_f64()
                .filter(|close| *close != 0.0)
                .zip(adjusted_close.as_f64())
                .map(|(close, adjusted)| adjusted / close);
            let adjusted = |name: &str| {
                let value = indicator_value(name);
                adjustment
                    .zip(value.as_f64())
                    .and_then(|(ratio, value)| serde_json::Number::from_f64(ratio * value))
                    .map(Value::Number)
                    .unwrap_or(value)
            };
            Ok(vec![
                date,
                markdown_value(&adjusted("open"), true),
                markdown_value(&adjusted("high"), true),
                markdown_value(&adjusted("low"), true),
                markdown_value(&adjusted_close, true),
                markdown_value(&indicator_value("volume"), true),
                event_value(result, "dividends", timestamp, "amount"),
                event_value(result, "splits", timestamp, "splitRatio"),
            ])
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(markdown_table(
        &columns,
        &rows,
        &[false, true, true, true, true, true, true, true],
    ))
}

fn event_value(result: &Value, event: &str, timestamp: i64, field: &str) -> String {
    result
        .pointer(&format!("/events/{event}/{timestamp}/{field}"))
        .map(|value| markdown_value(value, true))
        .unwrap_or_else(|| "0.0".into())
}

fn calendar_markdown(data: &Value) -> Result<String> {
    let events = data
        .pointer("/quoteSummary/result/0/calendarEvents")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("Yahoo Finance calendar response is missing calendarEvents"))?;
    let mut fields: Vec<(String, Vec<Value>)> = Vec::new();
    for (source, target) in [
        ("dividendDate", "Dividend Date"),
        ("exDividendDate", "Ex-Dividend Date"),
    ] {
        if let Some(timestamp) = events.get(source).and_then(yahoo_raw_i64) {
            fields.push((target.into(), vec![Value::String(unix_date(timestamp)?)]));
        }
    }
    if let Some(earnings) = events.get("earnings").and_then(Value::as_object) {
        let dates = earnings
            .get("earningsDate")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(yahoo_raw_i64)
                    .map(unix_date)
                    .map(|value| value.map(Value::String))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        fields.push(("Earnings Date".into(), dates));
        for (source, target) in [
            ("earningsHigh", "Earnings High"),
            ("earningsLow", "Earnings Low"),
            ("earningsAverage", "Earnings Average"),
            ("revenueHigh", "Revenue High"),
            ("revenueLow", "Revenue Low"),
            ("revenueAverage", "Revenue Average"),
        ] {
            fields.push((
                target.into(),
                vec![
                    earnings
                        .get(source)
                        .map(|value| normalize_yahoo_value(source, value))
                        .unwrap_or(Value::Null),
                ],
            ));
        }
    }
    let row_count = fields
        .iter()
        .map(|(_, values)| values.len())
        .max()
        .unwrap_or(0);
    let mut headers = vec![String::new()];
    headers.extend(fields.iter().map(|(name, _)| name.clone()));
    let rows = (0..row_count)
        .map(|index| {
            let mut row = vec![index.to_string()];
            row.extend(fields.iter().map(|(_, values)| {
                values
                    .get(index)
                    .or_else(|| (values.len() == 1).then(|| &values[0]))
                    .map(|value| markdown_value(value, true))
                    .unwrap_or_else(|| "nan".into())
            }));
            row
        })
        .collect::<Vec<_>>();
    let mut right = vec![true];
    right.extend(std::iter::repeat_n(false, fields.len()));
    Ok(markdown_table_owned(&headers, &rows, &right))
}

fn yahoo_raw_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.get("raw").and_then(Value::as_i64))
}

fn unix_date(timestamp: i64) -> Result<String> {
    Ok(OffsetDateTime::from_unix_timestamp(timestamp)
        .context("Yahoo Finance calendar timestamp is out of range")?
        .date()
        .to_string())
}

fn fundamentals_markdown(data: &Value, prefix: &str, keys: &[&str]) -> Result<String> {
    let result = data
        .pointer("/timeseries/result")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            anyhow!("Yahoo Finance fundamentals response is missing timeseries result")
        })?;
    let mut values: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
    let mut dates = BTreeSet::new();
    for object in result.iter().filter_map(Value::as_object) {
        for (typed_key, entries) in object {
            let Some(key) = typed_key.strip_prefix(prefix) else {
                continue;
            };
            let Some(entries) = entries.as_array() else {
                continue;
            };
            for entry in entries {
                let Some(date) = entry.get("asOfDate").and_then(Value::as_str) else {
                    continue;
                };
                let Some(value) = entry.pointer("/reportedValue/raw") else {
                    continue;
                };
                dates.insert(date.to_owned());
                values
                    .entry(key.into())
                    .or_default()
                    .insert(date.into(), value.clone());
            }
        }
    }
    let dates = dates.into_iter().rev().collect::<Vec<_>>();
    let mut headers = vec![String::new()];
    headers.extend(dates.iter().map(|date| format!("{date} 00:00:00")));
    let rows = keys
        .iter()
        .filter_map(|key| values.get(*key).map(|row| (*key, row)))
        .map(|(key, row)| {
            let mut cells = vec![key.to_owned()];
            cells.extend(dates.iter().map(|date| {
                row.get(date)
                    .map(|value| markdown_value(value, true))
                    .unwrap_or_else(|| "nan".into())
            }));
            cells
        })
        .collect::<Vec<_>>();
    let mut right = vec![false];
    right.extend(std::iter::repeat_n(true, dates.len()));
    Ok(markdown_table_owned(&headers, &rows, &right))
}

fn is_empty_json_value(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(value) => !value,
        Value::Number(value) => value.as_f64() == Some(0.0),
        Value::Array(values) => values.is_empty(),
        Value::Object(values) => values.is_empty(),
        Value::String(value) => value.is_empty(),
    }
}

fn deserialize_nullable_string<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

fn deserialize_nullable_f64<'de, D>(deserializer: D) -> std::result::Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<f64>::deserialize(deserializer)?.unwrap_or_default())
}

const BALANCE_SHEET_KEYS: &[&str] = &[
    "TreasurySharesNumber",
    "PreferredSharesNumber",
    "OrdinarySharesNumber",
    "ShareIssued",
    "NetDebt",
    "TotalDebt",
    "TangibleBookValue",
    "InvestedCapital",
    "WorkingCapital",
    "NetTangibleAssets",
    "CapitalLeaseObligations",
    "CommonStockEquity",
    "PreferredStockEquity",
    "TotalCapitalization",
    "TotalEquityGrossMinorityInterest",
    "MinorityInterest",
    "StockholdersEquity",
    "OtherEquityInterest",
    "GainsLossesNotAffectingRetainedEarnings",
    "OtherEquityAdjustments",
    "FixedAssetsRevaluationReserve",
    "ForeignCurrencyTranslationAdjustments",
    "MinimumPensionLiabilities",
    "UnrealizedGainLoss",
    "TreasuryStock",
    "RetainedEarnings",
    "AdditionalPaidInCapital",
    "CapitalStock",
    "OtherCapitalStock",
    "CommonStock",
    "PreferredStock",
    "TotalPartnershipCapital",
    "GeneralPartnershipCapital",
    "LimitedPartnershipCapital",
    "TotalLiabilitiesNetMinorityInterest",
    "TotalNonCurrentLiabilitiesNetMinorityInterest",
    "OtherNonCurrentLiabilities",
    "LiabilitiesHeldforSaleNonCurrent",
    "RestrictedCommonStock",
    "PreferredSecuritiesOutsideStockEquity",
    "DerivativeProductLiabilities",
    "EmployeeBenefits",
    "NonCurrentPensionAndOtherPostretirementBenefitPlans",
    "NonCurrentAccruedExpenses",
    "DuetoRelatedPartiesNonCurrent",
    "TradeandOtherPayablesNonCurrent",
    "NonCurrentDeferredLiabilities",
    "NonCurrentDeferredRevenue",
    "NonCurrentDeferredTaxesLiabilities",
    "LongTermDebtAndCapitalLeaseObligation",
    "LongTermCapitalLeaseObligation",
    "LongTermDebt",
    "LongTermProvisions",
    "CurrentLiabilities",
    "OtherCurrentLiabilities",
    "CurrentDeferredLiabilities",
    "CurrentDeferredRevenue",
    "CurrentDeferredTaxesLiabilities",
    "CurrentDebtAndCapitalLeaseObligation",
    "CurrentCapitalLeaseObligation",
    "CurrentDebt",
    "OtherCurrentBorrowings",
    "LineOfCredit",
    "CommercialPaper",
    "CurrentNotesPayable",
    "PensionandOtherPostRetirementBenefitPlansCurrent",
    "CurrentProvisions",
    "PayablesAndAccruedExpenses",
    "CurrentAccruedExpenses",
    "InterestPayable",
    "Payables",
    "OtherPayable",
    "DuetoRelatedPartiesCurrent",
    "DividendsPayable",
    "TotalTaxPayable",
    "IncomeTaxPayable",
    "AccountsPayable",
    "TotalAssets",
    "TotalNonCurrentAssets",
    "OtherNonCurrentAssets",
    "DefinedPensionBenefit",
    "NonCurrentPrepaidAssets",
    "NonCurrentDeferredAssets",
    "NonCurrentDeferredTaxesAssets",
    "DuefromRelatedPartiesNonCurrent",
    "NonCurrentNoteReceivables",
    "NonCurrentAccountsReceivable",
    "FinancialAssets",
    "InvestmentsAndAdvances",
    "OtherInvestments",
    "InvestmentinFinancialAssets",
    "HeldToMaturitySecurities",
    "AvailableForSaleSecurities",
    "FinancialAssetsDesignatedasFairValueThroughProfitorLossTotal",
    "TradingSecurities",
    "LongTermEquityInvestment",
    "InvestmentsinJointVenturesatCost",
    "InvestmentsInOtherVenturesUnderEquityMethod",
    "InvestmentsinAssociatesatCost",
    "InvestmentsinSubsidiariesatCost",
    "InvestmentProperties",
    "GoodwillAndOtherIntangibleAssets",
    "OtherIntangibleAssets",
    "Goodwill",
    "NetPPE",
    "AccumulatedDepreciation",
    "GrossPPE",
    "Leases",
    "ConstructionInProgress",
    "OtherProperties",
    "MachineryFurnitureEquipment",
    "BuildingsAndImprovements",
    "LandAndImprovements",
    "Properties",
    "CurrentAssets",
    "OtherCurrentAssets",
    "HedgingAssetsCurrent",
    "AssetsHeldForSaleCurrent",
    "CurrentDeferredAssets",
    "CurrentDeferredTaxesAssets",
    "RestrictedCash",
    "PrepaidAssets",
    "Inventory",
    "InventoriesAdjustmentsAllowances",
    "OtherInventories",
    "FinishedGoods",
    "WorkInProcess",
    "RawMaterials",
    "Receivables",
    "ReceivablesAdjustmentsAllowances",
    "OtherReceivables",
    "DuefromRelatedPartiesCurrent",
    "TaxesReceivable",
    "AccruedInterestReceivable",
    "NotesReceivable",
    "LoansReceivable",
    "AccountsReceivable",
    "AllowanceForDoubtfulAccountsReceivable",
    "GrossAccountsReceivable",
    "CashCashEquivalentsAndShortTermInvestments",
    "OtherShortTermInvestments",
    "CashAndCashEquivalents",
    "CashEquivalents",
    "CashFinancial",
    "CashCashEquivalentsAndFederalFundsSold",
];

const CASH_FLOW_KEYS: &[&str] = &[
    "ForeignSales",
    "DomesticSales",
    "AdjustedGeographySegmentData",
    "FreeCashFlow",
    "RepurchaseOfCapitalStock",
    "RepaymentOfDebt",
    "IssuanceOfDebt",
    "IssuanceOfCapitalStock",
    "CapitalExpenditure",
    "InterestPaidSupplementalData",
    "IncomeTaxPaidSupplementalData",
    "EndCashPosition",
    "OtherCashAdjustmentOutsideChangeinCash",
    "BeginningCashPosition",
    "EffectOfExchangeRateChanges",
    "ChangesInCash",
    "OtherCashAdjustmentInsideChangeinCash",
    "CashFlowFromDiscontinuedOperation",
    "FinancingCashFlow",
    "CashFromDiscontinuedFinancingActivities",
    "CashFlowFromContinuingFinancingActivities",
    "NetOtherFinancingCharges",
    "InterestPaidCFF",
    "ProceedsFromStockOptionExercised",
    "CashDividendsPaid",
    "PreferredStockDividendPaid",
    "CommonStockDividendPaid",
    "NetPreferredStockIssuance",
    "PreferredStockPayments",
    "PreferredStockIssuance",
    "NetCommonStockIssuance",
    "CommonStockPayments",
    "CommonStockIssuance",
    "NetIssuancePaymentsOfDebt",
    "NetShortTermDebtIssuance",
    "ShortTermDebtPayments",
    "ShortTermDebtIssuance",
    "NetLongTermDebtIssuance",
    "LongTermDebtPayments",
    "LongTermDebtIssuance",
    "InvestingCashFlow",
    "CashFromDiscontinuedInvestingActivities",
    "CashFlowFromContinuingInvestingActivities",
    "NetOtherInvestingChanges",
    "InterestReceivedCFI",
    "DividendsReceivedCFI",
    "NetInvestmentPurchaseAndSale",
    "SaleOfInvestment",
    "PurchaseOfInvestment",
    "NetInvestmentPropertiesPurchaseAndSale",
    "SaleOfInvestmentProperties",
    "PurchaseOfInvestmentProperties",
    "NetBusinessPurchaseAndSale",
    "SaleOfBusiness",
    "PurchaseOfBusiness",
    "NetIntangiblesPurchaseAndSale",
    "SaleOfIntangibles",
    "PurchaseOfIntangibles",
    "NetPPEPurchaseAndSale",
    "SaleOfPPE",
    "PurchaseOfPPE",
    "CapitalExpenditureReported",
    "OperatingCashFlow",
    "CashFromDiscontinuedOperatingActivities",
    "CashFlowFromContinuingOperatingActivities",
    "TaxesRefundPaid",
    "InterestReceivedCFO",
    "InterestPaidCFO",
    "DividendReceivedCFO",
    "DividendPaidCFO",
    "ChangeInWorkingCapital",
    "ChangeInOtherWorkingCapital",
    "ChangeInOtherCurrentLiabilities",
    "ChangeInOtherCurrentAssets",
    "ChangeInPayablesAndAccruedExpense",
    "ChangeInAccruedExpense",
    "ChangeInInterestPayable",
    "ChangeInPayable",
    "ChangeInDividendPayable",
    "ChangeInAccountPayable",
    "ChangeInTaxPayable",
    "ChangeInIncomeTaxPayable",
    "ChangeInPrepaidAssets",
    "ChangeInInventory",
    "ChangeInReceivables",
    "ChangesInAccountReceivables",
    "OtherNonCashItems",
    "ExcessTaxBenefitFromStockBasedCompensation",
    "StockBasedCompensation",
    "UnrealizedGainLossOnInvestmentSecurities",
    "ProvisionandWriteOffofAssets",
    "AssetImpairmentCharge",
    "AmortizationOfSecurities",
    "DeferredTax",
    "DeferredIncomeTax",
    "DepreciationAmortizationDepletion",
    "Depletion",
    "DepreciationAndAmortization",
    "AmortizationCashFlow",
    "AmortizationOfIntangibles",
    "Depreciation",
    "OperatingGainsLosses",
    "PensionAndEmployeeBenefitExpense",
    "EarningsLossesFromEquityInvestments",
    "GainLossOnInvestmentSecurities",
    "NetForeignCurrencyExchangeGainLoss",
    "GainLossOnSaleOfPPE",
    "GainLossOnSaleOfBusiness",
    "NetIncomeFromContinuingOperations",
    "CashFlowsfromusedinOperatingActivitiesDirect",
    "TaxesRefundPaidDirect",
    "InterestReceivedDirect",
    "InterestPaidDirect",
    "DividendsReceivedDirect",
    "DividendsPaidDirect",
    "ClassesofCashPayments",
    "OtherCashPaymentsfromOperatingActivities",
    "PaymentsonBehalfofEmployees",
    "PaymentstoSuppliersforGoodsandServices",
    "ClassesofCashReceiptsfromOperatingActivities",
    "OtherCashReceiptsfromOperatingActivities",
    "ReceiptsfromGovernmentGrants",
    "ReceiptsfromCustomers",
];

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
    use axum::response::{IntoResponse, Response};
    use axum::routing::any;
    use std::sync::{Arc, Mutex as StdMutex};

    #[derive(Debug, Clone)]
    struct RecordedRequest {
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    }

    #[derive(Clone, Default)]
    struct Recorded(Arc<StdMutex<Vec<RecordedRequest>>>);

    async fn yahoo_handler(
        State(recorded): State<Recorded>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        recorded.0.lock().unwrap().push(RecordedRequest {
            method,
            uri: uri.clone(),
            headers,
            body,
        });
        let path = uri.path();
        if path == "/fc" {
            return (
                StatusCode::NOT_FOUND,
                [(header::SET_COOKIE, "A3=session-cookie; Path=/; Secure")],
                "",
            )
                .into_response();
        }
        if path == "/crumb" {
            return "test-crumb".into_response();
        }
        let query = uri.query().unwrap_or_default();
        let payload = match path {
            path if path.starts_with("/quoteSummary/") && query.contains("calendarEvents") => {
                json!({"quoteSummary":{"result":[{"calendarEvents":{
                    "dividendDate": 1767225600_i64,
                    "earnings":{"earningsDate":[1769904000_i64],"earningsHigh":{"raw":2.5},"revenueAverage":{"raw":1000}}
                }}]}})
            }
            path if path.starts_with("/quoteSummary/") => json!({"quoteSummary":{"result":[{
                "assetProfile":{"longBusinessSummary":"Rust\u{a0}systems","maxAge":1},
                "summaryDetail":{"marketCap":{"raw":1234,"fmt":"1.23K"}},
                "quoteType":{"exchange":"NMS"}
            }]}}),
            "/quote" => json!({"quoteResponse":{"result":[{
                "symbol":"AAPL","regularMarketPrice":189.5,"currency":"USD",
                "regularMarketChangePercent":null,"regularMarketTime":{"raw":1767225600,"fmt":"2026-01-01 12:00PM EST"}
            }]}}),
            path if path.starts_with("/timeseries1/") => {
                json!({"timeseries":{"error":null,"result":[{
                    "trailingPegRatio":[{"asOfDate":"2026-01-01","reportedValue":{"raw":2.25}}]
                }]}})
            }
            path if path.starts_with("/chart/") => json!({"chart":{"result":[{
                "meta":{"gmtoffset":0},"timestamp":[1767225600_i64],
                "indicators":{"quote":[{"open":[188.0],"high":[190.0],"low":[187.0],"close":[189.5],"volume":[100]}],"adjclose":[{"adjclose":[180.0]}]},
                "events":{"dividends":{"1767225600":{"amount":0.25}}}
            }]}}),
            path if path.starts_with("/timeseries2/") => {
                let entries = if query.contains("CapitalExpenditure") {
                    json!([
                        {"meta":{},"timestamp":[1767139200_i64],"annualFreeCashFlow":[{"asOfDate":"2025-12-31","reportedValue":{"raw":500.0}}]},
                        {"meta":{},"timestamp":[1767139200_i64],"quarterlyFreeCashFlow":[{"asOfDate":"2025-12-31","reportedValue":{"raw":125.0}}]}
                    ])
                } else {
                    json!([
                        {"meta":{},"timestamp":[1767139200_i64],"annualTotalAssets":[{"asOfDate":"2025-12-31","reportedValue":{"raw":9000.0}}]},
                        {"meta":{},"timestamp":[1767139200_i64],"quarterlyTotalAssets":[{"asOfDate":"2025-12-31","reportedValue":{"raw":9100.0}}]}
                    ])
                };
                json!({"timeseries":{"result":entries}})
            }
            "/news" => json!({"data":{"tickerStream":{"stream":[
                {"title":"Rust earnings","publisher":"Wire","ad":[]},
                {"title":"Sponsored","ad":[{"id":"paid"}]}
            ]}}}),
            _ => return (StatusCode::NOT_FOUND, "missing fixture").into_response(),
        };
        axum::Json(payload).into_response()
    }

    async fn server() -> (YahooFinanceClient, Recorded, tokio::task::JoinHandle<()>) {
        let recorded = Recorded::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .fallback(any(yahoo_handler))
            .with_state(recorded.clone());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (
            YahooFinanceClient::new_with_base(&base).unwrap(),
            recorded,
            handle,
        )
    }

    #[tokio::test]
    async fn python_contract_uses_cookie_crumb_all_sections_and_upstream_order() {
        let (client, recorded, handle) = server().await;
        let report = client
            .report(&YahooFinanceRequest {
                stock_code: "AAPL".into(),
                info: true,
                history: true,
                count: true,
                financials: true,
                income_stmt: true,
                balance_sheet: true,
                cash_flow_statement: true,
                news: true,
            })
            .await
            .unwrap();
        handle.abort();

        let headings = [
            "# Information:",
            "# History:",
            "# Calendar:",
            "# Balance sheet:",
            "# Quarterly balance sheet:",
            "# Cash flow statement:",
            "# Quarterly cash flow statement:",
            "# News:",
        ];
        let positions = headings
            .iter()
            .map(|heading| report.find(heading).unwrap())
            .collect::<Vec<_>>();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(report.contains("Rust systems"));
        assert!(report.contains("marketCap"));
        assert!(report.contains("1234"));
        assert!(report.contains("86400"));
        assert!(report.contains("2026-01-01 12:00PM EST"));
        assert!(report.contains("trailingPegRatio"));
        assert!(report.contains("180"));
        assert!(report.contains("TotalAssets"));
        assert!(report.contains("FreeCashFlow"));
        assert!(report.contains("Rust earnings"));
        assert!(!report.contains("Sponsored"));
        assert!(!report.contains("# Count"));
        assert!(!report.contains("# Income"));

        let requests = recorded.0.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.uri.path() == "/fc")
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.uri.path() == "/crumb")
                .count(),
            1
        );
        for request in requests
            .iter()
            .filter(|request| !matches!(request.uri.path(), "/fc" | "/crumb" | "/quote"))
        {
            assert!(
                request
                    .uri
                    .query()
                    .unwrap_or_default()
                    .contains("crumb=test-crumb")
            );
            assert_eq!(request.headers[header::COOKIE], "A3=session-cookie");
        }
        let news = requests
            .iter()
            .find(|request| request.uri.path() == "/news")
            .unwrap();
        assert_eq!(news.method, Method::POST);
        let news_body: Value = serde_json::from_slice(&news.body).unwrap();
        assert_eq!(news_body["serviceConfig"]["snippetCount"], 10);
        assert_eq!(news_body["serviceConfig"]["s"], json!(["AAPL"]));
    }

    #[tokio::test]
    async fn go_contract_preserves_query_headers_and_zero_values_for_nulls() {
        let (client, recorded, handle) = server().await;
        let envelope = client
            .quote_go(
                &["AAPL".into(), "MSFT".into(), "0005.HK".into()],
                &["symbol".into(), "regularMarketPrice".into()],
            )
            .await
            .unwrap();
        handle.abort();

        assert_eq!(envelope.results.len(), 1);
        assert_eq!(envelope.results[0].symbol, "AAPL");
        assert_eq!(envelope.results[0].regular_market_price, 189.5);
        assert_eq!(envelope.results[0].regular_market_change_percent, 0.0);
        {
            let requests = recorded.0.lock().unwrap();
            assert_eq!(requests.len(), 1);
            let request = &requests[0];
            let query: BTreeMap<_, _> =
                reqwest::Url::parse(&format!("http://local{}", request.uri))
                    .unwrap()
                    .query_pairs()
                    .into_owned()
                    .collect();
            assert_eq!(query["symbols"], "AAPL,MSFT,0005.HK");
            assert_eq!(query["fields"], "symbol,regularMarketPrice");
            assert_eq!(request.headers[USER_AGENT], GO_USER_AGENT);
            assert_eq!(request.headers[ACCEPT], "application/json");
            assert!(!request.uri.query().unwrap().contains("crumb"));
        }

        let error = client.quote_go(&[], &[]).await.unwrap_err();
        assert!(error.to_string().contains("symbols"));
    }

    #[tokio::test]
    async fn empty_symbol_short_circuits_and_cookie_bootstrap_fails_closed() {
        let (client, recorded, handle) = server().await;
        let report = client
            .report(&YahooFinanceRequest::python_defaults(""))
            .await
            .unwrap();
        assert!(report.is_empty());
        assert!(recorded.0.lock().unwrap().is_empty());
        handle.abort();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().fallback(any(|| async { "no cookie" }));
        let server_handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = YahooFinanceClient::new_with_base(&base).unwrap();
        let error = client
            .report(&YahooFinanceRequest::python_defaults("AAPL"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no cookies"));
        server_handle.abort();
    }

    #[test]
    fn full_yfinance_key_sets_and_nullable_go_scalars_are_preserved() {
        assert_eq!(BALANCE_SHEET_KEYS.len(), 145);
        assert_eq!(CASH_FLOW_KEYS.len(), 123);
        let quote: YahooFinanceQuote = serde_json::from_value(json!({
            "symbol": null,
            "regularMarketPrice": null,
            "currency": null,
            "regularMarketChangePercent": null
        }))
        .unwrap();
        assert_eq!(quote.symbol, "");
        assert_eq!(quote.currency, "");
        assert_eq!(quote.regular_market_price, 0.0);
        assert_eq!(quote.regular_market_change_percent, 0.0);
    }

    #[tokio::test]
    #[ignore = "requires live Yahoo Finance access; anti-bot or egress limits may return HTTP 403/429"]
    async fn live_yahoo_finance_default_report() {
        let report = YahooFinanceClient::default()
            .report(&YahooFinanceRequest::python_defaults("AAPL"))
            .await
            .unwrap();
        assert!(report.contains("# Information:"));
        assert!(report.contains("# News:"));
    }
}
