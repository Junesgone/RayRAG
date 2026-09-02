//! WenCai（同花顺问财）连接器 — RAGFlow `agent/tools/wencai.py`（pywencai）的 Rust 实现
//!
//! 上游语义（对齐 pywencai `get_robot_data` / `get_page`）：
//!   1. POST http://www.iwencai.com/customized/chart/get-robot-data
//!      请求体为 JSON：question（必填）、perpage（字符串）、page、source、
//!      version、add_info、log_info、secondary_intent（query_type）。
//!   2. 解析响应 `data.answer[0].txt[0].content`（对象或 JSON 字符串）中的
//!      components；若唯一组件为 `xuangu_tableV1`（选股表格），再向
//!      footer url（`/gateway/urp/v7/landing/getDataList`）以表单格式
//!      请求分页数据，取 `answer.components[0].data.datas` 行渲染 Markdown。
//!   3. 其余 show_type（container/txt/tab/dragon_tiger_stock/textblocklinkone/
//!      common）按 pywencai convert.py 的 handler 语义渲染为 Markdown 分段。
//!
//! 对齐 RAGFlow wencai.py 的输出规则：
//!   - 值 dict 含 "meta" 键 → 跳过；DataFrame 含 image_url 列 → 跳过；
//!   - 各段以 "\n\n" 连接后写入 formalized_content。
//!
//! 注：pywencai 依赖 JS 生成的 `hexin-v` 反爬 token，Rust 侧无法计算，
//! 支持通过环境变量 IWENCAI_HEXIN_V（或节点参数 hexin_v）注入。

use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// 问财 robot 数据端点（与 pywencai 一致，http 而非 https）。
const WENCAI_ENDPOINT: &str = "http://www.iwencai.com/customized/chart/get-robot-data";

/// 选股表格分页数据端点路径（相对 footer url）。
const WENCAI_LANDING_PATH: &str = "/gateway/urp/v7/landing/getDataList";

/// 合法 query_type（对齐 RAGFlow wencai.py check_valid_value 列表）。
pub const WENCAI_QUERY_TYPES: &[&str] = &[
    "stock",
    "zhishu",
    "fund",
    "hkstock",
    "usstock",
    "threeboard",
    "conbond",
    "insurance",
    "futures",
    "lccp",
    "foreign_exchange",
];

/// add_info（对齐 pywencai 固定请求体字段）。
const ADD_INFO: &str = "{\"urp\":{\"scene\":1,\"company\":1,\"business\":1},\"contentType\":\"json\",\"searchInfo\":true}";
/// log_info（对齐 pywencai 固定请求体字段）。
const LOG_INFO: &str = "{\"input_type\":\"click\"}";
/// source（对齐 pywencai 固定请求体字段）。
const WENCAI_SOURCE: &str = "Ths_iwencai_Xuangu";

/// 问财查询请求。
#[derive(Debug, Clone)]
pub struct WenCaiRequest {
    /// 选股问题/条件（必填；为空时返回空串，对齐上游 `if not kwargs.get("query")`）。
    pub query: String,
    /// 返回行数上限（默认 10，钳制到 1..=100）。
    pub top_n: usize,
    /// 查询类型（默认 stock，需在 [`WENCAI_QUERY_TYPES`] 内）。
    pub query_type: String,
    /// 可选 cookie（原样透传给 iwencai）。
    pub cookie: String,
}

/// 问财客户端。
#[derive(Debug, Clone)]
pub struct WenCaiClient {
    http: reqwest::Client,
    /// 可选 `hexin-v` 反爬 token（env IWENCAI_HEXIN_V 或节点参数注入）。
    hexin_v: Option<String>,
}

impl Default for WenCaiClient {
    fn default() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(crate::common::cmd_timeout::duration())
                .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36")
                .build()
                .expect("wencai http client"),
            hexin_v: std::env::var("IWENCAI_HEXIN_V").ok().filter(|v| !v.trim().is_empty()),
        }
    }
}

impl WenCaiClient {
    /// 查询问财（对齐上游 `pywencai.get` + RAGFlow wencai.py 的 Markdown 输出）。
    pub async fn search(&self, request: &WenCaiRequest) -> Result<String> {
        self.search_at(request, WENCAI_ENDPOINT).await
    }

    /// 支持测试注入端点的内部实现。
    async fn search_at(&self, request: &WenCaiRequest, endpoint: &str) -> Result<String> {
        let query = request.query.trim().to_string();
        if query.is_empty() {
            // 对齐 RAGFlow：无 query 时输出空串
            return Ok(String::new());
        }
        let query_type = request.query_type.trim().to_string();
        if !WENCAI_QUERY_TYPES.contains(&query_type.as_str()) {
            bail!(
                "WenCai query_type must be one of: {}",
                WENCAI_QUERY_TYPES.join(", ")
            );
        }
        let top_n = request.top_n.clamp(1, 100);

        let payload = build_payload(&query, top_n, &query_type);
        let mut builder = self.http.post(endpoint).json(&payload);
        if let Some(token) = &self.hexin_v {
            builder = builder.header("hexin-v", token);
        }
        if !request.cookie.trim().is_empty() {
            builder = builder.header("cookie", request.cookie.trim());
        }
        let response = builder.send().await?;
        if !response.status().is_success() {
            bail!("WenCai HTTP {}", response.status());
        }
        let body: Value = response.json().await?;
        self.parse_answer(&body, request, top_n, &query_type, endpoint)
            .await
    }

    /// 解析 get-robot-data 响应并渲染 Markdown（对齐 pywencai convert + RAGFlow 输出规则）。
    async fn parse_answer(
        &self,
        body: &Value,
        request: &WenCaiRequest,
        top_n: usize,
        query_type: &str,
        endpoint: &str,
    ) -> Result<String> {
        let content = match answer_content(body) {
            Some(content) => content,
            None => return Ok(String::new()),
        };
        // content 可能是 JSON 字符串（需二次解析）或直接是对象
        let parsed = match content {
            Value::String(text) => serde_json::from_str::<Value>(text).unwrap_or(Value::Null),
            other => other.clone(),
        };
        let components = parsed
            .get("components")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if components.is_empty() {
            return Ok(String::new());
        }

        // 唯一 xuangu_tableV1 组件 → 走分页数据接口拿真实行（对齐 pywencai get_page）
        if components.len() == 1
            && components[0].get("show_type").and_then(Value::as_str) == Some("xuangu_tableV1")
        {
            if let Some(rows) = self
                .fetch_landing_rows(&components[0], request, top_n, query_type, endpoint)
                .await?
                && let Some(table) = render_rows_table(&rows) {
                    return Ok(table);
                }
            // 分页数据不可用时回退：渲染 condition/comp_id/uuid 字典
            let mut fallback = serde_json::Map::new();
            for key in ["condition", "comp_id", "uuid"] {
                if let Some(value) = components[0].get(key) {
                    fallback.insert(key.to_string(), value.clone());
                }
            }
            return Ok(render_object_table(&fallback));
        }

        // 多组件：按 show_type handler 渲染分段
        let sections = render_components(&components);
        Ok(sections.join("\n\n"))
    }

    /// 向 footer url（landing 分页接口）请求真实数据行（对齐 pywencai get_page）。
    async fn fetch_landing_rows(
        &self,
        comp: &Value,
        request: &WenCaiRequest,
        top_n: usize,
        query_type: &str,
        endpoint: &str,
    ) -> Result<Option<Vec<Value>>> {
        let footer_url = comp
            .get("config")
            .and_then(|config| config.get("other_info"))
            .and_then(|info| info.get("footer_info"))
            .and_then(|footer| footer.get("url"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if footer_url.is_empty() {
            return Ok(None);
        }
        let landing_url = if footer_url.starts_with("http://") || footer_url.starts_with("https://")
        {
            footer_url.to_string()
        } else {
            // 相对路径 → 拼接 robot 端点所在的 scheme://host
            match url::Url::parse(endpoint) {
                Ok(base) => format!(
                    "{}://{}{}",
                    base.scheme(),
                    base.host_str().unwrap_or("www.iwencai.com"),
                    footer_url
                ),
                Err(_) => return Ok(None),
            }
        };

        // 表单字段 = footer url 查询参数 ∪ {perpage, page} ∪ 原始请求字段（对齐 get_page 的 kwargs 合并）
        let mut form: BTreeMap<String, String> = BTreeMap::new();
        if let Ok(parsed) = url::Url::parse(&landing_url) {
            for (key, value) in parsed.query_pairs() {
                form.insert(key.into_owned(), value.into_owned());
            }
        }
        form.insert("perpage".into(), top_n.to_string());
        form.insert("page".into(), "1".into());
        form.insert("question".into(), request.query.trim().to_string());
        form.insert("secondary_intent".into(), query_type.to_string());
        form.insert("source".into(), WENCAI_SOURCE.to_string());
        form.insert("version".into(), "2.0".into());
        form.insert("add_info".into(), ADD_INFO.to_string());
        form.insert("log_info".into(), LOG_INFO.to_string());

        let mut builder = self.http.post(&landing_url).form(&form);
        if let Some(token) = &self.hexin_v {
            builder = builder.header("hexin-v", token);
        }
        let response = match builder.send().await {
            Ok(response) => response,
            Err(_) => return Ok(None),
        };
        if !response.status().is_success() {
            return Ok(None);
        }
        let body: Value = match response.json().await {
            Ok(body) => body,
            Err(_) => return Ok(None),
        };
        let rows = body
            .get("answer")
            .and_then(Value::as_array)
            .and_then(|answers| answers.first())
            .and_then(|answer| answer.get("components"))
            .and_then(Value::as_array)
            .and_then(|comps| comps.first())
            .and_then(|first| first.get("data"))
            .and_then(|data| data.get("datas"))
            .and_then(Value::as_array)
            .cloned();
        Ok(rows)
    }
}

/// 构建 get-robot-data 请求体（对齐 pywencai 字段与类型：perpage 为字符串）。
fn build_payload(query: &str, top_n: usize, query_type: &str) -> Value {
    json!({
        "add_info": ADD_INFO,
        "perpage": top_n.to_string(),
        "page": 1,
        "source": WENCAI_SOURCE,
        "log_info": LOG_INFO,
        "version": "2.0",
        "secondary_intent": query_type,
        "question": query,
    })
}

/// 提取 `data.answer[0].txt[0].content`（对齐 convert.py）。
fn answer_content(body: &Value) -> Option<&Value> {
    let answer = body.get("data")?.get("answer")?.as_array()?;
    let first = answer.first()?;
    let txt = first.get("txt")?.as_array()?;
    txt.first()?.get("content")
}

/// 组件 key（对齐 get_key：title_config.data.h1 || config.title || show_type）。
fn component_key(comp: &Value) -> String {
    comp.get("title_config")
        .and_then(|config| config.get("data"))
        .and_then(|data| data.get("h1"))
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
        .or_else(|| {
            comp.get("config")
                .and_then(|config| config.get("title"))
                .and_then(Value::as_str)
                .filter(|key| !key.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            comp.get("show_type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        })
}

/// 渲染多组件响应（对齐 multi_show_type_handler + RAGFlow wencai.py 输出规则）。
fn render_components(components: &[Value]) -> Vec<String> {
    let mut sections = Vec::new();
    for comp in components {
        let show_type = comp.get("show_type").and_then(Value::as_str).unwrap_or("");
        let key = component_key(comp);
        match show_type {
            "container" => {
                // 子组件按 uuid 查找，段标题用子组件 show_type（对齐 container_handler）
                let children = comp
                    .get("config")
                    .and_then(|config| config.get("children"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for uuid in children {
                    let child = components.iter().find(|candidate| {
                        candidate.get("uuid").and_then(Value::as_str) == uuid.as_str()
                            || candidate.get("puuid").and_then(Value::as_str) == uuid.as_str()
                    });
                    let Some(child) = child else { continue };
                    let child_type = child
                        .get("show_type")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if let Some(rendered) = render_component_value(child, components) {
                        sections.push(format!("{child_type}\n{rendered}"));
                    }
                }
            }
            "txt1" | "txt2" => {
                // 纯文本段（对齐 txt_handler）
                let content = comp
                    .get("data")
                    .and_then(|data| data.get("content"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if !content.is_empty() {
                    sections.push(format!("{key}\n{content}"));
                }
            }
            "tab1" | "tab4" => {
                // 多 tab 组件：每个 tab 下再按子组件 show_type 渲染
                let tabs = comp
                    .get("tab_list")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for tab in &tabs {
                    let tab_name = tab
                        .get("tab_name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let tab_components = tab
                        .get("list")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let mut tab_sections = Vec::new();
                    for tcomp in &tab_components {
                        if let Some(rendered) = render_component_value(tcomp, &tab_components) {
                            tab_sections.push(rendered);
                        }
                    }
                    if !tab_sections.is_empty() {
                        sections.push(format!("{tab_name}\n{}", tab_sections.join("\n\n")));
                    }
                }
            }
            "dragon_tiger_stock" => {
                // 龙虎榜：data.datas[0] 为单行
                if let Some(row) = comp
                    .get("data")
                    .and_then(|data| data.get("datas"))
                    .and_then(Value::as_array)
                    .and_then(|datas| datas.first())
                    .cloned()
                    && let Some(table) = render_rows_table(&[row]) {
                        sections.push(format!("{key}\n{table}"));
                    }
            }
            "textblocklinkone" => {
                // 文本块链接：data.result.data 为行数组
                if let Some(rows) = comp
                    .get("data")
                    .and_then(|data| data.get("result"))
                    .and_then(|result| result.get("data"))
                    .and_then(Value::as_array)
                    .cloned()
                    && let Some(table) = render_rows_table(&rows) {
                        sections.push(format!("{key}\n{table}"));
                    }
            }
            "nestedblocks" | "wiki1" => {
                // 需要额外 HTTP 拉取子页面，Rust 侧跳过（wiki1 上游亦已注释）
            }
            _ => {
                // common/未知类型：data.datas 数组 → 表格；否则 data 对象 → 两列表
                if let Some(rendered) = render_component_value(comp, components) {
                    sections.push(format!("{key}\n{rendered}"));
                }
            }
        }
    }
    sections
}

/// 渲染单个组件值（对齐 show_type_handler：datas 数组 → DataFrame；否则 data 原样）。
fn render_component_value(comp: &Value, _components: &[Value]) -> Option<String> {
    let data = comp.get("data")?;
    if let Some(rows) = data.get("datas").and_then(Value::as_array) {
        return render_rows_table(rows);
    }
    match data {
        Value::Object(map) => {
            // 对齐 RAGFlow：dict 含 "meta" 键 → 跳过
            if map.contains_key("meta") {
                return None;
            }
            Some(render_object_table(map))
        }
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        _ => None,
    }
}

/// 行数组 → Markdown 表格（对齐 pandas DataFrame.to_markdown 的 pipe 格式）。
/// 表头为各行键的并集（按首次出现顺序）；含 image_url 列时跳过（对齐上游）。
fn render_rows_table(rows: &[Value]) -> Option<String> {
    let mut headers: Vec<String> = Vec::new();
    for row in rows {
        if let Value::Object(map) = row {
            for key in map.keys() {
                if !headers.iter().any(|header| header == key) {
                    headers.push(key.clone());
                }
            }
        }
    }
    if headers.is_empty() {
        return None;
    }
    if headers.iter().any(|header| header == "image_url") {
        return None;
    }
    let mut out = format!("| index | {} |\n| --- |", headers.join(" | "));
    for _ in &headers {
        out.push_str(" --- |");
    }
    out.push('\n');
    for (index, row) in rows.iter().enumerate() {
        let mut cells = Vec::with_capacity(headers.len());
        for header in &headers {
            let value = row.get(header).map(format_cell).unwrap_or_default();
            cells.push(value);
        }
        out.push_str(&format!("| {index} | {} |\n", cells.join(" | ")));
    }
    Some(out)
}

/// 对象 → 两列 Markdown 表格（对齐 DataFrame.from_dict(orient='index') 的简化渲染）。
fn render_object_table(map: &serde_json::Map<String, Value>) -> String {
    let mut out = String::from("| key | value |\n| --- | --- |\n");
    for (key, value) in map {
        out.push_str(&format!(
            "| {} | {} |\n",
            escape_cell(key),
            format_cell(value)
        ));
    }
    out
}

/// 单元格格式化：标量原样，对象/数组 → 紧凑 JSON。
fn format_cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => escape_cell(text),
        Value::Object(_) | Value::Array(_) => {
            escape_cell(&serde_json::to_string(value).unwrap_or_default())
        }
        other => other.to_string(),
    }
}

/// Markdown 单元格转义：竖线转义、换行折叠。
fn escape_cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::Form, routing::post};
    use std::collections::HashMap;

    /// 构造 get-robot-data 响应：content 为 JSON 字符串或对象均可。
    fn robot_response(content: Value) -> Json<Value> {
        Json(json!({
            "status_code": 0,
            "data": {
                "answer": [
                    { "txt": [ { "content": content } ] }
                ]
            }
        }))
    }

    /// 标准选股表格组件（xuangu_tableV1）。
    fn xuangu_component(landing_url: &str) -> Value {
        json!({
            "show_type": "xuangu_tableV1",
            "cid": "123",
            "puuid": "uuid-1",
            "condition": {"stock": "涨跌幅>5%"},
            "data": {
                "meta": { "extra": { "condition": "涨跌幅>5%", "row_count": 2 } }
            },
            "config": {
                "title": "选股结果",
                "other_info": { "footer_info": { "url": landing_url } }
            }
        })
    }

    #[test]
    fn payload_serialization_matches_pywencai() {
        let payload = build_payload("贵州茅台 2024年报", 10, "stock");
        assert_eq!(payload["question"], "贵州茅台 2024年报");
        // perpage 是字符串（对齐 pywencai），page 是整数
        assert_eq!(payload["perpage"], "10");
        assert_eq!(payload["page"], 1);
        assert_eq!(payload["secondary_intent"], "stock");
        assert_eq!(payload["source"], "Ths_iwencai_Xuangu");
        assert_eq!(payload["version"], "2.0");
        assert!(
            payload["add_info"]
                .as_str()
                .unwrap()
                .contains("contentType")
        );
        assert!(payload["log_info"].as_str().unwrap().contains("input_type"));
    }

    #[test]
    fn payload_uses_top_n_string() {
        let payload = build_payload("q", 25, "fund");
        assert_eq!(payload["perpage"], "25");
        assert_eq!(payload["secondary_intent"], "fund");
    }

    #[tokio::test]
    async fn rejects_invalid_query_type() {
        let client = WenCaiClient::default();
        let error = client
            .search(&WenCaiRequest {
                query: "涨跌幅".into(),
                top_n: 10,
                query_type: "invalid_type".into(),
                cookie: String::new(),
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("WenCai query_type must be one of"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn empty_query_returns_empty() {
        let client = WenCaiClient::default();
        let output = client
            .search(&WenCaiRequest {
                query: "  ".into(),
                top_n: 10,
                query_type: "stock".into(),
                cookie: String::new(),
            })
            .await
            .unwrap();
        assert!(output.is_empty());
    }

    /// 完整链路：xuangu_tableV1 单组件 → landing 分页接口 → Markdown 表格。
    #[tokio::test]
    async fn xuangu_table_renders_rows_from_landing_api() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .route(
                "/customized/chart/get-robot-data",
                post(move |body: Json<Value>| async move {
                    assert_eq!(body["question"], "贵州茅台");
                    assert_eq!(body["secondary_intent"], "stock");
                    robot_response(json!({
                        "components": [xuangu_component(&format!("http://{addr}/gateway/urp/v7/landing/getDataList?question=test"))]
                    }))
                }),
            )
            .route(
                "/gateway/urp/v7/landing/getDataList",
                post(|form: Form<HashMap<String, String>>| async move {
                    // 表单契约：url_params ∪ perpage ∪ page ∪ 原始字段
                    assert_eq!(form.get("perpage").map(String::as_str), Some("10"));
                    assert_eq!(form.get("page").map(String::as_str), Some("1"));
                    Json(json!({
                        "answer": [
                            { "components": [ { "data": { "datas": [
                                { "股票代码": "600519", "股票名称": "贵州茅台", "涨跌幅": "1.23%" },
                                { "股票代码": "000858", "股票名称": "五粮液", "涨跌幅": "-0.45%" }
                            ] } } ] }
                        ]
                    }))
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = WenCaiClient {
            http: reqwest::Client::builder()
                .timeout(crate::common::cmd_timeout::duration())
                .build()
                .unwrap(),
            hexin_v: None,
        };
        let output = client
            .search_at(
                &WenCaiRequest {
                    query: "贵州茅台".into(),
                    top_n: 10,
                    query_type: "stock".into(),
                    cookie: String::new(),
                },
                &format!("http://{addr}/customized/chart/get-robot-data"),
            )
            .await
            .unwrap();
        assert!(output.starts_with("| index |"), "{output}");
        assert!(output.contains("600519"));
        assert!(output.contains("贵州茅台"));
        assert!(output.contains("五粮液"));
        assert!(output.contains("涨跌幅"));
    }

    /// 多组件（common）响应：data.datas 行 → 表格；含 image_url 列 → 跳过。
    #[tokio::test]
    async fn multi_component_common_renders_and_skips_image_url() {
        let app = Router::new().route(
            "/customized/chart/get-robot-data",
            post(|_: Json<Value>| async {
                robot_response(json!({
                    "components": [
                        {
                            "show_type": "common",
                            "title_config": { "data": { "h1": "业绩预告" } },
                            "data": { "datas": [
                                { "股票名称": "宁德时代", "业绩": "预增" },
                                { "股票名称": "比亚迪", "业绩": "预增" }
                            ] }
                        },
                        {
                            "show_type": "common",
                            "config": { "title": "图表" },
                            "data": { "datas": [
                                { "image_url": "http://x/1.png", "股票名称": "A" }
                            ] }
                        }
                    ]
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = WenCaiClient::default();
        let output = client
            .search_at(
                &WenCaiRequest {
                    query: "业绩预增".into(),
                    top_n: 10,
                    query_type: "stock".into(),
                    cookie: String::new(),
                },
                &format!("http://{addr}/customized/chart/get-robot-data"),
            )
            .await
            .unwrap();
        assert!(output.contains("业绩预告"));
        assert!(output.contains("宁德时代"));
        assert!(output.contains("比亚迪"));
        // image_url 段被跳过
        assert!(!output.contains("image_url"));
    }

    /// dict 值含 "meta" 键 → 跳过；txt 组件 → 文本段。
    #[tokio::test]
    async fn meta_dict_skipped_and_txt_rendered() {
        let app = Router::new().route(
            "/customized/chart/get-robot-data",
            post(|_: Json<Value>| async {
                robot_response(json!({
                    "components": [
                        {
                            "show_type": "common",
                            "config": { "title": "含meta" },
                            "data": { "meta": { "x": 1 }, "datas": [] }
                        },
                        {
                            "show_type": "txt1",
                            "config": { "title": "资讯" },
                            "data": { "content": "今日两市成交额突破万亿" }
                        }
                    ]
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = WenCaiClient::default();
        let output = client
            .search_at(
                &WenCaiRequest {
                    query: "资讯".into(),
                    top_n: 10,
                    query_type: "stock".into(),
                    cookie: String::new(),
                },
                &format!("http://{addr}/customized/chart/get-robot-data"),
            )
            .await
            .unwrap();
        assert!(!output.contains("含meta"), "{output}");
        assert!(output.contains("今日两市成交额突破万亿"));
    }

    /// content 为 JSON 字符串时也能解析（对齐 convert.py 的二次 json.loads）。
    #[test]
    fn content_as_json_string_is_parsed() {
        let content = json!({
            "components": [
                {
                    "show_type": "common",
                    "config": { "title": "字符串content" },
                    "data": { "datas": [ { "代码": "600000" } ] }
                }
            ]
        });
        let body = json!({
            "data": { "answer": [ { "txt": [ { "content": content.to_string() } ] } ] }
        });
        let parsed = answer_content(&body).unwrap();
        let value = match parsed {
            Value::String(text) => serde_json::from_str::<Value>(text).unwrap(),
            other => other.clone(),
        };
        let components = value.get("components").and_then(Value::as_array).unwrap();
        let sections = render_components(components);
        assert_eq!(sections.len(), 1);
        assert!(sections[0].contains("600000"));
    }

    /// 行渲染：竖线转义 + 表头并集。
    #[test]
    fn rows_table_escapes_pipes_and_unions_headers() {
        let rows = vec![
            json!({ "a": "x|y", "b": 1 }),
            json!({ "a": "z", "b": 2, "c": true }),
        ];
        let table = render_rows_table(&rows).unwrap();
        assert!(table.starts_with("| index | a | b | c |"));
        assert!(table.contains("x\\|y"));
        assert!(table.contains("true"));
    }

    #[test]
    fn rows_table_skips_image_url_columns() {
        let rows = vec![json!({ "image_url": "http://x", "a": 1 })];
        assert!(render_rows_table(&rows).is_none());
    }

    /// 真实网络测试（默认忽略）。
    #[tokio::test]
    #[ignore]
    async fn live_query_returns_markdown() {
        let client = WenCaiClient::default();
        let output = client
            .search(&WenCaiRequest {
                query: "2024年净利润增长超过50%的股票".into(),
                top_n: 5,
                query_type: "stock".into(),
                cookie: String::new(),
            })
            .await
            .unwrap();
        assert!(!output.is_empty());
    }
}
