//! RayRAG Web UI — Rust SSR, mirrors RAGFlow's complete UI flow.
//! Every button, every page, every interaction matches RAGFlow's design.

use crate::server::AppState;
use axum::{
    extract::{Path, State},
    response::{Html, IntoResponse, Redirect},
};
use std::sync::Arc;

fn t(k: &str, zh: bool) -> &str {
    if !zh {
        return k;
    }
    match k {
        "Dashboard" => "仪表盘",
        "Knowledge Bases" => "知识库",
        "Search" => "搜索",
        "Chat" => "对话",
        "Chat apps" => "对话应用",
        "Create chat" => "创建对话应用",
        "No chat app created yet" => "尚未创建对话应用",
        "Click to create your first chat app" => "点击创建你的第一个对话应用",
        "New conversation" => "新建对话",
        "No conversations yet" => "暂无对话",
        "Chat app not found" => "对话应用不存在",
        "Chat Setting" => "对话设置",
        "Basic Settings" => "基本设置",
        "Icon" => "图标",
        "Description" => "描述",
        "LLM" => "大语言模型",
        "Select model" => "选择模型",
        "Converts text into numerical vectors for meaning similarity search and memory retrieval." => {
            "将文本转换为数值向量，用于语义相似度搜索和记忆检索。"
        }
        "Analyzes conversation content, extracts key information, and generates structured memory summaries." => {
            "分析对话内容，提取关键信息，并生成结构化的记忆摘要。"
        }
        "Opener" => "开场白",
        "System Prompt" => "系统提示词",
        "Prompt Engine" => "提示引擎",
        "Cross Languages" => "多语言检索",
        "Similarity Threshold" => "相似度阈值",
        "Top N" => "检索条数",
        "Empty Response" => "空回复文案",
        "Rerank Model" => "重排模型",
        "Reranking model" => "Reranking模型",
        "Rerank Top K" => "重排条数",
        "Refine Multiturn" => "多轮精炼",
        "Reasoning" => "推理模式",
        "Vector Weight" => "向量权重",
        "Show Chunk Metadata" => "显示切片元数据",
        "Metadata Fields" => "元数据字段",
        "Metadata Filter" => "元数据过滤",
        "Disabled" => "禁用",
        "Automatic" => "自动",
        "Semi Automatic" => "半自动",
        "Manual" => "手动",
        "Manual Fields" => "手动字段",
        "Search apps" => "检索应用",
        "Create search" => "创建检索应用",
        "No search app created yet" => "尚未创建检索应用",
        "Click to create your first search app" => "点击创建你的第一个检索应用",
        "Search app not found" => "检索应用不存在",
        "Test search" => "测试检索",
        "Hybrid" => "混合检索",
        "Rerank" => "重排序",
        // Upstream has exactly two i18n topK labels: `knowledgeDetails.topK`
        // = `Top-K` (zh `Top-K`, components/rerank.tsx — KB testing, chat app
        // settings, agent retrieval forms) and `skillSearch.topK` =
        // `Top K Results` (zh `返回结果数量`, skill search config modal).
        // `next-search/search-setting.tsx:467` hard-codes a literal `Top K`,
        // mirrored untranslated at RayRAG's search settings sidebar.
        "Top-K" => "Top-K",
        "Top K Results" => "返回结果数量",

        "Agents" => "智能体",
        "Memories" => "记忆",
        "Files" => "文件",
        "Settings" => "设置",
        "Model providers" => "模型提供商",
        "Set default models" => "设置默认模型",
        "Please complete these settings before beginning" => "开始前请完成这些设置",
        "Admin" => "管理",
        "Status" => "状态",
        "Login" => "登录",
        "Upload" => "上传",
        "Create" => "创建",
        "Delete" => "删除",
        "Send" => "发送",
        "Ask" => "提问",
        "Generate" => "生成",
        "Manage KBs" => "管理知识库",
        "Search Docs" => "搜索文档",
        "New Chat" => "新对话",
        "New KB" => "新建知识库",
        "Overview" => "概览",
        "Configuration" => "配置",
        "Testing" => "检索测试",
        "Knowledge Graph" => "知识图谱",
        "Documents" => "文档",
        "Rename" => "重命名",
        "Save" => "保存",
        "Test" => "测试",
        "Build" => "构建",
        "Chunk Size" => "分块大小",
        "Overlap" => "重叠",
        "Embedding Model" => "嵌入模型",
        "Please select an embedding model" => "请选择嵌入模型",
        "Please select" => "请选择",
        "Select value" => "請選 擇",
        "Please select a embedding model." => "请选择嵌入模型",
        "Select an embedding model" => "选择嵌入模型",
        "Skill Search Configuration" => "技能搜索配置",
        "The default embedding model used by the dataset. Once the dataset has chunks, when switching the embedding model, the system randomly samples a few chunks for a compatibility check, re-embeds them with the new embedding model, and computes cosine similarity between the new and old vectors. Switching is allowed only when the average similarity of the sample is ≥ 0.9. Otherwise, you must delete all chunks in the dataset before you can change it." => {
            "知识库采用的默认嵌入模型。一旦知识库内已经产生了文本块，更换嵌入模型时，系统将随机抽取若干 chunk 进行兼容性校验，使用新嵌入模型重新编码并计算新旧向量的余弦相似度，样本平均相似度需 ≥ 0.9 方可切换。否则，必须删除知识库内的所有文本块后才能更改。"
        }
        "Threshold" => "相似度阈值",
        "Name" => "名称",
        "Created" => "创建时间",
        "Size" => "大小",
        "Reparse" => "重新解析",
        "Parse Status" => "解析状态",
        "Chunks" => "块数",
        "Updated" => "更新时间",
        "Chunk Method" => "分块方法",
        "Chunk Token Num" => "分块 Token 数",
        "Delimiters" => "分隔符",
        "Auto Keywords" => "自动关键词",
        "Auto Questions" => "自动问题",
        "Enable Children Chunks" => "父子分块（enable_children）",
        "Enable RAPTOR" => "启用 RAPTOR（递归摘要）",
        "Enable GraphRAG" => "启用 GraphRAG（实体抽取）",
        "Back to KBs" => "返回知识库",
        "Test Query" => "测试查询",
        "Question" => "查询问题",
        "Results" => "检索结果",
        "No documents yet" => "暂无文档",
        "Run" => "运行",
        "Publish" => "发布",
        "Cancel" => "取消",
        "Copy" => "复制",
        "Download" => "下载",
        "Install" => "安装",
        "Uninstall" => "卸载",
        "Components" => "组件",
        "Run Log" => "运行日志",
        "Add User" => "添加用户",
        "API Keys" => "API 密钥",
        "Delete Key" => "删除密钥",
        "Close" => "关闭",
        "New Agent" => "新建智能体",
        "Create Memory" => "创建记忆",
        "Edit Memory" => "编辑记忆",
        "Clear All" => "清空全部",
        "Messages" => "消息",
        "Memory Messages" => "记忆消息",
        "Memory extract is configured with Prompts and Temperature from Advanced Settings." => {
            "记忆提取使用高级设置中的提示词和温度值进行配置。"
        }
        "Search messages" => "搜索消息",
        "Agent" => "智能体",
        "All agents" => "全部智能体",
        "Session ID" => "会话 ID",
        "Type" => "类型",
        "Valid date" => "有效日期",
        "Forget at" => "遗忘于",
        "Enable" => "启用",
        "Action" => "操作",
        "Content" => "内容",
        "Content embed" => "内容向量",
        "Forget" => "遗忘",
        "Forget message" => "遗忘消息",
        "Are you sure you want to forget?" => "确定遗忘吗？",
        "After forgetting, this message will not be retrieved by agents." => {
            "遗忘后，智能体将无法检索此消息。"
        }
        "Previous" => "上一页",
        "Next" => "下一页",
        "No messages yet" => "暂无消息",
        "Loading messages..." => "正在加载消息…",
        "Back to Memories" => "返回记忆列表",
        "Memory Configuration" => "记忆配置",
        "Basic settings" => "基础设置",
        "Advanced settings" => "高级设置",
        "Avatar" => "头像",
        "Chat Model" => "对话模型",
        "Memory Type" => "记忆类型",
        "Memory capacity (bytes)" => "记忆容量（字节）",
        "Permissions" => "权限",
        "Only me" => "仅自己",
        "Team" => "团队",
        "Storage type" => "存储类型",
        "Forgetting policy" => "遗忘策略",
        "Temperature" => "温度",
        "System prompt" => "系统提示词",
        "User prompt" => "用户提示词",
        "Reset" => "重置",
        "Knowledge Base" => "知识库",
        "Document Type" => "文档类型",
        "Stop" => "停止",
        "Page not found, please enter a correct address." => "页面不存在，请输入正确的地址。",
        "Empty" => "清空",
        "Export" => "导出",
        "Export JSON" => "导出 JSON",
        "Import JSON" => "导入 JSON",
        "Versions" => "版本管理",
        "Invite" => "邀请",
        "Link to KB" => "关联知识库",
        "Bulk Delete" => "批量删除",
        "Clear" => "清空选择",
        "Temporary Conversation" => "临时对话",
        _ => k,
    }
}
fn detect_lang(h: &axum::http::HeaderMap) -> bool {
    h.get("Cookie")
        .and_then(|c| c.to_str().ok())
        .map(|c| c.contains("lang=zh-CN"))
        .unwrap_or(true)
}

/// Serialize a string for an inline JavaScript value without permitting an
/// attacker-controlled path segment to terminate the surrounding `<script>`.
fn json_script_string(value: &str) -> String {
    serde_json::to_string(value)
        .expect("string serializes")
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

fn json_script_value(value: &serde_json::Value) -> String {
    serde_json::to_string(value)
        .expect("JSON value serializes")
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

const CSS: &str = include_str!("./web_css.txt");

/// External links rendered by the global header (upstream `header.tsx`:
/// Discord invite, GitHub repository, user-guide docs). Defaults match upstream
/// so the UI is a 1:1 copy, but each can be repointed through the environment
/// so a mainland-China deployment can use reachable mirrors.
fn header_link(kind: &str) -> String {
    let (env_key, default) = match kind {
        "discord" => (
            "RAYRAG_DISCORD_URL",
            "https://discord.com/invite/NjYzJD3GM3",
        ),
        "github" => ("RAYRAG_GITHUB_URL", "https://github.com/infiniflow/ragflow"),
        _ => (
            "RAYRAG_HELP_URL",
            "https://ragflow.io/docs/dev/category/user-guides",
        ),
    };
    std::env::var(env_key).unwrap_or_else(|_| default.to_string())
}

/// Shared `<head>` for every page: CSS bundle, tooltip/drawer helpers and the
/// fetch wrapper that attaches the bearer token and bounces expired sessions
/// (never while the standalone login page is on screen, which would loop).
fn page_head(title: &str, zh: bool) -> String {
    let lc = if zh { "zh-CN" } else { "en" };
    let mut h = String::with_capacity(8192);
    h.push_str("<!DOCTYPE html><html lang='");
    h.push_str(lc);
    h.push_str("'><head><meta charset='UTF-8'><meta name='viewport' content='width=device-width,initial-scale=1'><title>");
    h.push_str(title);
    h.push_str(" · RayRAG</title><style>");
    h.push_str(CSS);
    h.push_str(".tip{display:inline-block;width:15px;height:15px;line-height:15px;border-radius:50%;background:var(--tm);color:var(--bg,#fff);font-size:11px;text-align:center;cursor:help;position:relative;vertical-align:middle}
.tip:hover::after{content:attr(data-tip);position:absolute;left:22px;top:-4px;width:280px;background:var(--card,#1e1e2e);border:1px solid var(--bd);border-radius:8px;padding:8px 10px;font-size:12px;line-height:1.5;color:var(--t);z-index:50;box-shadow:0 4px 16px rgba(0,0,0,.25);white-space:normal}
.lang-chip{border:1px solid var(--bd);border-radius:14px;padding:2px 10px;font-size:12px;background:transparent;color:var(--t);cursor:pointer}
.lang-chip.on{background:var(--p);color:#fff;border-color:var(--p)}
\n.settings-drawer{position:fixed;top:0;right:0;width:440px;max-width:92vw;height:100vh;background:var(--bgc);border-left:1px solid var(--bd);display:flex;flex-direction:column;transform:translateX(100%);transition:transform .3s ease;z-index:120}\n.settings-drawer.open{transform:translateX(0)}\n.set-group{padding:8px 0 16px;border-bottom:1px solid var(--bd);margin-bottom:12px}\n.set-title{display:block;font-size:14px;margin-bottom:8px;color:var(--p)}\n.set-label{display:block;font-size:12px;color:var(--tm);margin:8px 0 4px}\n.set-switch{display:flex;align-items:center;gap:8px;margin-top:6px;font-size:13px}\n.set-sw-label{color:var(--t2)}\n</style><script>(function(){var nativeFetch=window.fetch.bind(window);window.fetch=function(input,init){init=init||{};var url=typeof input==='string'?input:(input&&input.url)||'';if(url.indexOf('/api/v1/')===0){var token=localStorage.getItem('rayrag_token');if(token){var headers=new Headers(init.headers||{});if(!headers.has('Authorization'))headers.set('Authorization','Bearer '+token);init.headers=headers}}return nativeFetch(input,init).then(function(response){if(response.status===401&&url!='/api/v1/auth/login'&&location.pathname!='/login'){localStorage.removeItem('rayrag_token');location.href='/login'}return response})}})()</script></head><body>");
    h
}

/// Standalone page shell: upstream's `/login` renders the flip-card auth page
/// without the global header (`login-next/index.tsx`), so RayRAG serves it with
/// the shared head only — no navbar, no avatar fetch, no session bounce loop.
fn layout_plain(title: &str, body: &str, zh: bool) -> String {
    let mut h = page_head(title, zh);
    h.push_str("<div class='ct'>");
    h.push_str(body);
    h.push_str("</div><script>if(localStorage.getItem('theme')==='light')document.body.classList.add('light');function toggleTheme(){document.body.classList.toggle('light');var l=document.body.classList.contains('light');localStorage.setItem('theme',l?'light':'dark')}</script></body></html>");
    h
}

fn layout(title: &str, body: &str, zh: bool) -> String {
    // Upstream `header.*` labels (web/src/locales en.ts / zh.ts): Home /
    // Dataset / Chat / Search / Agent / Memory / File. The global navbar
    // carries exactly these seven entries; every other area (data sources,
    // chat channels, model providers, MCP, team, profile, API) lives behind
    // the avatar, which links to `/user-setting`.
    let (home, dataset, chat, search, agent, memory, file, theme_label, avatar_label) = if zh {
        (
            "首页",
            "知识库",
            "聊天",
            "搜索",
            "智能体",
            "记忆",
            "文件管理",
            "主题",
            "用户设置",
        )
    } else {
        (
            "Home",
            "Dataset",
            "Chat",
            "Search",
            "Agent",
            "Memory",
            "File",
            "Theme",
            "User settings",
        )
    };
    let language_name = if zh { "简体中文" } else { "English" };
    let lc = if zh { "zh-CN" } else { "en" };
    let mut h = page_head(title, zh);
    // Upstream `layouts/components/header.tsx`: logo, the DesktopNavbar pill,
    // then the right cluster — Discord, GitHub, language DropdownMenu, help
    // link, ThemeButton and the avatar link to `/user-setting`.
    h.push_str("<header class='hdr'><a href='/' class='logo' aria-label='RayRAG'><svg width='32' height='32' aria-hidden='true'><circle cx='16' cy='16' r='14' fill='#4f46e5'/><text x='16' y='22' text-anchor='middle' fill='#fff' font-size='15' font-weight='700'>R</text></svg></a>");
    h.push_str("<nav class='gnav' aria-label='Global'>");
    h.push_str("<a class='gnav-icon' href='/' data-nav='/' aria-label='");
    h.push_str(home);
    h.push_str("'><svg width='22' height='22' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.5' stroke-linecap='round' stroke-linejoin='round' aria-hidden='true'><path d='M3 10.5 12 3l9 7.5'/><path d='M5 9.5V21h14V9.5'/><path d='M9.5 21v-6h5v6'/></svg></a>");
    for (href, nav_key, label, testid) in [
        ("/datasets", "/datasets", dataset, ""),
        ("/chats", "/chats", chat, " data-testid='nav-chat'"),
        (
            "/searches",
            "/searches",
            search,
            " data-testid='nav-search'",
        ),
        ("/agents", "/agents", agent, " data-testid='nav-agent'"),
        ("/memories", "/memories", memory, ""),
        ("/files", "/files", file, ""),
    ] {
        h.push_str("<a href='");
        h.push_str(href);
        h.push_str("' data-nav='");
        h.push_str(nav_key);
        h.push_str("'");
        h.push_str(testid);
        h.push_str(">");
        h.push_str(label);
        h.push_str("</a>");
    }
    h.push_str("</nav><div class='hr' data-testid='auth-status'>");
    // Community and documentation links mirror upstream header.tsx; each URL
    // can be pointed at a mainland-reachable mirror through the environment.
    h.push_str("<a class='hdr-icon' href='");
    h.push_str(&header_link("discord"));
    h.push_str("' target='_blank' rel='noreferrer noopener' aria-label='Discord'><svg width='18' height='18' viewBox='0 0 24 24' fill='currentColor' aria-hidden='true'><path d='M19.3 5.3A16 16 0 0 0 15.4 4l-.3.6a12 12 0 0 1 3.3 1.6 11.4 11.4 0 0 0-9-.3l-.3-.3A12 12 0 0 1 12.5 4.6L12.2 4a16 16 0 0 0-4 1.3C5.6 9.1 4.9 12.8 5.2 16.4a16.1 16.1 0 0 0 4.8 2.4l.6-1a10.4 10.4 0 0 1-1.6-.8l.4-.3a11.5 11.5 0 0 0 9.3 0l.4.3a10.4 10.4 0 0 1-1.6.8l.6 1a16.1 16.1 0 0 0 4.8-2.4c.3-4.2-.6-7.9-2.5-11.1ZM9.7 14.3c-.9 0-1.7-.9-1.7-1.9s.7-1.9 1.7-1.9 1.7.8 1.7 1.9-.8 1.9-1.7 1.9Zm4.6 0c-.9 0-1.7-.9-1.7-1.9s.7-1.9 1.7-1.9 1.7.8 1.7 1.9-.8 1.9-1.7 1.9Z'/></svg></a>");
    h.push_str("<a class='hdr-icon' href='");
    h.push_str(&header_link("github"));
    h.push_str("' target='_blank' rel='noreferrer noopener' aria-label='GitHub'><svg width='18' height='18' viewBox='0 0 24 24' fill='currentColor' aria-hidden='true'><path d='M12 2a10 10 0 0 0-3.2 19.5c.5.1.7-.2.7-.5v-1.8c-2.8.6-3.4-1.3-3.4-1.3-.4-1.1-1.1-1.4-1.1-1.4-.9-.6.1-.6.1-.6 1 .1 1.5 1 1.5 1 .9 1.5 2.3 1.1 2.9.8.1-.7.4-1.1.6-1.4-2.2-.3-4.6-1.1-4.6-5 0-1.1.4-2 1-2.7-.1-.3-.4-1.3.1-2.7 0 0 .8-.3 2.7 1a9.4 9.4 0 0 1 5 0c1.9-1.3 2.7-1 2.7-1 .5 1.4.2 2.4.1 2.7.6.7 1 1.6 1 2.7 0 3.9-2.4 4.7-4.6 5 .4.3.7.9.7 1.9v2.8c0 .3.2.6.7.5A10 10 0 0 0 12 2Z'/></svg></a>");
    h.push_str("<div class='menu' id='hdrLangMenu'><button type='button' class='hdr-icon' aria-haspopup='menu' aria-expanded='false' data-testid='language-menu' aria-label='");
    h.push_str(language_name);
    h.push_str("' onclick='toggleHeaderMenu(\"hdrLangMenu\",event)'>");
    h.push_str(language_name);
    h.push_str("<span aria-hidden='true'>⌄</span></button><div class='menu-panel' role='menu'>");
    h.push_str("<button type='button' role='menuitemradio' class='menu-item' aria-checked='");
    h.push_str(if zh { "false" } else { "true" });
    h.push_str("' onclick='setLang(\"en\")'>English</button>");
    h.push_str("<button type='button' role='menuitemradio' class='menu-item' aria-checked='");
    h.push_str(if zh { "true" } else { "false" });
    h.push_str("' onclick='setLang(\"zh-CN\")'>简体中文</button></div></div>");
    h.push_str("<a class='hdr-icon' href='");
    h.push_str(&header_link("help"));
    h.push_str("' target='_blank' rel='noreferrer noopener' aria-label='Help'><svg width='18' height='18' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.6' stroke-linecap='round' aria-hidden='true'><circle cx='12' cy='12' r='9.2'/><path d='M9.6 9.3a2.5 2.5 0 1 1 3.2 2.4c-.6.2-.9.7-.9 1.3v.4'/><circle cx='12' cy='16.6' r='.9' fill='currentColor' stroke='none'/></svg></a>");
    h.push_str("<button type='button' class='hdr-icon' onclick='toggleTheme()' aria-label='");
    h.push_str(theme_label);
    h.push_str("' title='");
    h.push_str(theme_label);
    h.push_str("'><span aria-hidden='true'>◐</span></button>");
    h.push_str("<a class='av' id='hdrAvatar' href='/user-setting' data-testid='settings-entrypoint' aria-label='");
    h.push_str(avatar_label);
    h.push_str("'>U</a></div></header>");
    h.push_str("<div class='ct'>");
    h.push_str(body);
    h.push_str("</div>");
    h.push_str("<script>if(localStorage.getItem('theme')==='light')document.body.classList.add('light');function toggleTheme(){document.body.classList.toggle('light');var l=document.body.classList.contains('light');localStorage.setItem('theme',l?'light':'dark');syncThemeSwitch()};function syncThemeSwitch(){var dark=!document.body.classList.contains('light');document.querySelectorAll('.theme-switch').forEach(function(node){node.setAttribute('aria-checked',dark?'true':'false');var sun=node.querySelector('.theme-sun');var moon=node.querySelector('.theme-moon');if(sun)sun.classList.toggle('on',!dark);if(moon)moon.classList.toggle('on',dark)})};syncThemeSwitch();function toggleLang(){var n=document.cookie.indexOf('lang=zh-CN')>=0?'en':'zh-CN';document.cookie='lang='+n+';path=/;max-age=31536000';location.reload()};function setLang(code){document.cookie='lang='+code+';path=/;max-age=31536000';location.reload()};function toggleHeaderMenu(id,event){if(event){event.stopPropagation()}var menu=document.getElementById(id);if(!menu)return;var open=menu.classList.contains('open');document.querySelectorAll('.menu.open').forEach(function(item){item.classList.remove('open');var trigger=item.querySelector('button[aria-haspopup]');if(trigger)trigger.setAttribute('aria-expanded','false')});if(!open){menu.classList.add('open');var trigger=menu.querySelector('button[aria-haspopup]');if(trigger)trigger.setAttribute('aria-expanded','true')}};document.addEventListener('click',function(){document.querySelectorAll('.menu.open').forEach(function(item){item.classList.remove('open');var trigger=item.querySelector('button[aria-haspopup]');if(trigger)trigger.setAttribute('aria-expanded','false')})});document.addEventListener('keydown',function(event){if(event.key!=='Escape')return;document.querySelectorAll('.menu.open').forEach(function(item){item.classList.remove('open')})});(function(){var path=location.pathname;var map={'/':['/','/dashboard','/home'],'/datasets':['/datasets','/dataset','/kbs'],'/chats':['/chats','/chat'],'/searches':['/searches','/search'],'/agents':['/agents','/agent-list','/agent-templates'],'/memories':['/memories','/memory'],'/files':['/files']};Object.keys(map).forEach(function(key){var match=map[key].some(function(prefix){return path===prefix||path.indexOf(prefix+'/')===0});if(!match)return;var link=document.querySelector('.gnav a[data-nav=\"'+key+'\"]');if(link)link.setAttribute('aria-current','page')})})();(function(){if(location.pathname==='/login'||!localStorage.getItem('rayrag_token'))return;fetch('/api/v1/user/info').then(function(response){return response.json()}).then(function(result){var user=(result&&result.data)||{};var name=String(user.nickname||user.email||'U').trim()||'U';var avatar=document.getElementById('hdrAvatar');if(avatar)avatar.textContent=name.charAt(0).toUpperCase()}).catch(function(){})})()</script></body></html>");
    h
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UserSettingSection {
    DataSource,
    ChatChannel,
    Model,
    Mcp,
    Team,
    Profile,
    Api,
}

impl UserSettingSection {
    fn key(self) -> &'static str {
        match self {
            Self::DataSource => "/data-source",
            Self::ChatChannel => "/chat-channel",
            Self::Model => "/model",
            Self::Mcp => "/mcp",
            Self::Team => "/team",
            Self::Profile => "/profile",
            Self::Api => "/api",
        }
    }

    /// Upstream `pages/user-setting/sidebar/index.tsx` icons: LucideServer,
    /// LucideMessagesSquare, LucideBox, the MCP glyph, LucideUsers,
    /// LucideUser and LucideUnplug, inlined as SVG.
    fn icon(self) -> &'static str {
        match self {
            Self::DataSource => {
                "<svg width='16' height='16' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.7' stroke-linecap='round' stroke-linejoin='round' aria-hidden='true'><rect x='2' y='2' width='20' height='8' rx='2'/><rect x='2' y='14' width='20' height='8' rx='2'/><path d='M6 6h.01M6 18h.01'/></svg>"
            }
            Self::ChatChannel => {
                "<svg width='16' height='16' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.7' stroke-linecap='round' stroke-linejoin='round' aria-hidden='true'><path d='M14 9a2 2 0 0 1-2 2H6l-4 4V4a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2z'/><path d='M18 9h2a2 2 0 0 1 2 2v11l-4-4h-6a2 2 0 0 1-2-2v-1'/></svg>"
            }
            Self::Model => {
                "<svg width='16' height='16' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.7' stroke-linecap='round' stroke-linejoin='round' aria-hidden='true'><path d='M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z'/><path d='m3.3 7 8.7 5 8.7-5'/><path d='M12 22V12'/></svg>"
            }
            Self::Mcp => {
                "<svg width='16' height='16' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.7' stroke-linecap='round' stroke-linejoin='round' aria-hidden='true'><path d='M12 22v-5'/><path d='M9 8V2'/><path d='M15 8V2'/><path d='M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8Z'/></svg>"
            }
            Self::Team => {
                "<svg width='16' height='16' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.7' stroke-linecap='round' stroke-linejoin='round' aria-hidden='true'><path d='M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2'/><circle cx='9' cy='7' r='4'/><path d='M22 21v-2a4 4 0 0 0-3-3.87'/><path d='M16 3.13a4 4 0 0 1 0 7.75'/></svg>"
            }
            Self::Profile => {
                "<svg width='16' height='16' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.7' stroke-linecap='round' stroke-linejoin='round' aria-hidden='true'><path d='M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2'/><circle cx='12' cy='7' r='4'/></svg>"
            }
            Self::Api => {
                "<svg width='16' height='16' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.7' stroke-linecap='round' stroke-linejoin='round' aria-hidden='true'><path d='m19 5 3-3'/><path d='m2 22 3-3'/><path d='M6.3 20.3a2.4 2.4 0 0 0 3.4 0L12 18l-6-6-2.3 2.3a2.4 2.4 0 0 0 0 3.4Z'/><path d='M7.5 13.5 10 11'/><path d='M10.5 16.5 13 14'/><path d='m12 6 6 6 2.3-2.3a2.4 2.4 0 0 0 0-3.4l-2.6-2.6a2.4 2.4 0 0 0-3.4 0Z'/></svg>"
            }
        }
    }

    fn label(self, zh: bool) -> &'static str {
        match (self, zh) {
            (Self::DataSource, true) => "数据源",
            (Self::ChatChannel, true) => "聊天渠道",
            (Self::Model, true) => "模型提供商",
            (Self::Mcp, true) => "MCP",
            (Self::Team, true) => "团队",
            (Self::Profile, true) => "概要",
            (Self::Api, true) => "API",
            (Self::DataSource, false) => "Data sources",
            (Self::ChatChannel, false) => "Chat channels",
            (Self::Model, false) => "Model providers",
            (Self::Mcp, false) => "MCP",
            (Self::Team, false) => "Team",
            (Self::Profile, false) => "Profile",
            (Self::Api, false) => "API",
        }
    }
}

const USER_SETTING_SECTIONS: [UserSettingSection; 7] = [
    UserSettingSection::DataSource,
    UserSettingSection::ChatChannel,
    UserSettingSection::Model,
    UserSettingSection::Mcp,
    UserSettingSection::Team,
    UserSettingSection::Profile,
    UserSettingSection::Api,
];

/// RAGFlow's `/user-setting` subtree keeps the global header and renders one
/// responsive 64/303 px settings sidebar around every child route. User data
/// is filled through the authenticated self API so the HTML never interpolates
/// account-controlled strings.
fn user_setting_layout(title: &str, body: &str, active: UserSettingSection, zh: bool) -> String {
    let theme_label = if zh { "主题" } else { "Theme" };
    let logout_label = if zh { "退出登录" } else { "Log out" };
    let mut nav = String::new();
    for section in USER_SETTING_SECTIONS {
        let label = section.label(zh);
        let is_active = section == active;
        nav.push_str("<a class='user-setting-nav-item");
        if is_active {
            nav.push_str(" active");
        }
        nav.push_str("' href='/user-setting");
        nav.push_str(section.key());
        nav.push_str("' data-setting-key='");
        nav.push_str(section.key());
        nav.push_str("' aria-label='");
        nav.push_str(label);
        nav.push('\'');
        if is_active {
            nav.push_str(" aria-current='page'");
        }
        if section == UserSettingSection::Model {
            nav.push_str(" data-testid='settings-nav-model-providers'");
        }
        nav.push_str("><span class='user-setting-nav-icon' aria-hidden='true'>");
        nav.push_str(section.icon());
        nav.push_str("</span><span class='user-setting-nav-label'>");
        nav.push_str(label);
        nav.push_str("</span></a>");
    }

    // Upstream footer: version + ThemeSwitch on one row, then the Log out
    // button (`setting.logout`).
    let theme_pressed = if zh { "深色" } else { "dark" };
    let shell = format!(
        r#"<div class='user-setting-layout' data-testid='user-setting-layout'><aside class='user-setting-sidebar' aria-label='{sidebar_label}'><div class='user-setting-account'><span class='user-setting-avatar' id='userSettingAvatar' aria-hidden='true'>U</span><span class='user-setting-email' id='userSettingEmail'>...</span></div><nav class='user-setting-nav'>{nav}</nav><footer class='user-setting-footer'><div class='user-setting-footer-row'><span class='user-setting-version'>{version}</span><button type='button' class='theme-switch' role='switch' aria-checked='true' aria-label='{theme_label}' data-theme-state='{theme_pressed}' onclick='toggleTheme()'><span class='theme-switch-track' aria-hidden='true'><svg class='theme-sun' width='14' height='14' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.8' stroke-linecap='round'><circle cx='12' cy='12' r='4'/><path d='M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4'/></svg><svg class='theme-moon' width='14' height='14' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.8' stroke-linecap='round' stroke-linejoin='round'><path d='M21 12.8A8.5 8.5 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8Z'/></svg></span></button></div><button type='button' class='user-setting-footer-button user-setting-logout' aria-label='{logout_label}' onclick='logoutUser()'><span aria-hidden='true'><svg width='16' height='16' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.7' stroke-linecap='round' stroke-linejoin='round'><path d='M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4'/><path d='m16 17 5-5-5-5'/><path d='M21 12H9'/></svg></span><span class='user-setting-footer-label'>{logout_label}</span></button></footer></aside><main class='user-setting-content' data-setting-page='{active_key}'>{body}</main></div><script>function logoutUser(){{localStorage.removeItem('rayrag_token');document.cookie='rayrag_token=;path=/;max-age=0';location.href='/login'}}function settingUnauthorized(){{logoutUser()}}function loadSettingUser(){{fetch('/api/v1/user/info').then(function(response){{return response.json()}}).then(function(result){{if(result.code===401){{settingUnauthorized();return}}var user=result.data||{{}};var name=(user.nickname||user.email||'U').trim();document.getElementById('userSettingAvatar').textContent=name.charAt(0).toUpperCase()||'U';document.getElementById('userSettingEmail').textContent=user.email||user.nickname||''}}).catch(function(){{}})}}loadSettingUser()</script>"#,
        version = crate::build_info::version_line(),
        active_key = active.key(),
        sidebar_label = if zh { "用户设置" } else { "User settings" },
    );
    layout(title, &shell, zh)
}

pub async fn user_setting_index() -> Redirect {
    Redirect::temporary("/user-setting/data-source")
}

pub async fn user_setting_profile_page(
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let body = r#"<h1>Profile</h1><div class='card' data-testid='profile-card'><div class='profile-rows'><div class='profile-row'><label>Username</label><div class='profile-value'><span id='profileNickname'>...</span><button class='btn btn-o btn-sm' type='button' onclick='openProfileEdit("name")'>Edit</button></div></div><div class='profile-row'><label>Avatar</label><div class='profile-value'><img id='profileAvatar' class='profile-avatar' alt='Avatar' hidden><span id='profileAvatarFallback' class='profile-avatar profile-avatar-letter'>A</span><input id='profileAvatarFile' type='file' accept='image/*' hidden onchange='uploadAvatar()'><button class='btn btn-o btn-sm' type='button' onclick='document.getElementById("profileAvatarFile").click()'>Upload</button></div></div><div class='profile-row'><label>Time Zone</label><div class='profile-value'><span id='profileTimezone'>...</span><button class='btn btn-o btn-sm' type='button' onclick='openProfileEdit("timezone")'>Edit</button></div></div><div class='profile-row'><label>Email</label><div class='profile-value'><span id='profileEmail'>...</span><span class='user-setting-description' style='margin:0'>The email is your login account and cannot be changed here.</span></div></div><div class='profile-row'><label>Password</label><div class='profile-value'><span>********</span><button class='btn btn-o btn-sm' type='button' onclick='openProfileEdit("password")'>Edit</button></div></div></div></div><div class='modal-overlay' id='profileEditDialog' data-testid='profile-edit-dialog' onclick='if(event.target===this)closeProfileEdit()'><div class='modal' style='max-width:440px' role='dialog' aria-modal='true' aria-labelledby='profileEditTitle'><h2 id='profileEditTitle'>Edit Name</h2><div id='profileNameFields'><label class='provider-instance-field'>Username<input id='profileEditName' placeholder='Username' autocomplete='off'><p id='profileEditNameError' class='provider-instance-error'></p></label></div><div id='profileTimezoneFields' hidden><label class='provider-instance-field'>Time Zone<input id='profileEditTimezone' placeholder='Select a timeZone' autocomplete='off' oninput='tzRenderList()'><div id='profileTzList' class='provider-tz-list'></div><p id='profileEditTimezoneError' class='provider-instance-error'></p></label></div><div id='profilePasswordFields' hidden><label class='provider-instance-field'>Current Password<input id='profileCurPw' type='password' autocomplete='current-password'><p id='profileCurPwError' class='provider-instance-error'></p></label><label class='provider-instance-field'>New Password<input id='profileNewPw' type='password' placeholder='At least 8 characters' autocomplete='new-password'><p id='profileNewPwError' class='provider-instance-error'></p></label><label class='provider-instance-field'>Confirm Password<input id='profileConfirmPw' type='password' autocomplete='new-password'><p id='profileConfirmPwError' class='provider-instance-error'></p></label></div><p id='profileEditStatus' class='provider-instance-status' aria-live='polite'></p><div class='provider-instance-actions'><button class='btn btn-o' type='button' onclick='closeProfileEdit()'>Cancel</button><button class='btn btn-p' id='profileEditSave' type='button' onclick='saveProfileEdit()'>Save</button></div></div></div><script>var PROFILE_TIMEZONES=(function(){var src=(typeof Intl.supportedValuesOf==='function')?Intl.supportedValuesOf('timeZone'):[];var arr=src.map(function(tz){var dtf=new Intl.DateTimeFormat('en-US',{hourCycle:'h24',timeZone:tz,timeZoneName:'longOffset'});var parts=dtf.formatToParts(new Date());var offsetString=parts[parts.length-1].value;var m=/^GMT([+-])(\d{2}):(\d{2})$/i.exec(offsetString);var sign=m?m[1]:'+';var hours=m?m[2]:'00';var minutes=m?m[3]:'00';return{name:offsetString+' '+tz,id:tz,offset:(sign==='-'?-1:1)*(Number(hours)*60+Number(minutes)),offsetString:offsetString}});arr.sort(function(a,b){return a.offset-b.offset});return arr})();var PROFILE_LOADED={timezone:''};var profileEditType='name';function profileApi(url,options){return fetch(url,options).then(function(response){return response.json()})}async function loadProfile(){var data=await profileApi('/api/v1/user/info');var user=data.data||{};document.getElementById('profileNickname').textContent=user.nickname||'';document.getElementById('profileEmail').textContent=user.email||'';var storedTz=user.timezone||'';PROFILE_LOADED.timezone=storedTz;document.getElementById('profileTimezone').textContent=(PROFILE_TIMEZONES.find(function(tz){return tz.name===storedTz})||{}).name||'';if(user.avatar){document.getElementById('profileAvatar').src=user.avatar;document.getElementById('profileAvatar').hidden=false;document.getElementById('profileAvatarFallback').hidden=true}else{document.getElementById('profileAvatar').hidden=true;document.getElementById('profileAvatarFallback').hidden=false;document.getElementById('profileAvatarFallback').textContent=(user.nickname||'A').charAt(0).toUpperCase()}}function openProfileEdit(type){profileEditType=type;var titles={name:'Edit Name',timezone:'Edit Time Zone',password:'Edit Password'};document.getElementById('profileEditTitle').textContent=titles[type]||'Edit';document.getElementById('profileNameFields').hidden=type!=='name';document.getElementById('profileTimezoneFields').hidden=type!=='timezone';document.getElementById('profilePasswordFields').hidden=type!=='password';['profileEditNameError','profileEditTimezoneError','profileCurPwError','profileNewPwError','profileConfirmPwError'].forEach(function(id){document.getElementById(id).textContent=''});document.getElementById('profileEditStatus').textContent='';if(type==='name'){document.getElementById('profileEditName').value=document.getElementById('profileNickname').textContent}if(type==='timezone'){var tzInput=document.getElementById('profileEditTimezone');tzInput.value=PROFILE_LOADED.timezone;tzRenderList()}if(type==='password'){['profileCurPw','profileNewPw','profileConfirmPw'].forEach(function(id){document.getElementById(id).value=''})}document.getElementById('profileEditDialog').classList.add('show')}function closeProfileEdit(){document.getElementById('profileEditDialog').classList.remove('show');document.getElementById('profileTzList').replaceChildren()}function tzRenderList(){var input=document.getElementById('profileEditTimezone');var q=input.value.trim().toLowerCase();var list=document.getElementById('profileTzList');list.replaceChildren();PROFILE_TIMEZONES.forEach(function(tz){if(q&&tz.name.toLowerCase().indexOf(q)<0)return;var item=document.createElement('div');item.textContent=tz.name;item.onclick=function(){input.value=tz.name;list.replaceChildren()};list.appendChild(item)})}async function saveProfileEdit(){var status=document.getElementById('profileEditStatus');var payload={};if(profileEditType==='name'){var name=document.getElementById('profileEditName').value.trim();if(!name){document.getElementById('profileEditNameError').textContent='Username is required';return}if(name.length>100){document.getElementById('profileEditNameError').textContent='Username must be at most 100 characters';return}payload={nickname:name}}else if(profileEditType==='timezone'){var tzValue=document.getElementById('profileEditTimezone').value.trim();if(!tzValue){document.getElementById('profileEditTimezoneError').textContent='select your timezone';return}payload={timezone:tzValue}}else{var current=document.getElementById('profileCurPw').value;var next=document.getElementById('profileNewPw').value;var confirm=document.getElementById('profileConfirmPw').value;var valid=true;if(!current){document.getElementById('profileCurPwError').textContent='Please input the current password';valid=false}if(next.length<8){document.getElementById('profileNewPwError').textContent='The new password must be at least 8 characters';valid=false}if(next!==confirm){document.getElementById('profileConfirmPwError').textContent='The two passwords do not match';valid=false}if(!valid)return;payload={password:current,new_password:next}}var button=document.getElementById('profileEditSave');button.disabled=true;status.textContent='Saving…';try{var data=await profileApi('/api/v1/users/me',{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify(payload)});if(data.code!==0)throw new Error(data.message||('HTTP '+data.code));if(profileEditType==='password'){localStorage.removeItem('rayrag_token');document.cookie='rayrag_token=; path=/; max-age=0';location.href='/login';return}closeProfileEdit();loadProfile()}catch(error){status.textContent=error.message}finally{button.disabled=false}}async function uploadAvatar(){var file=document.getElementById('profileAvatarFile').files[0];if(!file)return;if(!file.type.startsWith('image/')){alert('Please select an image file');return}var dataUrl=await new Promise(function(resolve,reject){var reader=new FileReader();reader.onload=function(){resolve(reader.result)};reader.onerror=function(){reject(reader.error)};reader.readAsDataURL(file)});var data=await profileApi('/api/v1/users/me',{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({avatar:dataUrl})});if(data.code===0)loadProfile();else alert('Error: '+(data.message||'unknown'))}loadProfile()</script>"#;
    Html(user_setting_layout(
        if zh { "个人资料" } else { "Profile" },
        body,
        UserSettingSection::Profile,
        zh,
    ))
}

pub async fn user_setting_team_page(
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let title = if zh { "团队" } else { "Team" };
    let body = r#"<h1>Team</h1><div class='card' data-testid='team-members-card'><div class='user-setting-card-head'><div><h2>Team members</h2><span id='teamWorkspaceTitle' class='team-workspace-label'>Workspace</span></div><div class='user-setting-actions'><input id='memberSearch' type='search' placeholder='Search' aria-label='Search team members' oninput='renderMemberTable()'><button class='btn btn-p btn-sm' type='button' onclick='openTeamInvite()'>Invite member</button></div></div><div class='user-setting-table-wrap'><table class='team-table'><thead><tr><th>Name</th><th><button class='team-sort' type='button' id='teamSort' onclick='toggleMemberSort()'>Date <span id='teamSortIcon'>↕</span></button></th><th>Email</th><th>State</th><th>Action</th></tr></thead><tbody id='memberRows'></tbody></table></div></div><div class='card' data-testid='joined-teams-card'><div class='user-setting-card-head'><h2>Joined teams</h2><div class='user-setting-actions'><input id='teamSearch' type='search' placeholder='Search' aria-label='Search joined teams' oninput='renderTeamTable()'></div></div><div class='user-setting-table-wrap'><table class='team-table'><thead><tr><th>Name</th><th>State</th><th>Action</th></tr></thead><tbody id='teamRows'></tbody></table></div></div><div class='modal-overlay' id='teamInviteDialog' data-testid='team-invite-dialog' onclick='if(event.target===this)closeTeamInvite()'><div class='modal' style='max-width:440px' role='dialog' aria-modal='true' aria-labelledby='teamInviteTitle'><h2 id='teamInviteTitle'>Add</h2><label class='provider-instance-field'>Email<input id='inviteEmail' type='email' autocomplete='email' placeholder='Email'><p id='inviteEmailError' class='provider-instance-error'></p></label><p class='user-setting-description'>Only registered users can be invited. Please register the account before sending an invitation.</p><p id='inviteStatus' class='provider-instance-status' aria-live='polite'></p><div class='provider-instance-actions'><button class='btn btn-o' type='button' onclick='closeTeamInvite()'>Cancel</button><button class='btn btn-p' id='inviteOk' type='button' onclick='submitTeamInvite()'>Ok</button></div></div></div><div class='modal-overlay' id='teamConfirmDialog' data-testid='team-confirm-dialog' onclick='if(event.target===this)closeTeamConfirm()'><div class='modal' style='max-width:440px' role='alertdialog' aria-modal='true' aria-labelledby='teamConfirmTitle'><h2 id='teamConfirmTitle'>Confirm</h2><p id='teamConfirmText' class='user-setting-description'></p><p id='teamConfirmStatus' class='provider-instance-status' aria-live='polite'></p><div class='provider-instance-actions'><button class='btn btn-o' type='button' onclick='closeTeamConfirm()'>Cancel</button><button class='btn btn-d' id='teamConfirmOk' type='button' onclick='runTeamConfirm()'>Delete</button></div></div></div><script>var teamState={members:[],teams:[],memberSort:null,memberFilter:'',teamFilter:'',selfId:null,confirmAction:null};function teamApi(url,options){return fetch(url,options).then(function(response){return response.json()})}function formatTeamDate(ms){if(!ms)return '';var date=new Date(ms);var pad=function(value){return String(value).padStart(2,'0')};return date.getFullYear()+'-'+pad(date.getMonth()+1)+'-'+pad(date.getDate())+' '+pad(date.getHours())+':'+pad(date.getMinutes())}function teamRoleBadge(role){var label={owner:'Owner',admin:'Admin',normal:'Member',invite:'Invited'}[role]||role;return "<span class='badge team-role-"+role+"'>"+label+"</span>"}async function loadTeam(){var info=await teamApi('/api/v1/user/info');var me=(info.data&&info.data.id)||null;teamState.selfId=me;var workspace=(info.data&&info.data.nickname)||'';document.getElementById('teamWorkspaceTitle').textContent=workspace+' workspace';if(me){var members=await teamApi('/api/v1/tenants/'+encodeURIComponent(me)+'/users');teamState.members=members.data||[]}var teams=await teamApi('/api/v1/tenants');teamState.teams=teams.data||[];renderMemberTable();renderTeamTable()}function sortedMembers(){var rows=teamState.members.filter(function(row){var query=teamState.memberFilter.toLowerCase();return !query||(row.nickname||'').toLowerCase().indexOf(query)>=0||(row.email||'').toLowerCase().indexOf(query)>=0});if(teamState.memberSort){rows=rows.slice().sort(function(left,right){var diff=(left.update_date||0)-(right.update_date||0);return teamState.memberSort==='asc'?diff:-diff})}return rows}function renderMemberTable(){teamState.memberFilter=document.getElementById('memberSearch').value;var body=document.getElementById('memberRows');body.replaceChildren();sortedMembers().forEach(function(row){var tr=document.createElement('tr');tr.dataset.testid='team-member-row';var name=document.createElement('td');name.textContent=row.nickname||'';var date=document.createElement('td');date.textContent=formatTeamDate(row.update_date);var email=document.createElement('td');email.textContent=row.email||'';var role=document.createElement('td');role.innerHTML=teamRoleBadge(row.role);var action=document.createElement('td');if(row.role!=='owner'){var remove=document.createElement('button');remove.type='button';remove.className='btn btn-d btn-sm';remove.textContent='Delete';remove.onclick=function(){confirmRemoveMember(row)};action.appendChild(remove)}tr.append(name,date,email,role,action);body.appendChild(tr)});if(!sortedMembers().length){var tr=document.createElement('tr');var td=document.createElement('td');td.colSpan=5;td.className='team-empty';td.textContent='No members.';tr.appendChild(td);body.appendChild(tr)}}function toggleMemberSort(){teamState.memberSort=teamState.memberSort==='asc'?'desc':teamState.memberSort==='desc'?null:'asc';document.getElementById('teamSortIcon').textContent=teamState.memberSort==='asc'?'↑':teamState.memberSort==='desc'?'↓':'↕';renderMemberTable()}function sortedTeams(){var query=teamState.teamFilter.toLowerCase();return teamState.teams.filter(function(row){var name=row.tenant_id===teamState.selfId?(document.getElementById('teamWorkspaceTitle').textContent.split(' workspace')[0]):row.tenant_id;return !query||name.toLowerCase().indexOf(query)>=0})}function renderTeamTable(){teamState.teamFilter=document.getElementById('teamSearch').value;var body=document.getElementById('teamRows');body.replaceChildren();sortedTeams().forEach(function(row){var tr=document.createElement('tr');tr.dataset.testid='joined-team-row';var name=document.createElement('td');name.textContent=row.tenant_id===teamState.selfId?(document.getElementById('teamWorkspaceTitle').textContent.split(' workspace')[0]):row.tenant_id;var role=document.createElement('td');role.innerHTML=teamRoleBadge(row.role||'normal');var action=document.createElement('td');if(row.role==='invite'){var accept=document.createElement('button');accept.type='button';accept.className='btn btn-p btn-sm';accept.textContent='Accept';accept.onclick=function(){agreeTeam(row.tenant_id)};action.appendChild(accept);var decline=document.createElement('button');decline.type='button';decline.className='btn btn-o btn-sm';decline.textContent='Decline';decline.onclick=function(){declineTeam(row.tenant_id)};action.appendChild(decline)}else{var quit=document.createElement('button');quit.type='button';quit.className='btn btn-o btn-sm';quit.textContent='Quit';quit.onclick=function(){confirmQuitTeam(row.tenant_id)};action.appendChild(quit)}tr.append(name,role,action);body.appendChild(tr)});if(!sortedTeams().length){var tr=document.createElement('tr');var td=document.createElement('td');td.colSpan=3;td.className='team-empty';td.textContent='No joined teams.';tr.appendChild(td);body.appendChild(tr)}}function openTeamInvite(){document.getElementById('inviteEmail').value='';document.getElementById('inviteEmailError').textContent='';document.getElementById('inviteStatus').textContent='';document.getElementById('teamInviteDialog').classList.add('show')}function closeTeamInvite(){document.getElementById('teamInviteDialog').classList.remove('show')}async function submitTeamInvite(){var email=document.getElementById('inviteEmail').value.trim();var error=document.getElementById('inviteEmailError');if(!email){error.textContent='Please input email';return}if(!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email)){error.textContent='Please input a valid email';return}var button=document.getElementById('inviteOk');button.disabled=true;document.getElementById('inviteStatus').textContent='Sending…';try{var data=await teamApi('/api/v1/tenants/'+encodeURIComponent(teamState.selfId)+'/users',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({email:email})});if(data.code!==0)throw new Error(data.message||('HTTP '+data.code));closeTeamInvite();loadTeam()}catch(caught){document.getElementById('inviteStatus').textContent=caught.message}finally{button.disabled=false}}function confirmRemoveMember(row){teamState.confirmAction=function(){return removeMember(row.user_id)};document.getElementById('teamConfirmText').textContent='Are you sure to remove this member?';document.getElementById('teamConfirmStatus').textContent='';document.getElementById('teamConfirmDialog').classList.add('show')}async function removeMember(userId){return teamApi('/api/v1/tenants/'+encodeURIComponent(teamState.selfId)+'/users',{method:'DELETE',headers:{'Content-Type':'application/json'},body:JSON.stringify({user_id:userId})})}async function agreeTeam(tenantId){var data=await teamApi('/api/v1/tenants/'+encodeURIComponent(tenantId),{method:'PATCH'});if(data.code===0)loadTeam();else alert('Error: '+(data.message||'unknown'))}function declineTeam(tenantId){teamState.confirmAction=function(){return quitTeam(tenantId)};document.getElementById('teamConfirmText').textContent='Are you sure you want to quit the team you joined?';document.getElementById('teamConfirmStatus').textContent='';document.getElementById('teamConfirmDialog').classList.add('show')}function confirmQuitTeam(tenantId){teamState.confirmAction=function(){return quitTeam(tenantId)};document.getElementById('teamConfirmText').textContent='Are you sure you want to quit the team you joined?';document.getElementById('teamConfirmStatus').textContent='';document.getElementById('teamConfirmDialog').classList.add('show')}async function quitTeam(tenantId){return teamApi('/api/v1/tenants/'+encodeURIComponent(tenantId)+'/users',{method:'DELETE',headers:{'Content-Type':'application/json'},body:JSON.stringify({user_id:teamState.selfId})})}function closeTeamConfirm(){document.getElementById('teamConfirmDialog').classList.remove('show')}async function runTeamConfirm(){var button=document.getElementById('teamConfirmOk');button.disabled=true;document.getElementById('teamConfirmStatus').textContent='Processing…';try{var data=await teamState.confirmAction();if(data.code!==0)throw new Error(data.message||('HTTP '+data.code));closeTeamConfirm();loadTeam()}catch(error){document.getElementById('teamConfirmStatus').textContent=error.message}finally{button.disabled=false}}loadTeam()</script>"#;
    Html(user_setting_layout(
        title,
        body,
        UserSettingSection::Team,
        zh,
    ))
}

pub async fn user_setting_api_page(
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let body = r#"<h1>API</h1><div class='card'><h2>API KEY</h2><p class='user-setting-description'>Generate and manage personal API keys.</p><button type='button' class='btn btn-p' onclick='generateSettingKey()'>Generate</button><div id='settingApiKeyList' class='user-setting-table-wrap' aria-live='polite'>Loading...</div></div><div class='card' data-testid='langfuse-card'><div class='user-setting-card-head'><h2>Langfuse</h2><div class='user-setting-actions'><a id='langfuseView' class='btn btn-o btn-sm' target='_blank' rel='noopener noreferrer' hidden>View</a><button type='button' class='btn btn-p btn-sm' onclick='openLangfuseDialog()'>Configuration</button></div></div><p class='user-setting-description'>Traces, evals, prompt management and metrics to debug and improve your LLM application.</p></div><div class='modal-overlay' id='langfuseDialog' data-testid='langfuse-config-dialog' onclick='if(event.target===this)closeLangfuseDialog()'><div class='modal' style='max-width:520px' role='dialog' aria-modal='true' aria-labelledby='langfuseDialogTitle'><h2 id='langfuseDialogTitle'>Configuration Langfuse</h2><label class='provider-instance-field'>Secret key<input id='lfSecret' type='password' placeholder='Please enter the secret key' autocomplete='new-password'></label><p id='lfSecretError' class='provider-instance-error'></p><label class='provider-instance-field'>Public key<input id='lfPublic' type='password' placeholder='Please enter the public key' autocomplete='new-password'></label><p id='lfPublicError' class='provider-instance-error'></p><label class='provider-instance-field'>Host<input id='lfHost' placeholder='https://cloud.langfuse.com' autocomplete='off'></label><p id='lfHostError' class='provider-instance-error'></p><p id='lfStatus' class='provider-instance-status' aria-live='polite'></p><div class='langfuse-dialog-footer'><a class='langfuse-docs-link' href='https://langfuse.com/docs' target='_blank' rel='noopener noreferrer'>View Langfuse&#39;s documentation ↗</a><div class='provider-instance-actions'><button class='btn btn-d' id='lfDelete' type='button' onclick='confirmDeleteLangfuse()'>Delete</button><button class='btn btn-o' type='button' onclick='closeLangfuseDialog()'>Cancel</button><button class='btn btn-p' id='lfSave' type='button' onclick='saveLangfuse()'>Save</button></div></div></div></div><div class='modal-overlay' id='langfuseDeleteDialog' data-testid='langfuse-delete-dialog' onclick='if(event.target===this)closeLangfuseDeleteDialog()'><div class='modal' style='max-width:440px' role='alertdialog' aria-modal='true' aria-labelledby='langfuseDeleteTitle'><h2 id='langfuseDeleteTitle'>Delete Langfuse configuration</h2><p id='langfuseDeleteText' class='user-setting-description'>Are you sure you want to delete the Langfuse configuration?</p><p id='lfDeleteStatus' class='provider-instance-status' aria-live='polite'></p><div class='provider-instance-actions'><button class='btn btn-o' type='button' onclick='closeLangfuseDeleteDialog()'>Cancel</button><button class='btn btn-d' id='lfDeleteOk' type='button' onclick='deleteLangfuse()'>Delete</button></div></div></div><script>function settingApiCall(url,options){return fetch(url,options).then(function(r){return r.json()})}function loadSettingKeys(){settingApiCall('/api/v1/system/api_keys').then(function(d){var list=document.getElementById('settingApiKeyList');list.replaceChildren();var keys=d.data||[];if(!keys.length){var empty=document.createElement('p');empty.className='user-setting-description';empty.textContent='No API keys yet.';list.appendChild(empty);return}var table=document.createElement('table');var head=document.createElement('thead');head.innerHTML='<tr><th>Key</th><th>Created</th><th></th></tr>';table.appendChild(head);var tbody=document.createElement('tbody');keys.forEach(function(k){var row=document.createElement('tr');var key=document.createElement('td');key.className='co';key.textContent=k.token;var created=document.createElement('td');created.textContent=k.create_date||'';var action=document.createElement('td');var button=document.createElement('button');button.type='button';button.className='btn btn-d btn-sm';button.textContent='Delete';button.addEventListener('click',function(){deleteSettingKey(k.token)});action.appendChild(button);row.append(key,created,action);tbody.appendChild(row)});table.appendChild(tbody);list.appendChild(table)})}function generateSettingKey(){settingApiCall('/api/v1/system/api_keys',{method:'POST'}).then(function(d){if(d.code===0)loadSettingKeys();else alert('Error: '+(d.message||'unknown'))})}function deleteSettingKey(key){if(!confirm('Delete this API key?'))return;settingApiCall('/api/v1/system/api_keys/'+encodeURIComponent(key),{method:'DELETE'}).then(function(d){if(d.code===0)loadSettingKeys();else alert('Error: '+(d.message||'unknown'))})}function langfuseStatus(text,error){var status=document.getElementById('lfStatus');status.textContent=text||'';status.style.color=error?'var(--d)':'var(--tm)'}function langfuseViewUrl(config){var host=(config&&config.host||'').replace(/\/+$/,'');if(!host||host==='https://cloud.langfuse.com')return 'https://cloud.langfuse.com/project/'+(config&&config.project_id||'');return host+'/project/'+(config&&config.project_id||'')}function loadLangfuse(){settingApiCall('/api/v1/langfuse/api-key').then(function(d){var view=document.getElementById('langfuseView');var config=d.data;if(!config){view.hidden=true;view.removeAttribute('href');return}view.href=langfuseViewUrl(config);view.hidden=false}).catch(function(){document.getElementById('langfuseView').hidden=true})}function openLangfuseDialog(){['lfSecret','lfPublic','lfHost'].forEach(function(id){document.getElementById(id).value=''});['lfSecretError','lfPublicError','lfHostError'].forEach(function(id){document.getElementById(id).textContent=''});langfuseStatus('',false);document.getElementById('lfDelete').hidden=true;settingApiCall('/api/v1/langfuse/api-key').then(function(d){if(d.data){document.getElementById('lfSecret').value=d.data.secret_key||'';document.getElementById('lfPublic').value=d.data.public_key||'';document.getElementById('lfHost').value=d.data.host||'';document.getElementById('lfDelete').hidden=false}}).catch(function(){});document.getElementById('langfuseDialog').classList.add('show')}function closeLangfuseDialog(){document.getElementById('langfuseDialog').classList.remove('show')}function saveLangfuse(){['lfSecretError','lfPublicError','lfHostError'].forEach(function(id){document.getElementById(id).textContent=''});var valid=true;if(!document.getElementById('lfSecret').value.trim()){document.getElementById('lfSecretError').textContent='Please enter the secret key';valid=false}if(!document.getElementById('lfPublic').value.trim()){document.getElementById('lfPublicError').textContent='Please enter the public key';valid=false}if(!valid)return;var button=document.getElementById('lfSave');button.disabled=true;langfuseStatus('Saving…',false);settingApiCall('/api/v1/langfuse/api-key',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({secret_key:document.getElementById('lfSecret').value,public_key:document.getElementById('lfPublic').value,host:document.getElementById('lfHost').value.trim()})}).then(function(d){if(d.code!==0)throw new Error(d.message||('HTTP '+d.code));closeLangfuseDialog();loadLangfuse()}).catch(function(error){langfuseStatus(error.message,true)}).finally(function(){button.disabled=false})}function confirmDeleteLangfuse(){closeLangfuseDialog();document.getElementById('lfDeleteStatus').textContent='';document.getElementById('langfuseDeleteDialog').classList.add('show')}function closeLangfuseDeleteDialog(){document.getElementById('langfuseDeleteDialog').classList.remove('show')}function deleteLangfuse(){var button=document.getElementById('lfDeleteOk');button.disabled=true;document.getElementById('lfDeleteStatus').textContent='Deleting…';settingApiCall('/api/v1/langfuse/api-key',{method:'DELETE'}).then(function(d){if(d.code!==0)throw new Error(d.message||('HTTP '+d.code));closeLangfuseDeleteDialog();loadLangfuse()}).catch(function(error){document.getElementById('lfDeleteStatus').textContent=error.message}).finally(function(){button.disabled=false})}loadSettingKeys();loadLangfuse()</script>"#;
    Html(user_setting_layout(
        "API",
        body,
        UserSettingSection::Api,
        zh,
    ))
}

pub async fn user_setting_mcp_page(
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let body = r#"<h1>MCP</h1><div class='card' data-testid='mcp-servers-card'><div class='user-setting-card-head'><div><h2>MCP servers</h2><p class='user-setting-description'>Customize the list of MCP servers</p></div><div class='user-setting-actions mcp-toolbar' role='toolbar'><input id='mcpSearch' type='search' placeholder='Search' aria-label='Search MCP servers' oninput='debounceMcpSearch()'><button class='btn btn-o btn-sm' id='mcpBulkToggle' type='button' onclick='toggleMcpBulk()'>Bulk manage</button><button class='btn btn-o btn-sm' type='button' onclick='openMcpImport()'>Import</button><button class='btn btn-p btn-sm' type='button' onclick='openMcpDialog()'>+ Add MCP</button></div></div><div id='mcpBulkBar' class='mcp-bulk-bar' hidden><label><input type='checkbox' id='mcpSelectAll' onchange='toggleMcpSelectAll()'> Select all</label><span id='mcpSelectedCount' class='user-setting-description' style='margin:0'>Selected 0</span><div class='user-setting-actions'><button class='btn btn-o btn-sm' type='button' onclick='exportMcpSelected()'>Export</button><button class='btn btn-d btn-sm' type='button' onclick='confirmDeleteMcpSelected()'>Delete</button></div></div><div id='mcpList' class='user-setting-card-grid mcp-grid' aria-live='polite'></div><div id='mcpEmpty' class='mcp-empty' hidden onclick='openMcpDialog()'><p class='user-setting-description'>No MCP servers available</p><button class='btn btn-o btn-sm' type='button'>+ Add Now</button></div></div><div class='modal-overlay' id='mcpDialog' data-testid='mcp-edit-dialog' onclick='if(event.target===this)closeMcpDialog()'><div class='modal' style='max-width:540px' role='dialog' aria-modal='true' aria-labelledby='mcpDialogTitle'><h2 id='mcpDialogTitle'>Add MCP</h2><label class='provider-instance-field'>Name<input id='mcpName' oninput='mcpFieldChanged()' placeholder='My MCP server' autocomplete='off'><p id='mcpNameError' class='provider-instance-error'></p></label><label class='provider-instance-field'>URL<input id='mcpUrl' oninput='mcpFieldChanged()' placeholder='https://api.example.com/v1/mcp' autocomplete='off'><p id='mcpUrlError' class='provider-instance-error'></p></label><label class='provider-instance-field'>Server type<select id='mcpServerType' onchange='mcpFieldChanged()'><option value='sse'>SSE</option><option value='streamable-http'>Streamable HTTP</option></select><p id='mcpTypeError' class='provider-instance-error'></p></label><label class='provider-instance-field'>Authorization Token<input id='mcpToken' oninput='mcpFieldChanged()' type='password' placeholder='e.g. eyJhbGciOiJIUzI1Ni...' autocomplete='new-password'></label><div class='mcp-tools-box'><div class='mcp-tools-head'><span id='mcpToolsCount'>0 tools available</span><button class='btn btn-o btn-sm' type='button' onclick='testMcpServer()'>Test</button></div><div id='mcpToolsList' class='mcp-tools-list'><p class='user-setting-description'>Test the connection to load tools.</p></div></div><p id='mcpStatus' class='provider-instance-status' aria-live='polite'></p><div class='provider-instance-actions'><button class='btn btn-o' type='button' onclick='closeMcpDialog()'>Cancel</button><button class='btn btn-p' id='mcpSave' type='button' onclick='saveMcpServer()'>Save</button></div></div></div><div class='modal-overlay' id='mcpImportDialog' data-testid='mcp-import-dialog' onclick='if(event.target===this)closeMcpImport()'><div class='modal' style='max-width:480px' role='dialog' aria-modal='true' aria-labelledby='mcpImportTitle'><h2 id='mcpImportTitle'>Import</h2><label class='provider-instance-field'>Name<input id='mcpImportFile' type='file' accept='.json,application/json'><p id='mcpImportError' class='provider-instance-error'></p></label><div id='mcpImportResult' class='mcp-import-result'></div><div class='provider-instance-actions'><button class='btn btn-o' type='button' onclick='closeMcpImport()'>Cancel</button><button class='btn btn-p' id='mcpImportSave' type='button' onclick='importMcpServers()'>Save</button></div></div></div><div class='modal-overlay' id='mcpDeleteDialog' data-testid='mcp-delete-dialog' onclick='if(event.target===this)closeMcpDelete()'><div class='modal' style='max-width:440px' role='alertdialog' aria-modal='true' aria-labelledby='mcpDeleteTitle'><h2 id='mcpDeleteTitle'>Delete MCP server</h2><p id='mcpDeleteText' class='user-setting-description'></p><p id='mcpDeleteStatus' class='provider-instance-status' aria-live='polite'></p><div class='provider-instance-actions'><button class='btn btn-o' type='button' onclick='closeMcpDelete()'>Cancel</button><button class='btn btn-d' id='mcpDeleteOk' type='button' onclick='deleteMcpServer()'>Delete</button></div></div></div><script>var mcpState={servers:[],bulk:false,selected:new Set(),editingId:null,searchTimer:null,toolsTested:false,fieldChanged:false};function mcpApi(url,options){return fetch(url,options).then(function(response){return response.json()})}function debounceMcpSearch(){clearTimeout(mcpState.searchTimer);mcpState.searchTimer=setTimeout(loadMcpServers,180)}function toggleMcpBulk(){mcpState.bulk=!mcpState.bulk;mcpState.selected.clear();document.getElementById('mcpBulkToggle').textContent=mcpState.bulk?'Exit bulk manage':'Bulk manage';document.getElementById('mcpBulkBar').hidden=!mcpState.bulk;renderMcpServers()}function toggleMcpSelectAll(){var all=document.getElementById('mcpSelectAll').checked;mcpState.selected.clear();if(all)mcpState.servers.forEach(function(server){mcpState.selected.add(server.id)});renderMcpServers()}function formatMcpDate(ms){if(!ms)return '';var date=new Date(ms);var pad=function(value){return String(value).padStart(2,'0')};return date.getFullYear()+'-'+pad(date.getMonth()+1)+'-'+pad(date.getDate())+' '+pad(date.getHours())+':'+pad(date.getMinutes())}function toolCount(server){var tools=server.variables&&server.variables.tools;if(tools&&typeof tools==='object')return Object.keys(tools).length;return 0}async function loadMcpServers(){var query=new URLSearchParams();if(document.getElementById('mcpSearch').value.trim())query.set('keywords',document.getElementById('mcpSearch').value.trim());var data=await mcpApi('/api/v1/mcp/servers'+(query.toString()?'?'+query.toString():''));mcpState.servers=(data.data&&data.data.mcp_servers)||[];renderMcpServers()}function renderMcpServers(){var list=document.getElementById('mcpList');list.replaceChildren();document.getElementById('mcpEmpty').hidden=!!mcpState.servers.length;document.getElementById('mcpSelectedCount').textContent='Selected '+mcpState.selected.size;mcpState.servers.forEach(function(server){list.appendChild(renderMcpCard(server))})}var mcpIconsUpload=['M21 15v4a2 2 0 0 1-2 2H5a2 2 5-5-4 0v-4','M17 8l-5-5-5 5','M12 3v12'];var mcpIconsPen=['M12 20h9','M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4Z'];var mcpIconTrash=['M3 6h18','M19 6v14a2 2 5-5-2-2H7a2 2-5-2-2V6','M8 6V4a2 2-1 4c1 0 2 1 2 2v2','M10 11v6','M14 11v6'];function mcpIconButton(paths,label,onClick,danger){var button=document.createElement('button');button.type='button';button.className='mcp-card-icon'+(danger?' mcp-card-icon-danger':'');button.setAttribute('aria-label',label);button.onclick=onClick;var svg=document.createElementNS('http://www.w3.org/2000/svg','svg');svg.setAttribute('viewBox','0 0 24 24');svg.setAttribute('width','16');svg.setAttribute('height','16');svg.setAttribute('fill','none');svg.setAttribute('stroke','currentColor');svg.setAttribute('stroke-width','2');svg.setAttribute('stroke-linecap','round');svg.setAttribute('stroke-linejoin','round');paths.forEach(function(d){var path=document.createElementNS('http://www.w3.org/2000/svg','path');path.setAttribute('d',d);svg.appendChild(path)});button.appendChild(svg);return button}function renderMcpCard(server){var card=document.createElement('article');card.className='user-setting-catalog-card mcp-card';if(mcpState.bulk)card.classList.add('mcp-card-bulk');card.dataset.testid='mcp-server-card';card.dataset.id=server.id;var head=document.createElement('div');head.className='mcp-card-head';var name=document.createElement('h3');name.textContent=server.name;name.className='mcp-card-name';head.appendChild(name);var actions=document.createElement('div');actions.className='mcp-card-actions';if(mcpState.bulk){var box=document.createElement('input');box.type='checkbox';box.checked=mcpState.selected.has(server.id);box.setAttribute('aria-label','Select '+server.name);box.onchange=function(){if(box.checked)mcpState.selected.add(server.id);else mcpState.selected.delete(server.id);renderMcpServers()};actions.appendChild(box)}else{actions.appendChild(mcpIconButton(mcpIconsUpload,'Export '+server.name,function(){exportMcpServer(server.ID, server.name)},false));actions.appendChild(mcpIconButton(mcpIconsPen,'Edit '+server.name,function(){openMcpDialog(server.id)},false));actions.appendChild(mcpIconButton(mcpIconTrash,'Delete '+server.name,function(){confirmDeleteMcp(server)},true))}head.appendChild(actions);card.appendChild(head);var meta=document.createElement('div');meta.className='mcp-card-meta';meta.appendChild(document.createTextNode(toolCount(server)+' cached tools'));var date=document.createElement('p');date.textContent=formatMcpDate(server.update_date||server.update_time);meta.appendChild(date);card.appendChild(meta);return card}function openMcpDialog(id){document.getElementById('mcpSave').disabled=true;mcpState.fieldChanged=false;mcpState.editingId=id||null;mcpState.toolsTested=false;['mcpNameError','mcpUrlError','mcpTypeError'].forEach(function(error){document.getElementById(error).textContent=''});document.getElementById('mcpStatus').textContent='';document.getElementById('mcpDialogTitle').textContent=id?'Edit MCP':'Add MCP';document.getElementById('mcpName').value='';document.getElementById('mcpUrl').value='';document.getElementById('mcpServerType').value='sse';document.getElementById('mcpToken').value='';document.getElementById('mcpToolsCount').textContent='0 tools available';document.getElementById('mcpToolsList').replaceChildren(document.createTextNode('Test the connection to load tools.'));document.getElementById('mcpDialog').classList.add('show');if(id){mcpApi('/api/v1/mcp/servers/'+encodeURIComponent(id)).then(function(response){var server=response.data;if(!server)return;document.getElementById('mcpName').value=server.name||'';document.getElementById('mcpUrl').value=server.url||'';document.getElementById('mcpServerType').value=server.server_type||'sse';renderMcpTools(server.variables&&server.variables.tools);if(toolCount(server)>0){document.getElementById('mcpSave').disabled=false;document.getElementById('mcpToolsCount').textContent=toolCount(server)+' tools available'}})}}function mcpFieldChanged(){mcpState.fieldChanged=true}function closeMcpDialog(){mcpState.toolsTested=false;mcpState.fieldChanged=false;document.getElementById('mcpSave').disabled=true;document.getElementById('mcpDialog').classList.remove('show')}function mcpToolObjects(tools){if(!tools||typeof tools!=='object')return [];return Object.keys(tools).map(function(name){var tool=tools[name]||{};return {name:name,description:tool.description||''}})}function renderMcpTools(tools){var list=document.getElementById('mcpToolsList');list.replaceChildren();var rows=mcpToolObjects(tools);document.getElementById('mcpToolsCount').textContent=rows.length+' tools available';if(!rows.length){var empty=document.createElement('p');empty.className='user-setting-description';empty.textContent='No tools available.';list.appendChild(empty);return}rows.forEach(function(tool){var item=document.createElement('div');item.className='mcp-tool-item';var name=document.createElement('div');name.textContent=tool.name;item.appendChild(name);var description=document.createElement('div');description.className='mcp-tool-desc';description.textContent=tool.description;item.appendChild(description);list.appendChild(item)})}async function testMcpServer(){var url=document.getElementById('mcpUrl').value.trim();var serverType=document.getElementById('mcpServerType').value;var token=document.getElementById('mcpToken').value;var status=document.getElementById('mcpStatus');if(!url){document.getElementById('mcpUrlError').textContent='Please input URL';return}status.textContent='Testing…';try{var data=await mcpApi('/api/v1/mcp/servers/'+(mcpState.editingId||'test')+'/test',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({url:url,server_type:serverType,variables:{authorization_token:token}})});if(data.code!==0)throw new Error(data.message||('HTTP '+data.code));mcpState.toolsTested=true;document.getElementById('mcpSave').disabled=false;mcpState.fieldChanged=false;var tools={};(data.data||[]).forEach(function(tool){tools[tool.name]=tool});renderMcpTools(tools);status.textContent='Connection succeeded'}catch(error){status.textContent=error.message;document.getElementById('mcpToolsList').replaceChildren(document.createTextNode('Test the connection to load tools.'))}}function validateMcpForm(){var valid=true;['mcpNameError','mcpUrlError','mcpTypeError'].forEach(function(id){document.getElementById(id).textContent=''});var name=document.getElementById('mcpName').value.trim();var url=document.getElementById('mcpUrl').value.trim();if(!name){document.getElementById('mcpNameError').textContent='Please input name';valid=false}else if(!/^[a-zA-Z0-9_-]{1,64}$/.test(name)){document.getElementById('mcpNameError').textContent='It must be 1-64 characters long and can only contain letters, numbers, hyphens, and underscores.';valid=false}if(!url){document.getElementById('mcpUrlError').textContent='Please input URL';valid=false}return valid}async function saveMcpServer(){var saveGateStatus=document.getElementById('mcpStatus');if(!validateMcpForm()){return}var saveGate=document.getElementById('mcpSave');if(saveGate.disabled){if(!mcpState.toolsTested){saveGateStatus.textContent='Please test the connection first.'}else if(mcpState.fieldChanged){saveGateStatus.textContent='Connection changed. Please test the connection again before saving.'}return}var button=document.getElementById('mcpSave');button.disabled=true;var status=document.getElementById('mcpStatus');status.textContent='Saving…';var mcpSaveToken=document.getElementById('mcpToken').value.trim();var payload={name:document.getElementById('mcpName').value.trim(),url:document.getElementById('mcpUrl').value.trim(),server_type:document.getElementById('mcpServerType').value,variables:{authorization_token:mcpSaveToken},headers:{Authorization:'Bearer '+mcpSaveToken}};try{var data=await mcpApi('/api/v1/mcp/servers'+(mcpState.editingId?'/'+encodeURIComponent(mcpState.editingId):''),{method:mcpState.editingId?'PUT':'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(payload)});if(data.code!==0)throw new Error(data.message||('HTTP '+data.code));closeMcpDialog();loadMcpServers()}catch(error){status.textContent=error.message}finally{button.disabled=false}}function openMcpImport(){document.getElementById('mcpImportFile').value='';document.getElementById('mcpImportError').textContent='';document.getElementById('mcpImportResult').replaceChildren();document.getElementById('mcpImportDialog').classList.add('show')}function closeMcpImport(){document.getElementById('mcpImportDialog').classList.remove('show')}async function importMcpServers(){var file=document.getElementById('mcpImportFile').files[0];var error=document.getElementById('mcpImportError');if(!file){error.textContent='Please select a JSON file';return}var result=document.getElementById('mcpImportResult');result.replaceChildren();try{var text=await file.text();var parsed=JSON.parse(text);var data=await mcpApi('/api/v1/mcp/servers/import',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({mcpServers:parsed.mcpServers||parsed})});if(data.code!==0)throw new Error(data.message||('HTTP '+data.code));(data.data&&data.data.results||[]).forEach(function(item){var line=document.createElement('p');line.className='user-setting-description';line.textContent=(item.success?'✓ ':'✗ ')+item.server+(item.message?' — '+item.message:'');result.appendChild(line)});closeMcpImport();loadMcpServers()}catch(parseError){error.textContent='Invalid JSON file: '+parseError.message}}async function exportMcpServer(id, name){var data=await mcpApi('/api/v1/mcp/servers/'+encodeURIComponent(id)+'?mode=download');if(data.code!==0){alert('Error: '+(data.message||'unknown'));return}downloadMcpJson(data.data, (name||'mcp')+'.json')}function downloadMcpJson(payload, filename){var blob=new Blob([JSON.stringify(payload,null,2)],{type:'application/json'});var anchor=document.createElement('a');anchor.href=URL.createObjectURL(blob);anchor.download=filename||'mcp-servers.json';anchor.click();setTimeout(function(){URL.revokeObjectURL(anchor.href)},1000)}async function exportMcpSelected(){if(!mcpState.selected.size){alert('Please select at least one MCP server');return}var merged={mcpServers:{}};for(var id of mcpState.selected){var data=await mcpApi('/api/v1/mcp/servers/'+encodeURIComponent(id)+'?mode=download');if(data.code===0&&data.data&&data.data.mcpServers)Object.assign(merged.mcpServers,data.data.mcpServers)}downloadMcpJson(merged, 'mcp.json')}function confirmDeleteMcp(server){var dialog=document.getElementById('mcpDeleteDialog');dialog.dataset.id=server.id;dialog.dataset.bulk='false';document.getElementById('mcpDeleteText').textContent='Are you sure you want to delete "'+server.name+'"?';document.getElementById('mcpDeleteStatus').textContent='';dialog.classList.add('show')}function confirmDeleteMcpSelected(){if(!mcpState.selected.size){alert('Please select at least one MCP server');return}var dialog=document.getElementById('mcpDeleteDialog');dialog.dataset.bulk='true';document.getElementById('mcpDeleteText').textContent='Are you sure you want to delete the selected '+mcpState.selected.size+' MCP server(s)?';document.getElementById('mcpDeleteStatus').textContent='';dialog.classList.add('show')}function closeMcpDelete(){document.getElementById('mcpDeleteDialog').classList.remove('show')}async function deleteMcpServer(){var dialog=document.getElementById('mcpDeleteDialog');var button=document.getElementById('mcpDeleteOk');button.disabled=true;document.getElementById('mcpDeleteStatus').textContent='Deleting…';try{var ids=dialog.dataset.bulk==='true'?Array.from(mcpState.selected):[dialog.dataset.id];for(var id of ids){var data=await mcpApi('/api/v1/mcp/servers/'+encodeURIComponent(id),{method:'DELETE'});if(data.code!==0)throw new Error(data.message||('HTTP '+data.code))}mcpState.selected.clear();closeMcpDelete();loadMcpServers()}catch(error){document.getElementById('mcpDeleteStatus').textContent=error.message}finally{button.disabled=false}}loadMcpServers()</script>"#;
    Html(user_setting_layout(
        "MCP",
        body,
        UserSettingSection::Mcp,
        zh,
    ))
}

pub async fn user_setting_chat_channel_page(
    State(_state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let title = if zh { "聊天渠道" } else { "Chat channels" };
    let body_raw: &'static str = r#"<h1>Chat channels</h1><p class='user-setting-description'>Manage your chat channel bots and credentials</p><section data-testid='added-channels-section'><h2>Added channels</h2><div id='addedChannelList' class='added-channels' aria-live='polite'></div></section><section class='user-setting-section' data-testid='available-channels-section'><h2>Available channels</h2><p class='user-setting-description'>Select a chat channel to add</p><div id='availableChannelGrid' class='available-channel-grid'></div></section><div class='modal-overlay' id='chatChannelDialog' data-testid='chat-channel-dialog' onclick='if(event.target===this)closeChatChannelDialog()'><div class='modal' style='max-width:520px' role='dialog' aria-modal='true' aria-labelledby='chatChannelDialogTitle'><h2 id='chatChannelDialogTitle'>Add bot</h2><label class='provider-instance-field'>Name<input id='ccName' placeholder='My bot' autocomplete='off'><p id='ccNameError' class='provider-instance-error'></p></label><div id='ccFields' class='provider-extra-fields'></div><div id='ccRuntime' class='cc-runtime-box' hidden><div id='ccRuntimeBody'></div></div><p id='ccStatus' class='provider-instance-status' aria-live='polite'></p><div class='provider-instance-actions'><button class='btn btn-o' type='button' onclick='closeChatChannelDialog()'>Cancel</button><button class='btn btn-p' id='ccSave' type='button' onclick='saveChatChannel()'>Save</button></div></div></div><div class='modal-overlay' id='chatChannelDeleteDialog' data-testid='chat-channel-delete-dialog' onclick='if(event.target===this)closeChatChannelDelete()'><div class='modal' style='max-width:440px' role='alertdialog' aria-modal='true' aria-labelledby='chatChannelDeleteTitle'><h2 id='chatChannelDeleteTitle'>Delete chat channel</h2><p class='user-setting-description'>Are you sure you want to delete this chat channel bot? This action cannot be undone.</p><p id='ccDeleteStatus' class='provider-instance-status' aria-live='polite'></p><div class='provider-instance-actions'><button class='btn btn-o' type='button' onclick='closeChatChannelDelete()'>Cancel</button><button class='btn btn-d' id='ccDeleteOk' type='button' onclick='deleteChatChannel()'>Delete</button></div></div></div><script>var CHAT_CHANNEL_INFO={clickclack:{name:'ClickClack',desc:'Connect a ClickClack bot',fields:[{key:'token',label:'Bot Token',type:'password',required:true}]},discord:{name:'Discord',desc:'Connect a Discord bot',fields:[{key:'token',label:'Bot Token',type:'password',required:true},{key:'application_id',label:'Application ID',type:'text',required:false,placeholder:'1234567890'}]},dingtalk:{name:'DingTalk',desc:'Connect a DingTalk bot',fields:[{key:'client_id',label:'Client ID',type:'text',required:true,placeholder:'dingxxxxxxxxxxxx'},{key:'client_secret',label:'Client Secret',type:'password',required:true}]},feishu:{name:'Feishu / Lark',desc:'Connect a Feishu / Lark bot',fields:[{key:'app_id',label:'App ID',type:'text',required:true,placeholder:'cli_xxxxxxxxxxxxxxxx'},{key:'app_secret',label:'App Secret',type:'password',required:true},{key:'domain',label:'Domain',type:'select',required:true,defaultValue:'feishu',options:[{label:'Feishu (mainland)',value:'feishu'},{label:'Lark (international)',value:'lark'}]}]},googlechat:{name:'Google Chat',desc:'Connect a Google Chat bot',fields:[{key:'auth_mode',label:'Auth Mode',type:'select',required:true,defaultValue:'webhook_url',options:[{label:'Webhook URL',value:'webhook_url'},{label:'Service Account JSON',value:'service_account'},{label:'Service Account File',value:'service_account_file'}]},{key:'webhook_url',label:'Webhook URL',type:'text',required:false,placeholder:'https://chat.googleapis.com/v1/spaces/.../messages?key=...&token=...'},{key:'service_account',label:'Service Account JSON',type:'textarea',required:false,placeholder:'{ "type": "service_account", ... }'},{key:'service_account_file',label:'Service Account File Path',type:'text',required:false,placeholder:'/path/to/sa.json'}]},irc:{name:'IRC',desc:'Connect to an IRC server',fields:[{key:'host',label:'Host',type:'text',required:true,placeholder:'irc.libera.chat'},{key:'port',label:'Port',type:'number',required:true,placeholder:'6697'},{key:'tls',label:'TLS',type:'switch',required:false},{key:'nick',label:'Nick',type:'text',required:true,placeholder:'my-bot'},{key:'username',label:'Username',type:'text',required:false,placeholder:'Defaults to nick'},{key:'password',label:'Server Password',type:'password',required:false},{key:'nickserv.account',label:'NickServ Account',type:'text',required:false},{key:'nickserv.password',label:'NickServ Password',type:'password',required:false}]},line:{name:'LINE',desc:'Connect a LINE messaging bot',fields:[{key:'channel_secret',label:'Channel Secret',type:'password',required:true},{key:'channel_access_token',label:'Channel Access Token',type:'password',required:true}]},matrix:{name:'Matrix',desc:'Connect a Matrix bot',fields:[{key:'homeserver',label:'Homeserver',type:'text',required:true,placeholder:'https://matrix.example.org'},{key:'user_id',label:'User ID',type:'text',required:true,placeholder:'@bot:example.org'},{key:'access_token',label:'Access Token',type:'password',required:false},{key:'password',label:'Password',type:'password',required:false},{key:'device_id',label:'Device ID',type:'text',required:false,placeholder:'OPENCLAW_BOT'}]},mattermost:{name:'Mattermost',desc:'Connect a Mattermost bot',fields:[{key:'base_url',label:'Base URL',type:'text',required:true,placeholder:'https://mattermost.example.com'},{key:'bot_token',label:'Bot Token',type:'password',required:true}]},msteams:{name:'Microsoft Teams',desc:'Connect a Microsoft Teams bot',fields:[{key:'app_id',label:'App ID',type:'text',required:true,placeholder:'00000000-0000-0000-0000-000000000000'},{key:'app_password',label:'App Password',type:'password',required:true},{key:'tenant_id',label:'Tenant ID',type:'text',required:true,placeholder:'00000000-0000-0000-0000-000000000000'}]},nextcloud_talk:{name:'Nextcloud Talk',desc:'Connect a Nextcloud Talk bot',fields:[{key:'base_url',label:'Base URL',type:'text',required:true,placeholder:'https://nextcloud.example.com'},{key:'bot_secret',label:'Bot Secret',type:'password',required:true}]},nostr:{name:'Nostr',desc:'Connect a Nostr bot',fields:[{key:'private_key',label:'Private Key',type:'password',required:true,placeholder:'nsec1xxxx or hex'},{key:'relays',label:'Relays',type:'tag',required:false,placeholder:'wss://relay.damus.io'}]},qqbot:{name:'QQ Bot',desc:'Connect a QQ bot',fields:[{key:'app_id',label:'App ID',type:'text',required:true,placeholder:'102000000'},{key:'client_secret',label:'Client Secret',type:'password',required:true},{key:'base_url',label:'Base URL',type:'text',required:false,placeholder:'https://api.sgroup.qq.com'}]},slack:{name:'Slack',desc:'Connect a Slack bot',fields:[{key:'bot_token',label:'Bot Token',type:'password',required:true,placeholder:'xoxb-xxxx'},{key:'app_token',label:'App Token',type:'password',required:false,placeholder:'xapp-xxxx (socket mode)'},{key:'signing_secret',label:'Signing Secret',type:'password',required:false},{key:'user_token',label:'User Token',type:'password',required:false,placeholder:'xoxp-xxxx'}]},synology_chat:{name:'Synology Chat',desc:'Connect a Synology Chat bot',fields:[{key:'token',label:'Token',type:'password',required:true},{key:'incoming_url',label:'Incoming URL',type:'text',required:true,placeholder:'https://nas.example.com/webapi/entry.cgi?...'}]},telegram:{name:'Telegram',desc:'Connect a Telegram bot',fields:[{key:'token',label:'Bot Token',type:'password',required:true,placeholder:'From @BotFather'}]},tlon:{name:'Tlon (Urbit)',desc:'Connect a Tlon (Urbit) bot',fields:[{key:'ship',label:'Ship',type:'text',required:true,placeholder:'~sampel-palnet'},{key:'url',label:'URL',type:'text',required:true,placeholder:'https://sampel-palnet.tlon.network'},{key:'code',label:'Code',type:'password',required:true}]},twitch:{name:'Twitch',desc:'Connect a Twitch chat bot',fields:[{key:'username',label:'Username',type:'text',required:true,placeholder:'my-bot'},{key:'access_token',label:'Access Token',type:'password',required:true,placeholder:'oauth:xxxx'},{key:'client_id',label:'Client ID',type:'text',required:true},{key:'channel',label:'Channel',type:'text',required:true,placeholder:'target-channel-name'}]},wecom:{name:'WeCom',desc:'Connect a WeCom bot',fields:[{key:'connection_type',label:'Connection Type',type:'select',required:true,defaultValue:'webhook',options:[{label:'Webhook',value:'webhook'},{label:'WebSocket',value:'websocket'}]},{key:'bot_id',label:'Bot ID',type:'text',required:true,placeholder:'AIBOTID',showIf:'connection_type==websocket'},{key:'secret',label:'Secret',type:'password',required:true,placeholder:'App Secret / Long-connection Secret'},{key:'corp_id',label:'Corp ID',type:'text',required:true,placeholder:'ww1234567890abcdef',showIf:'connection_type!=websocket'},{key:'agent_id',label:'Agent ID',type:'number',required:true,placeholder:'1000001',showIf:'connection_type!=websocket'},{key:'token',label:'Token',type:'password',required:true,showIf:'connection_type!=websocket'},{key:'aes_key',label:'AES Key',type:'password',required:1,placeholder:'43 chars',showIf:'connection_type!=websocket'}]},whatsapp:{name:'WhatsApp',desc:'Connect a WhatsApp bot (QR pairing)',fields:[]},yuanbao:{name:'Yuanbao',desc:'Connect a Tencent Yuanbao bot',fields:[{key:'app_key',label:'App Key',type:'text',required:true},{key:'app_secret',label:'App Secret',type:'password',required:true},{key:'token',label:'Token',type:'password',required:false,placeholder:'Optional pre-signed token'}]},zalo:{name:'Zalo',desc:'Connect a Zalo bot',fields:[{key:'bot_token',label:'Bot Token',type:'password',required:true}]},zalouser:{name:'Zalo (Personal)',desc:'Connect a personal Zalo account',fields:[{key:'cookies',label:'Cookies (JSON)',type:'textarea',required:true,placeholder:'{ ... }'},{key:'imei',label:'IMEI',type:'text',required:true},{key:'user_agent',label:'User Agent',type:'text',required:true,placeholder:'Mozilla/5.0 ...'}]}};var chatChannelState={channels:[],activeId:null,editingId:null,deleteId:null};function ccApi(url,options){return fetch(url,options).then(function(response){return response.json()})}function ccLabel(id){return (CHAT_CHANNEL_INFO[id]&&CHAT_CHANNEL_INFO[id].name)||id}function ccDesc(id){var m=(typeof CC_DESC_ZH!=='undefined')?CC_DESC_ZH:null;return (m&&m[id])||(CHAT_CHANNEL_INFO[id]&&CHAT_CHANNEL_INFO[id].desc)}function ccFieldVisible(expr){var ct=document.getElementById('cc-connection_type');var ctVal=ct?ct.value:'webhook';if(expr==='connection_type==websocket')return ctVal==='websocket';if(expr==='connection_type!=websocket')return ctVal!=='websocket';return true}function ccApplyShowIf(){document.querySelectorAll('#ccFields label[data-show-if]').forEach(function(label){var visible=ccFieldVisible(label.dataset.showIf);label.style.display=visible?'':'none';var nxt=label.nextElementSibling;if(nxt&&nxt.className&&nxt.className.indexOf('provider-instance-error')>=0){nxt.style.display=visible?'':'none'}})}async function loadChatChannels(){var data=await ccApi('/api/v1/chat-channels');chatChannelState.channels=data.data||[];renderAddedChannels();renderAvailableChannels()}function renderAddedChannels(){var list=document.getElementById('addedChannelList');list.replaceChildren();if(!chatChannelState.channels.length){var empty=document.createElement('p');empty.className='user-setting-description';empty.textContent='No chat channels added yet. Select one below to connect.';list.appendChild(empty);return}var groups={};chatChannelState.channels.forEach(function(row){var group=groups[row.channel]||{channel:row.channel,rows:[]};group.rows.push(row);groups[row.channel]=group});Object.values(groups).forEach(function(group){var card=document.createElement('article');card.className='user-setting-catalog-card cc-added-card';card.dataset.testid='added-channel-card';card.dataset.channel=group.channel;var head=document.createElement('div');head.className='cc-card-head';var icon=document.createElement('span');icon.className='user-setting-catalog-icon';icon.textContent=ccLabel(group.channel).charAt(0).toUpperCase();head.appendChild(icon);var name=document.createElement('strong');name.textContent=ccLabel(group.channel);head.appendChild(name);card.appendChild(head);group.rows.forEach(function(row){var item=document.createElement('div');item.className='cc-account';item.dataset.testid='added-channel-account';var label=document.createElement('span');label.textContent=row.name;item.appendChild(label);var actions=document.createElement('div');actions.className='cc-account-actions';var edit=document.createElement('button');edit.type='button';edit.className='btn btn-o btn-sm';edit.textContent='Edit';edit.onclick=function(){openChatChannelDialog(group.channel,row)};actions.appendChild(edit);var del=document.createElement('button');del.type='button';del.className='btn btn-d btn-sm';del.textContent='Delete';del.onclick=function(){confirmDeleteChatChannel(row)};actions.appendChild(del);item.appendChild(actions);card.appendChild(item)});list.appendChild(card)})}function renderAvailableChannels(){var grid=document.getElementById('availableChannelGrid');grid.replaceChildren();Object.keys(CHAT_CHANNEL_INFO).forEach(function(id){var info=CHAT_CHANNEL_INFO[id];var card=document.createElement('article');card.className='cc-available-card';card.dataset.testid='available-channel-card';card.dataset.channel=id;card.tabIndex=0;card.onclick=function(){openChatChannelDialog(id)};card.onkeydown=function(event){if(event.key==='Enter'||event.key===' '){event.preventDefault();openChatChannelDialog(id)}};var icon=document.createElement('span');icon.className='cc-available-icon';icon.textContent=info.name.charAt(0).toUpperCase();var title=document.createElement('h3');title.textContent=info.name;var add=document.createElement('button');add.type='button';add.className='btn btn-o btn-sm cc-add-btn';add.textContent='+ Add';add.onclick=function(event){event.stopPropagation();openChatChannelDialog(id)};var desc=document.createElement('p');desc.className='cc-available-desc';desc.textContent=ccDesc(id);card.append(icon,title,add,desc);grid.appendChild(card)})}function openChatChannelDialog(channelId,record){chatChannelState.activeId=channelId;chatChannelState.editingId=record?record.id:null;var info=CHAT_CHANNEL_INFO[channelId];document.getElementById('chatChannelDialogTitle').textContent=(record?'Edit ':'Add ')+info.name+' bot';document.getElementById('ccName').value=record?record.name:'';document.getElementById('ccNameError').textContent='';document.getElementById('ccStatus').textContent='';var fields=document.getElementById('ccFields');fields.replaceChildren();info.fields.forEach(function(field){var label=document.createElement('label');label.className='provider-instance-field';var span=document.createElement('span');span.textContent=field.label;label.appendChild(span);var existing=(record&&record.config&&record.config.credential)?field.key.split('.').reduce(function(o,k){return (o===undefined||o===null)?undefined:o[k]},record.config.credential):'';var control;if(field.type==='select'){control=document.createElement('select');control.id='cc-'+field.key;control.className='cc-input';(field.options||[]).forEach(function(o){var opt=document.createElement('option');opt.value=o.value;opt.textContent=o.label;var def=field.defaultValue||'';if(existing!==''&&existing!==undefined&&existing!==null?existing===o.value:def===o.value)opt.selected=true;control.appendChild(opt)})}else if(field.type==='switch'){control=document.createElement('input');control.type='checkbox';control.id='cc-'+field.key;control.className='cc-switch';if(existing===true||existing==='true'||existing===1||existing==='1')control.checked=true}else if(field.type==='textarea'){control=document.createElement('textarea');control.id='cc-'+field.key;control.placeholder=field.placeholder||'';control.value=existing}else if(field.type==='tag'){control=document.createElement('div');control.className='cc-tag-box';control.id='cc-'+field.key;var chips=document.createElement('div');chips.className='cc-tag-chips';var draft=document.createElement('input');draft.type='text';draft.className='cc-tag-draft';draft.placeholder=field.placeholder||'';draft.autocomplete='off';var addChip=function(text){var t=text.trim();if(!t)return;var vals=(control.dataset.values||'').split(',').filter(function(s){return s});if(vals.indexOf(t)>=0)return;vals.push(t);control.dataset.values=vals.join(',');renderChips()};var renderChips=function(){chips.replaceChildren();(control.dataset.values||'').split(',').filter(function(s){return s}).forEach(function(v){var chip=document.createElement('span');chip.className='cc-tag-chip';var txt=document.createElement('span');txt.textContent=v;chip.appendChild(txt);var x=document.createElement('button');x.type='button';x.className='cc-tag-remove';x.setAttribute('aria-label','Remove '+v);x.textContent='×';x.onclick=function(){var vals=(control.dataset.values||'').split(',').filter(function(s){return s&&s!==v});control.dataset.values=vals.join(',');renderChips()};chip.appendChild(x);chips.appendChild(chip)})};(Array.isArray(existing)?existing:[]).forEach(addChip);draft.onkeydown=function(e){if(e.key==='Enter'||e.key===','){e.preventDefault();addChip(draft.value);draft.value=''}else if(e.key==='Backspace'&&!draft.value){var vals=(control.dataset.values||'').split(',').filter(function(s){return s});vals.pop();control.dataset.values=vals.join(',');renderChips()}};control.appendChild(chips);control.appendChild(draft)}else{control=document.createElement('input');control.id='cc-'+field.key;control.type=field.type==='password'?'password':(field.type==='number'?'number':'text');control.placeholder=field.placeholder||'';control.autocomplete='off';control.value=(existing!==undefined&&existing!==null)?existing:(field.defaultValue||'')}if(field.showIf){label.dataset.showIf=field.showIf}label.appendChild(control);fields.appendChild(label);var error=document.createElement('p');error.id='cc-'+field.key+'-error';error.className='provider-instance-error';fields.appendChild(error)});ccApplyShowIf();var ctSel=document.getElementById('cc-connection_type');if(ctSel){ctSel.onchange=ccApplyShowIf}ccResetRuntime();document.getElementById('chatChannelDialog').classList.add('show')}function closeChatChannelDialog(){document.getElementById('chatChannelDialog').classList.remove('show');ccStopRuntimePoll()}function setPath(obj,path,value){var parts=path.split('.');var cur=obj;for(var i=0;i<parts.length-1;i++){if(!cur[parts[i]]||typeof cur[parts[i]]!=='object')cur[parts[i]]={};cur=cur[parts[i]]}cur[parts[parts.length-1]]=value}
async function saveChatChannel(){var info=CHAT_CHANNEL_INFO[chatChannelState.activeId];var name=document.getElementById('ccName').value.trim();var valid=true;document.getElementById('ccNameError').textContent='';if(!name){document.getElementById('ccNameError').textContent='Name is required';valid=false}var credential={};info.fields.forEach(function(field){var control=document.getElementById('cc-'+field.key);var error=document.getElementById('cc-'+field.key+'-error');error.textContent='';var label=control.closest('label');if(label&&label.style.display==='none'){return}var value;if(field.type==='switch'){value=control.checked;if(value)setPath(credential,field.key,true)}else if(field.type==='tag'){var raw=(control.dataset.values||'').trim();var arr=raw?raw.split(',').map(function(s){return s.trim()}).filter(function(s){return s}):[];if(field.required&&!arr.length){error.textContent=field.label+' is required';valid=false}if(arr.length)setPath(credential,field.key,arr)}else{value=control.value.trim();if(field.required&&!value){error.textContent=field.label+' is required';valid=false}if(value)setPath(credential,field.key,value)}});if(!valid)return;var button=document.getElementById('ccSave');button.disabled=true;document.getElementById('ccStatus').textContent='Saving…';var payload={name:name,channel:chatChannelState.activeId,config:{credential:credential}};try{var data=await ccApi('/api/v1/chat-channels'+(chatChannelState.editingId?'/'+encodeURIComponent(chatChannelState.editingId):''),{method:chatChannelState.editingId?'PATCH':'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(payload)});if(data.code!==0)throw new Error(data.message||('HTTP '+data.code));chatChannelState.editingId=(data.data&&data.data.id)||chatChannelState.editingId||'';if(chatChannelState.activeId==='whatsapp' && chatChannelState.editingId){ccStartRuntimePoll(chatChannelState.editingId);loadChatChannels()}else{closeChatChannelDialog();loadChatChannels()}}catch(error){document.getElementById('ccStatus').textContent=error.message}finally{button.disabled=false}}function confirmDeleteChatChannel(row){chatChannelState.deleteId=row.id;document.getElementById('ccDeleteStatus').textContent='';document.getElementById('chatChannelDeleteDialog').classList.add('show')}function closeChatChannelDelete(){document.getElementById('chatChannelDeleteDialog').classList.remove('show')}async function deleteChatChannel(){var button=document.getElementById('ccDeleteOk');button.disabled=true;document.getElementById('ccDeleteStatus').textContent='Deleting…';try{var data=await ccApi('/api/v1/chat-channels/'+encodeURIComponent(chatChannelState.deleteId),{method:'DELETE'});if(data.code!==0)throw new Error(data.message||('HTTP '+data.code));closeChatChannelDelete();loadChatChannels()}catch(error){document.getElementById('ccDeleteStatus').textContent=error.message}finally{button.disabled=false}}function ccRuntimeStatusText(st){var n=(st||'').toLowerCase();if(n==='connected')return 'Connected';if(n==='connecting')return 'Connecting...';if(n==='reconnecting')return 'Reconnecting...';if(n==='qr')return 'Scan the QR code below';if(n==='waiting')return 'Waiting for the channel to start';if(n==='error')return 'Runtime error';if(n==='disconnected')return 'Disconnected';if(n==='stopped')return 'Stopped';return st||''}
function ccRuntimeStatusClass(st){var n=(st||'').toLowerCase();if(n==='connected')return 'cc-pill-ok';if(n==='connecting'||n==='reconnecting'||n==='qr'||n==='waiting')return 'cc-pill-warn';if(n==='error'||n==='disconnected'||n==='stopped')return 'cc-pill-err';return 'cc-pill'}
function ccStopRuntimePoll(){if(chatChannelState.runtimeTimer){clearInterval(chatChannelState.runtimeTimer);chatChannelState.runtimeTimer=null}}
function ccResetRuntime(){ccStopRuntimePoll();chatChannelState.whatsappId='';var box=document.getElementById('ccRuntime');if(box)box.hidden=true;var body=document.getElementById('ccRuntimeBody');if(body)body.replaceChildren()}
function ccRenderRuntime(status,error,qr){var body=document.getElementById('ccRuntimeBody');body.replaceChildren();var pill=document.createElement('span');pill.className='cc-runtime-pill '+ccRuntimeStatusClass(status);pill.textContent=ccRuntimeStatusText(status);body.appendChild(pill);if(error){var err=document.createElement('p');err.className='cc-runtime-error';err.textContent=error;body.appendChild(err)}if(qr&&status!=='connected'){var img=document.createElement('img');img.src=qr;img.alt='WhatsApp QR';img.className='cc-runtime-qr';body.appendChild(img)}else if(status==='connected'){var ok=document.createElement('p');ok.className='cc-runtime-ok';ok.textContent='Channel is connected.';body.appendChild(ok)}else if(!status){var hint=document.createElement('p');hint.className='user-setting-description';hint.textContent='QR will appear after the channel starts.';body.appendChild(hint)}}
async function ccRuntimePoll(){var id=chatChannelState.whatsappId;if(!id)return;try{var data=await ccApi('/api/v1/chat-channels/'+encodeURIComponent(id)+'/runtime');var snap=(data.code===0&&data.data)||{};ccRenderRuntime(snap.status||'',snap.last_error||'',snap.qr_data_url||'')}catch(error){ccRenderRuntime('',error.message||'Failed to load QR.','')}}
function ccStartRuntimePoll(id){ccStopRuntimePoll();chatChannelState.whatsappId=id;ccRenderRuntime('','','');chatChannelState.runtimeTimer=setInterval(ccRuntimePoll,1000);ccRuntimePoll()}
loadChatChannels();</script>"#;
    // Upstream `web/src/locales/zh.ts::setting.chatChannelDesc` fixes the
    // Chinese names for all 23 chat channels; RayRAG renders them verbatim
    // under lang=zh-CN and keeps the English page free of the zh-only map
    // (CC_DESC_ZH is injected exactly when zh is requested).
    const CC_DESC_ZH_PAIRS: &str = "clickclack:'连接 ClickClack 机器人',discord:'连接 Discord 机器人',dingtalk:'连接钉钉机器人',feishu:'连接飞书 / Lark 机器人',googlechat:'连接 Google Chat 机器人',irc:'连接 IRC 服务器',line:'连接 LINE 消息机器人',matrix:'连接 Matrix 机器人',mattermost:'连接 Mattermost 机器人',msteams:'连接 Microsoft Teams 机器人',nextcloud_talk:'连接 Nextcloud Talk 机器人',nostr:'连接 Nostr 机器人',qqbot:'连接 QQ 机器人',slack:'连接 Slack 机器人',synology_chat:'连接群晖 Chat 机器人',telegram:'连接 Telegram 机器人',tlon:'连接 Tlon (Urbit) 机器人',twitch:'连接 Twitch 聊天机器人',wecom:'连接企业微信机器人',whatsapp:'连接 WhatsApp 机器人（扫码配对）',yuanbao:'连接腾讯元宝机器人',zalo:'连接 Zalo 机器人',zalouser:'连接个人 Zalo 账号'";
    let body: std::borrow::Cow<'static, str> = if zh {
        std::borrow::Cow::Owned(body_raw.replacen(
            "loadChatChannels();</script>",
            &format!(
                "var CC_DESC_ZH={{{}}};loadChatChannels();</script>",
                CC_DESC_ZH_PAIRS
            ),
            1,
        ))
    } else {
        std::borrow::Cow::Borrowed(body_raw)
    };
    Html(user_setting_layout(
        title,
        &body,
        UserSettingSection::ChatChannel,
        zh,
    ))
}

// ── Pages ───────────────────────────────────────────────────────

/// RAGFlow `/` (`pages/home/index.tsx`): welcome heading, the Dataset section
/// (up to six cards + "See all", or the dashed empty card) and the Applications
/// section with the Chat/Search/Agent/Memory `Segmented` control. The lists are
/// filled from the authenticated APIs exactly like the upstream hooks.
pub async fn index(req: axum::http::Request<axum::body::Body>) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let (
        title,
        welcome,
        dataset_label,
        chat_label,
        search_label,
        agent_label,
        memory_label,
        dataset_empty,
        chat_empty,
        search_empty,
        agent_empty,
        memory_empty,
    ) = if zh {
        (
            "首页",
            "欢迎来到",
            "知识库",
            "聊天",
            "搜索",
            "智能体",
            "记忆",
            "尚未创建知识库",
            "尚未创建聊天应用",
            "尚未创建搜索应用",
            "尚未创建智能体",
            "尚未创建记忆",
        )
    } else {
        (
            "Home",
            "Welcome to",
            "Dataset",
            "Chat",
            "Search",
            "Agent",
            "Memory",
            "No dataset created yet",
            "No chat app created yet",
            "No search app created yet",
            "No agent app created yet",
            "No memory created yet",
        )
    };
    let dataset_icon = "<svg width='24' height='24' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.6' stroke-linecap='round' stroke-linejoin='round' aria-hidden='true'><ellipse cx='12' cy='5' rx='8' ry='3'/><path d='M4 5v14c0 1.7 3.6 3 8 3s8-1.3 8-3V5'/><path d='M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3'/></svg>";
    let mut b = String::with_capacity(4096);
    b.push_str("<article class='home' data-testid='home-page'><header class='home-banner'><h1 class='home-welcome'><span class='home-welcome-text'>");
    b.push_str(welcome);
    b.push_str(" </span><span class='home-brand'>RayRAG</span></h1></header>");
    b.push_str("<section class='home-section'><header class='home-section-head'><h2 class='home-section-title'><span class='home-section-icon'>");
    b.push_str(dataset_icon);
    b.push_str("</span>");
    b.push_str(dataset_label);
    b.push_str("</h2></header><div class='home-card-row' id='homeDatasets' data-testid='home-datasets'></div></section>");
    b.push_str("<section class='home-section home-apps'><header class='home-section-head'><h2 class='home-section-title'><span class='home-section-icon' id='homeAppsIcon'>");
    b.push_str(chat_icon());
    b.push_str("</span><span id='homeAppsLabel'>");
    b.push_str(chat_label);
    b.push_str("</span></h2><div class='segmented' role='tablist' aria-label='Applications'>");
    for (key, label, icon, selected) in [
        ("chats", chat_label, chat_icon(), true),
        ("searches", search_label, search_icon(), false),
        ("agents", agent_label, agent_icon(), false),
        ("memories", memory_label, memory_icon(), false),
    ] {
        b.push_str("<button type='button' role='tab' class='segmented-item");
        if selected {
            b.push_str(" active");
        }
        b.push_str("' data-app='");
        b.push_str(key);
        b.push_str("' data-icon=\"");
        b.push_str(&icon.replace('\'', "&#39;"));
        b.push_str("\" aria-selected='");
        b.push_str(if selected { "true" } else { "false" });
        b.push_str("' data-testid='home-tab-");
        b.push_str(key);
        b.push_str("' onclick='homeSelectApp(\"");
        b.push_str(key);
        b.push_str("\")'>");
        b.push_str(label);
        b.push_str("</button>");
    }
    b.push_str("</div></header><div class='home-card-row' id='homeApps' data-testid='home-apps'></div></section></article>");
    b.push_str("<script>const HOME_LABELS={seeAll:");
    b.push_str(&json_script_string(if zh {
        "查看全部"
    } else {
        "See all"
    }));
    b.push_str(",empty:{datasets:");
    b.push_str(&json_script_string(dataset_empty));
    b.push_str(",chats:");
    b.push_str(&json_script_string(chat_empty));
    b.push_str(",searches:");
    b.push_str(&json_script_string(search_empty));
    b.push_str(",agents:");
    b.push_str(&json_script_string(agent_empty));
    b.push_str(",memories:");
    b.push_str(&json_script_string(memory_empty));
    b.push_str("},labels:{datasets:");
    b.push_str(&json_script_string(dataset_label));
    b.push_str(",chats:");
    b.push_str(&json_script_string(chat_label));
    b.push_str(",searches:");
    b.push_str(&json_script_string(search_label));
    b.push_str(",agents:");
    b.push_str(&json_script_string(agent_label));
    b.push_str(",memories:");
    b.push_str(&json_script_string(memory_label));
    b.push_str("},files:");
    b.push_str(&json_script_string(if zh { "个文件" } else { "files" }));
    b.push_str(",rename:");
    b.push_str(&json_script_string(if zh { "重命名" } else { "Rename" }));
    b.push_str(",remove:");
    b.push_str(&json_script_string(if zh { "删除" } else { "Delete" }));
    b.push_str(",removeConfirm:");
    b.push_str(&json_script_string(if zh {
        "确认删除该知识库？"
    } else {
        "Delete this dataset?"
    }));
    b.push_str("};var homeApp='chats';const HOME_ENDPOINT={chats:'/api/v1/chats',searches:'/api/v1/searchapps',agents:'/api/v1/agents',memories:'/api/v1/memories'};function homeEscape(value){return String(value==null?'':value).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/\"/g,'&quot;')}function homeInitial(name){var text=String(name||'R').trim();return (text.charAt(0)||'R').toUpperCase()}function homeSeeAll(href){return \"<article class='home-card home-see-all' tabindex='0' role='button' onclick=\\\"location.href='\"+href+\"'\\\"><span>\"+HOME_LABELS.seeAll+\"</span><svg width='16' height='16' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='2' stroke-linecap='round' stroke-linejoin='round' aria-hidden='true'><path d='m9 18 6-6-6-6'/></svg></article>\"}function homeEmpty(kind,href){return \"<article class='home-empty-card' tabindex='0' role='button' data-testid='home-empty-\"+kind+\"' onclick=\\\"location.href='\"+href+\"'\\\"><span class='home-empty-plus' aria-hidden='true'>+</span><span>\"+HOME_LABELS.empty[kind]+\"</span></article>\"}function homeAppCard(kind,item){var name=homeEscape(item.name||item.title);var stamp=item.update_time||item.update_date||item.created_at||'';var when=stamp?new Date(isNaN(Number(stamp))?stamp:Number(stamp)).toLocaleString():'';var href=kind==='chats'?'/chat/'+item.id:(kind==='searches'?'/search?sapp='+item.id:(kind==='agents'?'/agent/'+item.id:'/memory/'+item.id));return \"<article class='home-card' tabindex='0' role='button' data-testid='home-app-card' onclick=\\\"location.href='\"+href+\"'\\\"><span class='home-card-avatar' aria-hidden='true'>\"+homeInitial(name)+\"</span><span class='home-card-body'><span class='home-card-title'>\"+name+\"</span><span class='home-card-meta'>\"+homeEscape(when)+\"</span></span></article>\"}function homeDatasetCard(item){var name=homeEscape(item.name);var files=(item.doc_count||0)+' '+HOME_LABELS.files;return \"<article class='home-card' tabindex='0' role='button' data-testid='home-dataset-card' onclick=\\\"location.href='/dataset/\"+item.id+\"'\\\"><span class='home-card-avatar' aria-hidden='true'>\"+homeInitial(name)+\"</span><span class='home-card-body'><span class='home-card-title'>\"+name+\"</span><span class='home-card-meta'>\"+homeEscape(files)+\"</span></span><span class='home-card-more' role='button' tabindex='0' aria-label='More' data-testid='home-dataset-more' onclick=\\\"event.stopPropagation();homeDatasetMenu(event,'\"+item.id+\"','\"+name+\"')\\\">⋮</span></article>\"}function homeDatasetMenu(event,id,name){event.stopPropagation();var existing=document.getElementById('homeDatasetMenu');if(existing)existing.remove();var menu=document.createElement('div');menu.id='homeDatasetMenu';menu.className='home-card-menu';menu.innerHTML=\"<button type='button' onclick='homeRenameDataset(\\\"\"+id+\"\\\",\\\"\"+name+\"\\\")'>\"+HOME_LABELS.rename+\"</button><button type='button' class='danger' onclick='homeDeleteDataset(\\\"\"+id+\"\\\")'>\"+HOME_LABELS.remove+\"</button>\";document.body.appendChild(menu);var rect=event.currentTarget.getBoundingClientRect();menu.style.top=(rect.bottom+window.scrollY+4)+'px';menu.style.left=(rect.right+window.scrollX-140)+'px';setTimeout(function(){document.addEventListener('click',homeCloseMenu,{once:true})},0)}function homeCloseMenu(){var menu=document.getElementById('homeDatasetMenu');if(menu)menu.remove()}function homeRenameDataset(id,name){homeCloseMenu();var next=prompt(HOME_LABELS.rename,name);if(!next||next===name)return;fetch('/api/v1/datasets/'+id,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:next})}).then(function(response){return response.json()}).then(function(data){if(data.code!==0)throw new Error(data.message||'Rename failed');loadHomeDatasets()}).catch(function(error){alert(error.message)})}function homeDeleteDataset(id){homeCloseMenu();if(!confirm(HOME_LABELS.removeConfirm))return;fetch('/api/v1/datasets/'+id,{method:'DELETE'}).then(function(response){return response.json()}).then(function(data){if(data.code!==0)throw new Error(data.message||'Delete failed');loadHomeDatasets()}).catch(function(error){alert(error.message)})}function loadHomeDatasets(){fetch('/api/v1/datasets').then(function(response){return response.json()}).then(function(data){var list=(data.data&&(data.data.items||data.data))||[];if(!Array.isArray(list))list=[];var host=document.getElementById('homeDatasets');if(!host)return;if(!list.length){host.innerHTML=homeEmpty('datasets','/datasets?isCreate=true');return}var html=list.slice(0,6).map(homeDatasetCard).join('');html+=homeSeeAll('/datasets');host.innerHTML=html}).catch(function(){})}function homeSelectApp(kind){homeApp=kind;document.querySelectorAll('.segmented-item').forEach(function(button){var on=button.getAttribute('data-app')===kind;button.classList.toggle('active',on);button.setAttribute('aria-selected',on?'true':'false')});var label=document.getElementById('homeAppsLabel');if(label)label.textContent=HOME_LABELS.labels[kind]||kind;var icon=document.getElementById('homeAppsIcon');var source=document.querySelector('.segmented-item[data-app=\\\"'+kind+'\\\"]');if(icon&&source)icon.innerHTML=source.getAttribute('data-icon')||'';loadHomeApps()}function loadHomeApps(){var kind=homeApp;var endpoint=HOME_ENDPOINT[kind];var host=document.getElementById('homeApps');if(!host)return;host.innerHTML='';fetch(endpoint).then(function(response){return response.json()}).then(function(data){var payload=data.data||{};var list=Array.isArray(payload)?payload:(payload.items||payload.canvas||payload.memory_list||payload.chats||payload.list||[]);if(!Array.isArray(list))list=[];if(!list.length){host.innerHTML=homeEmpty(kind,kind==='chats'?'/chats?isCreate=true':(kind==='searches'?'/searches?isCreate=true':(kind==='agents'?'/agents?isCreate=true':'/memories?isCreate=true')));return}var html=list.slice(0,6).map(function(item){return homeAppCard(kind,item)}).join('');html+=homeSeeAll('/'+kind);host.innerHTML=html}).catch(function(){})}loadHomeDatasets();loadHomeApps()</script>");
    Html(layout(title, &b, zh))
}

fn chat_icon() -> &'static str {
    "<svg width='24' height='24' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.6' stroke-linecap='round' stroke-linejoin='round' aria-hidden='true'><path d='M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2Z'/><path d='M8 9h8M8 13h5'/></svg>"
}

fn search_icon() -> &'static str {
    "<svg width='24' height='24' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.6' stroke-linecap='round' stroke-linejoin='round' aria-hidden='true'><circle cx='11' cy='11' r='7'/><path d='m20 20-3.5-3.5'/></svg>"
}

fn agent_icon() -> &'static str {
    "<svg width='24' height='24' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.6' stroke-linecap='round' stroke-linejoin='round' aria-hidden='true'><rect x='5' y='5' width='14' height='14' rx='2'/><path d='M9 2v3M15 2v3M9 19v3M15 19v3M2 9h3M2 15h3M19 9h3M19 15h3'/></svg>"
}

fn memory_icon() -> &'static str {
    "<svg width='24' height='24' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.6' stroke-linecap='round' stroke-linejoin='round' aria-hidden='true'><path d='M12 5a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V8a3 3 0 0 0-3-3Z'/><path d='M9 8H6a3 3 0 0 0 0 6h3M15 8h3a3 3 0 0 1 0 6h-3'/></svg>"
}

/// RAGFlow `/logout`: clears the client-stored token (localStorage + cookie)
/// and bounces to the login page. RayRAG auth is stateless bearer-token based,
/// so the server only serves a tiny page that performs the local cleanup.
pub async fn logout_page() -> Html<String> {
    Html(
        r#"<!doctype html><meta charset='utf-8'><title>Signing out</title>
<script>
try{localStorage.removeItem('rayrag_token')}catch(e){}
document.cookie='rayrag_token=; path=/; max-age=0';
location.replace('/login');
</script>"#
            .to_string(),
    )
}

pub async fn dashboard(
    State(state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let kbs = state.kbs.list();
    let chunks = state.engine.read().unwrap().len();
    let docs_count: usize = state.docs.list_all().len();
    let convs = state.conversations.list();
    let conv_count = convs.len();
    let emb = if state.embedder.is_some() {
        "✅"
    } else {
        "⚠️"
    };
    let llm = if state.llm.is_some() { "✅" } else { "⚠️" };
    let mut kb_cards = String::new();
    for kb in kbs.iter().take(6) {
        let initial = kb
            .name
            .chars()
            .next()
            .unwrap_or('K')
            .to_uppercase()
            .collect::<String>();
        kb_cards.push_str(&format!("<div class='kb-card' onclick=\"location.href='/dataset/{}'\"><div class='av-lg'>{}</div><h3>{}</h3><p>{} files · {} chunks</p></div>", kb.id, initial, kb.name, kb.doc_count, kb.chunk_count));
    }
    if kbs.is_empty() {
        kb_cards="<p style='color:var(--tm);text-align:center;padding:40px'>No knowledge bases. Create one to get started.</p>".into();
    }
    let mut b = String::new();
    b.push_str("<h1>");
    b.push_str(t("Dashboard", zh));
    b.push_str("</h1>");
    b.push_str("<div class='st'><div class='sc'><span class='lbl'>KBs</span><span class='val'>");
    b.push_str(&kbs.len().to_string());
    b.push_str("</span></div><div class='sc'><span class='lbl'>Chunks</span><span class='val'>");
    b.push_str(&chunks.to_string());
    b.push_str("</span></div><div class='sc'><span class='lbl'>Docs</span><span class='val'>");
    b.push_str(&docs_count.to_string());
    b.push_str("</span></div><div class='sc'><span class='lbl'>Chats</span><span class='val'>");
    b.push_str(&conv_count.to_string());
    b.push_str("</span></div><div class='sc'><span class='lbl'>Embedder</span><span class='val' style='font-size:20px'>");
    b.push_str(emb);
    b.push_str("</span></div><div class='sc'><span class='lbl'>LLM</span><span class='val' style='font-size:20px'>");
    b.push_str(llm);
    b.push_str("</span></div></div>");
    b.push_str("<div class='card'><h2>");
    b.push_str(t("Knowledge Bases", zh));
    b.push_str(" <a href='/kbs' style='font-size:13px;color:var(--p);text-decoration:none;float:right'>See all →</a></h2><div class='kb-grid'>");
    b.push_str(&kb_cards);
    b.push_str("</div></div>");
    b.push_str("<div class='card'><h2>Quick Actions</h2><a class='btn btn-p' href='/kbs'>+ ");
    b.push_str(t("New KB", zh));
    b.push_str("</a> <a class='btn btn-o' href='/search'>");
    b.push_str(t("Search Docs", zh));
    b.push_str("</a> <a class='btn btn-o' href='/chat'>");
    b.push_str(t("New Chat", zh));
    b.push_str("</a></div>");
    b.push_str("<div class='card'><h2>Recent Chats</h2>");
    if convs.is_empty() {
        b.push_str("<p style='color:var(--tm)'>No conversations yet.</p>");
    } else {
        for c in convs.iter().rev().take(5) {
            let preview: String = c
                .messages
                .last()
                .map(|m| m.content.chars().take(60).collect())
                .unwrap_or_default();
            b.push_str(&format!(
                "<div style='padding:8px 0;border-bottom:1px solid var(--bd)'><a href='/chat/{}' style='color:var(--p);text-decoration:none;font-weight:600'>{}</a><p style='margin:2px 0 0;color:var(--tm);font-size:12px'>{}</p></div>",
                c.id,
                c.name.chars().take(40).collect::<String>(),
                preview
            ));
        }
    }
    b.push_str("</div>");
    Html(layout(t("Dashboard", zh), &b, zh))
}

pub async fn kbs_page(
    State(state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let kbs = state.kbs.list();
    let mut cards = String::new();
    for kb in &kbs {
        let initial = kb
            .name
            .chars()
            .next()
            .unwrap_or('K')
            .to_uppercase()
            .collect::<String>();
        let name_js = kb
            .name
            .replace('&', "&amp;")
            .replace('\'', "&#39;")
            .replace('"', "&quot;");
        let desc_js = kb
            .description
            .replace('&', "&amp;")
            .replace('\'', "&#39;")
            .replace('"', "&quot;");
        let menu = format!(
            "<div class='kb-menu' id='km-{menu_id}' style='display:none;position:absolute;top:34px;right:8px;background:var(--bg);border:1px solid var(--bd);border-radius:8px;box-shadow:0 4px 16px rgba(0,0,0,.15);z-index:50;min-width:150px;padding:4px'><button class='btn btn-o btn-sm' style='text-align:left;justify-content:flex-start;width:100%' data-id='{id}' data-name='{name_js}' data-desc='{desc_js}' onclick='event.stopPropagation();kbRenameOpen(this)'>✏️ {rename}</button><button class='btn btn-o btn-sm' style='text-align:left;justify-content:flex-start;width:100%' data-id='{id}' onclick='event.stopPropagation();exportKb(this)'>⬇️ {export}</button><button class='btn btn-d btn-sm' style='text-align:left;justify-content:flex-start;width:100%' data-id='{id}' data-name='{name_js}' onclick='event.stopPropagation();delKb(this)'>🗑 {delete}</button></div>",
            menu_id = format!("km-{}", kb.id),
            id = kb.id,
            name_js = name_js,
            desc_js = desc_js,
            rename = t("Rename", zh),
            export = t("Export", zh),
            delete = t("Delete", zh),
        );
        cards.push_str(&format!(
            "<div class='kb-card' style='position:relative' data-name='{nm}' data-created='{created}' onclick=\"location.href='/kbs/{id}'\"><div class='more' data-id='{id}' onclick='event.stopPropagation();toggleKbMenu(this)'>⋯</div>{menu}<div class='av-lg'>{initial}</div><h3>{name}</h3><p>{docs} files · {chunks} chunks</p><p style='margin-top:4px'>{desc}</p></div>",
            nm = kb.name.replace('\'', "&#39;"),
            created = kb.created_at,
            id = kb.id,
            menu = menu,
            initial = initial,
            name = kb.name,
            docs = kb.doc_count,
            chunks = kb.chunk_count,
            desc = if kb.description.is_empty() { "-".to_string() } else { kb.description.clone() },
        ));
    }
    if kbs.is_empty() {
        cards="<p style='color:var(--tm);text-align:center;padding:40px;grid-column:1/-1'>No knowledge bases yet. Click \"New KB\" to create one.</p>".into();
    }
    let mut b = String::new();
    b.push_str(
        "<div style='display:flex;align-items:center;margin-bottom:24px;gap:10px;flex-wrap:wrap'><h1 style='margin:0'>",
    );
    b.push_str(t("Knowledge Bases", zh));
    b.push_str("</h1><input id='kbSearch' placeholder='🔍 ");
    b.push_str(t("Search", zh));
    b.push_str("' style='max-width:220px;margin-left:auto' oninput='kbFilter()'><select id='kbSort' onchange='kbSort()' style='max-width:150px'><option value='name'>");
    b.push_str(t("Sort: Name", zh));
    b.push_str("</option><option value='created'>");
    b.push_str(t("Sort: Created", zh));
    b.push_str("</option></select><button class='btn btn-p' onclick='showCreate()'>+ ");
    b.push_str(t("New KB", zh));
    b.push_str("</button></div>");
    b.push_str("<div class='kb-grid' id='kbGrid'>");
    b.push_str(&cards);
    b.push_str("</div>");
    // Create modal
    b.push_str("<div class='modal-overlay' id='createModal'><div class='modal'><h2>");
    b.push_str(t("New KB", zh));
    b.push_str("</h2><input id='kbName' placeholder='");
    b.push_str(t("Name", zh));
    b.push_str("'><input id='kbDesc' placeholder='");
    b.push_str(t("Description", zh));
    b.push_str(
        "' style='margin-top:8px'><select id='kbPerm' style='margin-top:8px'><option value=''>",
    );
    b.push_str(t("Permission", zh));
    b.push_str(": ");
    b.push_str(t("Default", zh));
    b.push_str("</option><option value='team'>");
    b.push_str(t("Team", zh));
    b.push_str("</option><option value='private'>");
    b.push_str(t("Me", zh));
    b.push_str("</option></select><select id='kbEmbd' style='margin-top:8px'><option value=''>");
    b.push_str(t("Embedding", zh));
    b.push_str(": ");
    b.push_str(t("Default", zh));
    b.push_str("</option><option value='default'>default</option></select><select id='kbParseType' style='margin-top:8px'><option value='Built-in'>");
    b.push_str(t("Parse Type", zh));
    b.push_str(": Built-in</option><option value='Pipeline'>Pipeline</option></select><select id='kbChunkMethod' style='margin-top:8px'><option value='naive'>");
    b.push_str(t("Chunk Method", zh));
    b.push_str(": General</option><option value='qa'>Q&A</option><option value='resume'>Resume</option><option value='manual'>Manual</option><option value='table'>Table</option><option value='paper'>Paper</option><option value='book'>Book</option><option value='laws'>Laws</option><option value='presentation'>Presentation</option><option value='picture'>Picture</option><option value='one'>One</option><option value='audio'>Audio</option><option value='email'>Email</option><option value='tag'>Tag</option><option value='knowledge_graph'>Knowledge Graph</option></select><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='createKb()'>");
    b.push_str(t("Create", zh));
    b.push_str("</button><button class='btn btn-o' onclick='hideCreate()'>Cancel</button></div></div></div>");
    // Rename modal
    b.push_str("<div class='modal-overlay' id='renameModal'><div class='modal'><h2>");
    b.push_str(t("Rename", zh));
    b.push_str("</h2><input id='rnName'><input id='rnDesc' style='margin-top:8px'><input type='hidden' id='rnId'><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='renameKb()'>");
    b.push_str(t("Save", zh));
    b.push_str("</button><button class='btn btn-o' onclick='document.getElementById(\"renameModal\").classList.remove(\"show\")'>Cancel</button></div></div></div>");
    b.push_str("<script>function showCreate(){document.getElementById('createModal').classList.add('show')};function hideCreate(){document.getElementById('createModal').classList.remove('show')};async function createKb(){var n=document.getElementById('kbName').value;if(!n)return;var d=document.getElementById('kbDesc').value;var em=document.getElementById('kbEmbd');var p=document.getElementById('kbPerm');var pt=document.getElementById('kbParseType');var cm=document.getElementById('kbChunkMethod');var body={name:n,description:d};if(em&&em.value)body.embedding_model=em.value;if(p&&p.value)body.permission=p.value;if(pt&&pt.value)body.parse_type=pt.value;if(cm&&cm.value)body.chunk_method=cm.value;await fetch('/api/v1/datasets',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});location.reload()};");
    b.push_str("function kbFilter(){var q=(document.getElementById('kbSearch').value||'').toLowerCase();document.querySelectorAll('#kbGrid .kb-card').forEach(function(c){var n=(c.getAttribute('data-name')||'').toLowerCase();c.style.display=(!q||n.indexOf(q)>=0)?'':'none'})};");
    b.push_str("function kbSort(){var s=document.getElementById('kbSort').value;var grid=document.getElementById('kbGrid');var arr=Array.prototype.slice.call(grid.querySelectorAll('.kb-card'));arr.sort(function(a,b){if(s==='created')return (parseInt(b.getAttribute('data-created'),10)||0)-(parseInt(a.getAttribute('data-created'),10)||0);return (a.getAttribute('data-name')||'').localeCompare(b.getAttribute('data-name')||'')});arr.forEach(function(c){grid.appendChild(c)})};");
    b.push_str("function toggleKbMenu(btn){var id=btn.getAttribute('data-id');var m=document.getElementById('km-'+id);if(!m)return;var open=m.style.display==='block';document.querySelectorAll('.kb-menu').forEach(function(x){x.style.display='none'});m.style.display=open?'none':'block'};function closeKbMenus(){document.querySelectorAll('.kb-menu').forEach(function(x){x.style.display='none'})};document.addEventListener('click',function(e){if(!e.target.closest('.more'))closeKbMenus()});");
    b.push_str("function kbRenameOpen(btn){document.getElementById('rnId').value=btn.getAttribute('data-id');document.getElementById('rnName').value=btn.getAttribute('data-name')||'';document.getElementById('rnDesc').value=btn.getAttribute('data-desc')||'';document.getElementById('renameModal').classList.add('show')};function renameKb(){var id=document.getElementById('rnId').value;var n=document.getElementById('rnName').value;var d=document.getElementById('rnDesc').value;if(!n){alert('Name is required');return}fetch('/api/v1/datasets/'+id,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n,description:d})}).then(function(r){return r.json()}).then(function(res){if(res.code===0){location.reload()}else{alert('Rename failed: '+(res.message||'unknown'))}})};");
    b.push_str("function delKb(btn){var id=btn.getAttribute('data-id');var name=btn.getAttribute('data-name')||'';if(!confirm('Delete knowledge base '+name+'? This will remove all its documents and chunks.'))return;fetch('/api/v1/datasets/'+id,{method:'DELETE'}).then(function(){location.reload()})};");
    b.push_str("function exportKb(btn){var id=btn.getAttribute('data-id');fetch('/api/v1/datasets/'+id).then(function(r){return r.json()}).then(function(d){if(d.code!==0){alert('Export failed: '+(d.message||'unknown'));return}var kb=d.data||{};return fetch('/api/v1/datasets/'+id+'/documents').then(function(r){return r.json()}).then(function(dd){var docs=dd.data||[];return Promise.all(docs.map(function(doc){return fetch('/api/v1/datasets/'+id+'/documents/'+doc.id+'/chunks').then(function(r){return r.json()}).then(function(cd){doc.chunks=(cd.data||[]);return doc}).catch(function(){doc.chunks=[];return doc})}))}).then(function(docs){var out={id:kb.id,name:kb.name,description:kb.description,permission:kb.permission,embd_id:kb.embd_id,parser_config:kb.parser_config,chunk_count:kb.chunk_count,doc_count:kb.doc_count,exported_at:new Date().toISOString(),documents:docs};var blob=new Blob([JSON.stringify(out,null,2)],{type:'application/json'});var a=document.createElement('a');a.href=URL.createObjectURL(blob);a.download=(kb.name||'kb')+'.json';document.body.appendChild(a);a.click();setTimeout(function(){URL.revokeObjectURL(a.href);a.remove()},100)})}).catch(function(e){alert('Export failed: '+e)})};");
    b.push_str("</script>");
    Html(layout(t("Knowledge Bases", zh), &b, zh))
}
/// RAGFlow-compatible KB detail alias: `/dataset/{tab}/{kb_id}` routes
/// (files | retrieval | logs | configuration) render the unified detail page;
/// the tab is pre-selected client-side from the URL path.
pub async fn kb_detail_alias(
    State(state): State<Arc<AppState>>,
    Path((_tab, id)): Path<(String, String)>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    kb_detail_page(State(state), Path(id), req).await
}

pub async fn datasets_alias(
    State(state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    kbs_page(State(state), req).await
}

/// dataset/kb 配置页 embedding 树选择器字段（上游 common-item.tsx EmbeddingSelect，
/// testId ds-settings-basic-embedding-model-select）。label/tip 由页面行内单元格
/// 渲染（必填星标 + embeddingModelTip），此处留空以跳过共享 markup 的标签层。
const CONFIG_EMBD_SELECTOR_FIELD: DefaultModelField = DefaultModelField {
    field_id: "embd_id",
    dom_id: "embdSel",
    label: "",
    update_type: "embd_id",
    response_type: "embedding",
    accepted_types: &["embedding"],
    required: true,
    tooltip: "",
    placeholder: "Please select a embedding model.",
};

/// skills 搜索配置弹层 embedding 树选择器字段（上游 search-config-modal.tsx 的
/// ModelTreeSelect modelTypes=['embedding']，表单钩子经 saveSkillConfig 落库）。
const SKILL_EMBD_SELECTOR_FIELD: DefaultModelField = DefaultModelField {
    field_id: "embd_id",
    dom_id: "skillEmbd",
    label: "",
    update_type: "embd_id",
    response_type: "embedding",
    accepted_types: &["embedding"],
    required: true,
    tooltip: "",
    placeholder: "Select an embedding model",
};

/// memory 设置页 embedding 树选择器字段（上游 memory-model-form.tsx 经 common-item
/// EmbeddingSelect 共享嵌入选择；RayRAG 以共享默认模型树选择器落地，值随 memory 设置表单提交）。
const MEMORY_EMBD_SELECTOR_FIELD: DefaultModelField = DefaultModelField {
    field_id: "embd_id",
    dom_id: "cfgEmbd",
    label: "",
    update_type: "embd_id",
    response_type: "embedding",
    accepted_types: &["embedding"],
    required: true,
    tooltip: "",
    placeholder: "Select model",
};

/// memory 设置页 llm 树选择器字段（上游 ModelTreeSelect 不传 modelTypes → 默认
/// ModelTypeMap.llm_id=['chat']；值随 memory 设置表单提交，不写租户默认）。
const MEMORY_LLM_SELECTOR_FIELD: DefaultModelField = DefaultModelField {
    field_id: "llm_id",
    dom_id: "cfgLlm",
    label: "",
    update_type: "llm_id",
    response_type: "chat",
    accepted_types: &["chat"],
    required: true,
    tooltip: "",
    placeholder: "Select model",
};

/// Upstream knowledgeDetails.rerankTip wording (verbatim from en.ts/zh.ts locales; rendered per-language on the retrieval-test panel).
const RERANK_TIP_EN: &str = r#"Optional. If left empty, RAGFlow will use a combination of weighted keyword similarity and weighted vector cosine similarity; if a rerank model is selected, a weighted reranking score will replace the weighted vector cosine similarity. Please be aware that using a rerank model will significantly increase the system's response time. If you wish to use a rerank model, ensure you use a SaaS reranker; if you prefer a locally deployed rerank model, ensure you start RAGFlow with docker-compose-gpu.yml."#;
const RERANK_TIP_ZH: &str = r#"非必选项：若不选择 rerank 模型，系统将默认采用关键词相似度与向量余弦相似度相结合的混合查询方式；如果设置了 rerank 模型，则混合查询中的向量相似度部分将被 rerank 打分替代。请注意：采用 rerank 模型会非常耗时。如需选用 rerank 模型，建议使用 SaaS 的 rerank 模型服务；如果你倾向使用本地部署的 rerank 模型，请务必确保你使用 docker-compose-gpu.yml 启动 RAGFlow。"#;

/// Retrieval-test-panel reranking tree selector field (upstream testing-form.tsx RerankFormFields -> ModelTreeSelect modelTypes=[rerank] allowClear optional; value is sent as `rerank_id` in the /retrieval request body only and is not persisted).
const TEST_RERANK_SELECTOR_FIELD: DefaultModelField = DefaultModelField {
    field_id: "reranking_id",
    dom_id: "testRerankingSel",
    label: "",
    update_type: "reranking_id",
    response_type: "rerank",
    accepted_types: &["rerank"],
    required: false,
    tooltip: "",
    placeholder: "Select value",
};

pub async fn kb_detail_page(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let kb = state.kbs.get(&id);
    let name = kb.as_ref().map(|k| k.name.clone()).unwrap_or_default();
    let desc = kb
        .as_ref()
        .map(|k| k.description.clone())
        .unwrap_or_default();
    let docs = kb.as_ref().map(|k| k.doc_count).unwrap_or(0);
    let chunks = kb.as_ref().map(|k| k.chunk_count).unwrap_or(0);
    let initial = name
        .chars()
        .next()
        .unwrap_or('K')
        .to_uppercase()
        .collect::<String>();
    let mut b = String::new();
    // 页头：返回按钮 + 头像 + 名称/描述 + 统计
    b.push_str("<div style='display:flex;align-items:center;gap:12px;flex-wrap:wrap;margin-bottom:20px'><a class='btn btn-o' href='/kbs'>← ");
    b.push_str(t("Back to KBs", zh));
    b.push_str("</a><div class='av-lg'>");
    b.push_str(&initial);
    b.push_str("</div><div style='flex:1'><h1 style='margin:0'>");
    b.push_str(&name);
    b.push_str("</h1><p style='margin:2px 0 0;color:var(--tm);font-size:13px'>");
    b.push_str(&desc);
    b.push_str("</p></div><div class='st' style='margin:0'><div class='sc'><span class='lbl'>");
    b.push_str(t("Documents", zh));
    b.push_str("</span><span class='val'>");
    b.push_str(&docs.to_string());
    b.push_str("</span></div><div class='sc'><span class='lbl'>");
    b.push_str(t("Chunks", zh));
    b.push_str("</span><span class='val'>");
    b.push_str(&chunks.to_string());
    b.push_str("</span></div></div></div>");
    // Tab 导航（文档 / 配置 / 测试）
    b.push_str("<div class='tab-bar'><a class='active' id='nav0' onclick='showTab(0)'>📂 ");
    b.push_str(t("Documents", zh));
    b.push_str("</a><a id='nav1' onclick='showTab(1)'>⚙️ ");
    b.push_str(t("Configuration", zh));
    b.push_str("</a><a id='nav2' onclick='showTab(2)'>🧪 ");
    b.push_str(t("Testing", zh));
    b.push_str("</a></div>");
    // Tab 0：文档列表 + 上传 + 批量操作
    b.push_str("<div id='tab0'>");
    b.push_str("<div class='card'><h2>");
    b.push_str(t("Upload", zh));
    b.push_str("</h2><div style='display:flex;align-items:center;gap:8px;flex-wrap:wrap'><input type='file' id='fi' multiple style='max-width:400px'><button class='btn btn-p' id='upBtn' onclick='uploadDoc()'>");
    b.push_str(t("Upload", zh));
    b.push_str("</button><button class='btn btn-o' onclick='emptyAllDocs()'>");
    b.push_str(t("Empty", zh));
    b.push_str("</button><span id='upMsg' style='font-size:12px;color:var(--tm)'></span></div><p style='font-size:11px;color:var(--tm);margin-top:8px'>PDF, DOCX, TXT, MD, HTML, CSV, XLSX, EPUB, JSON, PPT, PNG/JPG(OCR), MP3/WAV(ASR)</p></div>");
    b.push_str("<div class='card' id='batchBar' style='display:none'><div style='display:flex;align-items:center;gap:8px;flex-wrap:wrap'><span id='selCount' style='font-size:13px;color:var(--tm)'></span><button class='btn btn-p btn-sm' onclick='batchParse()'>");
    b.push_str(t("Parse", zh));
    b.push_str("</button><button class='btn btn-o btn-sm' onclick='batchDisable(true)'>");
    b.push_str(t("Disable", zh));
    b.push_str("</button><button class='btn btn-o btn-sm' onclick='batchDisable(false)'>");
    b.push_str(t("Enable", zh));
    b.push_str("</button><button class='btn btn-d btn-sm' onclick='batchDelete()'>");
    b.push_str(t("Delete", zh));
    b.push_str(
        "</button><button class='btn btn-o btn-sm' onclick='docSelAll(false);updateBatchBar()'>✕ ",
    );
    b.push_str(t("Clear", zh));
    b.push_str("</button></div></div>");
    b.push_str("<div class='card'><h2>");
    b.push_str(t("Documents", zh));
    b.push_str(" <span id='docCount' style='font-size:13px;color:var(--tm)'></span></h2><div style='display:flex;align-items:center;gap:8px;margin-bottom:8px'><label style='font-size:12px;color:var(--tm)'>");
    b.push_str(t("Filter", zh));
    b.push_str(":</label><select id='docFilter' onchange='loadDocs()' style='max-width:140px'><option value='all'>");
    b.push_str(t("All", zh));
    b.push_str("</option><option value='enabled'>");
    b.push_str(t("Enabled", zh));
    b.push_str("</option><option value='disabled'>");
    b.push_str(t("Disabled", zh));
    b.push_str("</option><option value='running'>");
    b.push_str(t("Parsing", zh));
    b.push_str("</option><option value='done'>");
    b.push_str(t("Done", zh));
    b.push_str("</option><option value='failed'>");
    b.push_str(t("Failed", zh));
    b.push_str("</option></select></div><div id='dl' style='overflow-x:auto'>");
    b.push_str(t("Loading...", zh));
    b.push_str("</div></div></div>");
    // Tab 1：配置（名称/权限/解析/检索）
    b.push_str("<div id='tab1' style='display:none'>");
    b.push_str("<div class='card'><h2>");
    b.push_str(t("Configuration", zh));
    b.push_str("</h2><p style='color:var(--tm);margin-bottom:16px'>");
    b.push_str(t("Basic settings, parsing &amp; retrieval parameters (camelCase keys aligned with RAGFlow parser_config).", zh));
    b.push_str("</p><table style='width:100%'>");
    b.push_str("<tr><td style='width:220px'><span style='color:var(--d)'>*</span> ");
    b.push_str(t("Name", zh));
    b.push_str("</td><td><input id='cfgName' style='max-width:280px'></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Permission", zh));
    b.push_str("</td><td><select id='cfgPerm' style='max-width:280px'><option value='team'>");
    b.push_str(t("Team", zh));
    b.push_str("</option><option value='private'>");
    b.push_str(t("Me", zh));
    b.push_str("</option></select></td></tr>");
    b.push_str("<tr><td>");
    b.push_str("<span style='color:var(--d)'>*</span> ");
    b.push_str(t("Embedding Model", zh));
    b.push_str(" <span class='tip' tabindex='0' aria-label='");
    b.push_str(&html_escape(t("The default embedding model used by the dataset. Once the dataset has chunks, when switching the embedding model, the system randomly samples a few chunks for a compatibility check, re-embeds them with the new embedding model, and computes cosine similarity between the new and old vectors. Switching is allowed only when the average similarity of the sample is ≥ 0.9. Otherwise, you must delete all chunks in the dataset before you can change it.", zh)));
    b.push_str("' data-tip='");
    b.push_str(&html_escape(t("The default embedding model used by the dataset. Once the dataset has chunks, when switching the embedding model, the system randomly samples a few chunks for a compatibility check, re-embeds them with the new embedding model, and computes cosine similarity between the new and old vectors. Switching is allowed only when the average similarity of the sample is ≥ 0.9. Otherwise, you must delete all chunks in the dataset before you can change it.", zh)));
    b.push_str("'>?</span></td><td>");
    push_default_model_selector(&mut b, &CONFIG_EMBD_SELECTOR_FIELD, zh);
    b.push_str("</td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Page rank", zh));
    b.push_str("</td><td><input id='cfgPageRank' type='number' min='0' value='0' style='max-width:120px'></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Tag sets", zh));
    b.push_str("</td><td><input id='cfgTagSets' placeholder='tag1, tag2' style='max-width:280px'></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Avatar", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:8px'><input type='file' id='cfgAvatar' accept='image/*' onchange='onAvatarPick(this)'> <img id='cfgAvatarPreview' style='display:none;width:36px;height:36px;border-radius:50%;object-fit:cover'></label></td></tr>");
    b.push_str("<tr><td colspan=2 style='padding-top:16px;border-top:1px solid var(--bd)'><h3 style='font-size:14px;margin:0'>");
    b.push_str(t("Parsing Configuration", zh));
    b.push_str("</h3></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Chunk Method", zh));
    b.push_str("</td><td><select id='cfgMethod'><option value='naive'>Naive（通用）</option><option value='token'>Token（令牌）</option><option value='title'>Title（标题）</option></select></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Chunk Token Num", zh));
    b.push_str("</td><td><input type='number' id='cfgTokenNum' min='16' max='100000' value='2048' style='max-width:160px'></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Delimiters", zh));
    b.push_str("</td><td><textarea id='cfgDelims' rows='3' style='max-width:360px' placeholder='每行一个分隔符，例如：句号、分号、空行'></textarea></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Overlap", zh));
    b.push_str(" (%)</td><td><input type='number' id='cfgOverlap' min='0' max='50' value='5' style='max-width:160px'></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Auto Keywords", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='cfgAutoKeywords'> ");
    b.push_str(t("Auto Keywords", zh));
    b.push_str("</label></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Auto Questions", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='cfgAutoQuestions'> ");
    b.push_str(t("Auto Questions", zh));
    b.push_str("</label></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Enable Children Chunks", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='cfgChildren'> ");
    b.push_str(t("Enable Children Chunks", zh));
    b.push_str("</label></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Rerank", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='cfgRerank'> mxbai-rerank-large-v2</label></td></tr>");
    b.push_str("<tr><td colspan=2 style='padding-top:16px;border-top:1px solid var(--bd)'><h3 style='font-size:14px;margin:0'>RAPTOR（递归摘要）</h3></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Enable RAPTOR", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='cfgRaptor'> ");
    b.push_str(t("Enable RAPTOR", zh));
    b.push_str("</label></td></tr>");
    b.push_str("<tr><td colspan=2 style='padding-top:16px;border-top:1px solid var(--bd)'><h3 style='font-size:14px;margin:0'>GraphRAG（实体抽取）</h3></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Enable GraphRAG", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='cfgGraph'> <span class='tip' data-tip='Whether to use knowledge graph(s) in the specified dataset(s) during retrieval for multi-hop question answering. When enabled, the system will first use knowledge graph(s) and then fall back to ordinary retrieval if there is no answer.'></span>");
    b.push_str(t("Enable GraphRAG", zh));
    b.push_str("</label></td></tr>");
    b.push_str("<tr><td colspan=2 style='padding-top:16px;border-top:1px solid var(--bd)'><h3 style='font-size:14px;margin:0'>");
    b.push_str(t("Retrieval Configuration", zh));
    b.push_str("</h3></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Top-K", zh));
    b.push_str("</td><td><input type='number' id='cfgTopK' min='1' max='100' value='10' style='max-width:120px'> <span class='tip' data-tip='The number of retrieved chunks to be fed into the LLM for answer generation. Higher values improve accuracy at the cost of retrieval speed.'>?</span></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Similarity Threshold", zh));
    b.push_str(": <span id='cfgThrV' class='co'>0.2</span></td><td><input type='range' id='cfgThreshold' min='0' max='1' step='0.05' value='0.2' style='width:100%;max-width:280px' oninput='document.getElementById(\\\"cfgThrV\\\").textContent=this.value'> <span class='tip' data-tip='Optional. If left empty, RAGFlow will use a combination of weighted keyword similarity and weighted vector cosine similarity. If you set it to 0.2, the system will only use weighted keyword similarity.'></span></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Vector Weight", zh));
    b.push_str(": <span id='cfgVecV' class='co'>0.3</span></td><td><input type='range' id='cfgVector' min='0' max='1' step='0.05' value='0.3' style='width:100%;max-width:280px' oninput='document.getElementById(\\\"cfgVecV\\\").textContent=this.value'> <span class='tip' data-tip='Weight of the vector cosine similarity in the combined hybrid score. Remaining weight goes to keyword similarity.'></span></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Languages", zh));
    b.push_str("</td><td><span id='langChips' style='display:flex;flex-wrap:wrap;gap:6px'>");
    b.push_str("['English','Chinese','Spanish','French','German','Japanese','Korean','Vietnamese','Arabic','Turkish','Dutch'].forEach(function(nm){var x=document.createElement('button');x.type='button';x.className='lang-chip';x.dataset.lang=nm;x.textContent=nm;x.onclick=function(){toggleLang(x)};document.getElementById('langChips').appendChild(x)});");
    b.push_str("</span><input type='hidden' id='cfgLangs'> <span class='tip' data-tip='Select one or more languages for cross-language search. If no language is selected, the system searches with the original language of the files.'></span></td></tr>");
    b.push_str(
        "</table><div style='margin-top:16px'><button class='btn btn-p' onclick='saveConfig()'>",
    );
    b.push_str(t("Save", zh));
    b.push_str("</button> <span id='cfgMsg' style='font-size:12px;color:var(--tm)'></span></div></div></div>");
    // Tab 2：测试（检索测试面板）
    b.push_str("<div id='tab2' style='display:none'>");
    b.push_str("<div class='card'><h2>");
    b.push_str(t("Testing", zh));
    b.push_str("</h2><p style='color:var(--tm);margin-bottom:16px'>");
    b.push_str(t("输入测试问题，验证本知识库的检索质量。", zh));
    b.push_str("</p><div style='display:grid;grid-template-columns:1fr 1fr;gap:16px'><div><h3>");
    b.push_str(t("Test Query", zh));
    b.push_str("</h3><label style='font-size:12px;display:block'>");
    b.push_str(t("Question", zh));
    b.push_str("</label><input id='tq' placeholder='输入测试问题…'><label style='font-size:12px;display:block;margin-top:8px'>");
    b.push_str(t("Top-K", zh));
    b.push_str("</label><select id='tk'><option>5</option><option selected>10</option><option>20</option></select><label style='font-size:12px;display:block;margin-top:8px'>");
    b.push_str(t("Similarity Threshold", zh));
    b.push_str(": <span id='ttV' class='co'>0.2</span></label><input type='range' id='tt' min='0' max='1' step='0.05' value='0.2' style='width:100%' oninput='document.getElementById(\\\"ttV\\\").textContent=this.value'><label style='font-size:12px;display:block;margin-top:8px'>");
    b.push_str(t("Vector Weight", zh));
    b.push_str(": <span id='twV' class='co'>0.3</span></label><input type='range' id='tw' min='0' max='1' step='0.05' value='0.3' style='width:100%' oninput='document.getElementById(\\\"twV\\\").textContent=this.value'><label style='font-size:12px;display:block;margin-top:8px'><");
    b.push_str(t("Reranking model", zh));
    b.push_str(" <span class='tip' tabindex='0' aria-label='");
    b.push_str(&html_escape(if zh { RERANK_TIP_ZH } else { RERANK_TIP_EN }));
    b.push_str("' data-tip='");
    b.push_str(&html_escape(if zh { RERANK_TIP_ZH } else { RERANK_TIP_EN }));
    b.push_str(">?</span></label>");
    push_default_model_selector(&mut b, &TEST_RERANK_SELECTOR_FIELD, zh);
    b.push_str("<button class='btn btn-p' style='margin-top:12px;width:100%' onclick='runTest()'>");
    b.push_str(t("Test", zh));
    b.push_str("</button></div><div><h3>");
    b.push_str(t("Results", zh));
    b.push_str("</h3><div id='testRes' style='min-height:300px;background:var(--bg);padding:16px;border-radius:var(--r)'><p style='color:var(--tm);text-align:center;padding:40px'>");
    b.push_str(t("Results", zh));
    b.push_str(" 将显示在这里</p></div></div></div></div></div>");
    // 页面 JS
    let embd_fields_payload = serde_json::json!([
        {
            "fieldId": "embd_id",
            "domId": "embdSel",
            "updateType": "embd_id",
            "responseType": "embedding",
            "acceptedTypes": ["embedding"],
            "required": true,
            "placeholder": "Please select a embedding model.",
        },
        {
            "fieldId": "reranking_id",
            "domId": "testRerankingSel",
            "updateType": "reranking_id",
            "responseType": "rerank",
            "acceptedTypes": ["rerank"],
            "required": false,
            "placeholder": "Select value"
        }
    ]);
    b.push_str("<script>const MODEL_FIELDS=");
    b.push_str(&json_script_value(&embd_fields_payload));
    b.push_str(DEFAULT_MODEL_TREE_JS_HEAD);
    b.push_str(DEFAULT_MODEL_TREE_JS_BODY);
    b.push_str("async function saveModelDefault(type,nextValue){var field=modelField(type);if(!field)return;var trigger=document.getElementById(field.domId);if(!trigger)return;var state=defaultModelState(field);if(state.pending)return;var value=String(nextValue==null?'':nextValue).trim();if(value&&!defaultModelOption(field,value))return;var previous=trigger.dataset.savedValue||'';trigger.dataset.value=value;if(!value){trigger.dataset.savedValue='';renderDefaultModelTrigger(field);return}var parsed=parseDefaultModelValue(value);if(!parsed){trigger.dataset.value=previous;trigger.dataset.savedValue=previous;renderDefaultModelTrigger(field);return}trigger.dataset.savedValue=value;var typeMap={embd_id:'embedding'};var persistType=typeMap[type];if(!persistType){renderDefaultModelTrigger(field);return}state.pending=true;fetch('/api/v1/models/default',{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({model_type:persistType,model_provider:parsed.model_provider,model_instance:parsed.model_instance,model_name:parsed.model_name})}).then(function(response){return response.json()}).then(function(data){state.pending=false;if(data&&data.code===0){renderDefaultModelTrigger(field)}else{trigger.dataset.value=previous;trigger.dataset.savedValue=previous;renderDefaultModelTrigger(field)}}).catch(function(){state.pending=false;trigger.dataset.value=previous;trigger.dataset.savedValue=previous;renderDefaultModelTrigger(field)})}async function loadKbModelCatalog(){try{var response=await fetch('/api/v1/models');var data=await response.json();if(!response.ok||data.code!==0)return;DEFAULT_MODEL_CATALOG=(data.data||[]).map(normalizeDefaultModel).filter(function(model){return model.selector});MODEL_FIELDS.forEach(function(field){var state=defaultModelState(field);state.expanded.clear();expandDefaultModelTree(field);renderDefaultModelTrigger(field);renderDefaultModelTree(field)});normalizeDefaultModelStoredValues()}catch(error){}}loadKbModelCatalog();var avatarDataUrl='';function onAvatarPick(input){var file=input.files&&input.files[0];if(!file){avatarDataUrl='';var pv=document.getElementById('cfgAvatarPreview');if(pv){pv.style.display='none';pv.removeAttribute('src')}return}if(!/^image\\/(png|jpeg|jpg|webp|gif)$/i.test(file.type)){var m=document.getElementById('cfgMsg');if(m){m.textContent='仅支持 PNG/JPG/WebP/GIF 图片';m.style.color='var(--d)'}input.value='';return}if(file.size>5*1024*1024){var m2=document.getElementById('cfgMsg');if(m2){m2.textContent='头像图片不能超过 5MB';m2.style.color='var(--d)'}input.value='';return}var reader=new FileReader();reader.onload=function(){avatarDataUrl=String(reader.result||'');var pv=document.getElementById('cfgAvatarPreview');if(pv){pv.src=avatarDataUrl;pv.style.display='inline-block'}};reader.readAsDataURL(file)}function normalizeDefaultModelStoredValues(){MODEL_FIELDS.forEach(function(field){var trigger=document.getElementById(field.domId);if(!trigger)return;var value=trigger.dataset.value||'';if(!value||parseDefaultModelValue(value))return;var parts=value.split('/');if(parts.length!==3)return;var match=DEFAULT_MODEL_CATALOG.find(function(item){return item.name===parts[2]&&item.instance_id===parts[1]&&item.provider_id===parts[0]});if(!match)return;trigger.dataset.value=match.selector;trigger.dataset.savedValue=match.selector;renderDefaultModelTrigger(field)})}</script>");
    b.push_str("<script>var kbId='");
    b.push_str(&id);
    b.push_str("';var pollTimer=null;var disKey='rayrag_disabled_docs';function getDisabled(){try{return JSON.parse(localStorage.getItem(disKey)||'{}')}catch(e){return{}}}function isDocDisabled(id){var m=getDisabled();return !!(m[kbId]&&m[kbId][id])}function setDocDisabled(id,on){var m=getDisabled();if(!m[kbId])m[kbId]={};if(on){m[kbId][id]=1}else{delete m[kbId][id]}try{localStorage.setItem(disKey,JSON.stringify(m))}catch(e){}}function showTab(n){var m=location.pathname.match(/\\/dataset\\/(files|retrieval|logs|configuration)\\//);if(m){var map={files:0,retrieval:2,logs:5,configuration:3};n=map[m[1]]??n}for(var i=0;i<6;i++){var t=document.getElementById('tab'+i);var nav=document.getElementById('nav'+i);if(t)t.style.display='none';if(nav)nav.classList.remove('active')}document.getElementById('tab'+n).style.display='block';document.getElementById('nav'+n).classList.add('active')};");
    b.push_str("function fmtSize(n){n=n||0;if(n<1024)return n+' B';if(n<1048576)return (n/1024).toFixed(1)+' KB';return (n/1048576).toFixed(1)+' MB'};function fmtTime(ts){if(!ts)return '-';try{return new Date(ts).toLocaleString()}catch(e){return ts}};");
    b.push_str("function badge(run){run=run||'UNSTARTED';if(run==='DONE')return '<span class=\\\"badge bs\\\">DONE</span>';if(run==='RUNNING')return '<span class=\\\"badge bi\\\">RUNNING</span>';if(run==='FAILED')return '<span class=\\\"badge bw\\\" style=\\\"color:var(--d)\\\">FAILED</span>';return '<span class=\\\"badge bw\\\">'+(run||'UNSTARTED')+'</span>'};");
    b.push_str("function statusCell(dd){var pct=Math.max(0,Math.min(100,Math.round((dd.progress||0)*100)));var bar='<div class=\\\"progress-bar\\\" style=\\\"width:110px;height:6px;margin-top:3px\\\"><div class=\\\"fill\\\" style=\\\"width:'+pct+'%\\\"></div></div><span style=\\\"font-size:10px;color:var(--tm)\\\">'+pct+'%</span>';return badge(dd.run)+bar};");
    b.push_str("function esc(s){return String(s==null?'':s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/\"/g,'&quot;')};");
    b.push_str("function loadDocs(){fetch('/api/v1/datasets/'+kbId+'/documents').then(r=>r.json()).then(function(d){var docs=d.data||[];document.getElementById('docCount').textContent='('+docs.length+')';var f=document.getElementById('docFilter').value;var shown=docs.filter(function(x){if(f==='enabled')return !isDocDisabled(x.id);if(f==='disabled')return isDocDisabled(x.id);if(f==='running')return x.run==='RUNNING'||x.run==='UNSTARTED';if(f==='done')return x.run==='DONE';if(f==='failed')return x.run==='FAILED';return true});if(!shown.length){document.getElementById('dl').innerHTML='<p style=color:var(--tm);text-align:center;padding:30px>暂无文档</p>';updateBatchBar();return}var h='<table><thead><tr><th><input type=checkbox onclick=docSelAll(this.checked)></th><th>名称</th><th>解析状态</th><th>块数</th><th>大小</th><th>更新时间</th><th style=width:240px>操作</th></tr></thead><tbody>';shown.forEach(function(dd){var dis=isDocDisabled(dd.id);var nm=esc(dd.name);h+='<tr class='+(dis?'dis':'')+' style='+(dis?'opacity:.45':'')+'><td><input type=checkbox class=docCk value='+dd.id+' data-name='+nm+' '+(dis?'disabled':'')+'></td><td style=font-weight:500>'+nm+'</td><td style=min-width:150px>'+statusCell(dd)+(dis?' <span class=\\\"badge bw\\\">已禁用</span>':'')+'</td><td>'+(dd.chunk_count||0)+'</td><td>'+fmtSize(dd.size)+'</td><td style=color:var(--tm);font-size:12px>'+fmtTime(dd.created_at)+'</td><td><button class=\\\"btn btn-o btn-sm\\\" data-id=\\\"'+dd.id+'\\\" data-name=\\\"'+nm+'\\\" onclick=\\\"renameDoc(this)\\\" title=\\\"Rename\\\">✏️</button> <button class=\\\"btn btn-o btn-sm\\\" data-id=\\\"'+dd.id+'\\\"  <a class=\\\"btn btn-o btn-sm\\\" href=\\\"/api/v1/datasets/'+kbId+'/documents/'+dd.id+'/download\\\" title=\\\"Download\\\" download>⬇️</a> <button class=\\\"btn btn-o btn-sm\\\" data-id=\\\"'+dd.id+'\\\" onclick=\\\"reparseDoc(this)\\\">重新解析</button> <button class=\\\"btn btn-o btn-sm\\\" data-id=\\\"'+dd.id+'\\\" onclick=\\\"toggleDisable(this)\\\">'+(dis?'启用':'禁用')+'</button> <button class=\\\"btn btn-d btn-sm\\\" data-id=\\\"'+dd.id+'\\\" onclick=\\\"delDoc(this)\\\">删除</button></td></tr>'});h+='</tbody></table>';document.getElementById('dl').innerHTML=h;updateBatchBar()})};");
    b.push_str("function updateBatchBar(){var n=document.querySelectorAll('.docCk:checked').length;var bar=document.getElementById('batchBar');if(bar){bar.style.display=n?'block':'none';var sc=document.getElementById('selCount');if(sc)sc.textContent=n+' selected'}};function docSelAll(on){document.querySelectorAll('.docCk').forEach(function(x){if(!x.disabled)x.checked=on});updateBatchBar()};function emptyAllDocs(){if(!confirm('删除该知识库的全部文档？'))return;fetch('/api/v1/datasets/'+kbId+'/documents').then(r=>r.json()).then(function(d){var docs=d.data||[];if(!docs.length){alert('暂无文档');return}var n=0;function next(){if(n>=docs.length){loadDocs();return}var id=docs[n++].id;fetch('/api/v1/datasets/'+kbId+'/documents/'+id,{method:'DELETE'}).then(function(){next()}).catch(function(){next()})}next()})};document.addEventListener('change',function(e){if(e.target&&e.target.classList&&e.target.classList.contains('docCk'))updateBatchBar()});");
    b.push_str("function selIds(){var ids=[];document.querySelectorAll('.docCk:checked').forEach(function(x){ids.push(x.value)});return ids};function batchParse(){var ids=selIds();if(!ids.length){alert('请先选择文档');return}if(!confirm('重新解析选中的 '+ids.length+' 个文档？'))return;var n=0;function next(){if(n>=ids.length){loadDocs();schedulePoll();return}var id=ids[n++];fetch('/api/v1/datasets/'+kbId+'/documents/'+id+'/reparse',{method:'POST'}).then(function(){next()}).catch(function(){next()})}next()};");
    b.push_str("function batchDisable(on){var ids=selIds();if(!ids.length){alert('请先选择文档');return}ids.forEach(function(id){setDocDisabled(id,on)});loadDocs()};");
    b.push_str("function batchDelete(){var ids=selIds();if(!ids.length){alert('请先选择文档');return}if(!confirm('删除选中的 '+ids.length+' 个文档？'))return;var n=0;function next(){if(n>=ids.length){loadDocs();return}var id=ids[n++];fetch('/api/v1/datasets/'+kbId+'/documents/'+id,{method:'DELETE'}).then(function(){next()}).catch(function(){next()})}next()};");
    b.push_str("function renameDoc(btn){var id=btn.getAttribute('data-id');var old=btn.getAttribute('data-name')||'';var nn=prompt('重命名文档：',old);if(!nn)return;fetch('/api/v1/datasets/'+kbId+'/documents/'+id,{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:nn})}).then(r=>r.json()).then(function(){loadDocs()})};");
    b.push_str("function reparseDoc(btn){var id=btn.getAttribute('data-id');if(!confirm('重新解析该文档？'))return;fetch('/api/v1/datasets/'+kbId+'/documents/'+id+'/reparse',{method:'POST'}).then(function(){loadDocs();schedulePoll()})};");
    b.push_str("function delDoc(btn){var id=btn.getAttribute('data-id');if(!confirm('删除该文档？'))return;fetch('/api/v1/datasets/'+kbId+'/documents/'+id,{method:'DELETE'}).then(function(){loadDocs()})};");
    b.push_str("function toggleDisable(btn){var id=btn.getAttribute('data-id');setDocDisabled(id,!isDocDisabled(id));loadDocs()};");
    b.push_str("function schedulePoll(){if(pollTimer)return;pollTimer=setInterval(function(){fetch('/api/v1/datasets/'+kbId+'/documents').then(r=>r.json()).then(function(d){var docs=d.data||[];loadDocs();var busy=docs.some(function(x){return x.run==='RUNNING'||x.run==='UNSTARTED'});if(!busy&&pollTimer){clearInterval(pollTimer);pollTimer=null}})},3000)};");
    b.push_str("function uploadDoc(){var files=document.getElementById('fi').files;if(!files||!files.length)return;var btn=document.getElementById('upBtn');var msg=document.getElementById('upMsg');if(btn)btn.disabled=true;var n=0;function next(){if(n>=files.length){if(btn)btn.disabled=false;if(msg)msg.textContent='';loadDocs();schedulePoll();return}var f=files[n++];var fd=new FormData();fd.append('file',f,f.name);if(msg)msg.textContent='上传中 '+(n)+'/'+(files.length)+' …';fetch('/api/v1/datasets/'+kbId+'/documents',{method:'POST',body:fd}).then(r=>r.json()).then(function(){next()}).catch(function(){next()})}next()};");
    b.push_str("function loadConfig(){avatarDataUrl='';fetch('/api/v1/datasets/'+kbId).then(r=>r.json()).then(function(d){if(d.code!==0)return;var kb=d.data||{};var nm=document.getElementById('cfgName');if(nm)nm.value=kb.name||'';var pm=document.getElementById('cfgPerm');if(pm)pm.value=(kb.permission==='team')?'team':'private';var em=document.getElementById('embdSel');if(em&&kb.embd_id){em.dataset.value=kb.embd_id;em.dataset.savedValue=kb.embd_id;var embdField=modelField('embd_id');if(embdField){renderDefaultModelTrigger(embdField);renderDefaultModelTree(embdField)}normalizeDefaultModelStoredValues()}var pc={};try{pc=JSON.parse(kb.parser_config||'{}')}catch(e){}if(pc.chunk_method)document.getElementById('cfgMethod').value=pc.chunk_method;var tn=parseInt(pc.chunk_token_num,10);if(!isNaN(tn)&&tn>0)document.getElementById('cfgTokenNum').value=tn;var delims=Array.isArray(pc.delimiters)?pc.delimiters:(pc.delimiter?[pc.delimiter]:[]);document.getElementById('cfgDelims').value=delims.join('\\n');var op=parseFloat(pc.overlapped_percent);if(!isNaN(op))document.getElementById('cfgOverlap').value=Math.round(op*100);document.getElementById('cfgAutoKeywords').checked=!!pc.auto_keywords;document.getElementById('cfgAutoQuestions').checked=!!pc.auto_questions;document.getElementById('cfgChildren').checked=!!pc.enable_children;document.getElementById('cfgRerank').checked=!!(pc.rerank||pc.rerank_model);document.getElementById('cfgRaptor').checked=!!(pc.raptorEn||pc.raptor_enabled);document.getElementById('cfgGraph').checked=!!(pc.graphEn||pc.graphrag_enabled);var pr=parseInt(pc.page_rank,10);if(!isNaN(pr))document.getElementById('cfgPageRank').value=pr;if(kb.tag_sets)document.getElementById('cfgTagSets').value=kb.tag_sets;if(kb.avatar){avatarDataUrl=kb.avatar;var ap=document.getElementById('cfgAvatarPreview');if(ap){ap.src=kb.avatar;ap.style.display='inline-block'}}var tk=parseInt(pc.top_k,10);if(!isNaN(tk)&&tk>0)document.getElementById('cfgTopK').value=tk;var th=parseFloat(pc.similarity_threshold);if(!isNaN(th)){document.getElementById('cfgThreshold').value=th;document.getElementById('cfgThrV').textContent=th}var vw=parseFloat(pc.vector_similarity_weight);if(!isNaN(vw)){document.getElementById('cfgVector').value=vw;document.getElementById('cfgVecV').textContent=vw}var pc2={};try{pc2=JSON.parse(kb.prompt_config||'{}')}catch(e){}if(Array.isArray(pc2.cross_languages)){var lg=document.getElementById('cfgLangs');if(lg)lg.value=pc2.cross_languages.join(',');document.querySelectorAll('.lang-chip').forEach(function(c){if(pc2.cross_languages.indexOf(c.dataset.lang)>=0)c.classList.add('on')})}})};");
    b.push_str("function saveConfig(){var delims=document.getElementById('cfgDelims').value.split('\\n').map(function(s){return s.trim()}).filter(Boolean);if(!delims.length)delims=['\\n'];var body={name:document.getElementById('cfgName').value,permission:document.getElementById('cfgPerm').value,embd_id:(document.getElementById('embdSel').dataset.value||'default'),chunk_method:document.getElementById('cfgMethod').value,chunk_token_num:parseInt(document.getElementById('cfgTokenNum').value,10)||2048,delimiters:delims,overlapped_percent:Math.min(1,Math.max(0,(parseFloat(document.getElementById('cfgOverlap').value)||0)/100)),auto_keywords:document.getElementById('cfgAutoKeywords').checked,auto_questions:document.getElementById('cfgAutoQuestions').checked,enable_children:document.getElementById('cfgChildren').checked,rerank:document.getElementById('cfgRerank').checked,raptorEn:document.getElementById('cfgRaptor').checked,graphEn:document.getElementById('cfgGraph').checked,top_k:parseInt(document.getElementById('cfgTopK').value,10)||10,page_rank:parseInt(document.getElementById('cfgPageRank').value,10)||0,tag_sets:document.getElementById('cfgTagSets').value,avatar:avatarDataUrl,similarity_threshold:parseFloat(document.getElementById('cfgThreshold').value),vector_similarity_weight:parseFloat(document.getElementById('cfgVector').value)};body.prompt_config={cross_languages:(document.getElementById('cfgLangs').value||'').split(',').map(function(x){return x.trim()}).filter(Boolean)};var m=document.getElementById('cfgMsg');if(m){m.textContent='保存中…';m.style.color='var(--tm)'}fetch('/api/v1/datasets/'+kbId,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(r=>r.json()).then(function(d){if(m){if(d.code===0){m.textContent='✅ 已保存';m.style.color='var(--s)'}else{m.textContent='保存失败: '+(d.message||'未知错误');m.style.color='var(--d)'}}})};");
    b.push_str("function runTest(){var q=document.getElementById('tq').value;if(!q){alert('请输入测试问题');return}var k=parseInt(document.getElementById('tk').value,10);var t=parseFloat(document.getElementById('tt').value);var w=parseFloat(document.getElementById('tw').value);var rerankEl=document.getElementById('testRerankingSel');var rerankingId=rerankEl?rerankEl.dataset.value:'';;var body={question:q,kb_ids:[kbId],top_k:k,similarity_threshold:t,vector_similarity_weight:w};if(rerankingId)body.rerank_id=rerankingId;document.getElementById('testRes').innerHTML='<p style=color:var(--tm);text-align:center;padding:40px>检索中…</p>';fetch('/api/v1/retrieval',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(function(resp){return resp.json()}).then(function(d){if(d.code!==0){document.getElementById('testRes').innerHTML='<p style=color:var(--d)>检索失败: '+(d.message||'未知错误')+'</p>';return}var chunks=(d.data&&d.data.chunks)||[];var h='';chunks.forEach(function(c,i){h+='<div class=ri><span class=rs>#'+(i+1)+' · 相似度 '+(c.score||0).toFixed(4)+'</span><div class=rc>'+hl(c.content||'',q)+'</div><div class=rm>📄 '+(c.doc_name||'')+'</div></div>'});document.getElementById('testRes').innerHTML=h||'<p style=color:var(--tm);text-align:center;padding:40px>无匹配结果</p>'}).catch(function(e){document.getElementById('testRes').innerHTML='<p style=color:var(--d)>请求失败: '+e+'</p>'})};");
    b.push_str("function hl(t,q){if(!q)return t;try{return t.replace(new RegExp(q.replace(/[.*+?^${}()|[\\\\]\\\\]/g,'\\\\$&'),'gi'),function(m){return '<mark>'+m+'</mark>'})}catch(e){return t}};");
    b.push_str("fetch('/api/v1/providers').then(r=>r.json()).then(function(d){var sel=document.getElementById('embdSel');if(!sel)return;(d.data||[]).forEach(function(p){if(!p.models)return;(p.models||[]).forEach(function(m){var o=document.createElement('option');o.value=m;o.textContent=m;sel.appendChild(o)})})}).catch(function(){});");
    b.push_str("loadDocs();loadConfig();</script>");
    Html(layout(&name, &b, zh))
}

pub async fn dataset_page(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let kb = state.kbs.get(&id);
    let name = kb.as_ref().map(|k| k.name.clone()).unwrap_or_default();
    let desc = kb
        .as_ref()
        .map(|k| k.description.clone())
        .unwrap_or_default();
    let chunks = kb.as_ref().map(|k| k.chunk_count).unwrap_or(0);
    let docs = kb.as_ref().map(|k| k.doc_count).unwrap_or(0);
    let initial = name
        .chars()
        .next()
        .unwrap_or('K')
        .to_uppercase()
        .collect::<String>();
    let mut b = String::new();
    // RAGFlow-style: sidebar + content area
    b.push_str("<div class='ds-layout'><div class='ds-sidebar'><div class='av-lg'>");
    b.push_str(&initial);
    b.push_str("</div><h3>");
    b.push_str(&name);
    b.push_str("</h3><div class='stats'>");
    b.push_str(&docs.to_string());
    b.push(' ');
    b.push_str(t("Files", zh));
    b.push_str(" · ");
    b.push_str(&chunks.to_string());
    b.push_str(" chunks</div><div class='stats'>");
    b.push_str(&desc);
    b.push_str("</div>");
    // Sidebar nav
    b.push_str(
        "<ul class='ds-nav'><li><a href='#' onclick='showTab(0)' id='nav0' class='active'>📂 ",
    );
    b.push_str(t("Documents", zh));
    b.push_str("</a></li><li><a href='#' onclick='showTab(1)' id='nav1'>📊 ");
    b.push_str(t("Overview", zh));
    b.push_str("</a></li><li><a href='#' onclick='showTab(2)' id='nav2'>🧪 ");
    b.push_str(t("Testing", zh));
    b.push_str("</a></li><li><a href='#' onclick='showTab(3)' id='nav3'>⚙️ ");
    b.push_str(t("Configuration", zh));
    b.push_str("</a></li><li><a href='#' onclick='showTab(4)' id='nav4'>🕸️ ");
    b.push_str(t("Knowledge Graph", zh));
    b.push_str("</a></li><li><a href='#' onclick='showTab(5)' id='nav5'>📜 ");
    b.push_str(t("Logs", zh));
    b.push_str("</a></li></ul></div>");
    // Content
    b.push_str("<div class='ds-content'>");
    // Tab 0: Documents
    b.push_str("<div id='tab0'><div class='card'><h2>");
    b.push_str(t("Upload", zh));
    b.push_str("</h2><div style='display:flex;align-items:center;gap:8px;flex-wrap:wrap'><input type='file' id='fi' multiple style='max-width:400px'><button class='btn btn-p' id='upBtn' onclick='uploadDoc()'>");
    b.push_str(t("Upload", zh));
    b.push_str("</button><span id='upMsg' style='font-size:12px;color:var(--tm)'></span></div><p style='font-size:11px;color:var(--tm);margin-top:8px'>PDF, DOCX, TXT, MD, HTML, CSV, XLSX, EPUB, JSON, PPT, PNG/JPG(OCR), MP3/WAV(ASR)</p></div>");
    // Batch operations bar (解析 / 禁用 / 启用 / 删除) — appears once rows are selected
    b.push_str("<div class='card' id='batchBar' style='display:none'><div style='display:flex;align-items:center;gap:8px;flex-wrap:wrap'><span id='selCount' style='font-size:13px;color:var(--tm)'></span><button class='btn btn-p btn-sm' onclick='batchParse()'>");
    b.push_str(t("Parse", zh));
    b.push_str("</button><button class='btn btn-o btn-sm' onclick='batchDisable(true)'>");
    b.push_str(t("Disable", zh));
    b.push_str("</button><button class='btn btn-o btn-sm' onclick='batchDisable(false)'>");
    b.push_str(t("Enable", zh));
    b.push_str("</button><button class='btn btn-d btn-sm' onclick='batchDelete()'>");
    b.push_str(t("Delete", zh));
    b.push_str(
        "</button><button class='btn btn-o btn-sm' onclick='docSelAll(false);updateBatchBar()'>✕ ",
    );
    b.push_str(t("Clear", zh));
    b.push_str("</button></div></div>");
    b.push_str("<div class='card'><h2>");
    b.push_str(t("Documents", zh));
    b.push_str(" <span id='docCount' style='font-size:13px;color:var(--tm)'></span></h2><div style='display:flex;align-items:center;gap:8px;margin-bottom:8px'><label style='font-size:12px;color:var(--tm)'>");
    b.push_str(t("Filter", zh));
    b.push_str(":</label><select id='docFilter' onchange='loadDocs()' style='max-width:140px'><option value='all'>");
    b.push_str(t("All", zh));
    b.push_str("</option><option value='enabled'>");
    b.push_str(t("Enabled", zh));
    b.push_str("</option><option value='disabled'>");
    b.push_str(t("Disabled", zh));
    b.push_str("</option><option value='running'>");
    b.push_str(t("Parsing", zh));
    b.push_str("</option><option value='done'>");
    b.push_str(t("Done", zh));
    b.push_str("</option><option value='failed'>");
    b.push_str(t("Failed", zh));
    b.push_str("</option></select></div><div id='dl'>Loading...</div></div></div>");
    // Tab 1: Overview
    b.push_str(
        "<div id='tab1' style='display:none'><div class='st'><div class='sc'><span class='lbl'>",
    );
    b.push_str(t("Total Docs", zh));
    b.push_str("</span><span class='val'>");
    b.push_str(&docs.to_string());
    b.push_str("</span></div><div class='sc'><span class='lbl'>");
    b.push_str(t("Total Chunks", zh));
    b.push_str("</span><span class='val'>");
    b.push_str(&chunks.to_string());
    b.push_str("</span></div><div class='sc'><span class='lbl'>");
    b.push_str(t("Success", zh));
    b.push_str("</span><span class='val' style='color:var(--s)' id='ovDone'>0</span><div class='progress-bar'><div class='fill' id='ovDoneBar' style='width:0%'></div></div></div><div class='sc'><span class='lbl'>");
    b.push_str(t("Parsing", zh));
    b.push_str("</span><span class='val' style='color:#f59e0b' id='ovRun'>0</span><div class='progress-bar'><div class='fill' id='ovRunBar' style='width:0%;background:#f59e0b'></div></div></div><div class='sc'><span class='lbl'>");
    b.push_str(t("Failed", zh));
    b.push_str("</span><span class='val' style='color:var(--d)' id='ovFail'>0</span><div class='progress-bar'><div class='fill' id='ovFailBar' style='width:0%;background:var(--d)'></div></div></div></div><div class='card'><h2>");
    b.push_str(t("Recent Activity", zh));
    b.push_str("</h2><table><thead><tr><th>");
    b.push_str(t("Document", zh));
    b.push_str("</th><th>");
    b.push_str(t("Status", zh));
    b.push_str("</th><th>");
    b.push_str(t("Chunks", zh));
    b.push_str("</th><th>");
    b.push_str(t("Date", zh));
    b.push_str("</th></tr></thead><tbody id='actList'></tbody></table></div></div>");
    // Tab 2: Testing
    b.push_str("<div id='tab2' style='display:none'><div class='card'><h2>");
    b.push_str(t("Testing", zh));
    b.push_str("</h2><p style='color:var(--tm);margin-bottom:16px'>");
    b.push_str(t("Test retrieval quality with sample queries.", zh));
    b.push_str("</p><div style='display:grid;grid-template-columns:1fr 1fr;gap:16px'><div><h3>");
    b.push_str(t("Test Settings", zh));
    b.push_str("</h3><label style='font-size:12px'>");
    b.push_str(t("Question", zh));
    b.push_str("</label><input id='testQ' placeholder='");
    b.push_str(t("Enter test query...", zh));
    b.push_str("'><label style='font-size:12px;margin-top:8px;display:block'>");
    b.push_str(t("Top-K", zh));
    b.push_str("</label><select id='testK'><option>5</option><option selected>10</option><option>20</option></select><label style='font-size:12px;margin-top:8px;display:block'>");
    b.push_str(t("Similarity Threshold", zh));
    b.push_str(": <span id='testTV' class='co'>0.2</span></label><input type='range' id='testT' min='0' max='1' step='0.05' value='0.2' style='width:100%' oninput='document.getElementById(\\\"testTV\\\").textContent=this.value'><label style='font-size:12px;margin-top:8px;display:block'>");
    b.push_str(t("Vector Weight", zh));
    b.push_str(": <span id='testWV' class='co'>0.3</span></label><input type='range' id='testW' min='0' max='1' step='0.05' value='0.3' style='width:100%' oninput='document.getElementById(\\\"testWV\\\").textContent=this.value'><label style='font-size:12px;margin-top:8px;display:block'><");
    b.push_str(t("Reranking model", zh));
    b.push_str(" <span class='tip' tabindex='0' aria-label='");
    b.push_str(&html_escape(if zh { RERANK_TIP_ZH } else { RERANK_TIP_EN }));
    b.push_str("' data-tip='");
    b.push_str(&html_escape(if zh { RERANK_TIP_ZH } else { RERANK_TIP_EN }));
    b.push_str(">?</span></label>");
    push_default_model_selector(&mut b, &TEST_RERANK_SELECTOR_FIELD, zh);
    b.push_str(
        "<button class='btn btn-p' style='margin-top:12px;width:100%' onclick='testSearch()'>",
    );
    b.push_str(t("Test", zh));
    b.push_str("</button></div><div><h3>");
    b.push_str(t("Results", zh));
    b.push_str("</h3><div id='testResults' style='min-height:300px'><p style='color:var(--tm);text-align:center;padding:40px'>");
    b.push_str(t("Results", zh));
    b.push(' ');
    b.push_str(t("will appear here", zh));
    b.push_str("</p></div></div></div></div></div>");
    // Tab 3: Configuration
    b.push_str("<div id='tab3' style='display:none'><div class='card'><h2>");
    b.push_str(t("Configuration", zh));
    b.push_str("</h2><p style='color:var(--tm);margin-bottom:16px'>");
    b.push_str(t("Basic settings, parsing &amp; retrieval parameters (keys aligned with RAGFlow camelCase). Saved via PUT /api/v1/datasets/{id}.", zh));
    b.push_str("</p><table style='width:100%'>");
    b.push_str("<tr><td style='width:220px'><span style='color:var(--d)'>*</span> ");
    b.push_str(t("Name", zh));
    b.push_str("</td><td><input id='cfgName' style='max-width:280px'></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Permission", zh));
    b.push_str("</td><td><select id='cfgPerm' style='max-width:280px'><option value='team'>");
    b.push_str(t("Team", zh));
    b.push_str("</option><option value='private'>");
    b.push_str(t("Me", zh));
    b.push_str("</option></select></td></tr>");
    b.push_str("<tr><td>");
    b.push_str("<span style='color:var(--d)'>*</span> ");
    b.push_str(t("Embedding Model", zh));
    b.push_str(" <span class='tip' tabindex='0' aria-label='");
    b.push_str(&html_escape(t("The default embedding model used by the dataset. Once the dataset has chunks, when switching the embedding model, the system randomly samples a few chunks for a compatibility check, re-embeds them with the new embedding model, and computes cosine similarity between the new and old vectors. Switching is allowed only when the average similarity of the sample is ≥ 0.9. Otherwise, you must delete all chunks in the dataset before you can change it.", zh)));
    b.push_str("' data-tip='");
    b.push_str(&html_escape(t("The default embedding model used by the dataset. Once the dataset has chunks, when switching the embedding model, the system randomly samples a few chunks for a compatibility check, re-embeds them with the new embedding model, and computes cosine similarity between the new and old vectors. Switching is allowed only when the average similarity of the sample is ≥ 0.9. Otherwise, you must delete all chunks in the dataset before you can change it.", zh)));
    b.push_str("'>?</span></td><td>");
    push_default_model_selector(&mut b, &CONFIG_EMBD_SELECTOR_FIELD, zh);
    b.push_str("</td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Page rank", zh));
    b.push_str("</td><td><input id='cfgPageRank' type='number' min='0' value='0' style='max-width:120px'></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Tag sets", zh));
    b.push_str("</td><td><input id='cfgTagSets' placeholder='tag1, tag2' style='max-width:280px'></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Avatar", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:8px'><input type='file' id='cfgAvatar' accept='image/*' onchange='onAvatarPick(this)'> <img id='cfgAvatarPreview' style='display:none;width:36px;height:36px;border-radius:50%;object-fit:cover'></label></td></tr>");
    b.push_str("<tr><td colspan=2 style='padding-top:16px;border-top:1px solid var(--bd)'><h3 style='font-size:14px;margin:0'>");
    b.push_str(t("Parsing Configuration", zh));
    b.push_str("</h3></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Chunk Method", zh));
    b.push_str("</td><td><select id='chunkMethod' onchange='toggleChunkOptions()'><option value='naive'>Naive (token-based)</option><option value='manual'>Manual</option><option value='qa'>Q&amp;A</option><option value='table'>Table</option><option value='paper'>Paper</option><option value='book'>Book</option><option value='laws'>Laws</option><option value='presentation'>Presentation</option><option value='picture'>Picture</option><option value='audio'>Audio</option><option value='email'>Email</option><option value='one'>One</option><option value='resume'>Resume</option><option value='tag'>Tag</option><option value='knowledge_graph'>Knowledge Graph</option></select></td></tr>");
    b.push_str("<tr id='chunkSizeRow'><td>");
    b.push_str(t("Chunk Size", zh));
    b.push_str("</td><td><select id='chunkSize'><option>256</option><option selected>512</option><option>1024</option><option>2048</option></select></td></tr>");
    b.push_str("<tr id='overlapRow'><td>");
    b.push_str(t("Overlap", zh));
    b.push_str("</td><td><select id='overlap'><option>0%</option><option selected>5%</option><option>10%</option><option>20%</option></select></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Delimiter", zh));
    b.push_str("</td><td><input id='cfgDelim' value='\\n\\n' style='max-width:200px'></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Language", zh));
    b.push_str("</td><td><select id='cfgLang' style='max-width:280px'><option value='English'>English</option><option value='Chinese'>Chinese</option></select></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("PDF Parser", zh));
    b.push_str("</td><td><select id='cfgPdfParser' style='max-width:280px'><option value='Built-in'>Built-in</option><option value='DeepDOC'>DeepDOC</option></select></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Layout Recognize", zh));
    b.push_str("</td><td><select id='cfgLayoutRecognize' style='max-width:280px'><option value='DeepDOC'>DeepDOC</option><option value='PaddleOCR'>PaddleOCR</option><option value='OpenDataLoader'>OpenDataLoader</option><option value='MinerU'>MinerU</option><option value='SoMark'>SoMark</option></select></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Page Index", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='cfgPageIndex'> ");
    b.push_str(t("Extract page numbers", zh));
    b.push_str("</label></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Image & Table Context Window", zh));
    b.push_str("</td><td><input id='cfgImgTableWin' type='number' min='0' max='30' value='5' style='max-width:120px'></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Excel to HTML", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='cfgExcelHtml'> ");
    b.push_str(t("Convert Excel to HTML tables", zh));
    b.push_str("</label></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Children Delimiter", zh));
    b.push_str(
        "</td><td><input id='cfgChildrenDelims' value='\\n' style='max-width:200px'></td></tr>",
    );
    b.push_str("<tr><td>");
    b.push_str(t("Title Levels", zh));
    b.push_str("</td><td><input id='cfgTitleLevels' placeholder='0,1,2' style='max-width:200px'></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Auto Metadata", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='cfgAutoMeta'> ");
    b.push_str(t("Extract metadata automatically", zh));
    b.push_str("</label></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Parser Type", zh));
    b.push_str("</td><td><select id='cfgParserType'><option value='deepdoc'>DeepDOC (PDF/DOCX)</option><option value='plain'>Plain Text</option></select></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Auto Keywords", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='cfgAutoKeywords'> ");
    b.push_str(t("Auto Keywords", zh));
    b.push_str("</label></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Auto Questions", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='cfgAutoQuestions'> ");
    b.push_str(t("Auto Questions", zh));
    b.push_str("</label></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Enable Children Chunks", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='cfgChildren'> ");
    b.push_str(t("Enable Children Chunks", zh));
    b.push_str("</label></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Rerank", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='cfgRerank'> mxbai-rerank-large-v2</label></td></tr>");
    b.push_str("<tr><td colspan=2 style='padding-top:16px;border-top:1px solid var(--bd)'><h3 style='font-size:14px;margin:0'>RAPTOR (Recursive Abstractive Processing)</h3></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Enable RAPTOR", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='raptorEn'> ");
    b.push_str(t("Enable recursive summarization tree", zh));
    b.push_str("</label></td></tr>");
    b.push_str("<tr><td>Max Depth</td><td><select id='raptorDepth' disabled><option>3</option><option selected>5</option><option>7</option></select></td></tr>");
    b.push_str("<tr><td>Threshold</td><td><input id='raptorThreshold' type='number' step='0.01' min='0' max='1' value='0.7' disabled style='max-width:120px'></td></tr>");
    b.push_str("<tr><td>Generation scope</td><td><select id='raptorScope' disabled><option>Dataset</option><option>Single file</option></select></td></tr>");
    b.push_str("<tr><td>Prompt</td><td><textarea id='raptorPrompt' rows='2' disabled style='width:100%;font-size:12px'></textarea></td></tr>");
    b.push_str("<tr><td>Max token</td><td><input id='raptorMaxToken' type='number' value='512' disabled style='max-width:120px'></td></tr>");
    b.push_str("<tr><td>Clustering method</td><td><select id='raptorCluster' disabled><option>GMM</option><option>AHC</option></select></td></tr>");
    b.push_str("<tr><td>Max cluster</td><td><input id='raptorMaxCluster' type='number' value='64' disabled style='max-width:120px'></td></tr>");
    b.push_str("<tr><td>Random seed</td><td><input id='raptorSeed' type='number' value='0' disabled style='max-width:120px'></td></tr>");
    b.push_str("<tr><td colspan=2 style='padding-top:16px;border-top:1px solid var(--bd)'><h3 style='font-size:14px;margin:0'>GraphRAG (Entity Extraction)</h3></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Enable GraphRAG", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='graphEn'> ");
    b.push_str(t("Enable entity-relation extraction", zh));
    b.push_str("</label></td></tr>");
    b.push_str("<tr><td>Entity Types</td><td><input id='cfgEntityTypes' value='organization, person, location, event, concept' style='font-size:12px'></td></tr>");
    b.push_str("<tr><td>Method</td><td><select id='cfgGraphMethod'><option>Light</option><option>General</option></select></td></tr>");
    b.push_str("<tr><td>Batch chunk token size</td><td><input id='graphBatchSize' type='number' value='8196' style='max-width:120px'></td></tr>");
    b.push_str("<tr><td>Entity resolution</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='graphEntityRes'> <span style='font-size:12px'>Resolve entities across graph</span></label></td></tr>");
    b.push_str("<tr><td>Community reports</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='graphCommReports'> <span style='font-size:12px'>Generate community reports</span></label></td></tr>");
    b.push_str("<tr><td colspan=2 style='padding-top:16px;border-top:1px solid var(--bd)'><h3 style='font-size:14px;margin:0'>");
    b.push_str(t("Retrieval Configuration", zh));
    b.push_str("</h3></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Top-K", zh));
    b.push_str("</td><td><input type='number' id='cfgTopK' min='1' max='100' value='10' style='max-width:120px'> <span class='tip' data-tip='The number of retrieved chunks to be fed into the LLM for answer generation. Higher values improve accuracy at the cost of retrieval speed.'>?</span></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Similarity Threshold", zh));
    b.push_str(": <span id='cfgThrV' class='co'>0.2</span></td><td><input type='range' id='cfgThreshold' min='0' max='1' step='0.05' value='0.2' style='width:100%;max-width:280px' oninput='document.getElementById(\\\"cfgThrV\\\").textContent=this.value'> <span class='tip' data-tip='Optional. If left empty, RAGFlow will use a combination of weighted keyword similarity and weighted vector cosine similarity. If you set it to 0.2, the system will only use weighted keyword similarity.'></span></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Vector Weight", zh));
    b.push_str(": <span id='cfgVecV' class='co'>0.3</span></td><td><input type='range' id='cfgVector' min='0' max='1' step='0.05' value='0.3' style='width:100%;max-width:280px' oninput='document.getElementById(\\\"cfgVecV\\\").textContent=this.value'> <span class='tip' data-tip='Weight of the vector cosine similarity in the combined hybrid score. Remaining weight goes to keyword similarity.'></span></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Languages", zh));
    b.push_str("</td><td><span id='langChips' style='display:flex;flex-wrap:wrap;gap:6px'>");
    b.push_str("['English','Chinese','Spanish','French','German','Japanese','Korean','Vietnamese','Arabic','Turkish','Dutch'].forEach(function(nm){var x=document.createElement('button');x.type='button';x.className='lang-chip';x.dataset.lang=nm;x.textContent=nm;x.onclick=function(){toggleLang(x)};document.getElementById('langChips').appendChild(x)});");
    b.push_str("</span><input type='hidden' id='cfgLangs'> <span class='tip' data-tip='Select one or more languages for cross-language search. If no language is selected, the system searches with the original language of the files.'></span></td></tr>");
    b.push_str(
        "</table><div style='margin-top:16px'><button class='btn btn-p' onclick='saveConfig()'>",
    );
    b.push_str(t("Save", zh));
    b.push_str("</button> <span id='cfgMsg' style='font-size:12px;color:var(--tm)'></span></div></div></div>");
    // Tab 4: Knowledge Graph
    b.push_str("<div id='tab4' style='display:none'><div class='card'><h2>");
    b.push_str(t("Knowledge Graph", zh));
    b.push_str("</h2><p style='color:var(--tm);margin-bottom:16px'>");
    b.push_str(t(
        "Entity-relation graph extracted from indexed documents.",
        zh,
    ));
    b.push_str("</p><button class='btn btn-p' onclick='buildGraph()'>");
    b.push_str(t("Build", zh));
    b.push(' ');
    b.push_str(t("Graph", zh));
    b.push_str("</button><div id='graphResult' style='margin-top:16px;background:var(--bg);padding:20px;border-radius:var(--r);min-height:200px;font-family:monospace;font-size:12px;white-space:pre-wrap;line-height:1.8'><p style='color:var(--tm);text-align:center;padding:40px'>");
    b.push_str(t(
        "Click Build Graph to extract entities and relations.",
        zh,
    ));
    b.push_str("</p></div></div></div>");
    b.push_str("</div></div>");
    // JS
    let embd_fields_payload = serde_json::json!([
        {
            "fieldId": "embd_id",
            "domId": "embdSel",
            "updateType": "embd_id",
            "responseType": "embedding",
            "acceptedTypes": ["embedding"],
            "required": true,
            "placeholder": "Please select a embedding model.",
        },
        {
            "fieldId": "reranking_id",
            "domId": "testRerankingSel",
            "updateType": "reranking_id",
            "responseType": "rerank",
            "acceptedTypes": ["rerank"],
            "required": false,
            "placeholder": "Select value"
        }
    ]);
    b.push_str("<script>const MODEL_FIELDS=");
    b.push_str(&json_script_value(&embd_fields_payload));
    b.push_str(DEFAULT_MODEL_TREE_JS_HEAD);
    b.push_str(DEFAULT_MODEL_TREE_JS_BODY);
    b.push_str("async function saveModelDefault(type,nextValue){var field=modelField(type);if(!field)return;var trigger=document.getElementById(field.domId);if(!trigger)return;var state=defaultModelState(field);if(state.pending)return;var value=String(nextValue==null?'':nextValue).trim();if(value&&!defaultModelOption(field,value))return;var previous=trigger.dataset.savedValue||'';trigger.dataset.value=value;if(!value){trigger.dataset.savedValue='';renderDefaultModelTrigger(field);return}var parsed=parseDefaultModelValue(value);if(!parsed){trigger.dataset.value=previous;trigger.dataset.savedValue=previous;renderDefaultModelTrigger(field);return}trigger.dataset.savedValue=value;var typeMap={embd_id:'embedding'};var persistType=typeMap[type];if(!persistType){renderDefaultModelTrigger(field);return}state.pending=true;fetch('/api/v1/models/default',{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({model_type:persistType,model_provider:parsed.model_provider,model_instance:parsed.model_instance,model_name:parsed.model_name})}).then(function(response){return response.json()}).then(function(data){state.pending=false;if(data&&data.code===0){renderDefaultModelTrigger(field)}else{trigger.dataset.value=previous;trigger.dataset.savedValue=previous;renderDefaultModelTrigger(field)}}).catch(function(){state.pending=false;trigger.dataset.value=previous;trigger.dataset.savedValue=previous;renderDefaultModelTrigger(field)})}async function loadKbModelCatalog(){try{var response=await fetch('/api/v1/models');var data=await response.json();if(!response.ok||data.code!==0)return;DEFAULT_MODEL_CATALOG=(data.data||[]).map(normalizeDefaultModel).filter(function(model){return model.selector});MODEL_FIELDS.forEach(function(field){var state=defaultModelState(field);state.expanded.clear();expandDefaultModelTree(field);renderDefaultModelTrigger(field);renderDefaultModelTree(field)});normalizeDefaultModelStoredValues()}catch(error){}}loadKbModelCatalog();var avatarDataUrl='';function onAvatarPick(input){var file=input.files&&input.files[0];if(!file){avatarDataUrl='';var pv=document.getElementById('cfgAvatarPreview');if(pv){pv.style.display='none';pv.removeAttribute('src')}return}if(!/^image\\/(png|jpeg|jpg|webp|gif)$/i.test(file.type)){var m=document.getElementById('cfgMsg');if(m){m.textContent='仅支持 PNG/JPG/WebP/GIF 图片';m.style.color='var(--d)'}input.value='';return}if(file.size>5*1024*1024){var m2=document.getElementById('cfgMsg');if(m2){m2.textContent='头像图片不能超过 5MB';m2.style.color='var(--d)'}input.value='';return}var reader=new FileReader();reader.onload=function(){avatarDataUrl=String(reader.result||'');var pv=document.getElementById('cfgAvatarPreview');if(pv){pv.src=avatarDataUrl;pv.style.display='inline-block'}};reader.readAsDataURL(file)}function normalizeDefaultModelStoredValues(){MODEL_FIELDS.forEach(function(field){var trigger=document.getElementById(field.domId);if(!trigger)return;var value=trigger.dataset.value||'';if(!value||parseDefaultModelValue(value))return;var parts=value.split('/');if(parts.length!==3)return;var match=DEFAULT_MODEL_CATALOG.find(function(item){return item.name===parts[2]&&item.instance_id===parts[1]&&item.provider_id===parts[0]});if(!match)return;trigger.dataset.value=match.selector;trigger.dataset.savedValue=match.selector;renderDefaultModelTrigger(field)})}</script>");
    b.push_str("<script>var kbId='");
    b.push_str(&id);
    b.push_str("';var pollTimer=null;var disKey='rayrag_disabled_docs';function getDisabled(){try{return JSON.parse(localStorage.getItem(disKey)||'{}')}catch(e){return{}}}function isDocDisabled(id){var m=getDisabled();return !!(m[kbId]&&m[kbId][id])}function setDocDisabled(id,on){var m=getDisabled();if(!m[kbId])m[kbId]={};if(on){m[kbId][id]=1}else{delete m[kbId][id]}try{localStorage.setItem(disKey,JSON.stringify(m))}catch(e){}}function showTab(n){for(var i=0;i<6;i++){var t=document.getElementById('tab'+i);var nav=document.getElementById('nav'+i);if(t)t.style.display='none';if(nav)nav.classList.remove('active')}document.getElementById('tab'+n).style.display='block';document.getElementById('nav'+n).classList.add('active')};");
    b.push_str("function fmtSize(n){n=n||0;if(n<1024)return n+' B';if(n<1048576)return (n/1024).toFixed(1)+' KB';return (n/1048576).toFixed(1)+' MB'};function fmtTime(ts){if(!ts)return '-';try{return new Date(ts).toLocaleString()}catch(e){return ts}};");
    b.push_str("function badge(run){run=run||'UNSTARTED';if(run==='DONE')return '<span class=\\\"badge bs\\\">DONE</span>';if(run==='RUNNING')return '<span class=\\\"badge bi\\\">RUNNING</span>';if(run==='FAILED')return '<span class=\\\"badge bw\\\" style=\\\"color:var(--d)\\\">FAILED</span>';return '<span class=\\\"badge bw\\\">'+(run||'UNSTARTED')+'</span>'};");
    b.push_str("function statusCell(dd){var pct=Math.max(0,Math.min(100,Math.round((dd.progress||0)*100)));var bar='<div class=\\\"progress-bar\\\" style=\\\"width:110px;height:6px;margin-top:3px\\\"><div class=\\\"fill\\\" style=\\\"width:'+pct+'%\\\"></div></div><span style=\\\"font-size:10px;color:var(--tm)\\\">'+pct+'%</span>';return badge(dd.run)+bar};");
    b.push_str("function esc(s){return String(s==null?'':s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/\"/g,'&quot;')};");
    b.push_str("function loadDocs(){fetch('/api/v1/datasets/'+kbId+'/documents').then(function(r){return r.json()}).then(function(d){var docs=d.data||[];document.getElementById('docCount').textContent='('+docs.length+')';var f=document.getElementById('docFilter').value;var shown=docs.filter(function(x){if(f==='enabled')return !isDocDisabled(x.id);if(f==='disabled')return isDocDisabled(x.id);if(f==='running')return x.run==='RUNNING'||x.run==='UNSTARTED';if(f==='done')return x.run==='DONE';if(f==='failed')return x.run==='FAILED';return true});if(!shown.length){document.getElementById('dl').innerHTML='<p style=color:var(--tm);text-align:center;padding:30px>暂无文档</p>';updateBatchBar();return}var h='<table><thead><tr><th><input type=checkbox onclick=docSelAll(this.checked)></th><th>名称</th><th>上传时间</th><th style=min-width:150px>状态</th><th>块数</th><th>大小</th><th style=width:240px>操作</th></tr></thead><tbody>';shown.forEach(function(dd){var dis=isDocDisabled(dd.id);var nm=esc(dd.name);h+='<tr class='+(dis?'dis':'')+' style='+(dis?'opacity:.45':'')+'><td><input type=checkbox class=docCk value='+dd.id+' data-name='+nm+' '+(dis?'disabled':'')+'></td><td style=font-weight:500>'+nm+'</td><td style=color:var(--tm);font-size:12px>'+fmtTime(dd.created_at)+'</td><td>'+statusCell(dd)+(dis?' <span class=\\\"badge bw\\\">已禁用</span>':'')+'</td><td>'+(dd.chunk_count||0)+'</td><td>'+fmtSize(dd.size)+'</td><td><a class=\\\"btn btn-o btn-sm\\\" href=\\\"/documents/'+kbId+'/'+dd.id+'\\\">查看</a> <button class=\\\"btn btn-o btn-sm\\\" data-id=\\\"'+dd.id+'\\\" data-name=\\\"'+nm+'\\\" onclick=\\\"renameDoc(this)\\\" title=\\\"Rename\\\">✏️</button> <button class=\\\"btn btn-o btn-sm\\\" data-id=\\\"'+dd.id+'\\\"  <a class=\\\"btn btn-o btn-sm\\\" href=\\\"/api/v1/datasets/'+kbId+'/documents/'+dd.id+'/download\\\" title=\\\"Download\\\" download>⬇️</a> <button class=\\\"btn btn-o btn-sm\\\" data-id=\\\"'+dd.id+'\\\" onclick=\\\"reparseDoc(this)\\\">重新解析</button> <button class=\\\"btn btn-o btn-sm\\\" data-id=\\\"'+dd.id+'\\\" onclick=\\\"toggleDisable(this)\\\">'+(dis?'启用':'禁用')+'</button> <button class=\\\"btn btn-d btn-sm\\\" data-id=\\\"'+dd.id+'\\\" onclick=\\\"delDoc(this)\\\">删除</button></td></tr>'});h+='</tbody></table>';document.getElementById('dl').innerHTML=h;updateBatchBar();updateOverview(docs)})};");
    b.push_str("function updateBatchBar(){var n=document.querySelectorAll('.docCk:checked').length;var bar=document.getElementById('batchBar');if(bar){bar.style.display=n?'block':'none';var sc=document.getElementById('selCount');if(sc)sc.textContent=n+' selected'}};function docSelAll(on){document.querySelectorAll('.docCk').forEach(function(x){if(!x.disabled)x.checked=on});updateBatchBar()};document.addEventListener('change',function(e){if(e.target&&e.target.classList&&e.target.classList.contains('docCk'))updateBatchBar()});");
    b.push_str("function selIds(){var ids=[];document.querySelectorAll('.docCk:checked').forEach(function(x){ids.push(x.value)});return ids};function batchParse(){var ids=selIds();if(!ids.length){alert('请先选择文档');return}if(!confirm('重新解析选中的 '+ids.length+' 个文档？'))return;var n=0;function next(){if(n>=ids.length){loadDocs();schedulePoll();return}var id=ids[n++];fetch('/api/v1/datasets/'+kbId+'/documents/'+id+'/reparse',{method:'POST'}).then(function(){next()}).catch(function(){next()})}next()};");
    b.push_str("function batchDisable(on){var ids=selIds();if(!ids.length){alert('请先选择文档');return}ids.forEach(function(id){setDocDisabled(id,on)});loadDocs()};");
    b.push_str("function batchDelete(){var ids=selIds();if(!ids.length){alert('请先选择文档');return}if(!confirm('删除选中的 '+ids.length+' 个文档？'))return;var n=0;function next(){if(n>=ids.length){loadDocs();return}var id=ids[n++];fetch('/api/v1/datasets/'+kbId+'/documents/'+id,{method:'DELETE'}).then(function(){next()}).catch(function(){next()})}next()};");
    b.push_str("function renameDoc(btn){var id=btn.getAttribute('data-id');var old=btn.getAttribute('data-name')||'';var nn=prompt('重命名文档：',old);if(!nn)return;fetch('/api/v1/datasets/'+kbId+'/documents/'+id,{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:nn})}).then(function(r){return r.json()}).then(function(){loadDocs()})};");
    b.push_str("function reparseDoc(btn){var id=btn.getAttribute('data-id');if(!confirm('重新解析该文档？'))return;fetch('/api/v1/datasets/'+kbId+'/documents/'+id+'/reparse',{method:'POST'}).then(function(){loadDocs();schedulePoll()})};");
    b.push_str("function delDoc(btn){var id=btn.getAttribute('data-id');if(!confirm('删除该文档？'))return;fetch('/api/v1/datasets/'+kbId+'/documents/'+id,{method:'DELETE'}).then(function(){loadDocs()})};");
    b.push_str("function toggleDisable(btn){var id=btn.getAttribute('data-id');setDocDisabled(id,!isDocDisabled(id));loadDocs()};");
    b.push_str("function schedulePoll(){if(pollTimer)return;pollTimer=setInterval(function(){fetch('/api/v1/datasets/'+kbId+'/documents').then(function(r){return r.json()}).then(function(d){var docs=d.data||[];loadDocs();var busy=docs.some(function(x){return x.run==='RUNNING'||x.run==='UNSTARTED'});if(!busy&&pollTimer){clearInterval(pollTimer);pollTimer=null}})},3000)};");
    b.push_str("function uploadDoc(){var files=document.getElementById('fi').files;if(!files||!files.length)return;var btn=document.getElementById('upBtn');var msg=document.getElementById('upMsg');if(btn)btn.disabled=true;var n=0;function next(){if(n>=files.length){if(btn)btn.disabled=false;if(msg)msg.textContent='';loadDocs();schedulePoll();return}var f=files[n++];var fd=new FormData();fd.append('file',f,f.name);if(msg)msg.textContent='上传中 '+(n)+'/'+(files.length)+' …';fetch('/api/v1/datasets/'+kbId+'/documents',{method:'POST',body:fd}).then(function(r){return r.json()}).then(function(){next()}).catch(function(){next()})}next()};");
    b.push_str("function clearAll(){if(!confirm('Delete ALL documents in this dataset?'))return;fetch('/api/v1/datasets/'+kbId+'/documents').then(function(r){return r.json()}).then(function(d){var docs=d.data||[];if(!docs.length){loadDocs();return}var n=0;function next(){if(n>=docs.length){loadDocs();return}var id=docs[n++].id;fetch('/api/v1/datasets/'+kbId+'/documents/'+id,{method:'DELETE'}).then(function(){next()})}next()})};");
    b.push_str("function updateOverview(docs){var done=0,run=0,fail=0,other=0;docs.forEach(function(x){if(x.run==='DONE')done++;else if(x.run==='RUNNING'||x.run==='UNSTARTED')run++;else if(x.run==='FAILED')fail++;else other++});var total=docs.length||1;function set(id,bar,v){var e=document.getElementById(id);if(e)e.textContent=v;var b=document.getElementById(bar);if(b)b.style.width=Math.round(v/total*100)+'%'};set('ovDone','ovDoneBar',done);set('ovRun','ovRunBar',run);set('ovFail','ovFailBar',fail);var act=document.getElementById('actList');if(act){var h='';docs.slice(0,10).forEach(function(x){h+='<tr><td>'+esc(x.name)+'</td><td>'+badge(x.run)+'</td><td>'+(x.chunk_count||0)+'</td><td style=color:var(--tm);font-size:12px>'+fmtTime(x.updated_at)+'</td></tr>'});act.innerHTML=h||'<tr><td colspan=4 style=color:var(--tm);text-align:center>暂无活动</td></tr>'}};");
    b.push_str("function testSearch(){var q=document.getElementById('testQ').value;if(!q){alert('请输入测试问题');return}var k=parseInt(document.getElementById('testK').value,10);var t=parseFloat(document.getElementById('testT').value);var w=parseFloat(document.getElementById('testW').value);var rerankEl=document.getElementById('testRerankingSel');var rerankingId=rerankEl?rerankEl.dataset.value:'';;var body={question:q,kb_ids:[kbId],top_k:k,similarity_threshold:t,vector_similarity_weight:w};if(rerankingId)body.rerank_id=rerankingId;document.getElementById('testResults').innerHTML='<p style=color:var(--tm);text-align:center;padding:40px>检索中…</p>';fetch('/api/v1/retrieval',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(function(resp){return resp.json()}).then(function(d){if(d.code!==0){document.getElementById('testResults').innerHTML='<p style=color:var(--d)>检索失败: '+(d.message||'未知错误')+'</p>';return}var chunks=(d.data&&d.data.chunks)||[];var h='';chunks.forEach(function(c,i){h+='<div class=ri><span class=rs>#'+(i+1)+' · 相似度 '+(c.score||0).toFixed(4)+'</span><div class=rc>'+hl(c.content||'',q)+'</div><div class=rm>📄 '+(c.doc_name||'')+'</div></div>'});document.getElementById('testResults').innerHTML=h||'<p style=color:var(--tm);text-align:center;padding:40px>无匹配结果</p>'}).catch(function(e){document.getElementById('testResults').innerHTML='<p style=color:var(--d)>请求失败: '+e+'</p>'})};");
    b.push_str("function hl(t,q){if(!q)return t;try{return t.replace(new RegExp(q.replace(/[.*+?^${}()|[\\]\\\\]/g,'\\\\$&'),'gi'),function(m){return '<mark>'+m+'</mark>'})}catch(e){return t}};");
    b.push_str("function loadConfig(){avatarDataUrl='';fetch('/api/v1/datasets/'+kbId).then(function(r){return r.json()}).then(function(d){if(d.code!==0)return;var kb=d.data||{};var nm=document.getElementById('cfgName');if(nm)nm.value=kb.name||'';var pm=document.getElementById('cfgPerm');if(pm)pm.value=(kb.permission==='team')?'team':'private';var em=document.getElementById('embdSel');if(em&&kb.embd_id){em.dataset.value=kb.embd_id;em.dataset.savedValue=kb.embd_id;var embdField=modelField('embd_id');if(embdField){renderDefaultModelTrigger(embdField);renderDefaultModelTree(embdField)}normalizeDefaultModelStoredValues()}var pc={};try{pc=JSON.parse(kb.parser_config||'{}')}catch(e){}if(pc.chunk_method)document.getElementById('chunkMethod').value=pc.chunk_method;var cs=parseInt(pc.chunk_size,10);if(!isNaN(cs)&&cs>0){var csSel=document.getElementById('chunkSize');for(var j=0;j<csSel.options.length;j++){if(parseInt(csSel.options[j].value,10)===cs){csSel.selectedIndex=j;break}}}if(pc.overlap){var ov=document.getElementById('overlap');for(var k=0;k<ov.options.length;k++){if(ov.options[k].value===pc.overlap){ov.selectedIndex=k;break}}}if(pc.delimiter)document.getElementById('cfgDelim').value=pc.delimiter;if(pc.parser_type)document.getElementById('cfgParserType').value=pc.parser_type;var cd2=Array.isArray(pc.children_delimiters)?pc.children_delimiters.join('\\n'):'';if(cd2)document.getElementById('cfgChildrenDelims').value=cd2;if(pc.title_levels)document.getElementById('cfgTitleLevels').value=pc.title_levels.join(',');document.getElementById('cfgAutoMeta').checked=!!pc.enable_metadata;var pr=parseInt(pc.page_rank,10);if(!isNaN(pr))document.getElementById('cfgPageRank').value=pr;if(kb.tag_sets)document.getElementById('cfgTagSets').value=kb.tag_sets;if(kb.avatar){avatarDataUrl=kb.avatar;var ap=document.getElementById('cfgAvatarPreview');if(ap){ap.src=kb.avatar;ap.style.display='inline-block'}}document.getElementById('cfgExcelHtml').checked=!!pc.excel_to_html;if(pc.layout_recognize)document.getElementById('cfgLayoutRecognize').value=pc.layout_recognize;document.getElementById('cfgAutoKeywords').checked=!!pc.auto_keywords;document.getElementById('cfgAutoQuestions').checked=!!pc.auto_questions;document.getElementById('cfgChildren').checked=!!pc.enable_children;document.getElementById('cfgRerank').checked=!!(pc.rerank||pc.rerank_model);document.getElementById('raptorEn').checked=!!(pc.raptorEn||pc.raptor_enabled);var avatarDataUrl='';function onAvatarPick(input){var f=input.files&&input.files[0];if(!f)return;if(f.size>300*1024){alert('Avatar too large (max 300KB)');input.value='';return}var r=new FileReader();r.onload=function(){avatarDataUrl=r.result;var img=document.getElementById('cfgAvatarPreview');if(img){img.src=avatarDataUrl;img.style.display='inline-block'}};r.readAsDataURL(f)}var rd=document.getElementById('raptorDepth');rd.disabled=!document.getElementById('raptorEn').checked;var rth=parseFloat(pc.raptor_threshold);if(!isNaN(rth))document.getElementById('raptorThreshold').value=rth;if(pc.raptor_scope)document.getElementById('raptorScope').value=pc.raptor_scope;if(pc.raptor_prompt)document.getElementById('raptorPrompt').value=pc.raptor_prompt;var rmt=parseInt(pc.raptor_max_token,10);if(!isNaN(rmt))document.getElementById('raptorMaxToken').value=rmt;if(pc.raptor_cluster_method)document.getElementById('raptorCluster').value=pc.raptor_cluster_method;var rmc=parseInt(pc.raptor_max_cluster,10);if(!isNaN(rmc))document.getElementById('raptorMaxCluster').value=rmc;var rseed=parseInt(pc.raptor_random_seed,10);if(!isNaN(rseed))document.getElementById('raptorSeed').value=rseed;var gbs=parseInt(pc.graphrag_batch_chunk_size,10);if(!isNaN(gbs))document.getElementById('graphBatchSize').value=gbs;document.getElementById('graphEntityRes').checked=!!pc.graphrag_entity_resolution;document.getElementById('graphCommReports').checked=!!pc.graphrag_community_reports;if(pc.graphrag_method)document.getElementById('cfgGraphMethod').value=pc.graphrag_method;document.getElementById('graphEn').checked=!!(pc.graphEn||pc.graphrag_enabled);if(pc.entity_types)document.getElementById('cfgEntityTypes').value=pc.entity_types;var tk=parseInt(pc.top_k,10);if(!isNaN(tk)&&tk>0)document.getElementById('cfgTopK').value=tk;var th=parseFloat(pc.similarity_threshold);if(!isNaN(th)){document.getElementById('cfgThreshold').value=th;document.getElementById('cfgThrV').textContent=th}var vw=parseFloat(pc.vector_similarity_weight);if(!isNaN(vw)){document.getElementById('cfgVector').value=vw;document.getElementById('cfgVecV').textContent=vw}var pc2={};try{pc2=JSON.parse(kb.prompt_config||'{}')}catch(e){}if(Array.isArray(pc2.cross_languages)){var lg=document.getElementById('cfgLangs');if(lg)lg.value=pc2.cross_languages.join(',');document.querySelectorAll('.lang-chip').forEach(function(c){if(pc2.cross_languages.indexOf(c.dataset.lang)>=0)c.classList.add('on')})}})};");
    b.push_str("function saveConfig(){var body={name:document.getElementById('cfgName').value,permission:document.getElementById('cfgPerm').value,embd_id:(document.getElementById('embdSel').dataset.value||'default'),chunk_method:document.getElementById('chunkMethod').value,chunk_size:parseInt(document.getElementById('chunkSize').value,10)||512,overlap:document.getElementById('overlap').value,delimiter:document.getElementById('cfgDelim').value,parser_type:document.getElementById('cfgParserType').value,layout_recognize:document.getElementById('cfgLayoutRecognize').value,language:document.getElementById('cfgLang').value,pdf_parser:document.getElementById('cfgPdfParser').value,page_index:document.getElementById('cfgPageIndex').checked,image_and_table_window:parseInt(document.getElementById('cfgImgTableWin').value,10)||5,excel_to_html:document.getElementById('cfgExcelHtml').checked,children_delimiters:document.getElementById('cfgChildrenDelims').value.split('\\n').map(function(x){return x.trim()}).filter(Boolean),title_levels:document.getElementById('cfgTitleLevels').value.split(',').map(function(x){return x.trim()}).filter(Boolean),enable_metadata:document.getElementById('cfgAutoMeta').checked,page_rank:parseInt(document.getElementById('cfgPageRank').value,10)||0,tag_sets:document.getElementById('cfgTagSets').value,avatar:avatarDataUrl,auto_keywords:document.getElementById('cfgAutoKeywords').checked,auto_questions:document.getElementById('cfgAutoQuestions').checked,enable_children:document.getElementById('cfgChildren').checked,rerank:document.getElementById('cfgRerank').checked,raptorEn:document.getElementById('raptorEn').checked,graphEn:document.getElementById('graphEn').checked,entity_types:document.getElementById('cfgEntityTypes').value,top_k:parseInt(document.getElementById('cfgTopK').value,10)||10,similarity_threshold:parseFloat(document.getElementById('cfgThreshold').value),vector_similarity_weight:parseFloat(document.getElementById('cfgVector').value)};body.prompt_config={cross_languages:(document.getElementById('cfgLangs').value||'').split(',').map(function(x){return x.trim()}).filter(Boolean)};var rd=document.getElementById('raptorDepth');if(document.getElementById('raptorEn').checked)body.raptor_depth=parseInt(rd.value,10)||5;if(document.getElementById('raptorEn').checked){body.raptor_threshold=parseFloat(document.getElementById('raptorThreshold').value)||0.7;body.raptor_scope=document.getElementById('raptorScope').value;body.raptor_prompt=document.getElementById('raptorPrompt').value;body.raptor_max_token=parseInt(document.getElementById('raptorMaxToken').value,10)||512;body.raptor_cluster_method=document.getElementById('raptorCluster').value;body.raptor_max_cluster=parseInt(document.getElementById('raptorMaxCluster').value,10)||64;body.raptor_random_seed=parseInt(document.getElementById('raptorSeed').value,10)||0}body.graphrag_method=document.getElementById('cfgGraphMethod').value;body.graphrag_batch_chunk_size=parseInt(document.getElementById('graphBatchSize').value,10)||8196;body.graphrag_entity_resolution=document.getElementById('graphEntityRes').checked;body.graphrag_community_reports=document.getElementById('graphCommReports').checked;var m=document.getElementById('cfgMsg');if(m){m.textContent='保存中…';m.style.color='var(--tm)'}fetch('/api/v1/datasets/'+kbId,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(function(r){return r.json()}).then(function(d){if(m){if(d.code===0){m.textContent='✅ 已保存';m.style.color='var(--s)'}else{m.textContent='保存失败: '+(d.message||'未知错误');m.style.color='var(--d)'}}})};");
    b.push_str("function toggleLang(btn){btn.classList.toggle('on');var sel=[];document.querySelectorAll('.lang-chip.on').forEach(function(x){sel.push(x.dataset.lang)});var h=document.getElementById('cfgLangs');if(h)h.value=sel.join(',');}
function toggleChunkOptions(){var m=document.getElementById('chunkMethod').value;var show=['naive','manual'].indexOf(m)>=0;document.getElementById('chunkSizeRow').style.display=show?'':'none';document.getElementById('overlapRow').style.display=show?'':'none'};document.getElementById('raptorEn').onchange=function(){document.getElementById('raptorDepth').disabled=!this.checked};");
    b.push_str("function buildGraph(){document.getElementById('graphResult').innerHTML='<p style=color:var(--tm)>Building... (LLM entity extraction)</p>';fetch('/api/v1/datasets/'+kbId+'/graphrag/build',{method:'POST'}).then(function(r){return r.json()}).then(function(d){if(d.code!==0){document.getElementById('graphResult').textContent='Error: '+(d.message||'unknown');return}var data=d.data||{};var nodes=data.node_list||[];var maxD=1;nodes.forEach(function(n){maxD=Math.max(maxD,n.degree||1)});var R=120;var svg='<svg width=640 height=460 style=background:var(--bg);border-radius:var(--r)><text x=320 y=30 text-anchor=middle font-size=14 fill=var(--t2)>'+data.nodes+' nodes / '+data.edges+' edges</text>';nodes.slice(0,24).forEach(function(n,i){var ang=2*Math.PI*i/nodes.slice(0,24).length;var cx=320+R*Math.cos(ang);var cy=240+R*Math.sin(ang);var r=(n.degree||1)/maxD*22+10;svg+='<line x1=320 y1=240 x2='+cx+' y2='+cy+' stroke=var(--p) stroke-opacity=0.35></line><circle cx='+cx+' cy='+cy+' r='+r+' fill=var(--p) fill-opacity=0.15 stroke=var(--p)></circle><text x='+cx+' y='+(cy+r+14)+' text-anchor=middle font-size=11 fill=var(--t2)>'+String(n.name||'').slice(0,14)+'</text>'});svg+='<circle cx=320 cy=240 r=26 fill=var(--p)></circle><text x=320 y=245 text-anchor=middle font-size=11 fill=var(--bg)>KB</text></svg>';var h='<div style=margin-bottom:8px>Nodes: '+data.nodes+' | Edges: '+data.edges+' | Documents: '+data.documents+' | Entities: '+data.entities+'</div>'+svg+'<div style=margin-top:10px;font-size:12px;color:var(--tm)>(环形图：节点大小=degree，中心=知识库)</div>';document.getElementById('graphResult').innerHTML=h}).catch(function(e){document.getElementById('graphResult').textContent='Error: '+e})};");
    b.push_str("fetch('/api/v1/providers').then(function(r){return r.json()}).then(function(d){var sel=document.getElementById('embdSel');if(!sel)return;(d.data||[]).forEach(function(p){if(!p.models)return;(p.models||[]).forEach(function(m){var o=document.createElement('option');o.value=m;o.textContent=m;sel.appendChild(o)})})}).catch(function(){});");
    b.push_str("loadDocs();loadConfig();</script>");
    // Tab 5: Logs (RAGFlow parity) — parse run status with File filter
    b.push_str("<div id='tab5' style='display:none'><div class='card'><h2>");
    b.push_str(t("Logs", zh));
    b.push_str("</h2><div class='st' style='margin-bottom:12px'><div class='sc'><span class='lbl'>File</span><select id='logFile' onchange='filterLogs()' style='max-width:260px'><option value=''>All files</option></select></div></div>");
    b.push_str("<table style='width:100%'><thead><tr><th style='text-align:left'>Document</th><th style='text-align:left'>Status</th><th style='text-align:left'>Chunks</th><th style='text-align:left'>Progress</th></tr></thead><tbody id='logBody'></tbody></table>");
    b.push_str("<script>function filterLogs(){var f=document.getElementById('logFile').value;var rows=document.querySelectorAll('#logBody tr');rows.forEach(function(r){r.style.display=(!f||r.getAttribute('data-f')===f)?'':'none'})};function loadLogs(){fetch('/api/v1/datasets/'+kbId+'/documents').then(function(r){return r.json()}).then(function(d){var docs=d.data||[];var sel=document.getElementById('logFile');var cur=sel.value;sel.innerHTML='<option value=\"\">All files</option>';var seen={};var h='';docs.forEach(function(dd){var nm=dd.name||'?';if(!seen[nm]){seen[nm]=1;sel.innerHTML+='<option value=\"'+nm.replace(/'/g,'&#39;')+'\">'+nm+'</option>'}});sel.value=cur;docs.forEach(function(dd){var nm=dd.name||\'?\';var st=dd.run||dd.status||'UNSTARTED';var ch=dd.chunk_count!=null?dd.chunk_count:0;var pg=Math.max(0,Math.min(100,Math.round((dd.progress||0)*100)));h+='<tr data-f=\"'+nm.replace(/'/g,'&#39;')+'\"><td>'+nm+'</td><td>'+badge(st)+'</td><td>'+ch+'</td><td><div style=\"width:110px;height:6px;background:var(--bd)\"><div style=\"height:6px;width:'+pg+'%;background:var(--p)\"></div></div></td></tr>'});document.getElementById('logBody').innerHTML=h;filterLogs()})};loadLogs();</script></div>");
    Html(layout(&name, &b, zh))
}

pub async fn chat_page(
    State(state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let convs = state.conversations.list();
    let mut cards = String::new();
    for c in convs.iter().rev().take(20) {
        let initial = c
            .name
            .chars()
            .next()
            .unwrap_or('C')
            .to_uppercase()
            .collect::<String>();
        let _preview = c
            .messages
            .last()
            .map(|m| m.content.chars().take(80).collect::<String>())
            .unwrap_or_else(|| "No messages".into());
        cards.push_str(&format!("<div class='chat-card' onclick=\"location.href='/chat/{0}'\"><div style='display:flex;align-items:center;gap:12px'><div class='av' style='width:40px;height:40px;font-size:16px'>{1}</div><div><h3 style='font-size:15px'>{2}</h3><p style='font-size:12px;color:var(--tm)'>{3} msgs</p></div></div><p style='font-size:13px;color:var(--t2);margin-top:12px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap'>{4}</p><button class='btn btn-d btn-sm' style='margin-top:8px' onclick=\"event.stopPropagation();delConv('{5}')\">Del</button><button class='btn btn-o btn-sm' style='margin-top:8px' onclick=\"event.stopPropagation();renConv('{6}')\">✏️</button><button class='btn btn-o btn-sm' style='margin-top:8px' onclick=\"event.stopPropagation();shareConv('{7}')\">🔗 Share</button></div>", c.id, c.id, initial, c.name.chars().take(30).collect::<String>(), c.messages.len(), c.id, c.id, c.id));
    }
    if convs.is_empty() {
        cards="<p style='color:var(--tm);text-align:center;padding:40px;grid-column:1/-1'>No conversations yet.</p>".into();
    }
    let mut b = String::new();
    b.push_str(
        "<div style='display:flex;align-items:center;margin-bottom:24px'><h1 style='margin:0'>",
    );
    b.push_str(t("Chat", zh));
    b.push_str("</h1><button class='btn btn-p' style='margin-left:auto' onclick='newChat()'>+ ");
    b.push_str(t("New Chat", zh));
    b.push_str("</button></div>");
    b.push_str("<div class='chat-list'>");
    b.push_str(&cards);
    b.push_str("</div>");
    b.push_str("<div class='modal-overlay' id='newChatModal'><div class='modal'><h2>New Chat</h2><p style='color:var(--tm);margin-bottom:12px'>Start a new conversation with RayRAG.</p><input id='chatName' placeholder='Name' style='width:100%;margin-bottom:8px'><select id='chatKbs' multiple style='width:100%;min-height:90px;margin-bottom:16px'><option value=''>Loading knowledge bases…</option></select><div style='display:flex;gap:8px'><button class='btn btn-p' onclick='createChat()'>Create & Start</button><button class='btn btn-o' onclick='document.getElementById(\"newChatModal\").classList.remove(\"show\")'>Cancel</button></div></div></div>");
    b.push_str("<script>function renConv(id){var nn=prompt('Rename conversation:');if(!nn)return;fetch('/api/v1/conversations/'+id,{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:nn})}).then(r=>r.json()).then(function(d){if(d.code===0){location.reload()}else{alert(d.message||'Error')}})};function delConv(id){if(!confirm('Delete this conversation?'))return;fetch('/api/v1/conversations/'+id,{method:'DELETE'}).then(function(){location.reload()})};function shareConv(id){var u=location.origin+'/chat/'+id;navigator.clipboard.writeText(u).then(function(){alert('Link copied: '+u)}).catch(function(){prompt('Share link:',u)})};function newChat(){var sel=document.getElementById('chatKbs');sel.innerHTML='<option value=\'\'>Loading…</option>';fetch('/api/v1/datasets').then(function(r){return r.json()}).then(function(d){var kbs=d.data||[];if(!kbs.length){sel.innerHTML='<option value=\'\'>No knowledge bases</option>';return}sel.innerHTML=kbs.map(function(k){return '<option value=\''+k.id+'\'>'+k.name+'</option>'}).join('')}).catch(function(){sel.innerHTML='<option value=\'\'>Load failed</option>'});document.getElementById('newChatModal').classList.add('show')};function createChat(){var n=document.getElementById('chatName').value||'New Chat';var kbs=[];[...document.querySelectorAll('#chatKbs option:checked')].forEach(function(o){if(o.value)kbs.push(o.value)});fetch('/api/v1/chats',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:n,kb_ids:kbs})}).then(r=>r.json()).then(d=>{var id=d.data?d.data.id:d.id;location.href='/chat/'+id})}</script>");
    Html(layout(t("Chat", zh), &b, zh))
}

pub async fn chat_detail_page(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let conv = state.conversations.get(&id);
    let name = conv
        .as_ref()
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "New Chat".into());
    let conv_kb_ids = conv
        .as_ref()
        .map(|c| c.kb_ids.join(","))
        .unwrap_or_default();
    let mut msgs = String::new();
    if let Some(c) = &conv {
        for (i, m) in c.messages.iter().enumerate() {
            if m.role == "user" {
                msgs.push_str(&format!("<div class='cmu'>{}</div>", m.content));
            } else {
                let like_active = if m.thumbup == Some(true) {
                    " style='color:var(--s)'"
                } else {
                    ""
                };
                let dislike_active = if m.thumbup == Some(false) {
                    " style='color:var(--d)'"
                } else {
                    ""
                };
                msgs.push_str(&format!("<div class='cma' id='msg{}'>{}<div class='msg-actions'><button class='btn btn-ghost btn-sm' onclick=\"copyMsg({})\" title='Copy'>📋</button><button class='btn btn-ghost btn-sm' onclick=\"regenMsg({},'{}')\" title='Regenerate'>🔄</button><button class='btn btn-ghost btn-sm' onclick=\"likeMsg('{}',1,this)\" title='Like'{}>👍</button><button class='btn btn-ghost btn-sm' onclick=\"likeMsg('{}',0,this)\" title='Dislike'{}>👎</button></div></div>",i,m.content,i,i,m.id,m.id,like_active,m.id,dislike_active));
            }
        }
    }
    if msgs.is_empty() {
        msgs = "<p style='color:var(--tm);text-align:center;padding:60px'>Start chatting below</p>"
            .into();
    }
    let mut b = String::new();
    b.push_str("<div style='display:flex;gap:16px;min-height:550px'>");
    // Left sidebar: back + conversation info + settings
    b.push_str("<div class='card' style='width:240px;flex-shrink:0'><div style='display:flex;gap:8px;flex-wrap:wrap'><a href='/chat' style='font-size:13px;color:var(--p);text-decoration:none'>← Back</a><button class='btn btn-o btn-sm' style='font-size:11px;padding:2px 8px' onclick='renConv()'>✏️ Rename</button><button class='btn btn-o btn-sm' style='font-size:11px;padding:2px 8px' onclick='shareConv()'>🔗 Share</button><a class='btn btn-o btn-sm' style='font-size:11px;padding:2px 8px' href='/api/v1/conversations/");
    b.push_str(&id);
    b.push_str("/export' target='_blank'>Export</a><button class='btn btn-d btn-sm' style='font-size:11px;padding:2px 8px' onclick='clearConv()'>Clear</button><button class='btn btn-o btn-sm' style='font-size:11px;padding:2px 8px' onclick='tempConv()'>💬 临时对话</button></div><h3 style='margin-top:12px'>");
    b.push_str(&name);
    b.push_str("</h3><p style='font-size:11px;color:var(--tm)'>ID: ");
    let id_short: String = id.chars().take(8).collect();
    b.push_str(&id_short);
    b.push_str("...</p>");
    // Knowledge bases attached to this conversation (RAGFlow sidebar parity)
    b.push_str("<div style='margin-top:12px'><h3 style='font-size:13px;margin-bottom:8px'>Knowledge Bases</h3>");
    if conv_kb_ids.is_empty() {
        b.push_str("<p style='font-size:11px;color:var(--tm)'>No knowledge base attached. Use the temporary chat to pick KBs.</p>");
    } else {
        for kb_id in conv_kb_ids.split(',') {
            if kb_id.trim().is_empty() {
                continue;
            }
            let kb_name = state
                .kbs
                .get(kb_id.trim())
                .map(|kb| kb.name.clone())
                .unwrap_or_else(|| kb_id.trim().to_string());
            b.push_str(&format!(
                "<div style='display:flex;align-items:center;gap:6px;padding:4px 8px;background:var(--bg);border-radius:6px;margin-bottom:4px;font-size:12px'>📚 {}</div>",
                kb_name
            ));
        }
    }
    b.push_str("</div>");
    // Settings panel (RAGFlow-style)
    b.push_str("<script>function renConv(){var nn=prompt('Rename conversation:',document.getElementById('convName').textContent);if(!nn)return;fetch('/api/v1/conversations/'+document.getElementById('convId').value,{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:nn})}).then(r=>r.json()).then(d=>{if(d.code===0){location.reload()}else{alert('Error: '+(d.message||'unknown'))}})};function shareConv(){var u=location.origin+location.pathname;navigator.clipboard.writeText(u).then(function(){alert('Link copied: '+u)}).catch(function(){prompt('Share link:',u)})};function clearConv(){if(!confirm('Clear all messages?'))return;fetch('/api/v1/conversations/'+document.getElementById('convId').value+'/clear',{method:'POST'}).then(r=>r.json()).then(d=>{if(d.code===0){location.reload()}else{alert('Error: '+(d.message||'unknown'))}})};function tempConv(){document.getElementById('tempQ').value='';document.getElementById('tempOut').textContent='';document.getElementById('tempModal').classList.add('show')};function sendTemp(){var q=document.getElementById('tempQ').value;if(!q)return;var kbs=(document.getElementById('tempKbs').value||'').split(',').filter(Boolean);document.getElementById('tempOut').textContent='Thinking...';fetch('/api/v1/completions',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:q,kb_ids:kbs})}).then(r=>r.json()).then(d=>{document.getElementById('tempOut').textContent=d.code===0?(d.data&&d.data.answer?d.data.answer:'ok'):('Error: '+(d.message||'unknown'))})}</script><input type='hidden' id='convId' value='");
    b.push_str(&id);
    b.push_str("'><input type='hidden' id='tempKbs' value='");
    b.push_str(&conv_kb_ids);
    b.push_str("'><span id='convName' style='display:none'>");
    b.push_str(&name);
    b.push_str("</span>");
    b.push_str("<div style='margin-top:20px;border-top:1px solid var(--bd);padding-top:16px'><h3 style='font-size:13px;margin-bottom:8px'>Chat Settings</h3>");
    b.push_str("<label style='font-size:11px;display:block;margin:6px 0'>Model</label><select id='chatModel'><option>MiniMax-M3</option><option>Qwen3.5-9B</option><option>GPT-4</option></select>");
    b.push_str("<label style='font-size:11px;display:block;margin:6px 0'>Temperature</label><input type='range' min='0' max='2' step='0.1' value='0.7' id='chatTemp' style='padding:0'><span id='tempVal' style='font-size:11px;color:var(--tm)'>0.7</span>");
    b.push_str("<label style='font-size:11px;display:block;margin:6px 0'>Max Tokens</label><select id='chatMaxTok'><option>512</option><option selected>1024</option><option>2048</option><option>4096</option></select>");
    b.push_str("<label style='font-size:11px;display:block;margin:6px 0'>System Prompt</label><textarea id='chatSys' rows='3' style='font-size:12px' placeholder='You are a helpful assistant...'>You are a helpful AI assistant. Use the context to answer questions.</textarea>");
    b.push_str("<label style='font-size:11px;display:flex;align-items:center;gap:4px;margin-top:8px'><input type='checkbox' checked> Show citations</label>");
    b.push_str("<label style='font-size:11px;display:flex;align-items:center;gap:4px;margin-top:4px'><input type='checkbox'> Stream response</label>");
    b.push_str("</div></div>");
    // Temporary conversation (RAGFlow: 临时对话) — stateless scratch chat
    b.push_str("<div class='modal-overlay' id='tempModal'><div class='modal' style='max-width:640px'><h2>💬 临时对话</h2><p style='color:var(--tm);font-size:12px;margin-bottom:12px'>不保存到会话历史</p><input id='tempQ' placeholder='Ask...' style='width:100%'><button class='btn btn-p' style='margin-top:8px;width:100%' onclick='sendTemp()'>Send</button><div id='tempOut' style='margin-top:12px;white-space:pre-wrap;line-height:1.6;font-size:13px'></div><button class='btn btn-o' style='margin-top:12px' onclick=document.getElementById('tempModal').classList.remove('show')>Close</button></div></div>");
    // Chat area
    b.push_str("<div class='card' style='flex:1;display:flex;flex-direction:column'><div style='flex:1;overflow-y:auto;padding:12px;background:var(--bg);border-radius:var(--r);margin-bottom:12px;min-height:400px' id='chatMsgs'>");
    b.push_str(&msgs);
    b.push_str("</div><div class='sb'><select id='chatModel' onchange='window.chatModel=this.value' style='margin-right:8px;max-width:180px'><option value=''>Model: default</option></select><input id='chatInput' data-testid='chat-textarea' placeholder='Ask anything...' onkeydown='if(event.key===&quot;Enter&quot;)sendMsg()'><button class='btn btn-o' id='thinkBtn' data-testid='chat-detail-thinking-toggle' onclick='toggleThink()' title='Thinking'>🤔</button><button class='btn btn-p' data-testid='chat-detail-send' onclick='sendMsg()'>");
    b.push_str(t("Send", zh));
    b.push_str("</button><span id='streamStatus' data-testid='chat-stream-status' style='display:none;color:var(--p);font-size:12px'>● Streaming</span><button class='btn btn-d' id='stopBtn' data-testid='chat-detail-stop' onclick='stopGen()' style='display:none'>⏹ ");
    b.push_str(t("Stop", zh));
    b.push_str("</button></div></div></div>");
    b.push_str("<style>.msg-actions{display:flex;gap:2px;margin-top:8px;opacity:0;transition:opacity .2s}.cma:hover .msg-actions{opacity:1}.btn-ghost{padding:2px 6px;font-size:14px;background:transparent;border:0;cursor:pointer;color:var(--tm)}.btn-ghost:hover{color:var(--t);background:var(--bgch)}</style>");
    b.push_str("<script>var sid='");
    b.push_str(&id);
    b.push_str("';var msgIdx=");
    b.push_str(
        &conv
            .as_ref()
            .map(|c| c.messages.len())
            .unwrap_or(0)
            .to_string(),
    );
    b.push(';');
    b.push_str("document.getElementById('chatTemp').oninput=function(){document.getElementById('tempVal').textContent=this.value};");
    b.push_str("fetch('/api/v1/providers').then(r=>r.json()).then(function(d){var ps=d.data||[];var sel=document.getElementById('chatModel');if(!sel)return;(ps||[]).forEach(function(p){if(!p.models)return;(p.models||[]).forEach(function(m){var o=document.createElement('option');o.value=p.id+':'+m;o.textContent=m;sel.appendChild(o)})})}).catch(function(){});function toggleThink(){window.__thinking=!window.__thinking;var b=document.getElementById('thinkBtn');if(b)b.style.borderColor=window.__thinking?'var(--p)':''}function stopGen(){if(window.__ac){window.__ac.abort();document.getElementById('stopBtn').style.display='none';var ss2=document.getElementById('streamStatus');if(ss2)ss2.style.display='none'}};function sendMsg(){var q=document.getElementById('chatInput').value;if(!q)return;var m=document.getElementById('chatMsgs');m.innerHTML+='<div class=cmu>'+q+'</div>';document.getElementById('chatInput').value='';var mi=msgIdx;var box=m.innerHTML+='';var ac=new AbortController();window.__ac=ac;var st=document.getElementById('stopBtn');if(st)st.style.display='inline-block';var ss0=document.getElementById('streamStatus');if(ss0)ss0.style.display='inline';fetch('/api/v1/chats/'+sid+'/completions',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:q,stream:true,reasoning:window.__thinking===true}),signal:ac.signal}).then(function(r){var reader=r.body.getReader();var dec=new TextDecoder();var buf='';var acc='';var mid='';var refs=[];var el=document.createElement('div');el.className='cma';el.id='msg'+mi;el.innerHTML='<span style=color:var(--tm)>🤔 思考中…</span>';m.appendChild(el);m.scrollTop=m.scrollHeight;function pump(){return reader.read().then(function(x){if(x.done){if(st)st.style.display='none';var ss1=document.getElementById('streamStatus');if(ss1)ss1.style.display='none';el.innerHTML+=refsHtml(refs);el.innerHTML+='<div class=msg-actions><button class=btn-ghost btn-sm onclick=copyMsg('+mi+') title=Copy>📋</button><button class=btn-ghost btn-sm onclick=regenMsg('+mi+',&quot;'+mid+'&quot;) title=Regenerate>🔄</button><button class=btn-ghost btn-sm onclick=likeMsg(&quot;'+mid+'&quot;,1,this) title=Like>👍</button><button class=btn-ghost btn-sm onclick=likeMsg(&quot;'+mid+'&quot;,0,this) title=Dislike>👎</button></div></div>';m.scrollTop=m.scrollHeight;msgIdx++;return}buf+=dec.decode(x.value,{stream:true});var lines=buf.split('\\n');buf=lines.pop();lines.forEach(function(line){if(line.indexOf('data:')!==0)return;var data=line.slice(5).trim();if(data==='[DONE]')return;try{var ev=JSON.parse(data);if(ev.object==='chat.completion.references'){refs=ev.references||refs;return}var c=ev.choices&&ev.choices[0]&&ev.choices[0].delta&&ev.choices[0].delta.content||'';if(c){if(acc===''){el.innerHTML=''}acc+=c;el.innerHTML=acc;m.scrollTop=m.scrollHeight}}catch(e){}});return pump()})}function refsHtml(refs){if(!refs||!refs.length)return '';var h='<details class=refs style=margin-top:6px><summary style=cursor:pointer;font-size:11px;color:var(--p)>📎 '+refs.length+' references</summary><div style=margin-top:6px>';refs.forEach(function(r,i){h+='<div class=cc>['+(i+1)+'] '+(r.content||'').substring(0,120)+(r.score?' <span style=color:var(--tm)>score '+(r.score).toFixed(4)+'</span>':'')+'</div>'});h+='</div></details>';return h}return pump()}).catch(function(e){if(st)st.style.display='none';if(e&&e.name==='AbortError'){el.innerHTML+='<span style=color:var(--tm)>⏹ Stopped</span>'}else{el.innerHTML='<span style=color:var(--err)>Error</span>'}})}");
    b.push_str("function copyMsg(i){var el=document.getElementById('msg'+i);if(el){var t=el.innerText.replace(/📋🔄👍👎/g,'').trim();navigator.clipboard.writeText(t);alert('Copied!')}}");
    b.push_str("function regenMsg(i,mid){var el=document.getElementById('msg'+i);if(!el||!mid)return;var old=el.innerHTML;el.innerHTML='<p style=color:var(--tm)>Regenerating...</p>';fetch('/api/v1/chats/'+sid+'/messages/'+mid+'/regenerate',{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'}).then(async function(r){var d=await r.json();if(!r.ok)throw new Error(d.message||'Regenerate failed');var a=d.data?d.data.answer:'...';var refs=(d.data&&d.data.citations||[]).map(function(c){return {content:c}});var rh=refsHtml(refs);el.innerHTML=a+rh+'<div class=msg-actions><button class=btn-ghost btn-sm onclick=copyMsg('+i+') title=Copy>📋</button><button class=btn-ghost btn-sm onclick=regenMsg('+i+',&quot;'+mid+'&quot;) title=Regenerate>🔄</button><button class=btn-ghost btn-sm onclick=likeMsg(&quot;'+mid+'&quot;,1,this) title=Like>👍</button><button class=btn-ghost btn-sm onclick=likeMsg(&quot;'+mid+'&quot;,0,this) title=Dislike>👎</button></div>'}).catch(function(e){el.innerHTML=old;alert(e.message)})}");
    b.push_str("function likeMsg(mid,v,btn){if(!mid)return;fetch('/api/v1/chats/'+sid+'/messages/'+mid+'/feedback',{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({thumbup:!!v})}).then(async function(r){var d=await r.json();if(!r.ok)throw new Error(d.message||'Feedback failed');var actions=btn.parentElement;actions.querySelectorAll('button').forEach(function(b){b.style.color=''});btn.style.color=v?'var(--s)':'var(--d)'}).catch(function(e){alert(e.message)})}");
    b.push_str("</script>");
    Html(layout(&name, &b, zh))
}

/// Upstream next-search hooks.ts parity: the search page sidebar carries a
/// rerank `ModelTreeSelect` (shared tree selector) revealed when the rerank
/// switch is enabled; the search-app runtime seeds it via `__SAPP__`.
const SEARCH_PAGE_RERANK_SELECTOR_FIELD: DefaultModelField = DefaultModelField {
    field_id: "rerank_id",
    dom_id: "searchRerankSel",
    label: "Model",
    update_type: "rerank_id",
    response_type: "rerank",
    accepted_types: &["rerank"],
    required: false,
    tooltip: "",
    placeholder: "Please select",
};

/// Upstream next-search `search-setting.tsx` keeps every drawer value in
/// react-hook-form state; only `Save`/`Run` consume it. The shared tree body
/// persists tenant defaults through `PATCH /api/v1/models/default`, so pages
/// whose selector is purely form-local (retrieval testing, `/search`) redeclare
/// the hook after the shared body to shadow it.
const FORM_LOCAL_SAVE_MODEL_DEFAULT: &str = "async function saveModelDefault(type,nextValue){var field=modelField(type);if(!field)return;var trigger=document.getElementById(field.domId);if(!trigger)return;var state=defaultModelState(field);if(state.pending)return;var value=String(nextValue==null?'':nextValue).trim();if(value&&!defaultModelOption(field,value))return;var previous=trigger.dataset.savedValue||'';trigger.dataset.value=value;if(!value){trigger.dataset.savedValue='';renderDefaultModelTrigger(field);return}var parsed=parseDefaultModelValue(value);if(!parsed){trigger.dataset.value=previous;trigger.dataset.savedValue=previous;renderDefaultModelTrigger(field);return}trigger.dataset.savedValue=value;renderDefaultModelTrigger(field)}";

pub async fn search_page(
    State(state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let chunks = state.engine.read().unwrap().len();
    let mut b = String::new();
    // Upstream next-search hooks.ts parity: `/search?sapp={id}` loads the
    // search app and its config drives the retrieval call.
    let sapp_id: Option<String> = req.uri().query().and_then(|query| {
        query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == "sapp").then(|| value.to_string())
        })
    });
    let sapp = sapp_id
        .as_ref()
        .and_then(|id| state.search_apps.as_ref().and_then(|store| store.get(id)));
    if let Some(app) = &sapp {
        let seed = json_script_value(&serde_json::json!({
            "id": app.id,
            "name": app.name,
            "kb_ids": app.kb_ids,
            "hybrid": app.hybrid,
            "rerank": app.rerank,
            "rerank_id": app.rerank_id,
            "top_k": app.top_k,
            "similarity_threshold": app.similarity_threshold,
            "vector_similarity_weight": app.vector_similarity_weight,
        }));
        b.push_str("<script>window.__SAPP__=");
        b.push_str(&seed);
        b.push_str(";</script>");
    }
    // RAGFlow-style: centered search home
    b.push_str("<div class='search-home' id='searchHome'><h1>🐠 RayRAG Search</h1><p>");
    b.push_str(if zh {
        "👋 欢迎回来！输入你的问题开始搜索"
    } else {
        "👋 Hi there! Enter your question to search"
    });
    b.push_str("</p><div class='sb'><input id='query' placeholder='");
    b.push_str(if zh {
        "搜索知识库..."
    } else {
        "Search knowledge base..."
    });
    b.push_str("' style='font-size:16px;padding:14px 20px' onkeydown='if(event.key===&quot;Enter&quot;)doSearch()'><button class='btn btn-p' onclick='doSearch()' style='padding:14px 24px'>Search</button></div><p style='font-size:12px;color:var(--tm);margin-top:12px'>Index: ");
    b.push_str(&chunks.to_string());
    b.push_str(" chunks</p></div>");
    // Search results (hidden initially)
    b.push_str("<div id='searchView' style='display:none'><div style='display:flex;gap:16px'><div style='flex:1'><div class='card'><div class='sb'><input id='q2' placeholder='Search...' onkeydown='if(event.key===&quot;Enter&quot;)doSearch2()'><button class='btn btn-p' onclick='doSearch2()'>Search</button></div><div style='display:flex;gap:12px;margin-top:12px;align-items:center;flex-wrap:wrap'><label style='font-size:12px;display:flex;align-items:center;gap:4px'><input type='checkbox' id='rerank' onchange='searchRerankToggle()'> Re-rank</label><label style='font-size:12px;display:flex;align-items:center;gap:4px'><input type='checkbox' id='hybrid'> BM25+Vector</label><label style='font-size:12px'>");
    b.push_str(t("Knowledge Base", zh));
    b.push_str("</label><select id='kbSel' style='font-size:12px;max-width:200px' onchange='kbSelChange()'><option value='__all__'>All knowledge bases</option></select><label style='font-size:12px'>");
    b.push_str(t("Document Type", zh));
    b.push_str("</label><select id='docType' style='font-size:12px'><option value=''>All types</option><option value='pdf'>PDF</option><option value='doc'>DOC</option><option value='docx'>DOCX</option><option value='xls'>XLS</option><option value='xlsx'>XLSX</option><option value='ppt'>PPT</option><option value='pptx'>PPTX</option><option value='txt'>TXT</option><option value='md'>MD</option><option value='html'>HTML</option></select></div><div id='searchRerankRow' style='display:none;margin-top:8px'>");
    push_default_model_selector(&mut b, &SEARCH_PAGE_RERANK_SELECTOR_FIELD, zh);
    b.push_str("</div><div id='kbChips' style='margin-top:10px;display:flex;gap:6px;flex-wrap:wrap;font-size:12px'></div><button class='btn btn-p' style='font-size:12px;padding:4px 10px' onclick='saveApp()'>💾 Save as App</button><button class='btn btn-o' style='font-size:12px;padding:4px 10px;margin-left:auto' onclick='exportResults()'>⬇ Export MD</button></div></div><div id='results' style='margin-top:16px'></div></div>");
    // Settings sidebar
    b.push_str("<div class='card' style='width:240px;flex-shrink:0'><h3>Search Settings</h3><label style='font-size:12px;display:block;margin:8px 0'>");
    b.push_str("Top K");
    b.push_str("</label><select id='topK'><option>5</option><option selected>10</option><option>20</option><option>50</option></select><label style='font-size:12px;display:block;margin:8px 0'>");
    b.push_str(t("Threshold", zh));
    b.push_str("</label><select id='thresh'><option>0.0</option><option selected>0.3</option><option>0.5</option><option>0.7</option></select><label style='font-size:12px;display:block;margin:8px 0'>");
    b.push_str(t("Vector Weight", zh));
    b.push_str("</label><select id='vWeight'><option>0.3</option><option>0.5</option><option selected>0.7</option><option>1.0</option></select><h3 style='margin-top:20px'>Quick Questions</h3><a href='#' onclick='quickQ(\"What is RayRAG?\")' style='display:block;font-size:12px;color:var(--p);text-decoration:none;padding:4px 0'>What is RayRAG?</a><a href='#' onclick='quickQ(\"How does RAG work?\")' style='display:block;font-size:12px;color:var(--p);text-decoration:none;padding:4px 0'>How does RAG work?</a></div></div></div>");
    b.push_str("<script>function saveApp(){var n=prompt('Search app name:');if(!n)return;var kbs=[];[...document.getElementById('kbSel').selectedOptions].forEach(function(o){if(o.value)kbs.push(o.value)});var body={name:n,kb_ids:kbs,hybrid:document.getElementById('hybrid').checked,rerank:document.getElementById('rerank').checked};fetch('/api/v1/searchapps',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(r=>r.json()).then(function(d){if(d.code===0){alert('Search app saved: '+n)}else{alert('Save failed: '+(d.message||'unknown'))}})};var lastResults=[];var lastQuery='';function loadKbs(){fetch('/api/v1/datasets').then(r=>r.json()).then(d=>{var list=d.data||[];var chips='<label style=display:flex;align-items:center;gap:4px><input type=checkbox class=kbCk value=__all__ checked onchange=kbChipChange(this)> All</label>';var opts='<option value=__all__>All knowledge bases</option>';list.forEach(function(k){chips+='<label style=display:flex;align-items:center;gap:4px><input type=checkbox class=kbCk value='+k.id+' onchange=kbChipChange(this)> '+k.name+'</label>';opts+='<option value='+k.id+'>'+k.name+'</option>'});document.getElementById('kbChips').innerHTML=chips;document.getElementById('kbSel').innerHTML=opts;applySapp()})};function kbChipChange(c){if(c.value==='__all__'&&c.checked){document.querySelectorAll('.kbCk').forEach(function(x){x.checked=(x.value==='__all__')})}else{document.querySelectorAll('.kbCk').forEach(function(x){if(x.value==='__all__')x.checked=false})}};function kbSelChange(){var v=document.getElementById('kbSel').value;document.querySelectorAll('.kbCk').forEach(function(x){x.checked=(v==='__all__')?(x.value==='__all__'):(x.value===v)})};function applySapp(){var s=window.__SAPP__;if(!s)return;var cks=document.querySelectorAll('.kbCk');cks.forEach(function(c){c.checked=(s.kb_ids||[]).indexOf(c.value)>=0});if(!(s.kb_ids||[]).length){cks.forEach(function(c){if(c.value==='__all__')c.checked=true})}document.getElementById('rerank').checked=!!s.rerank;document.getElementById('hybrid').checked=!!s.hybrid;searchRerankToggle();var tk=document.getElementById('topK');var has=false;[...tk.options].forEach(function(o){if(String(o.value)===String(s.top_k))has=true});if(!has){var o=document.createElement('option');o.value=s.top_k;o.textContent=s.top_k;tk.appendChild(o)}tk.value=String(s.top_k);var th=document.getElementById('thresh');var thHas=false;[...th.options].forEach(function(o){if(String(o.value)===String(s.similarity_threshold))thHas=true});if(!thHas){var o=document.createElement('option');o.value=s.similarity_threshold;o.textContent=s.similarity_threshold;th.appendChild(o)}th.value=String(s.similarity_threshold);var vw=document.getElementById('vWeight');var vwHas=false;[...vw.options].forEach(function(o){if(String(o.value)===String(s.vector_similarity_weight))vwHas=true});if(!vwHas){var o=document.createElement('option');o.value=s.vector_similarity_weight;o.textContent=s.vector_similarity_weight;vw.appendChild(o)}vw.value=String(s.vector_similarity_weight)};loadKbs();function hl(t,q){if(!q)return t;try{return t.replace(new RegExp(q.replace(/[.*+?^${}()|[\\\\]\\\\]/g,'\\\\$&'),'gi'),function(m){return '<mark>'+m+'</mark>'})}catch(e){return t}};function doSearch(){var q=document.getElementById('query').value;if(!q)return;document.getElementById('searchHome').style.display='none';document.getElementById('searchView').style.display='block';document.getElementById('q2').value=q;runSearch(q)};function doSearch2(){runSearch(document.getElementById('q2').value)};function quickQ(q){document.getElementById('searchHome').style.display='none';document.getElementById('searchView').style.display='block';document.getElementById('q2').value=q;runSearch(q)};function searchPage(d){window.sPage+=d;runSearch(lastQuery)};function docTypeOk(c){var t=document.getElementById('docType').value;if(!t)return true;var n=(c.doc_name||c.document_name||'').toLowerCase();return n.indexOf('.'+t)>0};function runSearch(q){var r=document.getElementById('rerank').checked;var h=document.getElementById('hybrid').checked;var k=parseInt(document.getElementById('topK').value);var t=parseFloat(document.getElementById('thresh').value);var e=r?'/api/v1/retrieval':'/api/v1/search';var body={question:q,top_k:k};var kbs=[];var allOn=false;document.querySelectorAll('.kbCk:checked').forEach(function(c){if(c.value==='__all__'){allOn=true}else{kbs.push(c.value)}});if(kbs.length&&!allOn){body.kb_ids=kbs}var rrEl=document.getElementById('searchRerankSel');var rr=rrEl?(rrEl.dataset.value||''):'';if(r&&rr){body.rerank_id=rr}body.similarity_threshold=t;body.vector_similarity_weight=parseFloat(document.getElementById('vWeight').value);if(!h){body.vector_similarity_weight=0};document.getElementById('results').innerHTML='<p style=color:var(--tm);padding:24px>Searching...</p>';fetch(e,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(r=>r.json()).then(d=>{var items=((d.data||{}).chunks||(d.data||{}).results||[]).filter(docTypeOk);var h='';var gc=(d.data&&d.data.graph_context)||[];items.forEach(function(c,i){c=c.chunk||c;var content=(c.content||c.text||'').substring(0,500);var fullContent=(c.content||c.text||'');var docName=(c.doc_name||c.document_name||'Unknown');var score=(c.similarity||c.score||0).toFixed(4);h+='<div class=ri><span class=rs>#'+(i+1)+' &middot; '+score+(r?' &middot; reranked':'')+'</span><div class=rc>'+hl(content,q)+'</div><div class=rm>'+docName+' &middot; '+(c.chunk_id||c.id||'')+' <a href=# onclick=\\\"previewDoc(\\''+docName.replace(/'/g,'')+'\\',\\''+fullContent.replace(/'/g,'').substring(0,2000)+'\\')\\\" style=\\\"color:var(--p);font-size:11px;margin-left:8px\\\">📄 Preview</a></div></div>'});if(gc&&gc.length){h+='<div class=card style=margin-top:16px><h3>🔗 Graph Context</h3>'+gc.map(function(g,i){return '<div class=cc>['+(i+1)+'] '+g.substring(0,120)+'</div>'}).join('')+'</div>'}lastResults=items;lastQuery=q;document.getElementById('results').innerHTML=h||'<p style=color:var(--tm);padding:24px>No results found.</p>'})};function exportResults(){if(!lastResults.length){alert('No results to export');return}var md='# Search: '+lastQuery+'\\n\\n';lastResults.forEach(function(c,i){c=c.chunk||c;md+='## '+(i+1)+'. '+(c.doc_name||'Unknown')+' (score '+(c.similarity||c.score||0).toFixed(4)+')\\n\\n'+(c.content||c.text||'')+'\\n\\n'});var blob=new Blob([md],{type:'text/markdown'});var a=document.createElement('a');a.href=URL.createObjectURL(blob);a.download='search-export.md';a.click()};function previewDoc(name,content){var m=document.getElementById('docPreview');document.getElementById('docPreviewTitle').textContent=name;document.getElementById('docPreviewContent').textContent=content;m.classList.add('show')};function closePreview(){document.getElementById('docPreview').classList.remove('show')}</script>");
    b.push_str("<script>const MODEL_FIELDS=");
    b.push_str(&json_script_value(&serde_json::json!([{
        "fieldId": "rerank_id",
        "domId": "searchRerankSel",
        "updateType": "rerank_id",
        "responseType": "rerank",
        "acceptedTypes": ["rerank"],
        "required": false,
        "placeholder": "Please select"
    }])));
    b.push_str(DEFAULT_MODEL_TREE_JS_HEAD);
    b.push_str(DEFAULT_MODEL_TREE_JS_BODY);
    b.push_str("function seedModelTrigger(type,value){var field=modelField(type);if(!field)return;var trigger=document.getElementById(field.domId);if(!trigger)return;trigger.dataset.value=String(value||'');trigger.dataset.savedValue=String(value||'');renderDefaultModelTrigger(field)}");
    // Upstream `/search` keeps its settings drawer in react-hook-form state: the
    // picked rerank model only rides the retrieval request (`search_config
    // .rerank_id`). Shadow the shared tenant-default persist hook.
    b.push_str(FORM_LOCAL_SAVE_MODEL_DEFAULT);
    b.push_str("function searchRerankToggle(){var on=document.getElementById('rerank').checked;var row=document.getElementById('searchRerankRow');if(row)row.style.display=on?'':'none'}");
    b.push_str("async function loadSearchPageModelCatalog(){try{var response=await fetch('/api/v1/models');var data=await response.json();if(!response.ok||data.code!==0)return;DEFAULT_MODEL_CATALOG=(data.data||[]).map(normalizeDefaultModel).filter(function(model){return model.selector});MODEL_FIELDS.forEach(function(field){var state=defaultModelState(field);state.expanded.clear();expandDefaultModelTree(field);renderDefaultModelTrigger(field);renderDefaultModelTree(field)})}catch(error){}}");
    b.push_str("loadSearchPageModelCatalog().then(function(){var s=window.__SAPP__;if(s&&s.rerank_id)seedModelTrigger('rerank_id',s.rerank_id);searchRerankToggle();});");
    b.push_str("</script>");
    // Document preview modal
    b.push_str("<div class='modal-overlay' id='docPreview' onclick='if(event.target===this)closePreview()'><div class='modal' style='max-width:700px;max-height:80vh;overflow-y:auto'><div style='display:flex;align-items:center;margin-bottom:16px'><h2 id='docPreviewTitle' style='flex:1;margin:0'>Document</h2><button class='btn btn-ghost' onclick='closePreview()'>×</button></div><div id='docPreviewContent' style='font-size:14px;line-height:1.8;color:var(--t2);white-space:pre-wrap'></div></div></div>");
    Html(layout(t("Search", zh), &b, zh))
}

pub async fn agent_detail_page(
    axum::extract::Path(agent_id): axum::extract::Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let mut b = String::new();
    b.push_str("<div style='display:flex;align-items:center;gap:12px;flex-wrap:wrap'><a class='btn btn-o' href='/agents'>← ");
    b.push_str(t("Back to Agents", zh));
    b.push_str("</a><h1 style='margin:0;flex:1' id='agentTitle'>🤖 ...</h1><button class='btn btn-p' onclick='runAgent()'>▶ ");
    b.push_str(t("Run", zh));
    b.push_str("</button><button class='btn btn-o' onclick='saveAgent()'>💾 ");
    b.push_str(t("Save", zh));
    b.push_str("</button><button class='btn btn-p' onclick='publishAgent()'>🚀 ");
    b.push_str(t("Publish", zh));
    b.push_str("</button><button class='btn btn-o' onclick='openVersions()'>📚 ");
    b.push_str(t("Versions", zh));
    b.push_str("</button><button class='btn btn-o' data-testid='agent-launch' onclick='openChatTest()'>🚀 ");
    b.push_str(t("Launch", zh));
    b.push_str("</button><button class='btn btn-o' data-testid='agent-management' onclick='openVersions()'>⚙ ");
    b.push_str(t("Management", zh));
    b.push_str("</button><span id='autosaveSt' style='color:var(--tm);font-size:12px;margin-left:auto'>Autosaved —</span>");
    b.push_str("</button><button class='btn btn-o' onclick='exportAgentJson()'>⬇ ");
    b.push_str(t("Export JSON", zh));
    b.push_str("</button><button class='btn btn-d' onclick='delAgent()'>");
    b.push_str(t("Delete", zh));
    b.push_str("</button></div>");
    b.push_str("<div class='modal-overlay' id='verModal'><div class='modal' style='max-width:680px'><h2>📚 ");
    b.push_str(t("Versions", zh));
    b.push_str("</h2><div id='verList'>Loading...</div><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-o' onclick=document.getElementById('verModal').classList.remove('show')>");
    b.push_str(t("Close", zh));
    b.push_str("</button></div></div></div>");
    b.push_str("<div class='modal-overlay' id='pubModal'><div class='modal' style='max-width:480px'><h2>🚀 Confirm publish</h2><p style='color:var(--tm);font-size:13px'>");
    b.push_str(t("You are about to publish this agent", zh));
    b.push_str("</p><div style='display:flex;justify-content:flex-end;gap:8px;margin-top:16px'><button class='btn btn-o' onclick=document.getElementById('pubModal').classList.remove('show')>");
    b.push_str(t("Cancel", zh));
    b.push_str("</button><button class='btn btn-p' onclick='confirmPublish()'>");
    b.push_str(t("Confirm", zh));
    b.push_str("</button></div></div></div>");
    b.push_str("<div class='modal-overlay' id='chatTestModal'><div class='modal' style='max-width:560px'><h2>💬 ");
    b.push_str(t("Chat", zh));
    b.push_str("</h2><div class='card' style='max-height:220px;overflow:auto;margin-bottom:10px;font-size:13px' id='testOut'>");
    b.push_str(t("Drag & drop files here", zh));
    b.push_str("</div><div class='sb'><input id='testQ' placeholder='Send message' onkeydown='if(event.key===&quot;Enter&quot;)sendTestMsg()'><button class='btn btn-p' onclick='sendTestMsg()'>");
    b.push_str(t("Send message", zh));
    b.push_str("</button></div><div style='display:flex;justify-content:flex-end;margin-top:10px'><button class='btn btn-o' onclick=document.getElementById('chatTestModal').classList.remove('show')>");
    b.push_str(t("Close", zh));
    b.push_str("</button></div></div></div>");
    b.push_str("<div style='display:flex;gap:6px;margin-top:10px;align-items:center'><button class='btn btn-o btn-sm' title='Zoom In' onclick='zoomCanvas(1)'>🔍+</button><button class='btn btn-o btn-sm' title='Zoom Out' onclick='zoomCanvas(-1)'>🔍−</button><button class='btn btn-o btn-sm' title='Fit View' onclick='fitCanvas()'>⛶</button><button class='btn btn-o btn-sm' title='Toggle Interactivity' onclick='toggleInter()'>✋</button><span style='color:var(--tm);font-size:12px'>Begin · Conversational</span></div>");
    b.push_str("<div class='st' style='margin-top:16px'><div class='sc'><span class='lbl'>ID</span><span class='val' id='agentId' style='font-size:13px'>-</span></div><div class='sc'><span class='lbl'>KBs</span><span class='val' id='agentKbs'>-</span></div><div class='sc'><span class='lbl'>Status</span><span class='val' id='agentStatus' style='font-size:15px'>-</span></div></div>");
    b.push_str("<div style='display:flex;gap:16px;margin-top:16px;align-items:flex-start'><div class='card' style='width:230px;flex-shrink:0'><h2>");
    b.push_str(t("Components", zh));
    b.push_str("</h2><div id='compPanel' style='max-height:420px;overflow:auto;font-size:12px'>");
    {
        let catalog = crate::runtime::component_catalog();
        let mut by_category: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
        for (name, category) in &catalog {
            by_category.entry(category).or_default().push(name);
        }
        for (category, names) in &by_category {
            b.push_str("<h4 style='margin:10px 0 4px;text-transform:capitalize;color:var(--tm)'>");
            b.push_str(category);
            b.push_str("</h4>");
            for name in names {
                b.push_str(&format!(
                    "<div style='display:flex;align-items:center;gap:6px;padding:3px 0'><button class='btn btn-o btn-sm' style='padding:2px 6px;font-size:11px' onclick='addComponent(&quot;{}&quot;)' >＋</button><span style='overflow:hidden;text-overflow:ellipsis;white-space:nowrap' title='{}'>{}</span></div>",
                    html_escape(name),
                    html_escape(name),
                    html_escape(name)
                ));
            }
        }
    }
    b.push_str("</div></div><div class='card' style='flex:1'><h2>");
    b.push_str(t("Canvas", zh));
    b.push_str("</h2><div id='canvasView' style='overflow:auto;border:1px solid var(--bd);border-radius:var(--r);padding:8px;min-height:260px'><p style='color:var(--tm)'>Loading topology...</p></div></div></div>");
    b.push_str("<div style='display:grid;grid-template-columns:1fr 1fr;gap:16px;margin-top:16px'><div class='card'><h2>");
    b.push_str(t("Canvas DSL", zh));
    b.push_str("</h2><button class='btn btn-o btn-sm' style='margin-bottom:8px' onclick='editDsl()'>✏️ Edit DSL</button> <button class='btn btn-p btn-sm' style='margin-bottom:8px' onclick='saveDsl()'>💾 Save</button><textarea id='agentDslEdit' style='display:none;width:100%;min-height:300px;font-family:monospace;font-size:12px' spellcheck='false'></textarea><pre id='agentDsl' style='background:var(--bg);padding:16px;border-radius:var(--r);font-size:12px;overflow:auto;max-height:480px;white-space:pre-wrap'>Loading...</pre></div><div class='card'><h2>");
    b.push_str(t("Test", zh));
    b.push_str("</h2><input id='aq2' placeholder='Ask...' style='width:100%'><button class='btn btn-p' style='margin-top:8px;width:100%' onclick='askAgent()'>");
    b.push_str(t("Ask", zh));
    b.push_str("</button><div id='aa2' style='margin-top:12px;background:var(--bg);padding:16px;border-radius:var(--r);min-height:120px;white-space:pre-wrap;line-height:1.6;font-size:13px'></div></div><div class='card'><h2>");
    b.push_str(t("Run Log", zh));
    b.push_str("</h2><div id='runLog' style='font-size:12px;color:var(--t2);max-height:300px;overflow:auto'><p style='color:var(--tm)'>No runs yet.</p></div></div></div>");
    b.push_str("<script>var aid='");
    b.push_str(&agent_id);
    b.push_str("';function editDsl(){var pre=document.getElementById('agentDsl');var ta=document.getElementById('agentDslEdit');ta.value=pre.textContent;ta.style.display='block';pre.style.display='none';ta.focus()};function saveDsl(){var ta=document.getElementById('agentDslEdit');var v;try{v=JSON.parse(ta.value)}catch(e){alert('Invalid JSON: '+e.message);return}fetch('/api/v1/agents/'+aid,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({dsl:v})}).then(r=>r.json()).then(function(d){if(d.code===0){loadAgent()}else{alert('Save failed: '+(d.message||'unknown'))}})};function loadAgent(){fetch('/api/v1/agents/'+aid).then(r=>r.json()).then(d=>{var a=d.data||{};document.getElementById('agentTitle').textContent='🤖 '+(a.name||'Agent');document.getElementById('agentId').textContent=a.id||'-';document.getElementById('agentKbs').textContent=(a.kb_ids||[]).join(', ')||'none';document.getElementById('agentStatus').textContent=a.canvas_category||'standard';document.getElementById('agentDsl').textContent=JSON.stringify(a.dsl||a.canvas||{},null,2);renderTopology(a.dsl||a.canvas||{})})};function renderTopology(dsl){var nodes=(dsl.nodes||[]);var edges=(dsl.edges||[]);if(!nodes.length){document.getElementById('canvasView').innerHTML='<p style=color:var(--tm)>Empty canvas</p>';return}var xMin=1e9,yMin=1e9,xMax=-1e9,yMax=-1e9;var pos={};nodes.forEach(function(n){var p=n.position||{x:0,y:0};pos[n.id]=p;xMin=Math.min(xMin,p.x);yMin=Math.min(yMin,p.y);xMax=Math.max(xMax,p.x);yMax=Math.max(yMax,p.y)});var W=xMax-xMin+300,H=yMax-yMin+300;var svg='<svg width='+W+' height='+H+' style=background:var(--bg)><defs><marker id=arr markerWidth=8 markerHeight=8 refX=8 refY=4 orient=auto><path d=M0,0L8,4L0,8 fill=var(--p)></path></marker></defs>';edges.forEach(function(e){var s=pos[e.source]||{x:0,y:0};var t=pos[e.target]||{x:0,y:0};svg+='<line x1='+(s.x-xMin+150)+' y1='+(s.y-yMin+150)+' x2='+(t.x-xMin+150)+' y2='+(t.y-yMin+150)+' stroke=var(--p) stroke-width=1.5 marker-end=url(#arr)></line>'});nodes.forEach(function(n){var p=pos[n.id];var label=n.data&&(n.data.title||n.data.name)||n.id;svg+='<g><rect x='+(p.x-xMin+120)+' y='+(p.y-yMin+130)+' width=150 height=40 rx=8 fill=var(--card) stroke=var(--p)></rect><text x='+(p.x-xMin+195)+' y='+(p.y-yMin+153)+' text-anchor=middle font-size=12 fill=var(--t2)>'+label+'</text></g>'});svg+='</svg>';document.getElementById('canvasView').innerHTML=svg};function logRun(q,status,out){var lg=document.getElementById('runLog');var ph=lg.querySelector('p');if(ph)ph.remove();var d=new Date();var ts=d.getHours()+':'+('0'+d.getMinutes()).slice(-2)+':'+('0'+d.getSeconds()).slice(-2);var div=document.createElement('div');div.style.cssText='padding:8px;border-bottom:1px solid var(--bd);white-space:pre-wrap';div.innerHTML='<span style=color:var(--tm)>['+ts+'] '+status+' &middot; Q: '+(q||'').substring(0,120)+'</span><br>'+String(out).substring(0,600);lg.insertBefore(div,lg.firstChild)};function runAgent(){document.getElementById('aa2').textContent='Running agent...';logRun('Run agent once','running','');fetch('/api/v1/agents/'+aid+'/completions',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:'Run agent once'})}).then(r=>r.json()).then(d=>{var out=d.data?d.data.answer:JSON.stringify(d);document.getElementById('aa2').textContent=out;logRun('Run agent once',d.code===0?'done':'error',out)})};function askAgent(){var q=document.getElementById('aq2').value;if(!q)return;document.getElementById('aa2').textContent='Running...';logRun(q,'running','');fetch('/api/v1/agents/'+aid+'/completions',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:q})}).then(r=>r.json()).then(d=>{var out=d.data?d.data.answer:JSON.stringify(d);document.getElementById('aa2').textContent=out;logRun(q,d.code===0?'done':'error',out)})};function saveAgent(){var ta=document.getElementById('agentDslEdit');var raw=ta.style.display==='none'?document.getElementById('agentDsl').textContent:ta.value;var v;try{v=JSON.parse(raw)}catch(e){alert('Invalid JSON: '+e.message);return}fetch('/api/v1/agents/'+aid,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({dsl:v})}).then(r=>r.json()).then(function(d){if(d.code===0){alert('Saved');loadAgent();updateAutosave()}else{alert('Save failed: '+(d.message||'unknown'))}})};function publishAgent(){document.getElementById('pubModal').classList.add('show')}function confirmPublish(){fetch('/api/v1/agents/'+aid,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({release:true})}).then(r=>r.json()).then(function(d){document.getElementById('pubModal').classList.remove('show');if(d.code===0){openChatTest()}else{alert('Publish failed: '+(d.message||'unknown'))}})}function openChatTest(){document.getElementById('chatTestModal').classList.add('show')}function sendTestMsg(){var q=document.getElementById('testQ').value;if(!q)return;document.getElementById('testOut').textContent='Thinking...';fetch('/api/v1/agents/'+aid+'/completions',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:q})}).then(r=>r.json()).then(function(d){document.getElementById('testOut').textContent=d.data?d.data.answer:JSON.stringify(d)})}function updateAutosave(){var el=document.getElementById('autosaveSt');if(el)el.textContent='Autosaved '+new Date().toLocaleString()}function zoomCanvas(dir){var p=document.getElementById('compPanel');var cur=parseFloat(p.dataset.z||'12');cur=Math.min(20,Math.max(9,cur+dir));p.dataset.z=cur;p.style.fontSize=cur+'px'}function fitCanvas(){var p=document.getElementById('compPanel');p.dataset.z='12';p.style.fontSize='12px'}function toggleInter(){var p=document.getElementById('compPanel');p.classList.toggle('no-click')}function addComponent(name){fetch('/api/v1/agents/'+aid).then(r=>r.json()).then(d=>{var a=d.data||{};var dsl=a.dsl||a.canvas||{};if(!dsl.components)dsl.components={};var n=0;var base=name.replace(/[^A-Za-z0-9_]/g,'');var key=base+':'+n;while(dsl.components[key]){n++;key=base+':'+n}var node={'obj':{'component_name':name,'params':{}},'downstream':[],'upstream':[]};if(dsl.components['begin']){dsl.components['begin'].downstream=dsl.components['begin'].downstream||[];dsl.components['begin'].downstream.push(key);node.upstream=['begin']}dsl.components[key]=node;var ta=document.getElementById('agentDslEdit');var pre=document.getElementById('agentDsl');ta.value=JSON.stringify(dsl,null,2);ta.style.display='block';pre.style.display='none';ta.focus();alert('Component \"'+name+'\" added to DSL. Click 💾 Save to persist.')})};function delAgent(){if(!confirm('Delete this agent?'))return;fetch('/api/v1/agents/'+aid,{method:'DELETE'}).then(function(){location.href='/agents'})};function openVersions(){document.getElementById('verList').innerHTML='<p style=color:var(--tm)>Loading...</p>';document.getElementById('verModal').classList.add('show');fetch('/api/v1/agents/'+aid+'/versions').then(r=>r.json()).then(function(d){var vs=d.data||[];if(!vs.length){document.getElementById('verList').innerHTML='<p style=color:var(--tm)>No versions yet</p>';return}var h='<table><thead><tr><th>ID</th><th>Title</th><th>Created</th><th></th></tr></thead><tbody>';vs.forEach(function(v){h+='<tr><td style=font-family:monospace;font-size:12px>'+v.id+'</td><td>'+(v.title||'')+'</td><td style=color:var(--tm);font-size:12px>'+new Date(v.created_at).toLocaleString()+'</td><td><button class=btn btn-o btn-sm onclick=downloadVersion(&quot;'+v.id+'&quot;)>Download</button></td></tr>'});document.getElementById('verList').innerHTML=h+'</tbody></table>'})};function downloadVersion(vid){fetch('/api/v1/agents/'+aid+'/versions/'+encodeURIComponent(vid)).then(r=>r.json()).then(function(d){var v=(d.data||{}).dsl||{};var a=document.createElement('a');a.href='data:application/json;charset=utf-8,'+encodeURIComponent(JSON.stringify(v,null,2));a.download='agent_'+aid+'_v'+vid+'.json';a.click()})};function exportAgentJson(){var ta=document.getElementById('agentDslEdit');var raw=ta.style.display==='none'?document.getElementById('agentDsl').textContent:ta.value;var a=document.createElement('a');a.href='data:application/json;charset=utf-8,'+encodeURIComponent(raw);a.download='agent_'+aid+'.json';a.click()};loadAgent()</script>");
    Html(layout("Agent", &b, zh))
}

pub async fn agents_page(
    State(state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let agents = state.agents.list();
    let mut rows = String::new();
    for a in &agents {
        rows.push_str(&format!("<tr data-name={}><td style='font-weight:500'>{}</td><td style='color:var(--t2)'>{}</td><td class='co'>{}</td><td><button class='btn btn-p btn-sm' onclick=\"runA('{}')\" >Run</button> <button class='btn btn-o btn-sm' onclick=\"editA('{}')\" >Edit</button> <button class='btn btn-o btn-sm' onclick=\"dupA('{}')\" >Copy</button> <button class='btn btn-o btn-sm' onclick=\"renameA('{}','{}')\" >Rename</button> <button class='btn btn-d btn-sm' onclick=\"delA('{}')\" >Del</button></td></tr>",a.name.to_lowercase(),a.name,a.description,a.kb_ids.join(", "),a.id,a.id,a.id,a.id,a.name,a.id));
    }
    let mut b = String::new();
    b.push_str(
        "<div style='display:flex;align-items:center;gap:12px'><h1 style='flex:1;margin:0'>🤖 ",
    );
    b.push_str(t("Agents", zh));
    b.push_str("</h1><button class='btn btn-p' onclick='showCreate()'>+ ");
    b.push_str(t("New Agent", zh));
    b.push_str("</button></div><div class='card'><input id='agentQ' placeholder='Search agents...' oninput='agentFilter()' style='max-width:260px;margin-bottom:8px'><table><thead><tr><th>Name</th><th>Description</th><th>KBs</th><th style='width:240px'>Actions</th></tr></thead><tbody id='agentTb'>");
    b.push_str(&rows);
    b.push_str("</tbody></table></div>");
    b.push_str("<div class='modal-overlay' id='createModal'><div class='modal'><h2>");
    b.push_str(t("New Agent", zh));
    b.push_str("</h2><fieldset style='border:0;padding:0;margin:0 0 12px'><legend style='font-weight:600;margin-bottom:8px'>Choose agent type</legend><label style='margin-right:18px'><input type='radio' name='agentType' value='agent_canvas' checked> Agent flow</label><label><input type='radio' name='agentType' value='dataflow_canvas'> Ingestion pipeline</label></fieldset><input id='aname' placeholder='Name'><textarea id='adesc' placeholder='Description' style='margin-top:8px;min-height:60px'></textarea><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='createA()'>");
    b.push_str(t("Create", zh));
    b.push_str("</button><button class='btn btn-o' onclick='showTemplates()'>From Template</button><button class='btn btn-o' onclick=document.getElementById('agentJsonFile').click()>");
    b.push_str(t("Import JSON", zh));
    b.push_str("</button><input type='file' id='agentJsonFile' style='display:none' onchange='importAgentJson()'><button class='btn btn-o' onclick='document.getElementById(\"createModal\").classList.remove(\"show\")'>");
    b.push_str(t("Cancel", zh));
    b.push_str("</button></div></div></div>");
    b.push_str("<div class='card'><h2>Test Agent</h2><input id='aq' placeholder='Ask...' style='max-width:500px'><button class='btn btn-p' onclick='askA()'>Ask</button><div id='aa' style='margin-top:12px;background:var(--bg);padding:16px;border-radius:var(--r);min-height:40px;white-space:pre-wrap;line-height:1.6'></div></div><script>var agentTemplates={
'rag':{'name':'RAG 问答','dsl':{'components':{'begin':{'obj':{'component_name':'Begin','params':{}},'downstream':['ret'],'upstream':[]},'ret':{'obj':{'component_name':'Retrieval','params':{'query':'{{sys.query}}','top_k':4}},'downstream':['ans'],'upstream':['begin']},'ans':{'obj':{'component_name':'Message','params':{'content':['{{ret@formalized_content}}']}},'downstream':[],'upstream':['ret']}}}},
'search':{'name':'网络搜索','dsl':{'components':{'begin':{'obj':{'component_name':'Begin','params':{}},'downstream':['sx'],'upstream':[]},'sx':{'obj':{'component_name':'SearXNG','params':{'query':'{{sys.query}}'}},'downstream':['ans'],'upstream':['begin']},'ans':{'obj':{'component_name':'Message','params':{'content':['{{sx@formalized_content}}']}},'downstream':[],'upstream':['sx']}}}},
'ragchat':{'name':'RAG 对话','dsl':{'components':{'begin':{'obj':{'component_name':'Begin','params':{}},'downstream':['ret'],'upstream':[]},'ret':{'obj':{'component_name':'Retrieval','params':{'query':'{{sys.query}}','top_k':4}},'downstream':['llm'],'upstream':['begin']},'llm':{'obj':{'component_name':'Generate','params':{'query':'{{ret@formalized_content}}'}},'downstream':['ans'],'upstream':['ret']},'ans':{'obj':{'component_name':'Message','params':{'content':['{{llm@content}}']}},'downstream':[],'upstream':['llm']}}}}
");
    // Append the 25 seeded RAGFlow workflow templates (src/agent_templates.rs)
    // to the 3 built-in templates above, for 28 total in the agents page.
    let mut tpls = String::new();
    for (slug, name, desc, dsl) in crate::agent_templates::AGENT_TEMPLATES {
        let name_j = serde_json::to_string(name).unwrap_or_else(|_| "\"\"".to_string());
        let desc_j = serde_json::to_string(desc).unwrap_or_else(|_| "\"\"".to_string());
        let canvas_category = crate::agent_templates::template_canvas_category(slug);
        // `</` would close the <script> tag in the rendered HTML; `<\/` is a
        // no-op escape valid in both JS and JSON, so the DSL round-trips.
        let dsl_j = dsl.replace("</", "<\\/");
        tpls.push_str(&format!(
            ",'{}':{{'name':{},'description':{},'canvas_category':'{}','dsl':{}}}",
            slug, name_j, desc_j, canvas_category, dsl_j
        ));
    }
    b.push_str(&tpls);
    b.push_str("};
function showTemplates(){var names=Object.keys(agentTemplates);var pick=prompt('Choose template:\\n'+names.map(function(k,i){return (i+1)+'. '+agentTemplates[k].name}).join('\\n'));if(!pick)return;var idx=parseInt(pick)-1;if(isNaN(idx)||!names[idx]){alert('Invalid choice');return}var t=agentTemplates[names[idx]];var n=prompt('Agent name:',t.name);if(!n)return;fetch('/api/v1/agents',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n,description:(t.description?t.description:'Created from template: '+t.name),canvas_category:t.canvas_category||'agent_canvas',dsl:t.dsl})}).then(r=>r.json()).then(function(d){if(d.code===0){location.reload()}else{alert('Create failed: '+(d.message||'unknown'))}})};
var aid='default';function testA(id){aid=id};function showCreate(){document.getElementById('createModal').classList.add('show')};function createA(){var n=document.getElementById('aname').value;if(!n)return;var d=document.getElementById('adesc').value;fetch('/api/v1/agents',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n,description:d,canvas_category:document.querySelector('input[name=agentType]:checked')?document.querySelector('input[name=agentType]:checked').value:'agent_canvas'})}).then(function(){location.reload()})};function importAgentJson(){var f=document.getElementById('agentJsonFile').files[0];if(!f)return;var r=new FileReader();r.onload=function(){try{var obj=JSON.parse(r.result);var nm=obj.title||obj.name||'Imported Agent';fetch('/api/v1/agents',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:nm,description:obj.description||'',canvas_category:obj.canvas_category||'agent_canvas',dsl:obj.dsl||obj.canvas||{}})}).then(function(res){return res.json()}).then(function(d){if(d.code===0){location.reload()}else{alert('Import failed: '+(d.message||'unknown'))}})}catch(e){alert('Invalid JSON: '+e.message)}};r.readAsText(f)};function editA(id){window.location.href='/agents/'+id};function runA(id){window.location.href='/agents/'+id};function renameA(id,cur){var n=prompt('New name:',cur);if(!n)return;fetch('/api/v1/agents/'+id,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n})}).then(r=>r.json()).then(d=>{if(d.code===0){location.reload()}else{alert('Rename failed: '+(d.message||'unknown'))}})};function dupA(id){fetch('/api/v1/agents/'+id).then(function(r){return r.json()}).then(function(d){var a=d.data||{};fetch('/api/v1/agents',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:(a.name||'agent')+' copy',description:a.description||'',kb_ids:a.kb_ids||[],canvas_category:a.canvas_category||'agent_canvas',dsl:a.dsl||{}})}).then(function(){location.reload()})})};function delA(id){if(!confirm('Delete agent?'))return;fetch('/api/v1/agents/'+id,{method:'DELETE'}).then(function(){location.reload()})};function askA(){var q=document.getElementById('aq').value;fetch('/api/v1/agents/'+aid+'/completions',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:q})}).then(r=>r.json()).then(d=>{document.getElementById('aa').textContent=d.data?d.data.answer:JSON.stringify(d)})}</script>");
    Html(layout(t("Agents", zh), &b, zh))
}

pub async fn datasources_page(
    req: axum::http::Request<axum::body::Body>,
) -> Result<axum::response::Html<String>, axum::response::Response> {
    let in_user_settings = req.uri().path() == "/user-setting/data-source";
    let zh = crate::web::detect_lang(req.headers());
    let mut b = format!(
        "<h1>🔗 {}</h1>",
        if zh {
            "数据源（Data Sources）"
        } else {
            "Data Sources"
        }
    );
    // 上游风格：35 张 Available 卡片 + 本地扩展下拉（卡片点击/Hover Add 打开弹层）
    b.push_str("<div class='card'><h2>");
    b.push_str(if zh {
        "可用数据源"
    } else {
        "Available sources"
    });
    b.push_str("</h2><p class='user-setting-description'>Add a connector and sync its content into your knowledge base.</p><section class='ds-available-grid' id='dsAvailable' data-testid='available-sources-section' aria-live='polite'></section><div class='ds-local-row' style='margin-top:16px'><label class='provider-instance-field' style='margin-top:0;max-width:300px'>Local sources<select id='dsLocalType'><option value='local'>Local Files</option><option value='feishu'>飞书云空间</option><option value='dingtalk'>钉钉云盘</option><option value='tencent_docs'>腾讯文档</option><option value='yuque'>语雀</option><option value='wechat_docs'>微信文档</option></select></label><button class='btn btn-p' type='button' onclick='openDsAdd(document.getElementById(\"dsLocalType\").value,document.getElementById(\"dsLocalType\").selectedOptions[0].textContent)'>+ Add</button></div></div>");
    b.push_str("<div class='card'><h2>Sources</h2><div style='margin-bottom:8px'><select id='dsKb'><option value=''>Sync to KB (optional)</option></select></div><div id='dsList'>Loading...</div></div>");
    b.push_str("<div class='card'><h2>");
    b.push_str(if zh {
        "文件列表 / 同步预览"
    } else {
        "Files / Sync Preview"
    });
    b.push_str("</h2><div id='dsPreview' style='color:var(--t2)'>Select a source, then List / Sync.</div></div><script>");
    b.push_str("function dsHint(){var m={'gitlab':'url=https://gitlab.com, token=PAT, target=project id or owner/repo','github':'url=https://github.com, token=PAT, target=owner/repo','bitbucket':'url=https://bitbucket.org, token=app_password, target=workspace/repo','webdav':'url=https://webdav.example.com, token=user:password, target=/path','confluence':'url=https://xxx.atlassian.net/wiki, token=email:api_token, target=spaceKey','jira':'url=https://xxx.atlassian.net, token=email:api_token, target=project key','notion':'token=integration_token, target=database/page id','google_drive':'token=service_account_json, target=folder id','gmail':'token=service_account_json, target=query','gcs':'url=https://storage.googleapis.com, token=access_token, target=bucket','oracle':'url=https://objectstorage.region.oraclecloud.com, token=access:secret, target=bucket','s3':'url=https://s3.region.amazonaws.com, token=access:secret, target=bucket','r2':'url=https://<account>.r2.cloudflarestorage.com, token=access:secret, target=bucket','azure_blob':'url=https://acct.blob.core.windows.net, token=SAS, target=container','box':'token=access_token (OAuth), target=folder id','dropbox':'token=access_token (OAuth), target=folder path','onedrive':'token=OAuth credentials, target=folder id','outlook':'token=OAuth credentials, target=mailbox query','sharepoint':'url=site url, token=OAuth credentials, target=library','teams':'token=OAuth credentials, target=channel id','slack':'token=bot token, target=channel id','discord':'token=bot token, target=channel id','zendesk':'url=https://subdomain.zendesk.com, token=email:api_token','salesforce':'url=instance url, token=OAuth credentials','moodle':'url=https://moodle.example.com, token=api_token','airtable':'token=personal_access_token, target=base id','asana':'token=personal_access_token, target=project gid','imap':'url=imap host, token=user:password, target=folder','dingtalk_ai_table':'token=appKey:appSecret, target=app id (多维表格)','seafile':'url=https://seafile.example.com, token=api_token, target=library id','rss':'url=https://feed.example.com/rss, target=optional filter','rest_api':'url=api endpoint, token=optional, target=optional','mysql':'url=host, token=user:password, target=database','postgresql':'url=host, token=user:password, target=database','bigquery':'token=service_account_json, target=dataset','local':'url=absolute directory path, target=optional relative subdir','feishu':'token=app_id:app_secret, target=可选 folder_token','dingtalk':'token=app_key:app_secret, target=可选 space_id','tencent_docs':'token=access_token, target=可选 folder_id','yuque':'token=access_token, target=namespace/repo slug','wechat_docs':'token=access_token, target=可选 folder_id'};document.getElementById('dsHint').textContent=m[document.getElementById('dsType').value]||'';}");
    b.push_str(r#"function loadKbs(){fetch('/api/v1/datasets').then(function(r){return r.json()}).then(function(d){var kbs=d.data||[];var sel=document.getElementById('dsKb');sel.replaceChildren();var empty=document.createElement('option');empty.value='';empty.textContent='Sync to KB (optional)';sel.appendChild(empty);kbs.forEach(function(k){var option=document.createElement('option');option.value=k.id;option.textContent=k.name||k.id;sel.appendChild(option)})})}function dsCell(row,text){var cell=document.createElement('td');cell.textContent=text==null?'':String(text);row.appendChild(cell);return cell}function dsAction(text,className,handler){var button=document.createElement('button');button.type='button';button.className=className;button.textContent=text;button.addEventListener('click',handler);return button}function loadDs(){fetch('/api/v1/data_sources').then(function(r){return r.json()}).then(function(d){var list=d.data||[];var root=document.getElementById('dsList');root.replaceChildren();var table=document.createElement('table');var head=document.createElement('thead');head.innerHTML='<tr><th>Name</th><th>Type</th><th>URL</th><th>Target</th><th>State</th><th></th></tr>';table.appendChild(head);var body=document.createElement('tbody');list.forEach(function(x){var row=document.createElement('tr');dsCell(row,x.name);dsCell(row,x.type);dsCell(row,x.url||'');dsCell(row,x.target||'');var state=dsCell(row,'');var badge=document.createElement('span');badge.className=x.connected?'badge bs':'badge bw';badge.textContent=x.connected?'connected':'disconnected';state.appendChild(badge);var actions=dsCell(row,'');actions.append(dsAction('Connect','btn btn-p btn-sm',function(){connDs(x.id)}),document.createTextNode(' '),dsAction('Disconnect','btn btn-o btn-sm',function(){discDs(x.id)}),document.createTextNode(' '),dsAction('List','btn btn-o btn-sm',function(){listDs(x.id)}),document.createTextNode(' '),dsAction('Sync','btn btn-p btn-sm',function(){syncDs(x.id)}),document.createTextNode(' '),dsAction('Details','btn btn-o btn-sm',function(){location.href='/user-setting/data-source/'+encodeURIComponent(x.id)}),document.createTextNode(' '),dsAction('Del','btn btn-d btn-sm',function(){delDs(x.id)}));body.appendChild(row)});table.appendChild(body);root.appendChild(table)})}"#);
    b.push_str("function connDs(id){document.getElementById('dsPreview').textContent='Connecting...';fetch('/api/v1/data_sources/'+id+'/connect',{method:'POST'}).then(function(r){return r.json()}).then(function(d){document.getElementById('dsPreview').textContent=d.code===0?'Connected ✓':'Connect failed: '+(d.message||'unknown');loadDs();});}");
    b.push_str("function discDs(id){fetch('/api/v1/data_sources/'+id+'/disconnect',{method:'POST'}).then(function(r){return r.json()}).then(function(d){document.getElementById('dsPreview').textContent=d.code===0?'Disconnected':'Disconnect failed: '+(d.message||'unknown');loadDs();});}");
    b.push_str("function listDs(id){document.getElementById('dsPreview').textContent='Listing...';fetch('/api/v1/data_sources/'+id+'/files',{method:'POST'}).then(function(r){return r.json()}).then(function(d){if(d.code===0){var x=d.data||{};var h='<b>'+x.count+' files</b><table><tr><th>Name</th><th>Ext</th><th>Size</th><th>Updated</th></tr>';(x.files||[]).forEach(function(f){h=h+'<tr><td>'+(f.name||'')+'</td><td>'+(f.extension||'')+'</td><td>'+(f.size_bytes||0)+'</td><td>'+(f.updated_at||'')+'</td></tr>';});document.getElementById('dsPreview').innerHTML=h+'</table>';}else{document.getElementById('dsPreview').textContent='List failed: '+(d.message||'unknown');}});}");
    b.push_str("function syncDs(id){document.getElementById('dsPreview').textContent='Syncing...';var kb=document.getElementById('dsKb').value||'';var url='/api/v1/data_sources/'+id+'/sync'+(kb?('?kb_id='+encodeURIComponent(kb)):'');fetch(url,{method:'POST'}).then(function(r){return r.json()}).then(function(d){if(d.code===0){var x=d.data||{};var h='<b>'+x.count+' docs</b>'+(x.failures&&x.failures.length?' <span style=color:var(--d)>'+x.failures.length+' failures</span>':'')+'<table><tr><th>Title</th><th>Ext</th><th>Size</th><th>Type</th></tr>';(x.preview||[]).forEach(function(p){h=h+'<tr><td>'+(p.title||'')+'</td><td>'+(p.extension||'')+'</td><td>'+(p.size_bytes||0)+'</td><td>'+(p.type||'')+'</td></tr>';});document.getElementById('dsPreview').innerHTML=h+'</table>';}else{document.getElementById('dsPreview').textContent='Sync failed: '+(d.message||'unknown');}});}");
    b.push_str("function delDs(id){if(!confirm('Delete this data source?'))return;fetch('/api/v1/data_sources/'+id,{method:'DELETE'}).then(function(r){return r.json()}).then(function(d){if(d.code===0){loadDs();}else{alert(d.message||'delete failed');}});}loadKbs();loadDs();</script>");
    let schema = serde_json::to_string(&crate::data_source::form_fields::schema_for_ui())
        .unwrap_or_else(|_| "{}".to_string());
    b.push_str(&data_source_add_modal(&schema, zh));
    let info_json = serde_json::to_string(&crate::data_source::form_fields::source_info_for_ui())
        .unwrap_or_else(|_| "{}".to_string());
    b.push_str(&data_source_list_section(&info_json));
    let page = if in_user_settings {
        user_setting_layout(
            if zh { "数据源" } else { "Data sources" },
            &b,
            UserSettingSection::DataSource,
            zh,
        )
    } else {
        layout(if zh { "数据源" } else { "Data sources" }, &b, zh)
    };
    Ok(axum::response::Html(page))
}

/// Render the schema-driven data-source detail form. The template is kept in
/// raw strings with `__PLACEHOLDER__` tokens so the embedded JavaScript stays
/// brace-free and line-editable.
fn data_source_detail_body(name: &str, id: &str, source: &str, schema: &str, zh: bool) -> String {
    let head = r#"<style>#dsDetailTest[hidden],#dsFlatFields[hidden],#dsPruneField[hidden]{display:none!important}.ds-segment{display:inline-flex;flex-wrap:wrap;gap:2px;padding:3px;margin-top:8px;border:1px solid var(--bd);border-radius:var(--r);background:var(--bg);width:fit-content}.ds-segment button{border:0;border-radius:calc(var(--r) - 2px);padding:6px 12px;background:transparent;color:var(--t2);font-size:13px;cursor:pointer}.ds-segment button[aria-checked=true]{background:var(--p);color:#fff}.ds-suffix{padding-left:6px;color:var(--tm);font-style:italic;font-size:12px}.ds-field-tip{display:block;margin-top:2px;font-size:11px;line-height:1.4;color:var(--tm)}.provider-instance-field textarea{width:100%;min-height:88px;resize:vertical}.provider-instance-field input:disabled{opacity:.65;cursor:not-allowed}.ds-log-summary{max-width:32rem;white-space:normal;word-break:break-word}.ds-log-pager{display:flex;justify-content:flex-end;align-items:center;gap:8px;margin-top:12px}</style><h1 id='dsDetailTitle'>🔗 __NAME__</h1><a class='btn btn-o btn-sm' href='/user-setting/data-source'>← __BACK__</a><div class='card' data-testid='data-source-detail-card'><h2>__CONN__</h2><p class='user-setting-description'>__TYPE__</p><label class='provider-instance-field'>Name<input id='dsDetailName' autocomplete='off' disabled></label><div id='dsFlatFields'><label class='provider-instance-field'>URL<input id='dsDetailUrl' autocomplete='off'></label><label class='provider-instance-field'>Target<input id='dsDetailTarget' autocomplete='off'></label><label class='provider-instance-field'>Token<input id='dsDetailToken' type='password' autocomplete='new-password' placeholder='Configured; leave blank to keep'></label></div><div id='dsDynamicFields'></div><label class='provider-instance-switch'><span>Sync deleted files</span><input id='dsDetailSyncDeleted' type='checkbox' role='switch'></label><label class='provider-instance-field' id='dsPruneField'>Prune Freq<span class='ds-suffix'>(minutes)</span><input id='dsDetailPrune' type='number' min='0' step='any'></label><label class='provider-instance-field'>Refresh Freq<span class='ds-suffix'>(minutes)</span><input id='dsDetailRefresh' type='number' min='0' step='any'></label><label class='provider-instance-field'>Timeout Secs<span class='ds-suffix'>(seconds)</span><input id='dsDetailTimeout' type='number' min='0' step='any'></label><p id='dsDetailStatus' class='provider-instance-status' aria-live='polite'></p><div class='provider-instance-actions'><button class='btn btn-o' id='dsDetailTest' type='button' onclick='dsDetailTest()' hidden>Test</button><button class='btn btn-p' id='dsDetailPrimary' type='button' onclick='dsDetailPrimary()'>Save</button></div></div><div class='card' data-testid='data-source-logs-card'><h2>Sync Logs</h2><div class='user-setting-table-wrap'><table class='team-table'><thead><tr><th>Time Started</th><th>Status</th><th>Dataset</th><th>Task Type</th><th>Summary</th></tr></thead><tbody id='dsLogRows'></tbody></table><div class='ds-log-pager' id='dsLogPager'></div></div></div>__JS__"#;
    let js = r#"<script>window.DS_ZH=__DSZH__;var DS_DETAIL_ID='__ID__';var DS_SCHEMA=__SCHEMA__;var DS_SOURCE='__SOURCE__';var DS_SCHEMA_FIELDS=DS_SCHEMA[DS_SOURCE.toUpperCase()]||[];var DS_WRAPS={};var DS_RECORD=null;var DS_DIRTY=false;var DS_POLL=null;function dsDetailApi(url,options){return fetch(url,options).then(function(response){return response.json()})}function dsGet(obj,path){return (path||'').split('.').reduce(function(o,k){return o==null?undefined:o[k]},obj||{})}function dsSet(obj,path,value){var parts=path.split('.');var cur=obj;for(var i=0;i<parts.length-1;i++){if(typeof cur[parts[i]]!=='object'||cur[parts[i]]===null)cur[parts[i]]={};cur=cur[parts[i]]}cur[parts[parts.length-1]]=value}function dsInputOf(wrap){return wrap.querySelector('input,select,textarea')}function dsMarkDirty(){DS_DIRTY=true;dsUpdatePrimary()}function dsVisible(field){if(!field.show_when||!field.show_when.length)return true;return field.show_when.some(function(condition){var eq=String(dsFieldValue(condition.field))===String(condition.equals);return condition.negate?!eq:eq})}function dsRefreshVisibility(){DS_SCHEMA_FIELDS.forEach(function(field){var wrap=DS_WRAPS[field.name];if(wrap)wrap.hidden=!dsVisible(field)});document.getElementById('dsPruneField').hidden=!document.getElementById('dsDetailSyncDeleted').checked}function dsFieldValue(name){var wrap=DS_WRAPS[name];if(!wrap)return '';var input=dsInputOf(wrap);if(input.type==='checkbox')return input.checked?'true':'false';return input.value}function dsSetFieldValue(name,value){var wrap=DS_WRAPS[name];if(!wrap)return;var input=dsInputOf(wrap);if(input.type==='checkbox'){input.checked=String(value)==='true'}else{input.value=value==null?'':String(value)}if(wrap._seg){wrap._seg.querySelectorAll('button').forEach(function(button){button.setAttribute('aria-checked',String(button.dataset.value===String(input.value)))})}}function dsBuildField(field){var checkbox=field.type==='Checkbox'||field.type==='Switch';var wrap=document.createElement('label');wrap.className=checkbox?'provider-instance-switch':'provider-instance-field';var span=document.createElement('span');span.textContent=field.label+(field.required?' *':'');wrap.appendChild(span);if(field.tooltip_en||field.tooltip_zh){var tipEl=document.createElement('span');tipEl.className='ds-field-tip';tipEl.textContent=(window.DS_ZH&&field.tooltip_zh)?field.tooltip_zh:field.tooltip_en;wrap.appendChild(tipEl)}if(checkbox){var check=document.createElement('input');check.type='checkbox';check.addEventListener('change',function(){dsMarkDirty();dsRefreshVisibility()});wrap.appendChild(check);return wrap}var input;if(field.type==='Select'){input=document.createElement('select');(field.options||[]).forEach(function(option){var node=document.createElement('option');node.value=option.value;node.textContent=option.label;input.appendChild(node)});if(!field.required){var blank=document.createElement('option');blank.value='';blank.textContent='Please select';input.prepend(blank)}input.addEventListener('change',function(){dsMarkDirty();dsRefreshVisibility()})}else if(field.type==='Textarea'){input=document.createElement('textarea');input.rows=4;input.addEventListener('input',function(){dsMarkDirty();dsRefreshVisibility()})}else if(field.type==='Segmented'){input=document.createElement('input');input.type='hidden';var segment=document.createElement('div');segment.className='ds-segment';(field.options||[]).forEach(function(option){var button=document.createElement('button');button.type='button';button.textContent=option.label;button.dataset.value=option.value;button.addEventListener('click',function(){input.value=option.value;segment.querySelectorAll('button').forEach(function(node){node.setAttribute('aria-checked',String(node===button))});dsMarkDirty();dsRefreshVisibility()});segment.appendChild(button)});wrap.appendChild(input);wrap.appendChild(segment);wrap._seg=segment;return wrap}else{input=document.createElement('input');input.type=field.type==='Password'?'password':field.type==='Number'?'number':'text';if(input.type==='number')input.step='any';if(field.type==='Password')input.autocomplete='new-password';if(field.placeholder)input.placeholder=field.placeholder;input.addEventListener('input',function(){dsMarkDirty();dsRefreshVisibility()})}wrap.appendChild(input);return wrap}function dsBuildFields(){var root=document.getElementById('dsDynamicFields');DS_SCHEMA_FIELDS.forEach(function(field){var wrap=dsBuildField(field);DS_WRAPS[field.name]=wrap;root.appendChild(wrap)})}function dsBuildPayload(){var payload={name:document.getElementById('dsDetailName').value.trim(),sync_deleted_files:document.getElementById('dsDetailSyncDeleted').checked,prune_freq:Number(document.getElementById('dsDetailPrune').value)||0,refresh_freq:Number(document.getElementById('dsDetailRefresh').value)||0,timeout_secs:Number(document.getElementById('dsDetailTimeout').value)||0};if(DS_SCHEMA_FIELDS.length){var config={};DS_SCHEMA_FIELDS.forEach(function(field){dsSet(config,field.name,dsFieldValue(field.name))});payload.config=config}else{payload.url=document.getElementById('dsDetailUrl').value.trim();payload.target=document.getElementById('dsDetailTarget').value.trim();var token=document.getElementById('dsDetailToken').value;if(token)payload.token=token}return payload}function dsActionMode(){var status=(DS_RECORD&&DS_RECORD.status||'unstart').toLowerCase();if(DS_DIRTY)return 'save';if(status==='unstart')return 'save';if(status==='running'||status==='schedule')return 'stop';return 'resume'}function dsUpdatePrimary(){var mode=dsActionMode();var button=document.getElementById('dsDetailPrimary');button.dataset.mode=mode;button.textContent=mode==='save'?'Save':mode==='stop'?'Stop':'Resume'}function dsStatus(text,error){var status=document.getElementById('dsDetailStatus');status.textContent=text||'';status.style.color=error?'var(--d)':'var(--tm)'}var DS_LOGS={page:1,pageSize:10,total:0,rows:[]};var DS_LOG_TICK=null;function dsLogStatusBadge(status){var map={UNSTART:['Pending','bw'],'0':['Pending','bw'],RUNNING:['Running','bi'],'1':['Running','bi'],CANCEL:['Cancel','bw'],'2':['Cancel','bw'],DONE:['Success','bs'],'3':['Success','bs'],FAIL:['Failed','bw'],'4':['Failed','bw'],SCHEDULE:['Schedule','bi'],'5':['Schedule','bi']};var entry=map[status]||[status||'Unknown','bw'];var badge=document.createElement('span');badge.className='badge '+entry[1];if(status==='FAIL'||status==='4')badge.style.color='var(--d)';badge.textContent=entry[0];return badge}function dsFormatDuration(seconds){seconds=Math.max(0,seconds);var hours=Math.floor(seconds/3600);var minutes=Math.floor((seconds%3600)/60);var remaining=seconds%60;if(hours>0)return hours+'h '+minutes+'m '+remaining+'s';if(minutes>0)return minutes+'m '+remaining+'s';return remaining+'s'}function dsLogCountdownSeconds(log,now){var freq=Number(log.task_type==='prune'?(log.prune_freq||0):(log.refresh_freq||0));var scheduledAt=log.time_started?Date.parse(log.time_started):0;if(!freq||!scheduledAt)return null;return Math.ceil((scheduledAt+freq*60*1000-now)/1000)}function dsLogSummary(log,now){var status=String(log.status||'').toUpperCase();if(status==='SCHEDULE'||status==='5'){var countdown=dsLogCountdownSeconds(log,now);if(countdown!=null&&countdown>0)return 'Task starts in '+dsFormatDuration(countdown);return 'Scheduled'}if(status==='RUNNING'||status==='1')return log.task_type==='prune'?'Prune in progress':'Sync in progress';if(status==='FAIL'||status==='4')return log.error_msg||'Task failed';if(status==='CANCEL'||status==='2')return '';if(log.task_type==='prune')return 'deleted='+(log.docs_removed_from_index||0)+', error='+(log.error_count||0);var total=log.total_docs_indexed||0;var added=log.new_docs_indexed||0;return 'total='+total+', added='+added+', updated='+Math.max(0,total-added)+', error='+(log.error_count||0)}function dsRenderLogsPage(){var now=Date.now();var body=document.getElementById('dsLogRows');body.replaceChildren();(DS_LOGS.rows||[]).forEach(function(log,index){var row=document.createElement('tr');var time=document.createElement('td');time.textContent=log.update_date||log.created_at||'';var status=document.createElement('td');status.appendChild(dsLogStatusBadge(log.status));var dataset=document.createElement('td');if(log.kb_name){var link=document.createElement('a');link.href='/dataset/'+encodeURIComponent(log.kb_id||'');link.textContent=log.kb_name;link.style.color='var(--p)';link.style.textDecoration='none';dataset.appendChild(link)}var taskType=document.createElement('td');taskType.textContent=log.task_type||'sync';var summary=document.createElement('td');summary.className='ds-log-summary';summary.dataset.index=String(index);summary.textContent=dsLogSummary(log,now);row.append(time,status,dataset,taskType,summary);body.appendChild(row)});if(!(DS_LOGS.rows||[]).length){var row=document.createElement('tr');var cell=document.createElement('td');cell.colSpan=5;cell.className='team-empty';cell.textContent='No data';row.appendChild(cell);body.appendChild(row)}var pager=document.getElementById('dsLogPager');pager.replaceChildren();var total=DS_LOGS.total||0;var pages=Math.max(1,Math.ceil(total/DS_LOGS.pageSize));var prev=document.createElement('button');prev.type='button';prev.className='btn btn-o btn-sm';prev.textContent='Prev';prev.disabled=DS_LOGS.page<=1;prev.addEventListener('click',function(){loadDsLogs(DS_LOGS.page-1)});var next=document.createElement('button');next.type='button';next.className='btn btn-o btn-sm';next.textContent='Next';next.disabled=DS_LOGS.page>=pages;next.addEventListener('click',function(){loadDsLogs(DS_LOGS.page+1)});var info=document.createElement('span');info.style.fontSize='12px';info.style.color='var(--tm)';info.textContent='Page '+DS_LOGS.page+' of '+pages+' · '+total+' logs · '+DS_LOGS.pageSize+' per page';pager.append(prev,next,info)}async function loadDsLogs(page){page=Math.max(1,page);DS_LOGS.page=page;var data=await dsDetailApi('/api/v1/data_sources/'+DS_DETAIL_ID+'/logs?page='+page+'&page_size='+DS_LOGS.pageSize);var payload=(data&&data.data)||{};DS_LOGS.total=payload.total||0;DS_LOGS.rows=payload.logs||[];dsRenderLogsPage()}function dsStartLogTick(){if(DS_LOG_TICK)clearInterval(DS_LOG_TICK);DS_LOG_TICK=setInterval(function(){if(!(DS_LOGS.rows||[]).length)return;var now=Date.now();document.getElementById('dsLogRows').querySelectorAll('td.ds-log-summary').forEach(function(cell){var log=DS_LOGS.rows[Number(cell.dataset.index)];if(log)cell.textContent=dsLogSummary(log,now)})},1000)}async function dsMaybePoll(){if(DS_POLL)clearInterval(DS_POLL);DS_POLL=null;var status=(DS_RECORD&&DS_RECORD.status||'').toLowerCase();if(status==='running'||status==='schedule')DS_POLL=setInterval(function(){dsDetailApi('/api/v1/data_sources/'+DS_DETAIL_ID).then(function(data){DS_RECORD=data.data||DS_RECORD;loadDsLogs(DS_LOGS.page);dsUpdatePrimary()}).catch(function(){})},15000)}async function loadDsDetail(){var data=await dsDetailApi('/api/v1/data_sources/'+DS_DETAIL_ID);DS_RECORD=data.data||{};document.getElementById('dsDetailTitle').textContent='🔗 '+(DS_RECORD.name||'');document.getElementById('dsDetailName').value=DS_RECORD.name||'';document.getElementById('dsFlatFields').hidden=DS_SCHEMA_FIELDS.length>0;document.getElementById('dsDetailUrl').value=DS_RECORD.url||'';document.getElementById('dsDetailTarget').value=DS_RECORD.target||'';document.getElementById('dsDetailToken').value='';document.getElementById('dsDetailSyncDeleted').checked=!!DS_RECORD.sync_deleted_files;document.getElementById('dsDetailPrune').value=DS_RECORD.prune_freq==null?'':DS_RECORD.prune_freq;document.getElementById('dsDetailRefresh').value=DS_RECORD.refresh_freq==null?'':DS_RECORD.refresh_freq;document.getElementById('dsDetailTimeout').value=DS_RECORD.timeout_secs==null?'':DS_RECORD.timeout_secs;DS_SCHEMA_FIELDS.forEach(function(field){var value=dsGet(DS_RECORD.config,field.name);if(value===undefined||value===null||value==='')value=field.default_value||'';dsSetFieldValue(field.name,value)});document.getElementById('dsDetailTest').hidden=!(DS_SOURCE==='rest_api'||DS_SOURCE==='bigquery');dsRefreshVisibility();DS_DIRTY=false;dsUpdatePrimary();DS_LOGS.page=1;loadDsLogs(1);dsStartLogTick();dsMaybePoll()}async function dsDetailTest(){dsStatus('Testing…',false);var data=await dsDetailApi('/api/v1/data_sources/'+DS_DETAIL_ID+'/test',{method:'POST'});dsStatus(data.code===0?'Connection verified ✓':'Test failed: '+(data.message||'unknown'),data.code!==0);await loadDsDetail()}async function dsDetailPrimary(){var button=document.getElementById('dsDetailPrimary');var mode=button.dataset.mode;button.disabled=true;if(mode==='stop'){dsStatus('Stopping…',false);try{var stopped=await dsDetailApi('/api/v1/data_sources/'+DS_DETAIL_ID,{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({status:'cancel'})});if(stopped.code!==0)throw new Error(stopped.message||('HTTP '+stopped.code));dsStatus('Stopped ✓',false);await loadDsDetail()}catch(error){dsStatus(error.message,true)}finally{button.disabled=false}return}var payload=mode==='save'?dsBuildPayload():{status:'schedule'};if(mode==='save')payload.reschedule=true;dsStatus(mode==='save'?'Saving…':'Resuming…',false);try{var data=await dsDetailApi('/api/v1/data_sources/'+DS_DETAIL_ID,{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify(payload)});if(data.code!==0)throw new Error(data.message||('HTTP '+data.code));var nextStatus=data.data&&data.data.status||'';dsStatus(nextStatus==='fail'?'Sync failed; see logs below':'Saved ✓',nextStatus==='fail');await loadDsDetail()}catch(error){dsStatus(error.message,true)}finally{button.disabled=false}}dsBuildFields();loadDsDetail()</script>"#;
    let js = js
        .replace("__ID__", id)
        .replace("__SOURCE__", source)
        .replace("__SCHEMA__", schema)
        .replace("__DSZH__", if zh { "true" } else { "false" })
        .replace(
            "function dsMarkDirty()",
            "function dsFieldPath(name){return name.replace(/^config\\./,'')}function dsMarkDirty()",
        )
        .replace(
            "dsSet(config,field.name,dsFieldValue(field.name))",
            "dsSet(config,dsFieldPath(field.name),dsFieldValue(field.name))",
        )
        .replace(
            "var value=dsGet(DS_RECORD.config,field.name);",
            "var value=dsGet(DS_RECORD.config,dsFieldPath(field.name));",
        );
    head.replace("__NAME__", &html_escape(name))
        .replace("__BACK__", t("Back", zh))
        .replace("__CONN__", t("Connection", zh))
        .replace("__TYPE__", t("Type", zh))
        .replace(
            "<input id='dsDetailSyncDeleted' type='checkbox' role='switch'>",
            "<input id='dsDetailSyncDeleted' type='checkbox' role='switch' onchange='dsMarkDirty();dsRefreshVisibility()'>",
        )
        .replace("__JS__", &js)
}

/// Schema-driven Add data source modal for the data-source list page. The
/// type selector lives on the page; this overlay renders the per-source field
/// set for the selected type and POSTs the nested upstream wire shape.
fn data_source_add_modal(schema: &str, zh: bool) -> String {
    let overlay = r#"<div class='modal-overlay' id='dsAddModal' data-testid='data-source-add-modal' onclick='if(event.target===this)closeDsAdd()'><div class='modal' role='dialog' aria-modal='true' aria-labelledby='dsAddTitle'><h2 id='dsAddTitle'>Add data source</h2><label class='provider-instance-field'>Name<input id='dsAddName' autocomplete='off'></label><div id='dsAddFields'></div><p id='dsAddError' class='provider-instance-error'></p><div class='provider-instance-actions'><button class='btn btn-o' type='button' onclick='closeDsAdd()'>Cancel</button><button class='btn btn-p' id='dsAddConfirm' type='button' onclick='dscCreate()'>Confirm</button></div></div></div>"#;
    let box_dialog = r#"<div class='modal-overlay' id='dscBoxDialog' data-testid='box-oauth-dialog' onclick='if(event.target===this)closeDscBoxDialog()'><div class='modal' style='max-width:520px' role='dialog' aria-modal='true' aria-labelledby='dscBoxTitle'><h2 id='dscBoxTitle'>Configure Box OAuth credentials</h2><p class='user-setting-description'>Enter your Box application's Client ID, Client Secret, and Redirect URI. These values will be stored in the form field and can be used later to start the OAuth flow.</p><label class='provider-instance-field'>Client ID<input id='dscBoxClientId' placeholder='Enter Box Client ID' autocomplete='off'></label><label class='provider-instance-field'>Client Secret<input id='dscBoxClientSecret' type='password' placeholder='Enter Box Client Secret' autocomplete='new-password'></label><label class='provider-instance-field'>Redirect URI<input id='dscBoxRedirect' placeholder='https://example.com/box/oauth/callback' autocomplete='off'></label><p id='dscBoxMsg' class='provider-instance-status' hidden></p><div class='provider-instance-actions'><button class='btn btn-o' type='button' onclick='closeDscBoxDialog()'>Cancel</button><button class='btn btn-o' id='dscBoxRefresh' type='button' onclick='dscBoxPoll()' hidden>Refresh status</button><button class='btn btn-p' id='dscBoxSubmit' type='button' onclick='dscBoxSubmit()'>Submit &amp; Authorize</button></div></div></div>"#;
    let google_dialog = r#"<div class='modal-overlay' id='dscGoogleDialog' data-testid='google-oauth-dialog' onclick='if(event.target===this)closeDscGoogleDialog()'><div class='modal' style='max-width:520px' role='dialog' aria-modal='true' aria-labelledby='dscGoogleTitle'><h2 id='dscGoogleTitle'>Complete Google verification</h2><p class='user-setting-description'>The uploaded client credentials do not contain a refresh token. Run the verification flow once to mint reusable tokens.</p><label class='provider-instance-field'>Redirect URI<input id='dscGoogleRedirect' placeholder='https://example.com/google-drive/oauth/callback' autocomplete='off'></label><p id='dscGoogleMsg' class='provider-instance-status' hidden></p><div class='provider-instance-actions'><button class='btn btn-o' type='button' onclick='closeDscGoogleDialog()'>Cancel</button><button class='btn btn-o' id='dscGoogleRefresh' type='button' onclick='dscGooglePoll()' hidden>Refresh status</button><button class='btn btn-p' id='dscGoogleSubmit' type='button' onclick='dscGoogleAuthorize()'>Authorize with Google</button></div></div></div>"#;
    let js = r#"<script>window.DS_ZH=__DSZH__;var DS_SCHEMA=__SCHEMA__;var DSC_WRAPS={};var DSC_FIELDS=[];function dscPath(name){return name.replace(/^config\./,'')}function dscSet(obj,path,value){var parts=path.split('.');var cur=obj;for(var i=0;i<parts.length-1;i++){if(typeof cur[parts[i]]!=='object'||cur[parts[i]]===null)cur[parts[i]]={};cur=cur[parts[i]]}cur[parts[parts.length-1]]=value}function dscInputOf(wrap){return wrap.querySelector('input,select,textarea')}function dscValue(name){var wrap=DSC_WRAPS[name];if(!wrap)return '';var input=dscInputOf(wrap);if(input.type==='checkbox')return input.checked?'true':'false';return input.value}function dscVisible(field){if(!field.show_when||!field.show_when.length)return true;return field.show_when.some(function(condition){var eq=String(dscValue(condition.field))===String(condition.equals);return condition.negate?!eq:eq})}function dscRefresh(){DSC_FIELDS.forEach(function(field){var wrap=DSC_WRAPS[field.name];if(wrap)wrap.hidden=!dscVisible(field)})}function dscBuildField(field){var checkbox=field.type==='Checkbox'||field.type==='Switch';var wrap=document.createElement('label');wrap.className=checkbox?'provider-instance-switch':'provider-instance-field';var span=document.createElement('span');span.textContent=field.label+(field.required?' *':'');wrap.appendChild(span);if(field.tooltip_en||field.tooltip_zh){var tipEl=document.createElement('span');tipEl.className='ds-field-tip';tipEl.textContent=(window.DS_ZH&&field.tooltip_zh)?field.tooltip_zh:field.tooltip_en;wrap.appendChild(tipEl);}if(checkbox){var check=document.createElement('input');check.type='checkbox';check.addEventListener('change',function(){dscRefresh()});wrap.appendChild(check);return wrap}var input;if(field.type==='Select'){input=document.createElement('select');(field.options||[]).forEach(function(option){var node=document.createElement('option');node.value=option.value;node.textContent=option.label;input.appendChild(node)});if(!field.required){var blank=document.createElement('option');blank.value='';blank.textContent='Please select';input.prepend(blank)}if(field.default_value&&!field.required===false){input.value=field.default_value}input.addEventListener('change',function(){dscRefresh()})}else if(field.type==='Textarea'){input=document.createElement('textarea');input.rows=4}else if(field.type==='Segmented'){input=document.createElement('input');input.type='hidden';var segment=document.createElement('div');segment.className='ds-segment';(field.options||[]).forEach(function(option){var button=document.createElement('button');button.type='button';button.textContent=option.label;button.dataset.value=option.value;button.addEventListener('click',function(){input.value=option.value;segment.querySelectorAll('button').forEach(function(node){node.setAttribute('aria-checked',String(node===button))});dscRefresh()});segment.appendChild(button)});input.value=field.default_value||(field.options&&field.options[0]&&field.options[0].value)||'';segment.querySelectorAll('button').forEach(function(node){node.setAttribute('aria-checked',String(node.dataset.value===input.value))});wrap.appendChild(input);wrap.appendChild(segment);return wrap}else{input=document.createElement('input');input.type=field.type==='Password'?'password':field.type==='Number'?'number':'text';if(input.type==='number')input.step='any';if(field.type==='Password')input.autocomplete='new-password';if(field.placeholder)input.placeholder=field.placeholder;if(field.default_value)input.value=field.default_value}wrap.appendChild(input);return wrap}function dscFlatFields(){var root=document.getElementById('dsAddFields');['URL','Target','Token'].forEach(function(label){var wrap=document.createElement('label');wrap.className='provider-instance-field';var span=document.createElement('span');span.textContent=label;wrap.appendChild(span);var input=document.createElement('input');input.id='dsc'+label;if(label==='Token'){input.type='password';input.autocomplete='new-password'}wrap.appendChild(input);root.appendChild(wrap)})}function dscRenderFields(){var root=document.getElementById('dsAddFields');root.replaceChildren();DSC_WRAPS={};var kind=(document.getElementById('dsType').value||'').toUpperCase();DSC_FIELDS=DS_SCHEMA[kind]||[];if(!DSC_FIELDS.length){dscFlatFields();return}DSC_FIELDS.forEach(function(field){var wrap=dscBuildField(field);DSC_WRAPS[field.name]=wrap;root.appendChild(wrap)});dscRefresh()}function openDsAdd(){var select=document.getElementById('dsType');document.getElementById('dsAddTitle').textContent='Add data source: '+select.selectedOptions[0].textContent;document.getElementById('dsAddError').textContent='';document.getElementById('dsAddName').value='';dscRenderFields();document.getElementById('dsAddModal').classList.add('show')}function closeDsAdd(){document.getElementById('dsAddModal').classList.remove('show')}function dscCreate(){var name=document.getElementById('dsAddName').value.trim();var error='';if(!name)error='Name is required';DSC_FIELDS.forEach(function(field){if(field.required&&!dscValue(field.name).trim())error=error||(field.label+' is required')});if(error){document.getElementById('dsAddError').textContent=error;return}var body={name:name,type:document.getElementById('dsType').value};if(DSC_FIELDS.length){var config={};DSC_FIELDS.forEach(function(field){dscSet(config,dscPath(field.name),dscValue(field.name))});body.config=config}else{body.url=document.getElementById('dscUrl').value.trim();body.target=document.getElementById('dscTarget').value.trim();var token=document.getElementById('dscToken').value;if(token)body.token=token;if(!body.url||!body.target){document.getElementById('dsAddError').textContent='URL/Target required';return}}var button=document.getElementById('dsAddConfirm');button.disabled=true;fetch('/api/v1/data_sources',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(function(response){return response.json()}).then(function(data){if(data.code===0){closeDsAdd();loadDs()}else{document.getElementById('dsAddError').textContent=data.message||'create failed'}}).finally(function(){button.disabled=false})}</script>"#;
    let js = js
        .replace("__DSZH__", if zh { "true" } else { "false" })
        .replace("__SCHEMA__", schema)
        .replace(
            "function dscBuildField(field){var checkbox=",
            "function dscBuildField(field){if(field.name==='config.credentials.box_tokens')return dscBuildBoxField(field);if(field.name==='config.credentials.google_tokens'&&(window.DSC_TYPE==='gmail'||window.DSC_TYPE==='google_drive'))return dscBuildGoogleField(field);var checkbox=",
        )
        .replace(
            "function dscFlatFields()",
            "var DSC_BOX_FLOW='';var DSC_BOX_CREDS={};function dscBuildBoxField(field){var wrap=document.createElement('div');wrap.className='provider-instance-field';var span=document.createElement('span');span.textContent=field.label+(field.required?' *':'');var input=document.createElement('input');input.type='hidden';input.dataset.field=field.name;var chips=document.createElement('div');chips.id='dscBoxChips';chips.className='ds-box-chips';var button=document.createElement('button');button.type='button';button.className='btn btn-o';button.id='dscBoxBtn';button.textContent='Configure Box credentials';button.addEventListener('click',openDscBoxDialog);wrap.append(span,input,chips,button);try{var parsed=JSON.parse(input.value||'');if(parsed&&(parsed.client_id||parsed.access_token||parsed.authorization_code)){DSC_BOX_CREDS=parsed;dscBoxRefreshChips()}}catch(error){}return wrap}function dscBoxRefreshChips(){var chips=document.getElementById('dscBoxChips');if(!chips)return;var authorized=!!(DSC_BOX_CREDS.access_token||DSC_BOX_CREDS.authorization_code);var configured=!!DSC_BOX_CREDS.client_id;chips.replaceChildren();if(authorized){var a=document.createElement('span');a.className='ds-box-chip ds-box-authorized';a.textContent='Authorized';chips.appendChild(a)}else if(configured){var c=document.createElement('span');c.className='ds-box-chip ds-box-configured';c.textContent='Configured';chips.appendChild(c)}var b=document.getElementById('dscBoxBtn');if(b)b.textContent=authorized||configured?'Get Box credentials':'Configure Box credentials'}function openDscBoxDialog(){document.getElementById('dscBoxClientId').value=DSC_BOX_CREDS.client_id||'';document.getElementById('dscBoxClientSecret').value=DSC_BOX_CREDS.client_secret||'';document.getElementById('dscBoxRedirect').value=DSC_BOX_CREDS.redirect_uri||'';document.getElementById('dscBoxMsg').hidden=true;document.getElementById('dscBoxRefresh').hidden=!window.DSC_BOX_FLOW;document.getElementById('dscBoxDialog').classList.add('show')}function closeDscBoxDialog(){document.getElementById('dscBoxDialog').classList.remove('show')}function dscBoxMsg(text,error){var message=document.getElementById('dscBoxMsg');message.hidden=false;message.textContent=text||'';message.style.color=error?'var(--d)':'var(--tm)';document.getElementById('dscBoxRefresh').hidden=!window.DSC_BOX_FLOW}function dscBoxSubmit(){var cid=document.getElementById('dscBoxClientId').value.trim();var csec=document.getElementById('dscBoxClientSecret').value.trim();var redir=document.getElementById('dscBoxRedirect').value.trim();if(!cid||!csec||!redir){dscBoxMsg('Please fill in Client ID, Client Secret, and Redirect URI.',true);return}var button=document.getElementById('dscBoxSubmit');button.disabled=true;fetch('/api/v1/connectors/box/oauth/web/start',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({client_id:cid,client_secret:csec,redirect_uri:redir})}).then(function(response){return response.json()}).then(function(data){if(data.code!==0)throw new Error(data.message||'Failed to start Box authorization.');window.DSC_BOX_FLOW=data.data.flow_id;DSC_BOX_CREDS={client_id:cid,client_secret:csec,redirect_uri:redir};var field=document.querySelector(\"#dsAddFields input[data-field='config.credentials.box_tokens']\");if(field)field.value=JSON.stringify(DSC_BOX_CREDS);dscBoxRefreshChips();dscBoxMsg('Complete the Box consent in the opened window and return here.',false);var popup=window.open(data.data.authorization_url,'ragflow-box-oauth','width=600,height=720');if(!popup){dscBoxMsg('Popup was blocked. Please allow popups for this site.',true);window.DSC_BOX_FLOW='';document.getElementById('dscBoxRefresh').hidden=true}}).catch(function(error){dscBoxMsg(error.message||'Failed to start Box authorization.',true)}).finally(function(){button.disabled=false})}function dscBoxPoll(){var flow=window.DSC_BOX_FLOW;if(!flow){dscBoxMsg('Start browser authorization first.',true);return}dscBoxMsg('Checking authorization status...',false);fetch('/api/v1/connectors/box/oauth/web/result',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({flow_id:flow})}).then(function(response){return response.json()}).then(function(data){if(data.code===0){var creds=data.data.credentials||{};var finalValue={client_id:creds.client_id||DSC_BOX_CREDS.client_id,client_secret:creds.client_secret||DSC_BOX_CREDS.client_secret,redirect_uri:creds.redirect_uri||DSC_BOX_CREDS.redirect_uri};if(creds.code)finalValue.authorization_code=creds.code;if(creds.access_token)finalValue.access_token=creds.access_token;if(creds.refresh_token)finalValue.refresh_token=creds.refresh_token;var field=document.querySelector(\"#dsAddFields input[data-field='config.credentials.box_tokens']\");if(field)field.value=JSON.stringify(finalValue);DSC_BOX_CREDS=finalValue;dscBoxRefreshChips();dscBoxMsg('Box authorization completed.',false);window.DSC_BOX_FLOW='';document.getElementById('dscBoxRefresh').hidden=true;closeDscBoxDialog()}else if(data.code===106){dscBoxMsg('Authorization is still pending.',false)}else{dscBoxMsg(data.message||'Authorization failed.',true)}}).catch(function(){dscBoxMsg('Unable to retrieve authorization result.',true)})}var DSC_GOOGLE_FLOW='';var DSC_GOOGLE_PENDING='';function dscBuildGoogleField(field){var wrap=document.createElement('div');wrap.className='provider-instance-field';var span=document.createElement('span');span.textContent=field.label+(field.required?' *':'');var input=document.createElement('input');input.type='hidden';input.dataset.field=field.name;var chips=document.createElement('div');chips.id='dscGoogleChips';chips.className='ds-box-chips';var file=document.createElement('input');file.type='file';file.accept='.json,application/json';file.id='dscGoogleFile';file.addEventListener('change',function(){dscGoogleFileSelected(file.files&&file.files[0])});var hint=document.createElement('p');hint.className='user-setting-description';hint.textContent='Upload your Google OAuth JSON file.';wrap.append(span,input,chips,file,hint);dscGoogleRefreshChips();return wrap}function dscGoogleChip(text,className){var chip=document.createElement('span');chip.className='ds-box-chip '+className;chip.textContent=text;return chip}function dscGoogleRefreshChips(){var chips=document.getElementById('dscGoogleChips');if(!chips)return;chips.replaceChildren();var value=(document.querySelector(\"#dsAddFields input[data-field='config.credentials.google_tokens']\")||{}).value||'';try{var parsed=JSON.parse(value);if(parsed&&parsed.refresh_token){chips.appendChild(dscGoogleChip('Verified','ds-box-authorized'))}else if(parsed&&(parsed.web||parsed.installed)){chips.appendChild(dscGoogleChip('Needs authorization','ds-box-pending'))}else if(value){chips.appendChild(dscGoogleChip('Stored','ds-box-configured'))}}catch(error){}}function dscGoogleFileSelected(file){if(!file)return;var reader=new FileReader();reader.onload=function(){var text=String(reader.result||'');try{var parsed=JSON.parse(text);var redirect='';var uris=parsed.web&&parsed.web.redirect_uris||parsed.installed&&parsed.installed.redirect_uris;if(Array.isArray(uris)&&uris.length)redirect=uris[0];if(parsed.refresh_token){var field=document.querySelector(\"#dsAddFields input[data-field='config.credentials.google_tokens']\");if(field)field.value=text;dscGoogleRefreshChips();dscBoxMsg?dscBoxMsg('OAuth credentials uploaded.',false):null;return}DSC_GOOGLE_PENDING=text;document.getElementById('dscGoogleRedirect').value=redirect||'';document.getElementById('dscGoogleMsg').hidden=true;document.getElementById('dscGoogleRefresh').hidden=true;document.getElementById('dscGoogleDialog').classList.add('show')}catch(error){dscBoxMsg('Invalid JSON file.',true)}};reader.readAsText(file)}function closeDscGoogleDialog(){document.getElementById('dscGoogleDialog').classList.remove('show')}function dscGoogleMsg(text,error){var message=document.getElementById('dscGoogleMsg');message.hidden=false;message.textContent=text||'';message.style.color=error?'var(--d)':'var(--tm)';document.getElementById('dscGoogleRefresh').hidden=!window.DSC_GOOGLE_FLOW}function dscGoogleAuthorize(){if(!DSC_GOOGLE_PENDING){dscGoogleMsg('No Google credential file detected.',true);return}var redir=document.getElementById('dscGoogleRedirect').value.trim();if(!redir){dscGoogleMsg('Please fill in Redirect URI.',true);return}var button=document.getElementById('dscGoogleSubmit');button.disabled=true;var type=window.DSC_TYPE==='gmail'?'gmail':'google-drive';fetch('/api/v1/connectors/google/oauth/web/start?type='+type,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({credentials:DSC_GOOGLE_PENDING,redirect_uri:redir})}).then(function(response){return response.json()}).then(function(data){if(data.code!==0)throw new Error(data.message||'Failed to start browser authorization.');window.DSC_GOOGLE_FLOW=data.data.flow_id;dscGoogleMsg('Complete the Google consent in the popup window.',false);var popup=window.open(data.data.authorization_url,type==='gmail'?'ragflow-gmail-oauth':'ragflow-google-drive-oauth','width=600,height=720');if(!popup){dscGoogleMsg('Popup was blocked. Please allow popups for this site.',true);window.DSC_GOOGLE_FLOW='';document.getElementById('dscGoogleRefresh').hidden=true}}).catch(function(error){dscGoogleMsg(error.message||'Failed to start browser authorization.',true)}).finally(function(){button.disabled=false})}function dscGooglePoll(){var flow=window.DSC_GOOGLE_FLOW;if(!flow){dscGoogleMsg('Start browser authorization first.',true);return}var type=window.DSC_TYPE==='gmail'?'gmail':'google-drive';dscGoogleMsg('Checking authorization status...',false);fetch('/api/v1/connectors/google/oauth/web/result?type='+type,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({flow_id:flow})}).then(function(response){return response.json()}).then(function(data){if(data.code===0){var field=document.querySelector(\"#dsAddFields input[data-field='config.credentials.google_tokens']\");if(field)field.value=data.data.credentials||'';dscGoogleRefreshChips();dscGoogleMsg('Google credentials verified.',false);window.DSC_GOOGLE_FLOW='';document.getElementById('dscGoogleRefresh').hidden=true;DSC_GOOGLE_PENDING='';closeDscGoogleDialog()}else if(data.code===106){dscGoogleMsg('Authorization is still pending.',false)}else{dscGoogleMsg(data.message||'Authorization failed.',true)}}).catch(function(){dscGoogleMsg('Unable to retrieve authorization result.',true)})}function dscFlatFields()",
        )
        .replace(
            "var kind=(document.getElementById('dsType').value||'').toUpperCase();",
            "var kind=(window.DSC_TYPE||'').toUpperCase();",
        )
        .replace(
            "var body={name:name,type:document.getElementById('dsType').value};",
            "var body={name:name,type:window.DSC_TYPE};",
        )
        .replace(
            "function openDsAdd(){var select=document.getElementById('dsType');document.getElementById('dsAddTitle').textContent='Add data source: '+select.selectedOptions[0].textContent;",
            "function openDsAdd(key,label){window.DSC_TYPE=key||'';document.getElementById('dsAddTitle').textContent='Create your '+(label||key||'')+' connector';",
        )
        .replace(
            "if(field.default_value&&!field.required===false){input.value=field.default_value}",
            "if(field.default_value)input.value=field.default_value;",
        )
        .replace(
            "if(field.default_value)input.value=field.default_value;input.addEventListener",
            "if(field.default_value){input.value=field.default_value}else if(!field.required){input.value=''};input.addEventListener",
        )
        .replace(
            "input.id='dsc'+label;",
            "input.id=label==='URL'?'dscUrl':label==='Target'?'dscTarget':'dscToken';",
        )
        .replace(
            "dscBoxMsg?dscBoxMsg('OAuth credentials uploaded.',false):null;return}",
            "return}",
        )
        .replace(
            "dscBoxMsg('Invalid JSON file.',true)",
            "alert('Invalid JSON file.')",
        )
        .replace(
            "var value=(document.querySelector(\"#dsAddFields input[data-field='config.credentials.google_tokens']\")||{}).value||'';try{",
            "var value=(document.querySelector(\"#dsAddFields input[data-field='config.credentials.google_tokens']\")||{}).value||'';if(!value&&window.DSC_GOOGLE_PENDING){chips.appendChild(dscGoogleChip('Uploaded (pending)','ds-box-pending'));return}try{",
        )
        .replace(
            "DSC_GOOGLE_PENDING=text;document.getElementById('dscGoogleRedirect')",
            "DSC_GOOGLE_PENDING=text;dscGoogleRefreshChips();document.getElementById('dscGoogleRedirect')",
        )
        .replace(
            "dscGoogleMsg('Google credentials verified.',false);",
            "dscGoogleMsg('Google credentials updated.',false);",
        );
    format!(
        "<style>.ds-segment{{display:inline-flex;flex-wrap:wrap;gap:2px;padding:3px;margin-top:8px;border:1px solid var(--bd);border-radius:var(--r);background:var(--bg);width:fit-content}}.ds-segment button{{border:0;border-radius:calc(var(--r) - 2px);padding:6px 12px;background:transparent;color:var(--t2);font-size:13px;cursor:pointer}}.ds-segment button[aria-checked=true]{{background:var(--p);color:#fff}}.provider-instance-field textarea{{width:100%;min-height:88px;resize:vertical}}.ds-field-tip{{display:block;margin-top:2px;font-size:11px;line-height:1.4;color:var(--tm)}}.ds-box-chips{{display:flex;gap:6px;margin-top:8px}}.ds-box-chip{{font-size:11px;font-weight:600;padding:2px 8px;border-radius:999px}}.ds-box-authorized{{background:#d1fae5;color:#065f46}}.ds-box-configured{{background:#dbeafe;color:#1e40af}}.ds-box-pending{{background:#fef3c7;color:#92400e}}</style>{overlay}{box_dialog}{google_dialog}{js}"
    )
}

/// Upstream-style data-source list surface: a grid of 35 Available cards,
/// grouped Added cards with Settings/Trash row actions, and the delete
/// confirmation dialog (`added-source-card.tsx` + `delete-source-modal.tsx`).
fn data_source_list_section(info_json: &str) -> String {
    let style = r#"<style>.ds-available-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(260px,1fr));gap:12px;margin-top:12px}.ds-available-icon{width:32px;height:32px;display:flex;align-items:center;justify-content:center;border-radius:8px;background:var(--nab);font-weight:700}.ds-available-icon img{width:26px;height:26px;object-fit:contain}.ds-added-card{margin-top:16px;border:1px solid var(--bd);border-radius:var(--rs);background:var(--bgc);padding:12px 16px}.ds-added-head{display:flex;align-items:center;gap:8px;margin:0 0 8px;font-size:16px;font-weight:500}.ds-added-row{display:flex;align-items:center;gap:12px;padding:10px 0;border-bottom:1px solid var(--bd)}.ds-added-row:last-child{border-bottom:0}.ds-added-name{flex:1;min-width:0;overflow-wrap:anywhere}.ds-added-actions{display:flex;align-items:center;gap:6px;flex-wrap:wrap}.ds-icon-btn{display:inline-flex;align-items:center;justify-content:center;width:30px;height:30px;border:0;border-radius:8px;background:transparent;color:var(--t2);cursor:pointer}.ds-icon-btn:hover{background:var(--bgch);color:var(--t)}.ds-icon-btn.ds-trash:hover{color:var(--d)}.ds-local-row{display:flex;gap:8px;align-items:flex-end}.ds-empty{color:var(--tm);padding:16px 0}</style>"#;
    let delete_modal = r#"<div class='modal-overlay' id='dsDeleteModal' data-testid='data-source-delete-modal' onclick='if(event.target===this)closeDsDelete()'><div class='modal' style='max-width:560px' role='alertdialog' aria-modal='true' aria-labelledby='dsDeleteTitle'><h2 id='dsDeleteTitle'>Delete data source</h2><p class='user-setting-description'>Are you sure you want to delete this data source link?</p><div id='dsDeleteBox' class='ds-delete-box'></div><div class='provider-instance-actions'><button class='btn btn-o' type='button' onclick='closeDsDelete()'>Cancel</button><button class='btn btn-d' id='dsDeleteOk' type='button' onclick='confirmDsDelete()'>Confirm</button></div></div></div>"#;
    let js = r#"<script>var DS_SOURCE_INFO=__INFO__;var DS_DELETE_ID='';function dsIconHtml(info){if(info&&info.icon&&info.icon.indexOf('.svg')>=0)return "<img src='/"+info.icon+"' alt='' aria-hidden='true'>";return info&&info.icon||'🔗'}function dsRenderCards(){var root=document.getElementById('dsAvailable');root.replaceChildren();Object.keys(DS_SOURCE_INFO).forEach(function(key){var info=DS_SOURCE_INFO[key];var card=document.createElement('article');card.className='cc-available-card';card.dataset.type=key;card.tabIndex=0;var icon=document.createElement('span');icon.className='cc-available-icon ds-available-icon';icon.innerHTML=dsIconHtml(info);var title=document.createElement('h3');title.textContent=info.name;var desc=document.createElement('p');desc.className='cc-available-desc ds-available-desc';desc.textContent=info.description||'';var add=document.createElement('button');add.className='cc-add-btn btn btn-p btn-sm';add.type='button';add.textContent='+ Add';add.addEventListener('click',function(event){event.stopPropagation();openDsAdd(key,info.name)});card.addEventListener('click',function(){openDsAdd(key,info.name)});card.append(icon,title,desc,add);root.appendChild(card)})}function dsGroupedRows(list){var groups={};list.forEach(function(item){(groups[item.type]=groups[item.type]||[]).push(item)});return groups}function loadDs(){fetch('/api/v1/data_sources').then(function(response){return response.json()}).then(function(data){var list=data.data||[];var root=document.getElementById('dsList');root.replaceChildren();if(!list.length){var empty=document.createElement('p');empty.className='ds-empty';empty.textContent='No data sources added yet.';root.appendChild(empty);return}var groups=dsGroupedRows(list);Object.keys(groups).forEach(function(type){var info=DS_SOURCE_INFO[type]||{name:type,icon:'🔗'};var card=document.createElement('div');card.className='ds-added-card';card.dataset.type=type;var head=document.createElement('h2');head.className='ds-added-head';var headIcon=document.createElement('span');headIcon.className='ds-available-icon';headIcon.innerHTML=dsIconHtml(info);head.appendChild(headIcon);var headName=document.createElement('span');headName.textContent=info.name;head.appendChild(headName);card.appendChild(head);groups[type].forEach(function(item){var row=document.createElement('div');row.className='ds-added-row';var name=document.createElement('span');name.className='ds-added-name';name.textContent=item.name;var badge=document.createElement('span');badge.className=item.connected?'badge bs':'badge bw';badge.textContent=item.connected?'connected':'disconnected';var actions=document.createElement('span');actions.className='ds-added-actions';var gear=document.createElement('button');gear.type='button';gear.className='ds-icon-btn';gear.title='Settings';gear.innerHTML='<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>';gear.addEventListener('click',function(){location.href='/user-setting/data-source/'+encodeURIComponent(item.id)});var trash=document.createElement('button');trash.type='button';trash.className='ds-icon-btn ds-trash';trash.title='Delete';trash.innerHTML='<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>';trash.addEventListener('click',function(){openDsDelete(item.id,item.name,item.type)});actions.append(document.createTextNode(' '),gear,document.createTextNode(' '),trash);row.append(name,badge,actions);card.appendChild(row)});root.appendChild(card)})})}function openDsDelete(id,name,type){DS_DELETE_ID=id;var info=DS_SOURCE_INFO[type]||{name:type,icon:'🔗'};var box=document.getElementById('dsDeleteBox');box.replaceChildren();var icon=document.createElement('span');icon.className='ds-available-icon';icon.innerHTML=dsIconHtml(info);var label=document.createElement('span');label.textContent=name;box.append(icon,label);document.getElementById('dsDeleteModal').classList.add('show')}function closeDsDelete(){document.getElementById('dsDeleteModal').classList.remove('show')}function confirmDsDelete(){fetch('/api/v1/data_sources/'+DS_DELETE_ID,{method:'DELETE'}).then(function(response){return response.json()}).then(function(data){if(data.code===0){closeDsDelete();loadDs()}else{alert(data.message||'delete failed')}})}window.addEventListener('message',function(event){var payload=event.data;if(!payload||typeof payload!=='object')return;var expected=window.DSC_TYPE==='gmail'?'ragflow-gmail-oauth':'ragflow-google-drive-oauth';if(payload.type!==expected)return;if(!payload.flowId||payload.flowId!==window.DSC_GOOGLE_FLOW)return;if(payload.status==='success'){dscGoogleMsg('Authorization confirmed. Finalizing tokens...',false);dscGooglePoll()}else{dscGoogleMsg(payload.message||'Authorization window reported an error.',true);window.DSC_GOOGLE_FLOW='';var r=document.getElementById('dscGoogleRefresh');if(r)r.hidden=true}});dsRenderCards();loadDs()</script>"#;
    format!("{style}{delete_modal}{}", js.replace("__INFO__", info_json))
}

pub async fn data_source_detail_page(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> axum::response::Response {
    let zh = detect_lang(req.headers());
    let Some(record) = state.data_sources.as_ref().and_then(|store| store.get(&id)) else {
        return Html(layout(
            t("Data source", zh),
            &format!(
                "<h1>404</h1><p>{}: {id}</p>",
                t("Data source not found", zh)
            ),
            zh,
        ))
        .into_response();
    };
    let schema = serde_json::to_string(&crate::data_source::form_fields::schema_for_ui())
        .unwrap_or_else(|_| "{{}}".to_string());
    let body = data_source_detail_body(&record.name, &id, &record.kind, &schema, zh);
    Html(user_setting_layout(
        t("Data source", zh),
        &body,
        UserSettingSection::DataSource,
        zh,
    ))
    .into_response()
}

pub async fn files_page(
    State(_state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let mut b = String::new();
    b.push_str("<h1>📁 ");
    b.push_str(t("Files", zh));
    b.push_str("</h1><div class='card'><h2>");
    b.push_str(t("Upload", zh));
    b.push_str("</h2><input type='file' id='fu' style='max-width:400px'><button class='btn btn-p' onclick='uploadF()'>");
    b.push_str(t("Upload", zh));
    b.push_str("</button></div><div class='card' id='fileBatchBar' style='display:none'><div style='display:flex;align-items:center;gap:8px;flex-wrap:wrap'><span id='fileSelCount' style='font-size:13px;color:var(--tm)'></span><button class='btn btn-d btn-sm' onclick='batchDelSel()'>");
    b.push_str(t("Bulk Delete", zh));
    b.push_str("</button><button class='btn btn-o btn-sm' onclick='batchLinkSel()'>");
    b.push_str(t("Link to KB", zh));
    b.push_str("</button><button class='btn btn-o btn-sm' onclick='fileSelAll(false);updateFileBatch()'>✕ ");
    b.push_str(t("Clear", zh));
    b.push_str("</button></div></div><div class='card'><h2>Files</h2><input id='fileQ' placeholder='Search files...' oninput='fileFilter()' style='max-width:260px;margin-bottom:8px'><div id='fl'>Loading...</div></div><script>function fileFilter(){var q=(document.getElementById('fileQ').value||'').toLowerCase();document.querySelectorAll('#fl tr').forEach(function(tr){var m=tr.getAttribute('data-name')||'';tr.style.display=(!q||m.indexOf(q)>=0)?'':'none'})};function updateFileBatch(){var n=document.querySelectorAll('.fileCk:checked').length;var bar=document.getElementById('fileBatchBar');if(bar){bar.style.display=n?'block':'none';var sc=document.getElementById('fileSelCount');if(sc)sc.textContent=n+' selected'}};function fileSelAll(on){document.querySelectorAll('.fileCk').forEach(function(x){x.checked=on});updateFileBatch()};document.addEventListener('change',function(e){if(e.target&&e.target.classList&&e.target.classList.contains('fileCk'))updateFileBatch()});function loadF(){fetch('/api/v1/files').then(r=>r.json()).then(d=>{var files=d.data||[];var h='<table><thead><tr><th><input type=checkbox onclick=fileSelAll(this.checked)></th><th>Name</th><th>Size</th><th>Type</th><th>Uploaded</th><th></th></tr></thead><tbody>';files.forEach(function(f){var d=new Date(f.created_at);var ds=d?d.toLocaleString():'';var isF=(f.file_type==='folder');h+='<tr data-name=\\''+f.name.toLowerCase()+'\\'><td><input type=checkbox class=fileCk value='+f.id+'></td><td style=font-weight:500>'+(isF?'📁 ':'')+f.name+'</td><td>'+(isF?'-</td><td class=co>':(f.size/1024).toFixed(0)+'KB</td><td class=co>')+(isF?'folder':f.file_type)+'</td><td style=color:var(--tm);font-size:12px>'+ds+'</td><td>'+(isF?'<button class=btn btn-d btn-sm onclick=delFile('+f.id+')>Del</button>':'<a class=btn btn-o btn-sm href=/api/v1/files/'+f.id+'/download target=_blank>Download</a> <button class=btn btn-o btn-sm onclick=linkFile('+f.id+') title=Link>🔗 KB</button> <button class=btn btn-o btn-sm onclick=renameF('+f.id+',&quot;'+f.name.replace(/\"/g,'')+'&quot;)>Rename</button> <button class=btn btn-o btn-sm onclick=copyF('+f.id+')>Copy</button> ')+'</td></tr>';});document.getElementById('fl').innerHTML=h+'</tbody></table>';updateFileBatch()})};function linkFile(id){var kb=prompt('Link file to knowledge base (dataset ID):');if(!kb)return;fetch('/api/v1/file2document',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({file_ids:[id],dataset_id:kb})}).then(r=>r.json()).then(d=>{alert(d.code===0?('Linked: '+(d.message||'ok')):('Error: '+(d.message||'unknown')))})};function selFileIds(){var ids=[];document.querySelectorAll('.fileCk:checked').forEach(function(x){ids.push(x.value)});return ids};function batchDelSel(){var ids=selFileIds();if(!ids.length){alert('请先选择文件');return}if(!confirm('Delete '+ids.length+' selected file(s)?'))return;var n=0;function next(){if(n>=ids.length){loadF();return}var id=ids[n++];fetch('/api/v1/files/'+id,{method:'DELETE'}).then(function(){next()}).catch(function(){next()})}next()};function batchLinkSel(){var ids=selFileIds();if(!ids.length){alert('请先选择文件');return}var kb=prompt('Link '+ids.length+' file(s) to knowledge base (dataset ID):');if(!kb)return;fetch('/api/v1/file2document',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({file_ids:ids,dataset_id:kb})}).then(r=>r.json()).then(d=>{alert(d.code===0?('Linked: '+(d.message||'ok')):('Error: '+(d.message||'unknown')))})};function renameF(id,cur){var n=prompt('New file name:',cur);if(!n)return;fetch('/api/v1/files/'+id,{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n})}).then(r=>r.json()).then(d=>{if(d.code===0){loadF()}else{alert('Error: '+(d.message||'unknown'))}})};function copyF(id){fetch('/api/v1/files/'+id+'/copy',{method:'POST'}).then(r=>r.json()).then(d=>{if(d.code===0){loadF()}else{alert('Error: '+(d.message||'unknown'))}})};function delFile(id){if(!confirm('Delete this file?'))return;fetch('/api/v1/files/'+id,{method:'DELETE'}).then(r=>r.json()).then(d=>{if(d.code===0){loadF()}else{alert('Error: '+(d.message||'unknown'))}})};function uploadF(){var f=document.getElementById('fu').files[0];if(!f)return;var fd=new FormData();fd.append('file',f,f.name);fetch('/api/v1/files/upload',{method:'POST',body:fd}).then(function(){document.getElementById('fu').value='';loadF()});}function afItem(n){var m=document.getElementById('addFileMenu');m.classList.remove('open');if(n===1){document.getElementById('fu').click()}else{document.getElementById('folderName').value='';document.getElementById('newFolderModal').classList.add('show')}};function createFolder(){var n=document.getElementById('folderName').value.trim();if(!n){alert('Folder name is required');return}fetch('/api/v1/files/folder',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n})}).then(r=>r.json()).then(d=>{if(d.code===0){document.getElementById('newFolderModal').classList.remove('show');loadF()}else{alert('Error: '+(d.message||'unknown'))}});}document.getElementById('addFileBtn').addEventListener('click',function(e){e.stopPropagation();document.getElementById('addFileMenu').classList.toggle('open')});document.addEventListener('click',function(e){var m=document.getElementById('addFileMenu');if(m&&!m.contains(e.target)&&e.target.id!=='addFileBtn'){m.classList.remove('open')}});loadF()</script>");
    Html(layout(t("Files", zh), &b, zh))
}
pub async fn memories_page(
    State(state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let convs = state.conversations.list();
    let mut b = String::new();
    b.push_str("<h1>🧠 ");
    b.push_str(t("Memories", zh));
    b.push_str("</h1><div style='display:flex;align-items:center;gap:12px;margin-bottom:16px'><button class='btn btn-p' onclick='showMemModal()'>+ ");
    b.push_str(t("Create Memory", zh));
    b.push_str("</button><button class='btn btn-d' onclick='clearAllMem()'>🗑 ");
    b.push_str(t("Clear All", zh));
    b.push_str("</button></div><div class='card'><h2>Memory Apps <span style='font-size:12px;color:var(--tm)' id='memCount'></span></h2><div id='memList'>Loading...</div></div>");
    // Add / edit memory modal (RAGFlow memory fields: name / memory_type / embd / llm)
    b.push_str(
        "<div class='modal-overlay' id='memModal'><div class='modal'><h2 id='memModalTitle'>",
    );
    b.push_str(t("Create Memory", zh));
    b.push_str("</h2><input id='mname' placeholder='Name'><label style='font-size:12px;display:block;margin:8px 0 4px'>Memory Type</label><div style='font-size:12px;margin-bottom:4px'><a href='javascript:void(0)' onclick=\"document.querySelectorAll('.mtype').forEach(function(x){x.checked=true})\" style='color:var(--p)'>Select all</a></div><div style='display:flex;gap:10px;flex-wrap:wrap;font-size:12px'><label><input type='checkbox' class='mtype' value='raw'> raw</label><label><input type='checkbox' class='mtype' value='semantic'> semantic</label><label><input type='checkbox' class='mtype' value='episodic'> episodic</label><label><input type='checkbox' class='mtype' value='procedural'> procedural</label></div><label style='font-size:12px;display:block;margin:8px 0 4px'>Embedding Model</label><select id='membd'><option value='default'>default</option></select><label style='font-size:12px;display:block;margin:8px 0 4px'>Chat Model</label><select id='mllm'><option value='default'>default</option></select><textarea id='mdesc' placeholder='Description' style='margin-top:8px;min-height:50px'></textarea><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='saveMem()'>");
    b.push_str(t("Save", zh));
    b.push_str("</button><button class='btn btn-o' onclick='closeMemModal()'>");
    b.push_str(t("Cancel", zh));
    b.push_str("</button></div></div></div>");
    b.push_str("<script>var memEditId=null;function showMemModal(){memEditId=null;document.getElementById('mname').value='';document.getElementById('mdesc').value='';document.querySelectorAll('.mtype').forEach(function(x){x.checked=false});document.getElementById('memModalTitle').textContent='Create Memory';document.getElementById('memModal').classList.add('show')};function closeMemModal(){document.getElementById('memModal').classList.remove('show')};function editMem(m){memEditId=m.id;document.getElementById('mname').value=m.name||'';document.getElementById('mdesc').value=m.description||'';document.querySelectorAll('.mtype').forEach(function(x){x.checked=(m.memory_type||[]).indexOf(x.value)>=0});if(m.embd_id)document.getElementById('membd').value=m.embd_id;if(m.llm_id)document.getElementById('mllm').value=m.llm_id;document.getElementById('memModalTitle').textContent='Edit Memory';document.getElementById('memModal').classList.add('show')};function saveMem(){var n=document.getElementById('mname').value;if(!n){alert('Name required');return}var types=[];document.querySelectorAll('.mtype:checked').forEach(function(x){types.push(x.value)});if(!types.length){types=['raw']}var body={name:n,memory_type:types,embd_id:document.getElementById('membd').value||'default',llm_id:document.getElementById('mllm').value||'default',description:document.getElementById('mdesc').value};var url=memEditId?'/api/v1/memories/'+memEditId:'/api/v1/memories';var method=memEditId?'PUT':'POST';fetch(url,{method:method,headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(r=>r.json()).then(d=>{if(d.code===0){closeMemModal();loadMems()}else{alert(d.message||'Save failed')}})};function delMem(id){if(!confirm('Delete this memory?'))return;fetch('/api/v1/memories/'+id,{method:'DELETE'}).then(r=>r.json()).then(d=>{if(d.code===0){loadMems()}else{alert(d.message||'Delete failed')}})};function clearAllMem(){if(!confirm('Delete ALL memories?'))return;fetch('/api/v1/memories/clear',{method:'DELETE'}).then(r=>r.json()).then(d=>{if(d.code===0){loadMems()}else{alert(d.message||'Clear failed')}})};function loadMems(){fetch('/api/v1/memories').then(r=>r.json()).then(d=>{var list=(d.data&&d.data.memory_list)||[];document.getElementById('memCount').textContent='('+list.length+')';var h='<table><thead><tr><th>Name</th><th>Type</th><th>Storage</th><th>Embedding</th><th>Chat</th><th>Size</th><th></th></tr></thead><tbody>';list.forEach(function(m){h+='<tr><td style=font-weight:500>'+(m.name||'')+'</td><td>'+(m.memory_type||[]).join(', ')+'</td><td class=co>'+(m.storage_type||'')+'</td><td class=co>'+(m.embd_id||'')+'</td><td class=co>'+(m.llm_id||'')+'</td><td>'+(m.memory_size||0)+'</td><td><button class=btn btn-o btn-sm onclick=\\'editMem('+JSON.stringify(m).replace(/'/g,'&#39;')+')\\' style=\\'font-size:12px\\'>✏️ Edit</button> <button class=btn btn-d btn-sm onclick=\\'delMem(\\\"'+m.id+'\\\")\\'>Del</button></td></tr>'});document.getElementById('memList').innerHTML=h+'</tbody></table>'||'<p style=color:var(--tm);padding:20px>No memories yet.</p>'})};function loadModelOpts(){fetch('/api/v1/providers').then(r=>r.json()).then(function(d){var ps=d.data||[];var e=document.getElementById('membd');var l=document.getElementById('mllm');(ps||[]).forEach(function(p){if(!p.models)return;(p.models||[]).forEach(function(m){var o=document.createElement('option');o.value=p.id+':'+m;o.textContent=m;e.appendChild(o);var o2=document.createElement('option');o2.value=p.id+':'+m;o2.textContent=m;l.appendChild(o2)})})}).catch(function(){})};loadMems();loadModelOpts();</script>");
    // Keep conversation list below for reference (RAGFlow memory page also links sessions)
    if !convs.is_empty() {
        let mut rows = String::new();
        for c in convs.iter().rev().take(10) {
            let msgs = c.messages.len();
            rows.push_str(&format!("<tr><td style='font-weight:500'>{}</td><td>{} msgs</td><td><a class='btn btn-o btn-sm' href='/chat/{}'>View</a></td></tr>", c.name.chars().take(40).collect::<String>(), msgs, c.id));
        }
        b.push_str("<div class='card' style='margin-top:16px'><h2>Recent Conversations</h2><table><thead><tr><th>Name</th><th>Messages</th><th></th></tr></thead><tbody>");
        b.push_str(&rows);
        b.push_str("</tbody></table></div>");
    }
    Html(layout(t("Memories", zh), &b, zh))
}

/// Fixed-version Memory card surface. Business data is deliberately loaded
/// through authenticated APIs rather than embedded in this public HTML shell.
pub async fn memories_page_v2(req: axum::http::Request<axum::body::Body>) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let mut b = String::new();
    b.push_str("<div class='memory-list-head'><div><h1 style='margin-bottom:4px'>");
    b.push_str(t("Memories", zh));
    b.push_str("</h1><p style='color:var(--tm)'>Build durable context for your agents.</p></div><button class='btn btn-p' id='memCreate'>＋ ");
    b.push_str(t("Create Memory", zh));
    b.push_str("</button></div><div class='memory-filter card'><input id='memSearch' placeholder='Search memories' aria-label='Search memories'><details class='memory-filter-popover' id='memOwner'><summary class='btn btn-o'>Owner <span id='memOwnerCount'></span></summary><div class='memory-filter-panel'><strong>Owner</strong><div class='memory-agent-options' id='memOwnerOptions'><span class='sub'>Loading...</span></div><div class='memory-modal-actions'><button class='btn btn-o btn-sm' id='memOwnerClear' type='button'>Clear</button><button class='btn btn-p btn-sm' id='memOwnerApply' type='button'>Apply</button></div></div></details><select id='memType' aria-label='Memory type'><option value=''>All types</option><option value='raw'>Raw</option><option value='semantic'>Semantic</option><option value='episodic'>Episodic</option><option value='procedural'>Procedural</option></select><select id='memStorage' aria-label='Storage type'><option value=''>All storage</option><option value='table'>Table</option></select></div><div id='memList' class='kb-grid' aria-live='polite'><p style='color:var(--tm)'>Loading...</p></div><div class='memory-pager' id='memPager'></div>");
    b.push_str("<div class='modal-overlay' id='memModal' role='dialog' aria-modal='true'><div class='modal'><h2 id='memModalTitle'>");
    b.push_str(t("Create Memory", zh));
    b.push_str("</h2><label>Name<input id='mname' maxlength='128'></label><div id='memCreateFields'><label>Memory Type</label><p class='sub'>Raw is required for the original dialogue envelope.</p><div class='memory-type-grid'><label><input type='checkbox' class='mtype' value='raw' checked disabled> Raw</label><label><input type='checkbox' class='mtype' value='semantic'> Semantic</label><label><input type='checkbox' class='mtype' value='episodic'> Episodic</label><label><input type='checkbox' class='mtype' value='procedural'> Procedural</label></div><label>Embedding Model<select id='membd'><option value='default'>default</option></select></label><label>Chat Model<select id='mllm'><option value='default'>default</option></select></label></div><div class='memory-modal-actions'><button class='btn btn-o' id='memCancel'>");
    b.push_str(t("Cancel", zh));
    b.push_str("</button><button class='btn btn-p' id='memSave'>");
    b.push_str(t("Save", zh));
    b.push_str("</button></div></div></div>");
    b.push_str(
        r#"<script>
const memState={page:1,size:12,total:0,edit:null,request:0,owners:new Set(),ownerOptions:new Map()};
const byId=(id)=>document.getElementById(id);
function node(tag,cls,text){const n=document.createElement(tag);if(cls)n.className=cls;if(text!==undefined)n.textContent=String(text);return n}
function openMemory(memory){memState.edit=memory||null;byId('memModalTitle').textContent=memory?'Edit Memory':'Create Memory';byId('mname').value=memory?(memory.name||''):'';byId('memCreateFields').hidden=!!memory;document.querySelectorAll('.mtype').forEach((item)=>{item.checked=item.value==='raw'});byId('memModal').classList.add('show');byId('mname').focus()}
function closeMemory(){byId('memModal').classList.remove('show');memState.edit=null}
async function saveMemory(){const name=byId('mname').value.trim();if(!name){alert('Name required');return}let url='/api/v1/memories',method='POST',body;if(memState.edit){url+='/'+encodeURIComponent(memState.edit.id);method='PUT';body={name}}else{const types=Array.from(document.querySelectorAll('.mtype:checked')).map((item)=>item.value);body={name,memory_type:types,embd_id:byId('membd').value||'default',llm_id:byId('mllm').value||'default',description:''}}const button=byId('memSave');button.disabled=true;try{const response=await fetch(url,{method,headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});const data=await response.json();if(data.code!==0)throw new Error(data.message||'Save failed');closeMemory();await Promise.all([loadMemories(),loadOwners()])}catch(error){alert(error.message||String(error))}finally{button.disabled=false}}
async function deleteMemory(memory){if(!confirm('Delete memory "'+(memory.name||'')+'"?\nAfter deletion, all messages in this memory are removed.'))return;const response=await fetch('/api/v1/memories/'+encodeURIComponent(memory.id),{method:'DELETE'});const data=await response.json();if(data.code!==0){alert(data.message||'Delete failed');return}await Promise.all([loadMemories(),loadOwners()])}
function renderMemory(memory){const card=node('article','kb-card');card.tabIndex=0;card.addEventListener('click',()=>location.href='/memory/memory-message/'+encodeURIComponent(memory.id));card.addEventListener('keydown',(event)=>{if(event.key==='Enter'||event.key===' '){event.preventDefault();card.click()}});card.appendChild(node('div','av-lg',(memory.name||'M').trim().charAt(0).toUpperCase()||'M'));card.appendChild(node('h3','',memory.name||'Memory'));card.appendChild(node('p','',memory.description||'No description'));const meta=node('div','memory-card-meta');meta.appendChild(node('span','badge bi',(memory.memory_type||[]).join(', ')||'raw'));meta.appendChild(node('span','sub',new Date(memory.created_at||Date.now()).toLocaleDateString()));card.appendChild(meta);const wrap=node('div','memory-more');const trigger=node('button','btn btn-o btn-sm','•••');trigger.type='button';trigger.title='More actions';trigger.setAttribute('aria-label','More actions');const menu=node('div','memory-menu');const rename=node('button','','Rename');rename.type='button';rename.addEventListener('click',(event)=>{event.stopPropagation();openMemory(memory)});const remove=node('button','danger','Delete');remove.type='button';remove.addEventListener('click',(event)=>{event.stopPropagation();deleteMemory(memory)});menu.append(rename,remove);trigger.addEventListener('click',(event)=>{event.stopPropagation();document.querySelectorAll('.memory-menu.show').forEach((item)=>{if(item!==menu)item.classList.remove('show')});menu.classList.toggle('show')});wrap.append(trigger,menu);card.appendChild(wrap);return card}
function renderMemoryPager(){const pager=byId('memPager');pager.replaceChildren();const pages=Math.max(1,Math.ceil(memState.total/memState.size));const prev=node('button','btn btn-o btn-sm','Previous');prev.disabled=memState.page<=1;prev.onclick=()=>{memState.page--;loadMemories()};const next=node('button','btn btn-o btn-sm','Next');next.disabled=memState.page>=pages;next.onclick=()=>{memState.page++;loadMemories()};pager.append(node('span','sub',memState.total+' memories'),prev,node('span','badge bi',memState.page+' / '+pages),next)}
async function loadMemories(){const generation=++memState.request;const params=new URLSearchParams({page:String(memState.page),page_size:String(memState.size)});const search=byId('memSearch').value.trim(),type=byId('memType').value,storage=byId('memStorage').value;if(search)params.set('keywords',search);if(memState.owners.size)params.set('owner_ids',Array.from(memState.owners).join(','));if(type)params.set('memory_type',type);if(storage)params.set('storage_type',storage);const list=byId('memList');list.replaceChildren(node('p','sub','Loading...'));try{const response=await fetch('/api/v1/memories?'+params);const data=await response.json();if(generation!==memState.request)return;if(data.code!==0)throw new Error(data.message||'Load failed');const memories=(data.data&&data.data.memory_list)||[];memState.total=(data.data&&data.data.total_count)||0;list.replaceChildren();if(!memories.length){const empty=node('div','card memory-empty');empty.append(node('div','memory-empty-icon','M'),node('h2','',search?'No matching memories':'Create your first memory'));const action=node('button','btn btn-p','+ Create Memory');action.onclick=()=>openMemory(null);empty.appendChild(action);list.appendChild(empty)}else{memories.forEach((memory)=>list.appendChild(renderMemory(memory)))}renderMemoryPager()}catch(error){list.replaceChildren(node('p','',error.message||String(error)))}}
function renderOwnerOptions(){const host=byId('memOwnerOptions');host.replaceChildren();for(const [id,info] of memState.ownerOptions){const label=node('label');const input=node('input');input.type='checkbox';input.value=id;input.checked=memState.owners.has(id);label.append(input,document.createTextNode(' '+info.label+' ('+info.count+')'));host.appendChild(label)}if(!memState.ownerOptions.size)host.appendChild(node('span','sub','No owners'));byId('memOwnerCount').textContent=memState.owners.size?'('+memState.owners.size+')':''}
async function loadOwners(){try{const response=await fetch('/api/v1/memories?page=1&page_size=200');const data=await response.json();if(data.code!==0)return;memState.ownerOptions.clear();for(const memory of ((data.data&&data.data.memory_list)||[])){const id=memory.tenant_id||'';if(!id)continue;const current=memState.ownerOptions.get(id)||{label:memory.owner_name||id,count:0};current.count++;memState.ownerOptions.set(id,current)}renderOwnerOptions()}catch(_error){}}
function appendModelOptions(selectId,models,types){const select=byId(selectId);const groups=new Map();const seen=new Set(Array.from(select.options).map((option)=>option.value));for(const model of models){const capabilities=Array.isArray(model.model_type)?model.model_type:[model.model_type];if(!capabilities.some((type)=>types.includes(type))||!model.selector||seen.has(model.selector))continue;seen.add(model.selector);const label=(model.provider||model.provider_id||'Provider')+' / '+(model.instance_name||model.instance_id||'default');let group=groups.get(label);if(!group){group=document.createElement('optgroup');group.label=label;groups.set(label,group);select.appendChild(group)}const option=node('option','',model.name||model.selector);option.value=model.selector;group.appendChild(option)}}
async function loadMemoryModels(){try{const [response,defaultsResponse]=await Promise.all([fetch('/api/v1/models'),fetch('/api/v1/models/default')]);const data=await response.json(),defaults=await defaultsResponse.json();if(data.code!==0)return;const models=data.data||[];appendModelOptions('membd',models,['embedding']);appendModelOptions('mllm',models,['chat','image2text']);for(const model of (((defaults.data||{}).models)||[])){if(!model.enable||!model.selector)continue;if(model.model_type==='embedding')byId('membd').value=model.selector;if(model.model_type==='chat'||model.model_type==='image2text')byId('mllm').value=model.selector}}catch(_error){}}
let memorySearchTimer;byId('memSearch').addEventListener('input',()=>{clearTimeout(memorySearchTimer);memorySearchTimer=setTimeout(()=>{memState.page=1;loadMemories()},180)});byId('memType').onchange=byId('memStorage').onchange=()=>{memState.page=1;loadMemories()};byId('memOwnerApply').onclick=()=>{memState.owners=new Set(Array.from(byId('memOwnerOptions').querySelectorAll('input:checked')).map((item)=>item.value));memState.page=1;byId('memOwner').open=false;renderOwnerOptions();loadMemories()};byId('memOwnerClear').onclick=()=>{memState.owners.clear();memState.page=1;byId('memOwner').open=false;renderOwnerOptions();loadMemories()};byId('memCreate').onclick=()=>openMemory(null);byId('memCancel').onclick=closeMemory;byId('memSave').onclick=saveMemory;byId('memModal').addEventListener('click',(event)=>{if(event.target===byId('memModal'))closeMemory()});document.addEventListener('click',()=>document.querySelectorAll('.memory-menu.show').forEach((menu)=>menu.classList.remove('show')));loadOwners();loadMemories();loadMemoryModels();
</script>"#,
    );
    Html(layout(t("Memories", zh), &b, zh))
}

/// `/memory/memory-message/:id` — the fixed Memory detail hierarchy and
/// message operations, rendered as a data-free Rust shell. All mutable or
/// tenant-scoped values enter the DOM via `textContent` after authenticated
/// API calls, which keeps stored message text out of HTML/JavaScript contexts.
pub async fn memory_message_page(
    Path(memory_id): Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let memory_id_json = json_script_string(&memory_id);
    let mut b = String::new();
    b.push_str("<a class='btn btn-o btn-sm' href='/memories' style='margin-bottom:12px'>← ");
    b.push_str(t("Back to Memories", zh));
    b.push_str("</a><section class='card memory-detail-card'><div class='ds-layout'><aside class='ds-sidebar'><div class='av-lg' id='memoryAvatar'>M</div><h3 id='memoryName'>Memory</h3><p class='stats' id='memoryDescription'></p><p class='stats' id='memoryCreated'></p><ul class='ds-nav'><li><a class='active' id='memoryMessagesLink' href='#'>");
    b.push_str(t("Messages", zh));
    b.push_str("</a></li><li><a id='memorySettingLink' href='#'>");
    b.push_str(t("Configuration", zh));
    b.push_str("</a></li></ul></aside><main class='memory-detail-main'><div class='memory-detail-title'><div><h1>");
    b.push_str(t("Memory Messages", zh));
    b.push_str("</h1><p class='sub'>");
    b.push_str(t(
        "Memory extract is configured with Prompts and Temperature from Advanced Settings.",
        zh,
    ));
    b.push_str("</p></div><span class='badge bi' id='memoryStorage'>—</span></div><div class='memory-message-tools'><input id='messageSearch' placeholder='");
    b.push_str(t("Search messages", zh));
    b.push_str("' aria-label='Search messages'><details class='memory-filter-popover' id='agentFilter'><summary class='btn btn-o'>");
    b.push_str(t("Agent", zh));
    b.push_str(
        " <span id='agentFilterCount'></span></summary><div class='memory-filter-panel'><strong>",
    );
    b.push_str(t("Agent", zh));
    b.push_str("</strong><div class='memory-agent-options' id='agentOptions'></div><div class='memory-modal-actions'><button class='btn btn-o btn-sm' id='agentClear'>Clear</button><button class='btn btn-p btn-sm' id='agentApply'>Apply</button></div></div></details></div><div class='memory-table-wrap'><table class='memory-message-table'><thead><tr><th><button class='memory-expand' id='expandAll' title='Expand or collapse all' aria-label='Expand or collapse all'>+</button> ");
    b.push_str(t("Session ID", zh));
    b.push_str("</th>");
    for heading in [
        "Agent",
        "Type",
        "Valid date",
        "Forget at",
        "Enable",
        "",
        "Action",
    ] {
        b.push_str("<th>");
        if !heading.is_empty() {
            b.push_str(t(heading, zh));
        }
        b.push_str("</th>");
    }
    b.push_str("</tr></thead><tbody id='messageRows'><tr><td colspan='8'>");
    b.push_str(t("Loading messages...", zh));
    b.push_str("</td></tr></tbody></table></div><div class='memory-pager' id='messagePager'></div></main></div></section>");
    b.push_str("<div class='modal-overlay' id='contentModal' role='dialog' aria-modal='true'><div class='modal' style='max-width:640px'><h2>");
    b.push_str(t("Content", zh));
    b.push_str("</h2><p class='sub' id='contentSession'></p><div class='memory-modal-content' id='messageContent'></div><div id='vectorArea' hidden><button class='btn btn-o btn-sm' id='copyVector'>");
    b.push_str(t("Content embed", zh));
    b.push_str(" ⧉</button><pre class='memory-vector' id='messageVector'></pre></div><div class='memory-modal-actions'><button class='btn btn-o' data-close='contentModal'>");
    b.push_str(t("Close", zh));
    b.push_str("</button></div></div></div><div class='modal-overlay' id='forgetModal' role='dialog' aria-modal='true'><div class='modal'><h2>");
    b.push_str(t("Forget message", zh));
    b.push_str("</h2><p>");
    b.push_str(t("Are you sure you want to forget?", zh));
    b.push_str("</p><p id='forgetSession' style='font-weight:600;margin-top:10px'></p><p class='sub' style='color:var(--d);margin-top:8px'>");
    b.push_str(t(
        "After forgetting, this message will not be retrieved by agents.",
        zh,
    ));
    b.push_str(
        "</p><div class='memory-modal-actions'><button class='btn btn-o' data-close='forgetModal'>",
    );
    b.push_str(t("Cancel", zh));
    b.push_str("</button><button class='btn btn-d' id='confirmForget'>");
    b.push_str(t("Forget", zh));
    b.push_str("</button></div></div></div><div class='modal-overlay' id='taskModal' role='dialog' aria-modal='true'><div class='modal'><h2>Task Log</h2><div id='taskLog'></div><div class='memory-modal-actions'><button class='btn btn-o' data-close='taskModal'>");
    b.push_str(t("Close", zh));
    b.push_str(
        "</button></div></div></div><div id='memoryToast' class='memory-toast' hidden></div>",
    );
    b.push_str("<script>const memoryId=");
    b.push_str(&memory_id_json);
    b.push_str(
        r#";
const byId=(id)=>document.getElementById(id);const allowedSizes=[10,20,50,100];const initial=new URLSearchParams(location.search);const initialAgents=initial.getAll('agent_id');
const messageState={page:Math.max(1,Number(initial.get('page'))||1),size:allowedSizes.includes(Number(initial.get('size')))?Number(initial.get('size')):50,keywords:initial.get('keywords')||'',agents:new Set(initialAgents.length===1&&initialAgents[0].includes(',')?initialAgents[0].split(',').filter(Boolean):initialAgents),total:0,rows:[],expanded:new Set(),request:0,controller:null,forget:null,content:null};
function node(tag,cls,text){const n=document.createElement(tag);if(cls)n.className=cls;if(text!==undefined)n.textContent=String(text);return n}
function showToast(text){const toast=byId('memoryToast');toast.textContent=text;toast.hidden=false;clearTimeout(showToast.timer);showToast.timer=setTimeout(()=>toast.hidden=true,1600)}
function openModal(id){byId(id).classList.add('show')}function closeModal(id){byId(id).classList.remove('show')}
function messageRef(message){return encodeURIComponent(memoryId)+':'+encodeURIComponent(String(message.message_id))}
function forgotten(message){return message.forget_at!==null&&message.forget_at!==undefined&&message.forget_at!=='None'&&String(message.forget_at).length>0}
function typeLabel(value){const labels={raw:'Raw',semantic:'Semantic',episodic:'Episodic',procedural:'Procedural'};return labels[value]||value||''}
function taskDisplayState(task){const status=String(task&&task.status||'').toLowerCase();const progress=Number(task&&task.progress);if(status==='failed'||status==='cancelled'||progress<0)return'failed';if(status==='done'||progress>=1)return'done';if(status==='running'||status==='processing'||(progress>0&&progress<1))return'running';return'failed'}
function taskStartTime(value){if(value===null||value===undefined||value===''||value===0||value==='0')return'—';const started=new Date(value);return Number.isNaN(started.getTime())?'—':started.toLocaleString()}
async function loadConfig(){try{const response=await fetch('/api/v1/memories/'+encodeURIComponent(memoryId)+'/config');const data=await response.json();if(data.code!==0)throw new Error(data.message||'Memory not found');const memory=data.data||{};byId('memoryName').textContent=memory.name||'Memory';byId('memoryDescription').textContent=memory.description||'';byId('memoryCreated').textContent='Created '+new Date(memory.created_at||Date.now()).toLocaleDateString();byId('memoryAvatar').textContent=(memory.name||'M').trim().charAt(0).toUpperCase()||'M';byId('memoryMessagesLink').href='/memory/memory-message/'+encodeURIComponent(memoryId);byId('memorySettingLink').href='/memory/memory-setting/'+encodeURIComponent(memoryId)}catch(error){byId('memoryName').textContent=error.message||String(error)}}
function syncUrl(){const params=new URLSearchParams();if(messageState.keywords)params.set('keywords',messageState.keywords);for(const agent of messageState.agents)params.append('agent_id',agent);params.set('page',String(messageState.page));params.set('size',String(messageState.size));history.replaceState(null,'',location.pathname+'?'+params)}
function renderAgentFilter(rows){const counts=new Map();for(const message of rows){const id=message.agent_id||'';if(!id)continue;const current=counts.get(id)||{name:message.agent_name||'Unknown',count:0};current.count++;counts.set(id,current)}for(const id of messageState.agents){if(!counts.has(id))counts.set(id,{name:id,count:0})}const options=byId('agentOptions');options.replaceChildren();if(!counts.size){options.appendChild(node('span','sub','No agents on this page'))}for(const [id,info] of counts){const label=node('label');const checkbox=node('input');checkbox.type='checkbox';checkbox.value=id;checkbox.checked=messageState.agents.has(id);label.append(checkbox,document.createTextNode(' '+info.name+' ('+info.count+')'));options.appendChild(label)}byId('agentFilterCount').textContent=messageState.agents.size?'('+messageState.agents.size+')':''}
function actionButton(label,title,handler,danger){const button=node('button',danger?'btn btn-d btn-sm':'btn btn-o btn-sm',label);button.type='button';button.title=title;button.setAttribute('aria-label',title);button.onclick=handler;return button}
function setCells(row,message,isExtract,parentId){row.className=isExtract?'extract-row':'';if(isExtract){row.dataset.parent=parentId;row.hidden=!messageState.expanded.has(parentId)}if(forgotten(message))row.classList.add('forgotten');const sessionCell=node('td');if(!isExtract&&Array.isArray(message.extract)&&message.extract.length){const toggle=actionButton(messageState.expanded.has(parentId)?'-':'+','Expand extracted messages',()=>{if(messageState.expanded.has(parentId))messageState.expanded.delete(parentId);else messageState.expanded.add(parentId);renderRows()},false);toggle.className='memory-expand';sessionCell.appendChild(toggle)}else if(isExtract){sessionCell.appendChild(node('span','sub','> '))}sessionCell.appendChild(document.createTextNode(message.session_id||''));row.appendChild(sessionCell);row.appendChild(node('td','',message.agent_name||'Unknown'));row.appendChild(node('td','',typeLabel(message.message_type)));row.appendChild(node('td','',message.valid_at||''));row.appendChild(node('td','',message.forget_at&&message.forget_at!=='None'?message.forget_at:'—'));const statusCell=node('td');const status=node('input','memory-switch');status.type='checkbox';status.checked=!!message.status;status.disabled=forgotten(message);status.setAttribute('aria-label','Enable message');status.onchange=()=>updateStatus(message,status);statusCell.appendChild(status);row.appendChild(statusCell);const taskCell=node('td');if(message.task){const taskState=taskDisplayState(message.task);const taskButton=actionButton('','Open task log',()=>showTask(message.task),false);taskButton.className='memory-task-button';taskButton.dataset.taskStatus=taskState;const taskDot=node('span','memory-task-dot');taskDot.setAttribute('aria-hidden','true');taskButton.appendChild(taskDot);taskCell.appendChild(taskButton)}row.appendChild(taskCell);const actions=node('td');const group=node('div','memory-actions');group.append(actionButton('View','View content',()=>showContent(message),false));const forget=actionButton('Forget','Forget message',()=>askForget(message),true);forget.disabled=forgotten(message);group.append(forget);actions.appendChild(group);row.appendChild(actions)}
function renderRows(){const body=byId('messageRows');body.replaceChildren();if(!messageState.rows.length){const row=node('tr');const cell=node('td','', 'No messages yet');cell.colSpan=8;cell.style.textAlign='center';cell.style.padding='42px';row.appendChild(cell);body.appendChild(row);return}for(const message of messageState.rows){const parentId=String(message.message_id);const row=node('tr');setCells(row,message,false,parentId);body.appendChild(row);for(const extract of (message.extract||[])){const child=node('tr');setCells(child,extract,true,parentId);body.appendChild(child)}}}
function renderPager(){const pager=byId('messagePager');pager.replaceChildren();const pages=Math.max(1,Math.ceil(messageState.total/messageState.size));pager.appendChild(node('span','sub',messageState.total+' messages'));const size=node('select','memory-page-size');for(const value of allowedSizes){const option=node('option','',value+' / page');option.value=String(value);option.selected=value===messageState.size;size.appendChild(option)}size.onchange=()=>{messageState.size=Number(size.value);messageState.page=1;loadMessages()};pager.appendChild(size);const previous=actionButton('Previous','Previous page',()=>{messageState.page--;loadMessages()},false);previous.disabled=messageState.page<=1;pager.appendChild(previous);const start=Math.max(1,Math.min(messageState.page-2,pages-4));const end=Math.min(pages,start+4);for(let page=start;page<=end;page++){const button=actionButton(String(page),'Page '+page,()=>{messageState.page=page;loadMessages()},false);if(page===messageState.page)button.className='btn btn-p btn-sm';pager.appendChild(button)}const next=actionButton('Next','Next page',()=>{messageState.page++;loadMessages()},false);next.disabled=messageState.page>=pages;pager.appendChild(next)}
async function loadMessages(){syncUrl();const generation=++messageState.request;if(messageState.controller)messageState.controller.abort();messageState.controller=new AbortController();const params=new URLSearchParams({page:String(messageState.page),page_size:String(messageState.size)});if(messageState.keywords)params.set('keywords',messageState.keywords);for(const agent of messageState.agents)params.append('agent_id',agent);const body=byId('messageRows');body.replaceChildren();const loading=node('tr');const cell=node('td','', 'Loading messages...');cell.colSpan=8;loading.appendChild(cell);body.appendChild(loading);try{const response=await fetch('/api/v1/memories/'+encodeURIComponent(memoryId)+'?'+params,{signal:messageState.controller.signal});const data=await response.json();if(generation!==messageState.request)return;if(data.code!==0)throw new Error(data.message||'Load failed');const payload=data.data||{};messageState.rows=(payload.messages&&payload.messages.message_list)||[];messageState.total=(payload.messages&&payload.messages.total_count)||0;byId('memoryStorage').textContent=payload.storage_type||'—';renderAgentFilter(messageState.rows);renderRows();renderPager()}catch(error){if(error.name==='AbortError')return;body.replaceChildren();const row=node('tr');const errorCell=node('td','',error.message||String(error));errorCell.colSpan=8;row.appendChild(errorCell);body.appendChild(row)}}
async function updateStatus(message,input){input.disabled=true;const previous=!input.checked;try{const response=await fetch('/api/v1/messages/'+messageRef(message),{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({status:input.checked})});const data=await response.json();if(data.code!==0)throw new Error(data.message||'Update failed');showToast('Updated');await loadMessages()}catch(error){input.checked=previous;input.disabled=false;alert(error.message||String(error))}}
function askForget(message){messageState.forget=message;byId('forgetSession').textContent='Session ID: '+(message.session_id||'');openModal('forgetModal')}
async function forgetSelected(){const message=messageState.forget;if(!message)return;const button=byId('confirmForget');button.disabled=true;try{const response=await fetch('/api/v1/messages/'+messageRef(message),{method:'DELETE'});const data=await response.json();if(data.code!==0)throw new Error(data.message||'Forget failed');closeModal('forgetModal');showToast('Forgotten');await loadMessages()}catch(error){alert(error.message||String(error))}finally{button.disabled=false}}
async function showContent(message){messageState.content=message;byId('contentSession').textContent='Session ID: '+(message.session_id||'');byId('messageContent').textContent='Loading...';byId('vectorArea').hidden=true;byId('messageVector').textContent='';openModal('contentModal');try{const response=await fetch('/api/v1/messages/'+messageRef(message)+'/content');const data=await response.json();if(data.code!==0)throw new Error(data.message||'Load failed');const content=data.data||{};byId('messageContent').textContent=content.content||'';if(Array.isArray(content.content_embed)&&content.content_embed.length){byId('messageVector').textContent=JSON.stringify(content.content_embed);byId('vectorArea').hidden=false}}catch(error){byId('messageContent').textContent=error.message||String(error)}}
async function copyVector(){const value=byId('messageVector').textContent;try{await navigator.clipboard.writeText(value);showToast('Copied!')}catch(_error){const area=document.createElement('textarea');area.value=value;document.body.appendChild(area);area.select();document.execCommand('copy');area.remove();showToast('Copied!')}}
function showTask(task){const taskState=taskDisplayState(task);const statusLabels={done:'Done',running:'Running',failed:'Failed'};const log=byId('taskLog');log.replaceChildren();log.append(node('p','','Status: '+statusLabels[taskState]),node('p','sub','Started: '+taskStartTime(task&&task.create_time)),node('div','memory-modal-content',task&&task.progress_msg||''));openModal('taskModal')}
let searchTimer;byId('messageSearch').value=messageState.keywords;byId('messageSearch').addEventListener('input',(event)=>{clearTimeout(searchTimer);searchTimer=setTimeout(()=>{messageState.keywords=event.target.value.trim();messageState.page=1;loadMessages()},180)});byId('agentApply').onclick=()=>{messageState.agents=new Set(Array.from(byId('agentOptions').querySelectorAll('input:checked')).map((item)=>item.value));messageState.page=1;byId('agentFilter').open=false;loadMessages()};byId('agentClear').onclick=()=>{messageState.agents.clear();messageState.page=1;byId('agentFilter').open=false;loadMessages()};byId('expandAll').onclick=()=>{const expandable=messageState.rows.filter((row)=>(row.extract||[]).length).map((row)=>String(row.message_id));if(expandable.every((id)=>messageState.expanded.has(id)))messageState.expanded.clear();else expandable.forEach((id)=>messageState.expanded.add(id));renderRows()};byId('confirmForget').onclick=forgetSelected;byId('copyVector').onclick=copyVector;document.querySelectorAll('[data-close]').forEach((button)=>button.onclick=()=>closeModal(button.dataset.close));document.querySelectorAll('.modal-overlay').forEach((modal)=>modal.addEventListener('click',(event)=>{if(event.target===modal)closeModal(modal.id)}));loadConfig();loadMessages();
</script>"#,
    );
    Html(layout(t("Memory Messages", zh), &b, zh))
}

/// `/memory/memory-setting/:id` — fixed-version basic and advanced Memory
/// configuration. The shell contains no tenant data; configuration is fetched
/// through the authenticated API and written back with a single PUT.
pub async fn memory_setting_page(
    Path(memory_id): Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let memory_id_json = json_script_string(&memory_id);
    let mut b = String::new();
    b.push_str("<a class='btn btn-o btn-sm' href='/memories' style='margin-bottom:12px'>← ");
    b.push_str(t("Back to Memories", zh));
    b.push_str("</a><section class='card memory-detail-card'><div class='ds-layout'><aside class='ds-sidebar'><div class='av-lg' id='memoryAvatar'><span id='memoryAvatarInitial'>M</span><img id='memoryAvatarImage' alt='' hidden></div><h3 id='memoryName'>Memory</h3><p class='stats' id='memoryDescription'></p><p class='stats' id='memoryCreated'></p><ul class='ds-nav'><li><a id='memoryMessagesLink' href='#'>");
    b.push_str(t("Messages", zh));
    b.push_str("</a></li><li><a class='active' id='memorySettingLink' href='#'>");
    b.push_str(t("Configuration", zh));
    b.push_str("</a></li></ul></aside><main class='memory-detail-main'><div class='memory-detail-title'><div><h1>");
    b.push_str(t("Memory Configuration", zh));
    b.push_str("</h1><p class='sub'>Configure models, capacity and extraction behavior.</p></div></div><form id='memorySettingForm'><h2>");
    b.push_str(t("Basic settings", zh));
    b.push_str("</h2><div class='memory-setting-form'><label>");
    b.push_str(t("Name", zh));
    b.push_str("<input id='cfgName' maxlength='128' required></label><label>");
    b.push_str(t("Avatar", zh));
    b.push_str("<input id='cfgAvatar' type='file' accept='image/png,image/jpeg,image/webp,image/gif'></label><label class='wide'>");
    b.push_str(t("Description", zh));
    b.push_str("<textarea id='cfgDescription' maxlength='2000'></textarea></label><label>");
    b.push_str(t("Embedding Model", zh));
    b.push_str(" <span class='tip' tabindex='0' aria-label='");
    b.push_str(&html_escape(t(
        "Converts text into numerical vectors for meaning similarity search and memory retrieval.",
        zh,
    )));
    b.push_str("' data-tip='");
    b.push_str(&html_escape(t(
        "Converts text into numerical vectors for meaning similarity search and memory retrieval.",
        zh,
    )));
    b.push_str("'>?</span>");
    push_default_model_selector(&mut b, &MEMORY_EMBD_SELECTOR_FIELD, zh);
    b.push_str("</label><label>");
    b.push_str(t("LLM", zh));
    b.push_str(" <span class='tip' tabindex='0' aria-label='");
    b.push_str(&html_escape(t("Analyzes conversation content, extracts key information, and generates structured memory summaries.", zh)));
    b.push_str("' data-tip='");
    b.push_str(&html_escape(t("Analyzes conversation content, extracts key information, and generates structured memory summaries.", zh)));
    b.push_str("'>?</span>");
    push_default_model_selector(&mut b, &MEMORY_LLM_SELECTOR_FIELD, zh);
    b.push_str("</label><fieldset class='wide memory-fieldset'><legend>");
    b.push_str(t("Memory Type", zh));
    b.push_str("</legend><div class='memory-type-grid'><label><input type='checkbox' class='cfg-type' value='raw' disabled> Raw (required)</label><label><input type='checkbox' class='cfg-type' value='semantic'> Semantic</label><label><input type='checkbox' class='cfg-type' value='episodic'> Episodic</label><label><input type='checkbox' class='cfg-type' value='procedural'> Procedural</label></div><p class='sub' id='cfgLockHint'></p></fieldset><label class='wide'>");
    b.push_str(t("Memory capacity (bytes)", zh));
    b.push_str("<input id='cfgSize' type='number' min='1' max='10485760' step='1'><span class='sub'>Default 5242880 bytes; maximum 10485760 bytes.</span></label></div><details class='memory-advanced'><summary>");
    b.push_str(t("Advanced settings", zh));
    b.push_str("</summary><div class='memory-setting-form'><label>");
    b.push_str(t("Permissions", zh));
    b.push_str("<select id='cfgPermissions'><option value='me'>");
    b.push_str(t("Only me", zh));
    b.push_str("</option><option value='team'>");
    b.push_str(t("Team", zh));
    b.push_str("</option></select></label><label>");
    b.push_str(t("Storage type", zh));
    b.push_str("<select id='cfgStorage' disabled><option value='table'>Table</option></select></label><label>");
    b.push_str(t("Forgetting policy", zh));
    b.push_str("<select id='cfgPolicy'><option value='FIFO'>FIFO</option></select></label><label>");
    b.push_str(t("Temperature", zh));
    b.push_str("<span class='memory-range'><input id='cfgTemperature' type='range' min='0' max='1' step='0.01'><input id='cfgTemperatureNumber' type='number' min='0' max='1' step='0.01'></span></label><label class='wide'>");
    b.push_str(t("System prompt", zh));
    b.push_str("<textarea id='cfgSystemPrompt'></textarea></label><label class='wide'>");
    b.push_str(t("User prompt", zh));
    b.push_str("<textarea id='cfgUserPrompt'></textarea></label></div></details><p class='sub' id='configStatus' role='status'></p><div class='memory-modal-actions'><button class='btn btn-o' type='button' id='cfgReset'>");
    b.push_str(t("Reset", zh));
    b.push_str("</button><button class='btn btn-p' type='submit' id='cfgSave'>");
    b.push_str(t("Save", zh));
    b.push_str("</button></div></form></main></div></section><div id='memoryToast' class='memory-toast' hidden></div>");
    let memory_fields_payload = serde_json::json!([
        {"fieldId": "embd_id", "domId": "cfgEmbd", "updateType": "embd_id", "responseType": "embedding", "acceptedTypes": ["embedding"], "required": true, "placeholder": "Select model"},
        {"fieldId": "llm_id", "domId": "cfgLlm", "updateType": "llm_id", "responseType": "chat", "acceptedTypes": ["chat"], "required": true, "placeholder": "Select model"}
    ]);
    b.push_str("<script>const MODEL_FIELDS=");
    b.push_str(&json_script_value(&memory_fields_payload));
    b.push_str(DEFAULT_MODEL_TREE_JS_HEAD);
    b.push_str(DEFAULT_MODEL_TREE_JS_BODY);
    // memory-model-form keeps embd_id/llm_id in local form state (upstream
    // react-hook-form), so shadow the tenant-default persist hook.
    b.push_str(FORM_LOCAL_SAVE_MODEL_DEFAULT);
    b.push_str("function seedModelTrigger(type,value){var field=modelField(type);if(!field)return;var trigger=document.getElementById(field.domId);if(!trigger)return;trigger.dataset.value=String(value||'');trigger.dataset.savedValue=String(value||'');renderDefaultModelTrigger(field)}");
    b.push_str("async function loadMemoryModelCatalog(){try{var response=await fetch('/api/v1/models');var data=await response.json();if(!response.ok||data.code!==0)return;DEFAULT_MODEL_CATALOG=(data.data||[]).map(normalizeDefaultModel).filter(function(model){return model.selector});MODEL_FIELDS.forEach(function(field){var state=defaultModelState(field);state.expanded.clear();expandDefaultModelTree(field);renderDefaultModelTrigger(field);renderDefaultModelTree(field)})}catch(error){}}loadMemoryModelCatalog();");
    b.push_str("</script><script>const memoryId=");
    b.push_str(&memory_id_json);
    b.push_str(
        r#";
const byId=(id)=>document.getElementById(id);let avatarValue='';let configRequest=0;
function node(tag,text){const n=document.createElement(tag);if(text!==undefined)n.textContent=String(text);return n}
function showToast(text){const toast=byId('memoryToast');toast.textContent=text;toast.hidden=false;clearTimeout(showToast.timer);showToast.timer=setTimeout(()=>toast.hidden=true,1600)}
function setAvatar(memory){const name=(memory.name||'M').trim();byId('memoryAvatarInitial').textContent=name.charAt(0).toUpperCase()||'M';const image=byId('memoryAvatarImage');if(memory.avatar&&(memory.avatar.startsWith('data:image/')||/^https?:\/\//i.test(memory.avatar))){image.src=memory.avatar;image.hidden=false;byId('memoryAvatarInitial').hidden=true}else{image.removeAttribute('src');image.hidden=true;byId('memoryAvatarInitial').hidden=false}}
function populate(memory,messageCount){byId('memoryName').textContent=memory.name||'Memory';byId('memoryDescription').textContent=memory.description||'';byId('memoryCreated').textContent='Created '+new Date(memory.created_at||Date.now()).toLocaleDateString();byId('memoryMessagesLink').href='/memory/memory-message/'+encodeURIComponent(memoryId);byId('memorySettingLink').href='/memory/memory-setting/'+encodeURIComponent(memoryId);setAvatar(memory);avatarValue=memory.avatar||'';byId('cfgName').value=memory.name||'';byId('cfgDescription').value=memory.description||'';seedModelTrigger('embd_id',memory.embd_id||'default');seedModelTrigger('llm_id',memory.llm_id||'default');for(const checkbox of document.querySelectorAll('.cfg-type'))checkbox.checked=(memory.memory_type||[]).includes(checkbox.value);document.querySelector('.cfg-type[value="raw"]').checked=true;byId('cfgSize').value=String(Number(memory.memory_size)||5242880);byId('cfgPermissions').value=memory.permissions==='team'?'team':'me';byId('cfgStorage').value=memory.storage_type||'table';byId('cfgPolicy').value='FIFO';const temperature=Math.min(1,Math.max(0,Number.isFinite(Number(memory.temperature))?Number(memory.temperature):0.5));byId('cfgTemperature').value=String(temperature);byId('cfgTemperatureNumber').value=String(temperature);byId('cfgSystemPrompt').value=memory.system_prompt||'';byId('cfgUserPrompt').value=memory.user_prompt||'';const locked=messageCount>0;byId('cfgEmbd').disabled=locked;byId('cfgLlm').disabled=locked;for(const checkbox of document.querySelectorAll('.cfg-type'))checkbox.disabled=locked||checkbox.value==='raw';byId('cfgLockHint').textContent=locked?'Embedding and memory types are locked after messages are stored.':'Raw memory is required.'}
async function loadSetting(){const generation=++configRequest;byId('configStatus').textContent='Loading...';try{await loadMemoryModelCatalog();const [configResponse,messagesResponse]=await Promise.all([fetch('/api/v1/memories/'+encodeURIComponent(memoryId)+'/config'),fetch('/api/v1/memories/'+encodeURIComponent(memoryId)+'?page=1&page_size=1')]);const configData=await configResponse.json();const messagesData=await messagesResponse.json();if(generation!==configRequest)return;if(configData.code!==0)throw new Error(configData.message||'Memory not found');if(messagesData.code!==0)throw new Error(messagesData.message||'Messages unavailable');const count=Number(messagesData.data&&messagesData.data.messages&&messagesData.data.messages.total_count)||0;populate(configData.data||{},count);byId('configStatus').textContent=''}catch(error){byId('configStatus').textContent=error.message||String(error)}}
async function saveSetting(event){event.preventDefault();const name=byId('cfgName').value.trim();if(!name){byId('cfgName').focus();return}const memorySize=Number(byId('cfgSize').value);const temperature=Number(byId('cfgTemperatureNumber').value);if(!Number.isInteger(memorySize)||memorySize<1||memorySize>10485760){alert('Memory capacity must be an integer from 1 to 10485760 bytes.');return}if(!Number.isFinite(temperature)||temperature<0||temperature>1){alert('Temperature must be in range [0, 1].');return}const types=Array.from(document.querySelectorAll('.cfg-type:checked')).map((item)=>item.value);if(!types.includes('raw'))types.unshift('raw');const body={name,avatar:avatarValue,description:byId('cfgDescription').value,memory_size:memorySize,permissions:byId('cfgPermissions').value,forgetting_policy:'FIFO',temperature,llm_id:byId('cfgLlm').dataset.value||'default',system_prompt:byId('cfgSystemPrompt').value,user_prompt:byId('cfgUserPrompt').value};if(!byId('cfgEmbd').disabled)body.embd_id=byId('cfgEmbd').dataset.value||'default';if(!document.querySelector('.cfg-type[value="semantic"]').disabled)body.memory_type=types;const button=byId('cfgSave');button.disabled=true;byId('configStatus').textContent='Saving...';try{const response=await fetch('/api/v1/memories/'+encodeURIComponent(memoryId),{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});const data=await response.json();if(data.code!==0)throw new Error(data.message||'Save failed');showToast('Saved');await loadSetting()}catch(error){byId('configStatus').textContent=error.message||String(error)}finally{button.disabled=false}}
byId('cfgAvatar').onchange=(event)=>{const file=event.target.files&&event.target.files[0];if(!file)return;if(!file.type.startsWith('image/')||file.size>2*1024*1024){alert('Choose a PNG, JPEG, WebP or GIF image up to 2 MiB.');event.target.value='';return}const reader=new FileReader();reader.onload=()=>{avatarValue=String(reader.result||'');setAvatar({name:byId('cfgName').value,avatar:avatarValue})};reader.readAsDataURL(file)};byId('cfgName').oninput=()=>{if(!avatarValue)setAvatar({name:byId('cfgName').value,avatar:''})};byId('cfgTemperature').oninput=()=>byId('cfgTemperatureNumber').value=byId('cfgTemperature').value;byId('cfgTemperatureNumber').oninput=()=>byId('cfgTemperature').value=byId('cfgTemperatureNumber').value;byId('cfgReset').onclick=loadSetting;byId('memorySettingForm').onsubmit=saveSetting;loadSetting();
</script>"#,
    );
    Html(layout(t("Memory Configuration", zh), &b, zh))
}

pub async fn admin_page(
    State(state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let tasks = state.tasks.list();
    let tr: String = tasks
        .iter()
        .rev()
        .take(10)
        .map(|t| {
            format!(
                "<tr><td>{}</td><td><span class='badge {}'>{}</span></td><td>{}%</td></tr>",
                t.name,
                if t.status == "done" { "bs" } else { "bi" },
                t.status,
                (t.progress * 100.0) as u32
            )
        })
        .collect();
    let mut b = String::new();
    b.push_str("<h1>⚙️ ");
    b.push_str(t("Admin", zh));
    b.push_str("</h1><div class='card'><h2>System</h2><table><tr><td>Version</td><td class='co'>");
    b.push_str(&crate::build_info::version_line());
    b.push_str("</td></tr><tr><td>Index</td><td class='co'>");
    b.push_str(&state.index_path);
    b.push_str("</td></tr></table></div><div class='card'><h2>Tasks (");
    b.push_str(&tasks.len().to_string());
    b.push_str(
        ")</h2><table><thead><tr><th>Task</th><th>Status</th><th>%</th></tr></thead><tbody>",
    );
    b.push_str(&tr);
    b.push_str("</tbody></table></div>");
    b.push_str("<div class='card'><h2>System Variables</h2><p style='color:var(--tm);font-size:12px;margin-bottom:10px'>Durable RAGFlow-compatible variables. Sandbox changes take effect on the next execution; secrets are never returned in plaintext.</p><div style='overflow-x:auto'><table><thead><tr><th>Name</th><th>Type</th><th>Value</th><th></th></tr></thead><tbody id='sysVarRows'><tr><td colspan='4' style='color:var(--tm)'>Loading administrator settings…</td></tr></tbody></table></div><p id='sysVarMsg' style='font-size:12px;color:var(--tm);margin-top:8px'></p></div><script>function sysVarCell(tr,text,cls){var td=document.createElement('td');td.textContent=text;if(cls)td.className=cls;tr.appendChild(td);return td}function loadSysVars(){var rows=document.getElementById('sysVarRows');var msg=document.getElementById('sysVarMsg');fetch('/api/v1/system/variables').then(async function(r){var d=await r.json();if(!r.ok||d.code!==0)throw new Error(d.message||'Administrator access required');rows.textContent='';(d.data||[]).forEach(function(s){var tr=document.createElement('tr');sysVarCell(tr,s.name,'co');var type=sysVarCell(tr,'');var badge=document.createElement('span');badge.className='badge bw';badge.textContent=s.data_type;type.appendChild(badge);var value=sysVarCell(tr,'');var input=document.createElement('input');input.type=s.name==='mail.password'?'password':'text';input.value=s.value;input.style='width:100%;min-width:260px;font-family:monospace;font-size:12px';value.appendChild(input);var action=sysVarCell(tr,'');var button=document.createElement('button');button.className='btn btn-p btn-sm';button.textContent='Save';button.dataset.name=s.name;button.onclick=function(){saveSysVar(button,input)};action.appendChild(button);rows.appendChild(tr)});if(!(d.data||[]).length)rows.textContent='No variables'}).catch(function(e){rows.textContent='';var tr=document.createElement('tr');var td=sysVarCell(tr,e.message);td.colSpan=4;td.style.color='var(--d)';rows.appendChild(tr);msg.textContent='Sign in with an administrator account to manage system variables.'})}function saveSysVar(button,input){var name=button.dataset.name;var msg=document.getElementById('sysVarMsg');msg.textContent='Saving…';fetch('/api/v1/system/variables',{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({var_name:name,var_value:input.value})}).then(async function(r){var d=await r.json();if(!r.ok||d.code!==0)throw new Error(d.message||'Save failed');msg.textContent=name+' saved ✓';loadSysVars()}).catch(function(e){msg.textContent='Error: '+e.message})}loadSysVars()</script>");
    let users = state.users.list_users();
    let mut ur = String::new();
    for user in &users {
        ur.push_str(&format!(
            "<tr data-name={} data-email={}><td style='font-weight:500'>{}</td><td class='co'>{}</td><td><span class='badge {}'>{}</span></td><td><button class='btn btn-o btn-sm' onclick='resetPw(\"{}\")'>Reset PW</button> <button class='btn btn-d btn-sm' onclick='delUser(\"{}\")'>Del</button></td></tr>",
            user.nickname.to_lowercase(), user.email.to_lowercase(), user.nickname, user.email, if user.role == "admin" { "bs" } else { "bw" }, if user.role == "admin" { "admin" } else { "user" }, user.id, user.id
        ));
    }
    b.push_str("<div class='card'><h2>Users (");
    b.push_str(&users.len().to_string());
    b.push_str(") <button class='btn btn-p' style='margin-left:12px' onclick='showAddUser()'>+ ");
    b.push_str(t("Add User", zh));
    b.push_str("</button></h2><input id='userQ' placeholder='Search users...' oninput='userFilter()' style='max-width:260px;margin-bottom:8px'><table><thead><tr><th>Name</th><th>Email</th><th>Role</th><th style='width:200px'>Actions</th></tr></thead><tbody id='userTb'>");
    b.push_str(&ur);
    b.push_str("</tbody></table></div><div class='modal-overlay' id='addUserModal'><div class='modal'><h2>");
    b.push_str(t("Add User", zh));
    b.push_str("</h2><input id='uname' placeholder='Username'><input id='upass' type='password' placeholder='Password (min 6)' style='margin-top:8px'><select id='urole' style='margin-top:8px'><option value='normal'>Normal User</option><option value='admin'>Superuser</option></select><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='addUser()'>");
    b.push_str(t("Create", zh));
    b.push_str("</button><button class='btn btn-o' onclick='closeAddUser()'>");
    b.push_str(t("Cancel", zh));
    b.push_str("</button></div></div></div><script>function userFilter(){var q=(document.getElementById('userQ').value||'').toLowerCase();document.querySelectorAll('#userTb tr').forEach(function(tr){var m=tr.getAttribute('data-name')||'';var e=tr.getAttribute('data-email')||'';tr.style.display=(!q||m.indexOf(q)>=0||e.indexOf(q)>=0)?'':'none'});}function resetPw(id){var n=prompt('New password (min 6 chars):');if(!n)return;fetch('/api/v1/admin/users/'+id+'/password',{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({new_password:n})}).then(r=>r.json()).then(d=>{if(d.code===0){location.reload()}else{alert('Error: '+(d.message||'unknown'))}})};function delUser(id){if(!confirm('Delete this user?'))return;fetch('/api/v1/admin/users/'+id,{method:'DELETE'}).then(r=>r.json()).then(d=>{if(d.code===0){location.reload()}else{alert('Error: '+(d.message||'unknown'))}})};function showAddUser(){document.getElementById('addUserModal').classList.add('show')};function closeAddUser(){document.getElementById('addUserModal').classList.remove('show')};function addUser(){var u=document.getElementById('uname').value;var p=document.getElementById('upass').value;if(!u||!p){alert('Username and password required');return}var r=document.getElementById('urole').value;fetch('/api/v1/admin/users',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({username:u,password:p,role:r})}).then(r=>r.json()).then(d=>{if(d.code===0){location.reload()}else{alert('Error: '+(d.message||'unknown'))}})}</script>");
    Html(layout(t("Admin", zh), &b, zh))
}

pub async fn login_page(req: axum::http::Request<axum::body::Body>) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let b = "<div class='auth-card card'><h1 style='text-align:center;margin-bottom:24px'>🐠 RayRAG</h1><div id='loginForm'><input id='email' autocomplete='username' placeholder='Email'><input id='password' type='password' autocomplete='current-password' placeholder='Password'><button class='btn btn-p' onclick='login()' style='width:100%;margin-top:8px'>Login</button></div><div id='regForm' style='display:none'><input id='nickname' placeholder='Nickname'><input id='remail' placeholder='Email'><input id='rpassword' type='password' placeholder='Password (min 6)'> <button class='btn btn-p' onclick='register()' style='width:100%;margin-top:8px'>Register</button></div><button class='btn btn-o' onclick='toggleAuth()' style='width:100%;margin-top:12px' id='authToggle'>Register</button><div id='error' style='color:var(--d);margin-top:8px;font-size:13px'></div></div><script>function setTok(t){localStorage.setItem('rayrag_token',t);document.cookie='rayrag_token='+t+';path=/;max-age=86400'};function login(){var e=document.getElementById('email').value;var p=document.getElementById('password').value;fetch('/api/v1/auth/login',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({email:e,password:p})}).then(r=>r.json()).then(d=>{d.code===0?(setTok(d.data.access_token),location.href='/'):document.getElementById('error').textContent=d.message})};function register(){var n=document.getElementById('nickname').value;var e=document.getElementById('remail').value;var p=document.getElementById('rpassword').value;fetch('/api/v1/user/register',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({nickname:n,email:e,password:p})}).then(r=>r.json()).then(function(d){if(d.code===0){loginWith(e,p)}else{document.getElementById('error').textContent=d.message}})};function loginWith(e,p){fetch('/api/v1/auth/login',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({email:e,password:p})}).then(r=>r.json()).then(d=>{d.code===0?(setTok(d.data.access_token),location.href='/'):document.getElementById('error').textContent=d.message})};function toggleAuth(){var r=document.getElementById('regForm').style.display==='none';document.getElementById('regForm').style.display=r?'block':'none';document.getElementById('loginForm').style.display=r?'none':'block';document.getElementById('authToggle').textContent=r?'Login':'Register'}</script>";
    Html(layout_plain(t("Login", zh), b, zh))
}

pub async fn document_page(
    State(_state): State<Arc<AppState>>,
    axum::extract::Path((kb_id, doc_id)): axum::extract::Path<(String, String)>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let mut b = String::new();
    b.push_str("<div style='display:flex;align-items:center;gap:12px;flex-wrap:wrap'><a class='btn btn-o' href='/dataset/");
    b.push_str(&kb_id);
    b.push_str("'>← ");
    b.push_str(t("Back to Dataset", zh));
    b.push_str("</a><h1 style='margin:0;flex:1' id='docTitle'>📄 ...</h1><button class='btn btn-o' onclick='reparse()'>");
    b.push_str(t("Reparse", zh));
    b.push_str("</button><button class='btn btn-d' onclick='delDoc()'>");
    b.push_str(t("Delete", zh));
    b.push_str("</button><button class='btn btn-o' onclick='exportChunks()'>⬇ ");
    b.push_str(t("Export", zh));
    b.push_str("</button></div>");
    b.push_str("<div class='modal-overlay' id='chunkModal'><div class='modal' style='max-width:700px'><h2>");
    b.push_str(t("Edit Chunk", zh));
    b.push_str("</h2><textarea id='chunkContent' style='min-height:200px;width:100%'></textarea><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='saveChunk()'>");
    b.push_str(t("Save", zh));
    b.push_str("</button><button class='btn btn-o' onclick='document.getElementById(\"chunkModal\").classList.remove(\"show\")'>");
    b.push_str(t("Cancel", zh));
    b.push_str("</button></div></div></div>");
    b.push_str("<div class='st' style='margin-top:16px'><div class='sc'><span class='lbl'>");
    b.push_str(t("Chunks", zh));
    b.push_str("</span><span class='val' id='docChunks'>-</span></div><div class='sc'><span class='lbl'>Status</span><span class='val' id='docStatus' style='font-size:15px'>-</span></div></div>");
    b.push_str("<div class='card'><h2>");
    b.push_str(t("Chunks", zh));
    b.push_str(
        " <span id='cTotal' style='font-size:13px;color:var(--tm)'></span></h2><div id='cl'>",
    );
    b.push_str(t("Loading...", zh));
    b.push_str("</div></div>");
    b.push_str("<script>var kbId='");
    b.push_str(&kb_id);
    b.push_str("';var docId='");
    b.push_str(&doc_id);
    b.push_str("';function chunkPage(d){window.cPage+=d;loadChunks()};function loadChunks(){fetch('/api/v1/datasets/'+kbId+'/documents/'+docId+'/chunks').then(r=>r.json()).then(d=>{var data=d.data||{};var chunks=data.chunks||[];var doc=data.doc||{};if(doc.name)document.getElementById('docTitle').textContent='📄 '+doc.name;if(doc.run)document.getElementById('docStatus').textContent=doc.run;document.getElementById('docChunks').textContent=chunks.length;document.getElementById('cTotal').textContent='('+chunks.length+')';var h='';window.cPage=0;window.cPageSize=20;var pageData=chunks;function renderPage(){var h='';var start=window.cPage*window.cPageSize;var end=Math.min(start+window.cPageSize,pageData.length);pageData.slice(start,end).forEach(function(c,i){h+=\"<div class='ri'><span class='rs'>#\"+(start+i+1)+\" &middot; pos \"+(c.position||0)+\" &middot; \"+(c.token_count||0)+\" tok</span><div class='rc' style='white-space:pre-wrap'>\"+(c.content||'').substring(0,800)+\"</div><button class='btn btn-o btn-sm' style='margin-top:6px' onclick=\"editChunk('\"+c.id+'\"','\"+(c.content||'').substring(0,2000).replace(/\'/g,'')+'\"')\">Edit</button></div>\"});var pn='<div style=margin-top:12px>';if(window.cPage>0)pn+='<button class=btn btn-o btn-sm onclick=chunkPage(-1)>← Prev</button> ';pn+='<span style=color:var(--tm);font-size:12px> Page '+(window.cPage+1)+' / '+Math.max(1,Math.ceil(pageData.length/window.cPageSize))+' ('+pageData.length+' chunks) </span>';if(end<pageData.length)pn+='<button class=btn btn-o btn-sm onclick=chunkPage(1)>Next →</button>';pn+='</div>';h+=pn;document.getElementById('cl').innerHTML=h||'<p style=color:var(--tm);text-align:center;padding:40px>No chunks</p>'})};var editingChunk='';function editChunk(cid,content){editingChunk=cid;document.getElementById('chunkContent').value=content;document.getElementById('chunkModal').classList.add('show')};function saveChunk(){var content=document.getElementById('chunkContent').value;fetch('/api/v1/datasets/'+kbId+'/documents/'+docId+'/chunks/'+editingChunk,{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({content:content})}).then(function(r){return r.json()}).then(function(d){document.getElementById('chunkModal').classList.remove('show');if(d.code===0){location.reload()}else{alert('Error: '+(d.message||'unknown'))}})};function reparse(){if(!confirm('Reparse this document?'))return;fetch('/api/v1/datasets/'+kbId+'/documents/'+docId+'/reparse',{method:'POST'}).then(()=>location.reload())};function delDoc(){if(!confirm('Delete this document?'))return;fetch('/api/v1/datasets/'+kbId+'/documents/'+docId,{method:'DELETE'}).then(()=>location.href='/dataset/'+kbId)};function exportChunks(){fetch('/api/v1/datasets/'+kbId+'/documents/'+docId+'/chunks').then(r=>r.json()).then(function(d){var data=d.data||{};var chunks=data.chunks||[];var a=document.createElement('a');a.href='data:application/json;charset=utf-8,'+encodeURIComponent(JSON.stringify(chunks,null,2));a.download='doc_'+docId+'_chunks.json';a.click()})};loadChunks()</script>");
    Html(layout("Document", &b, zh))
}

pub async fn skills_page(
    State(_state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let mut b = String::new();
    b.push_str("<h1>");
    b.push_str(t("Skills", zh));
    b.push_str("</h1><p style='color:var(--tm)'>");
    b.push_str(t(
        "Index and search reusable skills by name, tags, description, and content.",
        zh,
    ));
    b.push_str("</p><div class='toolbar' style='display:flex;gap:8px;flex-wrap:wrap;align-items:center'><input id='skillSpace' value='default' placeholder='Space ID' style='max-width:180px'><input id='skillQuery' placeholder='Search skills by meaning...' style='min-width:260px;flex:1' onkeydown='if(event.key===&quot;Enter&quot;)searchSkills()'><button class='btn btn-p' onclick='searchSkills()'>Search</button><button class='btn btn-o' onclick='openSkillModal()'>+ Add Skill</button><button class='btn btn-o' onclick='openSkillConfig()'>⚙ Configure Search</button><button class='btn btn-o' onclick='reindexSkills()'>↻ Reindex All</button></div><p id='skillMeta' style='color:var(--tm);font-size:12px'></p><div id='skillList' class='kb-grid'><p style='color:var(--tm)'>Loading...</p></div>");
    b.push_str("<div class='modal-overlay' id='skillModal'><div class='modal' style='max-width:680px'><h2>Add Skill</h2><label>Skill ID</label><input id='skillId' placeholder='my-skill'><label>Name</label><input id='skillName' placeholder='My Skill'><label>Folder ID</label><input id='skillFolder' placeholder='optional folder id'><label>Version</label><input id='skillVersion' value='1.0.0'><label>Tags</label><input id='skillTags' placeholder='rag, rust, search'><label>Description</label><textarea id='skillDescription'></textarea><label>Content</label><textarea id='skillContent' style='min-height:160px'></textarea><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='indexSkill()'>Index</button><button class='btn btn-o' onclick='closeSkillModal()'>Cancel</button></div></div></div>");
    b.push_str("<div class='modal-overlay' id='skillConfigModal'><div class='modal' style='max-width:620px'><h2>");
    b.push_str(t("Skill Search Configuration", zh));
    b.push_str("</h2><label>");
    b.push_str(t("Embedding Model", zh));
    b.push_str("</label>");
    push_default_model_selector(&mut b, &SKILL_EMBD_SELECTOR_FIELD, zh);
    b.push_str("<label>Vector Similarity Weight (0–1)</label><input id='skillVectorWeight' type='number' min='0' max='1' step='0.05' value='0.3'><label>Similarity Threshold (0–1)</label><input id='skillThreshold' type='number' min='0' max='1' step='0.05' value='0.2'><label>");
    b.push_str(t("Top K Results", zh));
    b.push_str("</label><input id='skillTopK' type='number' min='1' value='10'><label><input id='skillContentEnabled' type='checkbox'> Include content in embedding text</label><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='saveSkillConfig()'>Save</button><button class='btn btn-o' onclick='document.getElementById(&quot;skillConfigModal&quot;).classList.remove(&quot;show&quot;)'>Cancel</button></div></div></div>");
    b.push_str("<script>const MODEL_FIELDS=");
    let skill_fields_payload = serde_json::json!([
        {
            "fieldId": "embd_id",
            "domId": "skillEmbd",
            "updateType": "embd_id",
            "responseType": "embedding",
            "acceptedTypes": ["embedding"],
            "required": true,
            "placeholder": "Select an embedding model",
        }
    ]);
    b.push_str(&json_script_value(&skill_fields_payload));
    b.push_str(DEFAULT_MODEL_TREE_JS_HEAD);
    b.push_str(DEFAULT_MODEL_TREE_JS_BODY);
    b.push_str("function normalizeDefaultModelStoredValues(){MODEL_FIELDS.forEach(function(field){var trigger=document.getElementById(field.domId);if(!trigger)return;var value=trigger.dataset.value||'';if(!value||parseDefaultModelValue(value))return;var parts=value.split('/');if(parts.length!==3)return;var match=DEFAULT_MODEL_CATALOG.find(function(item){return item.name===parts[2]&&item.instance_id===parts[1]&&item.provider_id===parts[0]});if(!match)return;trigger.dataset.value=match.selector;trigger.dataset.savedValue=match.selector;renderDefaultModelTrigger(field)})}async function saveModelDefault(type,nextValue){var field=modelField(type);if(!field)return;var trigger=document.getElementById(field.domId);if(!trigger)return;var state=defaultModelState(field);if(state.pending)return;var value=String(nextValue==null?'':nextValue).trim();if(value&&!defaultModelOption(field,value))return;var previous=trigger.dataset.savedValue||'';trigger.dataset.value=value;if(!value){trigger.dataset.savedValue='';renderDefaultModelTrigger(field);return}var parsed=parseDefaultModelValue(value);if(!parsed){trigger.dataset.value=previous;trigger.dataset.savedValue=previous;renderDefaultModelTrigger(field);return}trigger.dataset.savedValue=value;var typeMap={embd_id:'embedding'};var persistType=typeMap[type];if(!persistType){renderDefaultModelTrigger(field);return}state.pending=true;fetch('/api/v1/models/default',{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({model_type:persistType,model_provider:parsed.model_provider,model_instance:parsed.model_instance,model_name:parsed.model_name})}).then(function(response){return response.json()}).then(function(data){state.pending=false;if(data&&data.code===0){renderDefaultModelTrigger(field)}else{trigger.dataset.value=previous;trigger.dataset.savedValue=previous;renderDefaultModelTrigger(field)}}).catch(function(){state.pending=false;trigger.dataset.value=previous;trigger.dataset.savedValue=previous;renderDefaultModelTrigger(field)})}async function loadSkillModelCatalog(){try{var response=await fetch('/api/v1/models');var data=await response.json();if(!response.ok||data.code!==0)return;DEFAULT_MODEL_CATALOG=(data.data||[]).map(normalizeDefaultModel).filter(function(model){return model.selector});MODEL_FIELDS.forEach(function(field){var state=defaultModelState(field);state.expanded.clear();expandDefaultModelTree(field);renderDefaultModelTrigger(field);renderDefaultModelTree(field)});normalizeDefaultModelStoredValues()}catch(error){}}loadSkillModelCatalog();</script>");
    b.push_str("<script>var SKILL_EMBD_REQUIRED=");
    b.push_str(&json_script_string(t(
        "Please select an embedding model",
        zh,
    )));
    b.push_str(";</script>");
    b.push_str(r#"<script>
function skillSpace(){return document.getElementById('skillSpace').value.trim()||'default'}
function esc(v){return String(v==null?'':v).replace(/[&<>"']/g,function(c){return {'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]})}
function searchSkills(){var q=document.getElementById('skillQuery').value;fetch('/api/v1/skills/search',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({space_id:skillSpace(),query:q,page:1,page_size:50})}).then(r=>r.json()).then(function(d){if(d.code!==0){throw new Error(d.message||'Search failed')}var data=d.data||{},skills=data.skills||[];document.getElementById('skillMeta').textContent=(data.total||0)+' skills · '+(data.search_type||'keyword');var h='';skills.forEach(function(s){h+='<div class="kb-card"><div class="av-lg">'+esc((s.name||'S').charAt(0).toUpperCase())+'</div><h3>'+esc(s.name)+'</h3><p style="color:var(--tm);font-size:12px">'+esc(s.description||'')+'</p><p style="font-size:12px">'+(s.tags||[]).map(function(t){return '<span class="badge">'+esc(t)+'</span>'}).join(' ')+'</p><p style="color:var(--tm);font-size:11px">v'+esc(s.version||'1.0.0')+' · score '+Number(s.score||0).toFixed(3)+'</p><button class="btn btn-d btn-sm" onclick="deleteSkill('+JSON.stringify(s.skill_id)+')">Delete index</button></div>'});document.getElementById('skillList').innerHTML=h||'<p style="color:var(--tm)">No skills indexed in this space.</p>'}).catch(function(e){document.getElementById('skillList').innerHTML='<p style="color:var(--danger)">'+esc(e.message)+'</p>'})}
function openSkillModal(){document.getElementById('skillModal').classList.add('show')}
function closeSkillModal(){document.getElementById('skillModal').classList.remove('show')}
function indexSkill(){var tags=document.getElementById('skillTags').value.split(',').map(function(v){return v.trim()}).filter(Boolean);var body={space_id:skillSpace(),skills:[{id:document.getElementById('skillId').value,name:document.getElementById('skillName').value,folder_id:document.getElementById('skillFolder').value,version:document.getElementById('skillVersion').value,tags:tags,description:document.getElementById('skillDescription').value,content:document.getElementById('skillContent').value}]};fetch('/api/v1/skills/index',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(r=>r.json()).then(function(d){if(d.code!==0)throw new Error(d.message||'Index failed');closeSkillModal();searchSkills()}).catch(function(e){alert(e.message)})}
function deleteSkill(id){if(!confirm('Delete this skill index?'))return;fetch('/api/v1/skills/index?space_id='+encodeURIComponent(skillSpace())+'&skill_id='+encodeURIComponent(id),{method:'DELETE'}).then(r=>r.json()).then(function(d){if(d.code!==0)throw new Error(d.message||'Delete failed');searchSkills()}).catch(function(e){alert(e.message)})}
function openSkillConfig(){fetch('/api/v1/skills/config?space_id='+encodeURIComponent(skillSpace())).then(r=>r.json()).then(function(d){var c=d.data||{};var skillTrigger=document.getElementById('skillEmbd');skillTrigger.dataset.value=c.embd_id||'';skillTrigger.dataset.savedValue=skillTrigger.dataset.value;var skillField=modelField('embd_id');if(skillField){renderDefaultModelTrigger(skillField);renderDefaultModelTree(skillField)}normalizeDefaultModelStoredValues();document.getElementById('skillVectorWeight').value=c.vector_similarity_weight==null?0.3:c.vector_similarity_weight;document.getElementById('skillThreshold').value=c.similarity_threshold==null?0.2:c.similarity_threshold;document.getElementById('skillTopK').value=c.top_k||10;document.getElementById('skillContentEnabled').checked=!!(c.field_config&&c.field_config.content&&c.field_config.content.enabled);document.getElementById('skillConfigModal').classList.add('show')})}
function saveSkillConfig(){var skillVal=(document.getElementById('skillEmbd').dataset.value||'');if(!skillVal){alert(SKILL_EMBD_REQUIRED);return}var fields={name:{enabled:true,weight:3},tags:{enabled:true,weight:2},description:{enabled:true,weight:1},content:{enabled:document.getElementById('skillContentEnabled').checked,weight:.5}};var body={space_id:skillSpace(),embd_id:skillVal,vector_similarity_weight:Number(document.getElementById('skillVectorWeight').value),similarity_threshold:Number(document.getElementById('skillThreshold').value),top_k:Number(document.getElementById('skillTopK').value),field_config:fields};fetch('/api/v1/skills/config',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(r=>r.json()).then(function(d){if(d.code!==0)throw new Error(d.message||'Save failed');document.getElementById('skillConfigModal').classList.remove('show');searchSkills()}).catch(function(e){alert(e.message)})}
function reindexSkills(){if(!confirm('Re-embed all indexed skills in this space?'))return;fetch('/api/v1/skills/reindex',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({space_id:skillSpace()})}).then(r=>r.json()).then(function(d){if(d.code!==0)throw new Error(d.message||'Reindex failed');alert('Reindexed '+(d.data.indexed_count||0)+' skills · '+d.data.version);searchSkills()}).catch(function(e){alert(e.message)})}
searchSkills();
</script>"#);
    Html(layout(t("Skills", zh), &b, zh))
}

#[derive(Clone, Copy)]
struct DefaultModelField {
    field_id: &'static str,
    dom_id: &'static str,
    label: &'static str,
    update_type: &'static str,
    response_type: &'static str,
    accepted_types: &'static [&'static str],
    required: bool,
    tooltip: &'static str,
    placeholder: &'static str,
}

const DEFAULT_MODEL_FIELDS: [DefaultModelField; 6] = [
    DefaultModelField {
        field_id: "llm_id",
        dom_id: "dmLlm",
        label: "LLM",
        update_type: "chat",
        response_type: "chat",
        accepted_types: &["chat", "image2text"],
        required: true,
        tooltip: "The default LLM for each newly created dataset.",
        placeholder: "Please select",
    },
    DefaultModelField {
        field_id: "embd_id",
        dom_id: "dmEmbedding",
        label: "Embedding",
        update_type: "embedding",
        response_type: "embedding",
        accepted_types: &["embedding"],
        required: false,
        tooltip: "The default embedding model for each newly created dataset. If you cannot find an embedding model from the dropdown, check if you are using RAGFlow slim edition (which does not include embedding models) or check https://ragflow.io/docs/dev/supported_models to see if your model provider supports this model.",
        placeholder: "Please select",
    },
    DefaultModelField {
        field_id: "img2txt_id",
        dom_id: "dmVlm",
        label: "VLM",
        update_type: "vision",
        response_type: "image2text",
        accepted_types: &["image2text"],
        required: false,
        tooltip: "The default VLM for each newly created dataset. It describes a picture or video. If you cannot find a model from the dropdown, check https://ragflow.io/docs/dev/supported_models to see if your model provider supports this model.",
        placeholder: "Please select",
    },
    DefaultModelField {
        field_id: "asr_id",
        dom_id: "dmAsr",
        label: "ASR",
        update_type: "asr",
        response_type: "speech2text",
        accepted_types: &["speech2text"],
        required: false,
        tooltip: "The default ASR model for each newly created dataset. Use this model to translate voices to corresponding text.",
        placeholder: "Please select",
    },
    DefaultModelField {
        field_id: "rerank_id",
        dom_id: "dmRerank",
        label: "Rerank",
        update_type: "rerank",
        response_type: "rerank",
        accepted_types: &["rerank"],
        required: false,
        tooltip: "The default rerank model for reranking chunks. If you cannot find a model from the dropdown, check https://ragflow.io/docs/dev/supported_models to see if your model provider supports this model.",
        placeholder: "Please select",
    },
    DefaultModelField {
        field_id: "tts_id",
        dom_id: "dmTts",
        label: "TTS",
        update_type: "tts",
        response_type: "tts",
        accepted_types: &["tts"],
        required: false,
        tooltip: "The default text-to-speech model. If you cannot find a model from the dropdown, check https://ragflow.io/docs/dev/supported_models to see if your model provider supports this model.",
        placeholder: "Please select",
    },
];

fn provider_capability_tags(provider_id: &str, provider_name: &str) -> Vec<&'static str> {
    use crate::providers::{ProviderKind, provider_has_kind};
    [
        (ProviderKind::Chat, "chat"),
        (ProviderKind::Embedding, "embedding"),
        (ProviderKind::Rerank, "rerank"),
        (ProviderKind::TextToSpeech, "tts"),
        (ProviderKind::SpeechToText, "speech2text"),
        (ProviderKind::ImageToText, "image2text"),
        (ProviderKind::Ocr, "ocr"),
    ]
    .into_iter()
    .filter_map(|(kind, tag)| provider_has_kind(provider_id, provider_name, kind).then_some(tag))
    .collect()
}

/// Upstream `un-add-model.tsx::mapModelKey[tag] || tag`. Delegates to the
/// single `crate::providers` table so card badges and the filter pills can no
/// longer drift apart; unknown tags fall back to the raw key verbatim.
fn model_type_label(model_type: &str) -> &str {
    crate::providers::capability_short_label_from_str(model_type).unwrap_or(model_type)
}

/// Fixed v0.26.4 `LIST_MODEL_PROVIDERS`: these provider modals replace the
/// single model-name field with a remote, multi-select model picker.
fn provider_uses_model_picker(provider_name: &str) -> bool {
    matches!(
        provider_name,
        "Ollama"
            | "OpenRouter"
            | "VLLM"
            | "OpenAI-API-Compatible"
            | "LM-Studio"
            | "VolcEngine"
            | "Xinference"
            | "LocalAI"
            | "BaiduYiyan"
            | "New API"
    )
}

/// Fixed `FACTORIES_WITH_BASE_URL` plus Anthropic: these provider modals render
/// the Base url input (and, for SiliconFlow/Tongyi-Qianwen, its
/// default/intl options) even before a tenant instance exists.
fn provider_shows_base_url(provider_name: &str) -> bool {
    matches!(
        provider_name,
        "OpenAI" | "Azure-OpenAI" | "Tongyi-Qianwen" | "MiniMax" | "SILICONFLOW" | "Anthropic"
    )
}

/// Fixed `LocalLlmConfigs` declare API-Key optional for local/self-hosted
/// factories while keeping instance name, base URL and model selection.
fn provider_api_key_optional(provider_name: &str) -> bool {
    matches!(
        provider_name,
        "Ollama"
            | "Xinference"
            | "ModelScope"
            | "LocalAI"
            | "LM-Studio"
            | "OpenAI-API-Compatible"
            | "RAGcon"
            | "TogetherAI"
            | "Replicate"
            | "OpenRouter"
            | "HuggingFace"
            | "GPUStack"
            | "VLLM"
            | "New API"
    )
}

fn provider_api_key_url(provider_name: &str) -> Option<&'static str> {
    match provider_name {
        "Ollama" => Some("https://github.com/ollama/ollama"),
        "VLLM" => Some("https://docs.vllm.ai/en/latest/serving/openai_compatible_server.html"),
        "LM-Studio" => Some("https://lmstudio.ai/docs/developer/openai-compat"),
        "LocalAI" => Some("https://localai.io/features/openai-functions/"),
        "Xinference" => Some("https://inference.readthedocs.io/"),
        "New API" => Some("https://docs.newapi.pro/"),
        "OpenAI" => Some("https://platform.openai.com/api-keys"),
        "Anthropic" => Some("https://console.anthropic.com/settings/keys"),
        "Gemini" => Some("https://aistudio.google.com/app/apikey"),
        "DeepSeek" => Some("https://platform.deepseek.com/api_keys"),
        "Moonshot" => Some("https://platform.moonshot.cn/console/api-keys"),
        "Tongyi-Qianwen" => Some("https://dashscope.console.aliyun.com/apiKey"),
        "ZHIPU-AI" => Some("https://open.bigmodel.cn/usercenter/apikeys"),
        "xAI" => Some("https://x.ai/api/"),
        "HuggingFace" => Some("https://huggingface.co/settings/tokens"),
        "Mistral" => Some("https://console.mistral.ai/api-keys/"),
        "Cohere" => Some("https://dashboard.cohere.com/api-keys"),
        "BaiduYiyan" => Some("https://console.bce.baidu.com/qianfan/ais/console/apiKey"),
        "Meituan" => Some("https://longcat.chat/platform/api_keys"),
        "Bedrock" => {
            Some("https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-2#/users")
        }
        "Azure-OpenAI" => {
            Some("https://portal.azure.com/#create/Microsoft.CognitiveServicesOpenAI")
        }
        "OpenRouter" => Some("https://openrouter.ai/keys"),
        "XunFei Spark" => Some("https://console.xfyun.cn/services/cbm"),
        "MiniMax" => Some("https://platform.minimaxi.com/user-center/basic-information"),
        "Groq" => Some("https://console.groq.com/keys"),
        "NVIDIA" => Some("https://build.nvidia.com/settings/api-keys"),
        "SILICONFLOW" => Some("https://cloud.siliconflow.cn/account/ak"),
        "Replicate" => Some("https://replicate.com/account/api-tokens"),
        "VolcEngine" => Some("https://console.volcengine.com/ark"),
        "Jina" => Some("https://jina.ai/embeddings/"),
        "Tencent Hunyuan" => Some("https://console.cloud.tencent.com/hunyuan/api-key"),
        "Tencent Cloud" => Some("https://console.cloud.tencent.com/cam/capi"),
        "ModelScope" => Some("https://modelscope.cn/my/myaccesstoken"),
        "Google Cloud" => Some("https://console.cloud.google.com/apis/credentials"),
        "Fish Audio" => Some("https://fish.audio/app/api-keys/"),
        "GiteeAI" => Some("https://ai.gitee.com/dashboard/settings/tokens"),
        "StepFun" => Some("https://platform.stepfun.com/interface-key"),
        "BaiChuan" => Some("https://platform.baichuan-ai.com/console/apikey"),
        "PPIO" => Some("https://ppio.com/settings/key-management"),
        "Voyage AI" => Some("https://dash.voyageai.com/api-keys"),
        "TogetherAI" => Some("https://api.together.xyz/settings/api-keys"),
        "NovitaAI" => Some("https://novita.ai/dashboard/key"),
        "Upstage" => Some("https://console.upstage.ai/api-keys"),
        "CometAPI" => Some("https://api.cometapi.com/console/token"),
        "302.AI" => Some("https://302.ai/apis/list"),
        "DeerAPI" => Some("https://api.deerapi.com/token"),
        "TokenPony" => Some("https://www.tokenpony.cn/#/user/keys"),
        "DeepInfra" => Some("https://deepinfra.com/dash/api_keys"),
        "PaddleOCR" => Some("https://www.paddleocr.ai/latest/"),
        "SoMark" => Some("https://somark.tech/workbench/apikey"),
        "n1n" => Some("https://docs.n1n.ai"),
        "Avian" => Some("https://avian.io"),
        "Perplexity" => Some("https://docs.perplexity.ai/docs/embeddings/quickstart"),
        _ => None,
    }
}

/// Fixed v0.26.4 `LlmIcon` resolution: the `svgIcons` factories serve a
/// branded static SVG from `/assets/svg/llm/`, everyone else uses the bundled
/// iconfont symbol from `IconMap`, and unmapped names fall back to the
/// upstream `moxing-default` glyph. Dark-theme `-dark` variants and the
/// iconfont bright/dark pairs are intentionally rendered with the bright
/// variant in the server-rendered shell for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderIcon {
    Svg(&'static str),
    Font(&'static str),
}

fn provider_icon(provider_name: &str) -> ProviderIcon {
    let svg = match provider_name {
        "LocalAI" => Some("local-ai.svg"),
        "Gemini" => Some("gemini.svg"),
        "StepFun" => Some("stepfun.svg"),
        "MinerU" => Some("mineru-bright.svg"),
        "PaddleOCR" => Some("paddleocr.svg"),
        "n1n" => Some("n1n.svg"),
        "Avian" => Some("avian.svg"),
        "RAGcon" => Some("ragcon.svg"),
        "SoMark" => Some("somark.svg"),
        "New API" => Some("new-api.svg"),
        _ => None,
    };
    if let Some(path) = svg {
        return ProviderIcon::Svg(path);
    }
    let symbol = match provider_name {
        "Tongyi-Qianwen" => "tongyi-qianwen",
        "Moonshot" => "moonshot",
        "OpenAI" => "openai",
        "ZHIPU-AI" => "zhipu",
        "WenXinYiYan" => "wenxin",
        "Ollama" => "ollama",
        "Xinference" => "xinference",
        "ModelScope" => "modelscope",
        "DeepSeek" => "deepseek",
        "VolcEngine" => "volcengine",
        "BaiChuan" => "baichuan",
        "Jina" => "jina",
        "MiniMax" => "MiniMax",
        "Mistral" => "mistral",
        "Azure-OpenAI" => "azure",
        "Bedrock" => "bedrock",
        "Groq" => "groq-next",
        "OpenRouter" => "open-router",
        "NVIDIA" => "nvidia",
        "LM-Studio" => "lm-studio",
        "OpenAI-API-Compatible" => "openai-api",
        "Cohere" => "cohere",
        "TogetherAI" => "together-bright",
        "Upstage" => "upstage",
        "NovitaAI" => "novita-ai",
        "SILICONFLOW" => "siliconflow",
        "PPIO" => "ppio",
        "Replicate" => "replicate",
        "Tencent Hunyuan" => "hunyuan",
        "XunFei Spark" => "spark",
        "BaiduYiyan" => "wenxinyiyan",
        "Fish Audio" => "fish-audio-bright",
        "Tencent Cloud" => "tencent-cloud",
        "Anthropic" => "anthropic",
        "Voyage AI" => "voyage",
        "Google Cloud" => "google-cloud",
        "HuggingFace" => "huggingface",
        "GPUStack" => "gpustack",
        "VLLM" => "vllm",
        "GiteeAI" => "gitee-ai",
        "302.AI" => "ai302",
        "DeepInfra" => "deepinfra",
        "xAI" => "xai",
        "TokenPony" => "tokenpony",
        "Meituan" => "longcat-bright",
        "LongCat" => "longcat-bright",
        "CometAPI" => "cometapi",
        "DeerAPI" => "deerapi",
        "Jiekou.AI" => "jiekouai-bright",
        "Builtin" => "builtin",
        "Perplexity" => "perplexity-bright",
        _ => "moxing-default",
    };
    ProviderIcon::Font(symbol)
}

fn provider_icon_html(provider_name: &str, class: &str) -> String {
    match provider_icon(provider_name) {
        ProviderIcon::Svg(path) => format!(
            "<img class='{class} provider-brand-icon' src='/assets/svg/llm/{path}' alt='' aria-hidden='true'>"
        ),
        ProviderIcon::Font(symbol) => format!(
            "<svg class='{class} provider-brand-icon' aria-hidden='true' focusable='false'><use href='/icons.svg#icon-{symbol}'></use></svg>"
        ),
    }
}

/// Emit one RAGFlow default-model tree selector: trigger button, popover,
/// search input and the `role='tree'` container. Extracted from
/// `providers_page` so every page that upstream renders through
/// `ModelTreeSelect` can reuse the identical markup.
fn push_default_model_selector(b: &mut String, field: &DefaultModelField, zh: bool) {
    b.push_str("<div class='provider-default-field' data-default-model-root='");
    b.push_str(field.dom_id);
    b.push_str("'><label id='");
    b.push_str(field.dom_id);
    b.push_str("Label' for='");
    b.push_str(field.dom_id);
    b.push_str("'>");
    if !field.label.is_empty() {
        if field.required {
            b.push_str("<span style='color:var(--d)'>*</span>");
        }
        b.push_str(field.label);
    }
    if !field.tooltip.is_empty() {
        b.push_str(" <span class='tip' tabindex='0' aria-label='");
        b.push_str(&html_escape(field.tooltip));
        b.push_str("' data-tip='");
        b.push_str(&html_escape(field.tooltip));
        b.push_str("'>?</span>");
    }
    b.push_str("</label><div class='default-model-control'><button id='");
    b.push_str(field.dom_id);
    b.push_str("' type='button' class='default-model-trigger' aria-labelledby='");
    b.push_str(field.dom_id);
    b.push_str("Label ");
    b.push_str(field.dom_id);
    b.push_str("Value' aria-haspopup='dialog' aria-expanded='false' aria-controls='");
    b.push_str(field.dom_id);
    b.push_str("Popover' data-state='closed' data-testid='default-model-");
    b.push_str(field.field_id);
    b.push_str("' data-field-id='");
    b.push_str(field.field_id);
    b.push_str("' data-update-type='");
    b.push_str(field.update_type);
    b.push_str("' data-response-type='");
    b.push_str(field.response_type);
    b.push_str("' data-model-types='");
    b.push_str(&field.accepted_types.join(" "));
    b.push_str("' data-value='' data-saved-value='' onclick=\"toggleDefaultModelTree('");
    b.push_str(field.update_type);
    b.push_str("')\"");
    if field.required {
        b.push_str(" aria-required='true'");
    }
    b.push_str("><span id='");
    b.push_str(field.dom_id);
    b.push_str("Value' class='default-model-value'><span class='default-model-placeholder'>");
    b.push_str(t(field.placeholder, zh));
    b.push_str("</span></span><span id='");
    b.push_str(field.dom_id);
    b.push_str(
        "Indicator' class='default-model-indicator' aria-hidden='true'>⌄</span></button><div id='",
    );
    b.push_str(field.dom_id);
    b.push_str("Popover' class='default-model-popover' role='dialog' aria-label='");
    b.push_str(field.label);
    b.push_str(" model tree' hidden><div class='default-model-search-wrap'><span aria-hidden='true'>⌕</span><input id='");
    b.push_str(field.dom_id);
    b.push_str("Search' class='default-model-search' type='search' placeholder='Search' aria-label='Search ");
    b.push_str(field.label);
    b.push_str(" models' oninput=\"searchDefaultModelTree('");
    b.push_str(field.update_type);
    b.push_str("',this.value)\"></div><div id='");
    b.push_str(field.dom_id);
    b.push_str("Tree' class='default-model-tree' role='tree'></div></div></div></div>");
}

/// Shared head of the RAGFlow default-model tree behaviour script (upstream
/// `web/src/components/model-tree-select.tsx` + `tree-select.tsx`). The
/// leading `;` closes the page-local `const MODEL_FIELDS=` payload; every
/// page defines its own `MODEL_FIELDS` before including this script.
const DEFAULT_MODEL_TREE_JS_HEAD: &str = r#";
function modelField(type){return MODEL_FIELDS.find(function(field){return field.updateType===type})}
"#;

/// Shared body of the default-model tree behaviour script: value parsing,
/// per-field state, tree build/filter/render, popover open/close/search and
/// outside-pointerdown/Escape listeners. Pages must define the globals
/// `MODEL_FIELDS`, `DEFAULT_MODEL_CATALOG`, `saveModelDefault` (persist
/// hook) and a catalog loader (e.g. `loadModelOptions`) themselves.
const DEFAULT_MODEL_TREE_JS_BODY: &str = r#"function modelMessage(text,error){var message=document.getElementById('dmMsg');if(message){message.textContent=text||'';message.style.color=error?'var(--d)':'var(--tm)'}}function parseDefaultModelValue(value){var parts=String(value||'').split('@');if(parts.length<3)return null;var provider=parts.pop();var instance=parts.pop();var name=parts.join('@');if(!provider||!instance||!name)return null;return {model_provider:provider,model_instance:instance,model_name:name}}
var DEFAULT_MODEL_CATALOG=[];
var DEFAULT_MODEL_STATES={};
	function defaultModelState(field){if(!DEFAULT_MODEL_STATES[field.updateType])DEFAULT_MODEL_STATES[field.updateType]={search:'',expanded:new Set(),requestId:0,pending:false};return DEFAULT_MODEL_STATES[field.updateType]}
	function defaultModelSelector(model){var provider=String(model.provider_name||model.provider||model.model_provider||'');var instance=String(model.instance_name||model.model_instance||'');var name=String(model.name||model.model_name||'').split('__')[0];return name&&instance&&provider?[name,instance,provider].join('@'):''}
	function normalizeDefaultModel(model){return {name:String(model.name||model.model_name||''),model_type:Array.isArray(model.model_type)?model.model_type.slice():[],provider_name:String(model.provider_name||model.provider||model.model_provider||''),provider_id:String(model.provider_id||''),instance_id:String(model.instance_id||''),instance_name:String(model.instance_name||model.model_instance||''),selector:defaultModelSelector(model),max_tokens:model.max_tokens==null?null:Number(model.max_tokens)}}
	function defaultModelOption(field,value){return DEFAULT_MODEL_CATALOG.find(function(model){return model.selector===value&&field.acceptedTypes.some(function(type){return model.model_type.indexOf(type)>=0})})}
	function modelOptionExists(field,value){return !!defaultModelOption(field,value)}
function buildDefaultModelTree(field){var providers=new Map();var seen=new Set();DEFAULT_MODEL_CATALOG.forEach(function(model){if(!field.acceptedTypes.some(function(type){return model.model_type.indexOf(type)>=0}))return;var selector=model.selector;if(!selector||seen.has(selector))return;seen.add(selector);var instances=providers.get(model.provider_name);if(!instances){instances=new Map();providers.set(model.provider_name,instances)}var models=instances.get(model.instance_name);if(!models){models=[];instances.set(model.instance_name,models)}models.push({id:selector,title:String(model.name).split('__')[0],kind:'model',provider:model.provider_name,instance:model.instance_name,children:null})});return Array.from(providers.entries()).map(function(providerEntry){var provider=providerEntry[0],instances=providerEntry[1];return {id:provider,title:provider,kind:'provider',children:Array.from(instances.entries()).map(function(instanceEntry){var instance=instanceEntry[0],models=instanceEntry[1];return {id:provider+'||'+instance,title:instance,kind:'instance',children:models}})}})}
function expandDefaultModelTree(field,nodes){var state=defaultModelState(field);(nodes||buildDefaultModelTree(field)).forEach(function(node){if(node.children&&node.children.length){state.expanded.add(node.id);expandDefaultModelTree(field,node.children)}})}
function filterDefaultModelTree(nodes,term){if(!term)return nodes;var query=term.toLowerCase();return nodes.reduce(function(result,node){var children=node.children?filterDefaultModelTree(node.children,term):null;var match=node.title.toLowerCase().indexOf(query)>=0;if(match||children&&children.length){result.push(Object.assign({},node,{children:match&&node.children?node.children:children}))}return result},[])}
	function defaultModelTreeRow(field,node,level,searching){var state=defaultModelState(field);var branch=!!(node.children&&node.children.length);var expanded=branch&&(searching||state.expanded.has(node.id));var row=document.createElement('button');row.type='button';row.className='default-model-node '+(branch?'default-model-branch':'default-model-leaf');row.style.paddingLeft=(level*20+4)+'px';row.setAttribute('role','treeitem');if(branch){row.setAttribute('aria-expanded',String(expanded))}else{var selected=document.getElementById(field.domId).dataset.value===node.id;row.setAttribute('aria-selected',String(selected));if(selected)row.classList.add('selected')}var chevron=document.createElement('span');chevron.className='default-model-chevron';chevron.textContent=branch?(expanded?'⌄':'›'):'';row.appendChild(chevron);if(!branch){var icon=document.createElement('span');icon.className='default-model-leaf-icon';icon.textContent=(node.provider||'M').charAt(0).toUpperCase();row.appendChild(icon)}var title=document.createElement('span');title.className='default-model-node-title';title.textContent=node.title;row.appendChild(title);row.addEventListener('click',function(event){event.stopPropagation();if(branch){if(state.expanded.has(node.id))state.expanded.delete(node.id);else state.expanded.add(node.id);renderDefaultModelTree(field)}else{selectDefaultModel(field.updateType,node.id)}});var wrapper=document.createElement('div');wrapper.appendChild(row);if(branch&&expanded){var group=document.createElement('div');group.setAttribute('role','group');node.children.forEach(function(child){group.appendChild(defaultModelTreeRow(field,child,level+1,searching))});wrapper.appendChild(group)}return wrapper}
function renderDefaultModelTree(field){var tree=document.getElementById(field.domId+'Tree');if(!tree)return;var state=defaultModelState(field);var nodes=filterDefaultModelTree(buildDefaultModelTree(field),state.search);tree.replaceChildren();if(!nodes.length){var empty=document.createElement('div');empty.className='default-model-empty';empty.textContent='No data available';tree.appendChild(empty);return}nodes.forEach(function(node){tree.appendChild(defaultModelTreeRow(field,node,0,!!state.search))});var searchInput=document.getElementById(field.domId+'Search');if(searchInput)searchInput.onkeydown=function(event){if(event.key!=='ArrowDown'&&event.key!=='ArrowUp')return;event.preventDefault();var rows=Array.prototype.slice.call(tree.querySelectorAll('button.default-model-node'));if(!rows.length)return;var target=event.key==='ArrowDown'?rows[0]:rows[rows.length-1];target.focus();target.scrollIntoView({block:'nearest'})};tree.onkeydown=function(event){var rows=Array.prototype.slice.call(tree.querySelectorAll('button.default-model-node'));if(!rows.length)return;var idx=rows.indexOf(document.activeElement);var focusRow=function(row){row.focus();row.scrollIntoView({block:'nearest'})};if(event.key==='ArrowDown'){focusRow(rows[(idx+1)%rows.length]);event.preventDefault()}else if(event.key==='ArrowUp'){focusRow(rows[(idx-1+rows.length)%rows.length]);event.preventDefault()}else if(event.key==='Home'){focusRow(rows[0]);event.preventDefault()}else if(event.key==='End'){focusRow(rows[rows.length-1]);event.preventDefault()}else if(event.key.length===1&&/[a-z0-9]/i.test(event.key)){var ch=event.key.toLowerCase();var hit=-1;for(var i=0;i<rows.length;i++){var title=((rows[i].querySelector('.default-model-node-title')||rows[i]).textContent||'').toLowerCase();if(title.indexOf(ch)===0){hit=i;break}}if(hit>=0){focusRow(rows[hit]);event.preventDefault()}}}}
function renderDefaultModelTrigger(field){var trigger=document.getElementById(field.domId);var valueNode=document.getElementById(field.domId+'Value');var indicator=document.getElementById(field.domId+'Indicator');if(!trigger||!valueNode||!indicator)return;var value=trigger.dataset.value||'';var parsed=parseDefaultModelValue(value);valueNode.replaceChildren();if(parsed){var icon=document.createElement('span');icon.className='default-model-selected-icon';icon.textContent=parsed.model_provider.charAt(0).toUpperCase();var name=document.createElement('span');name.className='default-model-selected-name';name.textContent=parsed.model_name;var instance=document.createElement('span');instance.className='default-model-selected-instance';instance.textContent=parsed.model_instance;valueNode.append(icon,name,instance)}else{var placeholder=document.createElement('span');placeholder.className='default-model-placeholder';placeholder.textContent=field.placeholder||'Select value';valueNode.appendChild(placeholder)}indicator.onclick=null;if(!field.required&&value){indicator.textContent='×';indicator.classList.add('clear');indicator.setAttribute('aria-hidden','false');indicator.setAttribute('aria-label','Clear '+field.fieldId);indicator.onclick=function(event){event.stopPropagation();saveModelDefault(field.updateType,'')}}else{indicator.textContent='⌄';indicator.classList.remove('clear');indicator.setAttribute('aria-hidden','true');indicator.removeAttribute('aria-label')}}
function closeDefaultModelTree(field,resetSearch){var trigger=document.getElementById(field.domId);var popover=document.getElementById(field.domId+'Popover');if(!trigger||!popover)return;popover.hidden=true;trigger.setAttribute('aria-expanded','false');trigger.dataset.state='closed';if(resetSearch){var state=defaultModelState(field);state.search='';var search=document.getElementById(field.domId+'Search');if(search)search.value=''}}
function closeAllDefaultModelTrees(exceptType){MODEL_FIELDS.forEach(function(field){if(field.updateType!==exceptType)closeDefaultModelTree(field,false)})}
function toggleDefaultModelTree(type){var field=modelField(type);if(!field)return;var trigger=document.getElementById(field.domId);var popover=document.getElementById(field.domId+'Popover');var opening=popover.hidden;closeAllDefaultModelTrees(opening?type:null);if(!opening){closeDefaultModelTree(field,false);return}popover.hidden=false;trigger.setAttribute('aria-expanded','true');trigger.dataset.state='open';renderDefaultModelTree(field);var search=document.getElementById(field.domId+'Search');setTimeout(function(){if(search)search.focus()},0)}
function searchDefaultModelTree(type,value){var field=modelField(type);if(!field)return;defaultModelState(field).search=String(value||'');renderDefaultModelTree(field)}
function selectDefaultModel(type,value){var field=modelField(type);if(!field)return;closeDefaultModelTree(field,true);saveModelDefault(type,value)}
	async function saveModelDefault(type,nextValue){var field=modelField(type);if(!field)return;var trigger=document.getElementById(field.domId);if(!trigger)return;var state=defaultModelState(field);if(state.pending)return;var saved=trigger.dataset.savedValue||'';var value=String(nextValue==null?'':nextValue).trim();if(field.required&&!value){trigger.dataset.value=saved;renderDefaultModelTrigger(field);modelMessage('LLM is required and cannot be cleared.',true);return}var option=value?defaultModelOption(field,value):null;if(value&&!option){trigger.dataset.value=saved;renderDefaultModelTrigger(field);modelMessage('Select a model from the available model tree.',true);return}var parsed=option?{model_provider:option.provider_name,model_instance:option.instance_name,model_name:String(option.name).split('__')[0]}:{model_provider:'',model_instance:'',model_name:''};var requestId=++state.requestId;state.pending=true;trigger.disabled=true;trigger.dataset.value=value;renderDefaultModelTrigger(field);var typeMap={llm_id:'chat',llm:'chat',embd_id:'embedding',reranking_id:'rerank',asr_id:'asr',img2txt_id:'vision',tts_id:'tts',ocr_id:'ocr'};var body=Object.assign(parsed,{model_type:typeMap[type]||type});modelMessage('Saving…',false);try{var response=await fetch('/api/v1/models/default',{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});var data=await response.json();if(!response.ok||data.code!==0)throw new Error(data.message||('HTTP '+response.status));if(state.requestId!==requestId)return;trigger.dataset.savedValue=value;try{await loadModelDefaults(type,requestId);modelMessage('Saved',false)}catch(refreshError){modelMessage('Saved; refresh failed: '+refreshError.message,true)}}catch(error){if(state.requestId===requestId){trigger.dataset.value=saved;renderDefaultModelTrigger(field);modelMessage('Error: '+error.message,true)}}finally{if(state.requestId===requestId){state.pending=false;trigger.disabled=false}}}
	async function loadModelDefaults(onlyType,requestId){var response=await fetch('/api/v1/models/default');var data=await response.json();if(!response.ok||data.code!==0)throw new Error(data.message||('HTTP '+response.status));if(onlyType&&defaultModelState(modelField(onlyType)).requestId!==requestId)return;var fields=onlyType?[modelField(onlyType)]:MODEL_FIELDS.filter(function(field){return !defaultModelState(field).pending});fields.forEach(function(field){var trigger=document.getElementById(field.domId);if(trigger){trigger.dataset.value='';trigger.dataset.savedValue=''}});((data.data&&data.data.models)||[]).forEach(function(model){if(model.enable===false)return;var field=fields.find(function(item){return item.responseType===model.model_type});if(!field)return;var normalized=normalizeDefaultModel(model);var value=model.selector||normalized.selector;if(value&&!DEFAULT_MODEL_CATALOG.some(function(item){return item.selector===value})){normalized.selector=value;DEFAULT_MODEL_CATALOG.push(normalized)}var trigger=document.getElementById(field.domId);if(trigger){trigger.dataset.value=value;trigger.dataset.savedValue=value}});fields.forEach(function(field){renderDefaultModelTrigger(field);renderDefaultModelTree(field)})}
async function loadModelOptions(){try{var response=await fetch('/api/v1/models');var data=await response.json();if(!response.ok||data.code!==0)throw new Error(data.message||('HTTP '+response.status));DEFAULT_MODEL_CATALOG=(data.data||[]).map(normalizeDefaultModel).filter(function(model){return model.selector});MODEL_FIELDS.forEach(function(field){var state=defaultModelState(field);state.expanded.clear();expandDefaultModelTree(field)});await loadModelDefaults()}catch(error){modelMessage('Error: '+error.message,true)}}
document.addEventListener('pointerdown',function(event){MODEL_FIELDS.forEach(function(field){var root=document.querySelector('[data-default-model-root="'+field.domId+'"]');var popover=document.getElementById(field.domId+'Popover');if(root&&popover&&!popover.hidden&&!root.contains(event.target))closeDefaultModelTree(field,false)})});
	document.addEventListener('keydown',function(event){if(event.key!=='Escape')return;var openField=MODEL_FIELDS.find(function(field){return document.getElementById(field.domId).getAttribute('aria-expanded')==='true'});if(!openField)return;event.preventDefault();event.stopPropagation();closeAllDefaultModelTrees(null);document.getElementById(openField.domId).focus()});
"#;

pub async fn providers_page(
    State(state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let in_user_settings = req.uri().path() == "/user-setting/model";
    let zh = detect_lang(req.headers());
    let providers = state.providers.list();
    let mut rows = String::new();
    for p in &providers {
        let factory = crate::model_meta::resolve_model_factory(&p.id, &p.name);
        let kinds = provider_capability_tags(&p.id, &p.name);
        let click = match &factory {
            Some(factory) => format!(
                " style='cursor:pointer' data-factory='{}' data-k='{}' onclick='showProviderModels(this)'",
                html_escape(factory),
                kinds.join(" ")
            ),
            None => format!(" data-k='{}'", kinds.join(" ")),
        };
        rows.push_str(&format!(
            "<tr{}><td>{} {}{}</td><td class='co'>{}</td><td>{}</td></tr>",
            click,
            if p.enabled { "🟢" } else { "⚫" },
            html_escape(&p.name),
            if factory.is_some() {
                " <span style='font-size:11px;color:var(--tm)'>📦 catalog</span>"
            } else {
                ""
            },
            html_escape(&p.api_base),
            html_escape(&p.models.join(", "))
        ));
    }
    let mut b = String::new();
    b.push_str("<h1>🤖 ");
    b.push_str(t("Model providers", zh));
    b.push_str("</h1>");
    b.push_str("<div class='card'><h2>");
    b.push_str(t("Set default models", zh));
    b.push_str("</h2><p style='color:var(--tm);margin-bottom:12px'>");
    b.push_str(t("Please complete these settings before beginning", zh));
    b.push_str("</p><div class='provider-default-grid'>");
    for field in DEFAULT_MODEL_FIELDS {
        push_default_model_selector(&mut b, &field, zh);
    }
    b.push_str(
        "</div><p id='dmMsg' style='font-size:12px;color:var(--tm);margin-top:8px'></p></div>",
    );
    b.push_str("<section class='added-models-section' data-testid='added-models-section'><h2>Added models</h2><div id='addedModelList' class='added-model-list'><p class='added-model-loading'>Loading…</p></div></section>");
    {
        let summary = crate::api::tenant_models::plan_available_providers(
            crate::api::tenant_models::factory_llm_entries(),
        );
        b.push_str("<section class='card' id='availableModels' data-testid='available-models-section'><h2>Available models <span id='availableShown' style='font-size:12px;color:var(--tm)'></span></h2><input id='providerSearch' data-testid='model-providers-search' type='search' placeholder='Search' aria-label='Search model providers' oninput='filterAvailableProviders()'><div class='provider-filter-bar' id='providerFilters'><button class='btn btn-o btn-sm provider-chip on' data-filter='' onclick=\"selectProviderFilter('',this)\">All <span id='providerCountAll'></span></button>");
        // Upstream `un-add-model.tsx` orders the tag pills with `orderMap` and
        // labels them with `mapModelKey[tag] || tag`. Both now come from the
        // shared `crate::providers` table rather than a second literal copy.
        let mut chip_types = [
            "chat",
            "embedding",
            "rerank",
            "tts",
            "speech2text",
            "image2text",
            "ocr",
        ];
        crate::providers::sort_model_types(&mut chip_types);
        for model_type in chip_types {
            b.push_str("<button class='btn btn-o btn-sm provider-chip' data-filter='");
            b.push_str(model_type);
            b.push_str("' onclick=\"selectProviderFilter('");
            b.push_str(model_type);
            b.push_str("',this)\">");
            b.push_str(model_type_label(model_type));
            b.push_str(" <span id='providerCount-");
            b.push_str(model_type);
            b.push_str("'></span></button>");
        }
        b.push_str("</div><div class='available-provider-grid' id='availableProviderGrid'>");
        for available in &summary {
            let name = &available.name;
            let provider = providers.iter().find(|provider| {
                crate::providers::canonical_provider_name(&provider.id, &provider.name) == *name
            });
            let provider_id = provider
                .map(|provider| provider.id.clone())
                .unwrap_or_else(|| crate::providers::provider_id_slug(name));
            let api_base = available
                .url
                .get("default")
                .filter(|value| !value.is_empty())
                .cloned()
                .or_else(|| provider.map(|provider| provider.api_base.clone()))
                .unwrap_or_default();
            let base_options: Vec<(&String, &String)> = available
                .url
                .iter()
                .filter(|(_, url)| !url.is_empty())
                .collect();
            let base_options_json = if base_options.len() > 1 {
                format!(
                    "[{}]",
                    base_options
                        .iter()
                        .map(|(url, region)| format!(
                            "[\"{}\",\"{}\"]",
                            url.replace('\\', "\\\\").replace('"', "\\\""),
                            region.replace('\\', "\\\\").replace('"', "\\\"")
                        ))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            } else {
                String::new()
            };
            let model_names = provider
                .map(|provider| provider.models.join(", "))
                .unwrap_or_else(|| {
                    crate::api::tenant_models::factory_llm_entries()
                        .iter()
                        .find(|entry| entry.name == *name)
                        .map(|entry| {
                            entry
                                .llm
                                .iter()
                                .map(|model| model.llm_name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default()
                });
            let kinds: Vec<&str> = available.model_types.iter().map(String::as_str).collect();
            let uses_picker = provider_uses_model_picker(name);
            let shows_base = provider_shows_base_url(name);
            let key_optional = provider_api_key_optional(name);
            let special_modal = match name.as_str() {
                "Bedrock" => "bedrock",
                "SoMark" => "somark",
                _ => "",
            };
            let factory = crate::model_meta::resolve_model_factory(&provider_id, name)
                .unwrap_or_else(|| crate::providers::provider_id_slug(name));
            let doc_link = provider_api_key_url(name)
                .map(|url| {
                    format!(
                        "<a class='available-provider-doc' href='{}' target='_blank' rel='noopener noreferrer' aria-label='Open {} API key page' onclick='event.stopPropagation()'>↗</a>",
                        html_escape(url),
                        html_escape(name)
                    )
                })
                .unwrap_or_default();
            b.push_str(&format!(
                "<article class='available-provider-card' tabindex='0' data-testid='available-model-card' data-provider='{}' data-name='{}' data-k='{}' data-factory='{}' data-provider-id='{}' data-api-base='{}' data-api-base-options='{}' data-show-base='{}' data-models='{}' data-model-picker='{}' data-api-key-optional='{}' data-special-modal='{}' onclick='showProviderInstanceModal(this)' onkeydown=\"if(event.key==='Enter'||event.key===' '){{event.preventDefault();showProviderInstanceModal(this)}}\"><div class='available-provider-head'><span class='available-provider-icon'>{}</span><span class='available-provider-name'>{}</span>{}<button class='btn btn-p btn-sm available-provider-add' aria-label='Add {}' onclick='event.stopPropagation();showProviderInstanceModal(this.closest(&quot;[data-testid=available-model-card]&quot;))'>+ Add</button></div><div class='available-provider-tags'>",
                html_escape(name),
                html_escape(&name.to_lowercase()),
                kinds.join(" "),
                html_escape(&factory),
                html_escape(&provider_id),
                html_escape(&api_base),
                html_escape(&base_options_json),
                shows_base,
                html_escape(&model_names),
                uses_picker,
                key_optional,
                special_modal,
                provider_icon_html(name, "provider-brand-icon"),
                html_escape(name),
                doc_link,
                html_escape(name),
            ));
            for kind in &kinds {
                b.push_str("<span class='badge'>");
                b.push_str(model_type_label(kind));
                b.push_str("</span>");
            }
            b.push_str("</div><p style='margin-top:9px'>");
            b.push_str(&available.model_count.to_string());
            b.push_str(" models</p></article>");
        }
        b.push_str("</div><p class='available-provider-empty' id='availableProviderEmpty'>No model providers found.</p></section>");
    }
    {
        let reranker = state.reranker.public_config();
        b.push_str(
            "<div class='card'><h2>Reranker</h2><label><input id='rerank-enabled' type='checkbox'",
        );
        if reranker.enabled {
            b.push_str(" checked");
        }
        b.push_str("> Enabled</label><label style='display:block;margin-top:12px'>API Base<input id='rerank-base' value='");
        b.push_str(&html_escape(&reranker.api_base));
        b.push_str("' placeholder='http://127.0.0.1:8899'></label><label style='display:block;margin-top:12px'>API Key<input id='rerank-key' type='password' placeholder='");
        b.push_str(if reranker.api_key_configured {
            "Configured; leave blank to keep"
        } else {
            "Optional"
        });
        b.push_str("'></label><label style='display:block;margin:12px 0'><input id='rerank-clear' type='checkbox'> Clear saved API key</label><button class='btn btn-p' onclick='saveReranker()'>Save</button><span id='rerank-status' style='margin-left:10px;color:var(--tm)'></span></div><script>function saveReranker(){var s=document.getElementById('rerank-status');fetch('/api/v1/reranker',{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({enabled:document.getElementById('rerank-enabled').checked,api_base:document.getElementById('rerank-base').value,api_key:document.getElementById('rerank-key').value||null,clear_api_key:document.getElementById('rerank-clear').checked})}).then(async r=>({ok:r.ok,data:await r.json()})).then(x=>{s.textContent=x.ok?'Saved':(x.data.message||'Failed')})}</script>");
    }
    b.push_str("<div class='card'><h2>RAG Chat Test</h2><input id='q' placeholder='Ask...' style='max-width:500px'><button class='btn btn-p' onclick='chat()'>Ask</button><div id='answer' style='margin-top:12px;background:var(--bg);padding:16px;border-radius:var(--r);min-height:40px;white-space:pre-wrap;line-height:1.6'></div></div><script>function chat(){var q=document.getElementById('q').value;fetch('/api/v1/completions',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:q})}).then(r=>r.json()).then(d=>{document.getElementById('answer').textContent=d.data?d.data.answer:JSON.stringify(d)})}</script>");
    b.push_str("<details class='card provider-admin-details'><summary>RayRAG provider administration</summary><div style='display:flex;justify-content:flex-end;margin:12px 0'><button class='btn btn-p btn-sm' onclick='showAddProv()'>+ Add custom provider</button></div><table><thead><tr><th>Provider</th><th>API Base</th><th>Models</th></tr></thead><tbody>");
    b.push_str(&rows);
    b.push_str("</tbody></table><p style='color:var(--tm);font-size:12px;margin-top:8px'>");
    b.push_str(t(
        "Click a provider row to browse its seeded model catalog (RAGFlow conf/models).",
        zh,
    ));
    b.push_str("</p></details>");
    b.push_str("<div class='modal-overlay' id='modelModal' onclick='if(event.target===this)this.classList.remove(\"show\")'><div class='modal' style='max-width:720px'><h2 id='modelModalTitle'>");
    b.push_str(t("Models", zh));
    b.push_str("</h2><div id='modelModalBody' style='max-height:60vh;overflow:auto;margin-top:12px'></div><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-o' onclick='document.getElementById(\"modelModal\").classList.remove(\"show\")'>");
    b.push_str(t("Close", zh));
    let model_fields_payload = serde_json::Value::Array(
        DEFAULT_MODEL_FIELDS
            .iter()
            .map(|field| {
                serde_json::json!({
                    "fieldId": field.field_id,
                    "domId": field.dom_id,
                    "updateType": field.update_type,
                    "responseType": field.response_type,
                    "acceptedTypes": field.accepted_types,
                    "required": field.required,
                    "placeholder": field.placeholder,
                })
            })
            .collect(),
    );
    b.push_str("</button></div></div></div><script>const MODEL_FIELDS=");
    b.push_str(&json_script_value(&model_fields_payload));
    b.push_str(DEFAULT_MODEL_TREE_JS_HEAD);
    b.push_str(DEFAULT_MODEL_TREE_JS_BODY);
    b.push_str("loadModelOptions();\nconst providerModels=");
    b.push_str(&json_script_value(
        &crate::model_meta::provider_models_payload(),
    ));
    b.push_str(";function showProviderModels(el){var factory=el.getAttribute('data-factory');var provider=providerModels[factory];if(!provider)return;document.getElementById('modelModalTitle').textContent=provider.name+' ('+factory+')';var body=document.getElementById('modelModalBody');body.replaceChildren();if(!provider.models||!provider.models.length){var empty=document.createElement('p');empty.style.color='var(--tm)';empty.textContent=");
    b.push_str(&json_script_string(t(
        "No static model list (models are discovered at runtime).",
        zh,
    )));
    b.push_str(";body.appendChild(empty)}else{provider.models.forEach(function(model){var row=document.createElement('div');row.style.cssText='padding:8px 0;border-bottom:1px solid var(--bd);display:flex;align-items:center;gap:8px;flex-wrap:wrap';var name=document.createElement('span');name.style.fontWeight='500';name.textContent=model.n;row.appendChild(name);(model.t||[]).forEach(function(tag){var badge=document.createElement('span');badge.className='badge bs';badge.textContent=tag;row.appendChild(badge)});body.appendChild(row)})}document.getElementById('modelModal').classList.add('show')}</script>");
    b.push_str(
        r##"<div class='modal-overlay' id='providerInstanceModal' data-testid='provider-instance-dialog' onclick='if(event.target===this)closeProviderInstanceModal()'><div class='modal provider-instance-modal' role='dialog' aria-modal='true' aria-labelledby='providerInstanceTitle'><div class='provider-instance-title'><h2 id='providerInstanceTitle'></h2><a id='providerInstanceDoc' class='available-provider-doc' target='_blank' rel='noopener noreferrer' aria-label='Open provider documentation'>↗</a></div><label class='provider-instance-field'>Instance name<input id='piName' name='instance_name' placeholder='Please input the instance name!' autocomplete='off'></label><p id='piNameError' class='provider-instance-error'></p><label class='provider-instance-field' id='piBaseField'>Base url<input id='piBase' name='base_url' list='piBaseOptions' placeholder='Please input your base url!' autocomplete='off'><datalist id='piBaseOptions'></datalist></label><p id='piBaseError' class='provider-instance-error'></p><label class='provider-instance-field'>API-Key<input id='piKey' name='api_key' type='password' placeholder='Please enter the API key' autocomplete='new-password'></label><p id='piKeyError' class='provider-instance-error'></p><label class='provider-instance-switch' id='piVisionField'><span>Vision</span><input id='piVision' type='checkbox' role='switch' aria-label='Vision'></label><label class='provider-instance-field' id='piGroupField'>Group ID<input id='piGroup' name='group_id' autocomplete='off'></label><section id='piModelPicker' class='provider-model-picker' aria-label='Provider models'><button class='btn btn-o provider-model-trigger' id='piListModels' type='button' aria-expanded='false' onclick='toggleProviderModelPicker()'>List models</button><div id='piModelPanel' class='provider-model-panel' hidden><input id='piModelSearch' type='search' placeholder='Search models' aria-label='Search models' oninput='renderProviderModelList()'><p id='piModelLoading' class='provider-model-empty'></p><div id='piModelList' class='provider-model-list' role='list'></div><button class='provider-model-add' type='button' aria-label='Add custom model' onclick='showProviderCustomModel()'>＋ <span>Add custom model</span></button></div><p id='piModelError' class='provider-instance-error'></p></section><p id='piStatus' class='provider-instance-status' aria-live='polite'></p><div class='provider-instance-actions'><button class='btn btn-o' id='piVerify' type='button' onclick='verifyProviderInstance()'>Verify</button><button class='btn btn-o' type='button' onclick='closeProviderInstanceModal()'>Cancel</button><button class='btn btn-p' id='piOk' type='button' onclick='saveProviderInstance()'>Ok</button></div></div></div>
<div class='modal-overlay provider-custom-model-overlay' id='providerCustomModelModal' style='z-index:1001' data-testid='add-custom-model-dialog' onclick='if(event.target===this)closeProviderCustomModel()'><div class='modal provider-custom-model-modal' role='dialog' aria-modal='true' aria-labelledby='providerCustomModelTitle'><h2 id='providerCustomModelTitle'>Add custom model</h2><label class='provider-instance-field'>Model name<input id='pcmName' name='name' placeholder='Model name' autocomplete='off'></label><fieldset class='added-model-type-fieldset' id='pcmTypes'><legend>Model type</legend><label><input type='checkbox' value='chat'> LLM</label><label><input type='checkbox' value='embedding'> Embedding</label><label><input type='checkbox' value='rerank'> Rerank</label><label><input type='checkbox' value='tts'> TTS</label><label><input type='checkbox' value='image2text'> VLM</label><label><input type='checkbox' value='ocr'> OCR</label><label><input type='checkbox' value='speech2text'> ASR</label></fieldset><label class='provider-instance-field'>Max tokens<input id='pcmMaxTokens' name='max_tokens' type='number' min='0' value='0' placeholder='Max tokens'></label><fieldset class='added-model-type-fieldset'><legend>Model features</legend><label class='provider-instance-switch'><span>Tool call</span><input id='pcmTools' type='checkbox' role='switch' aria-label='Tool call'></label></fieldset><p id='pcmError' class='provider-instance-error'></p><div class='provider-instance-actions'><button class='btn btn-o' type='button' onclick='closeProviderCustomModel()'>Cancel</button><button class='btn btn-p' type='button' onclick='addProviderCustomModel()'>Confirm</button><button class='btn btn-o' type='button' aria-label='Close' onclick='closeProviderCustomModel()'>Close</button></div></div></div>
<div class='modal-overlay' id='bedrockModal' data-testid='bedrock-dialog' onclick='if(event.target===this)closeBedrockModal()'><div class='modal provider-instance-modal bedrock-modal' role='dialog' aria-modal='true' aria-labelledby='bedrockTitle'><div class='provider-instance-title'><h2 id='bedrockTitle'></h2><a id='bedrockDoc' class='available-provider-doc' target='_blank' rel='noopener noreferrer' aria-label='Open provider documentation'>↗</a></div><label class='provider-instance-field'>Instance name<input id='bdName' name='instance_name' placeholder='Please input the instance name!' autocomplete='off' oninput='bedrockRefreshInstanceState()'></label><p id='bdNameError' class='provider-instance-error'></p><fieldset class='bedrock-model-types'><legend>Model type</legend><label><input type='checkbox' name='bdType' value='chat'> Chat</label><label><input type='checkbox' name='bdType' value='embedding'> Embedding</label></fieldset><p id='bdTypeError' class='provider-instance-error'></p><label class='provider-instance-field'>Model name<input id='bdModelName' name='llm_name' placeholder='Please input your model name!' autocomplete='off'></label><p id='bdModelError' class='provider-instance-error'></p><div id='bdAuthSection'><div class='bedrock-auth-segment' id='bdAuthMode' role='radiogroup' aria-label='AWS authentication mode' data-testid='bedrock-auth-mode'><button type='button' role='radio' aria-checked='true' data-mode='access_key_secret' onclick='setBedrockAuthMode("access_key_secret")'>Access Key</button><button type='button' role='radio' aria-checked='false' data-mode='iam_role' onclick='setBedrockAuthMode("iam_role")'>IAM Role</button><button type='button' role='radio' aria-checked='false' data-mode='assume_role' onclick='setBedrockAuthMode("assume_role")'>Assume Role</button></div><div id='bdAkSk'><label class='provider-instance-field'>AWS Access Key ID<input id='bdAk' name='bedrock_ak' placeholder='Please input your ACCESS KEY' autocomplete='off'></label><p id='bdAkError' class='provider-instance-error'></p><label class='provider-instance-field'>AWS Secret Access Key<input id='bdSk' name='bedrock_sk' type='password' placeholder='Please input your SECRET KEY' autocomplete='new-password'></label><p id='bdSkError' class='provider-instance-error'></p></div><div id='bdRoleArnField' hidden><label class='provider-instance-field'>AWS Role ARN<input id='bdRoleArn' name='aws_role_arn' placeholder='Please enter AWS Role ARN' autocomplete='off'></label><p id='bdRoleArnError' class='provider-instance-error'></p></div><p id='bdAssumeTip' class='bedrock-assume-tip' hidden>If you select this mode, the Amazon EC2 instance will assume its existing role to access AWS services. No additional credentials are required.</p><label class='provider-instance-field'>AWS region<input id='bdRegion' name='bedrock_region' list='bdRegionOptions' placeholder='Please select!' data-testid='bedrock-region-select' autocomplete='off'><datalist id='bdRegionOptions'></datalist></label><p id='bdRegionError' class='provider-instance-error'></p></div><label class='provider-instance-field'>Max tokens<input id='bdMaxTokens' name='max_tokens' type='number' min='0' value='8192' placeholder='The maximum context size of the model'></label><p id='bdMaxTokensError' class='provider-instance-error'></p><p id='bdStatus' class='provider-instance-status' aria-live='polite'></p><div class='provider-instance-actions'><button class='btn btn-o' id='bdVerify' type='button' onclick='verifyBedrock()'>Verify</button><button class='btn btn-o' type='button' onclick='closeBedrockModal()'>Cancel</button><button class='btn btn-p' id='bdOk' type='button' onclick='saveBedrock()'>Ok</button></div></div></div>
<div class='modal-overlay' id='somarkModal' data-testid='somark-dialog' onclick='if(event.target===this)closeSoMarkModal()'><div class='modal provider-instance-modal somark-modal' role='dialog' aria-modal='true' aria-labelledby='somarkTitle'><div class='provider-instance-title'><h2 id='somarkTitle'></h2><a id='somarkDoc' class='available-provider-doc' target='_blank' rel='noopener noreferrer' aria-label='Open provider documentation'>↗</a></div><label class='provider-instance-field'>Instance name<input id='smName' name='instance_name' placeholder='Please input the instance name!' autocomplete='off'></label><p id='smNameError' class='provider-instance-error'></p><label class='provider-instance-field'>Model name<input id='smModelName' name='llm_name' placeholder='somark-from-env-1' autocomplete='off'></label><p id='smModelNameError' class='provider-instance-error'></p><label class='provider-instance-field'>Base URL<input id='smBaseUrl' name='somark_base_url' placeholder='For SoMark API, fill in https://somark.tech/api/v1; for self-hosted deployment, fill in your local Base URL' autocomplete='off'></label><p id='smBaseUrlError' class='provider-instance-error'></p><label class='provider-instance-field'>API Key<input id='smApiKey' name='somark_api_key' type='password' placeholder='Required for SoMark API; leave blank for self-hosted deployment' autocomplete='new-password'></label><h3 class='somark-section-title'>Element Formats</h3><label class='provider-instance-field'>Image Format<select id='smImageFormat'><option value='url'>URL</option><option value='base64'>Base64</option><option value='none'>None</option></select></label><label class='provider-instance-field'>Formula Format<select id='smFormulaFormat'><option value='latex'>LaTeX</option><option value='mathml'>MathML</option><option value='ascii'>ASCII</option></select></label><label class='provider-instance-field'>Table Format<select id='smTableFormat'><option value='html'>HTML</option><option value='markdown'>Markdown</option><option value='image'>Image</option></select></label><label class='provider-instance-field'>Chemical Structural Formula Format<select id='smCsFormat'><option value='image'>Image</option></select></label><h3 class='somark-section-title'>Feature Config</h3><label class='provider-instance-switch'><span>Enable Text Cross Page</span><input type='checkbox' role='switch' data-key='somark_enable_text_cross_page' aria-label='Enable Text Cross Page'></label><label class='provider-instance-switch'><span>Enable Table Cross Page</span><input type='checkbox' role='switch' data-key='somark_enable_table_cross_page' aria-label='Enable Table Cross Page'></label><label class='provider-instance-switch'><span>Enable Title Level Recognition</span><input type='checkbox' role='switch' data-key='somark_enable_title_level_recognition' aria-label='Enable Title Level Recognition'></label><label class='provider-instance-switch'><span>Enable Inline Image</span><input type='checkbox' role='switch' data-key='somark_enable_inline_image' aria-label='Enable Inline Image'></label><label class='provider-instance-switch'><span>Enable Table Image</span><input type='checkbox' role='switch' data-key='somark_enable_table_image' aria-label='Enable Table Image'></label><label class='provider-instance-switch'><span>Enable Image Understanding</span><input type='checkbox' role='switch' data-key='somark_enable_image_understanding' aria-label='Enable Image Understanding'></label><label class='provider-instance-switch'><span>Keep Header Footer</span><input type='checkbox' role='switch' data-key='somark_keep_header_footer' aria-label='Keep Header Footer'></label><p id='smStatus' class='provider-instance-status' aria-live='polite'></p><div class='provider-instance-actions'><button class='btn btn-o' id='smVerify' type='button' onclick='verifySoMark()'>Verify</button><button class='btn btn-o' type='button' onclick='closeSoMarkModal()'>Cancel</button><button class='btn btn-p' id='smOk' type='button' onclick='saveSoMark()'>Add</button></div></div></div><script>
var providerInstanceCard=null;
var piModels=[];
var piSelectedModels=new Set();
var piModelsLoaded=false;
var providerSpecialConfig=null;var providerSpecialInstanceNames=null;
var PROVIDER_SPECIAL_FIELDS={'Azure-OpenAI':{types:[['chat','Chat'],['embedding','Embedding'],['image2text','Image2Text']],typeDefault:'embedding',showBase:true,baseRequired:true,showKey:true,keyRequired:false,keyLabel:'API-Key',keyPlaceholder:'Please enter the API key',fields:[{id:'pf_model_name',label:'Model name',type:'text',required:true,placeholder:'Please input your model name!',default:'gpt-3.5-turbo'},{id:'pf_api_version',label:'API-Version',type:'text',placeholder:'Please input API version',default:'2024-02-01'},{id:'pf_max_tokens',label:'Max tokens',type:'number',required:true,default:'8192'},{id:'pf_vision',label:'Vision',type:'switch',onTypes:['chat']}],buildKey:function(){var o={};var k=document.getElementById('piKey').value;if(k)o.api_key=k;var v=document.getElementById('pf_api_version').value;if(v)o.api_version=v;return o},buildModelTypes:function(){var types=specialSelectedTypes();if(document.getElementById('pf_vision').checked&&types.indexOf('chat')>=0&&types.indexOf('image2text')<0)types.push('image2text');return types}},'VolcEngine':{types:[['chat','Chat'],['embedding','Embedding'],['image2text','Image2Text']],typeDefault:'chat',showBase:false,showKey:true,keyRequired:true,keyLabel:'VOLC ARK_API_KEY',keyPlaceholder:'Please input your ARK_API_KEY',fields:[{id:'pf_model_name',label:'Model ID',type:'text',required:true,placeholder:'Please input your Model ID of the model'},{id:'pf_max_tokens',label:'Max tokens',type:'number',required:true,default:'8192'}],buildKey:function(){return document.getElementById('piKey').value}},'Google Cloud':{types:[['chat','Chat'],['image2text','Image2Text']],typeDefault:'chat',showBase:false,showKey:false,fields:[{id:'pf_model_name',label:'Model ID',type:'text',required:true,placeholder:'Please input your model ID!'},{id:'pf_google_project_id',label:'Project ID',type:'text',required:true,placeholder:'Please input your Project ID',credential:true},{id:'pf_google_region',label:'Region',type:'text',required:true,placeholder:'Please input Google Cloud Region',credential:true},{id:'pf_google_service_account_key',label:'Service Account Key(Leave blank if you use Application Default Credentials)',type:'text',required:true,placeholder:'Please input your Service Account Key',credential:true},{id:'pf_max_tokens',label:'Max tokens',type:'number',required:true,default:'8192'}],buildKey:function(){return {google_project_id:document.getElementById('pf_google_project_id').value,google_region:document.getElementById('pf_google_region').value,google_service_account_key:document.getElementById('pf_google_service_account_key').value}}},'Tencent Cloud':{types:[['speech2text','Speech2Text']],typeDefault:'speech2text',showBase:false,showKey:false,fields:[{id:'pf_model_name',label:'Model name',type:'select',required:true,options:['16k_zh','16k_zh_large','16k_multi_lang','16k_zh_dialect','16k_en','16k_yue','16k_zh-PY','16k_ja','16k_ko','16k_vi','16k_ms','16k_id','16k_fil','16k_th','16k_pt','16k_tr','16k_ar','16k_es','16k_hi','16k_fr','16k_zh_medical','16k_de'],default:'16k_zh'},{id:'pf_TencentCloud_sid',label:'TencentCloud Secret ID',type:'text',required:true,placeholder:'Please input your Secret ID',credential:true},{id:'pf_TencentCloud_sk',label:'TencentCloud Secret Key',type:'text',required:true,placeholder:'Please input your Secret Key',credential:true}],buildKey:function(){return {TencentCloud_sid:document.getElementById('pf_TencentCloud_sid').value,TencentCloud_sk:document.getElementById('pf_TencentCloud_sk').value}}},'XunFei Spark':{types:[['chat','Chat'],['tts','TTS']],typeDefault:'chat',showBase:false,showKey:false,fields:[{id:'pf_model_name',label:'Model name',type:'text',required:true,placeholder:'Please input your model name!'},{id:'pf_spark_api_password',label:'Spark APIPassword',type:'text',required:true,placeholder:'please input your APIPassword',credential:true},{id:'pf_spark_app_id',label:'Spark APP ID',type:'text',required:true,placeholder:'please input your APP ID',onTypes:['tts'],credential:true},{id:'pf_spark_api_secret',label:'Spark APISecret',type:'text',required:true,placeholder:'please input your APISecret',onTypes:['tts'],credential:true},{id:'pf_spark_api_key',label:'Spark APIKey',type:'text',required:true,placeholder:'please input your APIKey',onTypes:['tts'],credential:true},{id:'pf_max_tokens',label:'Max tokens',type:'number',required:true,default:'8192'}],buildKey:function(){return {spark_api_password:document.getElementById('pf_spark_api_password').value,spark_app_id:document.getElementById('pf_spark_app_id').value,spark_api_secret:document.getElementById('pf_spark_api_secret').value,spark_api_key:document.getElementById('pf_spark_api_key').value}}},'BaiduYiyan':{types:[['chat','Chat'],['embedding','Embedding'],['rerank','Rerank']],typeDefault:'chat',showBase:false,showKey:false,fields:[{id:'pf_model_name',label:'Model name',type:'text',required:true,placeholder:'Please input model name'},{id:'pf_yiyan_ak',label:'yiyan API KEY',type:'text',required:true,placeholder:'Please input your API KEY',credential:true},{id:'pf_yiyan_sk',label:'yiyan Secret KEY',type:'text',required:true,placeholder:'Please input your Secret KEY',credential:true},{id:'pf_max_tokens',label:'Max tokens',type:'number',required:true,default:'8192'}],buildKey:function(){return {yiyan_ak:document.getElementById('pf_yiyan_ak').value,yiyan_sk:document.getElementById('pf_yiyan_sk').value}}},'Fish Audio':{types:[['tts','TTS']],typeDefault:'tts',showBase:false,showKey:false,fields:[{id:'pf_model_name',label:'Model name',type:'text',required:true,placeholder:'Please give your speech synthesis model a name'},{id:'pf_fish_audio_ak',label:'Fish Audio API KEY',type:'text',required:true,placeholder:'Please input your API KEY',credential:true},{id:'pf_fish_audio_refid',label:'FishAudio Reference ID',type:'text',required:true,placeholder:'Please input the Reference ID (leave blank to use the default model).',credential:true},{id:'pf_max_tokens',label:'Max tokens',type:'number',required:true,default:'8192'}],buildKey:function(){return {fish_audio_ak:document.getElementById('pf_fish_audio_ak').value,fish_audio_refid:document.getElementById('pf_fish_audio_refid').value}}},'OpenDataLoader':{ocr:true,showBase:false,showKey:false,fields:[{id:'pf_model_name',label:'Model name',type:'text',required:true,placeholder:'Please input your model name!'},{id:'pf_opendataloader_apiserver',label:'Base URL',type:'text',required:true,placeholder:'Please input the OpenDataLoader API server URL!',credential:true},{id:'pf_opendataloader_api_key',label:'API Key',type:'text',placeholder:'API Key (optional)',credential:true}],buildKey:function(){var o={};var api=document.getElementById('pf_opendataloader_apiserver').value;if(api)o.opendataloader_apiserver=api;var key=document.getElementById('pf_opendataloader_api_key').value;if(key)o.opendataloader_api_key=key;o.llm_name=document.getElementById('pf_model_name').value;return o}},'PaddleOCR':{ocr:true,showBase:false,showKey:false,fields:[{id:'pf_model_name',label:'Model name',type:'text',required:true,placeholder:'Please input your model name!'},{id:'pf_paddleocr_api_url',label:'PaddleOCR API URL',type:'text',required:true,placeholder:'For example: https://paddleocr-server.com/layout-parsing',credential:true},{id:'pf_paddleocr_access_token',label:'AI Studio Access Token',type:'text',placeholder:'Access token for PaddleOCR API (optional)',credential:true},{id:'pf_paddleocr_algorithm',label:'Algorithm',type:'select',options:['PaddleOCR-VL-1.6','PaddleOCR-VL-1.5','PaddleOCR-VL','PP-OCRv6','PP-OCRv5','PP-StructureV3'],default:'PaddleOCR-VL'}],buildKey:function(){var o={};var url=document.getElementById('pf_paddleocr_api_url').value;if(url)o.paddleocr_api_url=url;var token=document.getElementById('pf_paddleocr_access_token').value;if(token)o.paddleocr_access_token=token;var alg=document.getElementById('pf_paddleocr_algorithm').value;if(alg)o.paddleocr_algorithm=alg;return o}},'MinerU':{ocr:true,showBase:false,showKey:false,fields:[{id:'pf_model_name',label:'Model name',type:'text',required:true,placeholder:'Please input your model name!'},{id:'pf_mineru_apiserver',label:'MinerU API Server URL',type:'text',required:true,placeholder:'Please input the MinerU API Server URL!',credential:true},{id:'pf_mineru_output_dir',label:'MinerU Output Directory',type:'text',placeholder:'Output directory (optional)'},{id:'pf_mineru_backend',label:'MinerU Backend',type:'select',required:true,options:['pipeline','vlm-transformers','vlm-vllm-engine','vlm-http-client','vlm-mlx-engine','vlm-vllm-async-engine','vlm-lmdeploy-engine'],default:'pipeline'},{id:'pf_mineru_server_url',label:'MinerU Server URL',type:'text',placeholder:'Please input the MinerU Server URL!',showIf:function(){return document.getElementById('pf_mineru_backend').value==='vlm-http-client'}},{id:'pf_mineru_delete_output',label:'Delete output after processing',type:'switch',default:true}],buildKey:function(){var o={};o.mineru_apiserver=document.getElementById('pf_mineru_apiserver').value;var out=document.getElementById('pf_mineru_output_dir').value;if(out)o.mineru_output_dir=out;o.mineru_backend=document.getElementById('pf_mineru_backend').value;if(document.getElementById('pf_mineru_backend').value==='vlm-http-client'){o.mineru_server_url=document.getElementById('pf_mineru_server_url').value}o.mineru_delete_output=document.getElementById('pf_mineru_delete_output').checked?'1':'0';return o}}};
function providerSpecialFieldConfig(provider){return PROVIDER_SPECIAL_FIELDS[provider]||null}
function specialSelectedTypes(){return Array.from(document.querySelectorAll('#piTypeFields input[type=checkbox]:checked')).map(function(input){return input.value})}
function ensureProviderExtraContainer(){var container=document.getElementById('piExtraFields');if(!container){container=document.createElement('div');container.id='piExtraFields';container.className='provider-extra-fields';var status=document.getElementById('piStatus');status.parentNode.insertBefore(container,status)}return container}
function renderSpecialProviderFields(config){var container=ensureProviderExtraContainer();container.replaceChildren();if(config.types){var set=document.createElement('fieldset');set.id='piTypeFields';set.className='bedrock-model-types';var legend=document.createElement('legend');legend.textContent='Model type';set.appendChild(legend);config.types.forEach(function(pair){var label=document.createElement('label');var box=document.createElement('input');box.type='checkbox';box.value=pair[0];box.checked=pair[0]===config.typeDefault;box.addEventListener('change',refreshSpecialFieldsState);label.appendChild(box);label.appendChild(document.createTextNode(' '+pair[1]));set.appendChild(label)});container.appendChild(set);var err=document.createElement('p');err.id='piTypeError';err.className='provider-instance-error';container.appendChild(err)}config.fields.forEach(function(field){var label=document.createElement('label');label.className='provider-instance-field';label.id=field.id+'-label';var span=document.createElement('span');span.textContent=field.label;label.appendChild(span);var control;if(field.type==='select'){control=document.createElement('select');(field.options||[]).forEach(function(value){var option=document.createElement('option');option.value=value;option.textContent=value;control.appendChild(option)});control.value=field.default||'';control.addEventListener('change',refreshSpecialFieldsState)}else if(field.type==='switch'){control=document.createElement('input');control.type='checkbox';control.checked=!!field.default}else{control=document.createElement('input');control.type=field.type==='number'?'number':'text';if(field.type==='number'){control.min='0'}control.placeholder=field.placeholder||'';control.value=field.default||''}control.id=field.id;label.appendChild(control);container.appendChild(label);var error=document.createElement('p');error.id=field.id+'-error';error.className='provider-instance-error';container.appendChild(error);if(field.showIf){label.hidden=!field.showIf()}if(field.onTypes){label.hidden=true}});refreshSpecialFieldsState()}
function refreshSpecialFieldsState(){var card=providerInstanceCard;if(!card)return;var config=providerSpecialFieldConfig(card.dataset.provider||'');if(!config)return;var exists=!!providerSpecialInstanceNames&&providerSpecialInstanceNames.has(document.getElementById('piName').value.trim());if(config.showBase)document.getElementById('piBaseField').hidden=exists;if(config.showKey)document.getElementById('piKeyField').hidden=exists;var types=specialSelectedTypes();config.fields.forEach(function(field){var control=document.getElementById(field.id);if(!control)return;var label=document.getElementById(field.id+'-label');if(!label)return;var onTypes=field.onTypes&&field.onTypes.some(function(type){return types.indexOf(type)>=0});var show;if(field.credential&&field.onTypes){show=!exists&&onTypes}else if(field.credential){show=!exists}else if(field.onTypes){show=onTypes}else{show=true}if(field.showIf)show=show&&field.showIf();label.hidden=!show;var error=document.getElementById(field.id+'-error');if(error)error.hidden=!show})}
function loadProviderSpecialInstanceNames(provider){var names=null;fetch('/api/v1/providers/'+encodeURIComponent(provider)+'/instances').then(function(response){return response.json()}).then(function(data){names=new Set();(data.data||[]).forEach(function(instance){if(instance&&instance.instance_name)names.add(instance.instance_name)});if(providerInstanceCard&&providerInstanceCard.dataset.provider===provider){providerSpecialInstanceNames=names;refreshSpecialFieldsState()}}).catch(function(){providerSpecialInstanceNames=new Set()})}
function providerSpecialValidate(){var config=providerSpecialConfig;if(!config)return true;var valid=true;['piNameError','piTypeError'].forEach(function(id){var el=document.getElementById(id);if(el)el.textContent=''});if(!document.getElementById('piName').value.trim()){document.getElementById('piNameError').textContent='Please input the instance name!';valid=false}if(config.types&&!specialSelectedTypes().length){document.getElementById('piTypeError').textContent='Please input your model type!';valid=false}if(config.showBase&&config.baseRequired&&!document.getElementById('piBase').value.trim()&&!(providerSpecialInstanceNames&&providerSpecialInstanceNames.has(document.getElementById('piName').value.trim()))){document.getElementById('piBaseError').textContent='Please input your base url!';valid=false}if(config.showKey&&config.keyRequired&&!document.getElementById('piKey').value.trim()&&!(providerSpecialInstanceNames&&providerSpecialInstanceNames.has(document.getElementById('piName').value.trim()))){document.getElementById('piKeyError').textContent=config.keyPlaceholder||'Please enter the API key';valid=false}var exists=!!providerSpecialInstanceNames&&providerSpecialInstanceNames.has(document.getElementById('piName').value.trim());config.fields.forEach(function(field){var control=document.getElementById(field.id);var error=document.getElementById(field.id+'-error');if(!control)return;if(field.showIf&&!field.showIf()){return}var onTypes=field.onTypes&&field.onTypes.some(function(type){return specialSelectedTypes().indexOf(type)>=0});if(field.onTypes&&!onTypes)return;if(field.credential&&exists)return;var value=field.type==='switch'?'true':control.value.trim();if(field.required&&!value){error.textContent=field.placeholder||('Please input the '+field.label);valid=false}else if(field.type==='number'&&value!==''&&(isNaN(Number(value))||Number(value)<0)){error.textContent='Max tokens cannot be less than 0.';valid=false}});return valid}
function providerSpecialModelTypes(){var config=providerSpecialConfig;if(!config)return null;if(config.ocr)return ['ocr'];if(config.buildModelTypes)return config.buildModelTypes();return specialSelectedTypes()}
function providerSpecialPayload(){var config=providerSpecialConfig;var modelName=document.getElementById('pf_model_name')?document.getElementById('pf_model_name').value.trim():'';var maxTokens=document.getElementById('pf_max_tokens')?(Number(document.getElementById('pf_max_tokens').value)||0):0;var modelInfo=[{model_name:modelName,model_type:providerSpecialModelTypes(),max_tokens:maxTokens}];var key=config.buildKey?config.buildKey():null;var extraObj={};(config.fields||[]).forEach(function(field){if(field.credential)return;if(field.id==='pf_model_name'||field.id==='pf_max_tokens')return;var el=document.getElementById(field.id);if(!el)return;var v=el.type==='checkbox'?String(el.checked):String(el.value).trim();if(v!==''&&v!=='false')extraObj[field.key||field.id]=v});var payload={instance_name:document.getElementById('piName').value.trim(),api_key:key,base_url:config.showBase?document.getElementById('piBase').value.trim():undefined,model_info:modelInfo};for(var ek in extraObj)payload.extra=payload.extra||{},payload.extra[ek]=extraObj[ek];return payload}
function providerSpecialVerifyPayload(){var payload=providerSpecialPayload();return {api_key:(typeof payload.api_key==='string')?payload.api_key:JSON.stringify(payload.api_key),base_url:payload.base_url,model_info:payload.model_info}}
document.getElementById('piKey').closest('label').id='piKeyField';
document.getElementById('piName').addEventListener('input',refreshSpecialFieldsState);
function providerInstanceSetStatus(text,error){var status=document.getElementById('piStatus');status.textContent=text||'';status.style.color=error?'var(--d)':'var(--tm)'}
function providerInstanceUsesPicker(){return !!providerInstanceCard&&providerInstanceCard.dataset.modelPicker==='true'}
function providerInstanceValidate(){if(providerSpecialConfig)return providerSpecialValidate();var name=document.getElementById('piName');var key=document.getElementById('piKey');var base=document.getElementById('piBase');var picker=providerInstanceUsesPicker();var optional=providerInstanceCard&&providerInstanceCard.dataset.apiKeyOptional==='true';var valid=true;['piNameError','piKeyError','piBaseError','piModelError'].forEach(function(id){document.getElementById(id).textContent=''});if(!name.value.trim()){document.getElementById('piNameError').textContent='Instance name is required';valid=false}if(picker&&!base.value.trim()){document.getElementById('piBaseError').textContent='Base url is required';valid=false}if(!optional&&!key.value.trim()){document.getElementById('piKeyError').textContent='Please enter the API key';valid=false}if(picker&&!piSelectedModels.size){document.getElementById('piModelError').textContent='Select at least one model';valid=false}return valid}
function resetProviderModelPicker(){piModels=[];piSelectedModels=new Set();piModelsLoaded=false;document.getElementById('piModelSearch').value='';document.getElementById('piModelLoading').textContent='';document.getElementById('piModelList').replaceChildren();document.getElementById('piModelPanel').hidden=true;document.getElementById('piListModels').setAttribute('aria-expanded','false')}
function providerBaseOptions(card){try{var raw=card.dataset.apiBaseOptions;if(raw){var parsed=JSON.parse(raw);if(Array.isArray(parsed))return parsed.filter(function(item){return Array.isArray(item)&&item.length>=2})}}catch(error){}return card.dataset.apiBase?[[card.dataset.apiBase,'default']]:[]}
function providerRegionForBaseUrl(base){if(!base)return null;var options=providerBaseOptions(providerInstanceCard);for(var i=0;i<options.length;i++){if(options[i][0]===base)return options[i][1]}return null}
function showProviderInstanceModal(card){var special=card.dataset.specialModal||'';if(special==='bedrock'){showBedrockModal(card);return}if(special==='somark'){showSoMarkModal(card);return}providerInstanceCard=card;var provider=card.dataset.provider||'';providerSpecialConfig=providerSpecialFieldConfig(provider);providerSpecialInstanceNames=null;var picker=card.dataset.modelPicker==='true'&&!providerSpecialConfig;document.getElementById('providerInstanceTitle').textContent=provider;var doc=document.getElementById('providerInstanceDoc');var cardDoc=card.querySelector('.available-provider-doc');if(cardDoc){doc.href=cardDoc.href;doc.hidden=false}else{doc.removeAttribute('href');doc.hidden=true}document.getElementById('piName').value='';document.getElementById('piKey').value='';document.getElementById('piBase').value=card.dataset.apiBase||'';document.getElementById('piBaseOptions').replaceChildren();providerBaseOptions(card).forEach(function(entry){var option=document.createElement('option');option.value=entry[0];option.label=entry[1];option.dataset.region=entry[1];document.getElementById('piBaseOptions').appendChild(option)});var keyLabel=document.getElementById('piKey').closest('label');keyLabel.childNodes[0].textContent=providerSpecialConfig&&providerSpecialConfig.keyLabel?providerSpecialConfig.keyLabel:'API-Key';document.getElementById('piKey').placeholder=providerSpecialConfig&&providerSpecialConfig.keyPlaceholder?providerSpecialConfig.keyPlaceholder:'Please enter the API key';document.getElementById('piBaseField').hidden=providerSpecialConfig?!providerSpecialConfig.showBase:!picker&&card.dataset.showBase!=='true';document.getElementById('piKeyField').hidden=providerSpecialConfig?!providerSpecialConfig.showKey:false;document.getElementById('piVisionField').hidden=!!providerSpecialConfig||!picker;document.getElementById('piVision').checked=false;document.getElementById('piModelPicker').hidden=!!providerSpecialConfig||!picker;document.getElementById('piGroupField').hidden=provider.toLowerCase()!=='minimax';document.getElementById('piGroup').value='';['piNameError','piKeyError','piBaseError','piModelError'].forEach(function(id){document.getElementById(id).textContent=''});resetProviderModelPicker();if(providerSpecialConfig){renderSpecialProviderFields(providerSpecialConfig);loadProviderSpecialInstanceNames(provider)}else{ensureProviderExtraContainer().replaceChildren()}providerInstanceSetStatus('',false);document.getElementById('providerInstanceModal').classList.add('show')}
function closeProviderInstanceModal(){closeProviderCustomModel();document.getElementById('providerInstanceModal').classList.remove('show');resetProviderModelPicker();providerInstanceCard=null}
document.addEventListener('keydown',function(event){if(event.key!=='Escape')return;var custom=document.getElementById('providerCustomModelModal');if(custom&&custom.classList.contains('show')){closeProviderCustomModel();return}if(document.getElementById('bedrockModal').classList.contains('show')){closeBedrockModal();return}if(document.getElementById('somarkModal').classList.contains('show')){closeSoMarkModal();return}var inst=document.getElementById('providerInstanceModal');if(inst&&inst.classList.contains('show')){closeProviderInstanceModal()}})
function normalizeProviderModel(model){return {name:String(model.name||''),model_types:Array.isArray(model.model_types)?model.model_types.slice():[],max_tokens:Number(model.max_tokens)||0,features:Array.isArray(model.features)?model.features.slice():[]}}
function providerModelInfo(model){var types=model.model_types.slice();if(document.getElementById('piVision').checked&&types.indexOf('chat')>=0&&types.indexOf('image2text')<0)types.push('image2text');return {model_name:model.name,model_type:types,max_tokens:model.max_tokens,extra:{is_tools:model.features.indexOf('is_tools')>=0}}}
function providerInstancePayload(){if(providerSpecialConfig)return providerSpecialPayload();var selected=piModels.filter(function(model){return piSelectedModels.has(model.name)}).map(providerModelInfo);return {instance_name:document.getElementById('piName').value.trim(),api_key:document.getElementById('piKey').value,base_url:document.getElementById('piBase').value.trim(),region:providerRegionForBaseUrl(document.getElementById('piBase').value.trim()),group_id:document.getElementById('piGroup').value.trim()||null,model_info:selected}}
function providerModelRow(model){var label=document.createElement('label');label.className='provider-model-row';label.dataset.name=model.name.toLowerCase();label.setAttribute('role','listitem');var details=document.createElement('span');details.className='provider-model-details';var name=document.createElement('strong');name.textContent=model.name;details.appendChild(name);model.model_types.forEach(function(type){var badge=document.createElement('span');badge.className='badge';badge.textContent=type;details.appendChild(badge)});var box=document.createElement('input');box.type='checkbox';box.checked=piSelectedModels.has(model.name);box.setAttribute('aria-label','Select '+model.name);box.onchange=function(){if(box.checked)piSelectedModels.add(model.name);else piSelectedModels.delete(model.name);renderProviderModelList()};label.appendChild(details);label.appendChild(box);return label}
function renderProviderModelList(){var list=document.getElementById('piModelList');var query=document.getElementById('piModelSearch').value.trim().toLowerCase();list.replaceChildren();var visible=piModels.filter(function(model){return !query||model.name.toLowerCase().indexOf(query)>=0});document.getElementById('piModelSearch').hidden=piModels.length<=10;if(piModels.length){var all=document.createElement('label');all.className='provider-model-row provider-model-all';all.setAttribute('role','listitem');var text=document.createElement('strong');text.textContent='All models';var box=document.createElement('input');box.type='checkbox';box.checked=piSelectedModels.size===piModels.length;box.setAttribute('aria-label','Select all models');box.onchange=function(){if(box.checked)piModels.forEach(function(model){piSelectedModels.add(model.name)});else piSelectedModels.clear();renderProviderModelList()};all.appendChild(text);all.appendChild(box);list.appendChild(all)}visible.forEach(function(model){list.appendChild(providerModelRow(model))});document.getElementById('piModelLoading').textContent=piModels.length?'':'No models available'}
async function loadProviderModels(){var provider=providerInstanceCard.dataset.provider;var query=new URLSearchParams();var base=document.getElementById('piBase').value.trim();var key=document.getElementById('piKey').value;if(base)query.set('base_url',base);if(key)query.set('api_key',key);document.getElementById('piModelLoading').textContent='Loading…';try{var response=await fetch('/api/v1/providers/'+encodeURIComponent(provider)+'/models'+(query.toString()?'?'+query.toString():''));var data=await response.json().catch(function(){return {}});if(!response.ok||data.code!==0)throw new Error(data.message||('HTTP '+response.status));piModels=(data.data||[]).map(normalizeProviderModel).filter(function(model){return model.name});piModelsLoaded=true;renderProviderModelList()}catch(error){piModels=[];piModelsLoaded=true;renderProviderModelList();providerInstanceSetStatus(error.message,true)}}
async function toggleProviderModelPicker(){var panel=document.getElementById('piModelPanel');var open=panel.hidden;panel.hidden=!open;document.getElementById('piListModels').setAttribute('aria-expanded',open?'true':'false');if(open&&!piModelsLoaded)await loadProviderModels()}
function showProviderCustomModel(){document.getElementById('pcmName').value='';document.getElementById('pcmMaxTokens').value='0';document.getElementById('pcmTools').checked=false;document.querySelectorAll('#pcmTypes input').forEach(function(input){input.checked=false});document.getElementById('pcmError').textContent='';document.getElementById('providerCustomModelModal').classList.add('show')}
function closeProviderCustomModel(){document.getElementById('providerCustomModelModal').classList.remove('show')}
function addProviderCustomModel(){var name=document.getElementById('pcmName').value.trim();var types=Array.from(document.querySelectorAll('#pcmTypes input:checked')).map(function(input){return input.value});var maxTokens=Number(document.getElementById('pcmMaxTokens').value);var error=document.getElementById('pcmError');if(!name){error.textContent='Model name is required';return}if(piModels.some(function(model){return model.name===name})){error.textContent='Model name already exists';return}if(!types.length){error.textContent='Model type is required';return}if(!Number.isFinite(maxTokens)||maxTokens<0){error.textContent='Max tokens must be 0 or greater';return}var features=document.getElementById('pcmTools').checked?['is_tools']:[];piModels.push({name:name,model_types:types,max_tokens:maxTokens,features:features});piSelectedModels.add(name);piModelsLoaded=true;renderProviderModelList();closeProviderCustomModel()}
async function providerInstanceApi(path,payload){var response=await fetch(path,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(payload)});var data=await response.json().catch(function(){return {}});if(!response.ok||data.code!==0)throw new Error(data.message||('HTTP '+response.status));return data}
async function verifyProviderInstance(){if(!providerInstanceValidate())return;var button=document.getElementById('piVerify');button.disabled=true;providerInstanceSetStatus('Verifying…',false);try{var provider=providerInstanceCard.dataset.provider;var payload=providerSpecialConfig?providerSpecialVerifyPayload():providerInstancePayload();await providerInstanceApi('/api/v1/providers/'+encodeURIComponent(provider)+'/connection',{api_key:payload.api_key,base_url:payload.base_url,region:payload.region,model_info:payload.model_info});providerInstanceSetStatus('Verification succeeded',false)}catch(error){providerInstanceSetStatus(error.message,true)}finally{button.disabled=false}}
async function saveProviderInstance(){if(!providerInstanceValidate())return;var button=document.getElementById('piOk');button.disabled=true;providerInstanceSetStatus('Saving…',false);try{var provider=providerInstanceCard.dataset.provider;await providerInstanceApi('/api/v1/providers/'+encodeURIComponent(provider)+'/instances',providerInstancePayload());closeProviderInstanceModal();await Promise.all([loadAddedModels(),loadModelOptions()])}catch(error){providerInstanceSetStatus(error.message,true)}finally{button.disabled=false}}
const BEDROCK_REGIONS=[['us-east-2','US East (Ohio)'],['us-east-1','US East (N. Virginia)'],['us-west-1','US West (N. California)'],['us-west-2','US West (Oregon)'],['af-south-1','Africa (Cape Town)'],['ap-east-1','Asia Pacific (Hong Kong)'],['ap-south-2','Asia Pacific (Hyderabad)'],['ap-southeast-3','Asia Pacific (Jakarta)'],['ap-southeast-5','Asia Pacific (Malaysia)'],['ap-southeast-4','Asia Pacific (Melbourne)'],['ap-south-1','Asia Pacific (Mumbai)'],['ap-northeast-3','Asia Pacific (Osaka)'],['ap-northeast-2','Asia Pacific (Seoul)'],['ap-southeast-1','Asia Pacific (Singapore)'],['ap-southeast-2','Asia Pacific (Sydney)'],['ap-east-2','Asia Pacific (Taipei)'],['ap-southeast-7','Asia Pacific (Thailand)'],['ap-northeast-1','Asia Pacific (Tokyo)'],['ca-central-1','Canada (Central)'],['ca-west-1','Canada West (Calgary)'],['eu-central-1','Europe (Frankfurt)'],['eu-west-1','Europe (Ireland)'],['eu-west-2','Europe (London)'],['eu-south-1','Europe (Milan)'],['eu-west-3','Europe (Paris)'],['eu-south-2','Europe (Spain)'],['eu-north-1','Europe (Stockholm)'],['eu-central-2','Europe (Zurich)'],['il-central-1','Israel (Tel Aviv)'],['mx-central-1','Mexico (Central)'],['me-south-1','Middle East (Bahrain)'],['me-central-1','Middle East (UAE)'],['sa-east-1','South America (São Paulo)'],['us-gov-east-1','AWS GovCloud (US-East)'],['us-gov-west-1','AWS GovCloud (US-West)']];
var bedrockCard=null;var bedrockInstanceNames=null;var bedrockAuthMode='access_key_secret';
function bedrockStatus(text,error){var status=document.getElementById('bdStatus');status.textContent=text||'';status.style.color=error?'var(--d)':'var(--tm)'}
function bedrockClearErrors(){['bdNameError','bdTypeError','bdModelError','bdAkError','bdSkError','bdRoleArnError','bdRegionError','bdMaxTokensError'].forEach(function(id){document.getElementById(id).textContent=''})}
function bedrockDoc(card){var title=document.getElementById('bedrockTitle');title.textContent=card.dataset.provider||'Bedrock';var doc=document.getElementById('bedrockDoc');var cardDoc=card.querySelector('.available-provider-doc');if(cardDoc){doc.href=cardDoc.href;doc.hidden=false}else{doc.removeAttribute('href');doc.hidden=true}}
function bedrockRegionOptions(){var list=document.getElementById('bdRegionOptions');list.replaceChildren();BEDROCK_REGIONS.forEach(function(entry){var option=document.createElement('option');option.value=entry[0];option.label=entry[1];list.appendChild(option)})}
function bedrockRefreshInstanceState(){var name=document.getElementById('bdName').value.trim();var exists=!!bedrockInstanceNames&&bedrockInstanceNames.has(name);document.getElementById('bdAuthSection').hidden=exists}
function loadBedrockInstanceNames(){var instance=document.getElementById('bedrockModal');fetch('/api/v1/providers/Bedrock/instances').then(function(response){return response.json()}).then(function(data){var names=new Set();(data.data||[]).forEach(function(instance){if(instance&&instance.instance_name)names.add(instance.instance_name)});bedrockInstanceNames=names;if(instance.classList.contains('show'))bedrockRefreshInstanceState()}).catch(function(){bedrockInstanceNames=new Set()})}
function setBedrockAuthMode(mode){bedrockAuthMode=mode;Array.from(document.querySelectorAll('#bdAuthMode [role=radio]')).forEach(function(button){button.setAttribute('aria-checked',button.dataset.mode===mode?'true':'false')});if(mode!=='access_key_secret'){document.getElementById('bdAk').value='';document.getElementById('bdSk').value=''}if(mode!=='iam_role'){document.getElementById('bdRoleArn').value=''}document.getElementById('bdAkSk').hidden=mode!=='access_key_secret';document.getElementById('bdRoleArnField').hidden=mode!=='iam_role';document.getElementById('bdAssumeTip').hidden=mode!=='assume_role'}
function showBedrockModal(card){bedrockCard=card;bedrockDoc(card);bedrockRegionOptions();document.getElementById('bdName').value='';document.getElementById('bdModelName').value='';document.getElementById('bdAk').value='';document.getElementById('bdSk').value='';document.getElementById('bdRoleArn').value='';document.getElementById('bdRegion').value='';document.getElementById('bdMaxTokens').value='8192';Array.from(document.querySelectorAll('#bedrockModal input[name=bdType]')).forEach(function(input){input.checked=input.value==='chat'});setBedrockAuthMode('access_key_secret');bedrockInstanceNames=null;bedrockRefreshInstanceState();bedrockClearErrors();bedrockStatus('',false);document.getElementById('bedrockModal').classList.add('show');loadBedrockInstanceNames()}
function closeBedrockModal(){document.getElementById('bedrockModal').classList.remove('show');bedrockCard=null}
function bedrockSelectedTypes(){return Array.from(document.querySelectorAll('#bedrockModal input[name=bdType]:checked')).map(function(input){return input.value})}
function bedrockInstanceExists(){var name=document.getElementById('bdName').value.trim();return !!bedrockInstanceNames&&bedrockInstanceNames.has(name)}
function bedrockValidate(){bedrockClearErrors();var valid=true;if(!document.getElementById('bdName').value.trim()){document.getElementById('bdNameError').textContent='Please input the instance name!';valid=false}var types=bedrockSelectedTypes();if(!types.length){document.getElementById('bdTypeError').textContent='Please input your model type!';valid=false}if(!document.getElementById('bdModelName').value.trim()){document.getElementById('bdModelError').textContent='Please input your model name!';valid=false}var raw=document.getElementById('bdMaxTokens').value.trim();var maxTokens=raw===''?NaN:Number(raw);if(raw===''||isNaN(maxTokens)||maxTokens<0){document.getElementById('bdMaxTokensError').textContent='Max tokens cannot be less than 0.';valid=false}if(!bedrockInstanceExists()){if(!document.getElementById('bdRegion').value.trim()){document.getElementById('bdRegionError').textContent='Please select!';valid=false}if(bedrockAuthMode==='access_key_secret'){if(!document.getElementById('bdAk').value.trim()){document.getElementById('bdAkError').textContent='Please input your ACCESS KEY';valid=false}if(!document.getElementById('bdSk').value.trim()){document.getElementById('bdSkError').textContent='Please input your SECRET KEY';valid=false}}if(bedrockAuthMode==='iam_role'&&!document.getElementById('bdRoleArn').value.trim()){document.getElementById('bdRoleArnError').textContent='Please enter AWS Role ARN';valid=false}}return valid}
function bedrockKeyPayload(){var key={auth_mode:bedrockAuthMode};if(bedrockAuthMode==='access_key_secret'){key.bedrock_ak=document.getElementById('bdAk').value.trim();key.bedrock_sk=document.getElementById('bdSk').value}if(bedrockAuthMode==='iam_role'){key.aws_role_arn=document.getElementById('bdRoleArn').value.trim()}key.bedrock_region=document.getElementById('bdRegion').value.trim();return key}
function bedrockModelPayload(){return {model_name:document.getElementById('bdModelName').value.trim(),model_type:bedrockSelectedTypes(),max_tokens:Number(document.getElementById('bdMaxTokens').value)}}
function bedrockSavePayload(){return {instance_name:document.getElementById('bdName').value.trim(),llm_factory:'Bedrock',api_key:bedrockKeyPayload(),region:'default',max_tokens:bedrockModelPayload().max_tokens,model_info:[bedrockModelPayload()]}}
async function verifyBedrock(){if(!bedrockValidate())return;var button=document.getElementById('bdVerify');button.disabled=true;bedrockStatus('Verifying…',false);try{await providerInstanceApi('/api/v1/providers/Bedrock/connection',{api_key:JSON.stringify(bedrockKeyPayload()),region:'default',model_info:[bedrockModelPayload()]});bedrockStatus('Your API key is valid.',false)}catch(error){bedrockStatus(error.message,true)}finally{button.disabled=false}}
async function saveBedrock(){if(!bedrockValidate())return;var button=document.getElementById('bdOk');button.disabled=true;bedrockStatus('Saving…',false);try{await providerInstanceApi('/api/v1/providers/Bedrock/instances',bedrockSavePayload());closeBedrockModal();await Promise.all([loadAddedModels(),loadModelOptions()])}catch(error){bedrockStatus(error.message,true)}finally{button.disabled=false}}
var somarkCard=null;
function somarkStatus(text,error){var status=document.getElementById('smStatus');status.textContent=text||'';status.style.color=error?'var(--d)':'var(--tm)'}
function somarkClearErrors(){['smNameError','smModelNameError','smBaseUrlError'].forEach(function(id){document.getElementById(id).textContent=''})}
function somarkDoc(card){var title=document.getElementById('somarkTitle');title.textContent=card.dataset.provider||'SoMark';var doc=document.getElementById('somarkDoc');var cardDoc=card.querySelector('.available-provider-doc');if(cardDoc){doc.href=cardDoc.href;doc.hidden=false}else{doc.removeAttribute('href');doc.hidden=true}}
function showSoMarkModal(card){somarkCard=card;somarkDoc(card);document.getElementById('smName').value='';document.getElementById('smModelName').value='';document.getElementById('smBaseUrl').value='';document.getElementById('smApiKey').value='';document.getElementById('smImageFormat').value='url';document.getElementById('smFormulaFormat').value='latex';document.getElementById('smTableFormat').value='html';document.getElementById('smCsFormat').value='image';Array.from(document.querySelectorAll('#somarkModal input[type=checkbox][role=switch]')).forEach(function(input){var key=input.dataset.key;input.checked=key==='somark_enable_table_image'||key==='somark_enable_image_understanding'});somarkClearErrors();somarkStatus('',false);document.getElementById('somarkModal').classList.add('show')}
function closeSoMarkModal(){document.getElementById('somarkModal').classList.remove('show');somarkCard=null}
function somarkValidate(){somarkClearErrors();var valid=true;if(!document.getElementById('smName').value.trim()){document.getElementById('smNameError').textContent='Please input the instance name!';valid=false}if(!document.getElementById('smModelName').value.trim()){document.getElementById('smModelNameError').textContent='Please input your model name';valid=false}if(!document.getElementById('smBaseUrl').value.trim()){document.getElementById('smBaseUrlError').textContent='Please input the Base URL';valid=false}return valid}
function somarkExtra(){var extra={somark_image_format:document.getElementById('smImageFormat').value,somark_formula_format:document.getElementById('smFormulaFormat').value,somark_table_format:document.getElementById('smTableFormat').value,somark_cs_format:document.getElementById('smCsFormat').value};Array.from(document.querySelectorAll('#somarkModal input[type=checkbox][role=switch]')).forEach(function(input){extra[input.dataset.key]=input.checked});return extra}
function somarkModelPayload(){return {model_name:document.getElementById('smModelName').value.trim(),model_type:['ocr'],max_tokens:0,extra:somarkExtra()}}
function somarkSavePayload(){return {instance_name:document.getElementById('smName').value.trim(),llm_factory:'SoMark',api_key:document.getElementById('smApiKey').value.trim()||'',base_url:document.getElementById('smBaseUrl').value.trim(),max_tokens:0,model_info:[somarkModelPayload()]}}
async function verifySoMark(){if(!somarkValidate())return;var button=document.getElementById('smVerify');button.disabled=true;somarkStatus('Verifying…',false);try{var payload=somarkSavePayload();await providerInstanceApi('/api/v1/providers/SoMark/connection',{api_key:payload.api_key,base_url:payload.base_url,model_info:payload.model_info});somarkStatus('Verified',false)}catch(error){somarkStatus('Verification failed: '+error.message,true)}finally{button.disabled=false}}
async function saveSoMark(){if(!somarkValidate())return;var button=document.getElementById('smOk');button.disabled=true;somarkStatus('Saving…',false);try{await providerInstanceApi('/api/v1/providers/SoMark/instances',somarkSavePayload());closeSoMarkModal();await Promise.all([loadAddedModels(),loadModelOptions()])}catch(error){somarkStatus(error.message,true)}finally{button.disabled=false}}
</script>"##,
    );
    b.push_str(
        r##"<div class='modal-overlay' id='addProvModal' onclick='if(event.target===this)this.classList.remove("show")'><div class='modal' style='max-width:520px'><h2>+ Add Provider</h2><label style='display:block;margin-top:12px'>Provider ID<input id='apId' placeholder='e.g. deepseek' style='width:100%'></label><label style='display:block;margin-top:12px'>Name<input id='apName' placeholder='Display name' style='width:100%'></label><label style='display:block;margin-top:12px'>API Base<input id='apBase' placeholder='https://api.example.com/v1' style='width:100%'></label><label style='display:block;margin-top:12px'>API Key<input id='apKey' type='password' placeholder='Optional' style='width:100%'></label><label style='display:block;margin-top:12px'>Models (comma separated)<input id='apModels' list='apModelSuggestions' placeholder='model-a, model-b' style='width:100%'></label><datalist id='apModelSuggestions'></datalist><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='saveAddProv()'>Save</button><button class='btn btn-o' onclick='document.getElementById("addProvModal").classList.remove("show")'>Cancel</button><span id='apStatus' style='margin-left:8px;color:var(--tm);font-size:12px'></span></div></div></div><script>
var activeProviderFilter='';
function availableCards(){return Array.from(document.querySelectorAll('[data-testid="available-model-card"]'))}
function filterAvailableProviders(){var query=(document.getElementById('providerSearch').value||'').trim().toLowerCase();var searched=0;var shown=0;var counts={};availableCards().forEach(function(card){var searchMatch=(card.dataset.name||'').includes(query);var kinds=(card.dataset.k||'').split(' ').filter(Boolean);if(searchMatch){searched++;new Set(kinds).forEach(function(kind){counts[kind]=(counts[kind]||0)+1})}var visible=searchMatch&&(!activeProviderFilter||kinds.indexOf(activeProviderFilter)>=0);card.style.display=visible?'':'none';if(visible)shown++});document.getElementById('providerCountAll').textContent=searched;['chat','embedding','rerank','tts','speech2text','image2text','ocr'].forEach(function(kind){var count=document.getElementById('providerCount-'+kind);if(count)count.textContent=counts[kind]||0});document.getElementById('availableShown').textContent=shown+' shown';document.getElementById('availableProviderEmpty').style.display=shown?'none':'block'}
function selectProviderFilter(kind){activeProviderFilter=(kind&&activeProviderFilter===kind)?'':kind;document.querySelectorAll('.provider-chip').forEach(function(chip){chip.classList.toggle('on',(chip.dataset.filter||'')===activeProviderFilter)});filterAvailableProviders()}
function showAddProv(card){var modal=document.getElementById('addProvModal');var selected=!!card;document.getElementById('apId').value=selected?(card.dataset.providerId||''):'';document.getElementById('apId').readOnly=selected;document.getElementById('apName').value=selected?(card.dataset.provider||''):'';document.getElementById('apBase').value=selected?(card.dataset.apiBase||''):'';document.getElementById('apKey').value='';document.getElementById('apModels').value=selected?(card.dataset.models||''):'';document.getElementById('apStatus').textContent='';modal.dataset.existing=selected?'true':'false';modal.classList.add('show')}
async function saveAddProv(){var modal=document.getElementById('addProvModal');var id=document.getElementById('apId').value.trim();var name=document.getElementById('apName').value.trim();var base=document.getElementById('apBase').value.trim();var key=document.getElementById('apKey').value.trim();var models=document.getElementById('apModels').value.split(',').map(function(value){return value.trim()}).filter(Boolean);var status=document.getElementById('apStatus');if(!id||!name||!base||!models.length){status.textContent='ID, Name, API Base, and at least one model are required';return}var body={name:name,api_base:base,api_key:key||null,models:models,enabled:true};status.textContent='Saving…';try{var response=await fetch('/api/v1/providers/'+encodeURIComponent(id),{method:modal.dataset.existing==='true'?'PUT':'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});var data=await response.json();if(!response.ok||data.code!==0)throw new Error(data.message||('HTTP '+response.status));status.textContent='Saved ✓';setTimeout(function(){location.reload()},500)}catch(error){status.textContent='Error: '+error.message}}
var apTimer=null;function apSearch(value){var list=document.getElementById('apModelSuggestions');if(!list)return;clearTimeout(apTimer);if(!value||value.length<2){list.replaceChildren();return}apTimer=setTimeout(async function(){try{var response=await fetch('/api/v1/models/search?q='+encodeURIComponent(value)+'&limit=15');var data=await response.json();list.replaceChildren();(data.data||[]).forEach(function(model){var option=document.createElement('option');option.value=model.name;option.label=(model.alias&&model.alias.length)?model.name+' — '+model.alias.slice(0,2).join(' / '):model.name;list.appendChild(option)})}catch(_error){}},250)}
document.getElementById('apModels').addEventListener('input',function(){apSearch(this.value.split(',').pop().trim())});filterAvailableProviders();
</script>"##,
    );
    b.push_str(
        r#"<div class='modal-overlay' id='deleteModelInstanceModal' data-testid='delete-model-instance-dialog' onclick='if(event.target===this)closeDeleteModelInstance()'><div class='modal' style='max-width:460px' role='alertdialog' aria-modal='true' aria-labelledby='deleteModelInstanceTitle'><h2 id='deleteModelInstanceTitle'>Confirm</h2><p id='deleteModelInstanceText' style='margin-top:12px;color:var(--t2)'></p><p id='deleteModelInstanceStatus' class='added-model-dialog-status'></p><div class='added-model-dialog-actions'><button class='btn btn-o' type='button' onclick='closeDeleteModelInstance()'>Cancel</button><button class='btn btn-d' id='deleteModelInstanceOk' type='button' onclick='confirmDeleteModelInstance()'>Confirm</button></div></div></div>
<div class='modal-overlay' id='editInstanceModelModal' data-testid='edit-instance-model-dialog' onclick='if(event.target===this)closeEditInstanceModel()'><div class='modal' style='max-width:520px' role='dialog' aria-modal='true' aria-labelledby='editInstanceModelTitle'><h2 id='editInstanceModelTitle'>Edit model</h2><label class='added-model-edit-field'>Name<input id='editInstanceModelName' disabled></label><fieldset id='editInstanceModelTypes' class='added-model-type-fieldset'><legend>Model types</legend><label><input type='checkbox' value='chat'> LLM</label><label><input type='checkbox' value='embedding'> Embedding</label><label><input type='checkbox' value='image2text'> VLM</label><label><input type='checkbox' value='speech2text'> ASR</label><label><input type='checkbox' value='rerank'> Rerank</label><label><input type='checkbox' value='tts'> TTS</label><label><input type='checkbox' value='ocr'> OCR</label></fieldset><label class='added-model-edit-field'>Max tokens<input id='editInstanceModelMaxTokens' type='number' disabled></label><p id='editInstanceModelStatus' class='added-model-dialog-status'></p><div class='added-model-dialog-actions'><button class='btn btn-o' type='button' onclick='closeEditInstanceModel()'>Cancel</button><button class='btn btn-p' id='editInstanceModelOk' type='button' onclick='saveEditInstanceModel()'>Ok</button></div></div></div><script>const ADDED_PROVIDER_DIRECTORY="#,
    );
    let added_provider_directory: Vec<_> = providers
        .iter()
        .map(|provider| {
            serde_json::json!({
                "id": provider.id,
                "name": crate::providers::canonical_provider_name(&provider.id, &provider.name),
            })
        })
        .collect();
    b.push_str(&json_script_value(&serde_json::json!(
        added_provider_directory
    )));
    let icon_directory: serde_json::Map<String, serde_json::Value> =
        crate::api::tenant_models::factory_llm_entries()
            .iter()
            .map(|entry| {
                let value = match provider_icon(&entry.name) {
                    ProviderIcon::Svg(path) => {
                        serde_json::json!({ "kind": "svg", "value": path })
                    }
                    ProviderIcon::Font(symbol) => {
                        serde_json::json!({ "kind": "font", "value": symbol })
                    }
                };
                (entry.name.clone(), value)
            })
            .collect();
    b.push_str(";const PROVIDER_ICON_DIRECTORY=");
    b.push_str(&serde_json::to_string(&icon_directory).expect("icon directory serializes"));
    b.push_str(
        r#";
function addedNode(tag,className,text){var node=document.createElement(tag);if(className)node.className=className;if(text!==undefined)node.textContent=text;return node}
function providerIconHtml(name){var entry=PROVIDER_ICON_DIRECTORY[name]||{kind:'font',value:'moxing-default'};if(entry.kind==='svg')return "<img class='provider-brand-icon' src='/assets/svg/llm/"+entry.value+"' alt='' aria-hidden='true'>";return "<svg class='provider-brand-icon' aria-hidden='true' focusable='false'><use href='/icons.svg#icon-'+entry.value+''></use></svg>"}
function providerDirectoryEntry(providerId){var entry=ADDED_PROVIDER_DIRECTORY.find(function(provider){return provider.id===providerId});if(entry)return entry;var card=Array.from(document.querySelectorAll('[data-testid="available-model-card"]')).find(function(candidate){return candidate.dataset.providerId===providerId});return {id:providerId,name:card&&card.dataset.provider?card.dataset.provider:providerId}}
async function addedApi(url,options){var response=await fetch(url,options);var data=await response.json().catch(function(){return {}});if(!response.ok||data.code!==0)throw new Error(data.message||('HTTP '+response.status));return data.data}
function addedUrl(providerName,instanceName,suffix){var value='/api/v1/providers/'+encodeURIComponent(providerName)+'/instances';if(instanceName!==undefined)value+='/'+encodeURIComponent(instanceName)+'/models';if(suffix!==undefined)value+='/'+encodeURIComponent(suffix);return value}
async function loadAddedModels(){var list=document.getElementById('addedModelList');list.replaceChildren(addedNode('p','added-model-loading','Loading…'));try{var instances=await addedApi('/api/v1/tenant/models');var groups=new Map();(instances||[]).forEach(function(instance){var directory=providerDirectoryEntry(instance.provider_id);var group=groups.get(instance.provider_id);if(!group){group={providerId:instance.provider_id,providerName:directory.name||instance.provider_id,instances:[]};groups.set(instance.provider_id,group)}group.instances.push(instance)});list.replaceChildren();if(!groups.size){list.appendChild(addedNode('p','added-model-empty','No model provider instances added.'));return}Array.from(groups.values()).forEach(function(group){list.appendChild(renderAddedProvider(group))})}catch(error){list.replaceChildren(addedNode('p','added-model-error','Error: '+error.message))}}
function renderAddedProvider(group){var card=addedNode('article','added-model-card');card.dataset.testid='added-model-card';card.dataset.provider=group.providerName;var head=addedNode('div','added-model-provider-head');var icon=addedNode('span','added-model-provider-icon');icon.innerHTML=providerIconHtml(group.providerName);head.appendChild(icon);head.appendChild(addedNode('span','added-model-provider-name',group.providerName));card.appendChild(head);var instances=addedNode('div','added-model-instances');group.instances.forEach(function(instance,index){instances.appendChild(renderAddedInstance(group,instance,index))});card.appendChild(instances);return card}
function renderAddedInstance(group,instance,index){var row=addedNode('section','added-model-instance');var header=addedNode('div','added-model-instance-head');header.appendChild(addedNode('span','added-model-instance-name',instance.instance_name));var actions=addedNode('div','added-model-instance-actions');var toggle=addedNode('button','btn btn-o btn-sm','View models');toggle.type='button';toggle.setAttribute('aria-expanded','false');var content=addedNode('div','added-model-content');content.hidden=true;content.dataset.loaded='false';content.id='added-model-content-'+group.providerId.replace(/[^a-zA-Z0-9_-]/g,'-')+'-'+instance.instance_id.replace(/[^a-zA-Z0-9_-]/g,'-')+'-'+index;toggle.addEventListener('click',function(){toggleAddedInstance(toggle,content,group.providerName,instance.instance_name)});actions.appendChild(toggle);var remove=addedNode('button','btn btn-d btn-sm added-model-delete','🗑');remove.type='button';remove.setAttribute('aria-label','Delete '+instance.instance_name);remove.addEventListener('click',function(){openDeleteModelInstance(group.providerName,instance.instance_name)});actions.appendChild(remove);header.appendChild(actions);row.appendChild(header);row.appendChild(content);return row}
async function toggleAddedInstance(button,content,providerName,instanceName){var open=button.getAttribute('aria-expanded')==='true';if(open){button.setAttribute('aria-expanded','false');button.textContent='View models';content.hidden=true;return}button.disabled=true;try{if(content.dataset.loaded!=='true')await loadAddedInstanceModels(providerName,instanceName,content);content.hidden=false;button.setAttribute('aria-expanded','true');button.textContent='Hide models'}catch(error){content.replaceChildren(addedNode('p','added-model-error','Error: '+error.message));content.hidden=false}finally{button.disabled=false}}
function modelTypeBadge(type){return addedNode('span','badge added-model-type',type)}
async function loadAddedInstanceModels(providerName,instanceName,content){content.replaceChildren(addedNode('p','added-model-loading','Loading…'));var models=await addedApi(addedUrl(providerName,instanceName));content.replaceChildren();content.dataset.loaded='true';var types=[];(models||[]).forEach(function(model){(model.model_type||[]).forEach(function(type){if(types.indexOf(type)<0)types.push(type)})});if(types.length){var summary=addedNode('div','added-model-type-summary');types.forEach(function(type){summary.appendChild(modelTypeBadge(type))});content.appendChild(summary)}var list=addedNode('ul','added-model-rows');(models||[]).forEach(function(model){list.appendChild(renderAddedModel(providerName,instanceName,model,content))});if(!models||!models.length)list.appendChild(addedNode('li','added-model-empty','No models.'));content.appendChild(list)}
function renderAddedModel(providerName,instanceName,model,content){var row=addedNode('li','added-model-row');var details=addedNode('div','added-model-details');details.appendChild(addedNode('span','added-model-name',model.name));(model.model_type||[]).slice(0,3).forEach(function(type){details.appendChild(modelTypeBadge(type))});if((model.model_type||[]).length>3){var more=modelTypeBadge('...');more.title=model.model_type.slice(3).join('\\n');details.appendChild(more)}var edit=addedNode('button','btn btn-o btn-sm added-model-edit','✎');edit.type='button';edit.setAttribute('aria-label','Edit custom model');edit.addEventListener('click',function(event){event.stopPropagation();openEditInstanceModel(providerName,instanceName,model,content)});details.appendChild(edit);row.appendChild(details);var toggle=document.createElement('input');toggle.type='checkbox';toggle.className='added-model-switch';toggle.setAttribute('role','switch');toggle.setAttribute('aria-label','Set '+model.name+' active');toggle.checked=model.status==='active';toggle.setAttribute('aria-checked',String(toggle.checked));toggle.addEventListener('change',function(){updateAddedModelStatus(toggle,providerName,instanceName,model.name)});row.appendChild(toggle);return row}
async function updateAddedModelStatus(toggle,providerName,instanceName,modelName){var checked=toggle.checked;toggle.disabled=true;try{await addedApi(addedUrl(providerName,instanceName,modelName),{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({status:checked?'active':'inactive'})});toggle.setAttribute('aria-checked',String(checked));await loadModelOptions()}catch(error){toggle.checked=!checked;toggle.setAttribute('aria-checked',String(toggle.checked));alert(error.message)}finally{toggle.disabled=false}}
function openDeleteModelInstance(providerName,instanceName){var modal=document.getElementById('deleteModelInstanceModal');modal.dataset.provider=providerName;modal.dataset.instance=instanceName;document.getElementById('deleteModelInstanceText').textContent='Are you sure you want to delete the instance "'+instanceName+'"?';document.getElementById('deleteModelInstanceStatus').textContent='';modal.classList.add('show')}
function closeDeleteModelInstance(){document.getElementById('deleteModelInstanceModal').classList.remove('show')}
async function confirmDeleteModelInstance(){var modal=document.getElementById('deleteModelInstanceModal');var button=document.getElementById('deleteModelInstanceOk');var status=document.getElementById('deleteModelInstanceStatus');button.disabled=true;status.textContent='Deleting…';try{await addedApi(addedUrl(modal.dataset.provider),{method:'DELETE',headers:{'Content-Type':'application/json'},body:JSON.stringify({instances:[modal.dataset.instance]})});closeDeleteModelInstance();await Promise.all([loadAddedModels(),loadModelOptions()])}catch(error){status.textContent='Error: '+error.message}finally{button.disabled=false}}
function openEditInstanceModel(providerName,instanceName,model,content){var modal=document.getElementById('editInstanceModelModal');modal.dataset.provider=providerName;modal.dataset.instance=instanceName;modal.dataset.model=model.name;modal.dataset.contentId=content.id;document.getElementById('editInstanceModelName').value=model.name;document.getElementById('editInstanceModelMaxTokens').value=model.max_tokens||0;document.querySelectorAll('#editInstanceModelTypes input[type=checkbox]').forEach(function(input){input.checked=(model.model_type||[]).indexOf(input.value)>=0});document.getElementById('editInstanceModelStatus').textContent='';modal.classList.add('show')}
function closeEditInstanceModel(){document.getElementById('editInstanceModelModal').classList.remove('show')}
async function saveEditInstanceModel(){var modal=document.getElementById('editInstanceModelModal');var button=document.getElementById('editInstanceModelOk');var status=document.getElementById('editInstanceModelStatus');var types=Array.from(document.querySelectorAll('#editInstanceModelTypes input[type=checkbox]:checked')).map(function(input){return input.value});if(!types.length){status.textContent='Select at least one model type.';return}button.disabled=true;status.textContent='Saving…';try{await addedApi(addedUrl(modal.dataset.provider,modal.dataset.instance),{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({model_name:[modal.dataset.model],model_type:types})});var content=document.getElementById(modal.dataset.contentId);closeEditInstanceModel();if(content)await loadAddedInstanceModels(modal.dataset.provider,modal.dataset.instance,content);await loadModelOptions()}catch(error){status.textContent='Error: '+error.message}finally{button.disabled=false}}
document.addEventListener('keydown',function(event){if(event.key==='Escape'){closeProviderInstanceModal();closeDeleteModelInstance();closeEditInstanceModel()}});loadAddedModels();
</script>"#,
    );
    let page = if in_user_settings {
        user_setting_layout(t("Model providers", zh), &b, UserSettingSection::Model, zh)
    } else {
        layout(t("Model providers", zh), &b, zh)
    };
    Html(page)
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub async fn not_found_page(req: axum::http::Request<axum::body::Body>) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let mut b = String::new();
    b.push_str("<div style='text-align:center;padding:80px 20px'><div style='font-size:72px;font-weight:800;color:var(--p)'>404</div><h1 style='margin:12px 0'>");
    b.push_str(t("Page not found, please enter a correct address.", zh));
    b.push_str("</h1><p style='color:var(--tm);margin-bottom:24px'>");
    b.push_str(t(
        "The page you requested does not exist or has been moved.",
        zh,
    ));
    b.push_str("</p><a class='btn btn-p' href='/dashboard'>← ");
    b.push_str(t("Back to Dashboard", zh));
    b.push_str("</a><a class='btn btn-o' href='/chat' style='margin-left:8px'>💬 ");
    b.push_str(t("Chat", zh));
    b.push_str("</a></div>");
    Html(layout("404", &b, zh))
}

pub async fn settings_page(
    State(state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let mut b = String::new();
    b.push_str("<h1>⚙️ ");
    b.push_str(t("Settings", zh));
    b.push_str("</h1><div class='card'><h2>System</h2><table><tr><td>Version</td><td class='co'>");
    b.push_str(&crate::build_info::version_line());
    b.push_str("</td></tr><tr><td>Language</td><td>Rust</td></tr><tr><td>Parsers</td><td>17</td></tr><tr><td>API</td><td>29</td></tr></table></div>" );
    b.push_str("<div class='card'><h2>");
    b.push_str(t("Change Password", zh));
    b.push_str("</h2><div style='display:grid;gap:8px;max-width:360px'><input id='curPw' type='password' placeholder='");
    b.push_str(t("Current Password", zh));
    b.push_str("'><input id='newPw' type='password' placeholder='");
    b.push_str(t("New Password (min 6 chars)", zh));
    b.push_str("'><button class='btn btn-p' onclick='changePw()'>");
    b.push_str(t("Update Password", zh));
    b.push_str("</button><p id='pwMsg' style='font-size:12px;color:var(--tm)'></p></div></div>");
    b.push_str("<div class='card'><h2>");
    b.push_str(t("API Keys", zh));
    b.push_str("</h2><p style='color:var(--tm);margin-bottom:12px'>Generate and manage personal API keys.</p><button class='btn btn-p' onclick='genKey()'>Generate</button><div id='akList' style='margin-top:12px'></div></div>");
    b.push_str("<div class='card'><h2>");
    b.push_str(t("Invite", zh));
    b.push_str("</h2><p style='color:var(--tm);margin-bottom:12px'>Invite a registered user to your team.</p><div style='display:grid;gap:8px;max-width:360px'><input id='invEmail' placeholder='user@example.com'><button class='btn btn-p' onclick='inviteUser()'>");
    b.push_str(t("Invite", zh));
    b.push_str("</button><p id='invMsg' style='font-size:12px;color:var(--tm)'></p></div></div>");
    b.push_str("<script>function changePw(){var c=document.getElementById('curPw').value;var n=document.getElementById('newPw').value;if(!c||!n){document.getElementById('pwMsg').textContent='Please fill both fields';return}fetch('/api/v1/user/password',{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({current_password:c,new_password:n})}).then(r=>r.json()).then(d=>{document.getElementById('pwMsg').textContent=d.code===0?'Password updated':'Error: '+(d.message||'unknown')})};function loadKeys(){fetch('/api/v1/system/api_keys').then(r=>r.json()).then(d=>{var keys=d.data||[];var e=document.getElementById('akList');if(!keys.length){e.innerHTML='<p style=color:var(--tm)>No API keys yet.</p>';return}var h='<table><thead><tr><th>Key</th><th>Created</th><th></th></tr></thead><tbody>';keys.forEach(function(k){h+='<tr><td style=font-family:monospace;font-size:12px>'+k.token+'</td><td style=color:var(--tm);font-size:12px>'+k.create_date+'</td><td><button class=\\'btn btn-d btn-sm\\' onclick=\\'delKey(&quot;'+k.token+'&quot;)\\'>Delete</button></td></tr>'});e.innerHTML=h+'</tbody></table>'})};function genKey(){fetch('/api/v1/system/api_keys',{method:'POST'}).then(r=>r.json()).then(d=>{if(d.code===0){loadKeys()}else{alert('Error: '+(d.message||'unknown'))}})};function delKey(k){if(!confirm('Delete this API key?'))return;fetch('/api/v1/system/api_keys/'+encodeURIComponent(k),{method:'DELETE'}).then(r=>r.json()).then(d=>{if(d.code===0){loadKeys()}else{alert('Error: '+(d.message||'unknown'))}})};function inviteUser(){var e=document.getElementById('invEmail').value;if(!e){document.getElementById('invMsg').textContent='Please enter an email';return}fetch('/api/v1/users/invite',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({email:e})}).then(r=>r.json()).then(d=>{document.getElementById('invMsg').textContent=d.code===0?'Invitation sent':'Error: '+(d.message||'unknown')})};loadKeys()</script>");

    // Chat channels (RAGFlow user-setting parity): registered channel builders
    b.push_str("<div class='card' style='margin-top:16px'><h3>Chat Channels</h3>");
    let channel_ids = state.channels.registered_ids().await;
    if channel_ids.is_empty() {
        b.push_str("<p style='color:var(--tm);font-size:12px'>No chat channels registered. Configure via environment (CHANNELS_*) or data sources.</p>");
    } else {
        b.push_str("<table style='width:100%;font-size:13px'><thead><tr><th style='text-align:left'>Platform</th><th style='text-align:left'>Status</th></tr></thead><tbody>");
        for cid in &channel_ids {
            b.push_str(&format!(
                "<tr><td>{}</td><td style='color:var(--s)'>● registered</td></tr>",
                cid
            ));
        }
        b.push_str("</tbody></table>");
    }
    b.push_str("</div>");
    // MCP servers (RAGFlow user-setting parity): built-in MCP tool catalogue
    b.push_str("<div class='card' style='margin-top:16px'><h3>MCP Servers</h3>");
    b.push_str("<p style='color:var(--tm);font-size:12px'>RayRAG exposes a built-in MCP server at <code>/api/v1/mcp/tools</code> with the following tools:</p>");
    b.push_str("<table style='width:100%;font-size:13px'><thead><tr><th style='text-align:left'>Tool</th><th style='text-align:left'>Description</th></tr></thead><tbody>");
    b.push_str("<tr><td><code>search_knowledge</code></td><td style='font-size:12px'>Search RayRAG knowledge base with vector similarity</td></tr>");
    b.push_str("<tr><td><code>list_datasets</code></td><td style='font-size:12px'>List available knowledge bases</td></tr>");
    b.push_str("</tbody></table></div>");
    Html(layout(t("Settings", zh), &b, zh))
}

pub async fn api_docs(req: axum::http::Request<axum::body::Body>) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let b = "<h1>📖 API Docs</h1><div class='card'><p style='color:var(--t2)'>45+ RESTful endpoints. Full RAG pipeline.</p><h3 style='margin-top:16px'>Auth & Users</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>GET</td><td class='co'>/api/v1/system/password-public-key</td><td>Optional RSA password transport public key</td></tr><tr><td>POST</td><td class='co'>/api/v1/auth/login</td><td>Login</td></tr><tr><td>POST</td><td class='co'>/api/v1/user/register</td><td>Register</td></tr><tr><td>PATCH</td><td class='co'>/api/v1/user/password</td><td>Change password</td></tr><tr><td>DELETE/PATCH</td><td class='co'>/api/v1/admin/users/{id}</td><td>Delete user / reset password</td></tr></table><h3>Knowledge Bases</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>GET/POST</td><td class='co'>/api/v1/datasets</td><td>KB list/create</td></tr><tr><td>PUT/DELETE</td><td class='co'>/api/v1/datasets/{id}</td><td>Update config / delete</td></tr><tr><td>POST</td><td class='co'>/api/v1/datasets/{id}/documents</td><td>Upload document</td></tr><tr><td>GET/DELETE/PATCH</td><td class='co'>/api/v1/datasets/{id}/documents/{did}</td><td>Get / delete / rename</td></tr><tr><td>POST</td><td class='co'>/api/v1/datasets/{id}/documents/{did}/reparse</td><td>Reparse</td></tr><tr><td>POST</td><td class='co'>/api/v1/datasets/{id}/graphrag/build</td><td>Build knowledge graph</td></tr><tr><td>POST</td><td class='co'>/api/v1/datasets/{id}/graphrag/query</td><td>Query graph context</td></tr></table><h3>Search & Chat</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>POST</td><td class='co'>/api/v1/retrieval</td><td>Vector/hybrid search</td></tr><tr><td>POST</td><td class='co'>/api/v1/completions</td><td>RAG chat</td></tr><tr><td>POST</td><td class='co'>/api/v1/chats/{id}/completions</td><td>RAG chat (history, SSE)</td></tr><tr><td>DELETE</td><td class='co'>/api/v1/conversations/{id}</td><td>Delete conversation</td></tr><tr><td>PATCH</td><td class='co'>/api/v1/conversations/{id}</td><td>Rename conversation</td></tr><tr><td>POST</td><td class='co'>/api/v1/conversations/{id}/clear</td><td>Clear messages</td></tr><tr><td>GET</td><td class='co'>/api/v1/conversations/{id}/export</td><td>Export Markdown</td></tr></table><h3>OpenAI-Compatible</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>POST</td><td class='co'>/api/v1/chat/completions</td><td>Chat (stream + non-stream)</td></tr><tr><td>POST</td><td class='co'>/api/v1/embeddings</td><td>Embeddings</td></tr><tr><td>POST</td><td class='co'>/api/v1/rerank</td><td>Rerank (Jina format)</td></tr><tr><td>GET</td><td class='co'>/api/v1/openai/models</td><td>OpenAI model list</td></tr></table><h3>Files & Memories</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>GET/DELETE</td><td class='co'>/api/v1/files/{id}</td><td>Get / delete file</td></tr><tr><td>GET</td><td class='co'>/api/v1/files/{id}/download</td><td>Download file</td></tr><tr><td>GET</td><td class='co'>/api/v1/models</td><td>Tenant model selector catalog</td></tr><tr><td>DELETE</td><td class='co'>/api/v1/memories/clear</td><td>Clear all conversations</td></tr></table><h3>Agents & System</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>GET/POST</td><td class='co'>/api/v1/agents</td><td>Agents CRUD</td></tr><tr><td>POST</td><td class='co'>/api/v1/agents/{id}/completions</td><td>Run agent</td></tr><tr><td>GET</td><td class='co'>/api/v1/stats</td><td>Stats</td></tr><tr><td>GET</td><td class='co'>/api/v1/system/healthz</td><td>Health check</td></tr></table></div>";
    Html(layout("API Docs", b, zh))
}

pub async fn status_page(req: axum::http::Request<axum::body::Body>) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let checks = crate::integration::IntegrationCheck::check();
    let passed = checks.iter().filter(|(_, o, _)| *o).count();
    let total = checks.len();
    let pct = passed
        .checked_mul(100)
        .and_then(|value| value.checked_div(total))
        .unwrap_or(0);
    let mut rows = String::new();
    for (name, ok, note) in &checks {
        rows.push_str(&format!(
            "<tr><td>{} {}</td><td style='color:var(--t2)'>{}</td></tr>",
            if *ok { "🟢" } else { "🔴" },
            name,
            note
        ));
    }
    let mut b = String::new();
    b.push_str("<h1>🔧 Status</h1><div class='st'><div class='sc'><span class='lbl'>Modules</span><span class='val'>");
    b.push_str(&total.to_string());
    b.push_str("</span></div><div class='sc'><span class='lbl'>OK</span><span class='val' style='color:var(--s)'>");
    b.push_str(&passed.to_string());
    b.push_str("</span></div><div class='sc'><span class='lbl'>Rate</span><span class='val'>");
    b.push_str(&pct.to_string());
    b.push_str("%</span></div></div><div class='card'><table><thead><tr><th>Module</th><th>Status</th></tr></thead><tbody>");
    b.push_str(&rows);
    b.push_str("</tbody></table></div><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='location.reload()'>↻ ");
    b.push_str(t("Refresh", zh));
    b.push_str("</button><button class='btn btn-o' onclick='copyReport()'>📋 ");
    b.push_str(t("Copy Report", zh));
    b.push_str("</button></div><script>function copyReport(){var rows='';document.querySelectorAll('.card table tbody tr').forEach(function(tr){rows+=tr.textContent.replace(/\\s+/g,' ').trim()+'\\n'});navigator.clipboard.writeText(rows).then(function(){alert('Report copied!')})}</script>");
    Html(layout("Status", &b, zh))
}

/// RAGFlow 0.26.4 parity: chat-apps list page (`/chats`).
pub async fn chat_apps_page(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let apps = state
        .chat_apps
        .as_ref()
        .map(|s| s.list())
        .unwrap_or_default();
    let mut cards = String::new();
    for a in &apps {
        let initial = if a.icon.is_empty() {
            a.name
                .chars()
                .next()
                .unwrap_or('C')
                .to_uppercase()
                .to_string()
        } else {
            a.icon.clone()
        };
        let desc = if a.description.is_empty() {
            "-".to_string()
        } else {
            a.description.clone()
        };
        cards.push_str(&format!(
            "<div class='chat-card' data-name='{name}' data-testid='chat-card' onclick=\"location.href='/chats/{id}'\"><div class='cc-top'><div class='av' style='width:40px;height:40px;font-size:18px'>{icon}</div><div class='cc-meta'><h3 style='font-size:15px;margin:0'>{name}</h3><p style='font-size:12px;color:var(--tm);margin:2px 0 0'>{desc}</p></div><div class='cc-actions' onclick='event.stopPropagation()'><button class='btn btn-o btn-sm' onclick=\"renApp('{id}','{name}')\">✏️</button><button class='btn btn-d btn-sm' onclick=\"delApp('{id}')\">🗑</button></div></div></div>",
            id = a.id, icon = initial, name = html_esc(&a.name, 30), desc = html_esc(&desc, 40)
        ));
    }
    let mut b = String::new();
    // Header: title + search + create button (ListFilterBar parity).
    b.push_str("<div style='display:flex;align-items:center;gap:12px;margin-bottom:24px'>");
    b.push_str(&format!("<h1 style='margin:0'>{}</h1>", t("Chat apps", zh)));
    b.push_str(&format!(
        "<input id='appSearch' placeholder='{}' style='margin-left:auto;width:240px' oninput='filterApps(this.value)'>",
        t("Search", zh)
    ));
    b.push_str(&format!(
        "<button class='btn btn-p' data-testid='create-chat' onclick='openNewApp()'>+ {}</button></div>",
        t("Create chat", zh)
    ));
    b.push_str("<div class='chat-list' id='appList'>");
    if apps.is_empty() {
        // EmptyAppCard parity: clickable large card.
        b.push_str(&format!(
            "<div class='empty-card' data-testid='chats-empty-create' onclick='openNewApp()'><div style='font-size:44px'>💬</div><p style='margin:12px 0 4px;font-size:15px'>{}</p><p style='color:var(--tm);font-size:12px;margin:0'>{}</p></div>",
            t("No chat app created yet", zh),
            t("Click to create your first chat app", zh),
        ));
    } else {
        b.push_str(&cards);
    }
    b.push_str("</div>");
    // Create dialog (RenameDialog parity: name only, 425px).
    b.push_str(&format!("<div class='modal-overlay' id='newAppModal'><div class='modal' style='max-width:425px' data-testid='rename-modal'><h2>{}</h2><input id='appName' placeholder='{}' style='width:100%;margin:12px 0'><div style='display:flex;justify-content:flex-end;gap:8px'><button class='btn btn-o' onclick='closeNewApp()'>{}</button><button class='btn btn-p' data-testid='rename-save' onclick='saveNewApp()'>{}</button></div></div></div>",
        t("Create chat", zh), t("Name", zh), t("Cancel", zh), t("Save", zh)));
    // Rename dialog.
    b.push_str(&format!("<div class='modal-overlay' id='renAppModal'><div class='modal' style='max-width:425px' data-testid='rename-modal'><h2>{}</h2><input id='renAppName' style='width:100%;margin:12px 0'><div style='display:flex;justify-content:flex-end;gap:8px'><button class='btn btn-o' onclick='closeRenApp()'>{}</button><button class='btn btn-p' data-testid='rename-save' onclick='saveRenApp()'>{}</button></div></div></div>",
        t("Rename", zh), t("Cancel", zh), t("Save", zh)));
    b.push_str("<input id='renAppId' type='hidden'>");
    // JS
    b.push_str("<script>");
    b.push_str("function filterApps(v){v=(v||'').toLowerCase();document.querySelectorAll('#appList .chat-card').forEach(function(c){c.style.display=(!v||(c.getAttribute('data-name')||'').toLowerCase().indexOf(v)>=0)?'':'none'})}");
    b.push_str("function openNewApp(){document.getElementById('appName').value='';document.getElementById('newAppModal').classList.add('show')}");
    b.push_str(
        "function closeNewApp(){document.getElementById('newAppModal').classList.remove('show')}",
    );
    b.push_str("function saveNewApp(){var n=document.getElementById('appName').value.trim();if(!n){alert('Name required');return}fetch('/api/v1/chatapps',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n})}).then(function(r){return r.json()}).then(function(d){if(d.code===0){location.reload()}else{alert(d.message||'create failed')}})}");
    b.push_str("function renApp(id,name){document.getElementById('renAppId').value=id;document.getElementById('renAppName').value=name;document.getElementById('renAppModal').classList.add('show')}");
    b.push_str(
        "function closeRenApp(){document.getElementById('renAppModal').classList.remove('show')}",
    );
    b.push_str("function saveRenApp(){var n=document.getElementById('renAppName').value.trim();var id=document.getElementById('renAppId').value;if(!n)return;fetch('/api/v1/chatapps/'+id,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n})}).then(function(r){return r.json()}).then(function(d){if(d.code===0){location.reload()}else{alert(d.message||'rename failed')}})}");
    b.push_str("function delApp(id){if(!confirm('Delete this chat app?'))return;fetch('/api/v1/chatapps/'+id,{method:'DELETE'}).then(function(r){return r.json()}).then(function(d){if(d.code===0){location.reload()}else{alert(d.message||'delete failed')}})}");
    b.push_str("</script>");
    Html(layout(t("Chat apps", zh), &b, zh))
}

/// RAGFlow share-page parity: public /chats/share?shared_id=...&from=chatbots.
/// Unauthenticated; renders title/avatar/prologue and streams /api/v1/chatbots/{id}/completions.
pub async fn chat_share_page(req: axum::http::Request<axum::body::Body>) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let ph = if zh {
        "输入问题…"
    } else {
        "Ask a question…"
    };
    let send = if zh { "发送" } else { "Send" };
    let b = core::str::from_utf8(&include_bytes!("web_share.html")[..])
        .unwrap_or("")
        .to_string()
        .replace("__PH__", ph)
        .replace("__SEND__", send);
    Html(layout(if zh { "分享对话" } else { "Shared Chat" }, &b, zh))
}

/// RAGFlow 0.26.4 parity: chat-app detail page (`/chats/{id}`).
pub async fn chat_app_detail_page(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::extract::Path(app_id): axum::extract::Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let app = state.chat_apps.as_ref().and_then(|s| s.get(&app_id));
    let (name, icon, desc) = match &app {
        Some(a) => {
            let ic = if a.icon.is_empty() {
                a.name
                    .chars()
                    .next()
                    .unwrap_or('C')
                    .to_uppercase()
                    .to_string()
            } else {
                a.icon.clone()
            };
            (
                a.name.clone(),
                ic,
                if a.description.is_empty() {
                    "-".into()
                } else {
                    a.description.clone()
                },
            )
        }
        None => {
            return Html(format!(
                "<h1>404</h1><p>{}: {app_id}</p>",
                t("Chat app not found", zh)
            ));
        }
    };
    let convs: Vec<_> = state
        .conversations
        .list()
        .into_iter()
        .filter(|c| c.app_id.as_deref() == Some(app_id.as_str()))
        .collect();
    let mut cards = String::new();
    for c in convs.iter().rev() {
        let preview = c
            .messages
            .last()
            .map(|m| m.content.chars().take(80).collect::<String>())
            .unwrap_or_else(|| "No messages".into());
        cards.push_str(&format!(
            "<div class='chat-card' onclick=\"location.href='/chat/{0}'\"><h3 style='font-size:14px;margin:0'>{1}</h3><p style='font-size:12px;color:var(--t2);margin:6px 0 0'>{2} msgs</p><p style='font-size:12px;color:var(--tm);margin:6px 0 0'>{3}</p></div>",
            c.id, html_esc(&c.name, 30), c.messages.len(), html_esc(&preview, 60)
        ));
    }
    if convs.is_empty() {
        cards = format!(
            "<p style='color:var(--tm);text-align:center;padding:32px'>{}</p>",
            t("No conversations yet", zh)
        );
    }

    // ---- settings form values (server-rendered from ChatApp) ----
    let app = app.as_ref().unwrap();
    let pc = app.prompt_config.as_ref().and_then(|v| v.as_object());
    let getb = |k: &str, d: bool| {
        pc.and_then(|o| o.get(k))
            .and_then(|v| v.as_bool())
            .unwrap_or(d)
    };
    let gets = |k: &str| {
        pc.and_then(|o| o.get(k))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let getn = |k: &str, d: f64| {
        pc.and_then(|o| o.get(k))
            .and_then(|v| v.as_f64())
            .unwrap_or(d)
    };
    let prologue = gets("prologue");
    let system = gets("system");
    let empty_response = gets("empty_response");
    let quote = getb("quote", true);
    let keyword = getb("keyword", false);
    let tts = getb("tts", false);
    let use_kg = getb("use_kg", false);
    let toc_enhance = getb("toc_enhance", false);
    let similarity = getn("similarity_threshold", 0.2);
    let top_n = getn("top_n", 8.0) as i64;
    let refine_multiturn = getb("refine_multiturn", true);
    let reasoning = getb("reasoning", false);
    let vector_similarity_weight = getn("vector_similarity_weight", 0.3);
    let ref_meta = pc
        .and_then(|o| o.get("reference_metadata"))
        .and_then(|v| v.as_object());
    let ref_include = ref_meta
        .and_then(|o| o.get("include"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let ref_fields = ref_meta
        .and_then(|o| o.get("fields"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    let meta_filter = pc
        .and_then(|o| o.get("meta_data_filter"))
        .and_then(|v| v.as_object());
    let mf_method = meta_filter
        .and_then(|o| o.get("method"))
        .and_then(|v| v.as_str())
        .unwrap_or("disabled")
        .to_string();
    let mf_manual = meta_filter
        .and_then(|o| o.get("manual"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    let rerank_id = app.rerank_id.clone().unwrap_or_default();
    let rtop_k = app
        .top_k
        .map(|v| v.to_string())
        .unwrap_or_else(|| "1024".into());
    let cross: Vec<String> = pc
        .and_then(|o| o.get("cross_languages"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let llm_id = app.llm_id.clone().unwrap_or_default();
    let mut kb_opts = String::new();
    for kb in state.kbs.list() {
        let sel = if app.kb_ids.contains(&kb.id) {
            " selected"
        } else {
            ""
        };
        kb_opts.push_str(&format!(
            "<option value='{}'{sel}>{}</option>",
            kb.id,
            html_esc(&kb.name, 40)
        ));
    }

    let mut b = String::new();
    // Header row: back + app identity + Settings button (RAGFlow chat-settings parity).
    b.push_str(&format!(
        "<div style='display:flex;align-items:center;gap:12px;margin-bottom:20px'><a class='btn btn-o btn-sm' href='/chats' style='text-decoration:none'>←</a><div class='av' style='width:44px;height:44px;font-size:20px'>{}</div><div><h1 style='margin:0;font-size:18px'>{}</h1><p style='color:var(--tm);font-size:12px;margin:2px 0 0'>{}</p></div><button class='btn btn-p' style='margin-left:auto' onclick='newConv(\"{}\")'>+ {}</button><button class='btn btn-o' style='margin-left:8px' data-testid='chat-settings' onclick='toggleSettings()'>⚙</button></div>",
        icon, html_esc(&name, 40), html_esc(&desc, 60), app_id, t("New conversation", zh)
    ));
    b.push_str("<div class='chat-list'>");
    b.push_str(&cards);
    b.push_str("</div>");

    // ---- Settings drawer (440px, RAGFlow chat-detail-settings parity) ----
    let sw = |k: &str, on: bool| {
        if on {
            format!("<input type='checkbox' id='sw_{k}' checked>")
        } else {
            format!("<input type='checkbox' id='sw_{k}'>")
        }
    };
    let langs = [
        "Chinese",
        "English",
        "Japanese",
        "Korean",
        "French",
        "German",
        "Spanish",
        "Portuguese",
        "Italian",
        "Dutch",
        "Polish",
    ];
    let mut chips = String::new();
    for l in langs {
        let on = if cross.contains(&l.to_string()) {
            " on"
        } else {
            ""
        };
        chips.push_str(&format!(
            "<span class='lang-chip{on}' data-lang='{l}' onclick='toggleLang(this)'>{l}</span>"
        ));
    }
    b.push_str(&format!(
        "<div class='settings-drawer' id='settingsDrawer'><div style='padding:16px 20px;display:flex;justify-content:space-between;align-items:center;border-bottom:1px solid var(--bd)'><b>{}</b><button class='btn btn-o btn-sm' data-testid='chat-detail-settings-close' onclick='toggleSettings()'>✕</button></div><div style='padding:16px 20px;overflow-y:auto;flex:1'>",
        t("Chat Setting", zh)
    ));
    // Basic settings section
    b.push_str(&format!(
        "<div class='set-group'><b class='set-title'>{}</b><label class='set-label'>{}</label><input id='f_icon' value='{}' placeholder='🤖' style='width:100%'><label class='set-label'>{}</label><input id='f_name' value='{}' style='width:100%'><label class='set-label'>{}</label><textarea id='f_desc' rows='2' style='width:100%'>{}</textarea>",
        t("Basic Settings", zh), t("Icon", zh), html_esc(&app.icon, 8), t("Name", zh), html_esc(&app.name, 128), t("Description", zh), html_esc(&app.description, 500)
    ));
    push_default_model_selector(&mut b, &DEFAULT_MODEL_FIELDS[0], zh);
    b.push_str(&format!(
        "<script>var dmSaved={};document.getElementById('dmLlm').dataset.value=dmSaved;document.getElementById('dmLlm').dataset.savedValue=dmSaved;</script>",
        serde_json::to_string(&llm_id).unwrap_or_default()
    ));
    b.push_str(&format!(
        "<label class='set-label'>{} (prologue)</label><textarea id='f_prologue' rows='3' style='width:100%'>{}</textarea><label class='set-label'>{}</label><select id='f_kbs' multiple style='width:100%;min-height:72px'>{}</select>",
        t("Opener", zh), html_esc(&prologue, 2000), t("Knowledge Bases", zh), kb_opts
    ));
    b.push_str("</div>");
    // Prompt engine section
    b.push_str(&format!(
        "<div class='set-group'><b class='set-title'>{}</b><label class='set-label'>{}</label><textarea id='f_system' rows='4' style='width:100%'>{}</textarea><label class='set-label'>{}</label><div id='langChips' style='display:flex;flex-wrap:wrap;gap:6px;margin-top:6px'>{}</div>",
        t("Prompt Engine", zh), t("System Prompt", zh), html_esc(&system, 4000), t("Cross Languages", zh), chips
    ));
    for (k, label) in [
        ("quote", "Quote"),
        ("keyword", "Keyword"),
        ("tts", "TTS"),
        ("use_kg", "Use KG"),
        ("toc_enhance", "TOC Enhance"),
    ] {
        let on = match k {
            "quote" => quote,
            "keyword" => keyword,
            "tts" => tts,
            "use_kg" => use_kg,
            _ => toc_enhance,
        };
        b.push_str(&format!("<label class='set-switch'>{}</label><span class='set-sw-label' style='margin-left:-6px'>{}</span>", sw(k, on), label));
    }
    for (k, label) in [
        ("quote", "Quote"),
        ("keyword", "Keyword"),
        ("tts", "TTS"),
        ("use_kg", "Use KG"),
        ("toc_enhance", "TOC Enhance"),
    ] {
        let on = match k {
            "quote" => quote,
            "keyword" => keyword,
            "tts" => tts,
            "use_kg" => use_kg,
            _ => toc_enhance,
        };
        b.push_str(&format!("<label class='set-switch'>{}</label><span class='set-sw-label' style='margin-left:-6px'>{}</span>", sw(k, on), label));
    }
    for (k, label, on) in [
        ("refine_multiturn", "Refine Multiturn", refine_multiturn),
        ("reasoning", "Reasoning", reasoning),
    ] {
        b.push_str(&format!("<label class='set-switch'>{}</label><span class='set-sw-label' style='margin-left:-6px'>{}</span>", sw(k, on), label));
    }
    b.push_str(&format!(
        "<label class='set-label' style='margin-top:8px'>{}</label><input id='f_rerank' value='{}' placeholder='BAAI/bge-reranker-v2-m3' style='width:100%'><label class='set-label'>{}</label><input id='f_rtopk' type='number' min='1' max='4096' value='{}' style='width:120px'>",
        t("Rerank Model", zh), html_esc(&rerank_id, 80), t("Rerank Top K", zh), rtop_k
    ));
    b.push_str(&format!(
        "<div style='display:flex;gap:12px;margin-top:8px'><div style='flex:1'><label class='set-label'>{}</label><input id='f_sim' type='number' step='0.05' min='0' max='1' value='{similarity}' style='width:100%'></div><div style='flex:1'><label class='set-label'>{}</label><input id='f_topn' type='number' step='1' min='1' max='128' value='{top_n}' style='width:100%'></div><div style='flex:1'><label class='set-label'>{}</label><input id='f_vsw' type='number' step='0.05' min='0' max='1' value='{vector_similarity_weight}' style='width:100%'></div></div><label class='set-label' style='margin-top:8px'>{}</label><textarea id='f_empty' rows='2' style='width:100%'>{}</textarea>",
        t("Similarity Threshold", zh), t("Top N", zh), t("Vector Weight", zh), t("Empty Response", zh), html_esc(&empty_response, 1000)
    ));
    b.push_str(&format!(
        "<label class='set-switch' style='margin-top:8px'><input type='checkbox' id='sw_refmeta'{}><span class='set-sw-label'>{}</span></label><label class='set-label'>{}</label><input id='f_reffields' value='{}' placeholder='title,author' style='width:100%'>",
        if ref_include { " checked" } else { "" }, t("Show Chunk Metadata", zh), t("Metadata Fields", zh), html_esc(&ref_fields, 200)
    ));
    let mf_sel = |v: &str| if v == mf_method { " selected" } else { "" };
    b.push_str(&format!(
        "<label class='set-label' style='margin-top:8px'>{}</label><select id='f_mfmethod' style='width:100%'><option value='disabled'{}>{}</option><option value='auto'{}>{}</option><option value='semi_auto'{}>{}</option><option value='manual'{}>{}</option></select><label class='set-label'>{}</label><input id='f_mfmanual' value='{}' placeholder='field1,field2' style='width:100%'>",
        t("Metadata Filter", zh),
        mf_sel("disabled"), t("Disabled", zh),
        mf_sel("auto"), t("Automatic", zh),
        mf_sel("semi_auto"), t("Semi Automatic", zh),
        mf_sel("manual"), t("Manual", zh),
        t("Manual Fields", zh), html_esc(&mf_manual, 300)
    ));
    b.push_str("</div>");
    // Footer buttons
    b.push_str(&format!(
        "<div style='display:flex;justify-content:flex-end;gap:8px;padding:12px 20px;border-top:1px solid var(--bd)'><button class='btn btn-o' data-testid='chat-detail-settings-cancel' onclick='toggleSettings()'>{}</button><button class='btn btn-p' onclick='saveSettings()'>{}</button></div>",
        t("Cancel", zh), t("Save", zh)
    ));
    b.push_str("</div>");
    // JS
    let llm_fields_payload = serde_json::json!([
        {
            "fieldId": "llm_id",
            "domId": "dmLlm",
            "updateType": "chat",
            "responseType": "chat",
            "acceptedTypes": ["chat", "image2text"],
            "placeholder": "Please select"
        }
    ]);
    b.push_str("<script>const MODEL_FIELDS=");
    b.push_str(&json_script_value(&llm_fields_payload));
    b.push_str(DEFAULT_MODEL_TREE_JS_HEAD);
    b.push_str(DEFAULT_MODEL_TREE_JS_BODY);
    let selector_boot = String::from("async ")
        + "function loadKbModelCatalog(){try{var response=await fetch('/api/v1/models');var data=await response.json();if(!response.ok||data.code!==0)return;DEFAULT_MODEL_CATALOG=(data.data||[]).map(normalizeDefaultModel).filter(function(model){return model.selector});MODEL_FIELDS.forEach(function(field){var state=defaultModelState(field);state.expanded.clear();expandDefaultModelTree(field);renderDefaultModelTrigger(field);renderDefaultModelTree(field)});normalizeDefaultModelStoredValues()}catch(error){}}"
        + "\n"
        + "function normalizeDefaultModelStoredValues(){MODEL_FIELDS.forEach(function(field){var trigger=document.getElementById(field.domId);if(!trigger)return;var value=trigger.dataset.value||'';if(!value||parseDefaultModelValue(value))return;var parts=value.split('/');if(parts.length!==3)return;var match=DEFAULT_MODEL_CATALOG.find(function(item){return item.name===parts[2]&&item.instance_id===parts[1]&&item.provider_id===parts[0]});if(!match)return;trigger.dataset.value=match.selector;trigger.dataset.savedValue=match.selector;renderDefaultModelTrigger(field)})}"
        + "\nloadKbModelCatalog().then(function(){var trig=document.getElementById('dmLlm');if(trig&&!trig.dataset.savedValue&&!trig.dataset.value){var first=DEFAULT_MODEL_CATALOG.find(function(m){return m.model_type.indexOf('chat')>=0});if(first){trig.dataset.value=first.selector;normalizeDefaultModelStoredValues();}}});</script>";
    b.push_str(&selector_boot);
    b.push_str(&format!(
        "<script>function toggleSettings(){{var d=document.getElementById('settingsDrawer');d.classList.toggle('open')}}function toggleLang(el){{el.classList.toggle('on')}}function newConv(appId){{fetch('/api/v1/chats',{{method:'POST',headers:{{'Content-Type':'application/json'}},body:JSON.stringify({{question:'New conversation',app_id:appId}})}}).then(function(r){{return r.json()}}).then(function(d){{if(d.code===0&&d.data){{location.href='/chat/'+d.data.id}}else{{alert(d.message||'create failed')}}}})}}function saveSettings(){{var appId='{app_id}';var langs=Array.from(document.querySelectorAll('#langChips .lang-chip.on')).map(function(x){{return x.getAttribute('data-lang')}});var kbs=Array.from(document.getElementById('f_kbs').selectedOptions).map(function(o){{return o.value}});var body={{name:document.getElementById('f_name').value,description:document.getElementById('f_desc').value,icon:document.getElementById('f_icon').value,kb_ids:kbs,llm_id:document.getElementById('dmLlm').dataset.value||null,llm_setting:null,prompt_config:{{prologue:document.getElementById('f_prologue').value,system:document.getElementById('f_system').value,cross_languages:langs,quote:document.getElementById('sw_quote').checked,keyword:document.getElementById('sw_keyword').checked,tts:document.getElementById('sw_tts').checked,use_kg:document.getElementById('sw_use_kg').checked,toc_enhance:document.getElementById('sw_toc_enhance').checked,refine_multiturn:document.getElementById('sw_refine_multiturn').checked,reasoning:document.getElementById('sw_reasoning').checked,similarity_threshold:parseFloat(document.getElementById('f_sim').value)||0.2,vector_similarity_weight:parseFloat(document.getElementById('f_vsw').value)||0.3,top_n:parseInt(document.getElementById('f_topn').value)||8,empty_response:document.getElementById('f_empty').value,reference_metadata:{{include:document.getElementById('sw_refmeta').checked,fields:document.getElementById('f_reffields').value.split(',').map(function(s){{return s.trim()}}).filter(Boolean)}},meta_data_filter:{{method:document.getElementById('f_mfmethod').value,manual:document.getElementById('f_mfmanual').value.split(',').map(function(s){{return s.trim()}}).filter(Boolean)}}}},rerank_id:document.getElementById('f_rerank').value.trim()||null,top_k:parseInt(document.getElementById('f_rtopk').value)||1024}};fetch('/api/v1/chatapps/'+appId,{{method:'PUT',headers:{{'Content-Type':'application/json'}},body:JSON.stringify(body)}}).then(function(r){{return r.json()}}).then(function(d){{if(d.code===0){{location.reload()}}else{{alert(d.message||'save failed')}}}})}}</script>",
        app_id = app_id
    ));
    Html(layout(t("Chat apps", zh), &b, zh))
}

/// Minimal HTML escape for user-controlled strings.
fn html_esc(input: &str, max_chars: usize) -> String {
    let s: String = input.chars().take(max_chars).collect();
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// RAGFlow 0.26.4 parity: search-apps list page (`/searches`).
pub async fn search_apps_page(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let apps = state
        .search_apps
        .as_ref()
        .map(|s| s.list())
        .unwrap_or_default();
    let mut cards = String::new();
    for a in &apps {
        let initial = if a.icon.is_empty() {
            a.name
                .chars()
                .next()
                .unwrap_or('S')
                .to_uppercase()
                .to_string()
        } else {
            a.icon.clone()
        };
        let desc = if a.description.is_empty() {
            "-".to_string()
        } else {
            a.description.clone()
        };
        cards.push_str(&format!(
            "<div class='chat-card' data-name='{name}' data-testid='search-card' onclick=\"location.href='/searches/{id}'\"><div class='cc-top'><div class='av' style='width:40px;height:40px;font-size:18px'>{icon}</div><div class='cc-meta'><h3 style='font-size:15px;margin:0'>{name}</h3><p style='font-size:12px;color:var(--tm);margin:2px 0 0'>{desc}</p></div><div class='cc-actions' onclick='event.stopPropagation()'><button class='btn btn-o btn-sm' onclick=\"renSApp('{id}','{name}')\">✏️</button><button class='btn btn-d btn-sm' onclick=\"delSApp('{id}')\">🗑</button></div></div></div>",
            id = a.id, icon = initial, name = html_esc(&a.name, 30), desc = html_esc(&desc, 40)
        ));
    }
    let mut b = String::new();
    b.push_str("<div style='display:flex;align-items:center;gap:12px;margin-bottom:24px'>");
    b.push_str(&format!(
        "<h1 style='margin:0'>{}</h1>",
        t("Search apps", zh)
    ));
    b.push_str(&format!(
        "<input id='sappSearch' placeholder='{}' style='margin-left:auto;width:240px' oninput='filterSApps(this.value)'>",
        t("Search", zh)
    ));
    b.push_str(&format!(
        "<button class='btn btn-p' data-testid='create-search' onclick='openNewSApp()'>+ {}</button></div>",
        t("Create search", zh)
    ));
    b.push_str("<div class='chat-list' id='sappList'>");
    if apps.is_empty() {
        b.push_str(&format!(
            "<div class='empty-card' data-testid='search-empty-create' onclick='openNewSApp()'><div style='font-size:44px'>🔎</div><p style='margin:12px 0 4px;font-size:15px'>{}</p><p style='color:var(--tm);font-size:12px;margin:0'>{}</p></div>",
            t("No search app created yet", zh),
            t("Click to create your first search app", zh),
        ));
    } else {
        b.push_str(&cards);
    }
    b.push_str("</div>");
    b.push_str(&format!("<div class='modal-overlay' id='newSAppModal'><div class='modal' style='max-width:425px' data-testid='rename-modal'><h2>{}</h2><input id='sappName' placeholder='{}' style='width:100%;margin:12px 0'><div style='display:flex;justify-content:flex-end;gap:8px'><button class='btn btn-o' onclick='closeNewSApp()'>{}</button><button class='btn btn-p' data-testid='rename-save' onclick='saveNewSApp()'>{}</button></div></div></div>",
        t("Create search", zh), t("Name", zh), t("Cancel", zh), t("Save", zh)));
    b.push_str(&format!("<div class='modal-overlay' id='renSAppModal'><div class='modal' style='max-width:425px' data-testid='rename-modal'><h2>{}</h2><input id='renSAppName' style='width:100%;margin:12px 0'><div style='display:flex;justify-content:flex-end;gap:8px'><button class='btn btn-o' onclick='closeRenSApp()'>{}</button><button class='btn btn-p' data-testid='rename-save' onclick='saveRenSApp()'>{}</button></div></div></div>",
        t("Rename", zh), t("Cancel", zh), t("Save", zh)));
    b.push_str("<input id='renSAppId' type='hidden'>");
    b.push_str("<script>");
    b.push_str("function filterSApps(v){v=(v||'').toLowerCase();document.querySelectorAll('#sappList .chat-card').forEach(function(c){c.style.display=(!v||(c.getAttribute('data-name')||'').toLowerCase().indexOf(v)>=0)?'':'none'})}");
    b.push_str("function openNewSApp(){document.getElementById('sappName').value='';document.getElementById('newSAppModal').classList.add('show')}");
    b.push_str(
        "function closeNewSApp(){document.getElementById('newSAppModal').classList.remove('show')}",
    );
    b.push_str("function saveNewSApp(){var n=document.getElementById('sappName').value.trim();if(!n){alert('Name required');return}fetch('/api/v1/searchapps',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n})}).then(function(r){return r.json()}).then(function(d){if(d.code===0){location.reload()}else{alert(d.message||'create failed')}})}");
    b.push_str("function renSApp(id,name){document.getElementById('renSAppId').value=id;document.getElementById('renSAppName').value=name;document.getElementById('renSAppModal').classList.add('show')}");
    b.push_str(
        "function closeRenSApp(){document.getElementById('renSAppModal').classList.remove('show')}",
    );
    b.push_str("function saveRenSApp(){var n=document.getElementById('renSAppName').value.trim();var id=document.getElementById('renSAppId').value;if(!n)return;fetch('/api/v1/searchapps/'+id,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n})}).then(function(r){return r.json()}).then(function(d){if(d.code===0){location.reload()}else{alert(d.message||'rename failed')}})}");
    b.push_str("function delSApp(id){if(!confirm('Delete this search app?'))return;fetch('/api/v1/searchapps/'+id,{method:'DELETE'}).then(function(r){return r.json()}).then(function(d){if(d.code===0){location.reload()}else{alert(d.message||'delete failed')}})}");
    b.push_str("</script>");
    Html(layout(t("Search apps", zh), &b, zh))
}

/// Upstream next-search/search-setting.tsx parity: the search-app rerank
/// section is a `use_rerank` switch that reveals a rerank `ModelTreeSelect`
/// (shared tree selector, accepted type `rerank`) when enabled.
const SEARCH_RERANK_SELECTOR_FIELD: DefaultModelField = DefaultModelField {
    field_id: "rerank_id",
    dom_id: "saRerankSel",
    label: "Model",
    update_type: "rerank_id",
    response_type: "rerank",
    accepted_types: &["rerank"],
    required: false,
    tooltip: "",
    placeholder: "Please select",
};

/// RAGFlow 0.26.4 parity: search-app detail page (`/searches/{id}`).
pub async fn search_app_detail_page(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::extract::Path(app_id): axum::extract::Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let app = state.search_apps.as_ref().and_then(|s| s.get(&app_id));
    let app = match app {
        Some(a) => a,
        None => {
            return Html(format!(
                "<h1>404</h1><p>{}: {app_id}</p>",
                t("Search app not found", zh)
            ));
        }
    };
    let initial = if app.icon.is_empty() {
        app.name
            .chars()
            .next()
            .unwrap_or('S')
            .to_uppercase()
            .to_string()
    } else {
        app.icon.clone()
    };
    let mut kb_opts = String::new();
    for kb in state.kbs.list() {
        let sel = if app.kb_ids.contains(&kb.id) {
            " selected"
        } else {
            ""
        };
        kb_opts.push_str(&format!(
            "<option value='{}'{sel}>{}</option>",
            kb.id,
            html_esc(&kb.name, 40)
        ));
    }
    let ck = |on: bool| if on { " checked" } else { "" };
    let mut b = String::new();
    b.push_str(&format!(
        "<div style='display:flex;align-items:center;gap:12px;margin-bottom:20px'><a class='btn btn-o btn-sm' href='/searches' style='text-decoration:none'>←</a><div class='av' style='width:44px;height:44px;font-size:20px'>{}</div><div><h1 style='margin:0;font-size:18px'>{}</h1><p style='color:var(--tm);font-size:12px;margin:2px 0 0'>{}</p></div><button class='btn btn-p' style='margin-left:auto' onclick=\"location.href='/search?sapp={}'\">🔍 {}</button></div>",
        initial, html_esc(&app.name, 40), html_esc(if app.description.is_empty() { "-" } else { &app.description }, 60), app_id, t("Test search", zh)
    ));
    b.push_str(&format!(
        "<div class='set-group'><b class='set-title'>{}</b><label class='set-label'>{}</label><select id='sa_kbs' multiple style='width:100%;min-height:72px'>{}</select><div style='display:flex;gap:16px;margin-top:10px'><label class='set-switch'><input type='checkbox' id='sa_hybrid'{}><span class='set-sw-label'>{}</span></label><label class='set-switch'><input type='checkbox' id='sa_rerank' onchange='saRerankToggle()'{}><span class='set-sw-label'>{}</span></label></div>",
        t("Basic Settings", zh), t("Knowledge Bases", zh), kb_opts, ck(app.hybrid), t("Hybrid", zh), ck(app.rerank), t("Rerank Model", zh)
    ));
    // Upstream search-setting.tsx: the rerank model selector renders only
    // when use_rerank is enabled.
    b.push_str("<div id='saRerankRow' style='display:none;margin-top:8px'>");
    push_default_model_selector(&mut b, &SEARCH_RERANK_SELECTOR_FIELD, zh);
    b.push_str("</div>");
    b.push_str(&format!(
        "<label class='set-label'>{}</label><input id='sa_topk' type='number' min='1' max='128' value='{}' style='width:120px'><label class='set-label'>{}</label><input id='sa_thresh' type='number' step='0.05' min='0' max='1' value='{}' style='width:120px'><label class='set-label'>{}</label><input id='sa_vsw' type='number' step='0.05' min='0' max='1' value='{}' style='width:120px'></div>",
        t("Top-K", zh), app.top_k, t("Similarity Threshold", zh), app.similarity_threshold, t("Vector Weight", zh), app.vector_similarity_weight
    ));
    b.push_str(&format!(
        "<div style='display:flex;justify-content:flex-end;gap:8px'><button class='btn btn-p' onclick='saveSApp()'>{}</button></div>",
        t("Save", zh)
    ));
    b.push_str(&format!(
        "<script>function saveSApp(){{var id='{app_id}';var kbs=Array.from(document.getElementById('sa_kbs').selectedOptions).map(function(o){{return o.value}});fetch('/api/v1/searchapps/'+id,{{method:'PUT',headers:{{'Content-Type':'application/json'}},body:JSON.stringify({{kb_ids:kbs,hybrid:document.getElementById('sa_hybrid').checked,rerank:document.getElementById('sa_rerank').checked,rerank_id:(document.getElementById('saRerankSel')&&document.getElementById('saRerankSel').dataset.value)||'',top_k:parseInt(document.getElementById('sa_topk').value)||8,similarity_threshold:parseFloat(document.getElementById('sa_thresh').value),vector_similarity_weight:parseFloat(document.getElementById('sa_vsw').value)}})}}).then(function(r){{return r.json()}}).then(function(d){{if(d.code===0){{location.reload()}}else{{alert(d.message||'save failed')}}}})}}</script>",
        app_id = app_id
    ));
    let app_seed = json_script_value(&serde_json::json!({ "rerank_id": app.rerank_id }));
    b.push_str("<script>window.__APP_SEED__=");
    b.push_str(&app_seed);
    b.push_str(";</script>");
    b.push_str("<script>const MODEL_FIELDS=");
    b.push_str(&json_script_value(&serde_json::json!([{
        "fieldId": "rerank_id",
        "domId": "saRerankSel",
        "updateType": "rerank_id",
        "responseType": "rerank",
        "acceptedTypes": ["rerank"],
        "required": false,
        "placeholder": "Please select"
    }])));
    b.push_str(DEFAULT_MODEL_TREE_JS_HEAD);
    b.push_str(DEFAULT_MODEL_TREE_JS_BODY);
    b.push_str("function seedModelTrigger(type,value){var field=modelField(type);if(!field)return;var trigger=document.getElementById(field.domId);if(!trigger)return;trigger.dataset.value=String(value||'');trigger.dataset.savedValue=String(value||'');renderDefaultModelTrigger(field)}");
    b.push_str("function saRerankToggle(){var on=document.getElementById('sa_rerank').checked;var row=document.getElementById('saRerankRow');if(row)row.style.display=on?'':'none'}");
    b.push_str("async function loadSearchAppModelCatalog(){try{var response=await fetch('/api/v1/models');var data=await response.json();if(!response.ok||data.code!==0)return;DEFAULT_MODEL_CATALOG=(data.data||[]).map(normalizeDefaultModel).filter(function(model){return model.selector});MODEL_FIELDS.forEach(function(field){var state=defaultModelState(field);state.expanded.clear();expandDefaultModelTree(field);renderDefaultModelTrigger(field);renderDefaultModelTree(field)})}catch(error){}}");
    b.push_str("loadSearchAppModelCatalog().then(function(){var seed=window.__APP_SEED__||{};if(seed.rerank_id)seedModelTrigger('rerank_id',seed.rerank_id);saRerankToggle();});");
    b.push_str("</script>");
    Html(layout(t("Search apps", zh), &b, zh))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escape_escapes_special_chars() {
        assert_eq!(
            html_escape("a&b<c>d\"e'f"),
            "a&amp;b&lt;c&gt;d&quot;e&#39;f"
        );
        assert_eq!(html_escape("plain"), "plain");
    }

    #[test]
    fn zh_translations_cover_new_button_labels() {
        for key in [
            "Run",
            "Publish",
            "Save",
            "Cancel",
            "Copy",
            "Download",
            "Install",
            "Uninstall",
            "Components",
            "Run Log",
            "Add User",
            "API Keys",
            "Delete Key",
            "Close",
            "New Agent",
            "Create",
            "Delete",
            "Rename",
            "Upload",
            "Generate",
        ] {
            let zh_value = t(key, true);
            assert_ne!(zh_value, key, "missing zh translation for {key}");
        }
    }

    #[test]
    fn english_is_identity() {
        assert_eq!(t("Run", false), "Run");
        assert_eq!(t("任意键", false), "任意键");
    }
}

#[cfg(test)]
mod kb_ui_tests {
    use super::*;
    use crate::api::compilation_templates::CompilationTemplateStore;
    use crate::api::document::DocStore;
    use crate::api::document_metadata::DocumentMetadataStore;
    use crate::api::evaluation::EvaluationStore;
    use crate::api::features::{
        AgentRunRegistry, AgentStore, CanvasVersionStore, MemoryStore, ProviderStore, TaskQueue,
    };
    use crate::api::file_mgr::FileStore;
    use crate::api::tenant_models::TenantModelStore;
    use crate::auth::UserStore;
    use crate::channels::ChannelRegistry;
    use crate::kb::{KbStore, TenantStore};
    use crate::llm::ConvStore;
    use crate::logging::LogLevelManager;
    use crate::rerank::RerankerManager;
    use crate::search::SearchEngine;
    use crate::task_executor::TaskExecutor;
    use axum::body::Body;
    use axum::extract::{Path, State};
    use axum::http::Request;
    use axum::response::IntoResponse;
    use std::sync::{Arc, RwLock};

    fn test_state() -> (tempfile::TempDir, Arc<AppState>) {
        let root = tempfile::tempdir().unwrap();
        let static_dir = root.path().join("web");
        std::fs::create_dir_all(&static_dir).unwrap();
        // 预写空 users.json，避免 UserStore::new 首次 bootstrap 依赖 RAYRAG_ADMIN_PASSWORD。
        let users_path = root.path().join("users.json");
        std::fs::write(&users_path, "[]").unwrap();
        let users = Arc::new(UserStore::new(users_path.to_str().unwrap()).unwrap());
        let state = Arc::new(AppState {
            static_dir: static_dir.to_string_lossy().into_owned(),
            port: 0,
            users,
            kbs: Arc::new(KbStore::new(root.path().join("kbs.json").to_str().unwrap()).unwrap()),
            tenants: Arc::new(
                TenantStore::new(root.path().join("tenants.json").to_str().unwrap()).unwrap(),
            ),
            conversations: Arc::new(
                ConvStore::new(root.path().join("conversations.json").to_str().unwrap()).unwrap(),
            ),
            llm: None,
            embedder: None,
            reranker: Arc::new(RerankerManager::in_memory(None)),
            graphs: Arc::new(crate::graph_store::GraphStore::in_memory()),
            engine: Arc::new(RwLock::new(SearchEngine::new())),
            vector_mirror: Arc::new(crate::store::OnlineVectorMirror::disabled()),
            index_path: root
                .path()
                .join("index.json")
                .to_string_lossy()
                .into_owned(),
            model_path: root.path().join("models").to_string_lossy().into_owned(),
            max_upload_bytes: 64 * 1024 * 1024,
            docs: Arc::new(DocStore::new(root.path().join("docs.json").to_str().unwrap()).unwrap()),
            document_metadata: Arc::new(DocumentMetadataStore::in_memory()),
            files: Arc::new(FileStore::new(root.path().join("uploads").to_str().unwrap()).unwrap()),
            data_sources: None,
            search_apps: None,
            chat_apps: Some(Arc::new(
                crate::api::chatapp_mgr::ChatAppStore::new(
                    root.path().join("chat_apps.json").to_str().unwrap(),
                )
                .unwrap(),
            )),
            providers: Arc::new(ProviderStore::in_memory()),
            tenant_models: Arc::new(TenantModelStore::in_memory()),
            memories: Arc::new(MemoryStore::in_memory()),
            memory_messages: Arc::new(crate::api::joint_services::MemoryMessageService::in_memory()),
            skill_index: Arc::new(crate::api::skill_index::SkillIndexStore::in_memory()),
            system_settings: Arc::new(crate::api::system_settings::SystemSettingsStore::in_memory()),
            langfuse: Arc::new(crate::api::langfuse::LangfuseStore::in_memory()),
            mcp_servers: Arc::new(crate::api::mcp_mgr::McpServerStore::in_memory()),
            chat_channels: Arc::new(crate::api::chat_channel_mgr::ChatChannelStore::in_memory()),
            tasks: Arc::new(TaskQueue::in_memory()),
            task_executor: Arc::new(TaskExecutor::new("test_executor", 4)),
            agents: Arc::new(AgentStore::in_memory()),
            agent_runs: Arc::new(AgentRunRegistry::default()),
            agent_checkpoints: Arc::new(crate::agent_checkpoint::AgentCheckpointStore::in_memory()),
            canvas_versions: Arc::new(CanvasVersionStore::in_memory()),
            compilation_templates: Arc::new(CompilationTemplateStore::in_memory()),
            evaluations: Arc::new(EvaluationStore::in_memory()),
            log_levels: Arc::new(LogLevelManager::in_memory()),
            chunk_feedback_enabled: false,
            chunk_feedback_weighting: crate::chunk_feedback::FeedbackWeighting::Relevance,
            channels: Arc::new(ChannelRegistry::new()),
            document_commit_lock: Arc::new(std::sync::Mutex::new(())),
            memory_commit_lock: Arc::new(std::sync::Mutex::new(())),
        });
        (root, state)
    }

    async fn body_of(resp: impl IntoResponse) -> String {
        let bytes = axum::body::to_bytes(resp.into_response().into_body(), 16 * 1024 * 1024)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn english_request(uri: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .header("cookie", "lang=en")
            .body(Body::empty())
            .unwrap()
    }

    #[test]
    fn inline_json_string_cannot_terminate_script() {
        let encoded = json_script_string("</script>\u{2028}&>");
        assert_eq!(encoded, "\"\\u003c/script\\u003e\\u2028\\u0026\\u003e\"");
        assert!(!encoded.contains("</script>"));
    }

    #[tokio::test]
    async fn memory_list_shell_uses_safe_dom_and_fixed_detail_route() {
        let html = body_of(memories_page_v2(english_request("/memories")).await).await;
        assert!(html.contains("id='memSearch'"));
        assert!(html.contains("id='memCreate'"));
        assert!(html.contains("id='memOwner'"));
        assert!(html.contains("owner_ids"));
        assert!(html.contains("/api/v1/models/default"));
        assert!(html.contains("['chat','image2text']"));
        assert!(html.contains("/memory/memory-message/"));
        assert!(html.contains("document.createElement"));
        assert!(html.contains("textContent"));
        assert!(!html.contains("innerHTML"));
        assert!(!html.contains("Recent Conversations"));
    }

    #[tokio::test]
    async fn memory_message_shell_has_fixed_hierarchy_actions_and_safe_id() {
        let malicious = "</script><script>alert(1)</script>\u{2028}&";
        let html = body_of(
            memory_message_page(
                Path(malicious.into()),
                english_request("/memory/memory-message/x"),
            )
            .await,
        )
        .await;
        for marker in [
            "memoryMessagesLink",
            "memorySettingLink",
            "messageSearch",
            "agentFilter",
            "contentModal",
            "forgetModal",
            "taskModal",
            "copyVector",
            "allowedSizes=[10,20,50,100]",
            "/api/v1/messages/",
            "taskDisplayState",
            "taskStartTime",
            "memory-task-button",
            "memory-task-dot",
            "taskButton.dataset.taskStatus=taskState",
            "status==='failed'||status==='cancelled'||progress<0",
            "[data-task-status=done]",
            "[data-task-status=running]",
            "[data-task-status=failed]",
            "width:32px;height:32px",
            ".memory-message-table tr:hover .memory-actions",
        ] {
            assert!(html.contains(marker), "missing message UI marker {marker}");
        }
        assert!(html.contains("\\u003c/script\\u003e"));
        assert!(!html.contains(malicious));
        assert!(html.contains("Started: '+taskStartTime(task&&task.create_time)"));
        assert!(!html.contains("task.create_time||Date.now()"));
        assert!(!html.contains("innerHTML"));
    }

    #[tokio::test]
    async fn memory_task_log_static_contract_keeps_api_log_as_text() {
        let malicious_log = r#"<img src=x onerror="alert(1)"></script><script>alert(2)</script>"#;
        let html = body_of(
            memory_message_page(
                Path("memory-a".into()),
                english_request("/memory/memory-message/memory-a"),
            )
            .await,
        )
        .await;

        assert!(!html.contains(malicious_log));
        assert!(html.contains(
            "function node(tag,cls,text){const n=document.createElement(tag);if(cls)n.className=cls;if(text!==undefined)n.textContent=String(text)"
        ));
        assert!(html.contains("node('div','memory-modal-content',task&&task.progress_msg||'')"));
        for unsafe_sink in [
            "taskLog.innerHTML",
            "log.innerHTML",
            "insertAdjacentHTML",
            "document.write",
        ] {
            assert!(
                !html.contains(unsafe_sink),
                "task log must not use unsafe DOM sink {unsafe_sink}"
            );
        }
    }

    #[tokio::test]
    async fn memory_setting_shell_has_basic_advanced_and_safe_id() {
        let malicious = "</script><script>alert(1)</script>";
        let html = body_of(
            memory_setting_page(
                Path(malicious.into()),
                english_request("/memory/memory-setting/x"),
            )
            .await,
        )
        .await;
        for marker in [
            "memoryMessagesLink",
            "memorySettingLink",
            "cfgName",
            "cfgAvatar",
            "cfgDescription",
            "cfgEmbd",
            "cfgLlm",
            "default-model-embd_id",
            "default-model-llm_id",
            "const MODEL_FIELDS=",
            "cfgSize",
            "cfgPermissions",
            "cfgPolicy",
            "cfgTemperature",
            "cfgSystemPrompt",
            "cfgUserPrompt",
            "cfgReset",
            "cfgSave",
            "/api/v1/models",
            "max='10485760'",
            "value='me'",
        ] {
            assert!(html.contains(marker), "missing setting UI marker {marker}");
        }
        assert!(html.contains("\\u003c/script\\u003e"));
        assert!(!html.contains(malicious));
        assert!(!html.contains("innerHTML"));
    }

    #[test]
    fn default_model_fields_keep_upstream_order_and_wire_types() {
        assert_eq!(
            DEFAULT_MODEL_FIELDS.map(|field| field.field_id),
            [
                "llm_id",
                "embd_id",
                "img2txt_id",
                "asr_id",
                "rerank_id",
                "tts_id",
            ]
        );
        assert_eq!(
            DEFAULT_MODEL_FIELDS.map(|field| field.update_type),
            ["chat", "embedding", "vision", "asr", "rerank", "tts"]
        );
        assert_eq!(
            DEFAULT_MODEL_FIELDS.map(|field| field.response_type),
            [
                "chat",
                "embedding",
                "image2text",
                "speech2text",
                "rerank",
                "tts",
            ]
        );
        assert!(DEFAULT_MODEL_FIELDS[0].required);
        assert!(
            DEFAULT_MODEL_FIELDS[1..]
                .iter()
                .all(|field| !field.required)
        );
    }

    #[tokio::test]
    async fn providers_page_wires_default_models_and_available_provider_clicks() {
        let (_root, state) = test_state();
        let req = axum::http::Request::builder()
            .uri("/user-setting/model")
            .header("accept-language", "en")
            .body(axum::body::Body::empty())
            .unwrap();
        let body = body_of(providers_page(axum::extract::State(state), req).await).await;
        for marker in [
            "dmLlm",
            "dmEmbedding",
            "dmVlm",
            "dmAsr",
            "dmRerank",
            "dmTts",
            "default-model-llm_id",
            "data-update-type='vision'",
            "data-update-type='asr'",
            "data-response-type='image2text'",
            "data-response-type='speech2text'",
            "The default LLM for each newly created dataset.",
            "fetch('/api/v1/models')",
            "fetch('/api/v1/models/default'",
            "parseDefaultModelValue",
            "model_name:name",
            "aria-haspopup='dialog'",
            "data-state='closed'",
            "default-model-control",
            "default-model-popover",
            "buildDefaultModelTree",
            "filterDefaultModelTree",
            "defaultModelTreeRow",
            "group.setAttribute('role','group')",
            "requestId:0,pending:false",
            "No data available",
            "provider_name",
        ] {
            assert!(
                body.contains(marker),
                "missing default-model marker {marker}"
            );
        }
        assert_eq!(body.matches(" aria-required='true'").count(), 1);
        assert_eq!(body.matches("class='default-model-popover'").count(), 6);
        assert_eq!(body.matches("aria-haspopup='dialog'").count(), 6);
        for field in DEFAULT_MODEL_FIELDS {
            assert!(!body.contains(&format!("id='{}Options'", field.dom_id)));
        }
        assert!(!body.contains("<script><script>"));
        assert!(!body.contains("dmChat"));
        assert!(!body.contains("dmVLM"));
        assert!(!body.contains("dmASR"));
        assert!(!body.contains("/api/v1/llm/models"));

        for marker in [
            "available-models-section",
            "model-providers-search",
            "available-model-card",
            "selectProviderFilter",
            "activeProviderFilter===kind",
            "new Set(kinds)",
            "showProviderInstanceModal(this)",
            "available-provider-add",
            "available-provider-doc",
            "https://platform.openai.com/api-keys",
            "event.stopPropagation()",
            "data-api-base-options='",
            "data-show-base='true'",
            "providerBaseOptions",
            "providerRegionForBaseUrl",
            "region:providerRegionForBaseUrl",
            "option.dataset.region=entry[1]",
        ] {
            assert!(
                body.contains(marker),
                "missing Available models marker {marker}"
            );
        }
        assert!(
            body.contains("data-k="),
            "provider cards must carry capability attributes"
        );
        assert_eq!(
            body.matches("data-testid='available-model-card'").count(),
            63,
            "fixed v0.26.4 exposes 63 Available provider cards"
        );
        assert_eq!(
            body.matches("data-show-base='true'").count(),
            6,
            "OpenAI/Azure-OpenAI/Tongyi-Qianwen/MiniMax/SILICONFLOW/Anthropic show Base url"
        );
        assert!(
            body.contains("https://api.siliconflow.cn/v1")
                && body.contains("https://api.siliconflow.com/v1"),
            "SILICONFLOW card must advertise default and intl base URLs"
        );
        assert!(
            body.contains("https://dashscope.aliyuncs.com/compatible-mode/v1")
                && body.contains("https://dashscope-intl.aliyuncs.com/compatible-model/v1"),
            "Tongyi-Qianwen card must advertise default and intl base URLs"
        );
        for canonical in [
            "Tongyi-Qianwen",
            "OpenAI-API-Compatible",
            "SoMark",
            "New API",
        ] {
            assert!(
                body.contains(&format!("data-provider='{canonical}'")),
                "missing canonical Available provider {canonical}"
            );
        }
        assert!(body.contains("addProvModal"), "missing add-provider modal");
        assert!(body.contains("models:models,enabled:true"));
        assert!(body.contains("?'PUT':'POST'"));
        for marker in [
            "provider-instance-dialog",
            "Instance name",
            "API-Key",
            "Base url",
            "Group ID",
            "verifyProviderInstance",
            "saveProviderInstance",
            "/connection",
            "method:'POST'",
            "Please input the instance name!",
            "Please enter the API key",
            ".provider-instance-field[hidden]",
            "data-model-picker='true'",
            "data-api-key-optional='true'",
            "List models",
            "Add custom model",
            "provider-model-list' role='list'",
            "setAttribute('role','listitem')",
            "if(event.key!=='Escape')return",
            "Model name",
            "Model type",
            "Max tokens",
            "Model features",
            "Tool call",
            "toggleProviderModelPicker",
            "loadProviderModels",
            "providerModelInfo",
            "extra:{is_tools:",
            "addProviderCustomModel",
        ] {
            assert!(
                body.contains(marker),
                "missing provider-instance modal marker {marker}"
            );
        }
        assert_eq!(
            body.matches("data-special-modal='bedrock'").count(),
            1,
            "exactly one Bedrock Available card routes to the dedicated modal"
        );
        assert_eq!(
            body.matches("data-special-modal='somark'").count(),
            1,
            "exactly one SoMark Available card routes to the dedicated modal"
        );
        for marker in [
            "special==='bedrock'",
            "special==='somark'",
            "data-testid='bedrock-dialog'",
            "data-testid='somark-dialog'",
            "AWS Access Key ID",
            "AWS Secret Access Key",
            "AWS Role ARN",
            "AWS region",
            "US East (Ohio)",
            "auth_mode:bedrockAuthMode",
            "key.bedrock_ak=",
            "key.bedrock_sk=",
            "key.aws_role_arn=",
            "key.bedrock_region=",
            "role='radiogroup'",
            "verifyBedrock",
            "saveBedrock",
            "Your API key is valid.",
            "Element Formats",
            "Feature Config",
            "Chemical Structural Formula Format",
            "Enable Text Cross Page",
            "Keep Header Footer",
            "somark_image_format:",
            "somark_enable_table_image",
            "verifySoMark",
            "saveSoMark",
            "llm_factory:'SoMark'",
            "model_type:['ocr']",
            "Verification failed",
        ] {
            assert!(
                body.contains(marker),
                "missing Bedrock/SoMark dedicated modal marker {marker}"
            );
        }
        assert_eq!(
            body.matches("US East (").count(),
            2,
            "the two US East regions render their fixed Bedrock labels"
        );
        for marker in [
            "PROVIDER_SPECIAL_FIELDS",
            "providerSpecialFieldConfig",
            "renderSpecialProviderFields",
            "providerSpecialValidate",
            "providerSpecialPayload",
            "providerSpecialVerifyPayload",
            "Azure-OpenAI",
            "api_version",
            "VOLC ARK_API_KEY",
            "google_service_account_key",
            "TencentCloud_sid",
            "spark_api_password",
            "yiyan_ak",
            "fish_audio_ak",
            "opendataloader_apiserver",
            "paddleocr_api_url",
            "mineru_backend",
            "16k_zh_medical",
            "PaddleOCR-VL-1.6",
            "vlm-lmdeploy-engine",
            "mineru_delete_output",
        ] {
            assert!(
                body.contains(marker),
                "missing provider-specific field registry marker {marker}"
            );
        }
        for marker in [
            "PROVIDER_ICON_DIRECTORY",
            "providerIconHtml",
            "href='/icons.svg#icon-openai'",
            "href='/icons.svg#icon-MiniMax'",
            "src='/assets/svg/llm/gemini.svg'",
            "src='/assets/svg/llm/mineru-bright.svg'",
            "src='/assets/svg/llm/paddleocr.svg'",
            "src='/assets/svg/llm/somark.svg'",
            "/icons.svg#icon-moxing-default'",
            "moxing-default",
        ] {
            assert!(
                body.contains(marker),
                "missing provider icon marker {marker}"
            );
        }
        let auth_bootstrap = body
            .find("var nativeFetch=window.fetch.bind(window)")
            .expect("missing authenticated fetch bootstrap");
        let initial_model_load = body
            .find("loadModelOptions();")
            .expect("missing initial model load");
        assert!(
            auth_bootstrap < initial_model_load,
            "API authentication must be installed before initial model requests"
        );

        for marker in [
            "added-models-section",
            "added-model-card",
            "View models",
            "Hide models",
            "delete-model-instance-dialog",
            "edit-instance-model-dialog",
            "role','switch'",
            "Edit custom model",
            "method:'PATCH'",
            "method:'DELETE'",
            "method:'PUT'",
            "instances:[modal.dataset.instance]",
            "model_name:[modal.dataset.model]",
            "model_type:types",
            "loadAddedModels",
        ] {
            assert!(
                body.contains(marker),
                "missing Added models marker {marker}"
            );
        }
        assert!(body.contains("/api/v1/providers/"));
        assert!(body.contains("/instances"));
        assert!(body.contains("/models"));
    }

    #[tokio::test]
    async fn skills_config_modal_uses_shared_default_model_tree() {
        let (_root, state) = test_state();
        let req = Request::builder()
            .uri("/skills")
            .body(Body::empty())
            .unwrap();
        let html = body_of(skills_page(State(state), req).await).await;
        // 弹层 embedding 行：共享树选择器 markup + MODEL_FIELDS 载荷 + 共享脚本
        assert!(
            html.contains("data-testid='default-model-embd_id'"),
            "tree trigger testid missing"
        );
        assert!(
            html.contains("data-default-model-root='skillEmbd'"),
            "skills selector root missing"
        );
        assert!(
            html.contains("\"skillEmbd\""),
            "MODEL_FIELDS payload missing"
        );
        assert!(
            html.contains("function modelField(type){return MODEL_FIELDS.find"),
            "shared head missing"
        );
        assert!(
            html.contains("function filterDefaultModelTree(nodes,term)"),
            "shared body missing"
        );
        assert!(
            html.contains("normalizeDefaultModelStoredValues"),
            "stored-value normalizer missing"
        );
        assert!(
            html.contains("Select an embedding model") || html.contains("选择嵌入模型"),
            "upstream placeholder missing"
        );
        assert!(
            html.contains("Skill Search Configuration") || html.contains("技能搜索配置"),
            "upstream dialog title missing"
        );
        // 保存校验：未选择嵌入模型时按上游文案阻止（skillSearch.pleaseSelectEmbeddingModel）
        assert!(
            html.contains("SKILL_EMBD_REQUIRED"),
            "required-model message missing"
        );
        assert!(
            html.contains("Please select an embedding model") || html.contains("请选择嵌入模型"),
            "upstream validation message missing"
        );
        // openSkillConfig 写 dataset.value；saveSkillConfig 读 dataset.value
        assert!(
            html.contains("skillTrigger.dataset.value=c.embd_id||''"),
            "openSkillConfig writes tree value"
        );
        assert!(
            html.contains("embd_id:skillVal"),
            "saveSkillConfig reads tree value"
        );
    }

    #[tokio::test]
    async fn skills_page_uses_skill_index_apis_instead_of_agent_installation() {
        let (_root, state) = test_state();
        let req = Request::builder()
            .uri("/skills")
            .header("accept-language", "en")
            .body(Body::empty())
            .unwrap();
        let html = body_of(skills_page(State(state), req).await).await;
        for endpoint in [
            "/api/v1/skills/config",
            "/api/v1/skills/search",
            "/api/v1/skills/index",
            "/api/v1/skills/reindex",
        ] {
            assert!(html.contains(endpoint), "missing Skill endpoint {endpoint}");
        }
        assert!(html.contains("skillSpace"));
        assert!(html.contains("skillContentEnabled"));
        assert!(!html.contains("installSkill("));
        assert!(!html.contains("component_catalog"));
    }

    #[tokio::test]
    async fn kbs_page_renders_create_search_and_card_action_menu() {
        let (_root, state) = test_state();
        state.kbs.create_for("u1", "Test KB", "desc").unwrap();
        let req = Request::builder().uri("/kbs").body(Body::empty()).unwrap();
        let html = body_of(kbs_page(State(state), req).await).await;
        // 创建按钮 + 搜索 + 排序（i18n：zh 默认渲染中文文案）
        assert!(
            html.contains("New KB") || html.contains("新建知识库"),
            "create button missing"
        );
        assert!(html.contains("kbSearch"), "search input missing");
        assert!(html.contains("kbSort"), "sort select missing");
        // 卡片操作菜单：重命名 / 导出 / 删除
        assert!(html.contains("toggleKbMenu"), "card menu trigger missing");
        assert!(html.contains("kbRenameOpen"), "rename menu item missing");
        assert!(html.contains("exportKb"), "export menu item missing");
        assert!(html.contains("delKb"), "delete menu item missing");
        // 创建 / 重命名弹窗与 JS 处理器
        assert!(html.contains("createModal"), "create modal missing");
        assert!(html.contains("renameModal"), "rename modal missing");
        assert!(html.contains("renameKb"), "rename handler missing");
        assert!(
            html.contains("/api/v1/datasets"),
            "datasets API call missing"
        );
    }

    #[tokio::test]
    async fn kb_config_embedding_uses_shared_default_model_tree() {
        let (_root, state) = test_state();
        let kb = state.kbs.create_for("u1", "Tree KB", "d").unwrap();
        let req = Request::builder()
            .uri("/kbs/x")
            .body(Body::empty())
            .unwrap();
        let html =
            body_of(kb_detail_page(State(state.clone()), Path(kb.id.clone()), req).await).await;
        // 配置 tab embedding 行：必填星标 + 上游 tip + 共享树选择器 markup
        // （上游 common-item.tsx EmbeddingSelect，testId ds-settings-basic-embedding-model-select）
        assert!(
            html.contains("data-testid='default-model-embd_id'"),
            "tree trigger testid missing"
        );
        assert!(
            html.contains("data-model-types='embedding'"),
            "accepted model types missing"
        );
        assert!(
            html.contains("aria-required='true'"),
            "required marker missing"
        );
        assert!(
            html.contains("The default embedding model used by the dataset.")
                || html.contains("知识库采用的默认嵌入模型。"),
            "upstream tooltip missing"
        );
        assert!(
            !html.contains("<select id='embdSel'"),
            "legacy select remains"
        );
        // 共享脚本：MODEL_FIELDS 载荷 + HEAD/BODY + 表单持久化钩子 + catalog 加载
        assert!(html.contains("\"embdSel\""), "MODEL_FIELDS payload missing");
        assert!(
            html.contains("function modelField(type){return MODEL_FIELDS.find"),
            "shared head missing"
        );
        assert!(
            html.contains("function filterDefaultModelTree(nodes,term)"),
            "shared body missing"
        );
        assert!(
            html.contains("(type,nextValue){var field=modelField(type)"),
            "form persist hook missing"
        );
        assert!(
            html.contains("loadKbModelCatalog"),
            "catalog loader missing"
        );
        assert!(
            html.contains("normalizeDefaultModelStoredValues"),
            "stored-value normalizer missing"
        );
        assert!(
            html.contains("Please select a embedding model.") || html.contains("请选择嵌入模型"),
            "upstream placeholder missing"
        );
        assert!(
            html.contains("document.addEventListener('pointerdown'"),
            "outside-close listener missing"
        );
        assert!(
            html.contains("tree.onkeydown=function(event)"),
            "tree keyboard navigation missing"
        );
        assert!(
            html.contains("scrollIntoView({block:'nearest'})"),
            "row scroll-into-view missing"
        );
        // saveConfig 读 dataset.value；loadConfig 写 dataset.value
        assert!(
            html.contains("(document.getElementById('embdSel').dataset.value||'default')"),
            "saveConfig reads tree value"
        );
        assert!(
            html.contains("em.dataset.value=kb.embd_id"),
            "loadConfig writes tree value"
        );
        // dataset 页同构接线
        let req2 = Request::builder()
            .uri("/datasets/x")
            .body(Body::empty())
            .unwrap();
        let html2 =
            body_of(dataset_page(State(state.clone()), Path(kb.id.clone()), req2).await).await;
        assert!(
            html2.contains("data-testid='default-model-embd_id'"),
            "dataset page tree missing"
        );
        assert!(
            html2.contains("loadKbModelCatalog"),
            "dataset page script missing"
        );
    }

    #[tokio::test]
    async fn kb_detail_page_renders_document_batch_config_and_testing_buttons() {
        let (_root, state) = test_state();
        let kb = state.kbs.create_for("u1", "Detail KB", "d").unwrap();
        let req = Request::builder()
            .uri("/kbs/x")
            .body(Body::empty())
            .unwrap();
        let html =
            body_of(kb_detail_page(State(state.clone()), Path(kb.id.clone()), req).await).await;
        // 文档列表页：上传 / 批量操作（解析/禁用/启用/删除）/ 状态列进度条 / 块数
        assert!(html.contains("uploadDoc"), "upload button missing");
        assert!(html.contains("batchParse"), "batch parse missing");
        assert!(html.contains("batchDisable"), "batch disable missing");
        assert!(html.contains("batchDelete"), "batch delete missing");
        assert!(html.contains("statusCell"), "status column missing");
        assert!(html.contains("progress-bar"), "progress bar missing");
        assert!(html.contains("chunk_count"), "chunk count missing");
        assert!(html.contains("docFilter"), "doc filter missing");
        // 配置页 tab：名称 / 权限 / 解析配置 / embedding / 检索参数 / 保存
        assert!(html.contains("cfgName"), "config name field missing");
        assert!(html.contains("cfgPerm"), "permission select missing");
        assert!(html.contains("cfgMethod"), "chunk method select missing");
        assert!(html.contains("embdSel"), "embedding select missing");
        assert!(html.contains("cfgTopK"), "retrieval top k missing");
        assert!(html.contains("cfgLangs"), "cross-language selector missing");
        assert!(
            html.contains("cross_languages"),
            "prompt_config save wiring missing"
        );
        assert!(html.contains("cfgThreshold"), "similarity slider missing");
        assert!(html.contains("cfgVector"), "vector weight slider missing");
        assert!(html.contains("saveConfig"), "save button missing");
        // 检索测试 tab：输入框 / 检索按钮 / 相似度滑块 / 结果卡片
        assert!(html.contains("id='tq'"), "test query input missing");
        assert!(html.contains("runTest"), "test button missing");
        // Reranking tree selector (upstream testing-form.tsx RerankFormFields)
        assert!(
            html.contains("data-testid='default-model-reranking_id'"),
            "test reranking tree trigger missing"
        );
        assert!(
            html.contains("data-model-types='rerank'"),
            "reranking accepted types missing"
        );
        assert!(
            !html.contains("<input type='checkbox' id='tr'>"),
            "legacy reranking checkbox remains"
        );
        assert!(
            html.contains("body.rerank_id="),
            "runTest must send rerank_id"
        );
        assert!(html.contains("id='tt'"), "similarity slider missing");
        assert!(html.contains("testRes"), "results area missing");
        assert!(html.contains("/api/v1/retrieval"), "retrieval API missing");
    }

    #[tokio::test]
    async fn search_page_seeds_search_app_runtime_config() {
        let (_root, mut state) = test_state();
        let dir =
            std::env::temp_dir().join(format!("rayrag-sapp-runtime-{}", uuid::Uuid::new_v4()));
        let store = std::sync::Arc::new(
            crate::api::searchapp_mgr::SearchAppStore::new(dir.to_str().unwrap()).unwrap(),
        );
        let app = store
            .create(
                "u1",
                crate::api::searchapp_mgr::SearchAppCreate {
                    name: "运行时检索".into(),
                    kb_ids: vec!["kb-seeded".into()],
                    hybrid: true,
                    rerank: true,
                    rerank_id: "mxbai-rerank-large-v2@GPU mxbai-rerank@mxbai-rerank-large-v2"
                        .into(),
                    top_k: 8,
                    similarity_threshold: 0.6,
                    vector_similarity_weight: 0.4,
                },
            )
            .unwrap();
        std::sync::Arc::get_mut(&mut state).unwrap().search_apps = Some(store);
        let req = Request::builder()
            .uri(format!("/search?sapp={}", app.id).as_str())
            .body(Body::empty())
            .unwrap();
        let html = body_of(search_page(State(state.clone()), req).await).await;
        // Upstream next-search hooks.ts: the search app runtime consumes the
        // app's config (kb_ids/rerank/rerank_id/top_k) for the retrieval call.
        assert!(html.contains("window.__SAPP__="), "sapp seed missing");
        assert!(
            html.contains("mxbai-rerank-large-v2@GPU mxbai-rerank@mxbai-rerank-large-v2"),
            "app rerank_id seed missing"
        );
        assert!(html.contains("applySapp"), "sapp apply wiring missing");
        assert!(
            html.contains("\"similarity_threshold\":0.6"),
            "app threshold seed missing"
        );
        assert!(
            html.contains("\"vector_similarity_weight\":0.4"),
            "app weight seed missing"
        );
        assert!(
            html.contains("body.similarity_threshold=t;body.vector_similarity_weight=parseFloat"),
            "unconditional threshold wiring missing"
        );
        assert!(
            html.contains("body.rerank_id="),
            "runSearch must send rerank_id"
        );
        assert!(
            html.contains("if(!h){body.vector_similarity_weight=0}"),
            "hybrid toggle wiring missing"
        );
        assert!(
            html.contains("data-testid='default-model-rerank_id'"),
            "search page rerank tree trigger missing"
        );
        assert!(
            html.contains("id='searchRerankRow'"),
            "search page rerank row missing"
        );
        assert!(
            html.contains("searchRerankToggle"),
            "search page rerank toggle missing"
        );
        assert!(
            html.contains("searchRerankSel"),
            "runSearch must read searchRerankSel"
        );
        // Upstream `/search` keeps the drawer in react-hook-form state: picking a
        // rerank model must not PATCH the tenant default (that belongs to the
        // search-app detail page, which persists `search_config.rerank_id`).
        assert!(
            html.contains(FORM_LOCAL_SAVE_MODEL_DEFAULT),
            "search page rerank picker must override the tenant-default persist hook"
        );
        let plain = Request::builder()
            .uri("/search")
            .body(Body::empty())
            .unwrap();
        let plain_html = body_of(search_page(State(state.clone()), plain).await).await;
        assert!(
            !plain_html.contains("window.__SAPP__="),
            "sapp seed leaked without query param"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn search_app_detail_page_renders_rerank_tree_selector() {
        let (_root, mut state) = test_state();
        let dir = std::env::temp_dir().join(format!("rayrag-sapp-test-{}", uuid::Uuid::new_v4()));
        let store = std::sync::Arc::new(
            crate::api::searchapp_mgr::SearchAppStore::new(dir.to_str().unwrap()).unwrap(),
        );
        let app = store
            .create(
                "u1",
                crate::api::searchapp_mgr::SearchAppCreate {
                    name: "检索应用".into(),
                    kb_ids: vec![],
                    hybrid: false,
                    rerank: false,
                    rerank_id: "prov/inst/bge-reranker-v2-m3".into(),
                    top_k: 8,
                    similarity_threshold: 0.2,
                    vector_similarity_weight: 0.3,
                },
            )
            .unwrap();
        std::sync::Arc::get_mut(&mut state).unwrap().search_apps = Some(store);
        let req = Request::builder()
            .uri("/searches/x")
            .body(Body::empty())
            .unwrap();
        let html =
            body_of(search_app_detail_page(State(state.clone()), Path(app.id.clone()), req).await)
                .await;
        // Upstream next-search/search-setting.tsx: rerank switch + rerank_id
        // ModelTreeSelect (rerank), revealed only when use_rerank is on.
        assert!(
            html.contains("data-testid='default-model-rerank_id'"),
            "rerank tree trigger missing"
        );
        assert!(
            html.contains("data-model-types='rerank'"),
            "rerank accepted types missing"
        );
        assert!(html.contains("id='saRerankRow'"), "conditional row missing");
        assert!(html.contains("saRerankToggle"), "row toggle missing");
        assert!(html.contains("rerank_id:"), "saveSApp must send rerank_id");
        assert!(html.contains("seedModelTrigger"), "seed wiring missing");
        assert!(html.contains("id='sa_thresh'"), "threshold input missing");
        assert!(html.contains("id='sa_vsw'"), "vector weight input missing");
        assert!(
            html.contains("Rerank Model") || html.contains("重排模型"),
            "rerank model switch label missing"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn dataset_page_renders_five_tabs_with_all_required_buttons() {
        let (_root, state) = test_state();
        let kb = state.kbs.create_for("u1", "Five Tab KB", "d").unwrap();
        let req = Request::builder()
            .uri("/dataset/x")
            .body(Body::empty())
            .unwrap();
        let html =
            body_of(dataset_page(State(state.clone()), Path(kb.id.clone()), req).await).await;
        // 5 个 tab
        for nav in ["nav0", "nav1", "nav2", "nav3", "nav4"] {
            assert!(html.contains(nav), "nav {} missing", nav);
        }
        for tab in ["tab0", "tab1", "tab2", "tab3", "tab4"] {
            assert!(html.contains(&format!("id='{}'", tab)), "{} missing", tab);
        }
        // 文档 tab：上传 / 批量操作 / 状态进度 / 块数
        assert!(html.contains("uploadDoc"), "upload missing");
        assert!(html.contains("batchParse"), "batch parse missing");
        assert!(html.contains("batchDisable"), "batch disable missing");
        assert!(html.contains("batchDelete"), "batch delete missing");
        assert!(html.contains("statusCell"), "status cell missing");
        assert!(html.contains("progress-bar"), "progress bar missing");
        // 概览 tab
        assert!(html.contains("ovDone"), "overview stats missing");
        assert!(html.contains("actList"), "recent activity missing");
        // 测试 tab：输入 / 检索按钮 / 相似度滑块 / 结果
        assert!(html.contains("id='testQ'"), "test query missing");
        // Reranking tree selector (upstream testing-form.tsx RerankFormFields)
        assert!(
            html.contains("data-testid='default-model-reranking_id'"),
            "test reranking tree trigger missing"
        );
        assert!(
            html.contains("data-model-types='rerank'"),
            "reranking accepted types missing"
        );
        assert!(
            !html.contains("<input type='checkbox' id='testRerank'>"),
            "legacy reranking checkbox remains"
        );
        assert!(
            html.contains("body.rerank_id="),
            "testSearch must send rerank_id"
        );
        assert!(html.contains("testSearch"), "test button missing");
        assert!(html.contains("id='testT'"), "threshold slider missing");
        assert!(html.contains("id='testW'"), "vector slider missing");
        assert!(html.contains("testResults"), "results missing");
        // 配置 tab：名称 / 权限 / 解析 / embedding / 检索参数 / 保存
        assert!(html.contains("cfgName"), "config name missing");
        assert!(html.contains("cfgPerm"), "config permission missing");
        assert!(html.contains("chunkMethod"), "chunk method missing");
        assert!(html.contains("embdSel"), "embedding missing");
        assert!(html.contains("cfgTopK"), "top k missing");
        assert!(html.contains("cfgThreshold"), "threshold missing");
        assert!(html.contains("cfgVector"), "vector weight missing");
        assert!(html.contains("saveConfig"), "save missing");
        // 知识图谱 tab
        assert!(html.contains("buildGraph"), "graph build missing");
        assert!(html.contains("graphResult"), "graph result missing");
        // API 调用
        assert!(html.contains("/api/v1/retrieval"), "retrieval API missing");
        assert!(html.contains("/api/v1/providers"), "providers API missing");
    }

    #[tokio::test]
    async fn files_page_renders_link_bulk_and_selection_buttons() {
        let (_root, state) = test_state();
        let req = Request::builder()
            .uri("/files")
            .body(Body::empty())
            .unwrap();
        let html = body_of(files_page(State(state), req).await).await;
        // 批量操作栏 + 勾选 + 关联知识库（RAGFlow: 上传/新建文件夹/批量移动/批量关联）
        assert!(html.contains("fileBatchBar"), "bulk bar missing");
        assert!(html.contains("batchDelSel"), "bulk delete missing");
        assert!(html.contains("batchLinkSel"), "bulk link missing");
        assert!(html.contains("fileSelAll"), "select-all missing");
        assert!(html.contains("linkFile"), "link-to-KB handler missing");
        assert!(
            html.contains("/api/v1/file2document"),
            "file2document API missing"
        );
    }

    #[tokio::test]
    async fn agents_page_renders_import_json_button() {
        let (_root, state) = test_state();
        let req = Request::builder()
            .uri("/agents")
            .body(Body::empty())
            .unwrap();
        let html = body_of(agents_page(State(state), req).await).await;
        // RAGFlow: 从空白创建 / 从模板创建 / 导入 JSON
        assert!(
            html.contains("agentJsonFile"),
            "import JSON file input missing"
        );
        assert!(
            html.contains("importAgentJson"),
            "import JSON handler missing"
        );
        assert!(
            html.contains("Choose agent type"),
            "agent type title missing"
        );
        assert!(
            html.contains("name='agentType'"),
            "agent type radios missing"
        );
        assert!(
            html.contains("value='agent_canvas'"),
            "agent wire category missing"
        );
        assert!(
            html.contains("value='dataflow_canvas'"),
            "dataflow wire category missing"
        );
    }

    #[tokio::test]
    async fn agent_detail_page_renders_versions_and_export_json_buttons() {
        let (_root, state) = test_state();
        let req = Request::builder()
            .uri("/agents/a1")
            .body(Body::empty())
            .unwrap();
        let html = body_of(agent_detail_page(Path("a1".to_string()), req).await).await;
        // RAGFlow: 运行 / 探索 / 版本管理(发布生产+下载) / 导出 JSON
        assert!(html.contains("openVersions"), "versions button missing");
        assert!(html.contains("verModal"), "versions modal missing");
        assert!(
            html.contains("downloadVersion"),
            "version download handler missing"
        );
        assert!(
            html.contains("exportAgentJson"),
            "export JSON handler missing"
        );
        assert!(
            html.contains("/api/v1/agents/'+aid+'/versions"),
            "versions API missing"
        );
        // RAGFlow canvas toolbar parity (agent/editor.tsx click-record)
        assert!(html.contains("agent-launch"), "Launch button missing");
        assert!(
            html.contains("agent-management"),
            "Management button missing"
        );
        assert!(html.contains("autosaveSt"), "Autosaved status missing");
        assert!(html.contains("zoomCanvas"), "zoom controls missing");
        assert!(html.contains("toggleInter"), "toggle interactivity missing");
        assert!(html.contains("pubModal"), "confirm publish modal missing");
        assert!(html.contains("chatTestModal"), "chat test modal missing");
        assert!(
            html.contains("confirmPublish"),
            "confirm publish handler missing"
        );
    }

    #[tokio::test]
    async fn kb_detail_page_renders_empty_all_button() {
        let (_root, state) = test_state();
        let kb = state.kbs.create_for("u1", "E KB", "d").unwrap();
        let req = Request::builder()
            .uri("/kbs/x")
            .body(Body::empty())
            .unwrap();
        let html =
            body_of(kb_detail_page(State(state.clone()), Path(kb.id.clone()), req).await).await;
        // RAGFlow 添加文件下拉：上传文件 / 清空
        assert!(html.contains("emptyAllDocs"), "empty-all button missing");
    }

    #[tokio::test]
    async fn chat_detail_page_renders_temporary_conversation() {
        let (_root, state) = test_state();
        let req = Request::builder()
            .uri("/chat/c1")
            .body(Body::empty())
            .unwrap();
        let html = body_of(chat_detail_page(State(state), Path("c1".to_string()), req).await).await;
        // RAGFlow: 临时对话（不落库）+ 嵌入 / 分享
        assert!(
            html.contains("tempConv"),
            "temp conversation button missing"
        );
        assert!(html.contains("tempModal"), "temp modal missing");
        assert!(html.contains("sendTemp"), "temp send handler missing");
        assert!(
            html.contains("/api/v1/completions"),
            "stateless completion API missing"
        );
        // RAGFlow message-input testids
        assert!(
            html.contains("chat-textarea") && html.contains("chat-detail-send"),
            "message input testids missing"
        );
        assert!(
            html.contains("chat-stream-status") && html.contains("chat-detail-stop"),
            "stream status / stop testids missing"
        );
    }

    #[tokio::test]
    async fn chat_app_detail_page_renders_full_settings_drawer_with_meta_filter() {
        let (_root, state) = test_state();
        let app_id = "a1".to_string();
        // seed a chat app via the store (insert a minimal ChatApp)
        if let Some(store) = &state.chat_apps {
            let app: crate::api::chatapp_mgr::ChatApp = serde_json::from_value(serde_json::json!({
                "id": "a1",
                "name": "设置抽屉",
                "owner_id": "t1",
                "kb_ids": [],
                "created_at": 1,
            }))
            .unwrap();
            let _ = store.insert(app);
        }
        let req = Request::builder()
            .uri("/chats/a1")
            .body(Body::empty())
            .unwrap();
        let html = body_of(chat_app_detail_page(State(state), Path(app_id), req).await).await;
        // settings drawer shell + prompt engine fields incl. meta_data_filter
        assert!(html.contains("settingsDrawer"), "drawer missing");
        assert!(html.contains("f_rerank"), "rerank field missing");
        assert!(
            html.contains("sw_refmeta"),
            "reference metadata switch missing"
        );
        assert!(
            html.contains("f_mfmethod"),
            "meta filter method select missing"
        );
        assert!(
            html.contains("f_mfmanual"),
            "meta filter manual fields missing"
        );
        assert!(
            html.contains("sw_refine_multiturn"),
            "refine multiturn missing"
        );
        assert!(html.contains("sw_reasoning"), "reasoning switch missing");
        assert!(html.contains("f_vsw"), "vector weight missing");
        assert!(
            html.contains("default-model-llm_id"),
            "shared llm selector missing"
        );
        assert!(
            html.contains("loadKbModelCatalog"),
            "selector catalog loader missing"
        );
        assert!(
            html.contains("normalizeDefaultModelStoredValues"),
            "selector normalizer missing"
        );
        assert!(
            html.contains("getElementById('dmLlm').dataset.value"),
            "llm save reads selector value missing"
        );
    }

    #[tokio::test]
    async fn document_page_renders_export_chunks_button() {
        let (_root, state) = test_state();
        let req = Request::builder()
            .uri("/documents/k1/d1")
            .body(Body::empty())
            .unwrap();
        let html = body_of(
            document_page(
                State(state),
                Path(("k1".to_string(), "d1".to_string())),
                req,
            )
            .await,
        )
        .await;
        // RAGFlow chunk 页：保存 / 导出
        assert!(
            html.contains("exportChunks"),
            "export chunks handler missing"
        );
    }

    #[tokio::test]
    async fn admin_page_loads_system_variables_through_the_protected_api() {
        let (_root, state) = test_state();
        let req = Request::builder()
            .uri("/admin")
            .body(Body::empty())
            .unwrap();
        let html = body_of(admin_page(State(state), req).await).await;
        assert!(html.contains("/api/v1/system/variables"));
        assert!(html.contains("Loading administrator settings"));
        assert!(!html.contains("sandbox.self_managed"));
        assert!(!html.contains("http://sandbox-executor-manager:9385"));
    }

    #[tokio::test]
    async fn temp_dump_rendered_html() {
        let (_root, state) = test_state();
        let kb = state.kbs.create_for("u1", "Dump KB", "d").unwrap();
        let req = || Request::builder().uri("/x").body(Body::empty()).unwrap();
        let files = body_of(files_page(State(state.clone()), req()).await).await;
        let agents = body_of(agents_page(State(state.clone()), req()).await).await;
        let ad = body_of(agent_detail_page(Path("a1".to_string()), req()).await).await;
        let kbd =
            body_of(kb_detail_page(State(state.clone()), Path(kb.id.clone()), req()).await).await;
        let cd =
            body_of(chat_detail_page(State(state.clone()), Path("c1".to_string()), req()).await)
                .await;
        let doc = body_of(
            document_page(
                State(state.clone()),
                Path(("k1".to_string(), "d1".to_string())),
                req(),
            )
            .await,
        )
        .await;
        let set = body_of(settings_page(State(test_state().1), req()).await).await;
        std::fs::write("/tmp/dump_files.html", &files).unwrap();
        std::fs::write("/tmp/dump_agents.html", &agents).unwrap();
        std::fs::write("/tmp/dump_agent_detail.html", &ad).unwrap();
        std::fs::write("/tmp/dump_kb_detail.html", &kbd).unwrap();
        std::fs::write("/tmp/dump_chat_detail.html", &cd).unwrap();
        std::fs::write("/tmp/dump_doc.html", &doc).unwrap();
        std::fs::write("/tmp/dump_settings.html", &set).unwrap();
        let dash = body_of(crate::web::dashboard(State(state.clone()), req()).await).await;
        std::fs::write("/tmp/dump_dashboard.html", &dash).unwrap();
    }

    #[tokio::test]
    async fn settings_page_renders_invite_button() {
        let req = Request::builder()
            .uri("/settings")
            .body(Body::empty())
            .unwrap();
        let html = body_of(settings_page(State(test_state().1), req).await).await;
        // RAGFlow user-setting 团队页：邀请
        assert!(html.contains("inviteUser"), "invite button missing");
        assert!(html.contains("/api/v1/users/invite"), "invite API missing");
    }

    #[tokio::test]
    async fn home_page_renders_welcome_dataset_and_application_sections() {
        // Upstream pages/home/index.tsx: banner + Datasets + Applications with
        // the Chat/Search/Agent/Memory Segmented control and per-type empty
        // cards.
        let request = Request::builder()
            .uri("/")
            .header("Cookie", "lang=en")
            .body(Body::empty())
            .unwrap();
        let html = body_of(index(request).await).await;
        let html = html.as_str();
        assert!(
            html.contains("data-testid='home-page'"),
            "home shell missing"
        );
        assert!(html.contains("class='home-banner'"));
        assert!(
            html.contains("Welcome to") && html.contains("class='home-brand'>RayRAG"),
            "welcome banner must lead with the upstream welcome label"
        );
        assert!(html.contains("data-testid='home-datasets'"));
        assert!(html.contains("data-testid='home-apps'"));
        for (key, label) in [
            ("chats", ">Chat</button>"),
            ("searches", ">Search</button>"),
            ("agents", ">Agent</button>"),
            ("memories", ">Memory</button>"),
        ] {
            assert!(
                html.contains(&format!("data-testid='home-tab-{key}'")),
                "segmented option {key} missing"
            );
            assert!(html.contains(label), "segmented label {label} missing");
        }
        assert!(html.contains("role='tablist'") && html.contains("aria-selected='true'"));
        assert!(html.contains("No dataset created yet"));
        assert!(html.contains("No chat app created yet"));
        assert!(html.contains("No search app created yet"));
        assert!(html.contains("No agent app created yet"));
        assert!(html.contains("No memory created yet"));
        assert!(html.contains("homeSelectApp") && html.contains("loadHomeDatasets"));
        assert!(html.contains("/api/v1/searchapps") && html.contains("/api/v1/memories"));
        assert!(html.contains("See all"));
        let zh_request = Request::builder()
            .uri("/")
            .header("Cookie", "lang=zh-CN")
            .body(Body::empty())
            .unwrap();
        let zh_html = body_of(index(zh_request).await).await;
        for label in [
            "欢迎来到",
            "知识库",
            "聊天",
            "搜索",
            "智能体",
            "记忆",
            "查看全部",
        ] {
            assert!(zh_html.contains(label), "zh home label {label} missing");
        }
    }

    #[test]
    fn global_header_matches_upstream_navbar_and_right_cluster() {
        // Upstream layouts/components/header.tsx + global-navbar.tsx: the pill
        // carries Home/Dataset/Chat/Search/Agent/Memory/File in that order, and
        // the right cluster is Discord, GitHub, the language menu, help, theme
        // and the avatar link into `/user-setting`.
        let html = layout("Home", "<p>body</p>", false);
        let nav_start = html.find("<nav class='gnav'").expect("global navbar");
        let nav_end = html.find("</nav>").expect("navbar close");
        let nav = &html[nav_start..nav_end];
        let mut cursor = 0;
        for (href, marker) in [
            ("/", "aria-label='Home'"),
            ("/datasets", "data-nav='/datasets'"),
            ("/chats", "data-testid='nav-chat'"),
            ("/searches", "data-testid='nav-search'"),
            ("/agents", "data-testid='nav-agent'"),
            ("/memories", "data-nav='/memories'"),
            ("/files", "data-nav='/files'"),
        ] {
            let needle = format!("href='{href}'");
            let offset = nav[cursor..]
                .find(&needle)
                .unwrap_or_else(|| panic!("navbar entry {href} missing"));
            cursor += offset + 1;
            let entry_start = nav[..cursor].rfind("<a ").expect("entry start");
            let entry_end = nav[cursor..]
                .find("</a>")
                .map(|end| cursor + end)
                .expect("entry end");
            let entry = &nav[entry_start..entry_end];
            assert!(
                entry.contains(marker),
                "navbar entry {href} lost its upstream marker {marker}: {entry}"
            );
        }
        // The seven upstream entries only: dashboard/data-source/skills/model
        // provider shortcuts moved behind the avatar.
        for legacy in [
            "href='/dashboard'",
            "href='/kbs'",
            "href='/datasources'",
            "href='/skills'",
            "href='/user-setting/model'",
        ] {
            assert!(
                !nav.contains(legacy),
                "navbar still carries the non-upstream entry {legacy}"
            );
        }
        let right = &html[nav_end..html.find("</header>").expect("header close")];
        assert!(right.contains("href='https://discord.com/invite/NjYzJD3GM3'"));
        assert!(right.contains("href='https://github.com/infiniflow/ragflow'"));
        assert!(right.contains("href='https://ragflow.io/docs/dev/category/user-guides'"));
        assert_eq!(
            right
                .matches("target='_blank' rel='noreferrer noopener'")
                .count(),
            3,
            "Discord/GitHub/help must each open in a new tab"
        );
        assert!(right.contains("id='hdrLangMenu'"), "language menu missing");
        assert!(right.contains("aria-haspopup='menu'"));
        assert!(right.contains("onclick='toggleHeaderMenu(\"hdrLangMenu\",event)'"));
        assert!(right.contains("role='menuitemradio'"));
        assert!(right.contains("aria-checked='true'") && right.contains("aria-checked='false'"));
        assert!(right.contains("onclick='toggleTheme()'"));
        assert!(
            right.contains("class='av' id='hdrAvatar' href='/user-setting'")
                && right.contains("data-testid='settings-entrypoint'"),
            "avatar must link into /user-setting"
        );
        // Active pill marking follows upstream's PathMap on the client.
        assert!(html.contains("link.setAttribute('aria-current','page')"));
        assert!(html.contains("'/memories':['/memories','/memory']"));
    }

    #[test]
    fn global_header_localizes_navbar_labels_like_upstream() {
        let zh = layout("首页", "<p>body</p>", true);
        for label in [
            "首页",
            "知识库",
            "聊天",
            "搜索",
            "智能体",
            "记忆",
            "文件管理",
        ] {
            assert!(zh.contains(label), "zh navbar label {label} missing");
        }
        assert!(zh.contains("简体中文") && zh.contains("English"));
        assert!(zh.contains("aria-checked='true'"));
        let en = layout("Home", "<p>body</p>", false);
        for label in [
            "Home", "Dataset", "Chat", "Search", "Agent", "Memory", "File",
        ] {
            assert!(en.contains(label), "en navbar label {label} missing");
        }
    }

    #[test]
    fn global_header_external_links_follow_environment_overrides() {
        // Safety: the test process has no other thread reading these variables.
        unsafe {
            std::env::set_var("RAYRAG_HELP_URL", "https://docs.example.cn/user-guides");
            std::env::set_var("RAYRAG_GITHUB_URL", "https://git.example.cn/rayrag");
        }
        let html = layout("Home", "<p>body</p>", false);
        assert!(html.contains("href='https://docs.example.cn/user-guides'"));
        assert!(html.contains("href='https://git.example.cn/rayrag'"));
        unsafe {
            std::env::remove_var("RAYRAG_HELP_URL");
            std::env::remove_var("RAYRAG_GITHUB_URL");
        }
        let restored = layout("Home", "<p>body</p>", false);
        assert!(restored.contains("href='https://ragflow.io/docs/dev/category/user-guides'"));
        assert!(restored.contains("href='https://github.com/infiniflow/ragflow'"));
    }

    #[test]
    fn user_setting_shell_has_fixed_menu_order_active_state_and_footer_actions() {
        let html = user_setting_layout(
            "Model providers",
            "<p>body</p>",
            UserSettingSection::Model,
            false,
        );
        assert!(html.contains("class='hdr'"), "global header must remain");
        assert!(html.contains("data-testid='settings-entrypoint'"));
        assert!(html.contains("data-testid='user-setting-layout'"));
        let mut cursor = 0;
        for key in [
            "/data-source",
            "/chat-channel",
            "/model",
            "/mcp",
            "/team",
            "/profile",
            "/api",
        ] {
            let offset = html[cursor..]
                .find(&format!("data-setting-key='{key}'"))
                .unwrap_or_else(|| panic!("missing settings item {key}"));
            cursor += offset + 1;
        }
        assert!(html.contains(
            "class='user-setting-nav-item active' href='/user-setting/model' data-setting-key='/model'"
        ));
        assert_eq!(html.matches(" aria-current='page'").count(), 1);
        assert!(html.contains(&format!(
            "class='user-setting-version'>{}",
            crate::build_info::version_line()
        )));
        assert!(html.contains("onclick='toggleTheme()'"));
        assert!(html.contains("onclick='logoutUser()'"));
        assert!(html.contains("result.code===401"));
    }

    #[test]
    fn provider_icons_resolve_svg_font_and_fallback() {
        assert_eq!(provider_icon("OpenAI"), ProviderIcon::Font("openai"));
        assert_eq!(provider_icon("MiniMax"), ProviderIcon::Font("MiniMax"));
        assert_eq!(
            provider_icon("Tencent Cloud"),
            ProviderIcon::Font("tencent-cloud")
        );
        assert_eq!(provider_icon("Gemini"), ProviderIcon::Svg("gemini.svg"));
        assert_eq!(
            provider_icon("MinerU"),
            ProviderIcon::Svg("mineru-bright.svg")
        );
        assert_eq!(
            provider_icon("PaddleOCR"),
            ProviderIcon::Svg("paddleocr.svg")
        );
        assert_eq!(
            provider_icon("Unknown Factory"),
            ProviderIcon::Font("moxing-default")
        );
    }

    #[tokio::test]
    async fn user_setting_api_page_wires_langfuse_card_dialog_and_persistence() {
        let html = body_of(user_setting_api_page(english_request("/user-setting/api")).await).await;
        for marker in [
            "data-testid='langfuse-card'",
            "data-testid='langfuse-config-dialog'",
            "data-testid='langfuse-delete-dialog'",
            "Configuration Langfuse",
            "Secret key",
            "Public key",
            "Please enter the secret key",
            "Please enter the public key",
            "https://cloud.langfuse.com",
            "View Langfuse&#39;s documentation",
            "/api/v1/langfuse/api-key",
            "method:'POST'",
            "method:'DELETE'",
            "langfuseViewUrl",
            "cloud.langfuse.com/project/",
            "loadLangfuse()",
            "Traces, evals, prompt management",
        ] {
            assert!(
                html.contains(marker),
                "missing Langfuse API-page marker {marker}"
            );
        }
        assert!(
            html.contains("Generate") && html.contains("/api/v1/system/api_keys"),
            "the personal API KEY card remains beside the Langfuse card"
        );
    }

    #[tokio::test]
    async fn user_setting_mcp_page_wires_server_crud_bulk_import_and_tools() {
        let html = body_of(user_setting_mcp_page(english_request("/user-setting/mcp")).await).await;
        for marker in [
            "data-testid='mcp-servers-card'",
            "MCP servers",
            "Customize the list of MCP servers",
            "Bulk manage",
            "Import",
            "+ Add MCP",
            "data-testid='mcp-edit-dialog'",
            "data-testid='mcp-import-dialog'",
            "data-testid='mcp-delete-dialog'",
            "Server type",
            "streamable-http",
            "Authorization Token",
            "tools available",
            "testMcpServer",
            "saveMcpServer",
            "importMcpServers",
            "exportMcpServer",
            "deleteMcpServer",
            "toggleMcpBulk",
            "/api/v1/mcp/servers",
            "/api/v1/mcp/servers/import",
            "mode=download",
            "No MCP servers available",
            "+ Add Now",
            "It must be 1-64 characters long",
        ] {
            assert!(html.contains(marker), "missing MCP page marker {marker}");
        }
    }

    #[tokio::test]
    async fn user_setting_team_page_wires_members_teams_invite_and_confirm_flows() {
        let html =
            body_of(user_setting_team_page(english_request("/user-setting/team")).await).await;
        for marker in [
            "data-testid='team-members-card'",
            "data-testid='joined-teams-card'",
            "data-testid='team-invite-dialog'",
            "data-testid='team-confirm-dialog'",
            "Team members",
            "Joined teams",
            "Invite member",
            "Only registered users can be invited",
            "Accept",
            "Decline",
            "Quit",
            "/api/v1/tenants/'+encodeURIComponent(me)+'/users'",
            "user_id:teamState.selfId",
            "Are you sure to remove this member?",
            "Are you sure you want to quit the team you joined?",
            "toggleMemberSort",
            "team-role-owner",
        ] {
            assert!(html.contains(marker), "missing Team page marker {marker}");
        }
    }

    #[tokio::test]
    async fn user_setting_profile_page_wires_edit_dialogs_and_avatar_upload() {
        let html =
            body_of(user_setting_profile_page(english_request("/user-setting/profile")).await)
                .await;
        for marker in [
            "data-testid='profile-card'",
            "data-testid='profile-edit-dialog'",
            "Username",
            "Avatar",
            "Time Zone",
            "Edit Name",
            "Edit Time Zone",
            "Edit Password",
            "Current Password",
            "Confirm Password",
            "Intl.supportedValuesOf('timeZone')",
            "Select a timeZone",
            "profileAvatarFile",
            "uploadAvatar",
            "saveProfileEdit",
            "/api/v1/users/me",
            "new_password",
            "The two passwords do not match",
            "reader.readAsDataURL",
        ] {
            assert!(
                html.contains(marker),
                "missing Profile page marker {marker}"
            );
        }
    }

    #[tokio::test]
    async fn user_setting_chat_channel_page_wires_add_edit_delete_flows() {
        let (_root, state) = test_state();
        let html = body_of(
            user_setting_chat_channel_page(
                State(state.clone()),
                english_request("/user-setting/chat-channel"),
            )
            .await,
        )
        .await;
        for marker in [
            "data-testid='added-channels-section'",
            "data-testid='available-channels-section'",
            "data-testid='chat-channel-dialog'",
            "data-testid='chat-channel-delete-dialog'",
            "Manage your chat channel bots and credentials",
            "Select a chat channel to add",
            "CHAT_CHANNEL_INFO",
            "Feishu / Lark",
            "QQ Bot",
            "WhatsApp",
            "ClickClack",
            "Google Chat",
            "Microsoft Teams",
            "Nextcloud Talk",
            "Slack",
            "LINE",
            "Zalo (Personal)",
            "Bot Token",
            "Client Secret",
            "AES Key",
            "/api/v1/chat-channels",
            "method:chatChannelState.editingId?'PATCH':'POST'",
            "defaultValue:'webhook',options:[{label:'Webhook'",
            "showIf:'connection_type==websocket'",
            "Are you sure you want to delete this chat channel bot?",
            "function setPath(obj,path,value)",
            "cc-tag-chips",
            "key:'nickserv.account'",
            "placeholder:'dingxxxxxxxxxxxx'",
            "placeholder:'irc.libera.chat'",
        ] {
            assert!(
                html.contains(marker),
                "missing Chat channel page marker {marker}"
            );
        }
        // `locales/en.ts::setting.chatChannelDesc` copy must stay verbatim
        // upstream; these five entries had drifted to invented "… bot" wording.
        for expected in [
            "irc:{name:'IRC',desc:'Connect to an IRC server'",
            "line:{name:'LINE',desc:'Connect a LINE messaging bot'",
            "twitch:{name:'Twitch',desc:'Connect a Twitch chat bot'",
            "yuanbao:{name:'Yuanbao',desc:'Connect a Tencent Yuanbao bot'",
            "zalouser:{name:'Zalo (Personal)',desc:'Connect a personal Zalo account'",
        ] {
            assert!(
                html.contains(expected),
                "chat channel description drifted from upstream: {expected}"
            );
        }
        for stale in [
            "Connect an IRC bot",
            "Connect a LINE bot",
            "Connect a Twitch bot",
            "Connect a Yuanbao bot",
            "Connect a Zalo (Personal) bot",
        ] {
            assert!(
                !html.contains(stale),
                "stale chat channel description still present: {stale}"
            );
        }
        // Upstream `locales/zh.ts::setting.chatChannelDesc` copy must stay
        // verbatim upstream on the zh page; the zh-only CC_DESC_ZH map is
        // injected exactly when lang=zh-CN is requested and never on en.
        let zh_html = body_of(
            user_setting_chat_channel_page(
                State(state.clone()),
                Request::builder()
                    .uri("/user-setting/chat-channel")
                    .header("cookie", "lang=zh-CN")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await,
        )
        .await;
        assert!(
            zh_html.contains("var CC_DESC_ZH={"),
            "zh page must inject the CC_DESC_ZH map"
        );
        assert!(
            !html.contains("var CC_DESC_ZH={"),
            "en page must not inject the CC_DESC_ZH map"
        );
        assert!(
            zh_html.contains("desc.textContent=ccDesc(id)"),
            "available card desc must render through ccDesc"
        );
        for (key, expected) in [
            ("clickclack", "连接 ClickClack 机器人"),
            ("discord", "连接 Discord 机器人"),
            ("dingtalk", "连接钉钉机器人"),
            ("feishu", "连接飞书 / Lark 机器人"),
            ("googlechat", "连接 Google Chat 机器人"),
            ("irc", "连接 IRC 服务器"),
            ("line", "连接 LINE 消息机器人"),
            ("matrix", "连接 Matrix 机器人"),
            ("mattermost", "连接 Mattermost 机器人"),
            ("msteams", "连接 Microsoft Teams 机器人"),
            ("nextcloud_talk", "连接 Nextcloud Talk 机器人"),
            ("nostr", "连接 Nostr 机器人"),
            ("qqbot", "连接 QQ 机器人"),
            ("slack", "连接 Slack 机器人"),
            ("synology_chat", "连接群晖 Chat 机器人"),
            ("telegram", "连接 Telegram 机器人"),
            ("tlon", "连接 Tlon (Urbit) 机器人"),
            ("twitch", "连接 Twitch 聊天机器人"),
            ("wecom", "连接企业微信机器人"),
            ("whatsapp", "连接 WhatsApp 机器人（扫码配对）"),
            ("yuanbao", "连接腾讯元宝机器人"),
            ("zalo", "连接 Zalo 机器人"),
            ("zalouser", "连接个人 Zalo 账号"),
        ] {
            assert!(
                zh_html.contains(&format!("{}:'{}'", key, expected)),
                "zh chat channel description drifted from upstream: {key}={expected}"
            );
        }
    }

    #[tokio::test]
    async fn top_k_labels_split_follow_upstream_sites() {
        let (_root, state) = test_state();
        let zh_request = |uri: &str| {
            Request::builder()
                .uri(uri)
                .header("cookie", "lang=zh-CN")
                .body(Body::empty())
                .unwrap()
        };
        let kb = state.kbs.create_for("u1", "TopK KB", "d").unwrap();
        let path = kb.id.clone();
        let kb_en = body_of(
            kb_detail_page(
                State(state.clone()),
                Path(path.clone()),
                english_request("/kbs/x"),
            )
            .await,
        )
        .await;
        let kb_zh = body_of(
            kb_detail_page(
                State(state.clone()),
                Path(path.clone()),
                zh_request("/kbs/x"),
            )
            .await,
        )
        .await;
        let ds_en = body_of(
            dataset_page(
                State(state.clone()),
                Path(path.clone()),
                english_request("/dataset/x"),
            )
            .await,
        )
        .await;
        let ds_zh = body_of(
            dataset_page(
                State(state.clone()),
                Path(path.clone()),
                zh_request("/dataset/x"),
            )
            .await,
        )
        .await;
        let sp_en =
            body_of(search_page(State(state.clone()), english_request("/search")).await).await;
        let sp_zh = body_of(search_page(State(state.clone()), zh_request("/search")).await).await;
        let sk_en =
            body_of(skills_page(State(state.clone()), english_request("/skills")).await).await;
        let sk_zh = body_of(skills_page(State(state.clone()), zh_request("/skills")).await).await;

        // `knowledgeDetails.topK` = `Top-K` (zh `Top-K`) — the KB/dataset
        // retrieval config + testing panels render components/rerank.tsx's
        // label, not the old merged `Top K`.
        for html in [&kb_en, &kb_zh, &ds_en, &ds_zh] {
            assert!(html.contains(">Top-K<"), "upstream `Top-K` label missing");
        }
        // `next-search/search-setting.tsx:467` hard-codes `Top K`, so the
        // search settings sidebar stays untranslated in both languages.
        assert!(
            sp_en.contains(">Top K<"),
            "search sidebar literal Top K missing"
        );
        assert!(
            sp_zh.contains(">Top K<"),
            "zh search sidebar must stay literal Top K"
        );
        assert!(
            !sp_zh.contains("召回条数"),
            "stale Top K translation still present"
        );
        // `skillSearch.topK` = `Top K Results` (zh `返回结果数量`).
        assert!(
            sk_en.contains(">Top K Results<"),
            "skill modal en label missing"
        );
        assert!(
            sk_zh.contains(">返回结果数量<"),
            "skill modal zh label missing"
        );
        assert!(
            !sk_en.contains(">Top K<"),
            "skill modal must not use the merged label"
        );
    }

    #[tokio::test]
    async fn profile_timezone_list_follows_upstream_runtime() {
        let (_root, state) = test_state();
        let html =
            body_of(user_setting_profile_page(english_request("/user-setting/profile")).await)
                .await;
        // Upstream builds the list at runtime from `Intl.supportedValuesOf`
        // with `longOffset` GMT names (`name: offsetString + ' ' + tz`),
        // sorted by offset — replicate the algorithm exactly.
        for marker in [
            "Intl.supportedValuesOf('timeZone')",
            "timeZoneName:'longOffset'",
            "offsetString+' '+tz",
            "a.offset-b.offset",
            "placeholder='Select a timeZone'",
            "tz.name===storedTz",
            "'select your timezone'",
            "provider-tz-list",
            "tzRenderList",
        ] {
            assert!(html.contains(marker), "profile tz marker missing: {marker}");
        }
        // The old hand-picked static list and its display fallback are gone.
        assert!(
            !html.contains("['Asia/Shanghai','Asia/Hong_Kong'"),
            "stale static timezone list still present"
        );
        assert!(
            !html.contains("||'Asia/Shanghai'"),
            "stale display fallback still present"
        );
    }

    #[test]
    fn profile_timezone_list_css_present() {
        let css = include_str!("web_css.txt");
        assert!(css.contains(".provider-tz-list"));
        assert!(css.contains(".provider-tz-list div:hover"));
    }

    #[tokio::test]
    async fn data_source_detail_page_missing_source_renders_404() {
        let (_root, state) = test_state();
        let html = body_of(
            data_source_detail_page(
                State(state),
                Path("missing-source".to_string()),
                english_request("/user-setting/data-source/missing-source"),
            )
            .await,
        )
        .await;
        assert!(
            html.contains("Data source not found"),
            "missing source must render the 404 body"
        );
    }

    #[test]
    fn data_source_detail_body_embeds_schema_and_state_machine() {
        let schema = serde_json::to_string(&crate::data_source::form_fields::schema_for_ui())
            .expect("schema serializes");
        let body = data_source_detail_body("gitlab-main", "s1", "gitlab", &schema, false);
        for marker in [
            "DS_SCHEMA=",
            "config.gitlab_url",
            "GitLab Personal Access Token",
            "id='dsDetailName' autocomplete='off' disabled",
            "dsDetailPrimary",
            "dsPruneField",
            "DS_SOURCE==='rest_api'||DS_SOURCE==='bigquery'",
            "method:'PATCH'",
            "reschedule=true",
            "status:'cancel'",
        ] {
            assert!(
                body.contains(marker),
                "missing data-source detail marker {marker}"
            );
        }
    }

    #[tokio::test]
    async fn data_source_patch_cancel_persists_status_without_syncing() {
        use crate::api::data_source_mgr::{
            DataSourceRecord, DataSourceStore, UpdateDataSourceRequest,
        };
        let (root, mut state) = test_state();
        let store = std::sync::Arc::new(
            DataSourceStore::new(root.path().join("ds-data").to_str().unwrap()).unwrap(),
        );
        std::sync::Arc::get_mut(&mut state).unwrap().data_sources = Some(store.clone());
        let id = "ds-cancel".to_string();
        store
            .insert(DataSourceRecord {
                id: id.clone(),
                name: "webdav-main".into(),
                kind: "webdav".into(),
                url: "https://dav.example.com".into(),
                token: String::new(),
                target: "/docs".into(),
                include_issues: false,
                include_merge_requests: false,
                max_items: 5,
                created_at: "2026-01-01T00:00:00Z".into(),
                connected: false,
                sync_deleted_files: false,
                prune_freq: None,
                refresh_freq: None,
                timeout_secs: None,
                status: "schedule".into(),
                extra: std::collections::HashMap::new(),
                sync_logs: Vec::new(),
            })
            .unwrap();
        let auth = crate::server::AuthContext {
            user_id: "u1".into(),
            is_admin: true,
        };
        let body: UpdateDataSourceRequest =
            serde_json::from_value(serde_json::json!({ "status": "cancel" })).unwrap();
        let response = crate::api::data_source_mgr::update_data_source(
            axum::extract::State(state),
            axum::Extension(auth),
            axum::extract::Path(id.clone()),
            axum::Json(body),
        )
        .await
        .expect("cancel patch succeeds");
        assert_eq!(response["code"], 0);
        assert_eq!(response["data"]["status"], "cancel");
        assert_eq!(store.get(&id).unwrap().status, "cancel");
    }

    #[tokio::test]
    async fn data_source_logs_endpoint_paginates_ragflow_shaped_rows() {
        use crate::api::data_source_mgr::{DataSourceRecord, DataSourceStore};
        let (root, mut state) = test_state();
        let store = std::sync::Arc::new(
            DataSourceStore::new(root.path().join("ds-logs").to_str().unwrap()).unwrap(),
        );
        std::sync::Arc::get_mut(&mut state).unwrap().data_sources = Some(store.clone());
        let id = "ds-logs-1".to_string();
        let record = DataSourceRecord {
            id: id.clone(),
            name: "logs".into(),
            kind: "webdav".into(),
            url: "https://dav.example.com".into(),
            token: String::new(),
            target: "/docs".into(),
            include_issues: false,
            include_merge_requests: false,
            max_items: 5,
            created_at: "2026-01-01T00:00:00Z".into(),
            connected: true,
            sync_deleted_files: false,
            prune_freq: Some(60),
            refresh_freq: Some(30),
            timeout_secs: None,
            status: "done".into(),
            extra: std::collections::HashMap::new(),
            sync_logs: Vec::new(),
        };
        store.insert(record.clone()).unwrap();
        for i in 0..12 {
            store
                .append_sync_log(
                    &id,
                    &record,
                    if i % 3 == 0 { "FAIL" } else { "DONE" },
                    "sync",
                    &format!("run {i}"),
                    Some(i as u64 + 1),
                    Some(i as u64 + 2),
                    None,
                    None,
                    None,
                    Some("kb1"),
                    Some("Finance"),
                )
                .unwrap();
        }
        let query: std::collections::HashMap<String, String> =
            serde_json::from_str(r#"{"page":"1","page_size":"5"}"#).unwrap();
        let response = crate::api::data_source_mgr::get_data_source_logs(
            axum::extract::State(state.clone()),
            axum::extract::Path(id.clone()),
            axum::extract::Query(query),
        )
        .await
        .expect("logs page 1 succeeds");
        assert_eq!(response["code"], 0);
        assert_eq!(response["data"]["total"], 12);
        assert_eq!(response["data"]["logs"].as_array().unwrap().len(), 5);
        let first = &response["data"]["logs"][0];
        assert_eq!(first["kb_name"], "Finance");
        assert_eq!(first["task_type"], "sync");
        assert!(first["time_started"].is_string());
        // 上游分页约束：page_size 上限 100，超过返回 400。
        let query: std::collections::HashMap<String, String> =
            serde_json::from_str(r#"{"page":"1","page_size":"101"}"#).unwrap();
        let error = crate::api::data_source_mgr::get_data_source_logs(
            axum::extract::State(state),
            axum::extract::Path(id),
            axum::extract::Query(query),
        )
        .await
        .expect_err("page_size above 100 is rejected");
        assert_eq!(error.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn data_source_patch_nested_config_persists_and_rebuilds_detail() {
        use crate::api::data_source_mgr::{
            DataSourceRecord, DataSourceStore, UpdateDataSourceRequest,
        };
        let (root, mut state) = test_state();
        let store = std::sync::Arc::new(
            DataSourceStore::new(root.path().join("ds-config").to_str().unwrap()).unwrap(),
        );
        std::sync::Arc::get_mut(&mut state).unwrap().data_sources = Some(store.clone());
        let id = "ds-config".to_string();
        store
            .insert(DataSourceRecord {
                id: id.clone(),
                name: "webdav-main".into(),
                kind: "webdav".into(),
                url: "https://dav.example.com".into(),
                token: String::new(),
                target: "/docs".into(),
                include_issues: false,
                include_merge_requests: false,
                max_items: 5,
                created_at: "2026-01-01T00:00:00Z".into(),
                connected: false,
                sync_deleted_files: false,
                prune_freq: None,
                refresh_freq: None,
                timeout_secs: None,
                status: "unstart".into(),
                extra: std::collections::HashMap::new(),
                sync_logs: Vec::new(),
            })
            .unwrap();
        let auth = crate::server::AuthContext {
            user_id: "u1".into(),
            is_admin: true,
        };
        let body: UpdateDataSourceRequest = serde_json::from_value(serde_json::json!({
            "config": {
                "base_url": "https://dav2.example.com",
                "credentials": {"username": "alice", "password": "secret"},
                "sync_deleted_files": true,
            },
            "prune_freq": 7,
        }))
        .unwrap();
        let response = crate::api::data_source_mgr::update_data_source(
            axum::extract::State(state),
            axum::Extension(auth),
            axum::extract::Path(id.clone()),
            axum::Json(body),
        )
        .await
        .expect("config patch succeeds");
        let record = store.get(&id).unwrap();
        assert_eq!(record.url, "https://dav2.example.com");
        assert_eq!(record.prune_freq, Some(7));
        assert!(record.sync_deleted_files);
        assert_eq!(record.extra["config.credentials.username"], "alice");
        assert_eq!(
            response["data"]["config"]["credentials"]["username"],
            "alice"
        );
        assert_eq!(response["data"]["config"]["sync_deleted_files"], true);
    }

    #[tokio::test]
    async fn data_source_create_accepts_nested_config_and_folds_flat_fields() {
        use crate::api::data_source_mgr::{CreateDataSourceRequest, DataSourceStore};
        let (root, mut state) = test_state();
        let store = std::sync::Arc::new(
            DataSourceStore::new(root.path().join("ds-create").to_str().unwrap()).unwrap(),
        );
        std::sync::Arc::get_mut(&mut state).unwrap().data_sources = Some(store.clone());
        let body: CreateDataSourceRequest = serde_json::from_value(serde_json::json!({
            "name": "gitlab-create",
            "type": "gitlab",
            "config": {
                "project_owner": "o",
                "project_name": "p",
                "gitlab_url": "https://g.example.com",
                "batch_size": "7",
                "credentials": {"gitlab_access_token": "t"},
            },
        }))
        .unwrap();
        let response = crate::api::data_source_mgr::create_data_source(
            axum::extract::State(state),
            axum::Json(body),
        )
        .await
        .expect("schema-driven create succeeds");
        let id = response["data"]["id"].as_str().unwrap().to_string();
        let record = store.get(&id).unwrap();
        assert_eq!(record.url, "https://g.example.com");
        assert_eq!(record.target, "o/p");
        assert_eq!(record.token, "t");
        assert_eq!(record.max_items, 7);
        assert_eq!(record.prune_freq, Some(5));
        assert_eq!(record.timeout_secs, Some(1740));
        assert_eq!(record.extra["config.project_owner"], "o");
    }

    #[tokio::test]
    async fn data_source_create_without_config_still_requires_url_and_target() {
        use crate::api::data_source_mgr::{CreateDataSourceRequest, DataSourceStore};
        let (root, mut state) = test_state();
        let store = std::sync::Arc::new(
            DataSourceStore::new(root.path().join("ds-flat").to_str().unwrap()).unwrap(),
        );
        std::sync::Arc::get_mut(&mut state).unwrap().data_sources = Some(store.clone());
        let body: CreateDataSourceRequest =
            serde_json::from_value(serde_json::json!({ "name": "n", "type": "webdav" })).unwrap();
        let error = crate::api::data_source_mgr::create_data_source(
            axum::extract::State(state),
            axum::Json(body),
        )
        .await
        .expect_err("flat create without url/target fails closed");
        assert!(
            error.1.contains("name/url/target are required"),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn datasources_page_wires_schema_driven_add_modal() {
        let (_root, _state) = test_state();
        let html = body_of(
            datasources_page(english_request("/user-setting/data-source"))
                .await
                .expect("data-source page renders"),
        )
        .await;
        for marker in [
            "data-testid='data-source-add-modal'",
            "openDsAdd(",
            "dscRenderFields",
            "dscCreate",
            "DS_SCHEMA=",
            "window.DS_ZH=false;",
            ".ds-field-tip{display:block",
            "tooltip_en",
            "config.bucket_name",
            "Create your ",
            "method:'POST'",
        ] {
            assert!(
                html.contains(marker),
                "missing data-source add-modal marker {marker}"
            );
        }
    }

    #[tokio::test]
    async fn datasources_page_renders_available_cards_and_grouped_delete_modal() {
        let (_root, _state) = test_state();
        let html = body_of(
            datasources_page(english_request("/user-setting/data-source"))
                .await
                .expect("data-source page renders"),
        )
        .await;
        for marker in [
            "data-testid='available-sources-section'",
            "data-testid='data-source-delete-modal'",
            "dsRenderCards",
            "dsGroupedRows",
            "openDsDelete",
            "DS_SOURCE_INFO=",
            "assets/svg/data-source/s3.svg",
            "Delete data source",
            "Are you sure you want to delete this data source link?",
            "dsLocalType",
            "data-testid='box-oauth-dialog'",
            "Configure Box OAuth credentials",
            "Submit &amp; Authorize",
            "/api/v1/connectors/box/oauth/web/start",
            "/api/v1/connectors/box/oauth/web/result",
            "data-testid='google-oauth-dialog'",
            "Complete Google verification",
            "Authorize with Google",
            "/api/v1/connectors/google/oauth/web/start?type=",
            "/api/v1/connectors/google/oauth/web/result?type=",
        ] {
            assert!(
                html.contains(marker),
                "missing data-source list marker {marker}"
            );
        }
    }
}
