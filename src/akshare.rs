//! AkShare 连接器 — RAGFlow `akshare.py`（ak.stock_news_em）的 Rust 实现
//!
//! 上游语义：输入 content（数组以 "," 连接）视为股票代码/关键词，调用东方财富
//! 搜索接口返回相关新闻，按 top_n 截断，输出 Markdown 链接列表。
//! 域名固定（search-api-web.eastmoney.com），无用户 URL 输入，天然免疫 SSRF。

use anyhow::{Result, bail};
use serde_json::Value;

const SEARCH_ENDPOINT: &str = "https://search-api-web.eastmoney.com/search/jsonp";

/// 单条东财新闻（对齐 akshare stock_news_em 的列：标题/链接/内容/时间/来源）。
#[derive(Debug, Clone)]
pub struct StockNews {
    pub title: String,
    pub url: String,
    pub content: String,
    pub date: String,
    pub media_name: String,
}

/// AkShare 新闻请求。
#[derive(Debug, Clone)]
pub struct AkShareRequest {
    pub symbol: String,
    pub top_n: usize,
}

/// 东方财富新闻客户端。
#[derive(Debug, Clone)]
pub struct AkShareClient {
    http: reqwest::Client,
}

impl Default for AkShareClient {
    fn default() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(crate::common::cmd_timeout::duration())
                .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36")
                .build()
                .expect("akshare http client"),
        }
    }
}

impl AkShareClient {
    /// 查询个股/关键词相关新闻（对齐 `ak.stock_news_em(symbol)`）。
    pub async fn stock_news_em(&self, request: &AkShareRequest) -> Result<Vec<StockNews>> {
        self.stock_news_em_at(request, SEARCH_ENDPOINT).await
    }

    /// 支持测试注入端点的内部实现。
    async fn stock_news_em_at(
        &self,
        request: &AkShareRequest,
        endpoint: &str,
    ) -> Result<Vec<StockNews>> {
        let symbol = request.symbol.trim();
        if symbol.is_empty() {
            bail!("AkShare symbol must not be empty");
        }
        let top_n = request.top_n.clamp(1, 50);
        let param = serde_json::json!({
            "uid": "",
            "keyword": symbol,
            "type": ["cmsArticleWebOld"],
            "client": "web",
            "clientType": "web",
            "clientVersion": "curr",
            "param": {
                "cmsArticleWebOld": {
                    "searchScope": "default",
                    "sort": "default",
                    "pageIndex": 1,
                    "pageSize": top_n,
                    "preTag": "<em>",
                    "postTag": "</em>"
                }
            }
        });
        let encoded = urlencoding(&param.to_string());
        let url = format!("{endpoint}?cb=&param={encoded}");
        let response = self
            .http
            .get(&url)
            .header("Referer", "https://so.eastmoney.com/")
            .send()
            .await?;
        if !response.status().is_success() {
            bail!("AkShare HTTP {}", response.status());
        }
        let body: Value = response.json().await?;
        if body.get("code").and_then(Value::as_i64) != Some(0) {
            let message = body
                .get("msg")
                .and_then(Value::as_str)
                .unwrap_or("unknown search error");
            bail!("AkShare error: {message}");
        }
        let articles = body
            .get("result")
            .and_then(|result| result.get("cmsArticleWebOld"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut news = Vec::with_capacity(articles.len());
        for article in articles {
            let title = strip_em(article.get("title").and_then(Value::as_str).unwrap_or(""));
            if title.is_empty() {
                continue;
            }
            news.push(StockNews {
                title,
                url: article
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                content: strip_em(article.get("content").and_then(Value::as_str).unwrap_or("")),
                date: article
                    .get("date")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                media_name: article
                    .get("mediaName")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            });
        }
        Ok(news)
    }

    /// 渲染为 Markdown 列表（对齐上游 DataFrame 行格式）。
    pub fn render_markdown(news: &[StockNews]) -> String {
        if news.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        for item in news {
            if !item.url.is_empty() {
                out.push_str(&format!(
                    "- [{}]({})\n  新闻内容: {} \n  发布时间: {} \n  文章来源: {}\n",
                    item.title, item.url, item.content, item.date, item.media_name
                ));
            } else {
                out.push_str(&format!(
                    "- {}\n  新闻内容: {} \n  发布时间: {} \n  文章来源: {}\n",
                    item.title, item.content, item.date, item.media_name
                ));
            }
        }
        out
    }
}

/// 移除东财搜索返回的 `<em>` 高亮标签。
fn strip_em(text: &str) -> String {
    text.replace("<em>", "")
        .replace("</em>", "")
        .trim()
        .to_string()
}

/// URL 查询参数编码（保留 JSON 字符）。
fn urlencoding(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};
    use serde_json::json;

    const SAMPLE_RESPONSE: &str = r#"{"code":0,"msg":"OK","result":{"cmsArticleWebOld":[
        {"date":"2026-08-03 18:43:00","title":"贵州茅台<em>600519</em>收盘上涨","content":"今日贵州茅台收报<em>1400</em>元","mediaName":"证券时报网","url":"http://finance.eastmoney.com/a/202608033829918487.html"},
        {"date":"2026-08-03 09:00:00","title":"无链接新闻","content":"内容正文","mediaName":"财联社","url":""}
    ]}}"#;

    #[test]
    fn strip_em_removes_highlight_tags() {
        assert_eq!(strip_em("贵州<em>茅台</em>涨了"), "贵州茅台涨了");
    }

    #[test]
    fn urlencoding_keeps_json_shape() {
        let encoded = urlencoding(r#"{"a":"1 2"}"#);
        assert!(encoded.contains("%7B") && encoded.contains("%22") && encoded.contains("%20"));
    }

    #[tokio::test]
    async fn parses_eastmoney_search_response() {
        let app = Router::new().route("/search/jsonp", get(|| async { SAMPLE_RESPONSE }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = AkShareClient {
            http: reqwest::Client::builder()
                .timeout(crate::common::cmd_timeout::duration())
                .build()
                .unwrap(),
        };
        let response = client
            .stock_news_em_at(
                &AkShareRequest {
                    symbol: "600519".into(),
                    top_n: 10,
                },
                &format!("http://{addr}/search/jsonp"),
            )
            .await
            .unwrap();
        assert_eq!(response.len(), 2);
        assert_eq!(response[0].title, "贵州茅台600519收盘上涨");
        assert!(response[0].content.contains("1400"));
        assert_eq!(response[0].media_name, "证券时报网");
        assert_eq!(response[1].title, "无链接新闻");

        let markdown = AkShareClient::render_markdown(&response);
        assert!(markdown.contains("[贵州茅台600519收盘上涨]"));
        assert!(markdown.contains("文章来源: 证券时报网"));
    }

    #[tokio::test]
    async fn rejects_empty_symbol() {
        let client = AkShareClient::default();
        let result = client
            .stock_news_em(&AkShareRequest {
                symbol: "  ".into(),
                top_n: 10,
            })
            .await;
        assert!(result.is_err());
    }
}
