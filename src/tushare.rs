//! TuShare 连接器 — RAGFlow `tushare.py` 的 Rust 实现
//!
//! 上游语义：POST api.tushare.pro 的 `news` 接口，按 src（新闻源）、时间范围、
//! keyword 过滤，输出 markdown 表格。token 从节点参数或 TUSHARE_TOKEN 读取。

use anyhow::{Result, bail};
use serde_json::{Value, json};

const TUSHARE_ENDPOINT: &str = "https://api.tushare.pro";

/// 支持的快讯新闻源（对齐上游 check_valid_value）。
pub const TUSHARE_SOURCES: &[&str] = &[
    "sina",
    "wallstreetcn",
    "10jqka",
    "eastmoney",
    "yuncaijing",
    "fenghuang",
    "jinrongjie",
];

/// TuShare 新闻请求。
#[derive(Debug, Clone)]
pub struct TuShareRequest {
    pub token: String,
    pub src: String,
    pub start_date: String,
    pub end_date: String,
    pub keyword: String,
}

/// TuShare 客户端。
#[derive(Debug, Clone)]
pub struct TuShareClient {
    http: reqwest::Client,
}

impl Default for TuShareClient {
    fn default() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(crate::common::cmd_timeout::duration())
                .build()
                .expect("tushare http client"),
        }
    }
}

impl TuShareClient {
    /// 查询快讯（对齐上游 tushare `news` api）。
    pub async fn news(&self, request: &TuShareRequest) -> Result<String> {
        self.news_at(request, TUSHARE_ENDPOINT).await
    }

    /// 支持测试注入端点的内部实现。
    async fn news_at(&self, request: &TuShareRequest, endpoint: &str) -> Result<String> {
        let token = request.token.trim();
        if token.is_empty() || token == "xxx" {
            bail!("TuShare token is required (node param `token` or env TUSHARE_TOKEN)");
        }
        if !TUSHARE_SOURCES.contains(&request.src.as_str()) {
            bail!("TuShare src must be one of: {}", TUSHARE_SOURCES.join(", "));
        }
        if request.start_date >= request.end_date {
            bail!("TuShare start_date must be earlier than end_date");
        }
        let payload = json!({
            "api_name": "news",
            "token": token,
            "params": {
                "src": request.src,
                "start_date": request.start_date,
                "end_date": request.end_date,
            }
        });
        let response = self.http.post(endpoint).json(&payload).send().await?;
        if !response.status().is_success() {
            bail!("TuShare HTTP {}", response.status());
        }
        let body: Value = response.json().await?;
        if body.get("code").and_then(Value::as_i64) != Some(0) {
            let message = body
                .get("msg")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            bail!("TuShare error: {message}");
        }
        let data = body
            .get("data")
            .ok_or_else(|| anyhow::anyhow!("TuShare missing data"))?;
        let fields: Vec<String> = data
            .get("fields")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let content_index = fields.iter().position(|f| f == "content");
        let items = data
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let keyword = request.keyword.trim().to_lowercase();
        let mut rows = Vec::new();
        for item in items {
            let values: Vec<&Value> = item
                .as_array()
                .map(|a| a.iter().collect())
                .unwrap_or_default();
            let content = content_index
                .and_then(|i| values.get(i).copied())
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if content.trim().is_empty() {
                continue;
            }
            if !keyword.is_empty() && !content.to_lowercase().contains(&keyword) {
                continue;
            }
            rows.push(content);
        }
        if rows.is_empty() {
            return Ok(String::new());
        }
        // 对齐上游 df.to_markdown()：两列序号 + content
        let mut out = String::from("| index | content |\n| --- | --- |\n");
        for (index, row) in rows.iter().enumerate() {
            let escaped = row.replace('|', "\\|");
            out.push_str(&format!("| {index} | {escaped} |\n"));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::post};

    fn sample_response() -> Json<Value> {
        Json(json!({
            "code": 0,
            "msg": "",
            "data": {
                "fields": ["content", "datetime"],
                "items": [
                    ["央行今日开展5000亿元逆回购操作", "2026-08-04 09:00:00"],
                    ["贵州茅台发布半年报", "2026-08-04 08:30:00"],
                ]
            }
        }))
    }

    #[tokio::test]
    async fn news_filters_by_keyword_and_renders_markdown() {
        let app = Router::new().route("/", post(|_: Json<Value>| async { sample_response() }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = TuShareClient {
            http: reqwest::Client::builder()
                .timeout(crate::common::cmd_timeout::duration())
                .build()
                .unwrap(),
        };
        // 客户端直连 mock：通过重写端点的内部方法
        let output = client
            .news_at(
                &TuShareRequest {
                    token: "test-token".into(),
                    src: "eastmoney".into(),
                    start_date: "2026-08-01 00:00:00".into(),
                    end_date: "2026-08-05 00:00:00".into(),
                    keyword: "茅台".into(),
                },
                &format!("http://{addr}/"),
            )
            .await
            .unwrap();
        assert!(output.contains("贵州茅台发布半年报"));
        assert!(!output.contains("央行"));
        assert!(output.starts_with("| index | content |"));
    }

    #[tokio::test]
    async fn rejects_missing_token() {
        let client = TuShareClient::default();
        let result = client
            .news(&TuShareRequest {
                token: "".into(),
                src: "eastmoney".into(),
                start_date: "2026-08-01 00:00:00".into(),
                end_date: "2026-08-05 00:00:00".into(),
                keyword: "".into(),
            })
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("token"));
    }

    #[tokio::test]
    async fn rejects_invalid_source() {
        let client = TuShareClient::default();
        let result = client
            .news(&TuShareRequest {
                token: "t".into(),
                src: "not-a-source".into(),
                start_date: "2026-08-01 00:00:00".into(),
                end_date: "2026-08-05 00:00:00".into(),
                keyword: "".into(),
            })
            .await;
        assert!(result.is_err());
    }
}
