//! Tencent Finance (腾讯财经) quote connector — mainland-China native, no API
//! key required. Uses the public `qt.gtimg.cn` quote endpoint (GBK-encoded)
//! and the UTF-8 `web.ifzq.gtimg.cn` search endpoint. Returns RAGFlow-shaped
//! tool rows.

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use encoding_rs::GBK;
use regex::Regex;
use reqwest::redirect::Policy as RedirectPolicy;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::OnceLock;

const TENCENT_QUOTE_ENDPOINT: &str = "https://qt.gtimg.cn/q";
const TENCENT_SUGGEST_ENDPOINT: &str = "https://smartbox.gtimg.cn/s3/?v=2&q=";
const MAX_TENCENT_RESPONSE_BODY: usize = 16 << 20;
const TENCENT_USER_AGENT: &str = concat!(
    "Mozilla/5.0 (X11; Linux x86_64) RayRAG/",
    env!("CARGO_PKG_VERSION")
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TencentFinanceRequest {
    /// Stock symbol, e.g. `sh600519`, `sz000001`, `hk00700`, `usAAPL`.
    pub symbol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TencentQuote {
    pub symbol: String,
    pub name: String,
    pub price: String,
    pub change: String,
    pub change_percent: String,
    pub open: String,
    pub high: String,
    pub low: String,
    pub volume: String,
    pub amount: String,
}

#[async_trait]
pub trait TencentFinanceProvider: Send + Sync {
    async fn quote(&self, request: &TencentFinanceRequest) -> Result<TencentQuote>;
}

#[derive(Debug, Clone)]
pub struct TencentFinanceClient {
    client: reqwest::Client,
    quote_endpoint: reqwest::Url,
}

impl Default for TencentFinanceClient {
    fn default() -> Self {
        Self::new_with_endpoints(TENCENT_QUOTE_ENDPOINT)
            .expect("fixed Tencent quote endpoint is valid")
    }
}

impl TencentFinanceClient {
    pub(crate) fn new_with_endpoints(quote_endpoint: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(RedirectPolicy::none())
            .timeout(crate::common::cmd_timeout::duration())
            .user_agent(TENCENT_USER_AGENT)
            .build()
            .context("could not build Tencent Finance HTTP client")?;
        Ok(Self {
            client,
            quote_endpoint: quote_endpoint
                .parse()
                .context("invalid Tencent quote endpoint")?,
        })
    }
}

fn quote_field_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#""([^"]*)""#).expect("valid quote field regex"))
}

fn parse_tencent_quote(symbol: &str, body: &[u8]) -> Result<TencentQuote> {
    // Response shape: v_sh600519="1~贵州茅台~600519~1700.00~...";
    let (decoded, _, _) = GBK.decode(body);
    let text = decoded.trim();
    let quoted = quote_field_re()
        .captures(text)
        .and_then(|capture| capture.get(1))
        .map(|m| m.as_str())
        .ok_or_else(|| anyhow!("tencent_finance: no quoted payload for {symbol}"))?;
    let fields: Vec<&str> = quoted.split('~').collect();
    // Field indices follow the well-known Tencent quote protocol.
    if fields.len() < 38 {
        bail!("tencent_finance: malformed quote response for {symbol}");
    }
    Ok(TencentQuote {
        symbol: symbol.to_string(),
        name: fields[1].to_string(),
        price: fields[3].to_string(),
        change: fields[31].to_string(),
        change_percent: fields[32].to_string(),
        open: fields[5].to_string(),
        high: fields[33].to_string(),
        low: fields[34].to_string(),
        volume: fields[36].to_string(),
        amount: fields[37].to_string(),
    })
}

#[async_trait]
impl TencentFinanceProvider for TencentFinanceClient {
    async fn quote(&self, request: &TencentFinanceRequest) -> Result<TencentQuote> {
        if request.symbol.trim().is_empty() {
            bail!("tencent_finance: symbol is required");
        }
        let url = self
            .quote_endpoint
            .clone()
            .query_pairs_mut()
            .extend_pairs([("q", request.symbol.as_str())])
            .finish()
            .clone();
        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("Tencent Finance request failed")?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .context("could not read Tencent Finance response body")?;
        if body.len() > MAX_TENCENT_RESPONSE_BODY {
            bail!(
                "Tencent Finance response exceeds {} bytes",
                MAX_TENCENT_RESPONSE_BODY
            );
        }
        if !status.is_success() {
            bail!("tencent_finance: upstream returned {}", status.as_u16());
        }
        parse_tencent_quote(&request.symbol, &body)
    }
}

pub fn tencent_quote_to_tool_row(quote: &TencentQuote) -> Value {
    let mut row = Map::new();
    row.insert("symbol".into(), Value::String(quote.symbol.clone()));
    row.insert("name".into(), Value::String(quote.name.clone()));
    row.insert("price".into(), Value::String(quote.price.clone()));
    row.insert("change".into(), Value::String(quote.change.clone()));
    row.insert(
        "change_percent".into(),
        Value::String(quote.change_percent.clone()),
    );
    row.insert("open".into(), Value::String(quote.open.clone()));
    row.insert("high".into(), Value::String(quote.high.clone()));
    row.insert("low".into(), Value::String(quote.low.clone()));
    row.insert("volume".into(), Value::String(quote.volume.clone()));
    row.insert("amount".into(), Value::String(quote.amount.clone()));
    Value::Object(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gbk_quote_response() {
        // Simulated GBK-encoded response for sh600519 (贵州茅台) following the
        // real Tencent protocol field layout: 3=price, 5=open, 31=change,
        // 32=change%, 33=high, 34=low, 36=volume, 37=amount.
        let text = "v_sh600519=\"1~贵州茅台~600519~1700.00~1695.00~1698.00~123456~789012~1700.00~1695.00~1690.00~1700.00~1699.00~5.00~0.30~1234567~2098765432~f~sh600519~1700.00~5.00~0.30~1698.00~1700.00~1690.00~1695.00~123456~789012~0.00~0.00~0.00~5.00~0.30~1700.00~1690.00~123456~123456~789012~1~0~0~1~20240802150000~0~0~0~0~0~0~0~0~0~\";";
        let (encoded, _, _) = GBK.encode(text);
        let quote = parse_tencent_quote("sh600519", &encoded).unwrap();
        assert_eq!(quote.name, "贵州茅台");
        assert_eq!(quote.symbol, "sh600519");
        assert_eq!(quote.price, "1700.00");
        assert_eq!(quote.change, "5.00");
        assert_eq!(quote.change_percent, "0.30");
    }

    #[test]
    fn malformed_response_is_rejected() {
        let error = parse_tencent_quote("sh600519", b"v_sh600519=\"1~only~two\"");
        assert!(error.is_err());
    }

    #[tokio::test]
    async fn empty_symbol_is_rejected() {
        let client = TencentFinanceClient::default();
        let error = client
            .quote(&TencentFinanceRequest { symbol: "".into() })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("symbol"));
    }

    #[tokio::test]
    #[ignore = "requires live access to qt.gtimg.cn"]
    async fn live_tencent_quote() {
        let quote = TencentFinanceClient::default()
            .quote(&TencentFinanceRequest {
                symbol: "sh600519".into(),
            })
            .await
            .unwrap();
        assert_eq!(quote.name, "贵州茅台");
    }
}
