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
        "Add Memory" => "添加记忆",
        "Clear All" => "清空全部",
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
        let name_js = kb.name.replace('&', "&amp;").replace('\'', "&#39;").replace('"', "&quot;");
        let desc_js = kb.description.replace('&', "&amp;").replace('\'', "&#39;").replace('"', "&quot;");
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
    b.push_str("' style='margin-top:8px'><select id='kbPerm' style='margin-top:8px'><option value=''>");
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
    b.push_str("</option><option value='default'>default</option></select><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='createKb()'>");
    b.push_str(t("Create", zh));
    b.push_str("</button><button class='btn btn-o' onclick='hideCreate()'>Cancel</button></div></div></div>");
    // Rename modal
    b.push_str("<div class='modal-overlay' id='renameModal'><div class='modal'><h2>");
    b.push_str(t("Rename", zh));
    b.push_str("</h2><input id='rnName'><input id='rnDesc' style='margin-top:8px'><input type='hidden' id='rnId'><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='renameKb()'>");
    b.push_str(t("Save", zh));
    b.push_str("</button><button class='btn btn-o' onclick='document.getElementById(\"renameModal\").classList.remove(\"show\")'>Cancel</button></div></div></div>");
    b.push_str("<script>function showCreate(){document.getElementById('createModal').classList.add('show')};function hideCreate(){document.getElementById('createModal').classList.remove('show')};async function createKb(){var n=document.getElementById('kbName').value;if(!n)return;var d=document.getElementById('kbDesc').value;var em=document.getElementById('kbEmbd');var p=document.getElementById('kbPerm');var body={name:n,description:d};if(em&&em.value)body.embedding_model=em.value;if(p&&p.value)body.permission=p.value;await fetch('/api/v1/datasets',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});location.reload()};");
    b.push_str("function kbFilter(){var q=(document.getElementById('kbSearch').value||'').toLowerCase();document.querySelectorAll('#kbGrid .kb-card').forEach(function(c){var n=(c.getAttribute('data-name')||'').toLowerCase();c.style.display=(!q||n.indexOf(q)>=0)?'':'none'})};");
    b.push_str("function kbSort(){var s=document.getElementById('kbSort').value;var grid=document.getElementById('kbGrid');var arr=Array.prototype.slice.call(grid.querySelectorAll('.kb-card'));arr.sort(function(a,b){if(s==='created')return (parseInt(b.getAttribute('data-created'),10)||0)-(parseInt(a.getAttribute('data-created'),10)||0);return (a.getAttribute('data-name')||'').localeCompare(b.getAttribute('data-name')||'')});arr.forEach(function(c){grid.appendChild(c)})};");
    b.push_str("function toggleKbMenu(btn){var id=btn.getAttribute('data-id');var m=document.getElementById('km-'+id);if(!m)return;var open=m.style.display==='block';document.querySelectorAll('.kb-menu').forEach(function(x){x.style.display='none'});m.style.display=open?'none':'block'};function closeKbMenus(){document.querySelectorAll('.kb-menu').forEach(function(x){x.style.display='none'})};document.addEventListener('click',function(e){if(!e.target.closest('.more'))closeKbMenus()});");
    b.push_str("function kbRenameOpen(btn){document.getElementById('rnId').value=btn.getAttribute('data-id');document.getElementById('rnName').value=btn.getAttribute('data-name')||'';document.getElementById('rnDesc').value=btn.getAttribute('data-desc')||'';document.getElementById('renameModal').classList.add('show')};function renameKb(){var id=document.getElementById('rnId').value;var n=document.getElementById('rnName').value;var d=document.getElementById('rnDesc').value;if(!n){alert('Name is required');return}fetch('/api/v1/datasets/'+id,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n,description:d,canvas_category:document.querySelector('input[name=agentType]:checked')?document.querySelector('input[name=agentType]:checked').value:'agent'})}).then(function(r){return r.json()}).then(function(res){if(res.code===0){location.reload()}else{alert('Rename failed: '+(res.message||'unknown'))}})};");
    b.push_str("function delKb(btn){var id=btn.getAttribute('data-id');var name=btn.getAttribute('data-name')||'';if(!confirm('Delete knowledge base '+name+'? This will remove all its documents and chunks.'))return;fetch('/api/v1/datasets/'+id,{method:'DELETE'}).then(function(){location.reload()})};");
    b.push_str("function exportKb(btn){var id=btn.getAttribute('data-id');fetch('/api/v1/datasets/'+id).then(function(r){return r.json()}).then(function(d){if(d.code!==0){alert('Export failed: '+(d.message||'unknown'));return}var kb=d.data||{};return fetch('/api/v1/datasets/'+id+'/documents').then(function(r){return r.json()}).then(function(dd){var docs=dd.data||[];return Promise.all(docs.map(function(doc){return fetch('/api/v1/datasets/'+id+'/documents/'+doc.id+'/chunks').then(function(r){return r.json()}).then(function(cd){doc.chunks=(cd.data||[]);return doc}).catch(function(){doc.chunks=[];return doc})}))}).then(function(docs){var out={id:kb.id,name:kb.name,description:kb.description,permission:kb.permission,embd_id:kb.embd_id,parser_config:kb.parser_config,chunk_count:kb.chunk_count,doc_count:kb.doc_count,exported_at:new Date().toISOString(),documents:docs};var blob=new Blob([JSON.stringify(out,null,2)],{type:'application/json'});var a=document.createElement('a');a.href=URL.createObjectURL(blob);a.download=(kb.name||'kb')+'.json';document.body.appendChild(a);a.click();setTimeout(function(){URL.revokeObjectURL(a.href);a.remove()},100)})}).catch(function(e){alert('Export failed: '+e)})};");
    b.push_str("</script>");
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
    b.push_str("</button><button class='btn btn-o btn-sm' onclick='docSelAll(false);updateBatchBar()'>✕ ");
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
    b.push_str(t("Embedding Model", zh));
    b.push_str("</td><td><select id='embdSel' style='max-width:280px'><option value=''>default</option></select></td></tr>");
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
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='cfgGraph'> ");
    b.push_str(t("Enable GraphRAG", zh));
    b.push_str("</label></td></tr>");
    b.push_str("<tr><td colspan=2 style='padding-top:16px;border-top:1px solid var(--bd)'><h3 style='font-size:14px;margin:0'>");
    b.push_str(t("Retrieval Configuration", zh));
    b.push_str("</h3></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Top K", zh));
    b.push_str("</td><td><input type='number' id='cfgTopK' min='1' max='100' value='10' style='max-width:120px'></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Similarity Threshold", zh));
    b.push_str(": <span id='cfgThrV' class='co'>0.2</span></td><td><input type='range' id='cfgThreshold' min='0' max='1' step='0.05' value='0.2' style='width:100%;max-width:280px' oninput='document.getElementById(\\\"cfgThrV\\\").textContent=this.value'></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Vector Weight", zh));
    b.push_str(": <span id='cfgVecV' class='co'>0.3</span></td><td><input type='range' id='cfgVector' min='0' max='1' step='0.05' value='0.3' style='width:100%;max-width:280px' oninput='document.getElementById(\\\"cfgVecV\\\").textContent=this.value'></td></tr>");
    b.push_str("</table><div style='margin-top:16px'><button class='btn btn-p' onclick='saveConfig()'>");
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
    b.push_str(t("Top K", zh));
    b.push_str("</label><select id='tk'><option>5</option><option selected>10</option><option>20</option></select><label style='font-size:12px;display:block;margin-top:8px'>");
    b.push_str(t("Similarity Threshold", zh));
    b.push_str(": <span id='ttV' class='co'>0.2</span></label><input type='range' id='tt' min='0' max='1' step='0.05' value='0.2' style='width:100%' oninput='document.getElementById(\\\"ttV\\\").textContent=this.value'><label style='font-size:12px;display:block;margin-top:8px'>");
    b.push_str(t("Vector Weight", zh));
    b.push_str(": <span id='twV' class='co'>0.3</span></label><input type='range' id='tw' min='0' max='1' step='0.05' value='0.3' style='width:100%' oninput='document.getElementById(\\\"twV\\\").textContent=this.value'><label style='font-size:12px;display:block;margin-top:8px'><input type='checkbox' id='tr'> ");
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
    b.push_str("';var pollTimer=null;var disKey='rayrag_disabled_docs';function getDisabled(){try{return JSON.parse(localStorage.getItem(disKey)||'{}')}catch(e){return{}}}function isDocDisabled(id){var m=getDisabled();return !!(m[kbId]&&m[kbId][id])}function setDocDisabled(id,on){var m=getDisabled();if(!m[kbId])m[kbId]={};if(on){m[kbId][id]=1}else{delete m[kbId][id]}try{localStorage.setItem(disKey,JSON.stringify(m))}catch(e){}}function showTab(n){for(var i=0;i<3;i++){var t=document.getElementById('tab'+i);var nav=document.getElementById('nav'+i);if(t)t.style.display='none';if(nav)nav.classList.remove('active')}document.getElementById('tab'+n).style.display='block';document.getElementById('nav'+n).classList.add('active')};");
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
    b.push_str("function loadConfig(){fetch('/api/v1/datasets/'+kbId).then(r=>r.json()).then(function(d){if(d.code!==0)return;var kb=d.data||{};var nm=document.getElementById('cfgName');if(nm)nm.value=kb.name||'';var pm=document.getElementById('cfgPerm');if(pm)pm.value=(kb.permission==='team')?'team':'private';var em=document.getElementById('embdSel');if(em&&kb.embd_id){for(var i=0;i<em.options.length;i++){if(em.options[i].value===kb.embd_id){em.selectedIndex=i;break}}}var pc={};try{pc=JSON.parse(kb.parser_config||'{}')}catch(e){}if(pc.chunk_method)document.getElementById('cfgMethod').value=pc.chunk_method;var tn=parseInt(pc.chunk_token_num,10);if(!isNaN(tn)&&tn>0)document.getElementById('cfgTokenNum').value=tn;var delims=Array.isArray(pc.delimiters)?pc.delimiters:(pc.delimiter?[pc.delimiter]:[]);document.getElementById('cfgDelims').value=delims.join('\\n');var op=parseFloat(pc.overlapped_percent);if(!isNaN(op))document.getElementById('cfgOverlap').value=Math.round(op*100);document.getElementById('cfgAutoKeywords').checked=!!pc.auto_keywords;document.getElementById('cfgAutoQuestions').checked=!!pc.auto_questions;document.getElementById('cfgChildren').checked=!!pc.enable_children;document.getElementById('cfgRerank').checked=!!(pc.rerank||pc.rerank_model);document.getElementById('cfgRaptor').checked=!!(pc.raptorEn||pc.raptor_enabled);document.getElementById('cfgGraph').checked=!!(pc.graphEn||pc.graphrag_enabled);var tk=parseInt(pc.top_k,10);if(!isNaN(tk)&&tk>0)document.getElementById('cfgTopK').value=tk;var th=parseFloat(pc.similarity_threshold);if(!isNaN(th)){document.getElementById('cfgThreshold').value=th;document.getElementById('cfgThrV').textContent=th}var vw=parseFloat(pc.vector_similarity_weight);if(!isNaN(vw)){document.getElementById('cfgVector').value=vw;document.getElementById('cfgVecV').textContent=vw}})};");
    b.push_str("function saveConfig(){var delims=document.getElementById('cfgDelims').value.split('\\n').map(function(s){return s.trim()}).filter(Boolean);if(!delims.length)delims=['\\n'];var body={name:document.getElementById('cfgName').value,permission:document.getElementById('cfgPerm').value,embd_id:document.getElementById('embdSel').value||'default',chunk_method:document.getElementById('cfgMethod').value,chunk_token_num:parseInt(document.getElementById('cfgTokenNum').value,10)||2048,delimiters:delims,overlapped_percent:Math.min(1,Math.max(0,(parseFloat(document.getElementById('cfgOverlap').value)||0)/100)),auto_keywords:document.getElementById('cfgAutoKeywords').checked,auto_questions:document.getElementById('cfgAutoQuestions').checked,enable_children:document.getElementById('cfgChildren').checked,rerank:document.getElementById('cfgRerank').checked,raptorEn:document.getElementById('cfgRaptor').checked,graphEn:document.getElementById('cfgGraph').checked,top_k:parseInt(document.getElementById('cfgTopK').value,10)||10,similarity_threshold:parseFloat(document.getElementById('cfgThreshold').value),vector_similarity_weight:parseFloat(document.getElementById('cfgVector').value)};var m=document.getElementById('cfgMsg');if(m){m.textContent='保存中…';m.style.color='var(--tm)'}fetch('/api/v1/datasets/'+kbId,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(r=>r.json()).then(function(d){if(m){if(d.code===0){m.textContent='✅ 已保存';m.style.color='var(--s)'}else{m.textContent='保存失败: '+(d.message||'未知错误');m.style.color='var(--d)'}}})};");
    b.push_str("function runTest(){var q=document.getElementById('tq').value;if(!q){alert('请输入测试问题');return}var k=parseInt(document.getElementById('tk').value,10);var t=parseFloat(document.getElementById('tt').value);var w=parseFloat(document.getElementById('tw').value);var r=document.getElementById('tr').checked;var body={question:q,kb_ids:[kbId],top_k:k,similarity_threshold:t,vector_similarity_weight:w};if(r)body.rerank=true;document.getElementById('testRes').innerHTML='<p style=color:var(--tm);text-align:center;padding:40px>检索中…</p>';fetch('/api/v1/retrieval',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(function(resp){return resp.json()}).then(function(d){if(d.code!==0){document.getElementById('testRes').innerHTML='<p style=color:var(--d)>检索失败: '+(d.message||'未知错误')+'</p>';return}var chunks=(d.data&&d.data.chunks)||[];var h='';chunks.forEach(function(c,i){h+='<div class=ri><span class=rs>#'+(i+1)+' · 相似度 '+(c.score||0).toFixed(4)+'</span><div class=rc>'+hl(c.content||'',q)+'</div><div class=rm>📄 '+(c.doc_name||'')+'</div></div>'});document.getElementById('testRes').innerHTML=h||'<p style=color:var(--tm);text-align:center;padding:40px>无匹配结果</p>'}).catch(function(e){document.getElementById('testRes').innerHTML='<p style=color:var(--d)>请求失败: '+e+'</p>'})};");
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
    b.push_str("</button><button class='btn btn-o btn-sm' onclick='docSelAll(false);updateBatchBar()'>✕ ");
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
    b.push_str("<div id='tab1' style='display:none'><div class='st'><div class='sc'><span class='lbl'>");
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
    b.push_str(t("Top K", zh));
    b.push_str("</label><select id='testK'><option>5</option><option selected>10</option><option>20</option></select><label style='font-size:12px;margin-top:8px;display:block'>");
    b.push_str(t("Similarity Threshold", zh));
    b.push_str(": <span id='testTV' class='co'>0.2</span></label><input type='range' id='testT' min='0' max='1' step='0.05' value='0.2' style='width:100%' oninput='document.getElementById(\\\"testTV\\\").textContent=this.value'><label style='font-size:12px;margin-top:8px;display:block'>");
    b.push_str(t("Vector Weight", zh));
    b.push_str(": <span id='testWV' class='co'>0.3</span></label><input type='range' id='testW' min='0' max='1' step='0.05' value='0.3' style='width:100%' oninput='document.getElementById(\\\"testWV\\\").textContent=this.value'><label style='font-size:12px;margin-top:8px;display:block'><input type='checkbox' id='testRerank'> ");
    b.push_str(t("Rerank", zh));
    b.push_str("</label><button class='btn btn-p' style='margin-top:12px;width:100%' onclick='testSearch()'>");
    b.push_str(t("Test", zh));
    b.push_str("</button></div><div><h3>");
    b.push_str(t("Results", zh));
    b.push_str("</h3><div id='testResults' style='min-height:300px'><p style='color:var(--tm);text-align:center;padding:40px'>");
    b.push_str(t("Results", zh));
    b.push_str(" ");
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
    b.push_str(t("Embedding Model", zh));
    b.push_str("</td><td><select id='embdSel' style='max-width:280px'><option value=''>default</option></select></td></tr>");
    b.push_str("<tr><td colspan=2 style='padding-top:16px;border-top:1px solid var(--bd)'><h3 style='font-size:14px;margin:0'>");
    b.push_str(t("Parsing Configuration", zh));
    b.push_str("</h3></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Chunk Method", zh));
    b.push_str("</td><td><select id='chunkMethod' onchange='toggleChunkOptions()'><option value='naive'>Naive (token-based)</option><option value='manual'>Manual</option><option value='qa'>Q&amp;A</option><option value='table'>Table</option><option value='paper'>Paper</option><option value='book'>Book</option><option value='laws'>Laws</option><option value='presentation'>Presentation</option><option value='one'>One</option><option value='resume'>Resume</option><option value='tag'>Tag</option><option value='knowledge_graph'>Knowledge Graph</option></select></td></tr>");
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
    b.push_str("<tr><td colspan=2 style='padding-top:16px;border-top:1px solid var(--bd)'><h3 style='font-size:14px;margin:0'>GraphRAG (Entity Extraction)</h3></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Enable GraphRAG", zh));
    b.push_str("</td><td><label style='display:flex;align-items:center;gap:6px'><input type='checkbox' id='graphEn'> ");
    b.push_str(t("Enable entity-relation extraction", zh));
    b.push_str("</label></td></tr>");
    b.push_str("<tr><td>Entity Types</td><td><input id='cfgEntityTypes' value='organization, person, location, event, concept' style='font-size:12px'></td></tr>");
    b.push_str("<tr><td>Method</td><td><select id='cfgGraphMethod'><option>Light</option><option>General</option></select></td></tr>");
    b.push_str("<tr><td colspan=2 style='padding-top:16px;border-top:1px solid var(--bd)'><h3 style='font-size:14px;margin:0'>");
    b.push_str(t("Retrieval Configuration", zh));
    b.push_str("</h3></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Top K", zh));
    b.push_str("</td><td><input type='number' id='cfgTopK' min='1' max='100' value='10' style='max-width:120px'></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Similarity Threshold", zh));
    b.push_str(": <span id='cfgThrV' class='co'>0.2</span></td><td><input type='range' id='cfgThreshold' min='0' max='1' step='0.05' value='0.2' style='width:100%;max-width:280px' oninput='document.getElementById(\\\"cfgThrV\\\").textContent=this.value'></td></tr>");
    b.push_str("<tr><td>");
    b.push_str(t("Vector Weight", zh));
    b.push_str(": <span id='cfgVecV' class='co'>0.3</span></td><td><input type='range' id='cfgVector' min='0' max='1' step='0.05' value='0.3' style='width:100%;max-width:280px' oninput='document.getElementById(\\\"cfgVecV\\\").textContent=this.value'></td></tr>");
    b.push_str("</table><div style='margin-top:16px'><button class='btn btn-p' onclick='saveConfig()'>");
    b.push_str(t("Save", zh));
    b.push_str("</button> <span id='cfgMsg' style='font-size:12px;color:var(--tm)'></span></div></div></div>");
    // Tab 4: Knowledge Graph
    b.push_str("<div id='tab4' style='display:none'><div class='card'><h2>");
    b.push_str(t("Knowledge Graph", zh));
    b.push_str("</h2><p style='color:var(--tm);margin-bottom:16px'>");
    b.push_str(t("Entity-relation graph extracted from indexed documents.", zh));
    b.push_str("</p><button class='btn btn-p' onclick='buildGraph()'>");
    b.push_str(t("Build", zh));
    b.push_str(" ");
    b.push_str(t("Graph", zh));
    b.push_str("</button><div id='graphResult' style='margin-top:16px;background:var(--bg);padding:20px;border-radius:var(--r);min-height:200px;font-family:monospace;font-size:12px;white-space:pre-wrap;line-height:1.8'><p style='color:var(--tm);text-align:center;padding:40px'>");
    b.push_str(t("Click Build Graph to extract entities and relations.", zh));
    b.push_str("</p></div></div></div>");
    b.push_str("</div></div>");
    // JS
    b.push_str("<script>var kbId='");
    b.push_str(&id);
    b.push_str("';var pollTimer=null;var disKey='rayrag_disabled_docs';function getDisabled(){try{return JSON.parse(localStorage.getItem(disKey)||'{}')}catch(e){return{}}}function isDocDisabled(id){var m=getDisabled();return !!(m[kbId]&&m[kbId][id])}function setDocDisabled(id,on){var m=getDisabled();if(!m[kbId])m[kbId]={};if(on){m[kbId][id]=1}else{delete m[kbId][id]}try{localStorage.setItem(disKey,JSON.stringify(m))}catch(e){}}function showTab(n){for(var i=0;i<5;i++){var t=document.getElementById('tab'+i);var nav=document.getElementById('nav'+i);if(t)t.style.display='none';if(nav)nav.classList.remove('active')}document.getElementById('tab'+n).style.display='block';document.getElementById('nav'+n).classList.add('active')};");
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
    b.push_str("function testSearch(){var q=document.getElementById('testQ').value;if(!q){alert('请输入测试问题');return}var k=parseInt(document.getElementById('testK').value,10);var t=parseFloat(document.getElementById('testT').value);var w=parseFloat(document.getElementById('testW').value);var r=document.getElementById('testRerank').checked;var body={question:q,kb_ids:[kbId],top_k:k,similarity_threshold:t,vector_similarity_weight:w};if(r)body.rerank=true;document.getElementById('testResults').innerHTML='<p style=color:var(--tm);text-align:center;padding:40px>检索中…</p>';fetch('/api/v1/retrieval',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(function(resp){return resp.json()}).then(function(d){if(d.code!==0){document.getElementById('testResults').innerHTML='<p style=color:var(--d)>检索失败: '+(d.message||'未知错误')+'</p>';return}var chunks=(d.data&&d.data.chunks)||[];var h='';chunks.forEach(function(c,i){h+='<div class=ri><span class=rs>#'+(i+1)+' · 相似度 '+(c.score||0).toFixed(4)+'</span><div class=rc>'+hl(c.content||'',q)+'</div><div class=rm>📄 '+(c.doc_name||'')+'</div></div>'});document.getElementById('testResults').innerHTML=h||'<p style=color:var(--tm);text-align:center;padding:40px>无匹配结果</p>'}).catch(function(e){document.getElementById('testResults').innerHTML='<p style=color:var(--d)>请求失败: '+e+'</p>'})};");
    b.push_str("function hl(t,q){if(!q)return t;try{return t.replace(new RegExp(q.replace(/[.*+?^${}()|[\\]\\\\]/g,'\\\\$&'),'gi'),function(m){return '<mark>'+m+'</mark>'})}catch(e){return t}};");
    b.push_str("function loadConfig(){fetch('/api/v1/datasets/'+kbId).then(function(r){return r.json()}).then(function(d){if(d.code!==0)return;var kb=d.data||{};var nm=document.getElementById('cfgName');if(nm)nm.value=kb.name||'';var pm=document.getElementById('cfgPerm');if(pm)pm.value=(kb.permission==='team')?'team':'private';var em=document.getElementById('embdSel');if(em&&kb.embd_id){for(var i=0;i<em.options.length;i++){if(em.options[i].value===kb.embd_id){em.selectedIndex=i;break}}}var pc={};try{pc=JSON.parse(kb.parser_config||'{}')}catch(e){}if(pc.chunk_method)document.getElementById('chunkMethod').value=pc.chunk_method;var cs=parseInt(pc.chunk_size,10);if(!isNaN(cs)&&cs>0){var csSel=document.getElementById('chunkSize');for(var j=0;j<csSel.options.length;j++){if(parseInt(csSel.options[j].value,10)===cs){csSel.selectedIndex=j;break}}}if(pc.overlap){var ov=document.getElementById('overlap');for(var k=0;k<ov.options.length;k++){if(ov.options[k].value===pc.overlap){ov.selectedIndex=k;break}}}if(pc.delimiter)document.getElementById('cfgDelim').value=pc.delimiter;if(pc.parser_type)document.getElementById('cfgParserType').value=pc.parser_type;document.getElementById('cfgAutoKeywords').checked=!!pc.auto_keywords;document.getElementById('cfgAutoQuestions').checked=!!pc.auto_questions;document.getElementById('cfgChildren').checked=!!pc.enable_children;document.getElementById('cfgRerank').checked=!!(pc.rerank||pc.rerank_model);document.getElementById('raptorEn').checked=!!(pc.raptorEn||pc.raptor_enabled);var rd=document.getElementById('raptorDepth');rd.disabled=!document.getElementById('raptorEn').checked;document.getElementById('graphEn').checked=!!(pc.graphEn||pc.graphrag_enabled);if(pc.entity_types)document.getElementById('cfgEntityTypes').value=pc.entity_types;var tk=parseInt(pc.top_k,10);if(!isNaN(tk)&&tk>0)document.getElementById('cfgTopK').value=tk;var th=parseFloat(pc.similarity_threshold);if(!isNaN(th)){document.getElementById('cfgThreshold').value=th;document.getElementById('cfgThrV').textContent=th}var vw=parseFloat(pc.vector_similarity_weight);if(!isNaN(vw)){document.getElementById('cfgVector').value=vw;document.getElementById('cfgVecV').textContent=vw}})};");
    b.push_str("function saveConfig(){var body={name:document.getElementById('cfgName').value,permission:document.getElementById('cfgPerm').value,embd_id:document.getElementById('embdSel').value||'default',chunk_method:document.getElementById('chunkMethod').value,chunk_size:parseInt(document.getElementById('chunkSize').value,10)||512,overlap:document.getElementById('overlap').value,delimiter:document.getElementById('cfgDelim').value,parser_type:document.getElementById('cfgParserType').value,language:document.getElementById('cfgLang').value,pdf_parser:document.getElementById('cfgPdfParser').value,page_index:document.getElementById('cfgPageIndex').checked,image_and_table_window:parseInt(document.getElementById('cfgImgTableWin').value,10)||5,excel_to_html:document.getElementById('cfgExcelHtml').checked,auto_keywords:document.getElementById('cfgAutoKeywords').checked,auto_questions:document.getElementById('cfgAutoQuestions').checked,enable_children:document.getElementById('cfgChildren').checked,rerank:document.getElementById('cfgRerank').checked,raptorEn:document.getElementById('raptorEn').checked,graphEn:document.getElementById('graphEn').checked,entity_types:document.getElementById('cfgEntityTypes').value,top_k:parseInt(document.getElementById('cfgTopK').value,10)||10,similarity_threshold:parseFloat(document.getElementById('cfgThreshold').value),vector_similarity_weight:parseFloat(document.getElementById('cfgVector').value)};var rd=document.getElementById('raptorDepth');if(document.getElementById('raptorEn').checked)body.raptor_depth=parseInt(rd.value,10)||5;var m=document.getElementById('cfgMsg');if(m){m.textContent='保存中…';m.style.color='var(--tm)'}fetch('/api/v1/datasets/'+kbId,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(function(r){return r.json()}).then(function(d){if(m){if(d.code===0){m.textContent='✅ 已保存';m.style.color='var(--s)'}else{m.textContent='保存失败: '+(d.message||'未知错误');m.style.color='var(--d)'}}})};");
    b.push_str("function toggleChunkOptions(){var m=document.getElementById('chunkMethod').value;var show=['naive','manual'].indexOf(m)>=0;document.getElementById('chunkSizeRow').style.display=show?'':'none';document.getElementById('overlapRow').style.display=show?'':'none'};document.getElementById('raptorEn').onchange=function(){document.getElementById('raptorDepth').disabled=!this.checked};");
    b.push_str("function buildGraph(){document.getElementById('graphResult').innerHTML='<p style=color:var(--tm)>Building... (LLM entity extraction)</p>';fetch('/api/v1/datasets/'+kbId+'/graphrag/build',{method:'POST'}).then(function(r){return r.json()}).then(function(d){if(d.code!==0){document.getElementById('graphResult').textContent='Error: '+(d.message||'unknown');return}var data=d.data||{};var nodes=data.node_list||[];var maxD=1;nodes.forEach(function(n){maxD=Math.max(maxD,n.degree||1)});var R=120;var svg='<svg width=640 height=460 style=background:var(--bg);border-radius:var(--r)><text x=320 y=30 text-anchor=middle font-size=14 fill=var(--t2)>'+data.nodes+' nodes / '+data.edges+' edges</text>';nodes.slice(0,24).forEach(function(n,i){var ang=2*Math.PI*i/nodes.slice(0,24).length;var cx=320+R*Math.cos(ang);var cy=240+R*Math.sin(ang);var r=(n.degree||1)/maxD*22+10;svg+='<line x1=320 y1=240 x2='+cx+' y2='+cy+' stroke=var(--p) stroke-opacity=0.35></line><circle cx='+cx+' cy='+cy+' r='+r+' fill=var(--p) fill-opacity=0.15 stroke=var(--p)></circle><text x='+cx+' y='+(cy+r+14)+' text-anchor=middle font-size=11 fill=var(--t2)>'+String(n.name||'').slice(0,14)+'</text>'});svg+='<circle cx=320 cy=240 r=26 fill=var(--p)></circle><text x=320 y=245 text-anchor=middle font-size=11 fill=var(--bg)>KB</text></svg>';var h='<div style=margin-bottom:8px>Nodes: '+data.nodes+' | Edges: '+data.edges+' | Documents: '+data.documents+' | Entities: '+data.entities+'</div>'+svg+'<div style=margin-top:10px;font-size:12px;color:var(--tm)>(环形图：节点大小=degree，中心=知识库)</div>';document.getElementById('graphResult').innerHTML=h}).catch(function(e){document.getElementById('graphResult').textContent='Error: '+e})};");
    b.push_str("fetch('/api/v1/providers').then(function(r){return r.json()}).then(function(d){var sel=document.getElementById('embdSel');if(!sel)return;(d.data||[]).forEach(function(p){if(!p.models)return;(p.models||[]).forEach(function(m){var o=document.createElement('option');o.value=m;o.textContent=m;sel.appendChild(o)})})}).catch(function(){});");
    b.push_str("loadDocs();loadConfig();</script>");
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
    b.push_str("</div><div class='sb'><select id='chatModel' onchange='window.chatModel=this.value' style='margin-right:8px;max-width:180px'><option value=''>Model: default</option></select><input id='chatInput' placeholder='Ask anything...' onkeydown='if(event.key===&quot;Enter&quot;)sendMsg()'><button class='btn btn-p' onclick='sendMsg()'>");
    b.push_str(t("Send", zh));
    b.push_str("</button><button class='btn btn-d' id='stopBtn' onclick='stopGen()' style='display:none'>⏹ ");
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
    b.push_str("document.getElementById('chatTemp').oninput=function(){document.getElementById('tempVal').textContent=this.value}");
    b.push_str("fetch('/api/v1/providers').then(r=>r.json()).then(function(d){var ps=d.data||[];var sel=document.getElementById('chatModel');if(!sel)return;(ps||[]).forEach(function(p){if(!p.models)return;(p.models||[]).forEach(function(m){var o=document.createElement('option');o.value=p.id+':'+m;o.textContent=m;sel.appendChild(o)})})}).catch(function(){});function stopGen(){if(window.__ac){window.__ac.abort();document.getElementById('stopBtn').style.display='none'}};function sendMsg(){var q=document.getElementById('chatInput').value;if(!q)return;var m=document.getElementById('chatMsgs');m.innerHTML+='<div class=cmu>'+q+'</div>';document.getElementById('chatInput').value='';var mi=msgIdx;var box=m.innerHTML+='';var ac=new AbortController();window.__ac=ac;var st=document.getElementById('stopBtn');if(st)st.style.display='inline-block';fetch('/api/v1/chats/'+sid+'/completions',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:q,stream:true}),signal:ac.signal}).then(function(r){var reader=r.body.getReader();var dec=new TextDecoder();var buf='';var acc='';var mid='';var refs=[];var el=document.createElement('div');el.className='cma';el.id='msg'+mi;el.innerHTML='<span style=color:var(--tm)>🤔 思考中…</span>';m.appendChild(el);m.scrollTop=m.scrollHeight;function pump(){return reader.read().then(function(x){if(x.done){if(st)st.style.display='none';el.innerHTML+=refsHtml(refs);el.innerHTML+='<div class=msg-actions><button class=btn-ghost btn-sm onclick=copyMsg('+mi+') title=Copy>📋</button><button class=btn-ghost btn-sm onclick=regenMsg('+mi+',&quot;'+mid+'&quot;) title=Regenerate>🔄</button><button class=btn-ghost btn-sm onclick=likeMsg(&quot;'+mid+'&quot;,1,this) title=Like>👍</button><button class=btn-ghost btn-sm onclick=likeMsg(&quot;'+mid+'&quot;,0,this) title=Dislike>👎</button></div></div>';m.scrollTop=m.scrollHeight;msgIdx++;return}buf+=dec.decode(x.value,{stream:true});var lines=buf.split('\n');buf=lines.pop();lines.forEach(function(line){if(line.indexOf('data:')!==0)return;var data=line.slice(5).trim();if(data==='[DONE]')return;try{var ev=JSON.parse(data);if(ev.object==='chat.completion.references'){refs=ev.references||refs;return}var c=ev.choices&&ev.choices[0]&&ev.choices[0].delta&&ev.choices[0].delta.content||'';if(c){if(acc===''){el.innerHTML=''}acc+=c;el.innerHTML=acc;m.scrollTop=m.scrollHeight}}catch(e){}});return pump()})}function refsHtml(refs){if(!refs||!refs.length)return '';var h='<details class=refs style=margin-top:6px><summary style=cursor:pointer;font-size:11px;color:var(--p)>📎 '+refs.length+' references</summary><div style=margin-top:6px>';refs.forEach(function(r,i){h+='<div class=cc>['+(i+1)+'] '+(r.content||'').substring(0,120)+(r.score?' <span style=color:var(--tm)>score '+(r.score).toFixed(4)+'</span>':'')+'</div>'});h+='</div></details>';return h}return pump()}).catch(function(e){if(st)st.style.display='none';if(e&&e.name==='AbortError'){el.innerHTML+='<span style=color:var(--tm)>⏹ Stopped</span>'}else{el.innerHTML='<span style=color:var(--err)>Error</span>'}})}");
    b.push_str("function copyMsg(i){var el=document.getElementById('msg'+i);if(el){var t=el.innerText.replace(/📋🔄👍👎/g,'').trim();navigator.clipboard.writeText(t);alert('Copied!')}}");
    b.push_str("function regenMsg(i,mid){var el=document.getElementById('msg'+i);if(!el||!mid)return;var old=el.innerHTML;el.innerHTML='<p style=color:var(--tm)>Regenerating...</p>';fetch('/api/v1/chats/'+sid+'/messages/'+mid+'/regenerate',{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'}).then(async function(r){var d=await r.json();if(!r.ok)throw new Error(d.message||'Regenerate failed');var a=d.data?d.data.answer:'...';var refs=(d.data&&d.data.citations||[]).map(function(c){return {content:c}});var rh=refsHtml(refs);el.innerHTML=a+rh+'<div class=msg-actions><button class=btn-ghost btn-sm onclick=copyMsg('+i+') title=Copy>📋</button><button class=btn-ghost btn-sm onclick=regenMsg('+i+',&quot;'+mid+'&quot;) title=Regenerate>🔄</button><button class=btn-ghost btn-sm onclick=likeMsg(&quot;'+mid+'&quot;,1,this) title=Like>👍</button><button class=btn-ghost btn-sm onclick=likeMsg(&quot;'+mid+'&quot;,0,this) title=Dislike>👎</button></div>'}).catch(function(e){el.innerHTML=old;alert(e.message)})}");
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
    b.push_str("<div id='searchView' style='display:none'><div style='display:flex;gap:16px'><div style='flex:1'><div class='card'><div class='sb'><input id='q2' placeholder='Search...' onkeydown='if(event.key===&quot;Enter&quot;)doSearch2()'><button class='btn btn-p' onclick='doSearch2()'>Search</button></div><div style='display:flex;gap:12px;margin-top:12px;align-items:center;flex-wrap:wrap'><label style='font-size:12px;display:flex;align-items:center;gap:4px'><input type='checkbox' id='rerank'> Re-rank</label><label style='font-size:12px;display:flex;align-items:center;gap:4px'><input type='checkbox' id='hybrid'> BM25+Vector</label><label style='font-size:12px'>");
    b.push_str(t("Knowledge Base", zh));
    b.push_str("</label><select id='kbSel' style='font-size:12px;max-width:200px' onchange='kbSelChange()'><option value='__all__'>All knowledge bases</option></select><label style='font-size:12px'>");
    b.push_str(t("Document Type", zh));
    b.push_str("</label><select id='docType' style='font-size:12px'><option value=''>All types</option><option value='pdf'>PDF</option><option value='doc'>DOC</option><option value='docx'>DOCX</option><option value='xls'>XLS</option><option value='xlsx'>XLSX</option><option value='ppt'>PPT</option><option value='pptx'>PPTX</option><option value='txt'>TXT</option><option value='md'>MD</option><option value='html'>HTML</option></select></div><div id='kbChips' style='margin-top:10px;display:flex;gap:6px;flex-wrap:wrap;font-size:12px'></div><button class='btn btn-o' style='font-size:12px;padding:4px 10px;margin-left:auto' onclick='exportResults()'>⬇ Export MD</button></div></div><div id='results' style='margin-top:16px'></div></div>");
    // Settings sidebar
    b.push_str("<div class='card' style='width:240px;flex-shrink:0'><h3>Search Settings</h3><label style='font-size:12px;display:block;margin:8px 0'>");
    b.push_str(t("Top K", zh));
    b.push_str("</label><select id='topK'><option>5</option><option selected>10</option><option>20</option><option>50</option></select><label style='font-size:12px;display:block;margin:8px 0'>");
    b.push_str(t("Threshold", zh));
    b.push_str("</label><select id='thresh'><option>0.0</option><option selected>0.3</option><option>0.5</option><option>0.7</option></select><label style='font-size:12px;display:block;margin:8px 0'>");
    b.push_str(t("Vector Weight", zh));
    b.push_str("</label><select id='vWeight'><option>0.3</option><option>0.5</option><option selected>0.7</option><option>1.0</option></select><h3 style='margin-top:20px'>Quick Questions</h3><a href='#' onclick='quickQ(\"What is RayRAG?\")' style='display:block;font-size:12px;color:var(--p);text-decoration:none;padding:4px 0'>What is RayRAG?</a><a href='#' onclick='quickQ(\"How does RAG work?\")' style='display:block;font-size:12px;color:var(--p);text-decoration:none;padding:4px 0'>How does RAG work?</a></div></div></div>");
    b.push_str("<script>var lastResults=[];var lastQuery='';function loadKbs(){fetch('/api/v1/datasets').then(r=>r.json()).then(d=>{var list=d.data||[];var chips='<label style=display:flex;align-items:center;gap:4px><input type=checkbox class=kbCk value=__all__ checked onchange=kbChipChange(this)> All</label>';var opts='<option value=__all__>All knowledge bases</option>';list.forEach(function(k){chips+='<label style=display:flex;align-items:center;gap:4px><input type=checkbox class=kbCk value='+k.id+' onchange=kbChipChange(this)> '+k.name+'</label>';opts+='<option value='+k.id+'>'+k.name+'</option>'});document.getElementById('kbChips').innerHTML=chips;document.getElementById('kbSel').innerHTML=opts})};function kbChipChange(c){if(c.value==='__all__'&&c.checked){document.querySelectorAll('.kbCk').forEach(function(x){x.checked=(x.value==='__all__')})}else{document.querySelectorAll('.kbCk').forEach(function(x){if(x.value==='__all__')x.checked=false})}};function kbSelChange(){var v=document.getElementById('kbSel').value;document.querySelectorAll('.kbCk').forEach(function(x){x.checked=(v==='__all__')?(x.value==='__all__'):(x.value===v)})};loadKbs();function hl(t,q){if(!q)return t;try{return t.replace(new RegExp(q.replace(/[.*+?^${}()|[\\\\]\\\\]/g,'\\\\$&'),'gi'),function(m){return '<mark>'+m+'</mark>'})}catch(e){return t}};function doSearch(){var q=document.getElementById('query').value;if(!q)return;document.getElementById('searchHome').style.display='none';document.getElementById('searchView').style.display='block';document.getElementById('q2').value=q;runSearch(q)};function doSearch2(){runSearch(document.getElementById('q2').value)};function quickQ(q){document.getElementById('searchHome').style.display='none';document.getElementById('searchView').style.display='block';document.getElementById('q2').value=q;runSearch(q)};function searchPage(d){window.sPage+=d;runSearch(lastQuery)};function docTypeOk(c){var t=document.getElementById('docType').value;if(!t)return true;var n=(c.doc_name||c.document_name||'').toLowerCase();return n.indexOf('.'+t)>0};function runSearch(q){var r=document.getElementById('rerank').checked;var h=document.getElementById('hybrid').checked;var k=parseInt(document.getElementById('topK').value);var t=parseFloat(document.getElementById('thresh').value);var e=r?'/api/v1/retrieval':'/api/v1/search';var body={question:q,top_k:k};var kbs=[];var allOn=false;document.querySelectorAll('.kbCk:checked').forEach(function(c){if(c.value==='__all__'){allOn=true}else{kbs.push(c.value)}});if(kbs.length&&!allOn){body.kb_ids=kbs}if(!r){body.similarity_threshold=t;body.vector_similarity_weight=parseFloat(document.getElementById('vWeight').value)};document.getElementById('results').innerHTML='<p style=color:var(--tm);padding:24px>Searching...</p>';fetch(e,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(r=>r.json()).then(d=>{var items=(d.data.chunks||d.data.results||[]).filter(docTypeOk);var h='';items.forEach(function(c,i){c=c.chunk||c;var content=(c.content||c.text||'').substring(0,500);var fullContent=(c.content||c.text||'');var docName=(c.doc_name||c.document_name||'Unknown');var score=(c.similarity||c.score||0).toFixed(4);h+='<div class=ri><span class=rs>#'+(i+1)+' &middot; '+score+(r?' &middot; reranked':'')+'</span><div class=rc>'+hl(content,q)+'</div><div class=rm>'+docName+' &middot; '+(c.chunk_id||c.id||'')+' <a href=# onclick=\\\"previewDoc(\\''+docName.replace(/'/g,'')+'\\',\\''+fullContent.replace(/'/g,'').substring(0,2000)+'\\')\\\" style=\\\"color:var(--p);font-size:11px;margin-left:8px\\\">📄 Preview</a></div></div>'});if(gc&&gc.length){h+='<div class=card style=margin-top:16px><h3>🔗 Graph Context</h3>'+gc.map(function(g,i){return '<div class=cc>['+(i+1)+'] '+g.substring(0,120)+'</div>'}).join('')+'</div>'}lastResults=items;lastQuery=q;document.getElementById('results').innerHTML=h||'<p style=color:var(--tm);padding:24px>No results found.</p>'})};function exportResults(){if(!lastResults.length){alert('No results to export');return}var md='# Search: '+lastQuery+'\\n\\n';lastResults.forEach(function(c,i){c=c.chunk||c;md+='## '+(i+1)+'. '+(c.doc_name||'Unknown')+' (score '+(c.similarity||c.score||0).toFixed(4)+')\\n\\n'+(c.content||c.text||'')+'\\n\\n'});var blob=new Blob([md],{type:'text/markdown'});var a=document.createElement('a');a.href=URL.createObjectURL(blob);a.download='search-export.md';a.click()};function previewDoc(name,content){var m=document.getElementById('docPreview');document.getElementById('docPreviewTitle').textContent=name;document.getElementById('docPreviewContent').textContent=content;m.classList.add('show')};function closePreview(){document.getElementById('docPreview').classList.remove('show')}</script>");
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
    b.push_str("';function editDsl(){var pre=document.getElementById('agentDsl');var ta=document.getElementById('agentDslEdit');ta.value=pre.textContent;ta.style.display='block';pre.style.display='none';ta.focus()};function saveDsl(){var ta=document.getElementById('agentDslEdit');var v;try{v=JSON.parse(ta.value)}catch(e){alert('Invalid JSON: '+e.message);return}fetch('/api/v1/agents/'+aid,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({dsl:v})}).then(r=>r.json()).then(function(d){if(d.code===0){loadAgent()}else{alert('Save failed: '+(d.message||'unknown'))}})};function loadAgent(){fetch('/api/v1/agents/'+aid).then(r=>r.json()).then(d=>{var a=d.data||{};document.getElementById('agentTitle').textContent='🤖 '+(a.name||'Agent');document.getElementById('agentId').textContent=a.id||'-';document.getElementById('agentKbs').textContent=(a.kb_ids||[]).join(', ')||'none';document.getElementById('agentStatus').textContent=a.canvas_category||'standard';document.getElementById('agentDsl').textContent=JSON.stringify(a.dsl||a.canvas||{},null,2);renderTopology(a.dsl||a.canvas||{})})};function renderTopology(dsl){var nodes=(dsl.nodes||[]);var edges=(dsl.edges||[]);if(!nodes.length){document.getElementById('canvasView').innerHTML='<p style=color:var(--tm)>Empty canvas</p>';return}var xMin=1e9,yMin=1e9,xMax=-1e9,yMax=-1e9;var pos={};nodes.forEach(function(n){var p=n.position||{x:0,y:0};pos[n.id]=p;xMin=Math.min(xMin,p.x);yMin=Math.min(yMin,p.y);xMax=Math.max(xMax,p.x);yMax=Math.max(yMax,p.y)});var W=xMax-xMin+300,H=yMax-yMin+300;var svg='<svg width='+W+' height='+H+' style=background:var(--bg)><defs><marker id=arr markerWidth=8 markerHeight=8 refX=8 refY=4 orient=auto><path d=M0,0L8,4L0,8 fill=var(--p)></path></marker></defs>';edges.forEach(function(e){var s=pos[e.source]||{x:0,y:0};var t=pos[e.target]||{x:0,y:0};svg+='<line x1='+(s.x-xMin+150)+' y1='+(s.y-yMin+150)+' x2='+(t.x-xMin+150)+' y2='+(t.y-yMin+150)+' stroke=var(--p) stroke-width=1.5 marker-end=url(#arr)></line>'});nodes.forEach(function(n){var p=pos[n.id];var label=n.data&&(n.data.title||n.data.name)||n.id;svg+='<g><rect x='+(p.x-xMin+120)+' y='+(p.y-yMin+130)+' width=150 height=40 rx=8 fill=var(--card) stroke=var(--p)></rect><text x='+(p.x-xMin+195)+' y='+(p.y-yMin+153)+' text-anchor=middle font-size=12 fill=var(--t2)>'+label+'</text></g>'});svg+='</svg>';document.getElementById('canvasView').innerHTML=svg};function logRun(q,status,out){var lg=document.getElementById('runLog');var ph=lg.querySelector('p');if(ph)ph.remove();var d=new Date();var ts=d.getHours()+':'+('0'+d.getMinutes()).slice(-2)+':'+('0'+d.getSeconds()).slice(-2);var div=document.createElement('div');div.style.cssText='padding:8px;border-bottom:1px solid var(--bd);white-space:pre-wrap';div.innerHTML='<span style=color:var(--tm)>['+ts+'] '+status+' &middot; Q: '+(q||'').substring(0,120)+'</span><br>'+String(out).substring(0,600);lg.insertBefore(div,lg.firstChild)};function runAgent(){document.getElementById('aa2').textContent='Running agent...';logRun('Run agent once','running','');fetch('/api/v1/agents/'+aid+'/completions',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:'Run agent once'})}).then(r=>r.json()).then(d=>{var out=d.data?d.data.answer:JSON.stringify(d);document.getElementById('aa2').textContent=out;logRun('Run agent once',d.code===0?'done':'error',out)})};function askAgent(){var q=document.getElementById('aq2').value;if(!q)return;document.getElementById('aa2').textContent='Running...';logRun(q,'running','');fetch('/api/v1/agents/'+aid+'/completions',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:q})}).then(r=>r.json()).then(d=>{var out=d.data?d.data.answer:JSON.stringify(d);document.getElementById('aa2').textContent=out;logRun(q,d.code===0?'done':'error',out)})};function saveAgent(){var ta=document.getElementById('agentDslEdit');var raw=ta.style.display==='none'?document.getElementById('agentDsl').textContent:ta.value;var v;try{v=JSON.parse(raw)}catch(e){alert('Invalid JSON: '+e.message);return}fetch('/api/v1/agents/'+aid,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({dsl:v})}).then(r=>r.json()).then(function(d){if(d.code===0){alert('Saved');loadAgent()}else{alert('Save failed: '+(d.message||'unknown'))}})};function publishAgent(){if(!confirm('Publish this agent as a released version?'))return;fetch('/api/v1/agents/'+aid,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({release:true})}).then(r=>r.json()).then(function(d){if(d.code===0){alert('Published');loadAgent()}else{alert('Publish failed: '+(d.message||'unknown'))}})};function addComponent(name){fetch('/api/v1/agents/'+aid).then(r=>r.json()).then(d=>{var a=d.data||{};var dsl=a.dsl||a.canvas||{};if(!dsl.components)dsl.components={};var n=0;var base=name.replace(/[^A-Za-z0-9_]/g,'');var key=base+':'+n;while(dsl.components[key]){n++;key=base+':'+n}var node={'obj':{'component_name':name,'params':{}},'downstream':[],'upstream':[]};if(dsl.components['begin']){dsl.components['begin'].downstream=dsl.components['begin'].downstream||[];dsl.components['begin'].downstream.push(key);node.upstream=['begin']}dsl.components[key]=node;var ta=document.getElementById('agentDslEdit');var pre=document.getElementById('agentDsl');ta.value=JSON.stringify(dsl,null,2);ta.style.display='block';pre.style.display='none';ta.focus();alert('Component \"'+name+'\" added to DSL. Click 💾 Save to persist.')})};function delAgent(){if(!confirm('Delete this agent?'))return;fetch('/api/v1/agents/'+aid,{method:'DELETE'}).then(function(){location.href='/agents'})};function openVersions(){document.getElementById('verList').innerHTML='<p style=color:var(--tm)>Loading...</p>';document.getElementById('verModal').classList.add('show');fetch('/api/v1/agents/'+aid+'/versions').then(r=>r.json()).then(function(d){var vs=d.data||[];if(!vs.length){document.getElementById('verList').innerHTML='<p style=color:var(--tm)>No versions yet</p>';return}var h='<table><thead><tr><th>ID</th><th>Title</th><th>Created</th><th></th></tr></thead><tbody>';vs.forEach(function(v){h+='<tr><td style=font-family:monospace;font-size:12px>'+v.id+'</td><td>'+(v.title||'')+'</td><td style=color:var(--tm);font-size:12px>'+new Date(v.created_at).toLocaleString()+'</td><td><button class=btn btn-o btn-sm onclick=downloadVersion(&quot;'+v.id+'&quot;)>Download</button></td></tr>'});document.getElementById('verList').innerHTML=h+'</tbody></table>'})};function downloadVersion(vid){fetch('/api/v1/agents/'+aid+'/versions/'+encodeURIComponent(vid)).then(r=>r.json()).then(function(d){var v=(d.data||{}).dsl||{};var a=document.createElement('a');a.href='data:application/json;charset=utf-8,'+encodeURIComponent(JSON.stringify(v,null,2));a.download='agent_'+aid+'_v'+vid+'.json';a.click()})};function exportAgentJson(){var ta=document.getElementById('agentDslEdit');var raw=ta.style.display==='none'?document.getElementById('agentDsl').textContent:ta.value;var a=document.createElement('a');a.href='data:application/json;charset=utf-8,'+encodeURIComponent(raw);a.download='agent_'+aid+'.json';a.click()};loadAgent()</script>");
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
    b.push_str("</h2><input id='aname' placeholder='Name'><textarea id='adesc' placeholder='Description' style='margin-top:8px;min-height:60px'></textarea><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='createA()'>");
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
function showTemplates(){var names=Object.keys(agentTemplates);var pick=prompt('Choose template:\\n'+names.map(function(k,i){return (i+1)+'. '+agentTemplates[k].name}).join('\n'));if(!pick)return;var idx=parseInt(pick)-1;if(isNaN(idx)||!names[idx]){alert('Invalid choice');return}var t=agentTemplates[names[idx]];var n=prompt('Agent name:',t.name);if(!n)return;fetch('/api/v1/agents',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n,description:(t.description?t.description:'Created from template: '+t.name),dsl:t.dsl})}).then(r=>r.json()).then(function(d){if(d.code===0){location.reload()}else{alert('Create failed: '+(d.message||'unknown'))}})};
var aid='default';function testA(id){aid=id};function showCreate(){document.getElementById('createModal').classList.add('show')};function createA(){var n=document.getElementById('aname').value;if(!n)return;var d=document.getElementById('adesc').value;fetch('/api/v1/agents',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n,description:d,canvas_category:document.querySelector('input[name=agentType]:checked')?document.querySelector('input[name=agentType]:checked').value:'agent'})}).then(function(){location.reload()})};function importAgentJson(){var f=document.getElementById('agentJsonFile').files[0];if(!f)return;var r=new FileReader();r.onload=function(){try{var obj=JSON.parse(r.result);var nm=obj.title||obj.name||'Imported Agent';fetch('/api/v1/agents',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:nm,description:obj.description||'',dsl:obj.dsl||obj.canvas||{}})}).then(function(res){return res.json()}).then(function(d){if(d.code===0){location.reload()}else{alert('Import failed: '+(d.message||'unknown'))}})}catch(e){alert('Invalid JSON: '+e.message)}};r.readAsText(f)};function editA(id){window.location.href='/agents/'+id};function runA(id){window.location.href='/agents/'+id};function renameA(id,cur){var n=prompt('New name:',cur);if(!n)return;fetch('/api/v1/agents/'+id,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n})}).then(r=>r.json()).then(d=>{if(d.code===0){location.reload()}else{alert('Rename failed: '+(d.message||'unknown'))}})};function dupA(id){fetch('/api/v1/agents/'+id).then(function(r){return r.json()}).then(function(d){var a=d.data||{};fetch('/api/v1/agents',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:(a.name||'agent')+' copy',description:a.description||'',kb_ids:a.kb_ids||[],dsl:a.dsl||{}})}).then(function(){location.reload()})})};function delA(id){if(!confirm('Delete agent?'))return;fetch('/api/v1/agents/'+id,{method:'DELETE'}).then(function(){location.reload()})};function askA(){var q=document.getElementById('aq').value;fetch('/api/v1/agents/'+aid+'/completions',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({question:q})}).then(r=>r.json()).then(d=>{document.getElementById('aa').textContent=d.data?d.data.answer:JSON.stringify(d)})}</script>");
    Html(layout(t("Agents", zh), &b, zh))
}

pub async fn datasources_page(
    req: axum::http::Request<axum::body::Body>,
) -> Result<axum::response::Html<String>, axum::response::Response> {
    let zh = crate::web::detect_lang(req.headers());
    let mut b = format!(
        "<h1>🔗 {}</h1>",
        if zh {
            "数据源（Data Sources）"
        } else {
            "Data Sources"
        }
    );
    // 类型选择 + 新建表单（RAGFlow 数据源页风格：类型 → 凭证 → 测试连接）
    b.push_str("<div class='card'><h2>");
    b.push_str(if zh { "新建数据源" } else { "Add Source" });
    b.push_str("</h2><div style='display:grid;grid-template-columns:1fr 1fr 1fr 1fr;gap:8px;margin-bottom:8px'>");
    b.push_str("<select id='dsType' onchange='dsHint()'><option value='gitlab'>GitLab</option><option value='blob'>Blob Storage</option><option value='local'>Local Files</option><option value='webdav'>WebDAV</option><option value='feishu'>飞书云空间</option><option value='confluence'>Confluence</option></select>");
    b.push_str("<input id='dsName' placeholder='Name'><input id='dsUrl' placeholder='URL'><input id='dsToken' type='password' placeholder='Token / app_id:app_secret'><input id='dsTarget' placeholder='Target (project/space/container)'>");
    b.push_str("</div><div id='dsHint' style='color:var(--t2);font-size:13px;margin-bottom:8px'></div><label><input type='checkbox' id='dsIssues'> Issues</label> <label><input type='checkbox' id='dsMRs'> Merge Requests</label> ");
    b.push_str("<button class='btn btn-p' onclick='addDs()'>+ Add</button></div>");
    b.push_str("<div class='card'><h2>Sources</h2><div style='margin-bottom:8px'><select id='dsKb'><option value=''>Sync to KB (optional)</option></select></div><div id='dsList'>Loading...</div></div>");
    b.push_str("<div class='card'><h2>");
    b.push_str(if zh { "文件列表 / 同步预览" } else { "Files / Sync Preview" });
    b.push_str("</h2><div id='dsPreview' style='color:var(--t2)'>Select a source, then List / Sync.</div></div><script>");
    b.push_str("function dsHint(){var m={gitlab:'url=https://gitlab.com, token=PAT, target=project id or owner/repo',blob:'url=https://acct.blob.core.windows.net, token=SAS, target=container',local:'url=absolute directory path, target=optional subdir',webdav:'url=https://webdav.example.com, token=user:password, target=/path',feishu:'token=app_id:app_secret, target=可选 folder_token',confluence:'url=https://xxx.atlassian.net/wiki, token=email:api_token, target=spaceKey'};document.getElementById('dsHint').textContent=m[document.getElementById('dsType').value]||'';}");
    b.push_str("function loadKbs(){fetch('/api/v1/datasets').then(function(r){return r.json()}).then(function(d){var kbs=d.data||[];var sel=document.getElementById('dsKb');var h='<option value=\'\'>Sync to KB (optional)</option>';kbs.forEach(function(k){h=h+'<option value=\'\"+k.id+\"\'>'+(k.name||k.id)+'</option>';});sel.innerHTML=h;});}");
    b.push_str("function loadDs(){fetch('/api/v1/data_sources').then(function(r){return r.json()}).then(function(d){var list=d.data||[];var h='<table><tr><th>Name</th><th>Type</th><th>URL</th><th>Target</th><th>State</th><th></th></tr>';list.forEach(function(x){var st=x.connected?'<span class=badge bs>connected</span>':'<span class=badge bw>disconnected</span>';h=h+'<tr><td>'+x.name+'</td><td>'+x.type+'</td><td>'+(x.url||'')+'</td><td>'+(x.target||'')+'</td><td>'+st+'</td><td><button class=btn-p onclick=connDs(\'\"+x.id+\"\')>Connect</button> <button class=btn-o onclick=discDs(\'\"+x.id+\"\')>Disconnect</button> <button class=btn-o onclick=listDs(\'\"+x.id+\"\')>List</button> <button class=btn-p onclick=syncDs(\'\"+x.id+\"\')>Sync</button> <button class=btn-d onclick=delDs(\'\"+x.id+\"\')>Del</button></td></tr>';});document.getElementById('dsList').innerHTML=h+'</table>';});}");
    b.push_str("function addDs(){var n=document.getElementById('dsName').value;var u=document.getElementById('dsUrl').value;var t=document.getElementById('dsToken').value;var g=document.getElementById('dsTarget').value;if(!n||!u||!g){alert('Name/URL/Target required');return;}var body={name:n,type:document.getElementById('dsType').value,url:u,token:t,target:g,include_issues:document.getElementById('dsIssues').checked,include_merge_requests:document.getElementById('dsMRs').checked};fetch('/api/v1/data_sources',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(function(r){return r.json()}).then(function(d){if(d.code===0){loadDs();document.getElementById('dsName').value='';document.getElementById('dsUrl').value='';document.getElementById('dsToken').value='';document.getElementById('dsTarget').value='';}else{alert(d.message||'create failed');}});}");
    b.push_str("function connDs(id){document.getElementById('dsPreview').textContent='Connecting...';fetch('/api/v1/data_sources/'+id+'/connect',{method:'POST'}).then(function(r){return r.json()}).then(function(d){document.getElementById('dsPreview').textContent=d.code===0?'Connected ✓':'Connect failed: '+(d.message||'unknown');loadDs();});}");
    b.push_str("function discDs(id){fetch('/api/v1/data_sources/'+id+'/disconnect',{method:'POST'}).then(function(r){return r.json()}).then(function(d){document.getElementById('dsPreview').textContent=d.code===0?'Disconnected':'Disconnect failed: '+(d.message||'unknown');loadDs();});}");
    b.push_str("function listDs(id){document.getElementById('dsPreview').textContent='Listing...';fetch('/api/v1/data_sources/'+id+'/files',{method:'POST'}).then(function(r){return r.json()}).then(function(d){if(d.code===0){var x=d.data||{};var h='<b>'+x.count+' files</b><table><tr><th>Name</th><th>Ext</th><th>Size</th><th>Updated</th></tr>';(x.files||[]).forEach(function(f){h=h+'<tr><td>'+(f.name||'')+'</td><td>'+(f.extension||'')+'</td><td>'+(f.size_bytes||0)+'</td><td>'+(f.updated_at||'')+'</td></tr>';});document.getElementById('dsPreview').innerHTML=h+'</table>';}else{document.getElementById('dsPreview').textContent='List failed: '+(d.message||'unknown');}});}");
    b.push_str("function syncDs(id){document.getElementById('dsPreview').textContent='Syncing...';var kb=document.getElementById('dsKb').value||'';var url='/api/v1/data_sources/'+id+'/sync'+(kb?('?kb_id='+encodeURIComponent(kb)):'');fetch(url,{method:'POST'}).then(function(r){return r.json()}).then(function(d){if(d.code===0){var x=d.data||{};var h='<b>'+x.count+' docs</b>'+(x.failures&&x.failures.length?' <span style=color:var(--d)>'+x.failures.length+' failures</span>':'')+'<table><tr><th>Title</th><th>Ext</th><th>Size</th><th>Type</th></tr>';(x.preview||[]).forEach(function(p){h=h+'<tr><td>'+(p.title||'')+'</td><td>'+(p.extension||'')+'</td><td>'+(p.size_bytes||0)+'</td><td>'+(p.type||'')+'</td></tr>';});document.getElementById('dsPreview').innerHTML=h+'</table>';}else{document.getElementById('dsPreview').textContent='Sync failed: '+(d.message||'unknown');}});}");
    b.push_str("function delDs(id){if(!confirm('Delete this data source?'))return;fetch('/api/v1/data_sources/'+id,{method:'DELETE'}).then(function(r){return r.json()}).then(function(d){if(d.code===0){loadDs();}else{alert(d.message||'delete failed');}});}dsHint();loadKbs();loadDs();</script>");
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
    b.push_str("</h2><input type='file' id='fu' style='max-width:400px'><button class='btn btn-p' onclick='uploadF()'>");
    b.push_str(t("Upload", zh));
    b.push_str("</button></div><div class='card' id='fileBatchBar' style='display:none'><div style='display:flex;align-items:center;gap:8px;flex-wrap:wrap'><span id='fileSelCount' style='font-size:13px;color:var(--tm)'></span><button class='btn btn-d btn-sm' onclick='batchDelSel()'>");
    b.push_str(t("Bulk Delete", zh));
    b.push_str("</button><button class='btn btn-o btn-sm' onclick='batchLinkSel()'>");
    b.push_str(t("Link to KB", zh));
    b.push_str("</button><button class='btn btn-o btn-sm' onclick='fileSelAll(false);updateFileBatch()'>✕ ");
    b.push_str(t("Clear", zh));
    b.push_str("</button></div></div><div class='card'><h2>Files</h2><input id='fileQ' placeholder='Search files...' oninput='fileFilter()' style='max-width:260px;margin-bottom:8px'><div id='fl'>Loading...</div></div><script>function fileFilter(){var q=(document.getElementById('fileQ').value||'').toLowerCase();document.querySelectorAll('#fl tr').forEach(function(tr){var m=tr.getAttribute('data-name')||'';tr.style.display=(!q||m.indexOf(q)>=0)?'':'none'})};function updateFileBatch(){var n=document.querySelectorAll('.fileCk:checked').length;var bar=document.getElementById('fileBatchBar');if(bar){bar.style.display=n?'block':'none';var sc=document.getElementById('fileSelCount');if(sc)sc.textContent=n+' selected'}};function fileSelAll(on){document.querySelectorAll('.fileCk').forEach(function(x){x.checked=on});updateFileBatch()};document.addEventListener('change',function(e){if(e.target&&e.target.classList&&e.target.classList.contains('fileCk'))updateFileBatch()});function loadF(){fetch('/api/v1/files').then(r=>r.json()).then(d=>{var files=d.data||[];var h='<table><thead><tr><th><input type=checkbox onclick=fileSelAll(this.checked)></th><th>Name</th><th>Size</th><th>Type</th><th>Uploaded</th><th></th></tr></thead><tbody>';files.forEach(function(f){var d=new Date(f.created_at);var ds=d?d.toLocaleString():'';h+='<tr data-name=\\''+f.name.toLowerCase()+'\\'><td><input type=checkbox class=fileCk value='+f.id+'></td><td style=font-weight:500>'+f.name+'</td><td>'+(f.size/1024).toFixed(0)+'KB</td><td class=co>'+f.file_type+'</td><td style=color:var(--tm);font-size:12px>'+ds+'</td><td><a class=btn btn-o btn-sm href=/api/v1/files/'+f.id+'/download target=_blank>Download</a> <button class=btn btn-o btn-sm onclick=linkFile('+f.id+') title=Link>🔗 KB</button> <button class=btn btn-o btn-sm onclick=renameF('+f.id+',&quot;'+f.name.replace(/\"/g,'')+'&quot;)>Rename</button> <button class=btn btn-o btn-sm onclick=copyF('+f.id+')>Copy</button> <button class=btn btn-d btn-sm onclick=delFile('+f.id+')>Del</button></td></tr>'});document.getElementById('fl').innerHTML=h+'</tbody></table>';updateFileBatch()})};function linkFile(id){var kb=prompt('Link file to knowledge base (dataset ID):');if(!kb)return;fetch('/api/v1/file2document',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({file_ids:[id],dataset_id:kb})}).then(r=>r.json()).then(d=>{alert(d.code===0?('Linked: '+(d.message||'ok')):('Error: '+(d.message||'unknown')))})};function selFileIds(){var ids=[];document.querySelectorAll('.fileCk:checked').forEach(function(x){ids.push(x.value)});return ids};function batchDelSel(){var ids=selFileIds();if(!ids.length){alert('请先选择文件');return}if(!confirm('Delete '+ids.length+' selected file(s)?'))return;var n=0;function next(){if(n>=ids.length){loadF();return}var id=ids[n++];fetch('/api/v1/files/'+id,{method:'DELETE'}).then(function(){next()}).catch(function(){next()})}next()};function batchLinkSel(){var ids=selFileIds();if(!ids.length){alert('请先选择文件');return}var kb=prompt('Link '+ids.length+' file(s) to knowledge base (dataset ID):');if(!kb)return;fetch('/api/v1/file2document',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({file_ids:ids,dataset_id:kb})}).then(r=>r.json()).then(d=>{alert(d.code===0?('Linked: '+(d.message||'ok')):('Error: '+(d.message||'unknown')))})};function renameF(id,cur){var n=prompt('New file name:',cur);if(!n)return;fetch('/api/v1/files/'+id,{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n})}).then(r=>r.json()).then(d=>{if(d.code===0){loadF()}else{alert('Error: '+(d.message||'unknown'))}})};function copyF(id){fetch('/api/v1/files/'+id+'/copy',{method:'POST'}).then(r=>r.json()).then(d=>{if(d.code===0){loadF()}else{alert('Error: '+(d.message||'unknown'))}})};function delFile(id){if(!confirm('Delete this file?'))return;fetch('/api/v1/files/'+id,{method:'DELETE'}).then(r=>r.json()).then(d=>{if(d.code===0){loadF()}else{alert('Error: '+(d.message||'unknown'))}})};function uploadF(){var f=document.getElementById('fu').files[0];if(!f)return;var fd=new FormData();fd.append('file',f,f.name);fetch('/api/v1/files/upload',{method:'POST',body:fd}).then(loadF)};loadF()</script>");
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
    b.push_str(t("Add Memory", zh));
    b.push_str("</button><button class='btn btn-d' onclick='clearAllMem()'>🗑 ");
    b.push_str(t("Clear All", zh));
    b.push_str("</button></div><div class='card'><h2>Memory Apps <span style='font-size:12px;color:var(--tm)' id='memCount'></span></h2><div id='memList'>Loading...</div></div>");
    // Add / edit memory modal (RAGFlow memory fields: name / memory_type / embd / llm)
    b.push_str("<div class='modal-overlay' id='memModal'><div class='modal'><h2 id='memModalTitle'>");
    b.push_str(t("Add Memory", zh));
    b.push_str("</h2><input id='mname' placeholder='Name'><label style='font-size:12px;display:block;margin:8px 0 4px'>Memory Type</label><div style='display:flex;gap:10px;flex-wrap:wrap;font-size:12px'><label><input type='checkbox' class='mtype' value='raw'> raw</label><label><input type='checkbox' class='mtype' value='semantic'> semantic</label><label><input type='checkbox' class='mtype' value='episodic'> episodic</label><label><input type='checkbox' class='mtype' value='procedural'> procedural</label></div><label style='font-size:12px;display:block;margin:8px 0 4px'>Embedding Model</label><select id='membd'><option value='default'>default</option></select><label style='font-size:12px;display:block;margin:8px 0 4px'>Chat Model</label><select id='mllm'><option value='default'>default</option></select><textarea id='mdesc' placeholder='Description' style='margin-top:8px;min-height:50px'></textarea><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='saveMem()'>");
    b.push_str(t("Save", zh));
    b.push_str("</button><button class='btn btn-o' onclick='closeMemModal()'>");
    b.push_str(t("Cancel", zh));
    b.push_str("</button></div></div></div>");
    b.push_str("<script>var memEditId=null;function showMemModal(){memEditId=null;document.getElementById('mname').value='';document.getElementById('mdesc').value='';document.querySelectorAll('.mtype').forEach(function(x){x.checked=false});document.getElementById('memModalTitle').textContent='Add Memory';document.getElementById('memModal').classList.add('show')};function closeMemModal(){document.getElementById('memModal').classList.remove('show')};function editMem(m){memEditId=m.id;document.getElementById('mname').value=m.name||'';document.getElementById('mdesc').value=m.description||'';document.querySelectorAll('.mtype').forEach(function(x){x.checked=(m.memory_type||[]).indexOf(x.value)>=0});if(m.embd_id)document.getElementById('membd').value=m.embd_id;if(m.llm_id)document.getElementById('mllm').value=m.llm_id;document.getElementById('memModalTitle').textContent='Edit Memory';document.getElementById('memModal').classList.add('show')};function saveMem(){var n=document.getElementById('mname').value;if(!n){alert('Name required');return}var types=[];document.querySelectorAll('.mtype:checked').forEach(function(x){types.push(x.value)});if(!types.length){types=['raw']}var body={name:n,memory_type:types,embd_id:document.getElementById('membd').value||'default',llm_id:document.getElementById('mllm').value||'default',description:document.getElementById('mdesc').value};var url=memEditId?'/api/v1/memories/'+memEditId:'/api/v1/memories';var method=memEditId?'PUT':'POST';fetch(url,{method:method,headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(r=>r.json()).then(d=>{if(d.code===0){closeMemModal();loadMems()}else{alert(d.message||'Save failed')}})};function delMem(id){if(!confirm('Delete this memory?'))return;fetch('/api/v1/memories/'+id,{method:'DELETE'}).then(r=>r.json()).then(d=>{if(d.code===0){loadMems()}else{alert(d.message||'Delete failed')}})};function clearAllMem(){if(!confirm('Delete ALL memories?'))return;fetch('/api/v1/memories/clear',{method:'DELETE'}).then(r=>r.json()).then(d=>{if(d.code===0){loadMems()}else{alert(d.message||'Clear failed')}})};function loadMems(){fetch('/api/v1/memories').then(r=>r.json()).then(d=>{var list=(d.data&&d.data.memory_list)||[];document.getElementById('memCount').textContent='('+list.length+')';var h='<table><thead><tr><th>Name</th><th>Type</th><th>Storage</th><th>Embedding</th><th>Chat</th><th>Size</th><th></th></tr></thead><tbody>';list.forEach(function(m){h+='<tr><td style=font-weight:500>'+(m.name||'')+'</td><td>'+(m.memory_type||[]).join(', ')+'</td><td class=co>'+(m.storage_type||'')+'</td><td class=co>'+(m.embd_id||'')+'</td><td class=co>'+(m.llm_id||'')+'</td><td>'+(m.memory_size||0)+'</td><td><button class=btn btn-o btn-sm onclick=\\'editMem('+JSON.stringify(m).replace(/'/g,'&#39;')+')\\' style=\\'font-size:12px\\'>✏️ Edit</button> <button class=btn btn-d btn-sm onclick=\\'delMem(\\\"'+m.id+'\\\")\\'>Del</button></td></tr>'});document.getElementById('memList').innerHTML=h+'</tbody></table>'||'<p style=color:var(--tm);padding:20px>No memories yet.</p>'})};function loadModelOpts(){fetch('/api/v1/providers').then(r=>r.json()).then(function(d){var ps=d.data||[];var e=document.getElementById('membd');var l=document.getElementById('mllm');(ps||[]).forEach(function(p){if(!p.models)return;(p.models||[]).forEach(function(m){var o=document.createElement('option');o.value=p.id+':'+m;o.textContent=m;e.appendChild(o);var o2=document.createElement('option');o2.value=p.id+':'+m;o2.textContent=m;l.appendChild(o2)})})}).catch(function(){})};loadMems();loadModelOpts();</script>");
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
    b.push_str("</button></div></div></div><script>function userFilter(){var q=(document.getElementById('userQ').value||'').toLowerCase();document.querySelectorAll('#userTb tr').forEach(function(tr){var m=tr.getAttribute('data-name')||'';var e=tr.getAttribute('data-email')||'';tr.style.display=(!q||m.indexOf(q)>=0||e.indexOf(q)>=0)?'':'none')})};function resetPw(id){var n=prompt('New password (min 6 chars):');if(!n)return;fetch('/api/v1/admin/users/'+id+'/password',{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({new_password:n})}).then(r=>r.json()).then(d=>{if(d.code===0){location.reload()}else{alert('Error: '+(d.message||'unknown'))}})};function delUser(id){if(!confirm('Delete this user?'))return;fetch('/api/v1/admin/users/'+id,{method:'DELETE'}).then(r=>r.json()).then(d=>{if(d.code===0){location.reload()}else{alert('Error: '+(d.message||'unknown'))}})};function showAddUser(){document.getElementById('addUserModal').classList.add('show')};function closeAddUser(){document.getElementById('addUserModal').classList.remove('show')};function addUser(){var u=document.getElementById('uname').value;var p=document.getElementById('upass').value;if(!u||!p){alert('Username and password required');return}var r=document.getElementById('urole').value;fetch('/api/v1/admin/users',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({username:u,password:p,role:r})}).then(r=>r.json()).then(d=>{if(d.code===0){location.reload()}else{alert('Error: '+(d.message||'unknown'))}})}</script>");
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

pub async fn skills_page(req: axum::http::Request<axum::body::Body>) -> impl IntoResponse {
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
            let initial = name
                .chars()
                .next()
                .unwrap_or('C')
                .to_uppercase()
                .collect::<String>();
            b.push_str(&format!(
                "<div class='kb-card'><div class='av-lg'>{initial}</div><h3>{name}</h3><p style='color:var(--tm);font-size:12px'>{}</p><div style='display:flex;gap:6px;margin-top:8px;flex-wrap:wrap'><button class='btn btn-p btn-sm' onclick='installSkill(&quot;{}&quot;)' >Install</button><button class='btn btn-d btn-sm' onclick='uninstallSkill(&quot;{}&quot;)' >Uninstall</button><a class='btn btn-o btn-sm' href='/agents'>Use in Agent →</a></div></div>",
                t("canvas component", zh),
                html_escape(name),
                html_escape(name)
            ));
        }
        b.push_str("</div>");
    }
    b.push_str("</div><a class='btn btn-p' href='/agents' style='margin-top:16px'>");
    b.push_str(t("Open Agents", zh));
    b.push_str("</a><script>function skillDsl(name){var key=name.replace(/[^A-Za-z0-9_]/g,'');var o={};o['begin']={'obj':{'component_name':'Begin','params':{}},'downstream':[key],'upstream':[]};o[key]={'obj':{'component_name':name,'params':{}},'downstream':[],'upstream':['begin']};return {'components':o}};function installSkill(name){var n=prompt('Agent name:',name+' Agent');if(!n)return;fetch('/api/v1/agents',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n,description:'Installed from skill: '+name,dsl:skillDsl(name)})}).then(r=>r.json()).then(d=>{if(d.code===0){location.href='/agents/'+(d.data&&d.data.id?d.data.id:'')}else{alert('Install failed: '+(d.message||'unknown'))}})};function uninstallSkill(name){fetch('/api/v1/agents').then(r=>r.json()).then(d=>{var list=(d.data||[]).filter(function(a){return a.description==='Installed from skill: '+name});if(!list.length){alert('No agents installed from this skill');return}if(!confirm('Uninstall skill? This deletes '+list.length+' agent(s) created from it.'))return;var done=0;list.forEach(function(a){fetch('/api/v1/agents/'+a.id,{method:'DELETE'}).then(function(){if(++done===list.length){location.reload()}})})})}</script>");
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
        let factory = crate::model_meta::resolve_model_factory(&p.id, &p.name);
        // Capability kinds (aligned with RAGFlow Model providers type filter).
        let mut kinds = Vec::new();
        use crate::providers::{provider_has_kind, ProviderKind};
        if provider_has_kind(&p.id, &p.name, ProviderKind::Chat) {
            kinds.push("llm");
        }
        if provider_has_kind(&p.id, &p.name, ProviderKind::Embedding) {
            kinds.push("embedding");
        }
        if provider_has_kind(&p.id, &p.name, ProviderKind::Rerank) {
            kinds.push("rerank");
        }
        if provider_has_kind(&p.id, &p.name, ProviderKind::TextToSpeech) {
            kinds.push("tts");
        }
        if provider_has_kind(&p.id, &p.name, ProviderKind::SpeechToText) {
            kinds.push("asr");
        }
        if provider_has_kind(&p.id, &p.name, ProviderKind::ImageToText) {
            kinds.push("vlm");
        }
        if provider_has_kind(&p.id, &p.name, ProviderKind::Ocr) {
            kinds.push("ocr");
        }
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
            p.name,
            if factory.is_some() {
                " <span style='font-size:11px;color:var(--tm)'>📦 catalog</span>"
            } else {
                ""
            },
            p.api_base,
            p.models.join(", ")
        ));
    }
    let mut b = String::new();
    b.push_str("<h1>🤖 ");
    b.push_str(t("Providers", zh));
    b.push_str("</h1><div class='card'><div style='display:flex;flex-wrap:wrap;gap:8px;align-items:center;margin-bottom:12px'><span id='pfCount' style='color:var(--tm);font-size:12px'></span><button class='btn btn-o btn-sm pfb on' data-f='all' onclick='pf(\"all\",this)'>All</button><button class='btn btn-o btn-sm pfb' data-f='llm' onclick='pf(\"llm\",this)'>LLM</button><button class='btn btn-o btn-sm pfb' data-f='embedding' onclick='pf(\"embedding\",this)'>Embedding</button><button class='btn btn-o btn-sm pfb' data-f='rerank' onclick='pf(\"rerank\",this)'>Rerank</button><button class='btn btn-o btn-sm pfb' data-f='tts' onclick='pf(\"tts\",this)'>TTS</button><button class='btn btn-o btn-sm pfb' data-f='asr' onclick='pf(\"asr\",this)'>ASR</button><button class='btn btn-o btn-sm pfb' data-f='vlm' onclick='pf(\"vlm\",this)'>VLM</button><button class='btn btn-o btn-sm pfb' data-f='ocr' onclick='pf(\"ocr\",this)'>OCR</button><span style='flex:1'></span><button class='btn btn-p btn-sm' onclick='showAddProv()'>+ Add</button></div><table><thead><tr><th>Provider</th><th>API Base</th><th>Models</th></tr></thead><tbody>");
    b.push_str(&rows);
    b.push_str("</tbody></table><p style='color:var(--tm);font-size:12px;margin-top:8px'>");
    b.push_str(t(
        "Click a provider row to browse its seeded model catalog (RAGFlow conf/models).",
        zh,
    ));
    b.push_str("</p></div>");
    {
        let summary = crate::model_meta::provider_catalog_summary();
        let total: usize = summary.iter().map(|(_, _, count)| count).sum();
        b.push_str("<div class='card'><h2>");
        b.push_str(t("Model Catalog", zh));
        b.push_str(" <span style='font-size:13px;color:var(--tm)'>");
        b.push_str(&summary.len().to_string());
        b.push_str(" ");
        b.push_str(t("providers", zh));
        b.push_str(" · ");
        b.push_str(&total.to_string());
        b.push_str(" ");
        b.push_str(t("models", zh));
        b.push_str("</span></h2><div class='kb-grid'>");
        for (factory, name, count) in &summary {
            b.push_str(&format!(
                "<div class='kb-card' data-factory='{}' onclick='showProviderModels(this)' style='cursor:pointer'><h3>{}</h3><p style='color:var(--tm);font-size:12px'>{count} {}</p></div>",
                html_escape(factory),
                html_escape(name),
                t("models", zh)
            ));
        }
        b.push_str("</div></div>");
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
    b.push_str("<div class='modal-overlay' id='modelModal' onclick='if(event.target===this)this.classList.remove(\"show\")'><div class='modal' style='max-width:720px'><h2 id='modelModalTitle'>");
    b.push_str(t("Models", zh));
    b.push_str("</h2><div id='modelModalBody' style='max-height:60vh;overflow:auto;margin-top:12px'></div><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-o' onclick='document.getElementById(\"modelModal\").classList.remove(\"show\")'>");
    b.push_str(t("Close", zh));
    b.push_str("</button></div></div></div><script>var providerModels=");
    b.push_str(&crate::model_meta::provider_models_payload().to_string());
    b.push_str(";function showProviderModels(el){var f=el.getAttribute('data-factory');var p=providerModels[f];if(!p)return;document.getElementById('modelModalTitle').textContent=p.name+' ('+f+')';var h='';if(!p.models||!p.models.length){h='<p style=color:var(--tm)>");
    b.push_str(t(
        "No static model list (models are discovered at runtime).",
        zh,
    ));
    b.push_str("</p>'}else{p.models.forEach(function(m){var tags=(m.t||[]).map(function(x){return '<span class=badge bs>'+x+'</span>'}).join(' ');h+='<div style=padding:8px 0;border-bottom:1px solid var(--bd);display:flex;align-items:center;gap:8px;flex-wrap:wrap><span style=font-weight:500>'+m.n+'</span>'+tags+'</div>'})}document.getElementById('modelModalBody').innerHTML=h;document.getElementById('modelModal').classList.add('show')}</script>");
    b.push_str("<div class='modal-overlay' id='addProvModal' onclick='if(event.target===this)this.classList.remove(\"show\")'><div class='modal' style='max-width:520px'><h2>+ Add Provider</h2><label style='display:block;margin-top:12px'>Provider ID<input id='apId' placeholder='e.g. deepseek' style='width:100%'></label><label style='display:block;margin-top:12px'>Name<input id='apName' placeholder='Display name' style='width:100%'></label><label style='display:block;margin-top:12px'>API Base<input id='apBase' placeholder='https://api.example.com/v1' style='width:100%'></label><label style='display:block;margin-top:12px'>API Key<input id='apKey' type='password' placeholder='Optional' style='width:100%'></label><label style='display:block;margin-top:12px'>Models (comma separated)<input id='apModels' placeholder='model-a, model-b' style='width:100%'></label><div style='display:flex;gap:8px;margin-top:16px'><button class='btn btn-p' onclick='saveAddProv()'>Save</button><button class='btn btn-o' onclick='document.getElementById(\"addProvModal\").classList.remove(\"show\")'>Cancel</button><span id='apStatus' style='margin-left:8px;color:var(--tm);font-size:12px'></span></div></div></div><script>function pf(k,btn){document.querySelectorAll('.pfb').forEach(function(x){x.classList.remove('on')});if(btn)btn.classList.add('on');document.querySelectorAll('tbody tr[data-k]').forEach(function(tr){var ks=(tr.getAttribute('data-k')||' ').split(' ');tr.style.display=(k==='all'||ks.indexOf(k)>=0)?'':'none'});var rows=document.querySelectorAll('tbody tr[data-k]');var n=0;rows.forEach(function(tr){if(tr.style.display!=='none')n++});var c=document.getElementById('pfCount');if(c)c.textContent=rows.length+' providers · '+n+' shown'};function showAddProv(){document.getElementById('apId').value='';document.getElementById('apName').value='';document.getElementById('apBase').value='';document.getElementById('apKey').value='';document.getElementById('apModels').value='';document.getElementById('apStatus').textContent='';document.getElementById('addProvModal').classList.add('show')};function saveAddProv(){var id=document.getElementById('apId').value.trim();var nm=document.getElementById('apName').value.trim();var base=document.getElementById('apBase').value.trim();var key=document.getElementById('apKey').value.trim();var ms=document.getElementById('apModels').value.split(',').map(function(x){return x.trim()}).filter(Boolean);if(!id||!nm){document.getElementById('apStatus').textContent='ID and Name required';return}var body={name:nm,api_base:base,api_key:key||null};if(ms.length)body.models=ms;var st=document.getElementById('apStatus');st.textContent='Saving…';fetch('/api/v1/providers/'+encodeURIComponent(id),{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(function(r){return r.json()}).then(function(d){if(d.code===0){st.textContent='Saved ✓';setTimeout(function(){location.reload()},500)}else{st.textContent=d.message||'Failed'}}).catch(function(e){st.textContent='Error: '+e});};pf('all',document.querySelector('.pfb'));</script>");
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
    b.push_str("<div class='card'><h2>");
    b.push_str(t("API Keys", zh));
    b.push_str("</h2><p style='color:var(--tm);margin-bottom:12px'>Generate and manage personal API keys.</p><button class='btn btn-p' onclick='genKey()'>Generate</button><div id='akList' style='margin-top:12px'></div></div>");
    b.push_str("<div class='card'><h2>");
    b.push_str(t("Invite", zh));
    b.push_str("</h2><p style='color:var(--tm);margin-bottom:12px'>Invite a registered user to your team.</p><div style='display:grid;gap:8px;max-width:360px'><input id='invEmail' placeholder='user@example.com'><button class='btn btn-p' onclick='inviteUser()'>");
    b.push_str(t("Invite", zh));
    b.push_str("</button><p id='invMsg' style='font-size:12px;color:var(--tm)'></p></div></div>");
    b.push_str("<script>function changePw(){var c=document.getElementById('curPw').value;var n=document.getElementById('newPw').value;if(!c||!n){document.getElementById('pwMsg').textContent='Please fill both fields';return}fetch('/api/v1/user/password',{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({current_password:c,new_password:n})}).then(r=>r.json()).then(d=>{document.getElementById('pwMsg').textContent=d.code===0?'Password updated':'Error: '+(d.message||'unknown')})};function loadKeys(){fetch('/api/v1/system/api_keys').then(r=>r.json()).then(d=>{var keys=d.data||[];var e=document.getElementById('akList');if(!keys.length){e.innerHTML='<p style=color:var(--tm)>No API keys yet.</p>';return}var h='<table><thead><tr><th>Key</th><th>Created</th><th></th></tr></thead><tbody>';keys.forEach(function(k){h+='<tr><td style=font-family:monospace;font-size:12px>'+k.token+'</td><td style=color:var(--tm);font-size:12px>'+k.create_date+'</td><td><button class=\\'btn btn-d btn-sm\\' onclick=\\'delKey(&quot;'+k.token+'&quot;)\\'>Delete</button></td></tr>'});e.innerHTML=h+'</tbody></table>'})};function genKey(){fetch('/api/v1/system/api_keys',{method:'POST'}).then(r=>r.json()).then(d=>{if(d.code===0){loadKeys()}else{alert('Error: '+(d.message||'unknown'))}})};function delKey(k){if(!confirm('Delete this API key?'))return;fetch('/api/v1/system/api_keys/'+encodeURIComponent(k),{method:'DELETE'}).then(r=>r.json()).then(d=>{if(d.code===0){loadKeys()}else{alert('Error: '+(d.message||'unknown'))}})};function inviteUser(){var e=document.getElementById('invEmail').value;if(!e){document.getElementById('invMsg').textContent='Please enter an email';return}fetch('/api/v1/users/invite',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({email:e})}).then(r=>r.json()).then(d=>{document.getElementById('invMsg').textContent=d.code===0?'Invitation sent':'Error: '+(d.message||'unknown')})};loadKeys()</script>");
    Html(layout(t("Settings", zh), &b, zh))
}

pub async fn api_docs(req: axum::http::Request<axum::body::Body>) -> impl IntoResponse {
    let zh = detect_lang(req.headers());
    let b = "<h1>📖 API Docs</h1><div class='card'><p style='color:var(--t2)'>45+ RESTful endpoints. Full RAG pipeline.</p><h3 style='margin-top:16px'>Auth & Users</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>POST</td><td class='co'>/api/v1/auth/login</td><td>Login</td></tr><tr><td>POST</td><td class='co'>/api/v1/user/register</td><td>Register</td></tr><tr><td>PATCH</td><td class='co'>/api/v1/user/password</td><td>Change password</td></tr><tr><td>DELETE/PATCH</td><td class='co'>/api/v1/admin/users/{id}</td><td>Delete user / reset password</td></tr></table><h3>Knowledge Bases</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>GET/POST</td><td class='co'>/api/v1/datasets</td><td>KB list/create</td></tr><tr><td>PUT/DELETE</td><td class='co'>/api/v1/datasets/{id}</td><td>Update config / delete</td></tr><tr><td>POST</td><td class='co'>/api/v1/datasets/{id}/documents</td><td>Upload document</td></tr><tr><td>GET/DELETE/PATCH</td><td class='co'>/api/v1/datasets/{id}/documents/{did}</td><td>Get / delete / rename</td></tr><tr><td>POST</td><td class='co'>/api/v1/datasets/{id}/documents/{did}/reparse</td><td>Reparse</td></tr><tr><td>POST</td><td class='co'>/api/v1/datasets/{id}/graphrag/build</td><td>Build knowledge graph</td></tr><tr><td>POST</td><td class='co'>/api/v1/datasets/{id}/graphrag/query</td><td>Query graph context</td></tr></table><h3>Search & Chat</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>POST</td><td class='co'>/api/v1/retrieval</td><td>Vector/hybrid search</td></tr><tr><td>POST</td><td class='co'>/api/v1/completions</td><td>RAG chat</td></tr><tr><td>POST</td><td class='co'>/api/v1/chats/{id}/completions</td><td>RAG chat (history, SSE)</td></tr><tr><td>DELETE</td><td class='co'>/api/v1/conversations/{id}</td><td>Delete conversation</td></tr><tr><td>PATCH</td><td class='co'>/api/v1/conversations/{id}</td><td>Rename conversation</td></tr><tr><td>POST</td><td class='co'>/api/v1/conversations/{id}/clear</td><td>Clear messages</td></tr><tr><td>GET</td><td class='co'>/api/v1/conversations/{id}/export</td><td>Export Markdown</td></tr></table><h3>OpenAI-Compatible</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>POST</td><td class='co'>/api/v1/chat/completions</td><td>Chat (stream + non-stream)</td></tr><tr><td>POST</td><td class='co'>/api/v1/embeddings</td><td>Embeddings</td></tr><tr><td>POST</td><td class='co'>/api/v1/rerank</td><td>Rerank (Jina format)</td></tr><tr><td>GET</td><td class='co'>/api/v1/models</td><td>Model list</td></tr></table><h3>Files & Memories</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>GET/DELETE</td><td class='co'>/api/v1/files/{id}</td><td>Get / delete file</td></tr><tr><td>GET</td><td class='co'>/api/v1/files/{id}/download</td><td>Download file</td></tr><tr><td>DELETE</td><td class='co'>/api/v1/memories/clear</td><td>Clear all conversations</td></tr></table><h3>Agents & System</h3><table><tr><th>Method</th><th>Path</th><th>Description</th></tr><tr><td>GET/POST</td><td class='co'>/api/v1/agents</td><td>Agents CRUD</td></tr><tr><td>POST</td><td class='co'>/api/v1/agents/{id}/completions</td><td>Run agent</td></tr><tr><td>GET</td><td class='co'>/api/v1/stats</td><td>Stats</td></tr><tr><td>GET</td><td class='co'>/api/v1/system/healthz</td><td>Health check</td></tr></table></div>";
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escape_escapes_special_chars() {
        assert_eq!(html_escape("a&b<c>d\"e'f"), "a&amp;b&lt;c&gt;d&quot;e&#39;f");
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
    use crate::api::document::DocStore;
    use crate::api::document_metadata::DocumentMetadataStore;
    use crate::api::evaluation::EvaluationStore;
    use crate::api::features::{
        AgentRunRegistry, AgentStore, CanvasVersionStore, MemoryStore,
        ProviderStore, TaskQueue,
    };
    use crate::api::file_mgr::FileStore;
    use crate::api::compilation_templates::CompilationTemplateStore;
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
        let users = Arc::new(
            UserStore::new(users_path.to_str().unwrap()).unwrap(),
        );
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
            index_path: root.path().join("index.json").to_string_lossy().into_owned(),
            model_path: root.path().join("models").to_string_lossy().into_owned(),
            max_upload_bytes: 64 * 1024 * 1024,
            docs: Arc::new(
                DocStore::new(root.path().join("docs.json").to_str().unwrap()).unwrap(),
            ),
            document_metadata: Arc::new(DocumentMetadataStore::in_memory()),
            files: Arc::new(
                FileStore::new(root.path().join("uploads").to_str().unwrap()).unwrap(),
            ),
            data_sources: None,
            providers: Arc::new(ProviderStore::in_memory()),
            tenant_models: Arc::new(TenantModelStore::in_memory()),
            memories: Arc::new(MemoryStore::in_memory()),
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
        });
        (root, state)
    }

    async fn body_of(resp: impl IntoResponse) -> String {
        let bytes = axum::body::to_bytes(resp.into_response().into_body(), 16 * 1024 * 1024)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn providers_page_renders_kind_filter_and_add_provider_modal() {
        let (_root, state) = test_state();
        let req = axum::http::Request::builder()
            .uri("/providers")
            .header("accept-language", "en")
            .body(axum::body::Body::empty())
            .unwrap();
        let body = body_of(providers_page(axum::extract::State(state), req).await).await;
        // RAGFlow-style capability filter buttons.
        for label in ["All", "LLM", "Embedding", "Rerank", "TTS", "ASR", "VLM", "OCR"] {
            assert!(body.contains(label), "missing filter button {label}");
        }
        // Rows carry data-k kind attributes for filtering.
        assert!(body.contains("data-k="), "rows must carry data-k kind attributes");
        // Add-provider modal + save wiring.
        assert!(body.contains("addProvModal"), "missing add-provider modal");
        assert!(body.contains("saveAddProv"), "missing add-provider save handler");
        assert!(body.contains("/api/v1/providers/"), "missing providers API wiring");
        assert!(body.contains("pf('all'"), "filter must initialize on All");
    }

    #[tokio::test]
    async fn kbs_page_renders_create_search_and_card_action_menu() {
        let (_root, state) = test_state();
        state
            .kbs
            .create_for("u1", "Test KB", "desc")
            .unwrap();
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
        assert!(html.contains("/api/v1/datasets"), "datasets API call missing");
    }

    #[tokio::test]
    async fn kb_detail_page_renders_document_batch_config_and_testing_buttons() {
        let (_root, state) = test_state();
        let kb = state.kbs.create_for("u1", "Detail KB", "d").unwrap();
        let req = Request::builder().uri("/kbs/x").body(Body::empty()).unwrap();
        let html = body_of(kb_detail_page(State(state.clone()), Path(kb.id.clone()), req).await).await;
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
        assert!(html.contains("cfgThreshold"), "similarity slider missing");
        assert!(html.contains("cfgVector"), "vector weight slider missing");
        assert!(html.contains("saveConfig"), "save button missing");
        // 检索测试 tab：输入框 / 检索按钮 / 相似度滑块 / 结果卡片
        assert!(html.contains("id='tq'"), "test query input missing");
        assert!(html.contains("runTest"), "test button missing");
        assert!(html.contains("id='tt'"), "similarity slider missing");
        assert!(html.contains("testRes"), "results area missing");
        assert!(html.contains("/api/v1/retrieval"), "retrieval API missing");
    }

    #[tokio::test]
    async fn dataset_page_renders_five_tabs_with_all_required_buttons() {
        let (_root, state) = test_state();
        let kb = state.kbs.create_for("u1", "Five Tab KB", "d").unwrap();
        let req = Request::builder().uri("/dataset/x").body(Body::empty()).unwrap();
        let html = body_of(dataset_page(State(state.clone()), Path(kb.id.clone()), req).await).await;
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
        let req = Request::builder().uri("/files").body(Body::empty()).unwrap();
        let html = body_of(files_page(State(state), req).await).await;
        // 批量操作栏 + 勾选 + 关联知识库（RAGFlow: 上传/新建文件夹/批量移动/批量关联）
        assert!(html.contains("fileBatchBar"), "bulk bar missing");
        assert!(html.contains("batchDelSel"), "bulk delete missing");
        assert!(html.contains("batchLinkSel"), "bulk link missing");
        assert!(html.contains("fileSelAll"), "select-all missing");
        assert!(html.contains("linkFile"), "link-to-KB handler missing");
        assert!(html.contains("/api/v1/file2document"), "file2document API missing");
    }

    #[tokio::test]
    async fn agents_page_renders_import_json_button() {
        let (_root, state) = test_state();
        let req = Request::builder().uri("/agents").body(Body::empty()).unwrap();
        let html = body_of(agents_page(State(state), req).await).await;
        // RAGFlow: 从空白创建 / 从模板创建 / 导入 JSON
        assert!(html.contains("agentJsonFile"), "import JSON file input missing");
        assert!(html.contains("importAgentJson"), "import JSON handler missing");
    }

    #[tokio::test]
    async fn agent_detail_page_renders_versions_and_export_json_buttons() {
        let (_root, state) = test_state();
        let req = Request::builder().uri("/agents/a1").body(Body::empty()).unwrap();
        let html = body_of(agent_detail_page(Path("a1".to_string()), req).await).await;
        // RAGFlow: 运行 / 探索 / 版本管理(发布生产+下载) / 导出 JSON
        assert!(html.contains("openVersions"), "versions button missing");
        assert!(html.contains("verModal"), "versions modal missing");
        assert!(html.contains("downloadVersion"), "version download handler missing");
        assert!(html.contains("exportAgentJson"), "export JSON handler missing");
        assert!(html.contains("/api/v1/agents/'+aid+'/versions"), "versions API missing");
    }

    #[tokio::test]
    async fn kb_detail_page_renders_empty_all_button() {
        let (_root, state) = test_state();
        let kb = state.kbs.create_for("u1", "E KB", "d").unwrap();
        let req = Request::builder().uri("/kbs/x").body(Body::empty()).unwrap();
        let html = body_of(kb_detail_page(State(state.clone()), Path(kb.id.clone()), req).await).await;
        // RAGFlow 添加文件下拉：上传文件 / 清空
        assert!(html.contains("emptyAllDocs"), "empty-all button missing");
    }

    #[tokio::test]
    async fn chat_detail_page_renders_temporary_conversation() {
        let (_root, state) = test_state();
        let req = Request::builder().uri("/chat/c1").body(Body::empty()).unwrap();
        let html = body_of(chat_detail_page(State(state), Path("c1".to_string()), req).await).await;
        // RAGFlow: 临时对话（不落库）+ 嵌入 / 分享
        assert!(html.contains("tempConv"), "temp conversation button missing");
        assert!(html.contains("tempModal"), "temp modal missing");
        assert!(html.contains("sendTemp"), "temp send handler missing");
        assert!(html.contains("/api/v1/completions"), "stateless completion API missing");
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
        assert!(html.contains("exportChunks"), "export chunks handler missing");
    }

    #[tokio::test]
    async fn temp_dump_rendered_html() {
        let (_root, state) = test_state();
        let kb = state.kbs.create_for("u1", "Dump KB", "d").unwrap();
        let req = || Request::builder().uri("/x").body(Body::empty()).unwrap();
        let files = body_of(files_page(State(state.clone()), req()).await).await;
        let agents = body_of(agents_page(State(state.clone()), req()).await).await;
        let ad = body_of(agent_detail_page(Path("a1".to_string()), req()).await).await;
        let kbd = body_of(kb_detail_page(State(state.clone()), Path(kb.id.clone()), req()).await).await;
        let cd = body_of(chat_detail_page(State(state.clone()), Path("c1".to_string()), req()).await).await;
        let doc = body_of(document_page(State(state.clone()), Path(("k1".to_string(), "d1".to_string())), req()).await).await;
        let set = body_of(settings_page(req()).await).await;
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
        let req = Request::builder().uri("/settings").body(Body::empty()).unwrap();
        let html = body_of(settings_page(req).await).await;
        // RAGFlow user-setting 团队页：邀请
        assert!(html.contains("inviteUser"), "invite button missing");
        assert!(html.contains("/api/v1/users/invite"), "invite API missing");
    }
}

