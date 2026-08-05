//! RayRAG Web UI — Rust SSR, mirrors RAGFlow's complete UI flow.
//! Every button, every page, every interaction matches RAGFlow's design.

use crate::server::{AppState, AuthContext};
use axum::{
    extract::{Extension, Path, State},
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
        "Agents" => "智能体",
        "Memories" => "记忆",
        "Files" => "文件",
        "Settings" => "设置",
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
        "Rerank" => "重排序",
        "Top K" => "返回数量",
        "Threshold" => "相似度阈值",
        "Name" => "名称",
        "Description" => "描述",
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
        "Similarity Threshold" => "相似度阈值",
        "Vector Weight" => "向量权重",
        "No documents yet" => "暂无文档",
        _ => k,
    }
}
fn detect_lang(h: &axum::http::HeaderMap) -> bool {
    h.get("Cookie")
        .and_then(|c| c.to_str().ok())
        .map(|c| c.contains("lang=zh-CN"))
        .unwrap_or(true)
}
const CSS: &str = include_str!("./web_css.txt");

fn layout(title: &str, body: &str, zh: bool) -> String {
    let db = t("Dashboard", zh);
    let kb = t("Knowledge Bases", zh);
    let sr = t("Search", zh);
    let ch = t("Chat", zh);
    let ag = t("Agents", zh);
    let mm = t("Memories", zh);
    let fl = t("Files", zh);
    let sk = t("Skills", zh);
    let pr = t("Providers", zh);
    let st = t("Settings", zh);
    let ad = t("Admin", zh);
    let sc = t("Status", zh);
    let lo = t("Login", zh);
    let lc = if zh { "zh-CN" } else { "en" };
    let lb = if zh { "EN" } else { "中" };
    let mut h = String::with_capacity(8192);
    h.push_str("<!DOCTYPE html><html lang='");
    h.push_str(lc);
    h.push_str("'><head><meta charset='UTF-8'><meta name='viewport' content='width=device-width,initial-scale=1'><title>");
    h.push_str(title);
    h.push_str(" · RayRAG</title><style>");
    h.push_str(CSS);
    h.push_str("</style></head><body>");
    h.push_str("<header class='hdr'><a href='/dashboard' class='logo'><svg width='28' height='28'><circle cx='14' cy='14' r='12' fill='#4f46e5'/><text x='14' y='19' text-anchor='middle' fill='#fff' font-size='13' font-weight='700'>R</text></svg>RayRAG</a>");
    h.push_str("<nav class='np'><a href='/dashboard'>");
    h.push_str(db);
    h.push_str("</a><a href='/kbs'>");
    h.push_str(kb);
    h.push_str("</a><a href='/search'>");
    h.push_str(sr);
    h.push_str("</a><a href='/chat'>");
    h.push_str(ch);
    h.push_str("</a><a href='/agents'>");
    h.push_str(ag);
    h.push_str("</a><a href='/memories'>");
    h.push_str(mm);
    h.push_str("</a><a href='/datasources'>");
    h.push_str("🔗");
    h.push_str("</a><a href='/files'>");
    h.push_str(fl);
    h.push_str("</a><a href='/skills'>");
    h.push_str(&sk);
    h.push_str("</a><a href='/providers'>");
    h.push_str(&pr);
    h.push_str("</a></nav>");
    h.push_str("<div class='hr'><button onclick='toggleTheme()' title='Theme'>🌓</button><button onclick='toggleLang()' title='Language'>");
    h.push_str(lb);
    h.push_str(
        "</button><div class='dd'><div class='av'>A</div><div class='dm'><a href='/settings'>⚙️ ",
    );
    h.push_str(st);
    h.push_str("</a><a href='/admin'>🔧 ");
    h.push_str(ad);
    h.push_str("</a><a href='/status'>📊 ");
    h.push_str(sc);
    h.push_str("</a><a href='/api-docs'>📖 API</a><a href='/login' style='color:#ef4444'>🚪 ");
    h.push_str(lo);
    h.push_str("</a></div></div></div></header>");
    h.push_str("<div class='ct'>");
    h.push_str(body);
    h.push_str("</div>");
    h.push_str("<script>(function(){if(localStorage.getItem('theme')==='dark')document.body.classList.add('dark');var nativeFetch=window.fetch.bind(window);window.fetch=function(input,init){init=init||{};var url=typeof input==='string'?input:(input&&input.url)||'';if(url.indexOf('/api/v1/')===0){var token=localStorage.getItem('rayrag_token');if(token){var headers=new Headers(init.headers||{});if(!headers.has('Authorization'))headers.set('Authorization','Bearer '+token);init.headers=headers}}return nativeFetch(input,init).then(function(response){if(response.status===401&&url!='/api/v1/auth/login'){localStorage.removeItem('rayrag_token');location.href='/login'}return response})}})();function toggleTheme(){document.body.classList.toggle('dark');var d=document.body.classList.contains('dark');localStorage.setItem('theme',d?'dark':'light')};function toggleLang(){var n=document.cookie.indexOf('lang=zh-CN')>=0?'en':'zh-CN';document.cookie='lang='+n+';path=/;max-age=31536000';location.reload()}</script></body></html>");
    h
}

// ── Pages ───────────────────────────────────────────────────────

pub async fn index() -> Redirect {
    Redirect::permanent("/dashboard")
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
        cards.push_str(&format!("<div class='kb-card' onclick=\"location.href='/kbs/{}'\"><div class='more' onclick='event.stopPropagation();showKbMenu(event,\"{}\",\"{}\")'>⋯</div><div class='av-lg'>{}</div><h3>{}</h3><p>{} files · {} chunks</p><p style='margin-top:4px'>{}</p></div>", kb.id, kb.id, kb.name.replace("'", "\\'"), initial, kb.name, kb.doc_count, kb.chunk_count, if kb.description.is_empty(){"-"}else{&kb.description}));
    }
    if kbs.is_empty() {
        cards="<p style='color:var(--tm);text-align:center;padding:40px;grid-column:1/-1'>No knowledge bases yet. Click \"New KB\" to create one.</p>".into();
    }
    let mut b = String::new();
    b.push_str(
        "<div style='display:flex;align-items:center;margin-bottom:24px'><h1 style='margin:0'>",
    );
    b.push_str(t("Knowledge Bases", zh));
    b.push_str("</h1><button class='btn btn-p' style='margin-left:auto' onclick='showCreate()'>+ ");
    b.push_str(t("New KB", zh));
    b.push_str("</button></div>");
    b.push_str("<div class='kb-grid'>");
    b.push_str(&cards);
    b.push_str("</div>");
    // Create modal
    b.push_str("<div class='modal-overlay' id='createModal'><div class='modal'><h2>");
    b.push_str(t("New KB", zh));
    b.push_str("</h2><input id='kbName' placeholder='");
    b.push_str(t("Name", zh));
    b.push_str("'><input id='kbDesc' placeholder='");
    b.push_str(t("Description", zh));
    b.push_str("' style='margin-top:8px'><select id='kbPerm' style='margin-top:8px'><option value=''>Permission: default</option><option value='team'>Team</option><option value='me'>Me</option></select><select id='kbEmbd' style='margin-top:8px'><option value=''>Embedding: default</option><option value='default'>default</option></select><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='createKb()'>");
    b.push_str(t("Create", zh));
    b.push_str("</button><button class='btn btn-o' onclick='hideCreate()'>Cancel</button></div></div></div>");
    // Rename modal
    b.push_str("<div class='modal-overlay' id='renameModal'><div class='modal'><h2>Rename</h2><input id='rnName'><input id='rnDesc' style='margin-top:8px'><input type='hidden' id='rnId'><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='renameKb()'>Save</button><button class='btn btn-o' onclick='document.getElementById(\"renameModal\").classList.remove(\"show\")'>Cancel</button></div></div></div>");
    b.push_str("<script>function agentFilter(){var q=(document.getElementById('agentQ').value||'').toLowerCase();document.querySelectorAll('#agentTb tr').forEach(function(tr){var m=tr.getAttribute('data-name')||'';tr.style.display=(!q||m.indexOf(q)>=0)?'':'none')})}document.getElementById('createModal').classList.add('show')};function hideCreate(){document.getElementById('createModal').classList.remove('show')};async function createKb(){var n=document.getElementById('kbName').value;if(!n)return;var d=document.getElementById('kbDesc').value;var em=document.getElementById('kbEmbd');var p=document.getElementById('kbPerm');var body={name:n,description:d,dsl:{}};if(em&&em.value)body.embedding_model=em.value;if(p&&p.value)body.permission=p.value;await fetch('/api/v1/datasets',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});location.reload()};function showKbMenu(e,id,name){e.stopPropagation();if(confirm('Rename or delete \"'+name+'\"?\\nOK=Rename, Cancel=Delete')){document.getElementById('rnId').value=id;document.getElementById('rnName').value=name;document.getElementById('renameModal').classList.add('show')}else{if(confirm('Delete \"'+name+'\"?')){fetch('/api/v1/datasets/'+id,{method:'DELETE'}).then(()=>location.reload())}}};function renameKb(){var id=document.getElementById('rnId').value;var n=document.getElementById('rnName').value;var d=document.getElementById('rnDesc').value;fetch('/api/v1/datasets/'+id,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n,description:d})}).then(()=>location.reload())}</script>");
    Html(layout(t("Knowledge Bases", zh), &b, zh))
}

pub async fn kb_detail_page(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let kb = state.kbs.get(&id);
    let name = kb.as_ref().map(|k| k.name.clone()).unwrap_or_default();
    let desc = kb.as_ref().map(|k| k.description.clone()).unwrap_or_default();
    let docs = kb.as_ref().map(|k| k.doc_count).unwrap_or(0);
    let chunks = kb.as_ref().map(|k| k.chunk_count).unwrap_or(0);
    let initial = name
        .chars()
        .next()
        .unwrap_or('K')
        .to_uppercase()
        .collect::<String>();
    let mut b = String::new();
    // 页头：返回按钮 + 头像 + 名称/描述 + 统计（RAGFlow 数据集详情页风格）
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
    // Tab 导航（纯 JS 切换：文档 / 配置 / 测试）
    b.push_str("<div class='tab-bar'><a class='active' id='nav0' onclick='showTab(0)'>📂 ");
    b.push_str(t("Documents", zh));
    b.push_str("</a><a id='nav1' onclick='showTab(1)'>⚙️ ");
    b.push_str(t("Configuration", zh));
    b.push_str("</a><a id='nav2' onclick='showTab(2)'>🧪 ");
    b.push_str(t("Testing", zh));
    b.push_str("</a></div>");
    // Tab 0：文档列表 + 上传
    b.push_str("<div id='tab0'>");
    b.push_str("<div class='card'><h2>");
    b.push_str(t("Upload", zh));
    b.push_str("</h2><div style='display:flex;align-items:center;gap:8px;flex-wrap:wrap'><input type='file' id='fi' multiple style='max-width:400px'><button class='btn btn-p' id='upBtn' onclick='uploadDoc()'>");
    b.push_str(t("Upload", zh));
    b.push_str("</button><span id='upMsg' style='font-size:12px;color:var(--tm)'></span></div><p style='font-size:11px;color:var(--tm);margin-top:8px'>PDF, DOCX, TXT, MD, HTML, CSV, XLSX, EPUB, JSON, PPT, PNG/JPG(OCR), MP3/WAV(ASR)</p></div>");
    b.push_str("<div class='card'><h2>");
    b.push_str(t("Documents", zh));
    b.push_str(" <span id='docCount' style='font-size:13px;color:var(--tm)'></span></h2><div id='dl' style='overflow-x:auto'>");
    b.push_str(t("Loading...", zh));
    b.push_str("</div></div></div>");
    // Tab 1：配置（RAGFlow Configuration 风格表单）
    b.push_str("<div id='tab1' style='display:none'>");
    b.push_str("<div class='card'><h2>");
    b.push_str(t("Configuration", zh));
    b.push_str("</h2><p style='color:var(--tm);margin-bottom:16px'>解析与分块参数（键名对齐 RAGFlow parser_config camelCase），保存后对新上传的文档生效。</p><table style='width:100%'>");
    b.push_str("<tr><td style='width:220px'>");
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
    b.push_str("<tr><td colspan=2 style='padding-top:16px;border-top:1px solid var(--bd)'><h3 style='font-size:14px;margin:0'>RAPTOR（递归摘要）</h3></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Enable RAPTOR", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='cfgRaptor'> ");
    b.push_str(t("Enable RAPTOR", zh));
    b.push_str("</label></td></tr>");
    b.push_str("<tr><td colspan=2 style='padding-top:16px;border-top:1px solid var(--bd)'><h3 style='font-size:14px;margin:0'>GraphRAG（实体抽取）</h3></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Enable GraphRAG", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='cfgGraph'> ");
    b.push_str(t("Enable GraphRAG", zh));
    b.push_str("</label></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Embedding Model", zh));
    b.push_str("</td><td><span id='cfgEmbd' class='co'>-</span></td></tr>");
    b.push_str("</table><div style='margin-top:16px'><button class='btn btn-p' onclick='saveConfig()'>");
    b.push_str(t("Save", zh));
    b.push_str("</button> <span id='cfgMsg' style='font-size:12px;color:var(--tm)'></span></div></div></div>");
    // Tab 2：测试（检索测试面板，仿搜索页 JS）
    b.push_str("<div id='tab2' style='display:none'>");
    b.push_str("<div class='card'><h2>");
    b.push_str(t("Testing", zh));
    b.push_str("</h2><p style='color:var(--tm);margin-bottom:16px'>输入测试问题，验证本知识库的检索质量。</p><div style='display:grid;grid-template-columns:1fr 1fr;gap:16px'><div><h3>");
    b.push_str(t("Test Query", zh));
    b.push_str("</h3><label style='font-size:12px;display:block'>");
    b.push_str(t("Question", zh));
    b.push_str("</label><input id='tq' placeholder='输入测试问题…'><label style='font-size:12px;display:block;margin-top:8px'>");
    b.push_str(t("Top K", zh));
    b.push_str("</label><select id='tk'><option>5</option><option selected>10</option><option>20</option></select><label style='font-size:12px;display:block;margin-top:8px'>");
    b.push_str(t("Similarity Threshold", zh));
    b.push_str("</label><select id='tt'><option value='0.0'>0.0</option><option selected value='0.2'>0.2</option><option value='0.3'>0.3</option><option value='0.5'>0.5</option></select><label style='font-size:12px;display:block;margin-top:8px'>");
    b.push_str(t("Vector Weight", zh));
    b.push_str("</label><select id='tw'><option>0.3</option><option selected>0.5</option><option>0.7</option><option>1.0</option></select><label style='font-size:12px;display:block;margin-top:8px'><input type='checkbox' id='tr'> ");
    b.push_str(t("Rerank", zh));
    b.push_str("</label><button class='btn btn-p' style='margin-top:12px;width:100%' onclick='runTest()'>");
    b.push_str(t("Test", zh));
    b.push_str("</button></div><div><h3>");
    b.push_str(t("Results", zh));
    b.push_str("</h3><div id='testRes' style='min-height:300px;background:var(--bg);padding:16px;border-radius:var(--r)'><p style='color:var(--tm);text-align:center;padding:40px'>");
    b.push_str(t("Results", zh));
    b.push_str(" 将显示在这里</p></div></div></div></div></div>");
    // 页面 JS
    b.push_str("<script>var kbId='");
    b.push_str(&id);
    b.push_str("';var pollTimer=null;function showTab(n){for(var i=0;i<3;i++){var t=document.getElementById('tab'+i);var nav=document.getElementById('nav'+i);if(t)t.style.display='none';if(nav)nav.classList.remove('active')}document.getElementById('tab'+n).style.display='block';document.getElementById('nav'+n).classList.add('active')};");
    b.push_str("function fmtSize(n){n=n||0;if(n<1024)return n+' B';if(n<1048576)return (n/1024).toFixed(1)+' KB';return (n/1048576).toFixed(1)+' MB'};function fmtTime(ts){if(!ts)return '-';try{return new Date(ts).toLocaleString()}catch(e){return ts}};");
    b.push_str("function badge(run){run=run||'UNSTARTED';if(run==='DONE')return '<span class=\\\"badge bs\\\">DONE</span>';if(run==='RUNNING')return '<span class=\\\"badge bi\\\">RUNNING</span>';if(run==='FAILED')return '<span class=\\\"badge bw\\\" style=\\\"color:var(--d)\\\">FAILED</span>';return '<span class=\\\"badge bw\\\">'+(run||'UNSTARTED')+'</span>'};");
    b.push_str("function loadDocs(){fetch('/api/v1/datasets/'+kbId+'/documents').then(r=>r.json()).then(function(d){var docs=d.data||[];document.getElementById('docCount').textContent='('+docs.length+')';if(!docs.length){document.getElementById('dl').innerHTML='<p style=color:var(--tm);text-align:center;padding:30px>暂无文档</p>';return}var h='<table><thead><tr><th>名称</th><th>解析状态</th><th>块数</th><th>大小</th><th>更新时间</th><th style=width:200px>操作</th></tr></thead><tbody>';docs.forEach(function(dd){var nm=(dd.name||'').replace(/\\\"/g,'');h+='<tr><td style=font-weight:500>'+dd.name+'</td><td>'+badge(dd.run)+'</td><td>'+(dd.chunk_count||0)+'</td><td>'+fmtSize(dd.size)+'</td><td style=color:var(--tm);font-size:12px>'+fmtTime(dd.created_at)+'</td><td><button class=\\\"btn btn-o btn-sm\\\" data-id=\\\"'+dd.id+'\\\" data-name=\\\"'+nm+'\\\" onclick=\\\"renameDoc(this)\\\">✏️</button> <button class=\\\"btn btn-o btn-sm\\\" data-id=\\\"'+dd.id+'\\\" onclick=\\\"reparseDoc(this)\\\">重新解析</button> <button class=\\\"btn btn-d btn-sm\\\" data-id=\\\"'+dd.id+'\\\" onclick=\\\"delDoc(this)\\\">删除</button></td></tr>'});h+='</tbody></table>';document.getElementById('dl').innerHTML=h})};");
    b.push_str("function renameDoc(btn){var id=btn.getAttribute('data-id');var old=btn.getAttribute('data-name')||'';var nn=prompt('重命名文档：',old);if(!nn)return;fetch('/api/v1/datasets/'+kbId+'/documents/'+id,{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:nn})}).then(r=>r.json()).then(function(){loadDocs()})};");
    b.push_str("function reparseDoc(btn){var id=btn.getAttribute('data-id');if(!confirm('重新解析该文档？'))return;fetch('/api/v1/datasets/'+kbId+'/documents/'+id+'/reparse',{method:'POST'}).then(function(){loadDocs();schedulePoll()})};");
    b.push_str("function delDoc(btn){var id=btn.getAttribute('data-id');if(!confirm('删除该文档？'))return;fetch('/api/v1/datasets/'+kbId+'/documents/'+id,{method:'DELETE'}).then(function(){loadDocs()})};");
    b.push_str("function schedulePoll(){if(pollTimer)return;pollTimer=setInterval(function(){fetch('/api/v1/datasets/'+kbId+'/documents').then(r=>r.json()).then(function(d){var docs=d.data||[];loadDocs();var busy=docs.some(function(x){return x.run==='RUNNING'||x.run==='UNSTARTED'});if(!busy&&pollTimer){clearInterval(pollTimer);pollTimer=null}})},3000)};");
    b.push_str("function uploadDoc(){var files=document.getElementById('fi').files;if(!files||!files.length)return;var btn=document.getElementById('upBtn');var msg=document.getElementById('upMsg');if(btn)btn.disabled=true;var n=0;function next(){if(n>=files.length){if(btn)btn.disabled=false;if(msg)msg.textContent='';loadDocs();schedulePoll();return}var f=files[n++];var fd=new FormData();fd.append('file',f,f.name);if(msg)msg.textContent='上传中 '+(n)+'/'+(files.length)+' …';fetch('/api/v1/datasets/'+kbId+'/documents',{method:'POST',body:fd}).then(r=>r.json()).then(function(){next()}).catch(function(){next()})}next()};");
    b.push_str("function loadConfig(){fetch('/api/v1/datasets/'+kbId).then(r=>r.json()).then(function(d){if(d.code!==0)return;var kb=d.data||{};var em=document.getElementById('cfgEmbd');if(em)em.textContent=kb.embd_id||'default';var pc={};try{pc=JSON.parse(kb.parser_config||'{}')}catch(e){}if(pc.chunk_method)document.getElementById('cfgMethod').value=pc.chunk_method;var tn=parseInt(pc.chunk_token_num,10);if(!isNaN(tn)&&tn>0)document.getElementById('cfgTokenNum').value=tn;var delims=Array.isArray(pc.delimiters)?pc.delimiters:(pc.delimiter?[pc.delimiter]:[]);document.getElementById('cfgDelims').value=delims.join('\\n');var op=parseFloat(pc.overlapped_percent);if(!isNaN(op))document.getElementById('cfgOverlap').value=Math.round(op*100);document.getElementById('cfgAutoKeywords').checked=!!pc.auto_keywords;document.getElementById('cfgAutoQuestions').checked=!!pc.auto_questions;document.getElementById('cfgChildren').checked=!!pc.enable_children;document.getElementById('cfgRaptor').checked=!!(pc.raptorEn||pc.raptor_enabled);document.getElementById('cfgGraph').checked=!!(pc.graphEn||pc.graphrag_enabled)})};");
    b.push_str("function saveConfig(){var delims=document.getElementById('cfgDelims').value.split('\\n').map(function(s){return s.trim()}).filter(Boolean);if(!delims.length)delims=['\\n'];var body={chunk_method:document.getElementById('cfgMethod').value,chunk_token_num:parseInt(document.getElementById('cfgTokenNum').value,10)||2048,delimiters:delims,overlapped_percent:Math.min(1,Math.max(0,(parseFloat(document.getElementById('cfgOverlap').value)||0)/100)),auto_keywords:document.getElementById('cfgAutoKeywords').checked,auto_questions:document.getElementById('cfgAutoQuestions').checked,enable_children:document.getElementById('cfgChildren').checked,raptorEn:document.getElementById('cfgRaptor').checked,graphEn:document.getElementById('cfgGraph').checked};var m=document.getElementById('cfgMsg');if(m)m.textContent='保存中…';fetch('/api/v1/datasets/'+kbId,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(r=>r.json()).then(function(d){if(m){if(d.code===0){m.textContent='✅ 已保存';m.style.color='var(--s)'}else{m.textContent='保存失败: '+(d.message||'未知错误');m.style.color='var(--d)'}}})};");
    b.push_str("function runTest(){var q=document.getElementById('tq').value;if(!q){alert('请输入测试问题');return}var k=parseInt(document.getElementById('tk').value,10);var t=parseFloat(document.getElementById('tt').value);var w=parseFloat(document.getElementById('tw').value);var r=document.getElementById('tr').checked;var body={question:q,kb_ids:[kbId],top_k:k,similarity_threshold:t,vector_similarity_weight:w};if(r)body.rerank=true;document.getElementById('testRes').innerHTML='<p style=color:var(--tm);text-align:center;padding:40px>检索中…</p>';fetch('/api/v1/retrieval',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(function(resp){return resp.json()}).then(function(d){if(d.code!==0){document.getElementById('testRes').innerHTML='<p style=color:var(--d)>检索失败: '+(d.message||'未知错误')+'</p>';return}var chunks=(d.data&&d.data.chunks)||[];var h='';chunks.forEach(function(c,i){h+='<div class=ri><span class=rs>#'+(i+1)+' · 相似度 '+(c.score||0).toFixed(4)+'</span><div class=rc>'+hl(c.content||'',q)+'</div><div class=rm>📄 '+(c.doc_name||'')+'</div></div>'});document.getElementById('testRes').innerHTML=h||'<p style=color:var(--tm);text-align:center;padding:40px>无匹配结果</p>'}).catch(function(e){document.getElementById('testRes').innerHTML='<p style=color:var(--d)>请求失败: '+e+'</p>'})};");
    b.push_str("function hl(t,q){if(!q)return t;try{return t.replace(new RegExp(q.replace(/[.*+?^${}()|[\\\\]\\\\]/g,'\\\\$&'),'gi'),function(m){return '<mark>'+m+'</mark>'})}catch(e){return t}};");
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
    b.push_str("</a></li></ul></div>");
    // Content
    b.push_str("<div class='ds-content'>");
    // Tab 0: Documents
    b.push_str("<div id='tab0'><div class='card'><h2>");
    b.push_str(t("Upload", zh));
    b.push_str("</h2><input type='file' id='fi' multiple style='max-width:400px'><button class='btn btn-p' id='upBtn' onclick='uploadDoc()' style='margin-left:8px'><button class='btn btn-o' style='margin-left:8px' onclick='delSelected()'>🗑 Batch Delete</button><button class='btn btn-o' style='margin-left:8px' onclick='clearAll()'>🗑 Clear All</button>");
    b.push_str(t("Upload", zh));
    b.push_str("</button><p style='font-size:11px;color:var(--tm);margin-top:8px'>PDF, DOCX, TXT, MD, HTML, CSV, XLSX, EPUB, JSON, PPT, PNG/JPG(OCR), MP3/WAV(ASR)</p></div><div class='card'><h2>");
    b.push_str(t("Documents", zh));
    b.push_str(" <span id='docCount' style='font-size:13px;color:var(--tm)'></span></h2><div id='dl'>Loading...</div></div></div>");
    // Tab 1: Overview
    b.push_str("<div id='tab1' style='display:none'><div class='st'><div class='sc'><span class='lbl'>Total Docs</span><span class='val'>");
    b.push_str(&docs.to_string());
    b.push_str(
        "</span></div><div class='sc'><span class='lbl'>Total Chunks</span><span class='val'>",
    );
    b.push_str(&chunks.to_string());
    b.push_str("</span></div><div class='sc'><span class='lbl'>Success</span><span class='val' style='color:var(--s)'>");
    b.push_str(&docs.to_string());
    b.push_str("</span><div class='progress-bar'><div class='fill' style='width:100%'></div></div></div><div class='sc'><span class='lbl'>Failed</span><span class='val' style='color:var(--d)'>0</span></div></div><div class='card'><h2>Recent Activity</h2><table><thead><tr><th>Document</th><th>Status</th><th>Chunks</th><th>Date</th></tr></thead><tbody id='actList'></tbody></table></div></div>");
    // Tab 2: Testing
    b.push_str("<div id='tab2' style='display:none'><div class='card'><h2>");
    b.push_str(t("Testing", zh));
    b.push_str("</h2><p style='color:var(--tm);margin-bottom:16px'>Test retrieval quality with sample queries.</p><div style='display:grid;grid-template-columns:1fr 1fr;gap:16px'><div><h3>Test Settings</h3><label style='font-size:12px'>Query</label><input id='testQ' placeholder='Enter test query...'><label style='font-size:12px;margin-top:8px;display:block'>");
    b.push_str(t("Top K", zh));
    b.push_str("</label><select id='testK'><option>5</option><option selected>10</option><option>20</option></select><label style='font-size:12px;margin-top:8px;display:block'>");
    b.push_str(t("Rerank", zh));
    b.push_str("</label><label><input type='checkbox' id='testRerank'> Enable rerank</label><button class='btn btn-p' style='margin-top:12px;width:100%' onclick='testSearch()'>");
    b.push_str(t("Test", zh));
    b.push_str("</button></div><div><h3>Results</h3><div id='testResults' style='min-height:300px'><p style='color:var(--tm);text-align:center;padding:40px'>Results will appear here</p></div></div></div></div></div>");
    // Tab 3: Configuration
    b.push_str("<div id='tab3' style='display:none'><div class='card'><h2>");
    b.push_str(t("Configuration", zh));
    b.push_str("</h2><table><tr><td style='width:160px'>Chunk Method</td><td><select id='chunkMethod' onchange='toggleChunkOptions()'><option value='naive'>Naive (token-based)</option><option value='manual'>Manual</option><option value='qa'>Q&A</option><option value='table'>Table</option><option value='paper'>Paper</option><option value='book'>Book</option><option value='laws'>Laws</option><option value='presentation'>Presentation</option><option value='one'>One</option><option value='knowledge_graph'>Knowledge Graph</option></select></td></tr><tr id='chunkSizeRow'><td>");
    b.push_str(t("Chunk Size", zh));
    b.push_str("</td><td><select id='chunkSize'><option>256</option><option selected>512</option><option>1024</option><option>2048</option></select></td></tr><tr id='overlapRow'><td>");
    b.push_str(t("Overlap", zh));
    b.push_str("</td><td><select id='overlap'><option>0%</option><option selected>5%</option><option>10%</option><option>20%</option></select></td></tr><tr><td>Embedding Model</td><td><select id='embdSel'><option value=''>default</option></select></td></tr><tr><td>Delimiter</td><td><input value='\\n\\n' style='max-width:200px'></td></tr><tr><td>Parser Type</td><td><select><option>DeepDOC (PDF/DOCX)</option><option>Plain Text</option></select></td></tr><tr><td>");
    b.push_str(t("Embedding Model", zh));
    b.push_str("</td><td><input value='all-MiniLM-L6-v2' readonly></td></tr><tr><td>");
    b.push_str(t("Rerank", zh));
    b.push_str("</td><td><label><input type='checkbox' checked> mxbai-rerank-large-v2</label></td></tr><tr><td colspan=2 style='padding-top:16px;border-top:1px solid var(--bd)'><h3 style='font-size:14px;margin:0'>RAPTOR (Recursive Abstractive Processing)</h3></td></tr><tr><td>Enable RAPTOR</td><td><label><input type='checkbox' id='raptorEn'> Enable recursive summarization tree</label></td></tr><tr><td>Max Depth</td><td><select id='raptorDepth' disabled><option>3</option><option selected>5</option><option>7</option></select></td></tr><tr><td colspan=2 style='padding-top:16px;border-top:1px solid var(--bd)'><h3 style='font-size:14px;margin:0'>GraphRAG (Entity Extraction)</h3></td></tr><tr><td>Enable GraphRAG</td><td><label><input type='checkbox' id='graphEn'> Enable entity-relation extraction</label></td></tr><tr><td>Entity Types</td><td><input value='organization, person, location, event, concept' style='font-size:12px'></td></tr><tr><td>Method</td><td><select><option>Light</option><option>General</option></select></td></tr></table><button class='btn btn-p' style='margin-top:16px' onclick='saveConfig()'>");
    b.push_str(t("Save", zh));
    b.push_str("</button></div></div>");
    // Tab 4: Knowledge Graph
    b.push_str("<div id='tab4' style='display:none'><div class='card'><h2>");
    b.push_str(t("Knowledge Graph", zh));
    b.push_str("</h2><p style='color:var(--tm);margin-bottom:16px'>Entity-relation graph extracted from indexed documents.</p><button class='btn btn-p' onclick='buildGraph()'>");
    b.push_str(t("Build", zh));
    b.push_str(" Graph</button><div id='graphResult' style='margin-top:16px;background:var(--bg);padding:20px;border-radius:var(--r);min-height:200px;font-family:monospace;font-size:12px;white-space:pre-wrap;line-height:1.8'><p style='color:var(--tm);text-align:center;padding:40px'>Click \"Build Graph\" to extract entities and relations.</p></div></div></div>");
    b.push_str("</div></div>");
    // JS
    b.push_str("<script>var kbId='");
    b.push_str(&id);
    b.push_str("';function showTab(n){for(var i=0;i<5;i++){var t=document.getElementById('tab'+i);var nav=document.getElementById('nav'+i);if(t)t.style.display='none';if(nav)nav.classList.remove('active')}document.getElementById('tab'+n).style.display='block';document.getElementById('nav'+n).classList.add('active')};");
    b.push_str("function clearAll(){if(!confirm('Delete ALL documents in this dataset?'))return;fetch('/api/v1/datasets/'+kbId+'/documents').then(r=>r.json()).then(function(d){var docs=d.data||[];if(!docs.length){loadDocs();return}var n=0;function next(){if(n>=docs.length){loadDocs();return}var id=docs[n++].id;fetch('/api/v1/datasets/'+kbId+'/documents/'+id,{method:'DELETE'}).then(function(){next()})}next()})};function renameDoc(id,oldName){var nn=prompt('Rename document:',oldName);if(!nn)return;fetch('/api/v1/datasets/'+kbId+'/documents/'+id,{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:nn})}).then(r=>r.json()).then(function(d){loadDocs()})};function docSelAll(c){document.querySelectorAll('.docCk').forEach(function(x){x.checked=c.checked})};function delSelected(){var ids=[];document.querySelectorAll('.docCk:checked').forEach(function(x){ids.push(x.value)});if(!ids.length)return;if(!confirm('Delete '+ids.length+' documents?'))return;var n=0;function next(){if(n>=ids.length){loadDocs();return}var id=ids[n++];fetch('/api/v1/datasets/'+kbId+'/documents/'+id,{method:'DELETE'}).then(function(){next()})}next()};function loadDocs(){fetch('/api/v1/datasets/'+kbId+'/documents').then(r=>r.json()).then(d=>{var docs=d.data||[];document.getElementById('docCount').textContent='('+docs.length+')';var h='<table><thead><tr><th><input type=checkbox onclick=docSelAll(this)></th><th>Name</th><th>Chunks</th><th>Status</th><th>Actions</th></tr></thead><tbody>';docs.forEach(function(d){h+='<tr><td><input type=checkbox class=docCk value='+d.id+'></td><td><strong>'+d.name+'</strong></td><td>'+d.chunk_count+'</td><td><span class=\"badge '+(d.run==='DONE'?'bs':'bw')+'\">'+(d.run||'ready')+'</span></td><td><a class=\"btn btn-o btn-sm\" href=\"/documents/'+kbId+'/'+d.id+'\">View</a><button class=\"btn btn-o btn-sm\" onclick='renameDoc(\"'+d.id+'\",\"'+d.name+'\")'>✏️</button><button class=\"btn btn-p btn-sm\" onclick=\"reparseDoc(\\''+d.id+'\\')\">Parse</button><button class=\"btn btn-d btn-sm\" onclick=\"delDoc(\\''+d.id+'\\')\">Del</button></td></tr>'});document.getElementById('dl').innerHTML=h+'</tbody></table>'})};");
    b.push_str("function reparseDoc(did){if(!confirm('Reparse?'))return;fetch('/api/v1/datasets/'+kbId+'/documents/'+did+'/reparse',{method:'POST'}).then(loadDocs)};var pollTimer=null;function uploadDoc(){var files=document.getElementById('fi').files;if(!files||!files.length)return;var btn=document.getElementById('upBtn');if(btn)btn.disabled=true;var n=0;function next(){if(n>=files.length){if(btn)btn.disabled=false;loadDocs();pollParse();return}var f=files[n++];var fd=new FormData();fd.append('file',f,f.name);fetch('/api/v1/datasets/'+kbId+'/documents',{method:'POST',body:fd}).then(r=>r.json()).then(function(){next()}).catch(function(){next()})}next()};function pollDocs(){fetch('/api/v1/datasets/'+kbId+'/documents').then(r=>r.json()).then(function(d){var docs=d.data||[];loadDocs();var running=docs.some(function(x){return x.run!=='DONE'&&x.run!=='failed'&&x.run!=='cancel'});if(!running&&pollTimer){clearInterval(pollTimer);pollTimer=null}})};function delDoc(did){if(!confirm('Delete?'))return;fetch('/api/v1/datasets/'+kbId+'/documents/'+did,{method:'DELETE'}).then(loadDocs)};");
    b.push_str("function testSearch(){var q=document.getElementById('testQ').value;if(!q)return;var k=parseInt(document.getElementById('testK').value);var r=document.getElementById('testRerank').checked;document.getElementById('testResults').innerHTML='<p style=color:var(--tm)>Searching...</p>';fetch('/api/v1/retrieval',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:q,top_k:k})}).then(r=>r.json()).then(d=>{var items=d.data.chunks||d.data.results||[];var h='';items.forEach(function(c,i){c=c.chunk||c;h+='<div class=ri><span class=rs>#'+(i+1)+' &middot; '+(c.similarity||c.score||0).toFixed(4)+'</span><div class=rc>'+hl((c.content||'').substring(0,300),q)+'</div><div class=rm>'+(c.doc_name||'')+'</div></div>'});document.getElementById('testResults').innerHTML=h||'<p style=color:var(--tm)>No results</p>'})};");
    b.push_str("function buildGraph(){document.getElementById('graphResult').innerHTML='<p style=color:var(--tm)>Building... (LLM entity extraction)</p>';fetch('/api/v1/datasets/'+kbId+'/graphrag/build',{method:'POST'}).then(r=>r.json()).then(d=>{if(d.code!==0){document.getElementById('graphResult').textContent='Error: '+(d.message||'unknown');return}var data=d.data||{};var nodes=data.node_list||[];var maxD=1;nodes.forEach(function(n){maxD=Math.max(maxD,n.degree||1)});var R=120;var svg='<svg width=640 height=460 style=background:var(--bg);border-radius:var(--r)><text x=320 y=30 text-anchor=middle font-size=14 fill=var(--t2)>'+data.nodes+' nodes / '+data.edges+' edges</text>';nodes.slice(0,24).forEach(function(n,i){var ang=2*Math.PI*i/nodes.slice(0,24).length;var cx=320+R*Math.cos(ang);var cy=240+R*Math.sin(ang);var r=(n.degree||1)/maxD*22+10;svg+='<line x1=320 y1=240 x2='+cx+' y2='+cy+' stroke=var(--p) stroke-opacity=0.35></line><circle cx='+cx+' cy='+cy+' r='+r+' fill=var(--p) fill-opacity=0.15 stroke=var(--p)></circle><text x='+cx+' y='+(cy+r+14)+' text-anchor=middle font-size=11 fill=var(--t2)>'+n.name.slice(0,14)+'</text>'});svg+='<circle cx=320 cy=240 r=26 fill=var(--p)></circle><text x=320 y=245 text-anchor=middle font-size=11 fill=var(--bg)>KB</text></svg>';var h='<div style=margin-bottom:8px>Nodes: '+data.nodes+' | Edges: '+data.edges+' | Documents: '+data.documents+' | Entities: '+data.entities+'</div>'+svg+'<div style=margin-top:10px;font-size:12px;color:var(--tm)>(环形图：节点大小=degree，中心=知识库)</div>';document.getElementById('graphResult').innerHTML=h})};fetch('/api/v1/providers').then(r=>r.json()).then(function(d){var sel=document.getElementById('embdSel');if(!sel)return;(d.data||[]).forEach(function(p){if(!p.models)return;(p.models||[]).forEach(function(m){var o=document.createElement('option');o.value=p.id+':'+m;o.textContent=m;sel.appendChild(o)})})}).catch(function(){});function saveConfig(){var body={chunk_method:document.getElementById('chunkMethod').value,chunk_size:parseInt(document.getElementById('chunkSize').value),overlap:document.getElementById('overlap').value};var raptor=document.getElementById('raptorEn').checked;if(raptor){body.raptor_enabled=true;body.raptor_depth=parseInt(document.getElementById('raptorDepth').value)}var graph=document.getElementById('graphEn').checked;if(graph){body.graphrag_enabled=true}fetch('/api/v1/datasets/'+kbId,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(r=>r.json()).then(d=>{if(d.code===0){alert('Configuration saved!')}else{alert('Error: '+(d.message||'unknown'))}})};function toggleChunkOptions(){var m=document.getElementById('chunkMethod').value;var show=['naive','manual'].indexOf(m)>=0;document.getElementById('chunkSizeRow').style.display=show?'':'none';document.getElementById('overlapRow').style.display=show?'':'none'};document.getElementById('raptorEn').onchange=function(){document.getElementById('raptorDepth').disabled=!this.checked};loadDocs()</script>");
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
        let preview = c
            .messages
            .last()
            .map(|m| m.content.chars().take(80).collect::<String>())
            .unwrap_or_else(|| "No messages".into());
        cards.push_str(&format!("<div class='chat-card' onclick=\"location.href='/chat/{}'\"><div style='display:flex;align-items:center;gap:12px'><div class='av' style='width:40px;height:40px;font-size:16px'>{}</div><div><h3 style='font-size:15px'>{}</h3><p style='font-size:12px;color:var(--tm)'>{} msgs</p></div></div><p style='font-size:13px;color:var(--t2);margin-top:12px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap'>{}</p><button class='btn btn-d btn-sm' style='margin-top:8px' onclick='event.stopPropagation();delConv('{}')'>Del</button><button class='btn btn-o btn-sm' style='margin-top:8px' onclick='event.stopPropagation();renConv('{}')'>✏️</button></div>", c.id, c.id, initial, c.name.chars().take(30).collect::<String>(), c.messages.len(), preview, c.id));
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
    b.push_str("<div class='modal-overlay' id='newChatModal'><div class='modal'><h2>New Chat</h2><p style='color:var(--tm);margin-bottom:16px'>Start a new conversation with RayRAG.</p><div style='display:flex;gap:8px'><button class='btn btn-p' onclick='createChat()'>Create & Start</button><button class='btn btn-o' onclick='document.getElementById(\"newChatModal\").classList.remove(\"show\")'>Cancel</button></div></div></div>");
    b.push_str("<script>function renConv(id){var nn=prompt('Rename conversation:');if(!nn)return;fetch('/api/v1/conversations/'+id,{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:nn})}).then(r=>r.json()).then(function(d){if(d.code===0){loadChats()}else{alert(d.message||'Error')}})};function delConv(id){if(!confirm('Delete this conversation?'))return;fetch('/api/v1/conversations/'+id,{method:'DELETE'}).then(function(){location.reload()})};function newChat(){document.getElementById('newChatModal').classList.add('show')};function createChat(){fetch('/api/v1/chats',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:'New Chat'})}).then(r=>r.json()).then(d=>{var id=d.data?d.data.id:d.id;location.href='/chat/'+id})}</script>");
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
    b.push_str("<div class='card' style='width:240px;flex-shrink:0'><div style='display:flex;gap:8px'><a href='/chat' style='font-size:13px;color:var(--p);text-decoration:none'>← Back</a><a class='btn btn-o btn-sm' style='font-size:11px;padding:2px 8px' href='/api/v1/conversations/");
    b.push_str(&id);
    b.push_str("/export' target='_blank'>Export</a><button class='btn btn-d btn-sm' style='font-size:11px;padding:2px 8px' onclick='clearConv()'>Clear</button></div><h3 style='margin-top:12px'>");
    b.push_str(&name);
    b.push_str("</h3><p style='font-size:11px;color:var(--tm)'>ID: ");
    b.push_str(&id[..8]);
    b.push_str("...</p>");
    // Settings panel (RAGFlow-style)
    b.push_str("<script>function clearConv(){if(!confirm('Clear all messages?'))return;fetch('/api/v1/conversations/'+document.getElementById('convId').value+'/clear',{method:'POST'}).then(r=>r.json()).then(d=>{if(d.code===0){location.reload()}else{alert('Error: '+(d.message||'unknown'))}})}</script><input type='hidden' id='convId' value='");
    b.push_str(&id);
    b.push_str("'>");
    b.push_str("<div style='margin-top:20px;border-top:1px solid var(--bd);padding-top:16px'><h3 style='font-size:13px;margin-bottom:8px'>Chat Settings</h3>");
    b.push_str("<label style='font-size:11px;display:block;margin:6px 0'>Model</label><select id='chatModel'><option>MiniMax-M3</option><option>Qwen3.5-9B</option><option>GPT-4</option></select>");
    b.push_str("<label style='font-size:11px;display:block;margin:6px 0'>Temperature</label><input type='range' min='0' max='2' step='0.1' value='0.7' id='chatTemp' style='padding:0'><span id='tempVal' style='font-size:11px;color:var(--tm)'>0.7</span>");
    b.push_str("<label style='font-size:11px;display:block;margin:6px 0'>Max Tokens</label><select id='chatMaxTok'><option>512</option><option selected>1024</option><option>2048</option><option>4096</option></select>");
    b.push_str("<label style='font-size:11px;display:block;margin:6px 0'>System Prompt</label><textarea id='chatSys' rows='3' style='font-size:12px' placeholder='You are a helpful assistant...'>You are a helpful AI assistant. Use the context to answer questions.</textarea>");
    b.push_str("<label style='font-size:11px;display:flex;align-items:center;gap:4px;margin-top:8px'><input type='checkbox' checked> Show citations</label>");
    b.push_str("<label style='font-size:11px;display:flex;align-items:center;gap:4px;margin-top:4px'><input type='checkbox'> Stream response</label>");
    b.push_str("</div></div>");
    // Chat area
    b.push_str("<div class='card' style='flex:1;display:flex;flex-direction:column'><div style='flex:1;overflow-y:auto;padding:12px;background:var(--bg);border-radius:var(--r);margin-bottom:12px;min-height:400px' id='chatMsgs'>");
    b.push_str(&msgs);
    b.push_str("</div><div class='sb'><select id='chatModel' onchange='window.chatModel=this.value' style='margin-right:8px;max-width:180px'><option value=''>Model: default</option></select><input id='chatInput' placeholder='Ask anything...' onkeydown='if(event.key===&quot;Enter&quot;)sendMsg()'><button class='btn btn-p' onclick='sendMsg()'>");
    b.push_str(t("Send", zh));
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
    b.push_str("document.getElementById('chatTemp').oninput=function(){document.getElementById('tempVal').textContent=this.value}");
    b.push_str("fetch('/api/v1/providers').then(r=>r.json()).then(function(d){var ps=d.data||[];var sel=document.getElementById('chatModel');if(!sel)return;(ps||[]).forEach(function(p){if(!p.models)return;(p.models||[]).forEach(function(m){var o=document.createElement('option');o.value=p.id+':'+m;o.textContent=m;sel.appendChild(o)})})}).catch(function(){});function sendMsg(){var q=document.getElementById('chatInput').value;if(!q)return;var m=document.getElementById('chatMsgs');m.innerHTML+='<div class=cmu>'+q+'</div>';document.getElementById('chatInput').value='';var mi=msgIdx;var box=m.innerHTML+='';fetch('/api/v1/chats/'+sid+'/completions',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:q,stream:true})}).then(function(r){var reader=r.body.getReader();var dec=new TextDecoder();var buf='';var acc='';var mid='';var el=document.createElement('div');el.className='cma';el.id='msg'+mi;el.innerHTML='<span style=color:var(--tm)>🤔 思考中…</span>';m.appendChild(el);m.scrollTop=m.scrollHeight;function pump(){return reader.read().then(function(x){if(x.done){el.innerHTML+=refsHtml(mid);el.innerHTML+='<div class=msg-actions><button class=btn-ghost btn-sm onclick=copyMsg('+mi+') title=Copy>📋</button><button class=btn-ghost btn-sm onclick=regenMsg('+mi+',&quot;'+mid+'&quot;) title=Regenerate>🔄</button><button class=btn-ghost btn-sm onclick=likeMsg(&quot;'+mid+'&quot;,1,this) title=Like>👍</button><button class=btn-ghost btn-sm onclick=likeMsg(&quot;'+mid+'&quot;,0,this) title=Dislike>👎</button></div></div>';m.scrollTop=m.scrollHeight;msgIdx++;return}buf+=dec.decode(x.value,{stream:true});var lines=buf.split('\n');buf=lines.pop();lines.forEach(function(line){if(line.indexOf('data:')!==0)return;var data=line.slice(5).trim();if(data==='[DONE]')return;try{var ev=JSON.parse(data);if(ev.object==='chat.completion.references'){var rs=(ev.references||[]).map(function(r,i){return '<div class=cc>['+(i+1)+'] '+(r.content||'').substring(0,80)+'</div>'}).join('');el.innerHTML+=rs;m.scrollTop=m.scrollHeight;return}var c=ev.choices&&ev.choices[0]&&ev.choices[0].delta&&ev.choices[0].delta.content||'';if(c){if(acc===''){el.innerHTML=''}acc+=c;el.innerHTML=acc;m.scrollTop=m.scrollHeight}}catch(e){}});return pump()})}function refsHtml(mid){return ''}return pump()}).catch(function(){el.innerHTML='<span style=color:var(--err)>Error</span>'})}");
    b.push_str("function copyMsg(i){var el=document.getElementById('msg'+i);if(el){var t=el.innerText.replace(/📋🔄👍👎/g,'').trim();navigator.clipboard.writeText(t);alert('Copied!')}}");
    b.push_str("function regenMsg(i,mid){var el=document.getElementById('msg'+i);if(!el||!mid)return;var old=el.innerHTML;el.innerHTML='<p style=color:var(--tm)>Regenerating...</p>';fetch('/api/v1/chats/'+sid+'/messages/'+mid+'/regenerate',{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'}).then(async function(r){var d=await r.json();if(!r.ok)throw new Error(d.message||'Regenerate failed');var a=d.data?d.data.answer:'...';var refs=(d.data&&d.data.citations||[]).map(function(c,n){return '<div class=cc>['+(n+1)+'] '+c.substring(0,100)+'</div>'}).join('');el.innerHTML=a+refs+'<div class=msg-actions><button class=btn-ghost btn-sm onclick=copyMsg('+i+') title=Copy>📋</button><button class=btn-ghost btn-sm onclick=regenMsg('+i+',&quot;'+mid+'&quot;) title=Regenerate>🔄</button><button class=btn-ghost btn-sm onclick=likeMsg(&quot;'+mid+'&quot;,1,this) title=Like>👍</button><button class=btn-ghost btn-sm onclick=likeMsg(&quot;'+mid+'&quot;,0,this) title=Dislike>👎</button></div>'}).catch(function(e){el.innerHTML=old;alert(e.message)})}");
    b.push_str("function likeMsg(mid,v,btn){if(!mid)return;fetch('/api/v1/chats/'+sid+'/messages/'+mid+'/feedback',{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({thumbup:!!v})}).then(async function(r){var d=await r.json();if(!r.ok)throw new Error(d.message||'Feedback failed');var actions=btn.parentElement;actions.querySelectorAll('button').forEach(function(b){b.style.color=''});btn.style.color=v?'var(--s)':'var(--d)'}).catch(function(e){alert(e.message)})}");
    b.push_str("</script>");
    Html(layout(&name, &b, zh))
}

pub async fn search_page(
    State(state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let chunks = state.engine.read().unwrap().len();
    let mut b = String::new();
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
    b.push_str("<div id='searchView' style='display:none'><div style='display:flex;gap:16px'><div style='flex:1'><div class='card'><div class='sb'><input id='q2' placeholder='Search...' onkeydown='if(event.key===&quot;Enter&quot;)doSearch2()'><button class='btn btn-p' onclick='doSearch2()'>Search</button></div><div style='display:flex;gap:12px;margin-top:12px;align-items:center'><label style='font-size:12px;display:flex;align-items:center;gap:4px'><input type='checkbox' id='rerank'> Re-rank</label><label style='font-size:12px;display:flex;align-items:center;gap:4px'><input type='checkbox' id='hybrid'> BM25+Vector</label></div><div id='kbChips' style='margin-top:10px;display:flex;gap:6px;flex-wrap:wrap;font-size:12px'></div><button class='btn btn-o' style='font-size:12px;padding:4px 10px;margin-left:auto' onclick='exportResults()'>⬇ Export MD</button></div></div><div id='results' style='margin-top:16px'></div></div>");
    // Settings sidebar
    b.push_str("<div class='card' style='width:240px;flex-shrink:0'><h3>Search Settings</h3><label style='font-size:12px;display:block;margin:8px 0'>");
    b.push_str(t("Top K", zh));
    b.push_str("</label><select id='topK'><option>5</option><option selected>10</option><option>20</option><option>50</option></select><label style='font-size:12px;display:block;margin:8px 0'>");
    b.push_str(t("Threshold", zh));
    b.push_str("</label><select id='thresh'><option>0.0</option><option selected>0.3</option><option>0.5</option><option>0.7</option></select><label style='font-size:12px;display:block;margin:8px 0'>");
    b.push_str(t("Vector Weight", zh));
    b.push_str("</label><select id='vWeight'><option>0.3</option><option>0.5</option><option selected>0.7</option><option>1.0</option></select><h3 style='margin-top:20px'>Quick Questions</h3><a href='#' onclick='quickQ(\"What is RayRAG?\")' style='display:block;font-size:12px;color:var(--p);text-decoration:none;padding:4px 0'>What is RayRAG?</a><a href='#' onclick='quickQ(\"How does RAG work?\")' style='display:block;font-size:12px;color:var(--p);text-decoration:none;padding:4px 0'>How does RAG work?</a></div></div></div>");
    b.push_str("<script>var lastResults=[];var lastQuery='';function loadKbs(){fetch('/api/v1/datasets').then(r=>r.json()).then(d=>{var list=d.data||[];var chips='<label style=display:flex;align-items:center;gap:4px><input type=checkbox class=kbCk value=__all__ checked> All</label>';list.forEach(function(k){chips+='<label style=display:flex;align-items:center;gap:4px><input type=checkbox class=kbCk value='+k.id+'> '+k.name+'</label>'});document.getElementById('kbChips').innerHTML=chips})};loadKbs();function hl(t,q){if(!q)return t;try{return t.replace(new RegExp(q.replace(/[.*+?^${}()|[\\]\\]/g,'\\$&'),'gi'),function(m){return '<mark>'+m+'</mark>'})}catch(e){return t}};function doSearch(){var q=document.getElementById('query').value;if(!q)return;document.getElementById('searchHome').style.display='none';document.getElementById('searchView').style.display='block';document.getElementById('q2').value=q;runSearch(q)};function doSearch2(){runSearch(document.getElementById('q2').value)};function quickQ(q){document.getElementById('searchHome').style.display='none';document.getElementById('searchView').style.display='block';document.getElementById('q2').value=q;runSearch(q)};function searchPage(d){window.sPage+=d;runSearch(lastQuery)};function runSearch(q){var r=document.getElementById('rerank').checked;var h=document.getElementById('hybrid').checked;var k=parseInt(document.getElementById('topK').value);var t=parseFloat(document.getElementById('thresh').value);var e=r?'/api/v1/retrieval':'/api/v1/search';var body={question:q,top_k:k};var kbs=[];var allOn=false;document.querySelectorAll('.kbCk:checked').forEach(function(c){if(c.value==='__all__'){allOn=true}else{kbs.push(c.value)}});if(kbs.length&&!allOn){body.kb_ids=kbs}if(!r){body.similarity_threshold=t;body.vector_similarity_weight=parseFloat(document.getElementById('vWeight').value)};document.getElementById('results').innerHTML='<p style=color:var(--tm);padding:24px>Searching...</p>';fetch(e,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(r=>r.json()).then(d=>{var items=d.data.chunks||d.data.results||[];var h='';items.forEach(function(c,i){c=c.chunk||c;var content=(c.content||c.text||'').substring(0,500);var fullContent=(c.content||c.text||'');var docName=(c.doc_name||c.document_name||'Unknown');var score=(c.similarity||c.score||0).toFixed(4);h+='<div class=ri><span class=rs>#'+(i+1)+' &middot; '+score+(r?' &middot; reranked':'')+'</span>'<div class=rc>'+hl(content,q)+'</div><div class=rm>'+docName+' &middot; '+(c.chunk_id||c.id||'')+' <a href=# onclick=\"previewDoc(\''+docName.replace(/'/g,'')+'\',\''+fullContent.replace(/'/g,'').substring(0,2000)+'\')\" style=\"color:var(--p);font-size:11px;margin-left:8px\">📄 Preview</a></div></div>'});if(gc&&gc.length){h+='<div class=card style=margin-top:16px><h3>🔗 Graph Context</h3>'+gc.map(function(g,i){return '<div class=cc>['+(i+1)+'] '+g.substring(0,120)+'</div>'}).join('')+'</div>'}lastResults=items;lastQuery=q;document.getElementById('results').innerHTML=h||'<p style=color:var(--tm);padding:24px>No results found.</p>'})};function exportResults(){if(!lastResults.length){alert('No results to export');return}var md='# Search: '+lastQuery+'\n\n';lastResults.forEach(function(c,i){c=c.chunk||c;md+='## '+(i+1)+'. '+(c.doc_name||'Unknown')+' (score '+(c.similarity||c.score||0).toFixed(4)+')\n\n'+(c.content||c.text||'')+'\n\n'});var blob=new Blob([md],{type:'text/markdown'});var a=document.createElement('a');a.href=URL.createObjectURL(blob);a.download='search-export.md';a.click()};function previewDoc(name,content){var m=document.getElementById('docPreview');document.getElementById('docPreviewTitle').textContent=name;document.getElementById('docPreviewContent').textContent=content;m.classList.add('show')};function closePreview(){document.getElementById('docPreview').classList.remove('show')}</script>");
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
    b.push_str("</button><button class='btn btn-d' onclick='delAgent()'>");
    b.push_str(t("Delete", zh));
    b.push_str("</button></div>");
    b.push_str("<div class='st' style='margin-top:16px'><div class='sc'><span class='lbl'>ID</span><span class='val' id='agentId' style='font-size:13px'>-</span></div><div class='sc'><span class='lbl'>KBs</span><span class='val' id='agentKbs'>-</span></div><div class='sc'><span class='lbl'>Status</span><span class='val' id='agentStatus' style='font-size:15px'>-</span></div></div>");
    b.push_str("<div class='card'><h2>");
    b.push_str(t("Canvas", zh));
    b.push_str("</h2><div id='canvasView' style='overflow:auto;border:1px solid var(--bd);border-radius:var(--r);padding:8px;min-height:260px'><p style='color:var(--tm)'>Loading topology...</p></div></div>");
    b.push_str("<div style='display:grid;grid-template-columns:1fr 1fr;gap:16px;margin-top:16px'><div class='card'><h2>");
    b.push_str(t("Canvas DSL", zh));
    b.push_str("</h2><button class='btn btn-o btn-sm' style='margin-bottom:8px' onclick='editDsl()'>✏️ Edit DSL</button> <button class='btn btn-p btn-sm' style='margin-bottom:8px' onclick='saveDsl()'>💾 Save</button><textarea id='agentDslEdit' style='display:none;width:100%;min-height:300px;font-family:monospace;font-size:12px' spellcheck='false'></textarea><pre id='agentDsl' style='background:var(--bg);padding:16px;border-radius:var(--r);font-size:12px;overflow:auto;max-height:480px;white-space:pre-wrap'>Loading...</pre></div><div class='card'><h2>");
    b.push_str(t("Test", zh));
    b.push_str("</h2><input id='aq2' placeholder='Ask...' style='width:100%'><button class='btn btn-p' style='margin-top:8px;width:100%' onclick='askAgent()'>");
    b.push_str(t("Ask", zh));
    b.push_str("</button><div id='aa2' style='margin-top:12px;background:var(--bg);padding:16px;border-radius:var(--r);min-height:120px;white-space:pre-wrap;line-height:1.6;font-size:13px'></div></div></div>");
    b.push_str("<script>var aid='");
    b.push_str(&agent_id);
    b.push_str("';function editDsl(){var pre=document.getElementById('agentDsl');var ta=document.getElementById('agentDslEdit');ta.value=pre.textContent;ta.style.display='block';pre.style.display='none';ta.focus()};function saveDsl(){var ta=document.getElementById('agentDslEdit');var v;try{v=JSON.parse(ta.value)}catch(e){alert('Invalid JSON: '+e.message);return}fetch('/api/v1/agents/'+aid,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({dsl:v})}).then(r=>r.json()).then(function(d){if(d.code===0){loadAgent()}else{alert('Save failed: '+(d.message||'unknown'))}})};function loadAgent(){fetch('/api/v1/agents/'+aid).then(r=>r.json()).then(d=>{var a=d.data||{};document.getElementById('agentTitle').textContent='🤖 '+(a.name||'Agent');document.getElementById('agentId').textContent=a.id||'-';document.getElementById('agentKbs').textContent=(a.kb_ids||[]).join(', ')||'none';document.getElementById('agentStatus').textContent=a.canvas_category||'standard';document.getElementById('agentDsl').textContent=JSON.stringify(a.dsl||a.canvas||{},null,2);renderTopology(a.dsl||a.canvas||{})})};function renderTopology(dsl){var nodes=(dsl.nodes||[]);var edges=(dsl.edges||[]);if(!nodes.length){document.getElementById('canvasView').innerHTML='<p style=color:var(--tm)>Empty canvas</p>';return}var xMin=1e9,yMin=1e9,xMax=-1e9,yMax=-1e9;var pos={};nodes.forEach(function(n){var p=n.position||{x:0,y:0};pos[n.id]=p;xMin=Math.min(xMin,p.x);yMin=Math.min(yMin,p.y);xMax=Math.max(xMax,p.x);yMax=Math.max(yMax,p.y)});var W=xMax-xMin+300,H=yMax-yMin+300;var svg='<svg width='+W+' height='+H+' style=background:var(--bg)><defs><marker id=arr markerWidth=8 markerHeight=8 refX=8 refY=4 orient=auto><path d=M0,0L8,4L0,8 fill=var(--p)></path></marker></defs>';edges.forEach(function(e){var s=pos[e.source]||{x:0,y:0};var t=pos[e.target]||{x:0,y:0};svg+='<line x1='+(s.x-xMin+150)+' y1='+(s.y-yMin+150)+' x2='+(t.x-xMin+150)+' y2='+(t.y-yMin+150)+' stroke=var(--p) stroke-width=1.5 marker-end=url(#arr)></line>'});nodes.forEach(function(n){var p=pos[n.id];var label=n.data&&(n.data.title||n.data.name)||n.id;svg+='<g><rect x='+(p.x-xMin+120)+' y='+(p.y-yMin+130)+' width=150 height=40 rx=8 fill=var(--card) stroke=var(--p)></rect><text x='+(p.x-xMin+195)+' y='+(p.y-yMin+153)+' text-anchor=middle font-size=12 fill=var(--t2)>'+label+'</text></g>'});svg+='</svg>';document.getElementById('canvasView').innerHTML=svg};function runAgent(){document.getElementById('aa2').textContent='Running agent...';fetch('/api/v1/agents/'+aid+'/completions',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:'Run agent once'})}).then(r=>r.json()).then(d=>{document.getElementById('aa2').textContent=d.data?d.data.answer:JSON.stringify(d)})};function askAgent(){var q=document.getElementById('aq2').value;if(!q)return;fetch('/api/v1/agents/'+aid+'/completions',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:q})}).then(r=>r.json()).then(d=>{document.getElementById('aa2').textContent=d.data?d.data.answer:JSON.stringify(d)})};function delAgent(){if(!confirm('Delete this agent?'))return;fetch('/api/v1/agents/'+aid,{method:'DELETE'}).then(function(){location.href='/agents'})};loadAgent()</script>");
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
        rows.push_str(&format!("<tr data-name={}><td style='font-weight:500'>{}</td><td style='color:var(--t2)'>{}</td><td class='co'>{}</td><td><button class='btn btn-p btn-sm' onclick=\"testA('{}')\">Test</button> <button class='btn btn-o btn-sm' onclick=\"editA('{}')\">Edit</button> <button class='btn btn-o btn-sm' onclick=\"dupA('{}')\">Copy</button> <button class='btn btn-d btn-sm' onclick=\"delA('{}')\">Del</button></td></tr>",a.name.to_lowercase(),a.name,a.description,a.kb_ids.join(", "),a.id,a.id,a.id,a.id));
    }
    let mut b = String::new();
    b.push_str("<div style='display:flex;align-items:center;gap:12px'><h1 style='flex:1;margin:0'>🤖 ");
    b.push_str(t("Agents", zh));
    b.push_str("</h1><button class='btn btn-p' onclick='showCreate()'>+ ");
    b.push_str(t("New Agent", zh));
    b.push_str("</button></div><div class='card'><input id='agentQ' placeholder='Search agents...' oninput='agentFilter()' style='max-width:260px;margin-bottom:8px'><table><thead><tr><th>Name</th><th>Description</th><th>KBs</th><th style='width:240px'>Actions</th></tr></thead><tbody id='agentTb'>");
    b.push_str(&rows);
    b.push_str("</tbody></table></div>");
    b.push_str("<div class='modal-overlay' id='createModal'><div class='modal'><h2>");
    b.push_str(t("New Agent", zh));
    b.push_str("</h2><input id='aname' placeholder='Name'><textarea id='adesc' placeholder='Description' style='margin-top:8px;min-height:60px'></textarea><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='createA()'>");
    b.push_str(t("Create", zh));
    b.push_str("</button><button class='btn btn-o' onclick='showTemplates()'>From Template</button><button class='btn btn-o' onclick='document.getElementById(\"createModal\").classList.remove(\"show\")'>");
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
        // `</` would close the <script> tag in the rendered HTML; `<\/` is a
        // no-op escape valid in both JS and JSON, so the DSL round-trips.
        let dsl_j = dsl.replace("</", "<\\/");
        tpls.push_str(&format!(
            ",'{}':{{'name':{},'description':{},'dsl':{}}}",
            slug, name_j, desc_j, dsl_j
        ));
    }
    b.push_str(&tpls);
    b.push_str("};
function showTemplates(){var names=Object.keys(agentTemplates);var pick=prompt('Choose template:\n'+names.map(function(k,i){return (i+1)+'. '+agentTemplates[k].name}).join('\n'));if(!pick)return;var idx=parseInt(pick)-1;if(isNaN(idx)||!names[idx]){alert('Invalid choice');return}var t=agentTemplates[names[idx]];var n=prompt('Agent name:',t.name);if(!n)return;fetch('/api/v1/agents',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n,description:(t.description?t.description:'Created from template: '+t.name),dsl:t.dsl})}).then(r=>r.json()).then(function(d){if(d.code===0){location.reload()}else{alert('Create failed: '+(d.message||'unknown'))}})};
var aid='default';function testA(id){aid=id};function showCreate(){document.getElementById('createModal').classList.add('show')};function createA(){var n=document.getElementById('aname').value;if(!n)return;var d=document.getElementById('adesc').value;fetch('/api/v1/agents',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n,description:d})}).then(function(){location.reload()})};function editA(id){window.location.href='/agents/'+id};function dupA(id){fetch('/api/v1/agents/'+id).then(function(r){return r.json()}).then(function(d){var a=d.data||{};fetch('/api/v1/agents',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:(a.name||'agent')+' copy',description:a.description||'',kb_ids:a.kb_ids||[],dsl:a.dsl||{}})}).then(function(){location.reload()})})};function delA(id){if(!confirm('Delete agent?'))return;fetch('/api/v1/agents/'+id,{method:'DELETE'}).then(function(){location.reload()})};function askA(){var q=document.getElementById('aq').value;fetch('/api/v1/agents/'+aid+'/completions',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:q})}).then(r=>r.json()).then(d=>{document.getElementById('aa').textContent=d.data?d.data.answer:JSON.stringify(d)})}</script>");
    Html(layout(t("Agents", zh), &b, zh))
}

pub async fn datasources_page(
    req: axum::http::Request<axum::body::Body>,
) -> Result<axum::response::Html<String>, axum::response::Response> {
    let zh = crate::web::detect_lang(req.headers());
    let mut b = format!("<h1>🔗 {}</h1>", if zh { "数据源（Data Sources）" } else { "Data Sources" });
    b.push_str("<div class='card'><h2>");
    b.push_str(if zh { "新建 GitLab 数据源" } else { "Add GitLab Source" });
    b.push_str("</h2><div style='display:grid;grid-template-columns:1fr 1fr 1fr 1fr;gap:8px;margin-bottom:8px'>");
    b.push_str("<input id='dsName' placeholder='Name'><input id='dsUrl' placeholder='https://gitlab.com'><input id='dsToken' type='password' placeholder='Private token'><input id='dsTarget' placeholder='Project id or owner/repo'>");
    b.push_str("</div><label><input type='checkbox' id='dsIssues'> Issues</label> <label><input type='checkbox' id='dsMRs'> Merge Requests</label> ");
    b.push_str("<button class='btn btn-p' onclick='addDs()'>+ Add</button></div>");
    b.push_str("<div class='card'><h2>Sources</h2><div style='margin-bottom:8px'><select id='dsKb'><option value=''>Sync to KB (optional)</option></select></div><div id='dsList'>Loading...</div></div>");
    b.push_str("<div class='card'><h2>");
    b.push_str(if zh { "同步预览" } else { "Sync Preview" });
    b.push_str("</h2><div id='dsPreview' style='color:var(--t2)'>Select a source and click Sync.</div></div><script>");
    // JS 全部单引号：拼接用单引号字符串，属性用单引号，无任何双引号字符
    b.push_str("function loadKbs(){fetch('/api/v1/datasets').then(function(r){return r.json()}).then(function(d){var kbs=d.data||[];var sel=document.getElementById('dsKb');var h='<option value=\'\'>Sync to KB (optional)</option>';kbs.forEach(function(k){h=h+'<option value=\'\"+k.id+\"\'>'+(k.name||k.id)+'</option>';});sel.innerHTML=h;});}function loadDs(){fetch('/api/v1/data_sources').then(function(r){return r.json()}).then(function(d){var list=d.data||[];var h='<table><tr><th>Name</th><th>Type</th><th>URL</th><th>Target</th><th>Token</th><th></th></tr>';list.forEach(function(x){h=h+'<tr><td>'+x.name+'</td><td>'+x.type+'</td><td>'+(x.url||'')+'</td><td>'+(x.target||'')+'</td><td>'+(x.token||'')+'</td><td><button class=btn-p onclick=syncDs(\'\"+x.id+\"\')>Sync</button> <button class=btn-d onclick=delDs(\'\"+x.id+\"\')>Del</button></td></tr>';});document.getElementById('dsList').innerHTML=h+'</table>';});}");
    b.push_str("function addDs(){var n=document.getElementById('dsName').value;var u=document.getElementById('dsUrl').value;var t=document.getElementById('dsToken').value;var g=document.getElementById('dsTarget').value;if(!n||!u||!g){alert('Name/URL/Target required');return;}var body={name:n,type:'gitlab',url:u,token:t,target:g,include_issues:document.getElementById('dsIssues').checked,include_merge_requests:document.getElementById('dsMRs').checked};fetch('/api/v1/data_sources',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(function(r){return r.json()}).then(function(d){if(d.code===0){loadDs();document.getElementById('dsName').value='';document.getElementById('dsUrl').value='';document.getElementById('dsToken').value='';document.getElementById('dsTarget').value='';}else{alert(d.message||'create failed');}});}");
    b.push_str("function syncDs(id){document.getElementById('dsPreview').textContent='Syncing...';var kb=document.getElementById('dsKb').value||'';var url='/api/v1/data_sources/'+id+'/sync'+(kb?('?kb_id='+encodeURIComponent(kb)):'');fetch(url,{method:'POST'}).then(function(r){return r.json()}).then(function(d){if(d.code===0){var x=d.data||{};var h='<b>'+x.count+' docs</b><table><tr><th>Title</th><th>Ext</th><th>Size</th><th>Type</th></tr>';(x.preview||[]).forEach(function(p){h=h+'<tr><td>'+(p.title||'')+'</td><td>'+(p.extension||'')+'</td><td>'+(p.size_bytes||0)+'</td><td>'+(p.type||'')+'</td></tr>';});document.getElementById('dsPreview').innerHTML=h+'</table>';}else{document.getElementById('dsPreview').textContent='Sync failed: '+(d.message||'unknown');}});}");
    b.push_str("function delDs(id){if(!confirm('Delete this data source?'))return;fetch('/api/v1/data_sources/'+id,{method:'DELETE'}).then(function(r){return r.json()}).then(function(d){if(d.code===0){loadDs();}else{alert(d.message||'delete failed');}});}loadKbs();loadDs();</script>");
    Ok(axum::response::Html(b))
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
    b.push_str("</h2><input type='file' id='fu' style='max-width:400px'><button class='btn btn-p' onclick='uploadF()'>Upload</button></div><div class='card'><h2>Files</h2><div id='fl'>Loading...</div></div><script>function fileFilter(){var q=(document.getElementById('fileQ').value||'').toLowerCase();document.querySelectorAll('#fl tr').forEach(function(tr){var m=tr.getAttribute('data-name')||'';tr.style.display=(!q||m.indexOf(q)>=0)?'':'none')})};function loadF(){fetch('/api/v1/files').then(r=>r.json()).then(d=>{var files=d.data||[];var h='<table><tr><th>Name</th><th>Size</th><th>Type</th><th></th></tr>';files.forEach(function(f){h+='<tr><td>'+f.name+'</td><td>'+(f.size/1024).toFixed(0)+'KB</td><td class=co>'+f.file_type+'</td><td><a class=\'btn btn-o btn-sm\' href=\'/api/v1/files/'+f.id+'/download\' target=\'_blank\'>View</a> <button class=\'btn btn-d btn-sm\' onclick=\'delFile(\"'+f.id+'\")\\'>Del</button></td></tr>'});document.getElementById('fl').innerHTML=h+'</table>'})};function delFile(id){if(!confirm('Delete this file?'))return;fetch('/api/v1/files/'+id,{method:'DELETE'}).then(r=>r.json()).then(d=>{if(d.code===0){loadF()}else{alert('Error: '+(d.message||'unknown'))}})};function uploadF(){var f=document.getElementById('fu').files[0];if(!f)return;var fd=new FormData();fd.append('file',f,f.name);fetch('/api/v1/files/upload',{method:'POST',body:fd}).then(loadF)};loadF()</script>");
    Html(layout(t("Files", zh), &b, zh))
}

pub async fn memories_page(
    State(state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let convs = state.conversations.list();
    let mut rows = String::new();
    for c in convs.iter().rev().take(20) {
        let msgs = c.messages.len();
        let p: String = c
            .messages
            .last()
            .map(|m| m.content.chars().take(60).collect())
            .unwrap_or_default();
        rows.push_str(&format!("<tr><td style='font-weight:500'>{}</td><td>{} msgs</td><td style='color:var(--tm);max-width:300px;overflow:hidden'>{}</td><td><a class='btn btn-o btn-sm' href='/chat/{}'>View</a> <button class='btn btn-d btn-sm' onclick=\"delConv('{}')\">Del</button></td></tr>",c.name.chars().take(40).collect::<String>(),msgs,p,c.id,c.id));
    }
    let mut b = String::new();
    b.push_str("<h1>🧠 ");
    b.push_str(t("Memories", zh));
    b.push_str("</h1><div class='card'><h2>Memory (");
    b.push_str(&convs.len().to_string());
    b.push_str(
        ")</h2><table><thead><tr><th>Name</th><th>Messages</th><th>Last</th><th></th></tr></thead><tbody>",
    );
    b.push_str(&rows);
    b.push_str("</tbody></table><button class='btn btn-d' style='margin-top:12px' onclick='clearAllMem()'>🗑 Clear All</button></div>");
    b.push_str("<script>function delConv(id){if(!confirm('Delete this conversation?'))return;fetch('/api/v1/conversations/'+id,{method:'DELETE'}).then(function(){location.reload()})};function clearAllMem(){if(!confirm('Delete ALL conversations?'))return;fetch('/api/v1/memories/clear',{method:'DELETE'}).then(function(){location.reload()})}</script>");
    Html(layout(t("Memories", zh), &b, zh))
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
    b.push_str("</h1><div class='card'><h2>System</h2><table><tr><td>Version</td><td class='co'>v");
    b.push_str(env!("CARGO_PKG_VERSION"));
    b.push_str("</td></tr><tr><td>Index</td><td class='co'>");
    b.push_str(&state.index_path);
    b.push_str("</td></tr></table></div><div class='card'><h2>Tasks (");
    b.push_str(&tasks.len().to_string());
    b.push_str(
        ")</h2><table><thead><tr><th>Task</th><th>Status</th><th>%</th></tr></thead><tbody>",
    );
    b.push_str(&tr);
    b.push_str("</tbody></table></div>");
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
    b.push_str(")</h2><input id='userQ' placeholder='Search users...' oninput='userFilter()' style='max-width:260px;margin-bottom:8px'><table><thead><tr><th>Name</th><th>Email</th><th>Role</th><th style='width:200px'>Actions</th></tr></thead><tbody id='userTb'>");
    b.push_str(&ur);
    b.push_str("</tbody></table></div><script>function userFilter(){var q=(document.getElementById('userQ').value||'').toLowerCase();document.querySelectorAll('#userTb tr').forEach(function(tr){var m=tr.getAttribute('data-name')||'';var e=tr.getAttribute('data-email')||'';tr.style.display=(!q||m.indexOf(q)>=0||e.indexOf(q)>=0)?'':'none')})};function resetPw(id){var n=prompt('New password (min 6 chars):');if(!n)return;fetch('/api/v1/admin/users/'+id+'/password',{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({new_password:n})}).then(r=>r.json()).then(d=>{if(d.code===0){location.reload()}else{alert('Error: '+(d.message||'unknown'))}})};function delUser(id){if(!confirm('Delete this user?'))return;fetch('/api/v1/admin/users/'+id,{method:'DELETE'}).then(r=>r.json()).then(d=>{if(d.code===0){location.reload()}else{alert('Error: '+(d.message||'unknown'))}})}</script>");
    Html(layout(t("Admin", zh), &b, zh))
}

pub async fn login_page(req: axum::http::Request<axum::body::Body>) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let b = "<div class='auth-card card'><h1 style='text-align:center;margin-bottom:24px'>🐠 RayRAG</h1><div id='loginForm'><input id='email' autocomplete='username' placeholder='Email'><input id='password' type='password' autocomplete='current-password' placeholder='Password'><button class='btn btn-p' onclick='login()' style='width:100%;margin-top:8px'>Login</button></div><div id='regForm' style='display:none'><input id='nickname' placeholder='Nickname'><input id='remail' placeholder='Email'><input id='rpassword' type='password' placeholder='Password (min 6)'> <button class='btn btn-p' onclick='register()' style='width:100%;margin-top:8px'>Register</button></div><button class='btn btn-o' onclick='toggleAuth()' style='width:100%;margin-top:12px' id='authToggle'>Register</button><div id='error' style='color:var(--d);margin-top:8px;font-size:13px'></div></div><script>function setTok(t){localStorage.setItem('rayrag_token',t);document.cookie='rayrag_token='+t+';path=/;max-age=86400'};function login(){var e=document.getElementById('email').value;var p=document.getElementById('password').value;fetch('/api/v1/auth/login',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({email:e,password:p})}).then(r=>r.json()).then(d=>{d.code===0?(setTok(d.data.access_token),location.href='/dashboard'):document.getElementById('error').textContent=d.message})};function register(){var n=document.getElementById('nickname').value;var e=document.getElementById('remail').value;var p=document.getElementById('rpassword').value;fetch('/api/v1/user/register',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({nickname:n,email:e,password:p})}).then(r=>r.json()).then(function(d){if(d.code===0){loginWith(e,p)}else{document.getElementById('error').textContent=d.message}})};function loginWith(e,p){fetch('/api/v1/auth/login',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({email:e,password:p})}).then(r=>r.json()).then(d=>{d.code===0?(setTok(d.data.access_token),location.href='/dashboard'):document.getElementById('error').textContent=d.message})};function toggleAuth(){var r=document.getElementById('regForm').style.display==='none';document.getElementById('regForm').style.display=r?'block':'none';document.getElementById('loginForm').style.display=r?'none':'block';document.getElementById('authToggle').textContent=r?'Login':'Register'}</script>";
    Html(layout(t("Login", zh), b, zh))
}

pub async fn document_page(
    State(state): State<Arc<AppState>>,
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
    b.push_str(" <span id='cTotal' style='font-size:13px;color:var(--tm)'></span></h2><div id='cl'>");
    b.push_str(t("Loading...", zh));
    b.push_str("</div></div>");
    b.push_str("<script>var kbId='");
    b.push_str(&kb_id);
    b.push_str("';var docId='");
    b.push_str(&doc_id);
    b.push_str("';function chunkPage(d){window.cPage+=d;loadChunks()};function loadChunks(){fetch('/api/v1/datasets/'+kbId+'/documents/'+docId+'/chunks').then(r=>r.json()).then(d=>{var data=d.data||{};var chunks=data.chunks||[];var doc=data.doc||{};if(doc.name)document.getElementById('docTitle').textContent='📄 '+doc.name;if(doc.run)document.getElementById('docStatus').textContent=doc.run;document.getElementById('docChunks').textContent=chunks.length;document.getElementById('cTotal').textContent='('+chunks.length+')';var h='';window.cPage=0;window.cPageSize=20;var pageData=chunks;function renderPage(){var h='';var start=window.cPage*window.cPageSize;var end=Math.min(start+window.cPageSize,pageData.length);pageData.slice(start,end).forEach(function(c,i){h+=\"<div class='ri'><span class='rs'>#\"+(start+i+1)+\" &middot; pos \"+(c.position||0)+\" &middot; \"+(c.token_count||0)+\" tok</span><div class='rc' style='white-space:pre-wrap'>\"+(c.content||'').substring(0,800)+\"</div><button class='btn btn-o btn-sm' style='margin-top:6px' onclick=\"editChunk('\"+c.id+'\"','\"+(c.content||'').substring(0,2000).replace(/\'/g,'')+'\"')\">Edit</button></div>\"});var pn='<div style=margin-top:12px>';if(window.cPage>0)pn+='<button class=btn btn-o btn-sm onclick=chunkPage(-1)>← Prev</button> ';pn+='<span style=color:var(--tm);font-size:12px> Page '+(window.cPage+1)+' / '+Math.max(1,Math.ceil(pageData.length/window.cPageSize))+' ('+pageData.length+' chunks) </span>';if(end<pageData.length)pn+='<button class=btn btn-o btn-sm onclick=chunkPage(1)>Next →</button>';pn+='</div>';h+=pn;document.getElementById('cl').innerHTML=h||'<p style=color:var(--tm);text-align:center;padding:40px>No chunks</p>'})};var editingChunk='';function editChunk(cid,content){editingChunk=cid;document.getElementById('chunkContent').value=content;document.getElementById('chunkModal').classList.add('show')};function saveChunk(){var content=document.getElementById('chunkContent').value;fetch('/api/v1/datasets/'+kbId+'/documents/'+docId+'/chunks/'+editingChunk,{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({content:content})}).then(function(r){return r.json()}).then(function(d){document.getElementById('chunkModal').classList.remove('show');if(d.code===0){location.reload()}else{alert('Error: '+(d.message||'unknown'))}})};function reparse(){if(!confirm('Reparse this document?'))return;fetch('/api/v1/datasets/'+kbId+'/documents/'+docId+'/reparse',{method:'POST'}).then(()=>location.reload())};function delDoc(){if(!confirm('Delete this document?'))return;fetch('/api/v1/datasets/'+kbId+'/documents/'+docId,{method:'DELETE'}).then(()=>location.href='/dataset/'+kbId)};loadChunks()</script>");
    Html(layout("Document", &b, zh))
}

pub async fn skills_page(
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let catalog = crate::runtime::component_catalog();
    let mut by_category: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for (name, category) in &catalog {
        by_category.entry(category).or_default().push(name);
    }
    let mut b = String::new();
    b.push_str("<h1>");
    b.push_str(t("Skills", zh));
    b.push_str("</h1><p style='color:var(--tm)'>");
    b.push_str(t(
        "Available canvas components and built-in tools. Open an Agent to compose them into a workflow.",
        zh,
    ));
    b.push_str("</p><div class='card'><h2>");
    b.push_str(t("Component Catalog", zh));
    b.push_str(" <span style='font-size:13px;color:var(--tm)'>");
    b.push_str(&catalog.len().to_string());
    b.push_str("</span></h2>");
    for (category, names) in &by_category {
        b.push_str("<h3 style='margin-top:18px;text-transform:capitalize'>");
        b.push_str(category);
        b.push_str("</h3><div class='kb-grid'>");
        for name in names {
            let initial = name.chars().next().unwrap_or('C').to_uppercase().collect::<String>();
            b.push_str(&format!(
                "<div class='kb-card'><div class='av-lg'>{initial}</div><h3>{name}</h3><p style='color:var(--tm);font-size:12px'>{}</p><a class='btn btn-o' style='margin-top:8px;font-size:12px;padding:4px 10px' href='/agents'>Use in Agent →</a></div>",
                t("canvas component", zh)
            ));
        }
        b.push_str("</div>");
    }
    b.push_str("</div><a class='btn btn-p' href='/agents' style='margin-top:16px'>");
    b.push_str(t("Open Agents", zh));
    b.push_str("</a>");
    Html(layout(t("Skills", zh), &b, zh))
}

pub async fn providers_page(
    State(state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let providers = state.providers.list();
    let mut rows = String::new();
    for p in &providers {
        rows.push_str(&format!(
            "<tr><td>{} {}</td><td class='co'>{}</td><td>{}</td></tr>",
            if p.enabled { "🟢" } else { "⚫" },
            p.name,
            p.api_base,
            p.models.join(", ")
        ));
    }
    let mut b = String::new();
    b.push_str("<h1>🤖 ");
    b.push_str(t("Providers", zh));
    b.push_str("</h1><div class='card'><table><thead><tr><th>Provider</th><th>API Base</th><th>Models</th></tr></thead><tbody>");
    b.push_str(&rows);
    b.push_str("</tbody></table></div>");
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
    Html(layout(t("Providers", zh), &b, zh))
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub async fn not_found_page() -> impl IntoResponse {
    let zh = false;
    let mut b = String::new();
    b.push_str("<div style='text-align:center;padding:80px 20px'><div style='font-size:72px;font-weight:800;color:var(--p)'>404</div><h1 style='margin:12px 0'>");
    b.push_str(t("Page not found", zh));
    b.push_str("</h1><p style='color:var(--tm);margin-bottom:24px'>");
    b.push_str(t("The page you requested does not exist or has been moved.", zh));
    b.push_str("</p><a class='btn btn-p' href='/dashboard'>← ");
    b.push_str(t("Back to Dashboard", zh));
    b.push_str("</a></div>");
    Html(layout("404", &b, zh))
}

pub async fn settings_page(req: axum::http::Request<axum::body::Body>) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let mut b = String::new();
    b.push_str("<h1>⚙️ ");
    b.push_str(t("Settings", zh));
    b.push_str("</h1><div class='card'><h2>System</h2><table><tr><td>Version</td><td class='co'>v");
    b.push_str(env!("CARGO_PKG_VERSION"));
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
    b.push_str("<div class='card'><h2>API Keys</h2><p style='color:var(--tm);margin-bottom:12px'>Generate.</p><button class='btn btn-p' onclick='genKey()'>Generate</button><div id='ak' style='margin-top:12px;background:var(--bg);padding:12px;border-radius:var(--r);font-family:monospace;font-size:13px;display:none'></div></div>");
    b.push_str("<script>function changePw(){var c=document.getElementById('curPw').value;var n=document.getElementById('newPw').value;if(!c||!n){document.getElementById('pwMsg').textContent='Please fill both fields';return}fetch('/api/v1/user/password',{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({current_password:c,new_password:n})}).then(r=>r.json()).then(d=>{document.getElementById('pwMsg').textContent=d.code===0?'Password updated':'Error: '+(d.message||'unknown')})};function genKey(){fetch('/api/v1/system/new_api_key',{method:'POST'}).then(r=>r.json()).then(d=>{var e=document.getElementById('ak');e.style.display='block';e.textContent=d.data.api_key})}</script>");
    Html(layout(t("Settings", zh), &b, zh))
}

pub async fn api_docs(req: axum::http::Request<axum::body::Body>) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let b = "<h1>📖 API Docs</h1><div class='card'><p style='color:var(--t2)'>45+ RESTful endpoints. Full RAG pipeline.</p><h3 style='margin-top:16px'>Auth & Users</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>POST</td><td class='co'>/api/v1/auth/login</td><td>Login</td></tr><tr><td>POST</td><td class='co'>/api/v1/user/register</td><td>Register</td></tr><tr><td>PATCH</td><td class='co'>/api/v1/user/password</td><td>Change password</td></tr><tr><td>DELETE/PATCH</td><td class='co'>/api/v1/admin/users/{id}</td><td>Delete user / reset password</td></tr></table><h3>Knowledge Bases</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>GET/POST</td><td class='co'>/api/v1/datasets</td><td>KB list/create</td></tr><tr><td>PUT/DELETE</td><td class='co'>/api/v1/datasets/{id}</td><td>Update config / delete</td></tr><tr><td>POST</td><td class='co'>/api/v1/datasets/{id}/documents</td><td>Upload document</td></tr><tr><td>GET/DELETE/PATCH</td><td class='co'>/api/v1/datasets/{id}/documents/{did}</td><td>Get / delete / rename</td></tr><tr><td>POST</td><td class='co'>/api/v1/datasets/{id}/documents/{did}/reparse</td><td>Reparse</td></tr><tr><td>POST</td><td class='co'>/api/v1/datasets/{id}/graphrag/build</td><td>Build knowledge graph</td></tr><tr><td>POST</td><td class='co'>/api/v1/datasets/{id}/graphrag/query</td><td>Query graph context</td></tr></table><h3>Search & Chat</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>POST</td><td class='co'>/api/v1/retrieval</td><td>Vector/hybrid search</td></tr><tr><td>POST</td><td class='co'>/api/v1/completions</td><td>RAG chat</td></tr><tr><td>POST</td><td class='co'>/api/v1/chats/{id}/completions</td><td>RAG chat (history, SSE)</td></tr><tr><td>DELETE</td><td class='co'>/api/v1/conversations/{id}</td><td>Delete conversation</td></tr><tr><td>PATCH</td><td class='co'>/api/v1/conversations/{id}</td><td>Rename conversation</td></tr><tr><td>POST</td><td class='co'>/api/v1/conversations/{id}/clear</td><td>Clear messages</td></tr><tr><td>GET</td><td class='co'>/api/v1/conversations/{id}/export</td><td>Export Markdown</td></tr></table><h3>OpenAI-Compatible</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>POST</td><td class='co'>/api/v1/chat/completions</td><td>Chat (stream + non-stream)</td></tr><tr><td>POST</td><td class='co'>/api/v1/embeddings</td><td>Embeddings</td></tr><tr><td>POST</td><td class='co'>/api/v1/rerank</td><td>Rerank (Jina format)</td></tr><tr><td>GET</td><td class='co'>/api/v1/models</td><td>Model list</td></tr></table><h3>Files & Memories</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>GET/DELETE</td><td class='co'>/api/v1/files/{id}</td><td>Get / delete file</td></tr><tr><td>GET</td><td class='co'>/api/v1/files/{id}/download</td><td>Download file</td></tr><tr><td>DELETE</td><td class='co'>/api/v1/memories/clear</td><td>Clear all conversations</td></tr></table><h3>Agents & System</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>GET/POST</td><td class='co'>/api/v1/agents</td><td>Agents CRUD</td></tr><tr><td>POST</td><td class='co'>/api/v1/agents/{id}/completions</td><td>Run agent</td></tr><tr><td>GET</td><td class='co'>/api/v1/stats</td><td>Stats</td></tr><tr><td>GET</td><td class='co'>/api/v1/system/healthz</td><td>Health check</td></tr></table></div>" ;
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
