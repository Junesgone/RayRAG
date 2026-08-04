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
    ("query", "Bing query or Canvas selector; defaults to sys.query"),
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
    ("query", "Baidu query or Canvas selector; defaults to sys.query"),
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
    ("query", "Bocha query or Canvas selector; defaults to sys.query"),
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
    ("_ERROR", "Terminal Tencent Finance request or parsing error"),
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
    ("_ERROR", "Terminal Jin10 request or parsing error"),
];
const QWEATHER_INPUTS: &[(&str, &str)] = &[
    (
        "location",
        "QWeather city name or Canvas selector; defaults to sys.query",
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
];
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
        ],
        outputs: NO_FIELDS,
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
            ("input_files", "Python/frontend file-selector list"),
            ("operation", "read, write, merge, transform, or output"),
            ("output_data", "Go-compatible grid for write"),
            (
                "transform_data",
                "Python-compatible records for transform/output",
            ),
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
        inputs: NO_FIELDS,
        outputs: NO_FIELDS,
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
        name: "tushare",
        category: ComponentCategory::Agent,
        inputs: &[
            ("content", "Keyword or stock symbol (array items joined with ',')"),
            ("token", "TuShare API token (falls back to env TUSHARE_TOKEN)"),
            ("src", "News source: sina/wallstreetcn/10jqka/eastmoney/yuncaijing/fenghuang/jinrongjie"),
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
            ("content", "Stock symbol or keyword (array items joined with ',')"),
            ("top_n", "Max news items (default 10)"),
        ],
        outputs: &[("content", "Markdown news list")],
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

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn component_catalog_is_unique_sorted_and_has_stable_shape() {
        let all = component_descriptors(&[]);
        assert!(!all.is_empty());
        assert!(
            all.windows(2)
                .all(|pair| pair[0].name.as_str() < pair[1].name.as_str())
        );
        let names: HashSet<_> = all.iter().map(|descriptor| &descriptor.name).collect();
        assert_eq!(names.len(), all.len());
        assert!(all.iter().all(|descriptor| {
            matches!(
                descriptor.category.as_str(),
                "agent" | "ingestion" | "shared"
            )
        }));
        let parallel = all
            .iter()
            .find(|descriptor| descriptor.name == "parallel")
            .unwrap();
        assert_eq!(
            parallel
                .inputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["cpn_id", "items_ref", "max_concurrency"]
        );
        assert_eq!(
            parallel
                .outputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["_result"]
        );
        let user_fill_up = all
            .iter()
            .find(|descriptor| descriptor.name == "userfillup")
            .unwrap();
        assert_eq!(
            user_fill_up
                .inputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["enable_tips", "inputs", "layout_recognize", "tips"]
        );
        assert_eq!(
            user_fill_up
                .outputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["*", "tips", "user_input"]
        );

        let message = all
            .iter()
            .find(|descriptor| descriptor.name == "message")
            .unwrap();
        assert_eq!(
            message
                .inputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["content", "stream", "text"]
        );
        assert_eq!(
            message
                .outputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["content", "downloads"]
        );
        let invoke = all
            .iter()
            .find(|descriptor| descriptor.name == "invoke")
            .unwrap();
        assert_eq!(
            invoke.inputs.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "clean_html",
                "datatype",
                "headers",
                "method",
                "proxy",
                "timeout",
                "url",
                "variables"
            ]
        );
        assert_eq!(
            invoke
                .outputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["_ERROR", "result"]
        );
        for name in ["generate", "llm"] {
            let llm = all
                .iter()
                .find(|descriptor| descriptor.name == name)
                .unwrap();
            assert_eq!(
                llm.inputs.keys().map(String::as_str).collect::<Vec<_>>(),
                [
                    "frequency_penalty",
                    "llm_id",
                    "max_retries",
                    "max_tokens",
                    "message_history_window_size",
                    "model_id",
                    "outputs.structured",
                    "presence_penalty",
                    "prompts",
                    "sys_prompt",
                    "temperature",
                    "top_p"
                ],
                "{name}"
            );
            assert_eq!(
                llm.outputs.keys().map(String::as_str).collect::<Vec<_>>(),
                ["_ERROR", "content", "structured"],
                "{name}"
            );
        }
        let agent = all
            .iter()
            .find(|descriptor| descriptor.name == "agent")
            .unwrap();
        assert_eq!(
            agent.inputs.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "frequency_penalty",
                "llm_id",
                "max_retries",
                "max_rounds",
                "max_tokens",
                "mcp",
                "message_history_window_size",
                "model_id",
                "outputs.structured",
                "presence_penalty",
                "prompts",
                "sys_prompt",
                "temperature",
                "tools",
                "top_p"
            ]
        );
        assert_eq!(
            agent.outputs.keys().map(String::as_str).collect::<Vec<_>>(),
            ["_ERROR", "content", "structured", "tool_calls"]
        );
        let duckduckgo = all
            .iter()
            .find(|descriptor| descriptor.name == "duckduckgo")
            .unwrap();
        assert_eq!(
            duckduckgo
                .inputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "channel",
                "delay_after_error",
                "max_retries",
                "query",
                "top_n"
            ]
        );
        assert_eq!(
            duckduckgo
                .outputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["_ERROR", "doc_aggs", "formalized_content", "json"]
        );
        let wikipedia = all
            .iter()
            .find(|descriptor| descriptor.name == "wikipedia")
            .unwrap();
        assert_eq!(
            wikipedia
                .inputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "delay_after_error",
                "language",
                "max_retries",
                "query",
                "top_n"
            ]
        );
        assert_eq!(
            wikipedia
                .outputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["_ERROR", "doc_aggs", "formalized_content", "json"]
        );
        let google = all
            .iter()
            .find(|descriptor| descriptor.name == "google")
            .unwrap();
        assert_eq!(
            google.inputs.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "api_key",
                "country",
                "delay_after_error",
                "language",
                "max_retries",
                "num",
                "q",
                "start"
            ]
        );
        assert_eq!(
            google
                .outputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["_ERROR", "doc_aggs", "formalized_content", "json"]
        );
        let google_scholar = all
            .iter()
            .find(|descriptor| descriptor.name == "googlescholar")
            .unwrap();
        assert_eq!(
            google_scholar
                .inputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "delay_after_error",
                "max_retries",
                "patents",
                "query",
                "sort_by",
                "top_n",
                "year_high",
                "year_low"
            ]
        );
        assert_eq!(
            google_scholar
                .outputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["_ERROR", "doc_aggs", "formalized_content", "json"]
        );
        let github = all
            .iter()
            .find(|descriptor| descriptor.name == "github")
            .unwrap();
        assert_eq!(
            github.inputs.keys().map(String::as_str).collect::<Vec<_>>(),
            ["delay_after_error", "max_retries", "query", "top_n"]
        );
        assert_eq!(
            github
                .outputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["_ERROR", "doc_aggs", "formalized_content", "json"]
        );
        let yahoo_finance = all
            .iter()
            .find(|descriptor| descriptor.name == "yahoofinance")
            .unwrap();
        assert_eq!(
            yahoo_finance
                .inputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "balance_sheet",
                "cash_flow_statement",
                "count",
                "delay_after_error",
                "financials",
                "history",
                "income_stmt",
                "info",
                "max_retries",
                "news",
                "stock_code"
            ]
        );
        assert_eq!(
            yahoo_finance
                .outputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["_ERROR", "report"]
        );
        let arxiv = all
            .iter()
            .find(|descriptor| descriptor.name == "arxiv")
            .unwrap();
        assert_eq!(
            arxiv.inputs.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "delay_after_error",
                "max_retries",
                "query",
                "sort_by",
                "top_n"
            ]
        );
        assert_eq!(
            arxiv.outputs.keys().map(String::as_str).collect::<Vec<_>>(),
            ["_ERROR", "doc_aggs", "formalized_content", "json"]
        );
        let pubmed = all
            .iter()
            .find(|descriptor| descriptor.name == "pubmed")
            .unwrap();
        assert_eq!(
            pubmed.inputs.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "delay_after_error",
                "email",
                "max_retries",
                "query",
                "top_n"
            ]
        );
        assert_eq!(
            pubmed
                .outputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["_ERROR", "doc_aggs", "formalized_content", "json"]
        );
        let categorize = all
            .iter()
            .find(|descriptor| descriptor.name == "categorize")
            .unwrap();
        assert_eq!(
            categorize
                .inputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["llm_id", "model_id"]
        );
        let excel = all
            .iter()
            .find(|descriptor| descriptor.name == "excelprocessor")
            .unwrap();
        assert_eq!(
            excel.inputs.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "bytes",
                "file_ref",
                "file_refs",
                "input_files",
                "operation",
                "output_data",
                "transform_data"
            ]
        );
        assert_eq!(
            excel.outputs.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "attachment",
                "bytes",
                "data",
                "markdown",
                "rows",
                "sheet_names",
                "size",
                "summary"
            ]
        );
    }

    #[test]
    fn component_catalog_filters_categories_without_duplicate_rows() {
        let ingestion = component_descriptors(&[
            ComponentCategory::Ingestion,
            ComponentCategory::Ingestion,
            ComponentCategory::Shared,
        ]);
        assert_eq!(
            ingestion
                .iter()
                .map(|descriptor| descriptor.name.as_str())
                .collect::<Vec<_>>(),
            ["extractor", "file", "parser", "tokenchunker", "tokenizer"]
        );
        assert!(
            ingestion
                .iter()
                .all(|descriptor| descriptor.category == "ingestion")
        );
    }

    #[test]
    fn component_category_and_agent_lookup_are_case_insensitive() {
        assert_eq!(
            ComponentCategory::parse(" INGESTION "),
            Some(ComponentCategory::Ingestion)
        );
        assert_eq!(ComponentCategory::parse("unknown"), None);
        assert!(is_supported_agent_component(" StringTransform "));
        assert!(is_supported_agent_component("DataOperations"));
        assert!(is_supported_agent_component(" DocGenerator "));
        assert!(is_supported_agent_component("DocsGenerator"));
        assert!(is_supported_agent_component(" ExcelProcessor "));
        assert!(is_supported_agent_component("Loop"));
        assert!(is_supported_agent_component("Parallel"));
        assert!(is_supported_agent_component("UserFillUp"));
        assert!(is_supported_agent_component("LLM"));
        assert!(is_supported_agent_component(" Invoke "));
        assert!(!is_supported_agent_component("browser"));
        assert!(!is_supported_agent_component("parser"));
    }

    #[test]
    fn progress_reports_exact_success_and_failure_protocol() {
        let mut calls = Vec::new();
        {
            let mut callback = |progress, message: &str| {
                calls.push((progress, message.to_owned()));
            };
            let value = track_progress("Parser", Some(&mut callback), || Ok::<_, anyhow::Error>(7))
                .unwrap();
            assert_eq!(value, 7);
        }
        assert_eq!(
            calls,
            [(0, "Parser Started".into()), (1, "Parser Done".into())]
        );

        calls.clear();
        let mut callback = |progress, message: &str| calls.push((progress, message.to_owned()));
        let error = track_progress("Tokenizer", Some(&mut callback), || {
            Err::<(), _>(anyhow!("boom"))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "boom");
        assert_eq!(
            calls,
            [
                (0, "Tokenizer Started".into()),
                (-1, "Tokenizer: boom".into())
            ]
        );
    }

    #[test]
    fn progress_without_observer_still_runs_and_preserves_error() {
        let called = AtomicBool::new(false);
        let value = track_progress("File", None, || {
            called.store(true, Ordering::Relaxed);
            Ok::<_, anyhow::Error>("ok")
        })
        .unwrap();
        assert_eq!(value, "ok");
        assert!(called.load(Ordering::Relaxed));

        let error = track_progress("File", None, || Err::<(), _>(anyhow!("exact"))).unwrap_err();
        assert_eq!(error.to_string(), "exact");
    }

    #[tokio::test]
    async fn timeout_returns_success_inner_error_and_elapsed_deadline() {
        assert_eq!(
            with_timeout(Duration::from_millis(50), async {
                Ok::<_, anyhow::Error>(3)
            })
            .await
            .unwrap(),
            3
        );

        let inner = with_timeout(Duration::from_millis(50), async {
            Err::<(), _>(anyhow!("inner"))
        })
        .await
        .unwrap_err();
        assert_eq!(inner.to_string(), "inner");

        let deadline = with_timeout(Duration::from_millis(10), async {
            std::future::pending::<Result<()>>().await
        })
        .await
        .unwrap_err();
        assert!(deadline.is::<tokio::time::error::Elapsed>());
    }

    #[test]
    fn elapsed_adds_timing_fields_and_preserves_business_values() {
        let output = track_elapsed("Parser", || {
            std::thread::sleep(Duration::from_millis(2));
            Ok(Map::from_iter([("chunks".into(), Value::from(3))]))
        })
        .unwrap();
        assert_eq!(output["chunks"], 3);
        let created = output["_created_time"].as_str().unwrap();
        assert!(created.contains('T') && created.ends_with('Z'), "{created}");
        assert!(output["_elapsed_time"].as_f64().unwrap() >= 0.001);

        let preserved = track_elapsed("Tokenizer", || {
            Ok(Map::from_iter([
                ("_created_time".into(), Value::String("caller".into())),
                ("_elapsed_time".into(), Value::from(42.0)),
            ]))
        })
        .unwrap();
        assert_eq!(preserved["_created_time"], "caller");
        assert_eq!(preserved["_elapsed_time"], 42.0);
    }

    #[test]
    fn elapsed_accepts_empty_output_and_attributes_errors() {
        let empty = track_elapsed("File", || Ok(Map::new())).unwrap();
        assert!(empty.contains_key("_created_time"));
        assert!(empty.contains_key("_elapsed_time"));

        let error = track_elapsed("Extractor", || Err(anyhow!("downstream"))).unwrap_err();
        assert_eq!(error.to_string(), "Extractor: downstream");
        assert_eq!(error.root_cause().to_string(), "downstream");
    }
}
