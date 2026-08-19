# RAGFlow Web UI Inventory

## 用户设置壳层逐点击记录（live v0.26.2 + RayRAG 隔离复点，2026-08-14）

> 浏览器证据边界：上游观察使用 Chrome 149/CDP `127.0.0.1:16002`，
> 报告 `/tmp/ragflow-user-settings-shell-audit.json` 记录 11 动作、7 页、11 截图、
> 38 网络事件、0 error、0 非 GET。当时页面显示 **v0.26.2**，且
> `/api/v1/system/version` 虽为 HTTP 200，业务体是 `code=503` /
> `RAGFlow is starting, please wait`；因此路由、壳层和按钮是 live 证据，
> 动态数据不作为固定 v0.26.4 的稳定数量证据。固定目标另以 commit
> `cb93883f3f8c975eecb2fed81210effeb3bdb06f` 的 `routes.tsx`、
> `user-setting/index.tsx`、`sidebar/index.tsx` 和 `sidebar/hooks.tsx` 核对。

| # | 模拟用户动作 | 点击后观察 |
|---:|---|---|
| 1 | hover 首页头像/用户入口 | 真实元素为无文本、无 `aria-label/title` 的 `a[href="/user-setting"]`；没有 tooltip、popover 或 dialog。 |
| 2 | 点击头像/用户入口 | 进入 `/user-setting/data-source`，而非 Profile；Data sources 变为唯一 active 项。 |
| 3 | 点击 Header 语言按钮 | `aria-haspopup=menu` 菜单展开，可见 16 种语言：English/简中/繁中/Indonesia/日本語/Español/Tiếng Việt/Русский/Português (Brasil)/Deutsch/Français/Italiano/Български/العربية/Türkçe/한국어。 |
| 4 | 按 Escape | 语言菜单关闭，visible menu 恢复为 0。 |
| 5 | 点击 `Data sources` | URL 为 `/user-setting/data-source`，显示 `Available sources`、数据源卡和 Add 入口；本次启动中页面可见 Confluence、Notion、Google Drive、Gmail、GCS、Oracle、S3、R2、Jira、Box、Dropbox、Bitbucket、GitLab、GitHub、Moodle、Discord。 |
| 6 | 点击 `Chat channels` | URL 为 `/user-setting/chat-channel`，显示 Discord / DingTalk / Feishu-Lark / QQ Bot / Telegram / WeCom / WhatsApp 七种渠道。 |
| 7 | 点击 `Model providers` | URL 为 `/user-setting/model`，显示 Set default models、Search 和类型 chips；因启动中响应本次显示 `All 0`，不取代下文稳定模型页审计。 |
| 8 | 点击 `MCP` | URL 为 `/user-setting/mcp`，显示 MCP servers / Search / Add MCP / Import / Bulk manage / Add Now。 |
| 9 | 点击 `Team` | URL 为 `/user-setting/team`，显示 Search / Invite member；启动中数据显示 `undefined workspace`，不作稳定内容证据。 |
| 10 | 点击 `Profile` | URL 为 `/user-setting/profile`，显示 Upload 和 3 个 Edit 入口。 |
| 11 | 点击 `API` | URL 为 `/user-setting/api`，当前启动中页面为空；document.title 与前六页一样均为 `RAGFlow`。 |

RayRAG 在隔离 Rust binary `127.0.0.1:18381` 上使用同一真实 Chrome 复点，
报告 `/tmp/rayrag-user-setting-browser-audit.json` 记录 13 动作、22 请求、0 失败、
0 Runtime exception、0 console error、0 异常响应。动作覆盖登录后点头像入口、
七个侧栏项、历史 back/forward、主题开关并恢复、切到移动视口以及登出；
唯一 POST 是审计登录。桌面侧栏实测 303 px，移动侧栏 64 px，每页始终只有
一个 `aria-current=page`。截图见 `/tmp/rayrag-user-setting-00-data-source.png`、
`/tmp/rayrag-user-setting-model.png`、`/tmp/rayrag-user-setting-api.png` 和
`/tmp/rayrag-user-setting-03-mobile.png`。

> 当前 Rust 只声称壳层/路由 partial：Chat channel 帐号 CRUD、MCP
> Search/Add/Import/Bulk、Team 完整成员表、Profile 完整编辑、API 完整文档、
> data-source detail route、固定 Header 精确外观与服务端 token revoke 仍未完成。

## Model providers 逐点击记录（live v0.26.2，2026-08-14）

> 浏览器证据边界：通过 fnOS Chrome 149 的 CDP `127.0.0.1:16002` 模拟真实用户
> 点击；页面左下角和 DOM 均显示 `v0.26.2`，所以这里只记为 live v0.26.2 交互证据，
> **不冒充固定 v0.26.4 浏览器证据**。固定目标另以 commit
> `cb93883f3f8c975eecb2fed81210effeb3bdb06f` 的
> `system-setting.tsx`、`un-add-model.tsx`、`used-model.tsx` 源码逐行核对。首轮原始报告位于
> `/tmp/ragflow-model-ui-audit.json`：24 个动作、17 张截图、36 条网络事件、0 error；
> Added models 补点报告为 `/tmp/ragflow-model-ui-added-audit.json`（动作 25–30），
> provider modal 补点为 `/tmp/ragflow-model-ui-provider-modal-audit.json`（动作 31–35），
> local provider 补点为 `/tmp/ragflow-local-provider-audit.json` 与
> `/tmp/ragflow-local-picker-audit.json`（动作 36–39）。补点仅把
> MiniMax-M3 从原始 inactive 切为 active 后立即恢复 inactive；编辑和删除均在确认前 Cancel。
> 默认 LLM 树的 provider/instance 折叠、Search/No data、Escape/重开深审计另见
> `/tmp/ragflow-llm-selector-deep-audit.json`（10 动作/10 截图/0 error/0 请求）。
> 最后折叠组并把复用的审计标签恢复为 `about:blank`，未改变原有 RAGFlow agent 标签。

| # | 模拟用户动作 | 点击后观察 |
|---:|---|---|
| 1 | 导航至 `/user-setting/model` | 六个默认模型依次为 `*LLM / Embedding / VLM / ASR / Rerank / TTS`；Added 有 5 个可展开组；Available 为 62 张卡；chip 为 All 62、LLM 37、Embedding 25、Rerank 10、TTS 8、ASR 11、VLM 17、OCR 4。 |
| 2 | hover LLM 帮助图标 | tooltip 出现：`The default LLM for each newly created dataset.`；移开后关闭。 |
| 3 | 点击 LLM 默认模型，再关闭 | `aria-expanded=false→true`；弹出带 Search 的树，显示 `MiniMax / default` 与 MiniMax 模型；关闭后恢复 false。 |
| 4 | 点击 Embedding 默认模型，再关闭 | 弹出带 Search 的树，内容为 `No data available`；关闭后恢复 closed。 |
| 5 | 点击 VLM 默认模型，再关闭 | 弹出带 Search 的树，内容为 `No data available`；关闭后恢复 closed。 |
| 6 | 点击 ASR 默认模型，再关闭 | 弹出带 Search 的树，内容为 `No data available`；关闭后恢复 closed。 |
| 7 | 点击 Rerank 默认模型，再关闭 | 弹出带 Search 的树，内容为 `No data available`；关闭后恢复 closed。 |
| 8 | 点击 TTS 默认模型，再关闭 | 弹出带 Search 的树，内容为 `No data available`；关闭后恢复 closed。 |
| 9 | 点击 MiniMax/default `View models`，再折叠 | 文案变 `Hide models`，展开 12 个 chat 模型行及开关；再点恢复 `View models`。 |
| 10 | 点击 OpenAI-API-Compatible/default `View models`，再折叠 | 展开 Qwen3.5-9B image2text、Qwen3.5-9B-64K chat、PaddleOCR-VL-1.6 image2text；再点折叠。 |
| 11 | 点击 OpenAI-API-Compatible/embedding `View models`，再折叠 | 展开 Qwen3-Embedding-4B embedding；再点折叠。 |
| 12 | 点击 PaddleOCR/default `View models`，再折叠 | 展开 PaddleOCR-VL-1.6 OCR；再点折叠。 |
| 13 | 点击 Xinference/default `View models`，再折叠 | 展开 mxbai-rerank-large-v2 Rerank；再点折叠。 |
| 14 | 点击 LLM chip | chip 变选中，卡片缩为 37 张。 |
| 15 | 点击 Embedding chip | 选中切换到 Embedding，卡片缩为 25 张。 |
| 16 | 点击 Rerank chip | 选中切换到 Rerank，卡片缩为 10 张。 |
| 17 | 点击 TTS chip | 选中切换到 TTS，卡片缩为 8 张。 |
| 18 | 点击 ASR chip | 选中切换到 ASR，卡片缩为 11 张。 |
| 19 | 点击 VLM chip | 选中切换到 VLM，卡片缩为 17 张。 |
| 20 | 点击 OCR chip | 只显示 Tongyi-Qianwen、MinerU、OpenDataLoader、PaddleOCR 4 张卡。 |
| 21 | 点击 All chip | 恢复 62 张卡。 |
| 22 | Search 输入 `DeepSeek`，再清空 | 输入后只剩 DeepSeek 1 张卡；清空恢复 62；计数随 Search 结果联动。 |
| 23 | hover OpenAI Available 卡 | 卡内 `Add` 的 opacity 从 0 变 1；移开后隐藏。 |
| 24 | 点击 OpenAI `Add`，再点 Cancel | 弹窗含 Instance name、API-Key、Base-Url（默认 `https://api.openai.com/v1`）、Verify、Cancel、Ok；Cancel 后 dialog 数恢复 0，未提交。 |
| 25 | 再点 MiniMax/default `View models` | 文案变 `Hide models`；发出 `GET /api/v1/providers/MiniMax/instances/default/models`；模型行出现 hover Pencil 与 active switch。 |
| 26 | 点击 MiniMax-M3 Pencil，再点 Cancel | `Edit model` 弹窗：Model name=`MiniMax-M3`（disabled）、Model type=`Chat`（唯一可编辑项）、Max tokens=`1000000`（disabled）；按钮 Cancel/Confirm/Close；Cancel 无请求。 |
| 27 | 点击 MiniMax/default 垃圾桶，再点 Cancel | 先出现 `Are you sure to delete it ?` alertdialog，按钮 Cancel/Delete；Cancel 后没有 DELETE 请求，实例保留。 |
| 28 | 点击原为 inactive 的 MiniMax-M3 switch | `aria-checked=false→true`；PATCH `/api/v1/providers/MiniMax/instances/default/models/MiniMax-M3`，body status=`active`，HTTP 200 / code 0。 |
| 29 | 再点 MiniMax-M3 switch 恢复 | `aria-checked=true→false`；同一路径 PATCH status=`inactive`，HTTP 200 / code 0；最终状态与动作前一致。 |
| 30 | 点击 `Hide models` 并恢复审计标签 | 组折叠；临时标签恢复 `about:blank`，已有 agent 标签未改动。 |
| 31 | 点击 OpenAI Available 卡右上外链箭头 | 新标签打开 `https://platform.openai.com/api-keys`；链接为 `target=_blank`、`rel="noopener noreferrer"`；源页面 dialog 数仍为 0，证明外链点击不会冒泡打开 Add。 |
| 32 | 点击 OpenAI 卡内 `Add` | 标题为 OpenAI；live v0.26.2 可见 Instance name、API-Key、Base-Url、Group ID，按钮 Verify/Cancel/Ok；此时无网络请求。 |
| 33 | 必填项留空点击 `Verify` | 原位显示 `Please input the instance name!` 与 `Please enter the API key`；dialog 保持打开，新增请求为 0。 |
| 34 | 点击 provider modal 的 `Cancel` | dialog 数恢复 0，无非 GET 请求。 |
| 35 | 关闭外链并恢复临时审计标签 | 审计创建的标签恢复 `about:blank`；报告的 `non_get_requests=[]`。 |
| 36 | 依次点击 Ollama、VLLM、OpenAI-API-Compatible、LM-Studio 卡，再 Cancel | 四个本地 provider 都显示 Instance name、Base url、可选 API-Key、Vision、List models、Verify/Cancel/Ok；Cancel 无写请求。 |
| 37 | Ollama 保持空 Base url，点击 `List models` | 只发 `GET /api/v1/providers/Ollama/models`；picker 展开并显示 `No models available` 与 `Add custom model`。 |
| 38 | 点击 `Add custom model` | 打开嵌套 dialog：Model name、Model type/Select value、Max tokens、Model features/Tool call、Cancel/Confirm/Close。 |
| 39 | Cancel/关闭本地 provider dialog | 没有 POST/PATCH/DELETE；Search、默认 selector 与 Added 展开状态均恢复干净。当前 live 页面仍明确为 v0.26.2。 |

固定 v0.26.4 源码确认：只有 LLM 必填且不可清空；其余五项可清空。选择值为
`model@instance@provider`，change 立即 PATCH `/api/v1/models/default`，更新类型依次为
`chat / embedding / vision / asr / rerank / tts`。Available Search 按 provider 名称子串过滤；
chip 在 Search 结果上继续筛选且重复点击当前 chip 会取消；整卡及 hover Add 均打开 provider modal。
Added models 按 provider→instance 分组；View/Hide 懒加载实例模型；实例删除必须确认；每个模型行
有 hover Pencil（仅 model type 可编辑）和 active/inactive switch，状态更新使用上表的 PATCH 路径。
固定 v0.26.4 的 generic provider 配置仅把 Group ID 用于 MiniMax；live v0.26.2 OpenAI 弹窗仍显示
Group ID，属于版本漂移，RayRAG 以固定源码为准。固定源码还确认 Verify 先校验整个表单，再调用
`POST /api/v1/providers/{provider}/connection` 且不持久化；Ok 调用
`POST /api/v1/providers/{provider}/instances`。本轮没有输入真实密钥或点击 Ok，因此浏览器捕获全为
只读；Rust 行为由本地 mock endpoint 的隔离 API 测试覆盖。

### RayRAG 隔离 binary 浏览器复点（v0.3.3bk）

为避开主工作树中用户正在编辑的 datasource/connectors 三个编译断点，本批在隔离 clone
仅补齐这些断点并构建同一 Rust 改动，启动于 `127.0.0.1:19472`；仍使用真实 Chrome 149/CDP
`127.0.0.1:16002` 模拟用户点击。首轮点击发现 OpenAI 错显 Group ID 与初始两个模型 API
401，分别由 `[hidden]` CSS 被覆盖及鉴权 fetch wrapper 安装过晚造成；修复后完整复点如下：

| 模拟用户动作 | RayRAG 修后观察 |
|---|---|
| 打开 `/user-setting/model` | 六项顺序为 `*LLM / Embedding / VLM / ASR / Rerank / TTS`；Added 空态正常；所有初始 API 为 2xx，无重定向。Available 为 61 张；固定 v0.26.4 源码按 66 factory 减实际 3 个排除项应为 63，仍是明确差距。 |
| hover OpenAI 卡 | `+ Add` 最终 opacity=1。 |
| 点击 OpenAI 外链 | 新标签 URL 为 `https://platform.openai.com/api-keys`；源 modal 不打开。 |
| 点击 OpenAI `Add` | 只显示 Instance name/API-Key/Base-Url，Group ID hidden；按钮 Verify/Cancel/Ok。 |
| 空表单点击 Verify | 出现两条固定必填错误；0 新请求，modal 保持打开。 |
| Cancel；再点击 MiniMax `Add` | Cancel 关闭；MiniMax 弹窗额外显示 Group ID，符合固定 v0.26.4。 |
| 点击 OpenAI 整卡，再按 Escape | 整卡打开同一 modal；Escape 关闭，最终无 dialog。 |

最终捕获为 0 非 GET、0 HTTP error、0 console error、0 page error；修后截图位于
`/tmp/rayrag-model-bk-02-provider-modal-final.png`。没有输入真实密钥、没有点击 Ok，也没有
创建/修改供应商实例。

### RayRAG 固定 63-provider 目录复点（v0.3.3bl）

固定 v0.26.4 `provider_api_service.list_providers(all_available=True)` 从 66 个 factory 排除
`FastEmbed`、`Builtin`、`siliconflow_intl`，得到 63 个 Available providers。本批隔离 Rust
binary 继续启动于 `127.0.0.1:19472`，Chrome/CDP 审计报告为
`/tmp/rayrag-provider-directory-audit.json`，最终截图为
`/tmp/rayrag-provider-directory-final.png`。

| 模拟用户动作 | RayRAG 点击后观察 |
|---|---|
| 打开 `/user-setting/model` | `63 shown`；顺序开头严格为 OpenAI、Anthropic、Gemini、DeepSeek、Moonshot、Tongyi-Qianwen、ZHIPU-AI、xAI；chip 为 All 63、LLM 37、Embedding 25、Rerank 10、TTS 8、ASR 11、VLM 17、OCR 4。 |
| Search 输入 `Tongyi-Qianwen` | 只剩 canonical Tongyi-Qianwen；清空恢复 63。 |
| 点击 OCR chip，再点同 chip | 首次只剩 Tongyi-Qianwen、MinerU、OpenDataLoader、PaddleOCR；再次点击取消筛选并恢复 63。 |
| Search 输入 `SoMark` | 只剩固定 v0.26.4 新增的 SoMark；清空恢复 63。 |
| 点击 OpenAI-API-Compatible `Add` | modal 标题正确，可见 Instance name/API-Key/Base-Url，Base-Url=`http://127.0.0.1:8088/v1`；Cancel 关闭。 |
| 点击 Tongyi-Qianwen 整卡 | 打开标题为 Tongyi-Qianwen 的同一 tenant modal；Cancel 关闭。 |
| 点击 New API 整卡 | 能打开 New API 标题，但 Base-Url 当前 hidden；固定源码的本地 provider 表单还要求 model type/name/Base URL/max tokens，故登记为下一批专属 modal 缺口。 |

最终 Search 为空、63 张卡可见、0 dialog、0 HTTP/console/page error；网络写入只有隔离实例登录，
没有 Verify/Ok/provider mutation。复用的浏览器标签恢复 `about:blank`，原 RAGFlow 两个标签未改变。

### RayRAG 本地模型 picker 全链路复点（v0.3.3bm）

固定 v0.26.4 源码确认 List models providers 为 Ollama、OpenRouter、VLLM、
OpenAI-API-Compatible、LM-Studio、VolcEngine、Xinference、LocalAI、BaiduYiyan、New API；
RayRAG 在一次性隔离数据目录中启动模型 mock `127.0.0.1:18188`、Rust server
`127.0.0.1:18189` 和 HeadlessChrome 144/CDP `127.0.0.1:16003`。浏览器报告为
`/tmp/rayrag-local-picker-browser-audit.json`，截图为
`/tmp/rayrag-local-picker-01-empty.png`、`02-discovered.png`、`02b-custom-open.png`、
`03-custom-selected.png`、`04-added.png`。

| 模拟用户动作 | RayRAG 点击后观察 |
|---|---|
| Search 输入 `New API`，点击唯一卡片 | 打开 New API dialog；可见 Instance name/Base url/API-Key/Vision/List models/Verify/Cancel/Ok。 |
| 空表单点击 Verify | 原位显示 Instance name、Base url、至少一项模型三条错误；新增请求为 0。 |
| 输入实例名和 mock Base url，点击 List models | GET discovery 返回 All models、audit-chat/chat、audit-embed/embedding；未持久化 provider。 |
| 选择 audit-chat，再点 Vision | 选中集合含 audit-chat；最终 payload 给该 chat 模型追加 image2text。 |
| 点击 Add custom model | 嵌套 dialog 在父 dialog 上层显示；输入 browser-custom、选 LLM、Max tokens=32768、Tool call 后 Confirm。 |
| 点击 Verify | POST connection 返回成功；此时 provider/tenant instance 均未持久化。 |
| 点击 Ok | POST instances 成功；不刷新页面即出现 `New API / Browser gateway / View models`，没有退化为内部 id `new-api`。 |
| 点击 View models | 文案变 Hide models；展开 audit-chat 与 browser-custom，均为 chat/image2text 且 active；持久文件确认自定义模型 `is_tools=true`。 |

本轮 9 组动作全部由真实 CDP 鼠标/键盘输入完成，Network 记录了 discovery GET、Verify POST、
Create POST 和 Added GET；`Runtime.exceptionThrown=[]`，最终 openDialogs=0。隔离 Chrome 使用独立
profile，未接触用户浏览器数据、缓存、日志或下载目录。Verify 当前仍是只读 model-list probe，
并非上游逐 capability 推理，因此该链路保持 `partial`。

### RayRAG 默认模型树逐点击复点（v0.3.3bn）

固定 v0.26.4 `model-tree-select.tsx` / `tree-select.tsx` 明确生成
provider→instance→model 三个节点深度，并以叶子 selector 更新默认模型。RayRAG 使用一次性
New API tenant instance 提供六类 mock 模型，模型服务在 `127.0.0.1:18288`，隔离 Rust binary
在 `127.0.0.1:18289`，独立 HeadlessChrome 144/CDP 在 `127.0.0.1:16003`。完整报告为
`/tmp/rayrag-model-tree-browser-audit.json`，截图为
`/tmp/rayrag-model-tree-00-initial.png` 至 `05-reloaded.png`。

| 模拟用户动作 | RayRAG 点击后观察 |
|---|---|
| 逐个点击 LLM/Embedding/VLM/ASR/Rerank/TTS trigger，再点击关闭 | 六项均 `closed→open→closed`，有 Search；弹层与 trigger 左边及宽度在 2px 子像素误差内一致。LLM 同时显示 chat 和 image2text 两个叶，另五项各一个匹配叶。 |
| 点击 LLM provider 行，再点恢复 | treeitem 从 4 行缩为 1 行，再恢复；只改变 branch 展开状态，没有请求或选择。 |
| 点击 instance 行，再点恢复 | treeitem 从 4 行缩为 2 行，再恢复；provider→instance→model 三层保持 4/24/44px 缩进。 |
| Search 输入 `audit` | 保留 provider/instance 与两个命中 model 叶；搜索期间匹配路径展开。 |
| Search 输入不存在值 | treeitem 变 0，仅显示 `No data available`。 |
| 按 Escape；再打开 | 弹层关闭、焦点回 LLM trigger，Search 原值保留；重开仍显示 No data。首轮真实回放曾发现 Chrome 会默认清空 search input，修复为 preventDefault 后通过。 |
| Search 改回 `audit` 后点击页面标题 | outside click 关闭弹层且保留 Search；再次打开后再清空。 |
| 依次打开六项并点击 model leaf | 每次叶子选择关闭弹层并清空 Search，触发 PATCH 后再 GET defaults；六个 body 类型严格为 chat、embedding、vision、asr、rerank、tts。 |
| Embedding 打开后 Search=`audit`，点击 × | PATCH 三个模型字段为空；value 清空，但弹层仍 open 且 Search 保留，符合 non-LLM clear 合同。 |
| 主动阻断 Rerank clear 的 PATCH | 页面先乐观清空，网络失败后恢复旧 selector 并显示 Error；trigger 从 pending disabled 恢复可用。 |
| reload 页面 | LLM、VLM、ASR、Rerank、TTS 均从 GET 回显，Embedding 保持已清空；特别确认 `vision/asr` 写入和 `image2text/speech2text` 响应映射没有丢值。 |

本轮 23 组动作均由真实鼠标/键盘完成，捕获 6 次选择 PATCH、1 次成功 clear PATCH、1 次
审计主动阻断的 rollback PATCH，以及每次成功后的 defaults GET；`Runtime.exceptionThrown=[]`，
最终六 trigger 均 closed、0 可见 popover。唯一 404 为无关 `favicon.ico`，唯一 loadingFailed
就是主动阻断的 Rerank 请求。隔离 profile 未接触用户浏览器数据、缓存、日志或下载目录。
固定源码搜索在“父标题命中但所有子项不命中”时可能产生可选伪叶，Rust 有意不复制：保留原始
后代并始终按 `node.kind` 限制只有 model leaf 可选择；该安全修正已记录为明确偏差。

### RayRAG Bedrock / SoMark 专属弹层逐点击复点（v0.3.3bp）

固定 v0.26.4 `setting-model/modal/bedrock-modal/index.tsx` 与
`somark-modal/index.tsx` 要求这两张 Available 卡各自使用专属弹层，而不是统一的
ProviderModal。RayRAG 用本地 OpenAI 兼容模型 mock（`127.0.0.1:18588`）与隔离 Rust binary
（`127.0.0.1:18589`）做真实点击，浏览器为 fnOS Chrome 149/CDP `127.0.0.1:16002`。完整报告
`/tmp/rayrag-somark-bedrock-browser-audit.json`：12 动作、9 截图、0 console/page error、
0 异常 API；补充已有实例名检查报告 `/tmp/rayrag-somark-bedrock-audit/` 下截图
`00-bedrock-empty.png` 至 `08-somark-models.png`。

| 模拟用户动作 | RayRAG 点击后观察 |
|---|---|
| Bedrock 卡点 `+ Add` | 打开专属 bedrock dialog（标题 Bedrock），非通用弹层；Instance name / Model type / Model name / 认证区 / AWS region / Max tokens。 |
| 空表单点 Verify | 只做原位校验：instance name、model name、ACCESS KEY、SECRET KEY、Please select!；不发任何 provider 请求。 |
| 依次点 Access Key / IAM Role / Assume Role 三段 | AK/SK 字段仅 Access Key 显示；IAM Role 只显示 AWS Role ARN；Assume Role 显示 EC2 已有角色提示；切换时清除非活动凭证字段。 |
| 填 bedrock-audit / anthropic.claude-sonnet / AK/SK / us-east-1 后 Verify | POST `/api/v1/providers/Bedrock/connection`，api_key 是 `{auth_mode,bedrock_ak,bedrock_sk,bedrock_region}` 的 JSON 字符串；状态显示 `Your API key is valid.` |
| 点 Ok | POST `/api/v1/providers/Bedrock/instances`，api_key 为密钥对象，region=default，model_info 单条 chat；弹层关闭，Added models 出现 Bedrock 组。 |
| 重开 Bedrock 弹层并输入已存在的 bedrock-audit | 认证区（三段、AK/SK、Role ARN、region）整体隐藏，与固定 `useFetchInstanceNameSet` 行为一致。 |
| SoMark 卡点 `+ Add` | 打开专属 somark dialog（标题 SoMark）；Instance name / Model name / Base URL / API Key / Element Formats 四选择器 / Feature Config 七开关。 |
| 空表单点 Verify | 原位校验 instance name、model name、Base URL；API Key 保持可选（自托管可留空）。 |
| 核对默认值 | 四选择器默认 url/latex/html/image；七开关只有 Enable Table Image 与 Enable Image Understanding 默认开。 |
| 打开 Enable Text Cross Page 与 Keep Header Footer，填 mock base 后 Verify | POST `/api/v1/providers/SoMark/connection`，model_info extra 含全部 11 个 `somark_*` 字段，编辑过的开关为 true；状态显示 `Verified`。 |
| 点 Add | POST `/api/v1/providers/SoMark/instances`，`llm_factory=SoMark`、model_type=[ocr]；Added models 出现 SoMark 组。 |
| 展开 SoMark `View models` 并直读 models API | 行显示 `somark-from-env-1` + ocr 徽标；`extra.ocr_config` 回读全部 11 个字段与编辑后的开关值，证明持久化非仅客户端临时状态。 |

### RayRAG Langfuse 配置弹层逐点击复点（v0.3.3bq）

固定 v0.26.4 `setting-model/langfuse/` 四文件把 Langfuse 卡放在 `/user-setting/api`
的 API 内容区。RayRAG 用本地 Langfuse public-API mock（`127.0.0.1:18688`，health +
projects 两个端点）与隔离 Rust binary（`127.0.0.1:18689`）真点，浏览器为 fnOS Chrome
149/CDP `127.0.0.1:16002`。完整报告 `/tmp/rayrag-langfuse-browser-audit.json`：8 动作、
6 截图、0 console/page error、0 异常 API；无效凭证补充检查
`/tmp/rayrag-langfuse-audit/` 下 `cdp_audit_invalid.py` 输出（对话保持打开、状态为
`Invalid Langfuse keys`）。截图 `00-api-page.png` 至 `05-deleted.png`。

| 模拟用户动作 | RayRAG 点击后观察 |
|---|---|
| 进入 `/user-setting/api` | Langfuse 卡与个人 API KEY 卡并存；卡内有标题、描述、Configuration 按钮；未配置时 View 隐藏。 |
| 点击 `Configuration` | 打开标题为 `Configuration Langfuse` 的弹层：Secret key / Public key（password）、Host（placeholder `https://cloud.langfuse.com`）、左下文档外链、Delete / Cancel / Save；全新配置时 Delete 隐藏。 |
| 空表单点 Save | 原位显示 `Please enter the secret key` 与 `Please enter the public key`；不发任何请求。 |
| 填 sk-audit / pk-audit / `http://127.0.0.1:18688` 后 Save | POST `/api/v1/langfuse/api-key` body 恰为三字段；后端以 Basic auth 探测 mock 的 health+projects 并缓存 `p-audit-1`；弹层关闭，View 出现且 href 为 `http://127.0.0.1:18688/project/p-audit-1`（自托管 host 优先，非硬编码 cloud.langfuse.com）。 |
| 重开 Configuration | secret/public/host 全部回填，Delete 出现。 |
| 填 `bad-pk` 后 Save（补充检查） | 弹层保持打开，状态显示上游同文案 `Invalid Langfuse keys`，未持久化。 |
| 点 Delete → 确认弹层 → Delete | DELETE `/api/v1/langfuse/api-key`；卡回到未配置状态，View 再次隐藏。 |
| 直读 GET | 返回 `code=0` 且无 `data`、message=`Have not record any Langfuse keys.`，与上游空记录响应一致。 |

### RayRAG provider 专属字段逐点击复点（v0.3.3br）

固定 v0.26.4 `provider-config-map.ts` / `use-provider-fields.tsx` /
`use-provider-modal-actions.ts` 要求 10 个 ProviderConfigMap 工厂在通用弹层里渲染各自的
动态凭据字段。RayRAG 用 catch-all OpenAI 兼容模型 mock（`127.0.0.1:18788`）与隔离 Rust
binary（`127.0.0.1:18789`）真点，浏览器为 fnOS Chrome 149/CDP `127.0.0.1:16002`。完整报告
`/tmp/rayrag-special-providers-browser-audit.json`：11 动作、4 截图、0 console/page
error、0 异常 API；截图 `01-azure-fields.png` 至 `04-ocr-saved.png`。

| 模拟用户动作 | RayRAG 点击后观察 |
|---|---|
| Azure-OpenAI 卡点 Add | 弹层出现 Model type 三选（默认 embedding）、Model name（默认 gpt-3.5-turbo）、API-Version（默认 2024-02-01）、Max tokens 8192；选 chat 才显示 Vision 开关，切回 embedding 即隐藏。 |
| 填 azure-audit + key + mock base 后 Ok | POST 的 `api_key` 为 `{api_key, api_version}` 合并对象，model_info 为 embedding gpt-3.5-turbo 8192；Added models 出现 Azure 组。 |
| 重开 Azure 并输入已存在实例名 | Base url 与 API-Key 整体隐藏（hideWhenInstanceExists）。 |
| VolcEngine Add | API-Key 标签变为 `VOLC ARK_API_KEY`，字段是 Model ID + Max tokens；保存时 api_key 为标量 ark key、model_name 即 endpoint id。 |
| Google Cloud Add | Project ID / Region / Service Account Key 三字段；保存时 api_key 为 `{google_project_id, google_region, google_service_account_key}`。 |
| Tencent Cloud Add | Model name 为 22 个 16k 引擎的选择器（默认 16k_zh），sid/sk 两字段；保存 model_type=`speech2text`、api_key 为 `{TencentCloud_sid, TencentCloud_sk}`。 |
| XunFei Spark Add | chat 时只有 Spark APIPassword；勾选 tts 后 APP ID / APISecret / APIKey 三字段出现；保存 api_key 含四键、model_type=`[chat,tts]`。 |
| BaiduYiYan / Fish Audio Add | 分别保存 `{yiyan_ak,yiyan_sk}` 与 `{fish_audio_ak,fish_audio_refid}` 对象。 |
| MinerU Add | Backend 选择器默认 pipeline，Server URL 仅选 vlm-http-client 时出现，Delete output 开关默认开；保存 api_key 含 `mineru_delete_output='1'`、model_type=`[ocr]`。 |
| PaddleOCR / OpenDataLoader Add | Algorithm 选择器默认 PaddleOCR-VL；OpenDataLoader 空 apiserver 点 Ok 原位报 `Please input the OpenDataLoader API server URL!`；两者保存 OCR cfg api_key 对象与 `[ocr]` model_info。 |

### RayRAG provider 品牌图标复点（v0.3.3bs）

固定 v0.26.4 `svg-icon.tsx` 的 `LlmIcon` 对 `svgIcons` 工厂使用 `assets/svg/llm/*.svg`
静态品牌图，其余使用 iconfont 的 `IconMap` symbol，未映射回退 `moxing-default`。
RayRAG 隔离 binary `127.0.0.1:18889` + fnOS Chrome 149/CDP `127.0.0.1:16002` 复核，
报告 `/tmp/rayrag-provider-icons-browser-audit.json`，截图 `01-provider-icons.png`。

| 模拟用户动作 | RayRAG 点击后观察 |
|---|---|
| 打开 `/user-setting/model` | 63 张 Available 卡全部渲染品牌图标而非首字母头像。 |
| 检查 OpenAI / MiniMax / Tencent Cloud 卡 | `<use href='#icon-openai'>` / `#icon-MiniMax` / `#icon-tencent-cloud`，且每个 symbol 都在加载后的 iconfont 定义集中解析成功。 |
| 检查 Gemini / MinerU / PaddleOCR / SoMark / New API 卡 | `<img src='/assets/svg/llm/gemini.svg'>` 等 10 个静态品牌图全部 `naturalWidth>0`（真实加载）。 |
| 统计 | 53 张 iconfont 卡 + 10 张静态 SVG 卡 = 63；0 console/page error、0 资产失败请求。 |

### RayRAG MCP servers 页逐点击复点（v0.3.3bt）

固定 v0.26.4 `user-setting/mcp/` 十二文件定义 MCP servers 页。RayRAG 用本地
streamable-http MCP mock（`127.0.0.1:18988`，initialize/tools/list 两方法）与隔离
binary（`127.0.0.1:18989`，`ALLOW_ANY_HOST=1` 允许回环 URL）真点，浏览器为 fnOS Chrome
149/CDP `127.0.0.1:16002`。报告 `/tmp/rayrag-mcp-browser-audit.json`：9 动作、5 截图、
0 console/page error、0 异常 API；截图 `00-mcp-empty.png` 至 `04-mcp-imported.png`。

| 模拟用户动作 | RayRAG 点击后观察 |
|---|---|
| 进入 `/user-setting/mcp` | 标题 `MCP servers` + `Customize the list of MCP servers`；工具栏 Search / Bulk manage / Import / + Add MCP；空态虚线框 `No MCP servers available` + Add Now。 |
| 点 Add Now，空表单 Save | 原位报 `Please input name` 与 `Please input URL`；名字含空格时报固定 1-64 字符正则文案。 |
| 填 audit-server + mock URL + Streamable HTTP + token 后 Test | `initialize`/`tools/list` 带 `Authorization: Bearer audit-token` 发往 mock；显示 `2 tools available`，工具名与描述逐条列出。 |
| Save | POST `/api/v1/mcp/servers` body 恰为 name/url/server_type/variables.authorization_token；卡片出现并显示 `2 cached tools`。 |
| Search 输入 nope / audit / 清空 | 卡片 1→0→1，关键词过滤即时生效。 |
| 点 Edit | 弹层回填 name/url/server type/token 与已缓存工具；改名后 Save 发 PUT。 |
| 点 Export | 发出 `GET /api/v1/mcp/servers/{id}?mode=download`，返回 `{mcpServers:{...}}` 导出体。 |
| Bulk manage → Select all | `Selected 1`；点 Delete 出确认弹层（selected 1 MCP server(s)）→ 确认后 DELETE，回到空态。 |
| Import 选 JSON 文件后 Save | POST `/api/v1/mcp/servers/import` 原样传 `{mcpServers}`；导入后卡片显示 `imported-server` + `2 cached tools`。 |

### RayRAG Team 页逐点击复点（v0.3.3bu）

固定 v0.26.4 `user-setting/setting-team/` 五文件定义 Team 页。RayRAG 隔离 binary
`127.0.0.1:19089`（开注册）用两个真实账号真点，浏览器为 fnOS Chrome 149/CDP
`127.0.0.1:16002`。报告 `/tmp/rayrag-team-browser-audit.json`：8 动作、3 截图、
0 page error、0 异常 API（唯一 400 是审计主动的未知邮箱邀请用例）；截图
`00-team-owner.png` 至 `02-member-invitation.png`。

| 模拟用户动作 | RayRAG 点击后观察 |
|---|---|
| 管理员进入 `/user-setting/team` | 标题 `{nickname} workspace`；Team members 表一行 owner（Owner 徽标、无 Delete）；Joined teams 表含自己的工作区。 |
| 点 Invite member，空/非法邮箱 Ok | 原位报 `Please input email`；未注册邮箱提交后状态显示上游文案 `User not found.`。 |
| 邀请已注册 member@test.local | POST `/api/v1/tenants/{id}/users` body `{email}`；成员表新增一行 Invited 徽标。 |
| 点该行 Delete → 确认 | `Are you sure to remove this member?` → DELETE → 行移除。 |
| 重新邀请后以成员登录并进入 Team | Joined teams 显示受邀团队的 `Accept` / `Decline`；点 Accept → PATCH `/api/v1/tenants/{id}`，徽标由 Invited 变 Member。 |
| 点 Quit → 确认 | `Are you sure you want to quit the team you joined?` → DELETE 自身成员关系 → 该行消失。 |
| 管理员点 Date 表头 | 排序图标按 ↕ → ↑ → ↓ 循环。 |

### RayRAG Profile 页逐点击复点（v0.3.3bv）

固定 v0.26.4 `user-setting/profile/` 三文件定义 Profile 页。RayRAG 隔离 binary
`127.0.0.1:19189` 真点，浏览器为 fnOS Chrome 149/CDP `127.0.0.1:16002`。报告
`/tmp/rayrag-profile-browser-audit.json`：7 动作、2 截图、0 page error（唯一 400
为审计主动的错误当前密码用例）；截图 `00-profile.png`、`01-profile-edited.png`。

| 模拟用户动作 | RayRAG 点击后观察 |
|---|---|
| 进入 `/user-setting/profile` | Username / Avatar / Time Zone / Email / Password 五行渲染自 `/api/v1/user/info`；Email 行显示只读说明。 |
| 点 Username 行 Edit | `Edit Name` 弹层；改名后 Save → PATCH `/api/v1/users/me` body `{nickname}`，行与全局头部同步更新。 |
| 点 Time Zone 行 Edit | `Edit Time Zone` 弹层；选 America/Los_Angeles 后 Save → PATCH `{timezone}` 并回显。 |
| 点 Avatar 行 Upload 选 PNG | 前端读 data URL 后即时 PATCH `{avatar}`；头像 `naturalWidth>0` 显示。 |
| 点 Password 行 Edit | `Edit Password` 弹层；错误当前密码提交后状态显示上游 `Password error!`；正确旧密码+新密码（≥8、两次一致）提交后清 token 并跳 `/login`。 |
| 用新密码重新登录 | 登录成功，证明改密与 token 失效闭环。 |

### RayRAG Chat channels 页逐点击复点（v0.3.3bw）

固定 v0.26.4 `user-setting/chat-channel/` 八文件定义 Chat channels 页。RayRAG 隔离
binary `127.0.0.1:19289` 真点，浏览器为 fnOS Chrome 149/CDP `127.0.0.1:16002`。报告
`/tmp/rayrag-chatchannel-browser-audit.json`：6 动作、3 截图、0 page error、0 异常
API；截图 `00-chat-channels.png` 至 `02-telegram-deleted.png`。

| 模拟用户动作 | RayRAG 点击后观察 |
|---|---|
| 进入 `/user-setting/chat-channel` | Added channels 显示 `No chat channels added yet. Select one below to connect.`；Available channels 7 张卡（Discord/DingTalk/Feishu/Lark/Telegram/QQ Bot/WeCom/WhatsApp）。 |
| 点 Telegram 卡，空表单 Save | 原位报 `Name is required` 与 `Bot Token is required`。 |
| 填名称+token 后 Save | POST `/api/v1/chat-channels` body 恰为 `{name, channel:'telegram', config:{credential:{token}}}`；Added 出现 Telegram 组。 |
| 点 DingTalk 卡 | 弹层标题 `Add DingTalk bot`，字段 App Key / App Secret；保存后 `config.credential` 带两键。 |
| 点 Telegram 组的 Edit | `Edit Telegram bot` 回填 name/token；改名后 PATCH 并即时更新列表。 |
| 点 Delete → 确认 | 固定 `Delete chat channel` 文案 → DELETE `/api/v1/chat-channels/{id}` → 只剩 DingTalk 组。 |

### RayRAG Data source 详情页逐点击复点（v0.3.3bz）

固定 v0.26.4 `data-source-detail-page/` 两文件定义数据源详情页。RayRAG 隔离 binary
`127.0.0.1:19489` 真点，浏览器为 fnOS Chrome 149/CDP `127.0.0.1:16002`。报告
`/tmp/rayrag-ds-detail-browser-audit.json`：6 动作、2 截图、0 page error、0 异常 API；
截图 `00-ds-detail.png`、`01-ds-synced.png`。

| 模拟用户动作 | RayRAG 点击后观察 |
|---|---|
| 创建 local 数据源后进入列表 | 每行新增 `Details` 按钮。 |
| 点 Details | 路由到 `/user-setting/data-source/{id}`，连接表单回填 name/url/target 与同步设置。 |
| 改名后 Save | PUT `/api/v1/data_sources/{id}`，状态显示 `Saved ✓`，标题同步更新。 |
| 点 Sync | POST `/{id}/sync`，状态显示 `Synced N docs ✓`；Sync Logs 表出现一条 success 行（time/status/message）。 |
| 点 ← Back | 返回 `/user-setting/data-source` 列表。 |

### RayRAG Data source 详情页逐源字段 + 状态机复点（v0.3.3ca）

固定 v0.26.4 `data-source/constant/` 六文件定义逐源动态表单。RayRAG 隔离 binary
`127.0.0.1:19489` + 本地 mock GitLab `127.0.0.1:19488`，浏览器为 fnOS Chrome
149/CDP `127.0.0.1:16002`。报告
`/tmp/rayrag-ca-audit/rayrag-datasource-fields-browser-audit.json`：8 动作、
3 截图（`00-detail-fields.png`、`01-saved-schedule.png`、`02-resumed.png`）、
0 console error、0 page error、0 异常 API。

| 模拟用户动作 | RayRAG 点击后观察 |
|---|---|
| 创建 gitlab 源并点 Details | 7 个 GITLAB 字段渲染：Project Owner/Name（必填星标）、Access Token（密码框）、GitLab URL、3 个 include 复选框；Name 禁用；Test 隐藏；Prune Freq 隐藏。 |
| 勾选 Sync deleted files | Prune Freq 即时出现；取消勾选后隐藏，再勾选保持。 |
| 填字段后点 Save | PATCH body 恰为嵌套 `config`（project_owner/project_name/credentials.gitlab_access_token/gitlab_url/include_*）+ `sync_deleted_files` + `reschedule:true`；mock 收到 `/api/v4/projects/...` 同步；状态 `Saved ✓`，主按钮变 `Stop`，日志出现 success 行。 |
| 点 Stop | PATCH `{status:"cancel"}`，主按钮变 `Resume`。 |
| 点 Resume | PATCH `{status:"schedule"}`，mock 收到第二轮同步，主按钮回 `Stop`。 |
| 重载详情页 | 嵌套 config 回填 Project Owner/Name、URL、token 与 sync 开关；Prune Freq 保持可见。 |

### RayRAG Data source Add 弹层复点（v0.3.3cb）

固定 v0.26.4 `add-datasource-modal.tsx` 定义创建弹层。RayRAG 隔离 binary
`127.0.0.1:19489`，浏览器为 fnOS Chrome 149/CDP `127.0.0.1:16002`。报告
`/tmp/rayrag-cb-audit/rayrag-datasource-add-modal-browser-audit.json`：6 动作、
4 截图（`00-add-s3.png` 至 `03-add-local.png`）、0 console error、0 page error、
0 异常 API。

| 模拟用户动作 | RayRAG 点击后观察 |
|---|---|
| 类型选 S3，点 + Add | 弹层标题 `Add data source: S3`；Bucket Name（必填星标）、Region（35+ 选项）、Prefix、Mode 与 Authentication 分段按钮、AWS Access Key 两字段。 |
| 点 Mode 的 S3 Compatible | Authentication 分段隐藏，Addressing Style 与 Endpoint URL 出现（AWS 密钥按上游 OR 条件保持可见）。 |
| 不填直接 Confirm / 只填 Name 再 Confirm | 原位分别报 `Name is required` 与 `Bucket Name is required`。 |
| 填 Bucket/Endpoint 后 Confirm | POST body 恰为嵌套 `config`（bucket_name/bucket_type=s3_compatible/credentials.endpoint_url…），列表出现 S3 Bucket。 |
| 类型选 local，点 + Add | 回退 URL/Target/Token 三个字段，Confirm 发送旧扁平 `{name,type,url,target}`。 |

### RayRAG Data source 列表卡片/分组/删除弹层复点（v0.3.3cc）

固定 v0.26.4 `index.tsx`/`added-source-card.tsx`/`delete-source-modal.tsx` 定义
列表布局。RayRAG 隔离 binary `127.0.0.1:19489`，浏览器为 fnOS Chrome 149/CDP
`127.0.0.1:16002`。报告
`/tmp/rayrag-cc-audit/rayrag-datasource-list-cards-browser-audit.json`：7 动作、
4 截图（`00-available-cards.png` 至 `03-deleted.png`）、0 console error、
0 page error、0 异常 API。

| 模拟用户动作 | RayRAG 点击后观察 |
|---|---|
| 进入数据源页 | 35 张 Available 卡渲染，S3 卡品牌 SVG 自然加载（naturalWidth>0），描述齐全；Added 空态 `No data sources added yet.`。 |
| 悬停 S3 卡 | 右下角 `+ Add` 从 opacity 0 变为 1；点卡打开 `Create your S3 connector` 弹层。 |
| 填名称/Bucket 后 Confirm | 嵌套 config POST，Added 区出现按 S3 分组的卡片与新行（名称 + disconnected 徽章）。 |
| 点行内 Settings 齿轮 | 路由到 `/user-setting/data-source/{id}` 且 Name 回填；返回列表正常。 |
| 点行内 Trash 垃圾桶 | 弹层标题 `Delete data source`，正文 `Are you sure you want to delete this data source link?`，框内显示图标与名称。 |
| 点 Cancel | 弹层关闭、行保留；再点 Trash 后点 Confirm | DELETE 请求发出，列表回到空态。 |

### RayRAG Box web-OAuth 字段复点（v0.3.3cd）

固定 v0.26.4 `box-token-field.tsx` 定义 Box 凭据字段。RayRAG 隔离 binary
`127.0.0.1:19489`，浏览器为 fnOS Chrome 149/CDP `127.0.0.1:16002`；account.box.com
弹窗用 route abort 模拟离网。报告
`/tmp/rayrag-cd-audit/rayrag-box-oauth-browser-audit.json`：7 动作、3 截图
（`00-box-field.png`、`01-box-waiting.png`、`02-box-authorized.png`）、
0 console error、0 page error、0 异常 API。

| 模拟用户动作 | RayRAG 点击后观察 |
|---|---|
| 点 Box 卡 | 弹层标题 `Create your Box connector`；box_tokens 显示 `Configure Box credentials` 按钮而非裸 textarea。 |
| 打开凭据弹层空提交 | 原位报 `Please fill in Client ID, Client Secret, and Redirect URI.`。 |
| 填三项后 Submit & Authorize | POST start body 恰为三字段；弹出 account.box.com authorize 窗口（带 state）；状态显示等待文案。 |
| 点 Refresh status（未回调） | 返回 code 106，状态显示 `Authorization is still pending.`。 |
| 回调注入 code 后再 Refresh | 返回 credentials，弹层关闭、字段出现 Authorized 徽章，隐藏值 JSON 含 authorization_code。 |
| 填 folder_id + 名称后 Confirm | `config.credentials.box_tokens` 为含 authorization_code 的 JSON 字符串，folder_id 透传。 |

## 全站首轮遍历（v0.26.2 实测，2026-08-06）

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

## 用户设置 `/user-setting`（7 个路由项 + footer Log out）
- **Data sources**（`/user-setting/data-source`）：35 种数据源 + `Add`：
  Confluence/Notion/Google Drive/Gmail/Google Cloud Storage/Oracle Storage/S3/R2/Jira/Box/Dropbox/Bitbucket/GitLab/GitHub/Moodle/Discord/Zendesk/WebDAV/Airtable/Asana/IMAP/Dingtalk AI Table/SeaFile/MySQL/PostgreSQL/REST API/RSS/OneDrive/Outlook/Salesforce/Azure Blob Storage/Microsoft Teams/Slack/SharePoint
- **Chat channels**（`/user-setting/chat-channel`）：Discord / DingTalk / Feishu-Lark /
  QQ Bot / Telegram / WeCom / WhatsApp 渠道管理
- **Model providers**（`/user-setting/model`）：`Set default models` + `Available models`；类型筛选 All 62/LLM 37/Embedding 25/Rerank 10/TTS 8/ASR 11/VLM 17/OCR 4；`View models`、`Search`、`Add`
- **MCP**（`/user-setting/mcp`）：MCP 服务器管理
- **Team**（`/user-setting/team`）：团队成员
- **Profile**（`/user-setting/profile`）：个人资料
- **API**（`/user-setting/api`）：`API KEY`（Show content/Configuration）+ 完整 HTTP API 文档（ERROR CODES 表、Deprecated API Aliases 表、OpenAI-Compatible API、Dataset/File/Chunk/Chat Assistant/Session/Agent/Memory/System/File Management/Search App 全部端点，含 Request/Response 示例）—— 即 RayRAG API 覆盖 checklist
- **Log out**

---

# 补充遍历（v0.26.2 实测，2026-08-13，fygo Chrome CDP + Playwright）

> 本次为 API 全链路可用的真实遍历：登录态 cookie 过期后按配方重新登录成功；
> 发现 v0.26.2 UI 为 Tailwind 重写版（非旧 AntD Tabs），以下为增量结论。

## 登录页补充
- **双表单并存**（桌面 + 移动响应式重复，DOM 中同时可见两组 Email/Password/Sign in/Sign up）
- 会话过期时任何路由自动重定向 `/login`；登录成功后回跳首页

## 数据集卡片 DOM（点击实现）
- 卡片 = `article.rounded-lg.border-border-button`（**不是** `<a>`，是 onClick 跳转）
- 标题 = 卡片内 `h3`；卡片头含名称 + `N files` + 更新时间

## 知识库详情页（侧边栏导航，非 Tabs）
- 左侧 `aside.w-64` → `nav` → `ul.space-y-5` → `li > a` 四入口：
  `Files` `/dataset/files/{id}`、`Retrieval testing` `/dataset/retrieval/{id}`、
  `Logs` `/dataset/logs/{id}`、`Configuration` `/dataset/configuration/{id}`
- 头部：名称 / `N files` / `30 GB` / `Created <date>`
### Files 页实测
- 提示：`Please wait for your files to finish parsing before starting an AI-powered chat.`
- 表格列：Name / Upload date / Enable / Chunks / Metadata(`N fields`) / Parse(`General` 下拉) / Action
- 行内：Enable switch、`N fields` 元数据按钮、`General` 解析方法下拉、行操作图标
- `Add file` + `50 / Page` 分页
### Retrieval testing 页实测
- 引导文案：`Conduct a retrieval test to check if RAGFlow can recover the intended content for the LLM...`（含"设置不会自动保存，需应用到 chat assistant"警告）
- 控件：`Select value` 下拉 ×2（相似度阈值等）、`Top 10` 下拉、`Run` 按钮
### Logs 页实测
- 统计卡片：Total files `30329`、`0% from last week`、Downloading/Success/Failed、Processing/Success/Failed
- 筛选按钮：`File` / `Dataset` + 表格列 ID/File name/Source/Ingestion pipeline/Start date/Task/Status/Operations（行含 Failed 状态）
### Configuration 页实测
- Basic：`*Name`、Language（`Chinese` 下拉）、Avatar（`Upload`，上限 4 MB）、Description、Permissions（`Only me` 下拉）、Embedding model、Page rank、Tag sets（`Select value`）
- Parse type：`Built-in`（默认）/`Pipeline` 单选 + 下拉 `General` / PDF parser `DeepDOC`（下拉含 General）
- 按钮：`Upload` `Link data source` `Cancel` `Save` `Settings` `Built-in pipeline introduction`

## Agent 画布编辑器 `/agent/{id}?category=agent_canvas`
- 顶栏：名称 + `Autosaved <时间>` + `Save` `Run` `Launch` `Publish` `Management`
- 分类选择器 `Begin / Conversational`；画布引擎 **React Flow**（`.react-flow__node` 节点 + handle）
- 空画布最小节点 = Begin + Conversational 两节点

## Memory 详情 `/memory/memory-message/{id}`
- 头部：名称 + `Created <date>`；导航：`Messages` / `Configuration`
- Messages：提示 `Memory extract is configured with Prompts and Temperature from Advanced Settings.`
- 表格列：Session ID / Agent / Type / Valid date / Forget at / Enable / Action；空态 `No data available`；`Total 0` + `50 / Page`

## Model providers `/user-setting/model`（全量）
- `Set default models`：`Please complete these settings before beginning` + 六能力默认选择器
  **`*LLM` / `Embedding` / `VLM` / `ASR` / `Rerank` / `TTS`**
- `Added models`：已添加列表（MiniMax/OpenAI-API-Compatible/embedding/PaddleOCR/Xinference，各带 `default` 徽标 + `View models`）
- `Available models`：类型筛选 chips **All 62 / LLM 37 / Embedding 25 / Rerank 10 / TTS 8 / ASR 11 / VLM 17 / OCR 4**
- 提供商卡片（能力徽标 + `Add`）：OpenAI(LLM/Embedding/TTS/ASR)、Anthropic(LLM)、Gemini(LLM/Embedding/VLM)、
  DeepSeek(LLM)、Moonshot(LLM/VLM)、Tongyi-Qianwen(LLM/Embedding/Rerank/TTS/ASR/VLM/OCR)、ZHIPU-AI(LLM/Embedding/ASR/VLM)、
  xAI(LLM/VLM)、HuggingFace、Cohere(LLM/Embedding/Rerank/ASR)、OpenRouter …

## 数据源 `/user-setting/data-source`（全量）
- 左侧菜单：Data sources / Chat channels / Model providers / MCP / Team / Profile / API / v0.26.2 / Log out
- 35 种 Available sources（各带描述 + `Add`）；已连接列表区显示已配置源（如 WebDAV 及其挂载点）
