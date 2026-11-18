//! Small runtime wrappers shared by ingestion and Canvas orchestration.
//!
//! Rust futures provide structured cancellation: dropping a parent future
//! drops the child operation at its next suspension point. `with_timeout`
//! adds a deadline to that model without recreating Go's context plumbing.

use anyhow::Result;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::future::Future;
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Component domains exposed by the catalog API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComponentCategory {
    Agent,
    Ingestion,
    Shared,
}

impl ComponentCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Ingestion => "ingestion",
            Self::Shared => "shared",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "agent" => Some(Self::Agent),
            "ingestion" => Some(Self::Ingestion),
            "shared" => Some(Self::Shared),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct StaticComponentDescriptor {
    name: &'static str,
    category: ComponentCategory,
    inputs: &'static [(&'static str, &'static str)],
    outputs: &'static [(&'static str, &'static str)],
}

/// JSON-friendly component catalog record.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ComponentDescriptor {
    pub name: String,
    pub category: String,
    pub inputs: BTreeMap<String, String>,
    pub outputs: BTreeMap<String, String>,
}

const NO_FIELDS: &[(&str, &str)] = &[];
const LLM_INPUTS: &[(&str, &str)] = &[
    (
        "llm_id",
        "Tenant model selector: model@provider or model@instance@provider",
    ),
    (
        "model_id",
        "Go component alias for the tenant model selector",
    ),
    ("sys_prompt", "System prompt template"),
    ("prompts", "Ordered role/content prompt templates"),
    (
        "message_history_window_size",
        "Prior conversation turns included before current prompts",
    ),
    ("temperature", "Optional sampling temperature"),
    ("top_p", "Optional nucleus sampling threshold"),
    ("presence_penalty", "Optional presence penalty"),
    ("frequency_penalty", "Optional frequency penalty"),
    ("max_tokens", "Optional provider output-token cap"),
    ("max_retries", "Additional model attempts after an error"),
    (
        "outputs.structured",
        "JSON schema enabling structured response parsing",
    ),
];
const LLM_OUTPUTS: &[(&str, &str)] = &[
    ("content", "Assistant text response"),
    ("structured", "Parsed structured JSON response"),
    ("_ERROR", "Terminal provider or structured parsing error"),
];
const AGENT_INPUTS: &[(&str, &str)] = &[
    (
        "llm_id",
        "Tenant model selector: model@provider or model@instance@provider",
    ),
    (
        "model_id",
        "Go component alias for the tenant model selector",
    ),
    ("sys_prompt", "System prompt template"),
    ("prompts", "Ordered role/content prompt templates"),
    (
        "message_history_window_size",
        "Prior conversation turns included before current prompts",
    ),
    ("temperature", "Optional sampling temperature"),
    ("top_p", "Optional nucleus sampling threshold"),
    ("presence_penalty", "Optional presence penalty"),
    ("frequency_penalty", "Optional frequency penalty"),
    ("max_tokens", "Optional provider output-token cap"),
    ("max_retries", "Additional model attempts after an error"),
    (
        "outputs.structured",
        "JSON schema enabling structured response parsing",
    ),
    (
        "tools",
        "Ordered object-shaped Agent tools; Retrieval, TavilySearch, TavilyExtract, DuckDuckGo, Wikipedia, GoogleScholar, GitHub, YahooFinance, ArXiv and PubMed are implemented",
    ),
    (
        "mcp",
        "MCP tool bindings; non-empty bindings currently fail closed",
    ),
    ("max_rounds", "Maximum tool-calling rounds before fallback"),
];
const AGENT_OUTPUTS: &[(&str, &str)] = &[
    ("content", "Final assistant text response"),
    ("structured", "Parsed structured JSON response"),
    ("tool_calls", "Observed OpenAI-compatible function calls"),
    ("_ERROR", "Terminal provider or structured parsing error"),
];
const TAVILY_SEARCH_INPUTS: &[(&str, &str)] = &[
    (
        "query",
        "Search query or Canvas selector; defaults to sys.query",
    ),
    (
        "api_key",
        "Static Tavily credential; TAVILY_API_KEY is the fallback",
    ),
    ("search_depth", "basic or advanced"),
    ("topic", "general or news"),
    ("max_results", "Positive result limit"),
    ("days", "Positive news lookback window"),
    ("include_answer", "Ask Tavily for its synthesized answer"),
    (
        "include_raw_content",
        "Accepted for DSL parity but forced off at execution",
    ),
    (
        "include_images",
        "Accepted for DSL parity but forced off at execution",
    ),
    (
        "include_image_descriptions",
        "Ask for image descriptions without image payloads",
    ),
    ("include_domains", "Allowed-domain string array"),
    ("exclude_domains", "Excluded-domain string array"),
];
const DUCKDUCKGO_INPUTS: &[(&str, &str)] = &[
    (
        "query",
        "Search query or Canvas selector; defaults to sys.query",
    ),
    (
        "channel",
        "text/news static channel or general/news Agent override",
    ),
    ("top_n", "Positive result limit; defaults to 10"),
    ("max_retries", "Additional search attempts after an error"),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const DUCKDUCKGO_OUTPUTS: &[(&str, &str)] = &[
    (
        "formalized_content",
        "RAGFlow kb_prompt-style DuckDuckGo results",
    ),
    ("json", "Python-compatible raw text/news result rows"),
    ("doc_aggs", "DuckDuckGo-result document aggregations"),
    ("_ERROR", "Terminal DuckDuckGo request or parsing error"),
];
const TAVILY_SEARCH_OUTPUTS: &[(&str, &str)] = &[
    ("formalized_content", "RAGFlow kb_prompt-style web results"),
    ("json", "Raw Tavily result objects"),
    ("doc_aggs", "Web-result document aggregations"),
    ("_ERROR", "Terminal Tavily configuration or request error"),
];
const TAVILY_EXTRACT_INPUTS: &[(&str, &str)] = &[
    (
        "urls",
        "URL string, comma-separated string, or string array",
    ),
    (
        "api_key",
        "Static Tavily credential; TAVILY_API_KEY is the fallback",
    ),
    ("extract_depth", "basic or advanced"),
    ("format", "markdown or text"),
    (
        "include_images",
        "Accepted for DSL parity but forced off at execution",
    ),
];
const TAVILY_EXTRACT_OUTPUTS: &[(&str, &str)] = &[
    ("json", "Raw Tavily extraction result objects"),
    ("_ERROR", "Terminal Tavily configuration or request error"),
];
const WIKIPEDIA_INPUTS: &[(&str, &str)] = &[
    (
        "query",
        "Specific Wikipedia subject or Canvas selector; defaults to sys.query",
    ),
    (
        "language",
        "Fixed Wikipedia language-code allowlist; defaults to en",
    ),
    ("top_n", "Positive result limit; defaults to 10"),
    (
        "max_retries",
        "Additional outer search attempts after an error",
    ),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const WIKIPEDIA_OUTPUTS: &[(&str, &str)] = &[
    (
        "formalized_content",
        "RAGFlow kb_prompt-style Wikipedia summaries",
    ),
    (
        "json",
        "Go-compatible results envelope of title/snippet/url rows",
    ),
    ("doc_aggs", "Wikipedia-result document aggregations"),
    ("_ERROR", "Terminal Wikipedia request error"),
];
const BAIKE_INPUTS: &[(&str, &str)] = &[
    (
        "query",
        "Specific Baidu Baike subject or Canvas selector; defaults to sys.query",
    ),
    ("top_n", "Positive result limit; defaults to 10"),
    (
        "max_retries",
        "Additional outer search attempts after an error",
    ),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const BAIKE_OUTPUTS: &[(&str, &str)] = &[
    (
        "formalized_content",
        "RAGFlow kb_prompt-style Baidu Baike summaries",
    ),
    (
        "json",
        "Go-compatible results envelope of title/snippet/url rows",
    ),
    ("doc_aggs", "Baidu Baike-result document aggregations"),
    ("_ERROR", "Terminal Baidu Baike request error"),
];
const GOOGLE_INPUTS: &[(&str, &str)] = &[
    (
        "q",
        "Google query or Canvas selector; defaults to sys.query",
    ),
    ("api_key", "Required SerpApi API key"),
    (
        "country",
        "Exact Google country-code allowlist; defaults to cn",
    ),
    (
        "language",
        "Exact Google language-code allowlist; defaults to en",
    ),
    (
        "start",
        "Integer pagination offset accepted by the DSL but unused upstream",
    ),
    (
        "num",
        "Integer result limit accepted by the DSL but unused upstream",
    ),
    (
        "max_retries",
        "Additional outer search attempts after an error",
    ),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const GOOGLE_OUTPUTS: &[(&str, &str)] = &[
    (
        "formalized_content",
        "RAGFlow kb_prompt-style Google organic results",
    ),
    ("json", "Raw SerpApi organic result objects"),
    ("doc_aggs", "Google-result document aggregations"),
    ("_ERROR", "Terminal Google request or parsing error"),
];
const GOOGLE_SCHOLAR_INPUTS: &[(&str, &str)] = &[
    (
        "query",
        "Google Scholar query or Canvas selector; defaults to sys.query",
    ),
    ("top_n", "Positive result limit; defaults to 12"),
    ("sort_by", "relevance or date; defaults to relevance"),
    ("year_low", "Optional inclusive lower publication year"),
    ("year_high", "Optional inclusive upper publication year"),
    (
        "patents",
        "Whether patent results are included; defaults to true",
    ),
    (
        "max_retries",
        "Additional outer search attempts after an error",
    ),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const GOOGLE_SCHOLAR_OUTPUTS: &[(&str, &str)] = &[
    (
        "formalized_content",
        "RAGFlow kb_prompt-style Google Scholar publication references",
    ),
    (
        "json",
        "Python scholarly-compatible core publication objects",
    ),
    ("doc_aggs", "Google Scholar-result document aggregations"),
    ("_ERROR", "Terminal Google Scholar request or parsing error"),
];
const GITHUB_INPUTS: &[(&str, &str)] = &[
    (
        "query",
        "GitHub repository query or Canvas selector; defaults to sys.query",
    ),
    (
        "top_n",
        "Positive result limit; Python backend defaults to 10 and the web UI seeds 5",
    ),
    (
        "max_retries",
        "Additional outer search attempts after an error",
    ),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const GITHUB_OUTPUTS: &[(&str, &str)] = &[
    (
        "formalized_content",
        "RAGFlow kb_prompt-style GitHub repository references",
    ),
    ("json", "Python-compatible raw GitHub repository items"),
    ("doc_aggs", "GitHub-result document aggregations"),
    ("_ERROR", "Terminal GitHub request or response error"),
];
const YAHOO_FINANCE_INPUTS: &[(&str, &str)] = &[
    (
        "stock_code",
        "Stock code, company name or Canvas selector; defaults to sys.query",
    ),
    (
        "info",
        "Render the yfinance information series; defaults true",
    ),
    ("history", "Render one month of daily market history"),
    (
        "count",
        "Validated for Python parity but not consumed by yahoofinance.py",
    ),
    (
        "financials",
        "Render the Yahoo calendar (the fixed Python component's historical label)",
    ),
    (
        "income_stmt",
        "Validated for Python parity but not consumed by yahoofinance.py",
    ),
    (
        "balance_sheet",
        "Render annual and quarterly balance sheets",
    ),
    (
        "cash_flow_statement",
        "Render annual and quarterly cash-flow statements",
    ),
    (
        "news",
        "Render the latest non-advertisement news; defaults true",
    ),
    (
        "max_retries",
        "Additional outer report attempts after an error",
    ),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const YAHOO_FINANCE_OUTPUTS: &[(&str, &str)] = &[
    ("report", "Python-compatible Markdown market-data report"),
    ("_ERROR", "Terminal Yahoo Finance request or response error"),
];
const ARXIV_INPUTS: &[(&str, &str)] = &[
    (
        "query",
        "arXiv query or Canvas selector; defaults to sys.query",
    ),
    ("top_n", "Positive result limit; defaults to 12"),
    (
        "sort_by",
        "submittedDate, lastUpdatedDate or relevance; Python default is submittedDate",
    ),
    (
        "max_retries",
        "Additional outer search attempts after an error",
    ),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const ARXIV_OUTPUTS: &[(&str, &str)] = &[
    (
        "formalized_content",
        "RAGFlow kb_prompt-style arXiv summaries",
    ),
    (
        "json",
        "Go-compatible results envelope with authors and PDF URLs",
    ),
    ("doc_aggs", "arXiv-result document aggregations"),
    ("_ERROR", "Terminal arXiv request or parsing error"),
];
const PUBMED_INPUTS: &[(&str, &str)] = &[
    (
        "query",
        "PubMed query or Canvas selector; defaults to sys.query",
    ),
    ("top_n", "Positive result limit; defaults to 12"),
    (
        "email",
        "NCBI contact email; Python fallback is A.N.Other@example.com",
    ),
    (
        "max_retries",
        "Additional outer search attempts after an error",
    ),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const PUBMED_OUTPUTS: &[(&str, &str)] = &[
    (
        "formalized_content",
        "RAGFlow kb_prompt-style PubMed article references",
    ),
    (
        "json",
        "Go-compatible results envelope with PMID, authors, journal and year",
    ),
    ("doc_aggs", "PubMed-result document aggregations"),
    ("_ERROR", "Terminal PubMed request or parsing error"),
];
const BING_INPUTS: &[(&str, &str)] = &[
    (
        "query",
        "Bing query or Canvas selector; defaults to sys.query",
    ),
    ("top_n", "Positive result limit; defaults to 10"),
    (
        "max_retries",
        "Additional outer search attempts after an error",
    ),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const BING_OUTPUTS: &[(&str, &str)] = &[
    (
        "formalized_content",
        "RAGFlow-style Bing search references (title/link/snippet)",
    ),
    ("json", "Go-compatible results envelope with Bing rows"),
    ("_ERROR", "Terminal Bing request or parsing error"),
];
const BAIDU_INPUTS: &[(&str, &str)] = &[
    (
        "query",
        "Baidu query or Canvas selector; defaults to sys.query",
    ),
    ("top_n", "Positive result limit; defaults to 10"),
    (
        "max_retries",
        "Additional outer search attempts after an error",
    ),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const BAIDU_OUTPUTS: &[(&str, &str)] = &[
    (
        "formalized_content",
        "RAGFlow-style Baidu search references (title/link/snippet)",
    ),
    ("json", "Go-compatible results envelope with Baidu rows"),
    ("_ERROR", "Terminal Baidu request or parsing error"),
];
const BOCHA_INPUTS: &[(&str, &str)] = &[
    (
        "query",
        "Bocha query or Canvas selector; defaults to sys.query",
    ),
    ("top_n", "Positive result limit; defaults to 10"),
    (
        "max_retries",
        "Additional outer search attempts after an error",
    ),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const BOCHA_OUTPUTS: &[(&str, &str)] = &[
    (
        "formalized_content",
        "RAGFlow-style Bocha search references (title/link/snippet)",
    ),
    ("json", "Go-compatible results envelope with Bocha rows"),
    ("_ERROR", "Terminal Bocha request or parsing error"),
];
const TENCENT_FINANCE_INPUTS: &[(&str, &str)] = &[
    (
        "symbol",
        "Tencent symbol (e.g. sh600519) or Canvas selector; defaults to sys.query",
    ),
    (
        "max_retries",
        "Additional outer search attempts after an error",
    ),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const TENCENT_FINANCE_OUTPUTS: &[(&str, &str)] = &[
    (
        "formalized_content",
        "RAGFlow-style Tencent quote row (name/price/change/high/low)",
    ),
    ("json", "Go-compatible results envelope with Tencent quote"),
    (
        "_ERROR",
        "Terminal Tencent Finance request or parsing error",
    ),
];
const BAIDU_SCHOLAR_INPUTS: &[(&str, &str)] = &[
    (
        "query",
        "Baidu Scholar query or Canvas selector; defaults to sys.query",
    ),
    ("top_n", "Positive result limit; defaults to 10"),
    (
        "max_retries",
        "Additional outer search attempts after an error",
    ),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const BAIDU_SCHOLAR_OUTPUTS: &[(&str, &str)] = &[
    (
        "formalized_content",
        "RAGFlow-style Baidu Scholar references (title/link/snippet)",
    ),
    ("json", "Go-compatible results envelope with Scholar rows"),
    ("_ERROR", "Terminal Baidu Scholar request or parsing error"),
];
const EASTMONEY_INPUTS: &[(&str, &str)] = &[
    (
        "symbol",
        "EastMoney A-share news symbol/company keyword or Canvas selector; defaults to sys.query",
    ),
    ("top_n", "Positive result limit; defaults to 10"),
    (
        "max_retries",
        "Additional outer search attempts after an error",
    ),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const EASTMONEY_OUTPUTS: &[(&str, &str)] = &[
    (
        "formalized_content",
        "RAGFlow-style EastMoney news references (title/link/snippet)",
    ),
    ("json", "Go-compatible results envelope with EastMoney rows"),
    ("_ERROR", "Terminal EastMoney request or parsing error"),
];
const JIN10_INPUTS: &[(&str, &str)] = &[
    (
        "type",
        "Jin10 data type: flash/calendar/symbols/news; defaults to flash",
    ),
    (
        "query",
        "Jin10 filter keyword or Canvas selector; defaults to sys.query",
    ),
    (
        "secret_key",
        "Jin10 open-data secret key; JIN10_SECRET_KEY is the fallback",
    ),
    (
        "flash_type",
        "Flash category 1..=5 (RAGFlow flash_type; defaults to 1)",
    ),
    (
        "calendar_type",
        "Calendar category: cj/qh/hk/us (defaults to cj)",
    ),
    (
        "calendar_datatype",
        "Calendar datatype: data/event/holiday (defaults to data)",
    ),
    (
        "symbols_type",
        "Symbols type: GOODS/FOREX/FUTURE/CRYPTO (defaults to GOODS)",
    ),
    (
        "symbols_datatype",
        "Symbols datatype: symbols/quotes (defaults to symbols)",
    ),
    ("filter", "Jin10 exclude filter (flash/news)"),
    ("top_n", "Positive result limit; defaults to 10"),
    (
        "max_retries",
        "Additional outer search attempts after an error",
    ),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const JIN10_OUTPUTS: &[(&str, &str)] = &[
    (
        "formalized_content",
        "RAGFlow-style Jin10 finance references (title/snippet)",
    ),
    ("json", "Go-compatible results envelope with Jin10 rows"),
    ("doc_aggs", "Jin10-result document aggregations"),
    ("_ERROR", "Terminal Jin10 request or parsing error"),
];
const QWEATHER_INPUTS: &[(&str, &str)] = &[
    (
        "location",
        "QWeather city name or Canvas selector; defaults to sys.query",
    ),
    (
        "web_apikey",
        "QWeather Web API key; QWEATHER_API_KEY is the fallback",
    ),
    (
        "type",
        "QWeather data type: weather/indices/airquality; defaults to weather",
    ),
    (
        "time_period",
        "Weather period: now/3d/7d/10d/15d/30d; defaults to now",
    ),
    ("lang", "Response language; defaults to zh"),
    ("paid", "Use paid api.qweather.com host; defaults to free"),
    ("top_n", "Positive result limit; defaults to 10"),
    (
        "max_retries",
        "Additional outer search attempts after an error",
    ),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const QWEATHER_OUTPUTS: &[(&str, &str)] = &[
    (
        "formalized_content",
        "RAGFlow-style QWeather references (title/snippet)",
    ),
    ("json", "Go-compatible results envelope with QWeather rows"),
    ("doc_aggs", "QWeather-result document aggregations"),
    ("_ERROR", "Terminal QWeather request or parsing error"),
];
const SEARXNG_INPUTS: &[(&str, &str)] = &[
    (
        "query",
        "SearXNG query or Canvas selector; defaults to sys.query",
    ),
    (
        "searxng_url",
        "SearXNG instance base URL (e.g. http://localhost:4000); defaults to SEARXNG_URL",
    ),
    ("top_n", "Positive result limit; defaults to 10"),
    (
        "max_retries",
        "Additional outer search attempts after an error",
    ),
    ("delay_after_error", "Non-negative retry delay in seconds"),
];
const SEARXNG_OUTPUTS: &[(&str, &str)] = &[
    (
        "formalized_content",
        "RAGFlow-style SearXNG references (title/link/snippet)",
    ),
    ("json", "Go-compatible results envelope with SearXNG rows"),
    ("_ERROR", "Terminal SearXNG request or parsing error"),
];
const DOC_GENERATOR_INPUTS: &[(&str, &str)] = &[
    (
        "content",
        "Markdown/text body with Canvas variable selectors",
    ),
    ("filename", "Optional sanitized output filename"),
    ("output_format", "pdf, docx, txt, markdown, html, or md"),
    (
        "header_text",
        "RAGFlow DocGeneratorParam header overlay text",
    ),
    (
        "footer_text",
        "RAGFlow DocGeneratorParam footer overlay text",
    ),
    (
        "watermark_text",
        "RAGFlow DocGeneratorParam diagonal watermark text",
    ),
    (
        "add_page_numbers",
        "Render page numbers in generated documents",
    ),
    ("add_timestamp", "Stamp generation time into the document"),
    (
        "include_download_info_in_content",
        "Embed the download descriptor JSON into the content",
    ),
    ("font_size", "Positive font size, at least 12"),
];
const DOC_GENERATOR_OUTPUTS: &[(&str, &str)] = &[
    ("doc_id", "Generated document UUID"),
    ("filename", "Sanitized filename with the selected extension"),
    ("mime_type", "Generated document MIME type"),
    ("size", "Generated byte size"),
    ("bytes", "Generated bytes encoded as base64"),
    ("download", "Python-compatible JSON download descriptor"),
    (
        "attachment",
        "Inline attachment descriptor for Rust consumers",
    ),
    ("preview_url", "RAGFlow-compatible attachment preview path"),
    ("created", "RFC3339 document generation timestamp"),
    ("_ERROR", "Terminal document generation error"),
];
/// 导出组件目录（供 /skills 页面展示：name + category）。
pub fn component_catalog() -> Vec<(&'static str, &'static str)> {
    COMPONENT_REGISTRY
        .iter()
        .map(|descriptor| (descriptor.name, descriptor.category.as_str()))
        .collect()
}

const COMPONENT_REGISTRY: &[StaticComponentDescriptor] = &[
    StaticComponentDescriptor {
        name: "agent",
        category: ComponentCategory::Agent,
        inputs: AGENT_INPUTS,
        outputs: AGENT_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "begin",
        category: ComponentCategory::Agent,
        inputs: NO_FIELDS,
        outputs: NO_FIELDS,
    },
    StaticComponentDescriptor {
        name: "google",
        category: ComponentCategory::Agent,
        inputs: GOOGLE_INPUTS,
        outputs: GOOGLE_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "googlescholar",
        category: ComponentCategory::Agent,
        inputs: GOOGLE_SCHOLAR_INPUTS,
        outputs: GOOGLE_SCHOLAR_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "github",
        category: ComponentCategory::Agent,
        inputs: GITHUB_INPUTS,
        outputs: GITHUB_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "yahoofinance",
        category: ComponentCategory::Agent,
        inputs: YAHOO_FINANCE_INPUTS,
        outputs: YAHOO_FINANCE_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "arxiv",
        category: ComponentCategory::Agent,
        inputs: ARXIV_INPUTS,
        outputs: ARXIV_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "pubmed",
        category: ComponentCategory::Agent,
        inputs: PUBMED_INPUTS,
        outputs: PUBMED_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "bing",
        category: ComponentCategory::Agent,
        inputs: BING_INPUTS,
        outputs: BING_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "baidu",
        category: ComponentCategory::Agent,
        inputs: BAIDU_INPUTS,
        outputs: BAIDU_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "bocha",
        category: ComponentCategory::Agent,
        inputs: BOCHA_INPUTS,
        outputs: BOCHA_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "tencentfinance",
        category: ComponentCategory::Agent,
        inputs: TENCENT_FINANCE_INPUTS,
        outputs: TENCENT_FINANCE_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "baiduscholar",
        category: ComponentCategory::Agent,
        inputs: BAIDU_SCHOLAR_INPUTS,
        outputs: BAIDU_SCHOLAR_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "eastmoney",
        category: ComponentCategory::Agent,
        inputs: EASTMONEY_INPUTS,
        outputs: EASTMONEY_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "jin10",
        category: ComponentCategory::Agent,
        inputs: JIN10_INPUTS,
        outputs: JIN10_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "qweather",
        category: ComponentCategory::Agent,
        inputs: QWEATHER_INPUTS,
        outputs: QWEATHER_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "searxng",
        category: ComponentCategory::Agent,
        inputs: SEARXNG_INPUTS,
        outputs: SEARXNG_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "categorize",
        category: ComponentCategory::Agent,
        inputs: &[
            (
                "llm_id",
                "Tenant model selector: model@provider or model@instance@provider",
            ),
            (
                "model_id",
                "Go Categorize alias for the tenant model selector",
            ),
            (
                "category_description",
                "RayRAG CategorizeParam: category name -> {to, description, examples}",
            ),
            ("query", "Query variable reference; defaults to sys.query"),
            (
                "message_history_window_size",
                "Prior conversation turns included before the query",
            ),
        ],
        outputs: &[
            ("category_name", "Winning category name"),
            ("_next", "Target component ids of the winning category"),
            ("_ERROR", "Terminal model or classification error"),
        ],
    },
    StaticComponentDescriptor {
        name: "duckduckgo",
        category: ComponentCategory::Agent,
        inputs: DUCKDUCKGO_INPUTS,
        outputs: DUCKDUCKGO_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "dataoperations",
        category: ComponentCategory::Agent,
        inputs: &[
            ("query", "List of data items to operate on"),
            ("operations", "Data transform operation name"),
            ("select_keys", "Keys retained by select_keys"),
            ("filter_values", "All-match value filter rules"),
            ("updates", "Key/value updates"),
            ("remove_keys", "Keys removed from each object"),
            ("rename_keys", "Old/new key mappings"),
        ],
        outputs: &[("result", "Transformed JSON payload")],
    },
    StaticComponentDescriptor {
        name: "docgenerator",
        category: ComponentCategory::Agent,
        inputs: DOC_GENERATOR_INPUTS,
        outputs: DOC_GENERATOR_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "docsgenerator",
        category: ComponentCategory::Agent,
        inputs: DOC_GENERATOR_INPUTS,
        outputs: DOC_GENERATOR_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "excelprocessor",
        category: ComponentCategory::Agent,
        inputs: &[
            ("bytes", "Base64 spreadsheet bytes or Canvas selector"),
            (
                "file_ref",
                "Single inline file descriptor or Canvas selector",
            ),
            ("file_refs", "Multiple inline files for merge"),
            (
                "input_files",
                "RAGFlow ExcelProcessorParam variable references",
            ),
            ("operation", "read, merge, transform, or output"),
            ("output_data", "Go-compatible grid for write"),
            (
                "transform_data",
                "RAGFlow ExcelProcessorParam data reference for transform/output",
            ),
            (
                "sheet_selection",
                "all, first, or comma-separated sheet names",
            ),
            ("merge_strategy", "concat or join"),
            ("join_on", "Column name used by the join strategy"),
            (
                "transform_instructions",
                "Natural-language instructions for LLM-guided transforms",
            ),
            ("output_format", "xlsx or csv"),
            ("output_filename", "Generated file base name"),
        ],
        outputs: &[
            ("attachment", "Inline generated CSV/XLSX descriptor"),
            ("bytes", "Generated file bytes encoded as base64"),
            ("data", "Sheet records keyed by sheet or source"),
            ("markdown", "Bounded Markdown table preview"),
            ("rows", "Go-compatible raw row grid"),
            ("sheet_names", "Workbook sheet names"),
            ("size", "Generated byte size or read row count"),
            ("summary", "Human-readable operation summary"),
            ("_ERROR", "Terminal Excel parse or generation error"),
        ],
    },
    StaticComponentDescriptor {
        name: "extractor",
        category: ComponentCategory::Ingestion,
        inputs: &[("file", "source file")],
        outputs: &[("document", "parsed document")],
    },
    StaticComponentDescriptor {
        name: "file",
        category: ComponentCategory::Ingestion,
        inputs: &[("path", "local file path")],
        outputs: &[("document", "validated source document")],
    },
    StaticComponentDescriptor {
        name: "generate",
        category: ComponentCategory::Agent,
        inputs: LLM_INPUTS,
        outputs: LLM_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "invoke",
        category: ComponentCategory::Agent,
        inputs: &[
            ("url", "HTTP/HTTPS endpoint with Canvas variable templates"),
            ("method", "GET, POST, or PUT"),
            ("headers", "JSON object with optional {variable} templates"),
            ("variables", "Ordered request parameter definitions"),
            ("datatype", "JSON or formdata request encoding"),
            ("timeout", "Per-request timeout in seconds"),
            ("proxy", "Optional validated HTTP/HTTPS proxy"),
            ("clean_html", "Extract text from an HTML response"),
        ],
        outputs: &[
            ("result", "Response body text"),
            ("_ERROR", "Canonical URL refusal or terminal request error"),
        ],
    },
    StaticComponentDescriptor {
        name: "listoperations",
        category: ComponentCategory::Agent,
        inputs: &[
            ("query", "Array variable reference to operate on"),
            (
                "operations",
                "nth, head, tail, filter, sort, or drop_duplicates",
            ),
            ("n", "Item index or head/tail count"),
            ("strict", "Raise instead of returning an empty result"),
            ("sort_method", "asc or desc"),
            (
                "filter",
                "Object with operator (=, contains, ...) and value",
            ),
        ],
        outputs: &[
            ("result", "Operation result array"),
            ("first", "First element of the result"),
            ("last", "Last element of the result"),
        ],
    },
    StaticComponentDescriptor {
        name: "iteration",
        category: ComponentCategory::Agent,
        inputs: &[
            (
                "items_ref",
                "RAGFlow IterationParam variable reference to the array to iterate",
            ),
            (
                "variable",
                "RAGFlow IterationParam per-item variable schema",
            ),
        ],
        outputs: &[("_ERROR", "Terminal non-array items_ref or iteration error")],
    },
    StaticComponentDescriptor {
        name: "iterationitem",
        category: ComponentCategory::Agent,
        inputs: NO_FIELDS,
        outputs: &[
            ("item", "Current array element of the parent iteration"),
            ("index", "Zero-based index of the current element"),
        ],
    },
    StaticComponentDescriptor {
        name: "exitloop",
        category: ComponentCategory::Agent,
        inputs: NO_FIELDS,
        outputs: NO_FIELDS,
    },
    StaticComponentDescriptor {
        name: "loopitem",
        category: ComponentCategory::Agent,
        inputs: NO_FIELDS,
        outputs: NO_FIELDS,
    },
    StaticComponentDescriptor {
        name: "loop",
        category: ComponentCategory::Agent,
        inputs: &[
            (
                "loop_variables",
                "Variables seeded once and shared by loop iterations",
            ),
            (
                "loop_termination_condition",
                "Post-iteration termination predicates",
            ),
            (
                "maximum_loop_count",
                "Iteration cap; zero uses the safety cap",
            ),
            (
                "logical_operator",
                "and or or; joins loop_termination_condition predicates",
            ),
        ],
        outputs: NO_FIELDS,
    },
    StaticComponentDescriptor {
        name: "parallel",
        category: ComponentCategory::Agent,
        inputs: &[
            (
                "cpn_id",
                "Stable component identifier used by the Parallel macro expansion",
            ),
            ("items_ref", "Variable reference to the list to iterate"),
            (
                "max_concurrency",
                "Maximum concurrent item subgraphs; zero is sequential",
            ),
        ],
        outputs: &[("_result", "Ordered per-item Canvas state snapshots")],
    },
    StaticComponentDescriptor {
        name: "llm",
        category: ComponentCategory::Agent,
        inputs: LLM_INPUTS,
        outputs: LLM_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "message",
        category: ComponentCategory::Agent,
        inputs: &[
            (
                "content",
                "One or more message templates; one entry is selected per run",
            ),
            ("text", "Go v2 alias for a single message template"),
            (
                "stream",
                "Fixed DSL streaming preference; HTTP streaming remains run-level",
            ),
        ],
        outputs: &[
            ("content", "Rendered message body"),
            (
                "downloads",
                "Normalized doc_id/filename/mime_type download descriptors",
            ),
        ],
    },
    StaticComponentDescriptor {
        name: "parser",
        category: ComponentCategory::Ingestion,
        inputs: &[("document", "validated source document")],
        outputs: &[("content", "structured parsed content")],
    },
    StaticComponentDescriptor {
        name: "retrieval",
        category: ComponentCategory::Agent,
        inputs: &[
            (
                "query",
                "Dataset search keywords or Canvas selector; defaults to sys.query",
            ),
            ("empty_response", "Fallback content when no chunk matches"),
            (
                "dataset_ids",
                "Dataset ids or Canvas variable references (kb_ids is the deprecated alias)",
            ),
            ("kb_ids", "Deprecated alias of dataset_ids"),
            (
                "similarity_threshold",
                "Vector similarity cutoff between 0 and 1; defaults to 0.2",
            ),
            (
                "keywords_similarity_weight",
                "Keyword/vector weight between 0 and 1; defaults to 0.5",
            ),
            ("top_n", "Positive result limit; defaults to 8"),
            ("top_k", "Positive candidate cap; defaults to 1024"),
            ("rerank_id", "Optional rerank model id"),
            ("meta_data_filter", "Manual metadata filter object"),
            (
                "retrieval_from",
                "dataset (only supported source; other values fail closed)",
            ),
            (
                "memory_ids",
                "Memory retrieval ids; non-empty values fail closed",
            ),
            ("use_kg", "Knowledge-graph mode; true fails closed"),
            ("toc_enhance", "TOC enhancement; true fails closed"),
            (
                "cross_languages",
                "Cross-language expansion ids; non-empty values fail closed",
            ),
        ],
        outputs: &[
            (
                "formalized_content",
                "RAGFlow kb_prompt-style retrieval references",
            ),
            ("json", "Retrieved chunks"),
            ("doc_aggs", "Retrieval document aggregations"),
            ("_ERROR", "Terminal retrieval or configuration error"),
        ],
    },
    StaticComponentDescriptor {
        name: "stringtransform",
        category: ComponentCategory::Agent,
        inputs: NO_FIELDS,
        outputs: NO_FIELDS,
    },
    StaticComponentDescriptor {
        name: "tavilyextract",
        category: ComponentCategory::Agent,
        inputs: TAVILY_EXTRACT_INPUTS,
        outputs: TAVILY_EXTRACT_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "tavilysearch",
        category: ComponentCategory::Agent,
        inputs: TAVILY_SEARCH_INPUTS,
        outputs: TAVILY_SEARCH_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "wikipedia",
        category: ComponentCategory::Agent,
        inputs: WIKIPEDIA_INPUTS,
        outputs: WIKIPEDIA_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "baike",
        category: ComponentCategory::Agent,
        inputs: BAIKE_INPUTS,
        outputs: BAIKE_OUTPUTS,
    },
    StaticComponentDescriptor {
        name: "code_exec",
        category: ComponentCategory::Agent,
        inputs: &[
            ("script", "Code to execute (falls back to input script)"),
            ("lang", "Language: python3 (default)"),
            (
                "arguments",
                "Object of named arguments passed to the sandbox",
            ),
        ],
        outputs: &[
            ("content", "Canonical execution result"),
            ("actual_type", "Inferred result type"),
        ],
    },
    StaticComponentDescriptor {
        name: "exesql",
        category: ComponentCategory::Agent,
        inputs: &[
            ("sql", "SQL to execute (defaults to sys.query)"),
            ("db_type", "postgres (only supported type)"),
            ("database", "Database name"),
            ("username", "Database username"),
            ("host", "Database host"),
            ("port", "Database port (default 5432)"),
            ("password", "Database password"),
            ("max_records", "Max result rows (default 1024)"),
        ],
        outputs: &[("result", "JSON array of result rows")],
    },
    StaticComponentDescriptor {
        name: "email",
        category: ComponentCategory::Agent,
        inputs: &[
            ("to_email", "Target email address"),
            ("cc_email", "Comma-split additional recipients"),
            ("subject", "Email subject"),
            ("content", "Email body (HTML)"),
            (
                "smtp_server",
                "SMTP server (falls back to env EMAIL_SMTP_SERVER)",
            ),
            ("smtp_port", "SMTP port (default 465)"),
            ("email", "Sender email (env EMAIL_SENDER)"),
            ("password", "Authorization code (env EMAIL_SMTP_PASSWORD)"),
            ("sender_name", "Display name (env EMAIL_SENDER_NAME)"),
        ],
        outputs: &[("success", "Whether the email was sent")],
    },
    StaticComponentDescriptor {
        name: "tushare",
        category: ComponentCategory::Agent,
        inputs: &[
            (
                "content",
                "Keyword or stock symbol (array items joined with ',')",
            ),
            (
                "token",
                "TuShare API token (falls back to env TUSHARE_TOKEN)",
            ),
            (
                "src",
                "News source: sina/wallstreetcn/10jqka/eastmoney/yuncaijing/fenghuang/jinrongjie",
            ),
            ("start_date", "Range start, format %Y-%m-%d %H:%M:%S"),
            ("end_date", "Range end, defaults to now"),
            ("keyword", "Case-insensitive content filter"),
        ],
        outputs: &[("content", "Markdown news table")],
    },
    StaticComponentDescriptor {
        name: "akshare",
        category: ComponentCategory::Agent,
        inputs: &[
            (
                "content",
                "Stock symbol or keyword (array items joined with ',')",
            ),
            ("top_n", "Max news items (default 10)"),
        ],
        outputs: &[("content", "Markdown news list")],
    },
    StaticComponentDescriptor {
        name: "translate",
        category: ComponentCategory::Agent,
        inputs: &[
            (
                "content",
                "Text to translate (array items joined with newline)",
            ),
            (
                "auth_key",
                "DeepL auth key (optional; RayRAG prefers BAIDU_TRANSLATE_APPID/KEY env, falls back to keyless Baidu sug endpoint)",
            ),
            (
                "source_lang",
                "Source language code, DeepL style (ZH/EN/JA/...; default auto)",
            ),
            (
                "target_lang",
                "Target language code, DeepL style (EN-GB/EN-US/ZH/...; default EN)",
            ),
        ],
        outputs: &[("content", "Translated text")],
    },
    StaticComponentDescriptor {
        name: "iwencai",
        category: ComponentCategory::Agent,
        inputs: &[
            (
                "query",
                "The question/conditions to select stocks (defaults to sys.query)",
            ),
            ("top_n", "Max result rows (default 10)"),
            (
                "query_type",
                "stock/zhishu/fund/hkstock/usstock/threeboard/conbond/insurance/futures/lccp/foreign_exchange",
            ),
            ("cookie", "Optional iwencai session cookie"),
        ],
        outputs: &[(
            "formalized_content",
            "Markdown tables rendered from the iwencai answer",
        )],
    },
    StaticComponentDescriptor {
        // RAGFlow canvas component_name is `WenCai`; keep the lowercase
        // `wencai` alias registered so saved canvases validate and dispatch.
        name: "wencai",
        category: ComponentCategory::Agent,
        inputs: &[
            (
                "query",
                "The question/conditions to select stocks (defaults to sys.query)",
            ),
            ("top_n", "Max result rows (default 10)"),
            (
                "query_type",
                "stock/zhishu/fund/hkstock/usstock/threeboard/conbond/insurance/futures/lccp/foreign_exchange",
            ),
            ("cookie", "Optional iwencai session cookie"),
        ],
        outputs: &[(
            "formalized_content",
            "Markdown tables rendered from the iwencai answer",
        )],
    },
    StaticComponentDescriptor {
        name: "crawler",
        category: ComponentCategory::Agent,
        inputs: &[
            ("content", "URL to crawl (array items joined with ' - ')"),
            ("query", "URL fallback when content is empty"),
            ("extract_type", "html | markdown | content"),
            ("proxy", "Optional HTTP proxy URL"),
        ],
        outputs: &[("content", "Extracted page content")],
    },
    StaticComponentDescriptor {
        name: "switch",
        category: ComponentCategory::Agent,
        inputs: NO_FIELDS,
        outputs: NO_FIELDS,
    },
    StaticComponentDescriptor {
        name: "tokenchunker",
        category: ComponentCategory::Ingestion,
        inputs: &[("document", "parsed document")],
        outputs: &[("chunks", "token-bounded chunks")],
    },
    StaticComponentDescriptor {
        name: "tokenizer",
        category: ComponentCategory::Ingestion,
        inputs: &[("text", "input text")],
        outputs: &[("tokens", "token count and boundaries")],
    },
    StaticComponentDescriptor {
        name: "userfillup",
        category: ComponentCategory::Agent,
        inputs: &[
            ("enable_tips", "Whether to show the form prompt"),
            ("inputs", "Named user-input field schemas"),
            ("layout_recognize", "Optional layout-recognition selector"),
            ("tips", "Prompt displayed while waiting for user input"),
        ],
        outputs: &[
            ("*", "Dynamic outputs named after declared form fields"),
            ("tips", "Rendered or waiting-state form prompt"),
            ("user_input", "Raw scalar or object supplied on resume"),
        ],
    },
    StaticComponentDescriptor {
        name: "variableaggregator",
        category: ComponentCategory::Agent,
        inputs: NO_FIELDS,
        outputs: NO_FIELDS,
    },
    StaticComponentDescriptor {
        name: "variableassigner",
        category: ComponentCategory::Agent,
        inputs: NO_FIELDS,
        outputs: NO_FIELDS,
    },
];

/// Return the immutable catalog, optionally filtered by category.
///
/// Duplicate categories do not duplicate rows. The result is always sorted by
/// normalized component name and both metadata maps are always non-null.
pub fn component_descriptors(categories: &[ComponentCategory]) -> Vec<ComponentDescriptor> {
    let mut descriptors: Vec<_> = COMPONENT_REGISTRY
        .iter()
        .filter(|descriptor| categories.is_empty() || categories.contains(&descriptor.category))
        .map(|descriptor| ComponentDescriptor {
            name: descriptor.name.to_owned(),
            category: descriptor.category.as_str().to_owned(),
            inputs: descriptor
                .inputs
                .iter()
                .map(|(name, description)| ((*name).to_owned(), (*description).to_owned()))
                .collect(),
            outputs: descriptor
                .outputs
                .iter()
                .map(|(name, description)| ((*name).to_owned(), (*description).to_owned()))
                .collect(),
        })
        .collect();
    descriptors.sort_by(|left, right| left.name.cmp(&right.name));
    descriptors
}

/// Case-insensitive lookup used by Canvas save/compile validation.
pub fn is_supported_agent_component(name: &str) -> bool {
    let name = name.trim();
    COMPONENT_REGISTRY.iter().any(|descriptor| {
        descriptor.category == ComponentCategory::Agent
            && descriptor.name.eq_ignore_ascii_case(name)
    })
}

/// Observer used by [`track_progress`].
pub type ProgressCallback<'a> = &'a mut dyn FnMut(i32, &str);

/// Run synchronous work with the fixed RAGFlow progress protocol.
///
/// The callback receives `0 / "<name> Started"` before work and then either
/// `1 / "<name> Done"` or `-1 / "<name>: <error>"`. With no observer the
/// operation and its original result pass through unchanged.
pub fn track_progress<T, E>(
    name: &str,
    mut callback: Option<ProgressCallback<'_>>,
    operation: impl FnOnce() -> std::result::Result<T, E>,
) -> std::result::Result<T, E>
where
    E: std::fmt::Display,
{
    if let Some(callback) = callback.as_mut() {
        callback(0, &format!("{name} Started"));
    }
    match operation() {
        Ok(value) => {
            if let Some(callback) = callback.as_mut() {
                callback(1, &format!("{name} Done"));
            }
            Ok(value)
        }
        Err(error) => {
            if let Some(callback) = callback.as_mut() {
                callback(-1, &format!("{name}: {error}"));
            }
            Err(error)
        }
    }
}

/// Await fallible work for at most `duration`.
///
/// An expired deadline is preserved as [`tokio::time::error::Elapsed`] inside
/// the returned `anyhow::Error`; an operation error passes through unchanged.
/// Dropping the caller cancels this future and its child through normal Rust
/// structured-cancellation semantics.
pub async fn with_timeout<T>(
    duration: Duration,
    operation: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::time::timeout(duration, operation).await?
}

/// Run synchronous work and add RAGFlow's timing fields if absent.
///
/// `_created_time` is an RFC3339 UTC timestamp captured before the operation;
/// `_elapsed_time` is a non-negative floating-point number of wall-clock
/// seconds. Business values supplied by the operation win on key conflicts.
pub fn track_elapsed(
    name: &str,
    operation: impl FnOnce() -> Result<Map<String, Value>>,
) -> Result<Map<String, Value>> {
    let started_at = Instant::now();
    let created_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("UTC OffsetDateTime always has an RFC3339 representation");
    let mut output = match operation() {
        Ok(output) => output,
        Err(error) => {
            let message = format!("{name}: {error}");
            return Err(error.context(message));
        }
    };
    output
        .entry("_created_time")
        .or_insert_with(|| Value::String(created_at));
    output
        .entry("_elapsed_time")
        .or_insert_with(|| Value::from(started_at.elapsed().as_secs_f64()));
    Ok(output)
}


