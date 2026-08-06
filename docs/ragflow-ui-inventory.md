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

## 知识库详情（4 Tab）
### Files `/dataset/files/{id}`
- 头部：名称、`N files`、`30 GB`、Created；Tab：Files/Retrieval testing/Logs/Configuration
- 工具栏：刷新、列设置、`Search`、`Add file`（下拉）
- 表格列：Select all / Name / Upload date / Enable(开关) / Chunks / Metadata(`N fields`) / Parse(`General` 下拉) / Action
- 行操作：预览(名称)、Metadata、Parse、启用开关、下载、删除、`...` 更多
### Retrieval testing `/dataset/retrieval/{id}`
- Setting：相似度阈值 `Select value`、`Top N`、问题 textarea、`Run`；Results 区
### Logs `/dataset/logs/{id}`
- 统计：Total files/Downloading/Processing；筛选 `File`/`Dataset` + `Search`
- 表格列：ID/File name/Source/Ingestion pipeline/Start date/Task/Status/Operations
### Configuration `/dataset/configuration/{id}`
- 基本：`*Name`、Language(`English`/`Chinese`)、Avatar、Description、Permissions(`Only me`)、Embedding model、Page rank、Tag sets
- 解析：Parse type(`General`/`DeepDOC`)、Built-in pipeline、`PDF parser`、Recommended chunk size、Delimiter for text、Child chunk are used for retrieval、PageIndex、Image & table context window、Auto metadata、Overlapped percent(%)、Auto-keyword、Auto-question、Excel to HTML、Indexing model
- 操作：`Upload`、`Link data source`、`Cancel`、`Save`、`Settings`、`Built-in pipeline introduction`

## Chat `/chats`
- 空态文案："No chat app created yet"（无应用时主体为空 + 引导）

## Search `/searches`
- 空态文案："No search app created yet"

## Agent `/agents`
- 标题 Agents + `Search` + `Create agent`（筛选下拉 + 主按钮）；卡片（名称/`Last saved at:` 时间/设置按钮）；分页

## Memory `/memories`
- 标题 Memory + `Search` + `Create memory`；卡片（名称/时间/设置按钮）；分页

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
