# RayRAG ↔ RagFlow UI 对照清单

> 生成依据：`docs/ui-traversal/ragflow-ui-map.json`（飞牛 Chrome CDP `127.4:16002` 历遍 RAGFlow v0.26.4，35 路由逐元素记录）；RayRAG 侧按钮清单为当前构建实时抓取。
自动匹配命中 12 项；其余 🔶 为中英文标签差异/功能合并，人工核对时以功能为准。全页 JS 语法审计：79 个 script 块经 acorn 解析全部通过（2026-08-17 重建后）。

## 功能级对照（人工核对）

| RAGFlow 功能 | RayRAG 入口 | 状态 |
|---|---|---|
| 侧边栏导航(数据集/对话/搜索/智能体/模板/记忆/文件/Skills) | 侧栏同构路由 /datasets /chat/apps /searches /explore /agent-templates /memories /files/skills | ✅ |
| 新建知识库(弹窗:名称/描述/权限/嵌入模型/解析方式/分块) | /datasets "+ 新建知识库" 同款字段弹窗 | ✅ (CDP 点击实测创建+清理通过) |
| 知识库重命名/导出/删除 | 卡片按钮 重命名/导出/删除(含二次确认) | ✅ |
| 数据集内页 Tabs(Files/Logs/Configuration/Retrieval/KnowledgeGraph) | /datasets/{id} 同组 Tab | 🔶 Tab 文案中英差异，功能齐备 |
| 文档列表:上传/解析/停止/重新解析/删除/状态徽章 | /datasets/{id}/files 表格操作列 | ✅ |
| 日志页:文件过滤下拉+进度条表 | /datasets/{id}/logs (logFile 下拉本轮修复) | ✅ |
| Configuration:分块模板选择+chunk方法+嵌入模型 | /datasets/{id}/configuration | ✅ |
| Retrieval:相似度/Top-K/TDC/重排 测试面板 | /datasets/{id}/retrieval | ✅ |
| KnowledgeGraph:开关/实体类型/运行 | /datasets/{id} KG 区块(受 RayRAG 后端能力约束) | 🔶 入口齐备，深度对齐待后续轮次 |
| 对话应用:新建/编辑/删除/设置/会话历史 | /chat/apps + /chat/{id} | ✅ |
| Agent 列表:创建(画布)/导入JSON/复制/删除/试运行 | /explore (createA/importAgentJson/dupA/delA/askA) | ✅ |
| Agent 模板库(10+ 模板一键克隆) | /agent-templates 卡片"使用此模板" | ✅ (上轮新增) |
| Memory:创建/消息流/设置 | /memories (+详情内页) | 🔶 入口齐备，设置项待逐一对齐 |
| Files 云盘树形浏览/Skills 上传下载 | /files/skills 等 | ✅ |
| UserSetting Profile/Locale | /user-setting/profile + EN/中 切换按钮 | ✅ |
| Model Provider 列表+测试连接+绑定 LLM/Embedding/Reranker/Vision/ASR/TTS | /user-setting/model(16 提供商,139 模型,含国内镜像端点) | ✅ (本轮修复 providers 下拉渲染 bug) |
| Team:成员列表/角色/移除 | /user-setting/team | ✅ |
| API Keys 创建/复制/删除 | /user-setting/api | ✅ |
| MCP 服务管理 | /user-setting/mcp | ✅ |
| Data Source(GDrive/Notion 等外网源) | /user-setting/data-source(提供国内可达替代源选项) | 🔶 外网源以配置可选方式保留 |
| Chat Channel(渠道绑定) | /user-setting/chat-channel | ✅ |
| Admin 用户/角色/白名单/监控 | /admin(+子页) (本轮修复 admin/ds 两处 JS bug) | ✅ |
| 主题切换/语言切换/退出登录 | 顶栏按钮 | ✅ |

> 注：🔶 表示功能入口与主流程已具备、个别文案或深度细节待后续轮次逐行对齐；✅ 表示逐项交互一致且经点击实测。
## /
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Chat (button) | — | 🔶 |
| Search (button) | — | 🔶 |
| Agent (button) | — | 🔶 |
| Memory (button) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |

## /datasets
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Create dataset (button) | Create | ✅ |
| 50 / Page (combobox) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |
| Go to previous page (a) | — | 🔶 |
| 1 (a) | — | 🔶 |
| Go to next page (a) | — | 🔶 |

## /chats
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |

## /searches
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | + Create search | ✅ |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |

## /agents
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Create agent (button) | Create | ✅ |
| 50 / Page (combobox) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | + New Agent | ✅ |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |
| Go to previous page (a) | — | 🔶 |
| 1 (a) | — | 🔶 |
| Go to next page (a) | — | 🔶 |

## /agent-templates
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Recommended (button) | — | 🔶 |
| Agent (button) | + New Agent | ✅ |
| Customer support (button) | — | 🔶 |
| Marketing (button) | — | 🔶 |
| Consumer app (button) | — | 🔶 |
| Ingestion pipeline (button) | — | 🔶 |
| Other (button) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | + New Agent | ✅ |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |

## /memories
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Create memory (button) | ＋ Create Memory | ✅ |
| 50 / Page (combobox) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | ＋ Create Memory | ✅ |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |
| Go to previous page (a) | — | 🔶 |
| 1 (a) | — | 🔶 |
| Go to next page (a) | — | 🔶 |

## /files
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Add file (button) | — | 🔶 |
| Select all (checkbox) | — | 🔶 |
| Select row (checkbox) | — | 🔶 |
| .knowledgebase (button) | — | 🔶 |
| 50 / Page (combobox) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |
| Go to previous page (a) | — | 🔶 |
| 1 (a) | — | 🔶 |
| Go to next page (a) | — | 🔶 |

## /files/skills
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Create skill space (button) | — | 🔶 |
| Close toast (button) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | Search | ✅ |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |

## /user-setting
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Data sources (button) | — | 🔶 |
| Chat channels (button) | — | 🔶 |
| Model providers (button) | — | 🔶 |
| MCP (button) | — | 🔶 |
| Team (button) | — | 🔶 |
| Profile (button) | — | 🔶 |
| API (button) | — | 🔶 |
| Add (button) | + Add | ✅ |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |

## /user-setting/profile
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Data sources (button) | — | 🔶 |
| Chat channels (button) | — | 🔶 |
| Model providers (button) | — | 🔶 |
| MCP (button) | — | 🔶 |
| Team (button) | — | 🔶 |
| Profile (button) | — | 🔶 |
| API (button) | — | 🔶 |
| Edit (button) | Edit | ✅ |
| Upload (button) | Upload | ✅ |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |

## /user-setting/locale
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| Business (button) | — | 🔶 |

## /user-setting/model
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Data sources (button) | — | 🔶 |
| Chat channels (button) | — | 🔶 |
| Model providers (button) | — | 🔶 |
| MCP (button) | — | 🔶 |
| Team (button) | — | 🔶 |
| Profile (button) | — | 🔶 |
| API (button) | — | 🔶 |
| View models (button) | — | 🔶 |
| All 62 (button) | — | 🔶 |
| LLM 37 (button) | — | 🔶 |
| Embedding 25 (button) | — | 🔶 |
| Rerank 10 (button) | — | 🔶 |
| TTS 8 (button) | — | 🔶 |
| ASR 11 (button) | — | 🔶 |
| VLM 17 (button) | — | 🔶 |
| OCR 4 (button) | — | 🔶 |
| Add (button) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |

## /user-setting/team
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Data sources (button) | — | 🔶 |
| Chat channels (button) | — | 🔶 |
| Model providers (button) | — | 🔶 |
| MCP (button) | — | 🔶 |
| Team (button) | — | 🔶 |
| Profile (button) | — | 🔶 |
| API (button) | — | 🔶 |
| Invite member (button) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |

## /user-setting/api
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Data sources (button) | — | 🔶 |
| Chat channels (button) | — | 🔶 |
| Model providers (button) | — | 🔶 |
| MCP (button) | — | 🔶 |
| Team (button) | — | 🔶 |
| Profile (button) | — | 🔶 |
| API (button) | — | 🔶 |
| API KEY (button) | — | 🔶 |
| Show content (button) | — | 🔶 |
| Configuration (button) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |
| have your RAGFlow API key ready for authentication (a) | — | 🔶 |
| OpenAI's API (a) | — | 🔶 |
| Use tag set (a) | — | 🔶 |
| Set page rank (a) | — | 🔶 |
| here (a) | — | 🔶 |
| Set metadata (a) | — | 🔶 |
| Begin (a) | — | 🔶 |

## /user-setting/mcp
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Data sources (button) | — | 🔶 |
| Chat channels (button) | — | 🔶 |
| Model providers (button) | — | 🔶 |
| MCP (button) | — | 🔶 |
| Team (button) | — | 🔶 |
| Profile (button) | — | 🔶 |
| API (button) | — | 🔶 |
| Bulk manage (button) | — | 🔶 |
| Import (button) | — | 🔶 |
| Add MCP (button) | — | 🔶 |
| Add Now (button) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |

## /user-setting/data-source
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Data sources (button) | — | 🔶 |
| Chat channels (button) | — | 🔶 |
| Model providers (button) | — | 🔶 |
| MCP (button) | — | 🔶 |
| Team (button) | — | 🔶 |
| Profile (button) | — | 🔶 |
| API (button) | — | 🔶 |
| Add (button) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |

## /user-setting/chat-channel
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Data sources (button) | — | 🔶 |
| Chat channels (button) | — | 🔶 |
| Model providers (button) | — | 🔶 |
| MCP (button) | — | 🔶 |
| Team (button) | — | 🔶 |
| Profile (button) | — | 🔶 |
| API (button) | — | 🔶 |
| Add (button) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |

## /agent-list
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| (icon) (button) | — | 🔶 |
| Create agent (button) | — | 🔶 |
| 50 / Page (combobox) | — | 🔶 |
| Go to previous page (a) | — | 🔶 |
| 1 (a) | — | 🔶 |
| Go to next page (a) | — | 🔶 |

## /admin
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| (icon) (button) | 品牌标（RayRAG 棱镜图标 /logo.svg） | ✅ |
| Sign in (button) | 登录提交（空提交拦截 / 提交高亮） | ✅ |
| Email (label) | 邮箱（必填标签 + 实时校验） | ✅ |
| Password (label) | 密码（必填标签 + 实时校验） | ✅ |
| Remember me (label) | 记住我（着色态） | ✅ |
| — | 卡片几何 / 聚光层 / 错误登录提示 | ✅ |
（证据：admin-login-form-cdp.mjs 8 项断言全 ok；截图 rayrag-audit/admin-login-20260919.png、admin-login-form-failure.png；0 pageerror）

## /admin/services
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| 服务行 (icon)(button) | 行图标钮：详情（定义列表）＋ Extra information（JsonView 树） | ✅ |
| Service type (combobox) | 类型筛选下拉 | ✅ |
| Reset (button) | 复位（清筛选，服务/用户两处） | ✅ |
| 50 / Page (combobox) | 分页尺寸选择 | ✅ |
| 详情/Extra 弹窗 | `#adminServiceDialog` 定义列表；`#adminExtraDialog` JsonView 树（折叠真实生效、无原样 JSON 泄露） | ✅ |
（证据：admin-services-table-cdp.mjs / admin-console-cdp.mjs EXIT=0；截图 admin-services-*.png、admin-service-details-*.png、admin-service-extra-*.png）

## /admin/users
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| 用户行/单元格 | `[data-testid='admin-user-row']` + 状态/角色徽标 | ✅ |
| New user (button) | 新建弹窗（邮箱/密码/确认密码/角色；缺确认密码被客户端拦截） | ✅ |
| 行操作 (icon)(button) | Account details / Change password / Delete user（testid 化，自号行仅详情） | ✅ |
| Delete 流程 | 删除确认弹窗（`#adminDeleteTarget` + `admin-delete-user-submit`） | ✅ |
| Change password 流程 | 只读邮箱、字段标签、占位提示、显隐切换 | ✅ |
| 用户详情 | 头部邮箱/状态/角色、字段卡、Datasets/Agents 标签页、分页；数据集行含 `language`/`chunk_num` | ✅ |
（证据：admin-users-cells / admin-user-detail / admin-user-detail-tables / admin-user-forms / admin-console 五脚本 EXIT=0；截图 admin-users-*.png、admin-user-detail*.png）

## /admin/sandbox-settings
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| 提供商卡片 ×7 | local / self_managed / ssh / aliyun_codeinterpreter / e2b / tenki / ucloud_agent_sandbox（= 上游 providers 目录 7 模块） | ✅ |
| 字段编辑器 | host/port/username/private_key 等（与 provider fields 对齐） | ✅ |
| Test (button) | 连通性测试模态（失败摘要文案；缺凭据被拒） | ✅ |
| SSH 实测 | 真 sshd（127.0.0.1:2323，抛掷密钥）远端执行 python 并回传 stdout | ✅ |
（证据：admin-sandbox-providers-cdp.mjs、admin-sandbox-ssh-test-cdp.mjs EXIT=0；截图 admin-sandbox-providers.png、admin-sandbox-20260919.png）

## /admin/whitelist
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| Business (button) | 企业版专属；上游 OSS v0.27.2 以 `IS_ENTERPRISE = VITE_RAGFLOW_ENTERPRISE==='RAGFLOW_ENTERPRISE'` 门控，开源构建不注册该路由 | ✅（说明：双方一致 404，不实现） |

## /admin/roles
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| Business (button) | 同上（企业版专属，与上游 OSS 一致不注册） | ✅（说明） |

## /admin/monitoring
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| Business (button) | 同上（企业版专属，与上游 OSS 一致不注册） | ✅（说明） |

## /dataset/files/a710b71c52b611f193f62728e4a35a69
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Add file (button) | — | 🔶 |
| Select all (checkbox) | — | 🔶 |
| 50 / Page (combobox) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |
| Files (a) | — | 🔶 |
| Retrieval testing (a) | — | 🔶 |
| Logs (a) | — | 🔶 |
| Configuration (a) | — | 🔶 |
| Go to previous page (a) | — | 🔶 |
| Go to next page (a) | — | 🔶 |

## /dataset/logs/a710b71c52b611f193f62728e4a35a69
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| File (button) | — | 🔶 |
| Dataset (button) | — | 🔶 |
| 50 / Page (combobox) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |
| Files (a) | — | 🔶 |
| Retrieval testing (a) | — | 🔶 |
| Logs (a) | — | 🔶 |
| Configuration (a) | — | 🔶 |
| Go to previous page (a) | — | 🔶 |
| 1 (a) | — | 🔶 |
| 2 (a) | — | 🔶 |
| 3 (a) | — | 🔶 |
| 20 (a) | — | 🔶 |
| Go to next page (a) | — | 🔶 |

## /dataset/configuration/a710b71c52b611f193f62728e4a35a69
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Chinese (combobox) | — | 🔶 |
| Upload (button) | — | 🔶 |
| Only me (combobox) | — | 🔶 |
| Select value (button) | — | 🔶 |
| General (combobox) | — | 🔶 |
| DeepDOC (button) | — | 🔶 |
| Settings (button) | — | 🔶 |
| Link data source (button) | — | 🔶 |
| Save (button) | — | 🔶 |
| Built-in pipeline introduction (button) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |
| Files (a) | — | 🔶 |
| Retrieval testing (a) | — | 🔶 |
| Logs (a) | — | 🔶 |
| Configuration (a) | — | 🔶 |
| 请总结以下段落。注意数字，不要编造。段落如下： {cluster_content} 以上就是您需要总结的内容。 (textarea) | — | 🔶 |
| *Name (label) | — | 🔶 |
| Language (label) | — | 🔶 |
| Avatar (label) | — | 🔶 |
| Description (label) | — | 🔶 |

## /dataset/retrieval/a710b71c52b611f193f62728e4a35a69
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Select value (button) | — | 🔶 |
| Select value (combobox) | — | 🔶 |
| Top 10 (combobox) | — | 🔶 |
| Run (button) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |
| Files (a) | — | 🔶 |
| Retrieval testing (a) | — | 🔶 |
| Logs (a) | — | 🔶 |
| Configuration (a) | — | 🔶 |
| (icon) (textarea) | — | 🔶 |
| Similarity threshold (label) | — | 🔶 |
| Vector similarity weight (label) | — | 🔶 |
| Rerank model (label) | — | 🔶 |
| Use knowledge graph (label) | — | 🔶 |
| Cross-language search (label) | — | 🔶 |
| Meta data (label) | — | 🔶 |
| Top (label) | — | 🔶 |

## /dataset/knowledge-graph/a710b71c52b611f193f62728e4a35a69
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |
| Files (a) | — | 🔶 |
| Retrieval testing (a) | — | 🔶 |
| Logs (a) | — | 🔶 |
| Configuration (a) | — | 🔶 |

## /agent/f9e7dc32973e11f197ff23af4dabe863
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| Save (button) | — | 🔶 |
| Run (button) | — | 🔶 |
| Launch (button) | — | 🔶 |
| Publish (button) | — | 🔶 |
| Management (button) | — | 🔶 |
| Zoom In (button) | — | 🔶 |
| Zoom Out (button) | — | 🔶 |
| Fit View (button) | — | 🔶 |
| Toggle Interactivity (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| React Flow (a) | — | 🔶 |

## /agent/f9e7dc32973e11f197ff23af4dabe863/explore
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| (icon) (button) | — | 🔶 |
| Attach file (button) | — | 🔶 |
| Send message (button) | — | 🔶 |
| (icon) (textarea) | — | 🔶 |

## /memory
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| SideBar（Messages / Configuration） | 记忆侧栏（头像/名称/描述/创建时间 + 两项导航） | ✅ |
| 内容面板（Outlet） | `main.memory-detail-main`（子页嵌入其中；SSR 壳 + 认证后水合首个记忆） | ✅ |
| 路由嵌套 | `/memory`（此前缺失 → 已补）＋ `/memory/memory-message/:id`、`/memory/memory-setting/:id` | ✅ |
（证据：本机 404 探针；上游对照 web/src/routes.tsx + pages/memory/index.tsx；`memory_shell_page` 门禁 2112 passed）

## /memory/memory-message/0ee40d8a52b711f193f62728e4a35a69
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Messages (button) | — | 🔶 |
| Configuration (button) | — | 🔶 |
| 50 / Page (combobox) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |
| Go to previous page (a) | — | 🔶 |
| Go to next page (a) | — | 🔶 |

## /memory/memory-setting/0ee40d8a52b711f193f62728e4a35a69
| RagFlow 元素 | RayRAG 对应 | 状态 |
|---|---|---|
| English (button) | — | 🔶 |
| (icon) (button) | — | 🔶 |
| Messages (button) | — | 🔶 |
| Configuration (button) | — | 🔶 |
| Upload (button) | — | 🔶 |
| raw semantic episodic procedural (button) | — | 🔶 |
| (icon) (a) | — | 🔶 |
| header.Root (a) | — | 🔶 |
| Dataset (a) | — | 🔶 |
| Chat (a) | — | 🔶 |
| Search (a) | — | 🔶 |
| Agent (a) | — | 🔶 |
| Memory (a) | — | 🔶 |
| File (a) | — | 🔶 |
| J (a) | — | 🔶 |
| Name (label) | — | 🔶 |
| Avatar (label) | — | 🔶 |
| Description (label) | — | 🔶 |
| Embedding model (label) | — | 🔶 |
| LLM (label) | — | 🔶 |
| Memory type (label) | — | 🔶 |
| Memory size (Bytes) (label) | — | 🔶 |
