//! Crawler 连接器 — RAGFlow `crawler.py`（crawl4ai）的 Rust 实现
//!
//! 对齐上游语义：
//! - 输入 content（数组以 " - " 连接）视为 URL，先过 SSRF 防护（scheme + 公网 IP 校验）
//! - `extract_type` 三选一：`html`（清洗后 HTML）/ `markdown`（HTML→Markdown 转换）/
//!   `content`（纯文本），默认 `markdown`
//! - 失败返回 "URL not valid"（SSRF 拒绝）或 "An unexpected error occurred: ..." 前缀
//!
//! 中国大陆网络适配：抓取本身走 reqwest（可配代理）；无外网时也可抓取国内站点。

use anyhow::{Result, anyhow, bail};
use reqwest::Client;
use scraper::{Html, Selector};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Crawler 组件参数，对齐 CrawlerParam（proxy / extract_type）。
#[derive(Debug, Clone)]
pub struct CrawlerRequest {
    pub url: String,
    pub extract_type: String,
    pub proxy: Option<String>,
}

impl Default for CrawlerRequest {
    fn default() -> Self {
        Self {
            url: String::new(),
            extract_type: "markdown".into(),
            proxy: None,
        }
    }
}

/// 连接器：抓取网页并按 extract_type 转换。
#[derive(Debug, Clone)]
pub struct CrawlerClient {
    client: Client,
}

impl Default for CrawlerClient {
    fn default() -> Self {
        Self::new()
    }
}

impl CrawlerClient {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(crate::common::cmd_timeout::duration())
                .user_agent("Mozilla/5.0 (compatible; RayRAG-Crawler/1.0)")
                .redirect(reqwest::redirect::Policy::limited(5))
                .build()
                .expect("build crawler client"),
        }
    }

    /// 抓取并转换。返回与上游 be_output 一致的正文。
    pub async fn fetch(&self, request: &CrawlerRequest) -> Result<String> {
        let url = reqwest::Url::parse(request.url.trim()).map_err(|_| anyhow!("URL not valid"))?;
        assert_url_is_safe(&url)
            .await
            .map_err(|_| anyhow!("URL not valid"))?;

        let raw = match &request.proxy {
            Some(proxy) if !proxy.trim().is_empty() => {
                let client = Client::builder()
                    .timeout(crate::common::cmd_timeout::duration())
                    .proxy(reqwest::Proxy::all(proxy)?)
                    .build()?;
                client.get(url).send().await?.error_for_status()?
            }
            _ => self.client.get(url).send().await?.error_for_status()?,
        };
        // 限制响应体大小（对齐 crawl4ai 的默认抓取范围，防 OOM）。这里真的执行上限：
        // 之前只有注释，`bytes()` 会把整页（以及无限流）全缓存进内存。
        let body = crate::common::cmd_timeout::read_body_limited(
            raw,
            crate::common::cmd_timeout::body_limit_bytes(),
            "Crawler",
        )
        .await?;
        let html = String::from_utf8_lossy(&body).into_owned();

        let extract_type = request.extract_type.trim().to_ascii_lowercase();
        match extract_type.as_str() {
            "html" => Ok(clean_html(&html)),
            "content" => Ok(strip_all_tags(&html)),
            _ => Ok(html_to_markdown(&html)),
        }
    }
}

/// 校验 URL：仅 http/https 且解析到公网地址（对齐 common/ssrf_guard.py assert_url_is_safe）。
async fn assert_url_is_safe(url: &reqwest::Url) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") {
        bail!("URL scheme is not allowed");
    }
    let raw_hostname = url
        .host_str()
        .filter(|hostname| !hostname.is_empty())
        .ok_or_else(|| anyhow!("URL is missing a host"))?;
    let hostname = raw_hostname
        .strip_prefix('[')
        .and_then(|hostname| hostname.strip_suffix(']'))
        .unwrap_or(raw_hostname);
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow!("URL has no usable port"))?;
    let addresses = if let Ok(ip) = hostname.parse::<IpAddr>() {
        vec![ip]
    } else {
        tokio::net::lookup_host((hostname, port))
            .await?
            .map(|addr| addr.ip())
            .collect::<Vec<_>>()
    };
    if addresses.is_empty() || addresses.iter().any(|ip| !ip_is_public(*ip)) {
        bail!("URL resolves to a non-public address");
    }
    Ok(())
}

fn ip_is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ipv4_is_public(ip),
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(ipv4_is_public)
            .unwrap_or_else(|| ipv6_is_public(ip)),
    }
}

fn ipv4_is_public(ip: Ipv4Addr) -> bool {
    ![
        (Ipv4Addr::new(0, 0, 0, 0), 8),
        (Ipv4Addr::new(10, 0, 0, 0), 8),
        (Ipv4Addr::new(100, 64, 0, 0), 10),
        (Ipv4Addr::new(127, 0, 0, 0), 8),
        (Ipv4Addr::new(169, 254, 0, 0), 16),
        (Ipv4Addr::new(172, 16, 0, 0), 12),
        (Ipv4Addr::new(192, 0, 0, 0), 24),
        (Ipv4Addr::new(192, 0, 2, 0), 24),
        (Ipv4Addr::new(192, 88, 99, 0), 24),
        (Ipv4Addr::new(192, 168, 0, 0), 16),
        (Ipv4Addr::new(198, 18, 0, 0), 15),
        (Ipv4Addr::new(198, 51, 100, 0), 24),
        (Ipv4Addr::new(203, 0, 113, 0), 24),
        (Ipv4Addr::new(224, 0, 0, 0), 4),
        (Ipv4Addr::new(240, 0, 0, 0), 4),
    ]
    .iter()
    .any(|(network, prefix)| ipv4_has_prefix(ip, *network, *prefix))
}

fn ipv4_has_prefix(ip: Ipv4Addr, network: Ipv4Addr, prefix: u32) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    (u32::from(ip) & mask) == (u32::from(network) & mask)
}

fn ipv6_is_public(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    !(segments[0] & 0xfe00 == 0xfc00 // fc00::/7 unique local
        || segments[0] & 0xffc0 == 0xfe80 // fe80::/10 link-local
        || segments[0] == 0 // ::/8 (including ::1)
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)) // documentation range
}

/// 清洗 HTML：移除 script/style/svg/head 等内容（对齐 crawl4ai cleaned_html 的意图）。
fn clean_html(html: &str) -> String {
    let document = Html::parse_document(html);
    let root = document.root_element();
    let mut out = String::new();
    walk_clean(&root, &mut out);
    compact_whitespace(&out)
}

fn walk_clean(node: &scraper::ElementRef<'_>, out: &mut String) {
    for child in node.children() {
        use scraper::Node;
        match child.value() {
            Node::Element(element) => {
                let name = element.name();
                if matches!(name, "script" | "style" | "svg" | "head" | "noscript") {
                    continue; // 跳过整棵子树
                }
                walk_clean(&scraper::ElementRef::wrap(child).expect("element ref"), out);
            }
            Node::Text(text) => out.push_str(&text.text),
            _ => {}
        }
    }
}

/// 纯文本提取（对齐 extracted_content）。
fn strip_all_tags(html: &str) -> String {
    let document = Html::parse_document(html);
    let root = document.root_element();
    let mut out = String::new();
    walk_clean(&root, &mut out);
    compact_whitespace(&out)
}

/// 轻量 HTML→Markdown 转换：标题/段落/链接/列表/粗斜体/代码块（对齐 crawl4ai markdown 的核心形态）。
fn html_to_markdown(html: &str) -> String {
    let document = Html::parse_document(html);
    let root = document.root_element();
    let body = root
        .select(&Selector::parse("body").expect("body selector"))
        .next()
        .unwrap_or(root);

    let mut out = String::new();
    walk_markdown(&body, &document, &mut out);
    compact_whitespace_preserve_newlines(&out)
}

fn walk_markdown(node: &scraper::ElementRef<'_>, document: &Html, out: &mut String) {
    for child in node.children() {
        use scraper::Node;
        let element = match child.value() {
            Node::Element(element) => element,
            Node::Text(text) => {
                let text = text.text.trim();
                if !text.is_empty() {
                    out.push_str(text);
                    out.push(' ');
                }
                continue;
            }
            _ => continue,
        };
        let name = element.name().to_string();
        let element_ref = scraper::ElementRef::wrap(child).expect("element ref");
        if matches!(
            name.as_str(),
            "script" | "style" | "head" | "noscript" | "svg"
        ) {
            continue;
        }
        match name.as_str() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                out.push('\n');
                out.push_str(&"#".repeat(name[1..].parse::<usize>().unwrap_or(1)));
                out.push(' ');
                for text in element_ref.text() {
                    out.push_str(text);
                }
                out.push('\n');
            }
            "p" | "div" | "section" | "article" | "li" | "tr" | "br" => {
                out.push('\n');
                walk_markdown(&element_ref, document, out);
            }
            "a" => {
                let text = element_ref.text().collect::<String>().trim().to_string();
                let href = element.attr("href").unwrap_or_default();
                if !text.is_empty() {
                    if href.starts_with("http://") || href.starts_with("https://") {
                        out.push_str(&format!("[{text}]({href})"));
                    } else {
                        out.push_str(&text);
                    }
                    out.push(' ');
                }
            }
            "strong" | "b" => {
                let text = element_ref.text().collect::<String>().trim().to_string();
                if !text.is_empty() {
                    out.push_str(&format!("**{text}**"));
                    out.push(' ');
                }
            }
            "em" | "i" => {
                let text = element_ref.text().collect::<String>().trim().to_string();
                if !text.is_empty() {
                    out.push_str(&format!("*{text}*"));
                    out.push(' ');
                }
            }
            "pre" => {
                out.push_str("\n```\n");
                for text in element_ref.text() {
                    out.push_str(text);
                }
                out.push_str("\n```\n");
            }
            "code" => {
                let text = element_ref.text().collect::<String>().trim().to_string();
                if !text.is_empty() {
                    out.push('`');
                    out.push_str(&text);
                    out.push('`');
                }
            }
            "img" => {
                let alt = element.attr("alt").unwrap_or_default();
                let src = element.attr("src").unwrap_or_default();
                if !alt.is_empty() {
                    out.push_str(&format!("![{alt}]({src})"));
                    out.push(' ');
                }
            }
            "ul" | "ol" | "table" | "blockquote" => {
                out.push('\n');
                walk_markdown(&element_ref, document, out);
            }
            _ => {
                walk_markdown(&element_ref, document, out);
            }
        }
    }
}

fn compact_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn compact_whitespace_preserve_newlines(text: &str) -> String {
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};

    const SAMPLE_HTML: &str = r#"<html><head><title>T</title><style>.x{}</style></head>
<body><h1>标题</h1><p>一段<b>加粗</b>文本 <a href="https://example.com/link">链接</a></p>
<pre><code>let x = 1;</code></pre><script>alert(1)</script></body></html>"#;

    #[tokio::test]
    async fn markdown_mode_converts_headings_links_and_code() {
        let md = html_to_markdown(SAMPLE_HTML);
        assert!(md.contains("# 标题"));
        assert!(md.contains("**加粗**"));
        assert!(md.contains("[链接](https://example.com/link)"));
        assert!(md.contains("let x = 1;"));
        assert!(!md.contains("alert(1)"));
    }

    #[tokio::test]
    async fn content_mode_strips_all_tags() {
        let content = strip_all_tags(SAMPLE_HTML);
        assert!(content.contains("标题"));
        assert!(content.contains("加粗"));
        assert!(!content.contains("<b>"));
        assert!(!content.contains("alert(1)"));
    }

    #[tokio::test]
    async fn clean_html_removes_scripts_and_styles() {
        let cleaned = clean_html(SAMPLE_HTML);
        assert!(!cleaned.contains("alert(1)"));
        assert!(!cleaned.contains(".x{}"));
        assert!(cleaned.contains("加粗"));
    }

    #[tokio::test]
    async fn fetch_rejects_private_loopback_addresses() {
        // 127.0.0.1 属于私网/回环，应被 SSRF 防护拒绝
        let client = CrawlerClient::new();
        let result = client
            .fetch(&CrawlerRequest {
                url: "http://127.0.0.1:9/x".into(),
                extract_type: "markdown".into(),
                proxy: None,
            })
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("URL not valid"));
    }

    #[tokio::test]
    async fn fetch_rejects_invalid_scheme() {
        let client = CrawlerClient::new();
        let result = client
            .fetch(&CrawlerRequest {
                url: "file:///etc/passwd".into(),
                extract_type: "markdown".into(),
                proxy: None,
            })
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("URL not valid"));
    }
}
