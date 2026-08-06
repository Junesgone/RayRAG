# RAGFlow Web UI Inventory（v0.26.2 实测，2026-08-06）

> 采集方式：真实浏览器登录 junesgone@outlook.com 逐页遍历（CDP 点击 + SPA 导航）。
> 用途：RayRAG 前端逐页对齐清单。路由均为实测。

## 全局 Header 导航（所有页面）
- Logo → `/`；`header.Root` → `/`；`Dataset` → `/datasets`；`Chat` → `/chats`
- `Search` → `/searches`；`Agent` → `/agents`；`Memory` → `/memories`；`File` → `/files`
- 右侧：Discord/GitHub/Docs 外链、语言切换（English）、用户头像 → `/user-setting`

## 登录页 `/login`
- Email（*）、Password（*）、Remember me、`Sign in`、`Sign up`
- 密码前端 RSA 加密（JSEncrypt，公钥内嵌 bundle；登录链路：auth/login → users/me → users/me/models → datasets → tenants）

## 注册页
- Email（*）、Nickname（*）、Password（*）、`Continue`、`Already have an account? Sign in`

## 首页 `/`
- Welcome to RAGFlow；Dataset 区（知识库卡片 + `See all`）；Chat 区（`Chat`/`Search`/`Agent`/`Memory` 入口 + 会话卡片）

## Dataset 列表 `/datasets`
- 标题 + 筛选按钮 + `Search` + `Create dataset`；卡片（名称/`N files`/更新时间/设置按钮）；分页 `Total N`/页码/`50 / Page`
- **Create dataset 弹窗**：`Name`（必填）、`Embedding model`（下拉）、`Parse type`（下拉，占位 "Please select a chunking method."）、`Built-in`（Pipeline 默认选项）、`Pipeline`（下拉）；按钮 `Save` / `Close`

## 知识库详情（4 Tab）
### Files `/dataset/files/{id}`
- 头部：名称、`N files`、`30 GB`、`Created 18/05/2026`；Tab：Files/Retrieval testing/Logs/Configuration
- Files 标题下提示："Please wait for your files to finish parsing before starting an AI-powered chat."
- 工具栏：刷新按钮、列设置按钮、`Search`、`Add file`（下拉 = **Upload file / Create empty file**）
- 表格列：Select all / Name / Upload date / Enable(switch) / Chunks / Metadata(`N fields` 按钮) / Parse(`General` 下拉) / 图标按钮×2 / Action
- 行内操作（实测）：Enable switch、`N fields`（metadata 弹窗）、`General`（解析方法下拉）、4 个图标按钮（含 `#icon-reparse` 重新解析）+ `...` 更多（下拉菜单）
- 行操作菜单项（前端 locale 源码确认）：`Preview` / `Edit` / `Delete` / `Download` / `Export` / `Reparse`
- **Upload file 弹窗**：`Files` / `Folder` 两个 Tab；拖放区 "drop your file here to upload"（file input multiple）；说明：本地部署单次 1GB 上限、批量 32 文件；cloud 10MB/128 文件；按钮 `Save` / `Close`
- 分页 50/Page（607 页，`More pages` 快捷跳页）
### Retrieval testing `/dataset/retrieval/{id}`
- Setting 完整字段：**Similarity threshold**（相似度阈值下拉）、**Vector similarity weight**（vector 0.30 / full-text 0.70 双滑块）、**Rerank model**（下拉）、**Use knowledge graph**（checkbox）、**Cross-language search**（checkbox）、**Meta data**（下拉）、**Top N**（默认 Top 10）
- 操作：问题 textarea + **Run**；Results 区（空态 "No test has been run yet"）
### Logs `/dataset/logs/{id}`
- 统计：Total files/Downloading/Processing；筛选 `File`/`Dataset` + `Search`
- 表格列：ID/File name/Source/Ingestion pipeline/Start date/Task/Status/Operations
### Configuration `/dataset/configuration/{id}`
- 基本：`*Name`、Language 下拉（**English/Chinese/Traditional Chinese/Russian/Indonesian/Spanish/Vietnamese/Japanese/Korean/Portuguese BR/German/French/Italian/Tamil/Telugu 等 20+**）、Avatar、Description、Permissions 下拉（**Only me / Team**）、Embedding model、Page rank、Tag sets
- 解析：Parse type 下拉 = **Chunking method 完整选项：General / Q&A / Resume / Manual / Table / Paper / Book / Laws / Presentation / One / Tag**；Built-in pipeline、PDF parser（**Light/General**）、Recommended chunk size、Delimiter for text、Child chunk are used for retrieval、PageIndex、Image & table context window、Auto metadata、Overlapped percent(%)、Auto-keyword、Auto-question、Excel to HTML、Indexing model
- 操作：`Upload`、`Link data source`、`Cancel`、`Save`、`Settings`、`Built-in pipeline introduction`

## Chat `/chats`
- 空态文案："No chat app created yet"（无应用时主体为空 + 引导）

## Search `/searches`
- 空态文案："No search app created yet"

## Agent `/agents`
- 标题 + `Search` + `Create agent`（下拉 = **Create from blank / Create from template / Import JSON file**）；agent 卡片（名称/`Last saved at:` 时间/设置按钮）；分页
- **Create agent 弹窗**：`Choose agent type`（**Agent flow** / **Ingestion pipeline** 两个类型卡片）、`Name`（必填，placeholder "Please input name"）；按钮 `Save` / `Close`
- 点击 Save 后进入 Agent 画布编辑器（flow 编辑器，节点拖拽 + 侧栏配置）

## Memory `/memories`
- 标题 Memory + `Search` + `Create memory`；卡片（名称/时间/设置按钮）；分页
- **Create memory 弹窗**：`Name`（placeholder "memory name"）、`*Memory type`（下拉 = **raw / semantic / episodic / procedural** + `(Select all)` 全选）、`Embedding model`（下拉）、`LLM`（下拉）；按钮 `Cancel` / `Confirm`；弹窗底部 `Clear` / `Close`

## File `/files`
- 标题 Files + `Search` + `Add file`；表格列：Select all/Name/Upload date/Size/Dataset/Action；分页

## 用户设置 `/user-setting`（左侧菜单 8 项）
- **Data sources**（`/user-setting/data-source`）：35 种数据源 + `Add`：
  Confluence/Notion/Google Drive/Gmail/Google Cloud Storage/Oracle Storage/S3/R2/Jira/Box/Dropbox/Bitbucket/GitLab/GitHub/Moodle/Discord/Zendesk/WebDAV/Airtable/Asana/IMAP/Dingtalk AI Table/SeaFile/MySQL/PostgreSQL/REST API/RSS/OneDrive/Outlook/Salesforce/Azure Blob Storage/Microsoft Teams/Slack/SharePoint
- **Chat channels**：渠道管理
- **Model providers**（`/user-setting/model`）：`Set default models` + `Available models`；类型筛选 All 62/LLM 37/Embedding 25/Rerank 10/TTS 8/ASR 11/VLM 17/OCR 4；`View models`、`Search`、`Add`
- **MCP**：MCP 服务器管理
- **Team**：团队成员
- **Profile**：个人资料
- **API**（`/user-setting/api`）：`API KEY`（Show content/Configuration）+ 完整 HTTP API 文档（ERROR CODES 表、Deprecated API Aliases 表、OpenAI-Compatible API、Dataset/File/Chunk/Chat Assistant/Session/Agent/Memory/System/File Management/Search App 全部端点，含 Request/Response 示例）—— 即 RayRAG API 覆盖 checklist
- **Log out**
