# Changelog

All notable changes to RayRAG are documented in this file.

## [0.3.4] — 2026-09-15（对标批次 v0.3.4e–v0.3.5m）

### Added

- **user-setting Profile 页对齐上游**：头部 `setting.profile` + `setting.profileDescription`，190px 标签列 + 边框值框 +
  `PenLine` 编辑按钮（用户名/头像/时区/邮箱/密码），`setting.emailDescription` 与 `setting.avatarTip` 说明文案。
- **AvatarUpload 交互**：无头像时 64×64 虚线上传按钮；有头像时 64×64 预览（hover 铅笔可重选）+ 右上角圆形
  `Remove image` 按钮清空；文件名白名单 `jpg|jpeg|png|webp|bmp`；选中后 canvas 居中裁剪为 64×64 PNG 再上传。
- **Profile 双语**：页面标签/说明/按钮与 JS 校验文案全部中英双语（概要/用户名/头像/时区/邮箱/密码/编辑、
  请选择图片文件、请确认新密码…），弹窗标题按上游保持英文硬编码。
- **CDP 探针** `scripts/ui-traversal/profile-page-cdp.mjs`：验证空态→上传→预览→删除→非法扩展名拒绝→三个编辑弹窗→zh 文案。

### Fixed

- **昵称前端字符集校验**：补 `PROFILE_NAME_PATTERN=/^[\p{L}\p{N} ._'-]+$/u`（对齐上游 `NICKNAME_PATTERN`），
  与 100 字符上限及服务端 `validate_nickname` 一致。


### Added

- **本地提供商字段集对齐上游 `local-llm-configs.ts`**：ModelScope/RAGcon/TogetherAI/Replicate/HuggingFace/GPUStack
  改用显式字段（`model_type` 多选且不预选、`model_name`、`max_tokens` 默认 8192、`Enable tool call` 仅 chat/image2text、
  `Does it support Vision?` 仅 chat、`Base url` 必填、API-Key 可选）；OpenRouter 增加 `Provider order`；
  picker 厂（Ollama 等 10 个）保持模型列表并隐藏通用 Vision 开关。
- **payload 对齐 `buildModelInfoFromValues`**：`model_info[0].extra.is_tools`、chat+vision → 追加 `image2text`、
  `provider_order` 随提交带上，开关字段不再泄漏进 `payload.extra`。
- **CDP 探针** `scripts/ui-traversal/local-provider-modal-cdp.mjs`：逐个打开六个本地厂与 OpenRouter 模态，
  记录类型集/默认值/开关可见性与 payload。

### Fixed

- **Available models 计数胶囊格式**：标签与计数改为相邻节点（`All63`/`VLM17`，间距交给 CSS），卡片 `+ Add`
  文案取自 `setting.addTheModel`（Add/添加）。


### Added

- **提供商「List models」选择器对齐上游 ToggleList**：内联面板 + `role='list'`、选项 >10 才显示带清空按钮的搜索行、
  `All models` 全选哨兵、每行能力胶囊（`setting.modelTypes` 文案，Chat/VLM/ASR）+ 悬停编辑铅笔、搜索无匹配
  `No matching results`、空目录 `No models available`、底部固定「＋ Add custom model」，滚动区上限 400px。
- **自定义模型弹窗编辑模式**：悬停铅笔进入 `Edit model`（名称锁定、回填类型/最大 Token/Tool call），保存写回目录与选中项；
  对话框仅保留上游的 Cancel/Confirm 两个按钮，关闭时重置表单与错误文案；文案与校验按 `use-custom-model-fields.tsx`
  （`modelNameRequired`/`modelNameDuplicate`/`modelTypeRequired`/`modelMaxTokensMinMessage`）本地化。
- **CDP 探针** `scripts/ui-traversal/provider-model-picker-cdp.mjs`：用 `page.route` 桩出 12 个模型，逐步验证展开、
  勾选、全选、搜索无匹配、清空、编辑、新增自定义模型与校验，全程截图。

### Fixed

- **点击模型行误开编辑弹窗**：整行用 `<label>` 包裹时，label 激活会转发给第一个可标注后代，而 `<button>` 也是
  labelable —— 铅笔排在复选框之前导致点行体等于点铅笔。改为上游的 `div` 行 + 行内点击切换 + 铅笔/复选框
  `stopPropagation`。
- **隐藏的编辑铅笔仍可点击**：仅 `opacity:0` 不阻断命中测试，补 `pointer-events:none`（hover/focus 恢复），
  与上游 `hidden group-hover:flex` 对齐。


### Added

- **提供商弹层 Base-Url 对齐上游 `InputSelect`**：由「文本框 + datalist」改为组合控件（已选值 + × 清除 + 下拉箭头触发器、
  可过滤弹层、每行 URL + region 徽标、`Add "<value>"` 自定义行、`No results found` 空态、Enter/Escape/Backspace
  与 150ms blur 提交），默认选中 regionKey 为 `default` 的地址，占位符/提示按 provider 取上游
  `setting.*BaseUrl*` 文案（minimax / tongyi-qianwen / siliconflow / anthropic / openai），zh/en 双语言。
- **Added models 本地化与类型映射**：实例展开按钮改用 `setting.showMoreModels`/`hideModels`（View models/展示更多模型、
  Hide models/隐藏模型），实例类型徽标改用共享 `crate::providers` 的 `mapModelKey` 映射（image2text→VLM、speech2text→ASR、chat→LLM 等）。
- **弹层/弹窗不透明表面**：新增 `--bgm`（深色 `#15141c` / 浅色 `#fff`），`.modal` 与 6 个浮层选择器不再使用 5% 白的 `--bgc`，
  修复弹窗背后卡片透字问题。
- **CDP 探针** `scripts/ui-traversal/provider-base-url-cdp.mjs`：真实点击卡片 → 弹窗 → 打开 Base-Url 弹层 → 切 region → 清除 →
  输入自定义 URL，逐步记录值与派生 region 并截图。

### Fixed

- **`region` 提交字段恒为空**：`data-api-base-options` 被 Rust 侧以 region 优先顺序输出（`[region, url]`），
  与前端/上游 `buildBaseUrlOptions` 的 `{value: url, regionKey}` 相反，导致 region 徽标显示 URL 且
  `providerRegionForBaseUrl` 永远返回 null；现改为 URL 优先并加断言防回归。


### Fixed

- **数据集 Configuration / Knowledge graph 面板错位（用户报告）**：Reranking 提示气泡
  `data-tip='…` 缺收尾单引号，浏览器把其后的 `<div class='provider-default-field'>` 等
  整段当作属性值吞掉，导致 `#tab3/#tab4` 落到 `.ds-layout`（侧栏列）而非 `.ds-content`。
  `kb_detail_page` 与 `dataset_page` 两处同源缺陷一并修复；新增引号感知深度扫描单测
  （`div_depth_at`，修复前 tab3 深度 7 vs tab0 4 必红）。
- **「Reranking model」标签被吞**：`<label …display:block'><` 末尾多余的 `<` 生成伪元素
  `<reranking model="" …>`，吞掉标签文字与 Use knowledge graph 复选框；同步修正
  `.ds-content` 收尾与检索面板左右两列结构。
- **配置页横向溢出 139677px**：dataset / kb_detail 两页残留 legacy JS 把 4000+ 个
  `<option>` 塞进已是弹层触发按钮的 `#embdSel`；删除注入后仍由共享默认模型树
  （`GET /api/v1/models`）提供模型列表，配置页宽度回到 1585px。
- **头像行结构**：配置表补回 `<tr><td>Avatar</td>` 行头、删除遗留空 Avatar 行、文件选择框限宽。
- **主样式表截断**：`src/web_css.txt` 中 `.cc-runtime-qr{width:224px;max-width:100.` 截断
  使浏览器在解析中途放弃，其后全部规则（`.provider-tz-list`、`.set-switch` 等）静默失效
  （CSSOM 517 → 537 条）；修复并新增 `stylesheet_has_no_truncated_rules` 结构化校验单测。

### Added

- **开关控件对齐上游 Switch**：`.set-switch input[type=checkbox]` 与
  `input[type=checkbox][role=switch]` 渲染为 36×20 圆角滑块（`appearance:none` + 白色滑块），
  替换原先在深色主题下显示为大白方块的浏览器原生复选框。
- **侧栏/导航全量实点脚本** `scripts/ui-traversal/sidebar-routes-cdp.mjs`：顶部导航 7 项、
  用户设置侧栏 7 项、数据集详情侧栏 5 项与 `/user-setting` 共 21 条路由逐页点击，记录状态码、
  主列高度/文本长度、横向溢出、page error 与截图；本次结果 21/21 通过。
- **诊断脚本**：`overflow-cdp.mjs`（定位横向溢出源头）、`testing-switch-cdp.mjs`（开关与伪标签）、
  `cfg-overflow-cdp.mjs`、`embdsel-desc-cdp.mjs`。


### Added

- **`/agent-templates` 模板画廊**：新增模板元数据 fixture（25 条，取自上游
  `agent/templates/*.json` 的本地化 title/description + canvas_type/canvas_category）、
  `GET /api/v1/agents/templates` 与 `/templates/{id}` 端点，以及上游组合的页面
  （分类侧栏 + 搜索 + 模板卡片网格 + hover「使用」遮罩 + Use 弹层按模板 DSL 建 agent
  后跳 `/agent/{id}`）。CDP 实点：25 模板 / 分类切换 3→10 卡 / Use 弹层预填。

### Added

- **`/login` 3D 翻转卡片**：`FlipCard3D` 结构（三 JS/CSS 对齐上游：perspective 舞台、正反两面、
  `rotate-y-180` 翻转、`backface-visibility:hidden`、活动面 `data-testid='auth-card-active'`、
  200 ms 切面），新增 `Remember me`（邮箱本地记忆 + 请求带 `remember`）与
  `Sign in with {channel}` 渠道按钮区（`/api/v1/auth/login/channels`，无渠道自动隐藏）。

### Added

- **数据集详情改上游 route 导航**：侧栏五项（Files / Retrieval testing / Logs /
  Configuration / Knowledge graph）改为真实路由链接 `/dataset/{tab}/{id}`，活动项按路径判定；
  解析日志面板并入 Logs 面板；`/dataset/{id}` 307 跳 `/dataset/files/{id}`。
  **修复**：面板脚本引用未定义的 `kbId` 导致 `showTab` 从不执行（所有路由都停在 Files 面板）、
  以及 `showTab` 正则的 Rust 字符串转义错误。

### Added

- **数据集配置页 Basic 字段对齐上游**：新增 `Language`（17 种语言，`ds-settings-basic-language-select`）
  与 `Description` 字段，字段顺序调整为 Name → Language → Avatar → Description → Permission →
  Embedding model → Page rank → Tag sets（上游 `general-form.tsx`），保存按钮加
  `ds-settings-basic-save-btn`，`PUT /api/v1/datasets/{id}` payload 增加 `description`。

### Fixed

- **模型提供商弹层字段顺序对齐上游**：`Instance name → API-Key → Base-Url`（原为
  Instance name → Base url → API-Key），标签统一为 `Base-Url`，去掉标题行重复的文档
  链接；同时给该链接的 JS 加空值保护，避免打开弹层时 TypeError（同类问题此前在
  `/files` 出现过）。

### Changed

- **`/user-setting/model` 收敛为上游三段结构**：移除自造页标题与 `Reranker` /
  `RAG Chat Test` / provider administration 区块（整体迁至 `/admin`），
  `Available models` 标题去掉计数后缀，提供商卡按钮统一为上游的 `Add`。


- **`/memories` 列表页对齐上游**：共享渲染器新增 `form_extra` / `page_script` /
  `lpExtraPayload()` 三个页面级挂钩，`/memories` 渲染上游组合（ListFilterBar +
  记忆卡片网格 + 分页 + 两分支空态 + ⋮ Rename/Delete），创建弹层按
  `createMemoryFields` 提供 Name、Memory type（Raw 固定勾选）、Embedding model 与 LLM
  （共享默认模型树，按 `/api/v1/models/default` 播种）；CDP 实点并截图。


- **`/agents` 列表页对齐上游**：共享 `AppListSpec` 扩展出「创建下拉菜单 / FlowType 卡片 /
  额外 ⋮ 项 / JSON 导入」四个能力，`/agents` 渲染上游组合——Create agent 下拉
  （Create from blank / Create from template → `/agent-templates` / Import JSON file）、
  创建弹层（Name + Agent flow / Ingestion pipeline 卡片）、卡片 ⋮（Rename / Edit tags /
  Delete → `Delete agent` 二次确认）与分页；CDP 实点四步并截图。


- **`/chats` 与 `/searches` 列表页对齐上游**：抽出共享渲染器 `AppListSpec` +
  `render_app_list_page`，两页复用上游组合（ListFilterBar = 标题/搜索/`Create chat`、
  `Create search` 主按钮 → 卡片网格（字母头像/名称/描述或更新时间 + ⋮）→ 分页页脚 →
  两分支空态；⋮ = Rename（共享弹层）/ Delete（`Delete chat`、`Delete search` 二次确认））。
  CDP 实点两页的创建弹层、⋮ 菜单、删除弹层并截图；`web::` 58 passed、lib 1446 passed。


- **`/files` 文件管理页对齐上游 `pages/files/index.tsx`**：ListFilterBar（Files 图标 +
  标题 + 搜索 + `Add file` 下拉 = Upload file / New folder）、选中后出现的批量栏
  （Move / Link to dataset / Delete）、上游六列表格（全选、Name、Upload date、Size、
  Dataset、Action）与分页页脚（总共 N + 50/20/100 + ‹ ›），新建文件夹/移动/链接知识库
  三个弹层。**修复**：旧脚本引用已删除的 `addFileBtn` 抛错导致 `loadF()` 不执行、
  表格永挂 "Loading..."；顺带修 `.bulk-bar[hidden]` 被 `display:flex` 覆盖的问题，
  并把头部外链环境变量测试改为注入式解析器（消除并行测试竞态）。


- **`/datasets` 列表页对齐上游 `pages/datasets/index.tsx`**：ListFilterBar
  （标题 + 搜索 + `Create dataset` 主按钮）、卡片网格（字母头像/名称/`N files`/⋮ 菜单）、
  `RAGFlowPagination` 式页脚（共 N 条 + 每页 12/24/48 + 上一页/下一页）与两种空态
  （无数据无关键词 → 居中虚线空卡可点击创建；搜索无结果 → 保留工具栏、空卡不可点）。
  创建弹层 = Name + Embedding model（共享默认模型树，默认取租户默认 embedding，
  仅随 POST body 提交）+ Parse type（Built-in/Pipeline）+ Chunking method + Save/Close；
  ⋮ 菜单 = Rename（弹层）/ Delete（`Delete dataset` 二次确认弹层）。⋮ 字形改内联 SVG。


- **首页 `/` 对齐上游 `pages/home/index.tsx`**（原为 308 跳 `/dashboard`）：欢迎横幅
  （`header.welcome` + 渐变品牌字）、Dataset 区（最多 6 张卡 + `common.seeAll` 卡，
  空态为 `empty.datasetTitle` 虚线卡）、Applications 区（Chat/Search/Agent/Memory
  分段控件 `role=tablist`/`aria-selected`，标题与卡行随选项切换，数据来自
  `/api/v1/chats|searchapps|agents|memories`），每类空态用上游 en/zh `empty.*` 文案；
  登录/注册成功后落到 `/`（与上游一致）。CDP 逐 Tab 实点验证（`home-dump-cdp.mjs`）。


- **全局头部对齐 RAGFlow `layouts/components/header.tsx` + `global-navbar.tsx`**：
  导航胶囊只保留上游七个入口（Home 图标 → `/`、Dataset `/datasets`、
  Chat `/chats`、Search `/searches`、Agent `/agents`、Memory `/memories`、
  File `/files`，含 `nav-chat`/`nav-search`/`nav-agent` test id 与
  `aria-current='page'` 活动态）；右侧簇改为 Discord、GitHub、语言下拉菜单
  （当前语言 + ⌄，点击展开、点击外部/Esc 关闭、`role=menuitemradio`）、
  用户手册外链、主题按钮、头像 → `/user-setting`。原先挂在头部的
  Dashboard/数据源/Skills/模型提供商入口全部移入头像之后的用户设置侧栏（与
  上游一致）。外链默认与上游逐字相同，可用 `RAYRAG_DISCORD_URL` /
  `RAYRAG_GITHUB_URL` / `RAYRAG_HELP_URL` 指向大陆可达镜像。
- **用户设置侧栏对齐上游 `pages/user-setting/sidebar/index.tsx`**：七项顺序与
  `setting.*` 标签（含 Profile=概要）、上游 Lucide 图标（server /
  messages-square / box / plug / users / user / unplug）内联 SVG、底部
  版本号 + 日/月 `ThemeSwitch` 药丸（`role=switch` + `aria-checked`）+
  `Log out`/登出 按钮。
- **`/login` 独立页面壳**（上游 `login-next/index.tsx` 无全局头部）：新增
  `layout_plain`，登录/注册页不再渲染导航、不再触发用户信息请求。
- **Docker 构建双网络适配**：新增 `docker/mirror-setup.sh` 与
  `RAYRAG_MIRROR_PROFILE=auto|cn|global`——`auto` 先探测清华镜像再决定；
  `cn` 走清华 apt + rsproxy crates.io + ghfast.top GitHub 代理，`global` 直连
  deb.debian.org / crates.io / github.com；zvec 预编译库按 profile 决定
  xget 与 GitHub 的先后顺序（另一侧始终作为回退）。
- **`vendor/zvec-rust/`（v0.7.1 两 crate 逐字副本 + `VENDOR.md` 刷新步骤）**：
  Docker 构建用 `[patch]` 指向本地副本，镜像构建不再 clone 上游仓库及其
  C++ 子模块，构建可离线/可复现；crates.io 版本与 `--locked` 语义仅在
  本地（非 Docker）构建保留。

### Fixed

- **登录页无限刷新**：全局头部脚本会在每个页面拉取 `/api/v1/user/info`，
  配合 401 跳转在 `/login` 上形成「刷新 → 401 → 跳 /login → 刷新」死循环。
  修复为：登录页使用独立壳（无头部脚本）；头像请求要求已登录且不在
  `/login`；401 跳转在 `/login` 上短路。
- **启动日志中立化**：删除含他家产品名与具体部署地址的日志
  （原 `Not using elasticsearch as doc engine…` 改为描述自身行为的 debug；
  embedding 端点从 info 降到 debug，info 只报模型名）。

### Changed

- 新增 3 个头部单测（导航顺序/test id/右侧簇/外链环境变量覆盖）与
  `header-parity-cdp.mjs`（对 RAGFlow 与 RayRAG 跑同一采集器）、
  `login-loop-cdp.mjs`（无限刷新回归）；`web::` 54 通过、lib 1442 通过。
- 台账：`header.tsx`、`global-navbar.tsx`、`theme-button.tsx` 由 reference
  改判 aligned，`user-setting/sidebar/index.tsx` 由 partial 改判 aligned，
  `login-next/index.tsx` 记为 partial；汇总 aligned 208 → 212。

- **zvec 0.5.1 → 0.7.1**：`Cargo.toml` 以 git tag `v0.7.1` 锁定
  `zvec-ai/zvec-rust`（该 patch 版本尚未发布到 crates.io，0.7.0 为最新已发布版），
  `Dockerfile` `ZVEC_RUST_VERSION=0.7.1` 拉取同名预编译 `libzvec_c_api.so`
  （含 `data/jieba_dict/`）。0.7.1 的 Rust API 与既有用法完全兼容
  （`--locked --all-targets` 0 错；1439 测试全绿），无需源码改动。
- **构建标识**：新增 `build.rs` 注入 git rev / 工作树脏标记 / 构建时间，
  `src/lib.rs::build_info` 暴露 `VERSION` + `PARITY_SLICE`；启动横幅与
  `/api/v1/version`、`/api/v1/system`、`/api/v1/admin/version`、用户设置页脚
  统一显示 `v0.3.4 (parity v0.3.4u, rev …, built …)`（此前恒为 0.1.2，
  新旧二进制无法区分）。
- **`/search` 检索页 rerank 模型树选择器**（上游 `next-search/hooks.ts` +
  `search-setting.tsx` 的 `use_rerank` + `rerank_id` 段）：rerank 开关展开共享
  默认模型树选择器（`SEARCH_PAGE_RERANK_SELECTOR_FIELD`），选中值随
  `POST /api/v1/retrieval` body 的 `rerank_id` 下发；`/search?sapp={id}` 打开时
  由 `__SAPP__.rerank_id` 反向播种。该选择器保持表单本地态（上游
  react-hook-form 语义），不写租户默认模型。
- **RAGFlow UI 遍历工具链**（`scripts/ui-traversal/`）：`ragflow-auth-cdp.mjs`
  （翻转卡片登录/注册自举，会话持久化于 fnOS Chrome profile）、
  `ragflow-ui-dump.mjs`（逐路由导出按钮/链接/字段/表头/标签页 + 截图，产物
  `docs/ui-traversal/ragflow-pages/`），作为 RayRAG UI 一比一对标的结构化基准。

### Changed

- `RayRAG/<version>` User-Agent 统一取自 `CARGO_PKG_VERSION`（原先硬编码
  0.1.2）；crate 版本 0.1.2 → 0.3.4；`FORM_LOCAL_SAVE_MODEL_DEFAULT` 抽为
  共享常量（`/search` 与 memory 设置页同源）。

## [0.3.4d] — 2026-08-19（未发版累积批次 v0.3.3ba–v0.3.4c）

### Added

- **聊天应用公共分享页**：`/chats/share?shared_id=&from=`（免登录，纯 Rust SSR），
  `GET /api/v1/chatbots/{id}/info` + `POST .../completions` 免登录 SSE 端点，
  对齐 RAGFlow share/widget 页；embed-dialog 式 `auth/release` 参数。
- **统一命令超时输入**：`RAYRAG_CMD_TIMEOUT`（用户 env 环境文件，默认 7200s）钳制全部外呼客户端与沙箱超时。
- **Data source 详情页**逐源动态字段集、Add 弹层 schema 驱动创建、列表页 35 卡分组 + Sync Logs 表
  （1:1 RagFlow 列/徽章/分页），MCP 编辑对话框与卡片操作，Provider 模态框 API 面补齐（provider_api_service.py 对齐）。
- Google Drive/Gmail web-OAuth token 字段 + start/callback/result 端点；Box web-OAuth 同套流程。
- ChatChannelKey 23 渠道目录全量移植；reasoning/thinking toggle 端到端；chat settings drawer（rerank/reasoning/refmeta）。
- meta_data_filter 表单（DatasetMetadata 4-method）+ AirTable/Box/RSS 等连接器去 Local stand-in。

### Fixed

- RAGFlow 前端产物边界清理（web/static/vs 等非必要静态文件移除，873 files → 精简）；
  CDP 交互遍历基准 35 路由固化。

## [0.3.3] — 2026-08-06

### Added

- **Provider 模型目录 20 → 60**：从运行中的 RAGFlow v0.26.2 容器
  `/ragflow/conf/models/` 整目录提取 60 个提供商 fixture（verbatim mirror），
  新增 40 个缺失提供商（anthropic/azure-openai/bedrock/hunyuan/xunfei/
  baichuan/stepfun/togetherai/perplexity/modelscope/xinference/mineru/
  paddleocr/jina/voyage/groq/mistral/replicate/novita/upstage/xiaomi/qiniu/
  deepinfra/gpustack 等），共 556 个模型条目；`model_meta.rs` 注册表
  `PROVIDER_MODEL_FIXTURES` 扩至 60 条 + `resolve_model_factory` 别名
  从 11 扩至 50（覆盖 Claude/混元/讯飞/百川/Azure/Bedrock/华为云等）。
- **RAGFlow Web UI 清单** `docs/ragflow-ui-inventory.md`：真实浏览器
  登录逐页实测，记录 15+ 页面层级的按钮/路由（登录/注册/首页/Dataset 列表/
  知识库 4 Tab/Chat/Search/Agent/Memory/File/用户设置 8 菜单 + 35 数据源 +
  Model providers 类型筛选 + API 文档页）——RayRAG 前端对齐的 checklist。

### Fixed

- 适配 60 fixture 的过时断言（catalog.len 20→60、端点覆盖 ≥50）。
- 自托管引擎（ollama/vllm/lmstudio/localai 等）无默认端点属正常，
  测试断言放宽而非强制补默认值。

### Added (follow-up)

- **PROVIDER_PRESETS 64 → 69**：新增 Xiaomi（MiMo，国内直连）、Qiniu
  （七牛云）、HuaweiCloud（ModelArts MaaS）、TokenHub（聚合中转）、
  OrcaRouter（模型路由）五个预设；新增 `FIXTURE_TO_PRESET` 映射表
  （24 条 fixture key → factory id），`provider_preset` 支持 fixture
  键回退解析，`provider_is_domestic("aliyun"/"hunyuan"/...)` 等语义
  修正为正确标记国内直连。
- **内嵌 llm_factories.json 63 → 65 工厂 / 1064 模型**：从运行中
  RAGFlow v0.26.2 容器同步最新工厂目录（补 Xiaomi、New API），
  转换回紧凑 `{n,t,mx,tools}` 格式并全校验。
- **Providers 页对齐 RAGFlow Model providers**：能力类型筛选
  （All/LLM/Embedding/Rerank/TTS/ASR/VLM/OCR + 计数）、每行
  `data-k` 类型属性、`+ Add` 按钮 + 添加提供商弹窗（POST
  /api/v1/providers/{id}，对齐 RAGFlow Add 流程）。

## [0.3.2] — 2026-08-05

### Fixed

- **OCR 解析链路打通**：FigureParser::new() 惰性接入 OcrClient（.env
  RAYRAG_OCR_PROVIDER=proxy + RAYRAG_OCR_BASE_URL）；修复 async 上下文
  内再建 runtime 的 panic（figure.rs:514 "Cannot start a runtime from
  within a runtime" → block_in_place + Handle::current().block_on）。
- 验证：PNG 上传→OCR 提取文字→索引→检索命中（score 0.902，content
  含 OCR 文字）。四个 GPU 服务（LLM/embedding/reranker/OCR）全部打通。

## [0.3.1] — 2026-08-05

### UI 按钮逐项核对补齐（RAGFlow JSX vs RayRAG 渲染 diff）

- 补 12 个按钮事件：batchDelSel/batchLinkSel（文档批量删除/关联）、
  clearConv/tempConv（清空/临时对话）、emptyAllDocs（清空文档）、
  exportAgentJson/exportChunks（导出 agent JSON/chunk）、inviteUser
  （邀请用户）、openVersions（版本）、sendTemp（临时发送）、
  showTemplates（模板）、uploadF（上传文件）
- web.rs +169/-12；新增 8 个 UI 测试（含按钮 diff 断言）

## [0.3.0] — 2026-08-05

### UI：页面按钮全面对齐 RAGFlow（web/src/pages 19 页面层级）

- **知识库组**：列表页创建按钮/搜索/排序/卡片操作菜单（重命名/导出/删除）
  + 弹窗；详情页文档表格（状态/进度/chunk 数）+ 批量操作栏（解析/禁用/
  删除）+ 上传按钮；dataset 5 tab（文档/配置/检索测试/chunk/概览）+ 配置
  表单（chunk 方法下拉/embedding/检索参数）+ 保存按钮 + 检索测试（输入/
  滑块/结果卡片）。
- **对话检索组**：对话页新建/重命名/删除/分享 + 消息区（发送/停止）+ 引用
  折叠；检索页知识库选择器/过滤/结果卡片；记忆页添加/编辑/删除；404 页。
- **Agent 管理组**：agents 列表新建/卡片操作；agent 详情运行/保存/发布 +
  组件面板 + 日志区；admin 用户表格 + 添加/删除/密码重置；files 上传/下载/
  删除；skills 安装/卸载；settings API Key 生成/删除。
- 所有按钮接现有 API（HTML + JS 事件）；web.rs +796 / server.rs +198 /
  kb.rs +28；新增 6 个 UI 渲染测试。

## [0.2.5] — 2026-08-05

### Changed

- **覆盖矩阵最终收尾**（纯文档，`docs/ragflow-v0.26.4-coverage.md`）：矩阵终态
  aligned 128 / full 541 / partial 18 / replaced 92 / N/A 61 / reference 3601 /
  unmapped 0（4441 文件快照）。摘要表按清单逐行重算，与库存行完全一致。
  agent/sandbox 22 个非代码文件、deepdoc/README×2、deepdoc/__init__.py、
  admin/build_cli_release.sh 由 partial 转为 N/A-by-design（Python 沙箱镜像/
  文档/构建脚本不属于 Rust 移植范围；等价语义在 src/sandbox.rs + Dockerfile
  多阶段构建中表达）。

## [0.2.5] — 2026-08-05

### Added

- **conf/ 配置处理**：all_models 逐字节一致核对（+27 个新模型回归锁定：
  gpt-5.2-pro/deepseek-v4-flash/glm-5/kimi-k2.6/minimax-m2.7/grok-4 等）、
  llm_factories 工厂映射（FactoryLlmInfo/factory_endpoint）、system_settings
  种子（14 行常量 + coerce_setting_value）、ServiceConf（Mysql/Minio/Es/Os/
  Infinity/Redis/UserDefaultLlm 子结构 + apply_to_settings）。
- **MCP SSE 服务器**（`src/mcp_server.rs` 新建）：ToolRegistry（ragflow_retrieval/
  search_knowledge/list_datasets）、McpServerCore（JSON-RPC 2.0 dispatch：
  initialize/notifications/ping/tools/list/tools/call，-32600/-32601/-32602 错误码）、
  RetrievalBackend trait + InMemory 后端；接入主 router `/mcp/sse` + `/mcp/messages`。
- **消息记忆服务**（`src/memory.rs` + `src/api/joint_services.rs`）：消息读写/查询、
  中文双引号正则对齐（Unicode 弯引号）。
- **矩阵终态**：partial 44 → **15**；sandbox 非代码 22 → N/A-by-design（等价语义
  在 src/sandbox.rs seccomp/超时 + Dockerfile）；whatsapp Node 网关/Go CLI → N/A。

## [0.2.4] — 2026-08-05

### Added

- **common 工具层**（`src/common.rs` ~1800 行）：常量表（RetCode/TaskStatus/
  StatusEnum/ParserType 15 种 chunk method/FileSource 31 种/LLMType/Storage/
  MemoryType 位标志）、string/text/time/float/misc/parser_config/query_base/
  connection_utils、crypto（AES-128/256-CBC + PKCS7 + RAGF magic 头 +
  PBKDF2-HMAC-SHA256，与 Python wire format 逐字节一致；SM4 标记 TODO）、
  ssrf_guard、参数校验；api/constants + api/validation 语义。
- **settings 层**（`src/settings.rs` 新建）：Settings::from_env/from_reader、
  StorageImpl/DocEngine 枚举、parse_model_entry（name@factory）、init_secret_key
  （≥32 字符）、FLOAT_ZERO/PARAM_MAXDEPTH。
- **benchmark**（`src/benchmark.rs` 新建）：BenchmarkDataset/Config、ndcg@10/
  map@5/mrr@10（rank-ordered）、LatencyStats（avg/p50/p95/max）、
  RetrievalRunner + run_retrieval_benchmark、to_markdown。
- **canvas 语义**（`src/agent.rs` 追加）：reset_sys_globals/reset_env_globals、
  get_history_window、is_canvas_reference、add_retrieval_reference、
  ToolUseTrace + merge_tool_use_trace、clean_tts_text。

## [0.2.3] — 2026-08-05

### Added

- **flow 组件契约**（`src/pipeline/flow.rs` ~700 行）：FromUpstream/check_payloads
  （alias 反序列化）、OutputFormat、ComponentOutput（ProcessBase 语义）、
  file_outputs、FlowLog/TraceEntry/ComponentTrace（trace 累积 + 加权进度）、
  ChunkDoc（REST 契约 + From<&Chunk> 转换）。
- **resume 实体资源修复**（`src/resume.rs`）：2 个 regex 崩溃 bug
  （`[—-]+` 非法区间、反斜杠转义层级）+ CORP_TKS 贪心最长匹配（对齐 jieba）。
- **服务层语义**（`src/api/joint_services.rs` +700）：seed_system_settings、
  SuperuserSeedPlan、LLM 工厂种子/清理（novita.ai 删除/QAnything→Youdao）、
  tenant_model 修复计划、canvas 模板规范化、langfuse/mcp_server/conversation/
  task 服务（SyncLog 单调计数 + 100 条裁剪 + resume 状态机）。
- **MCP 客户端**（`src/mcp_client.rs`）：SSE + Streamable HTTP 传输、
  initialize/list_tools/call_tool。
- **Helm chart**（`helm/` 新建）：最小可用 chart（deployment/service/configmap，
  镜像 rayrag:local，9390→8080，外部 PG18 依赖说明）；helm lint/template 通过。

## [0.2.2] — 2026-08-05

### Added

- **api/common**（`src/api/common.rs`）：ApiError/ApiErrorKind（400/403/404/409/500
  信封输出 `{"code","message"}`）、ok_json、base64、TeamPermission 检查。
- **运行时配置**（`src/api/runtime_config.rs`）：RwLock 热更新（init/get/get_env/
  set_env/load_config_manager/set_service_db，对齐 reload_config_base）。
- **联合服务**：memory_message（消息记忆读写）/tenant_model（租户模型绑定）/
  user_account 语义补入 api 层。
- **dialog.rs**：is_aggregate_sql/is_row_count_question/add_kb_filter（UUID 注入
  防护 + RAGFlow 两处怪癖忠实复刻）。
- **工具契约核对**（`src/runtime.rs` +278 / `src/agent.rs` +34）：17/19 工具已对齐；
  JIN10_INPUTS 补 7 参数、QWEATHER 补 web_apikey、retrieval 补 15 输入 4 输出、
  **wencai 别名**（canvas component_name=WenCai 此前匹配失败）。
- **资源加载器**（`src/resources.rs` 新建）：ner.json（235KB）+ synonym.json
  （268KB）磁盘加载（RAYRAG_RES_PATH 环境解析、缺失降级空表、值→键镜像）；
  CvModel 页面转写 prompt + figure 描述调用流补全。

## [0.2.1] — 2026-08-05

### Added

- **远程沙箱契约**（`src/sandbox.rs` +456）：RemoteSandboxClient/Config（task 提交/
  结果拉取、超时=执行超时、重试、/healthz）、CodeExecutionResult 系列、
  unsafe/unhandled_exception 失败分支。
- **插件机制**（`src/plugin.rs` 新建）：PluginManager（发现/加载/执行、env 注入、
  错误隔离）、LLM 工具插件（OpenAI tool 转换）、内置插件 1-2 个。
- **NLP 工具**（`src/nlp.rs` +658）：rag_tokenize（CJK 单字+二元组）、全角→半角、
  ~450 对繁→简映射（替代 OpenCC）、sub_special_char、QueryBase（rm_www/
  normalize_query 管线）、TermWeightComputer（NER×postag）。
- **LLM 工厂**（`src/llm.rs` +655）：ModelKind 7 类 + Chat/Ocr/Cv/Seq2txt/Tts
  trait 抽象 + OpenAi 系列实现（vision/audio/tts 多模态 payload）。
- **RAPTOR 检索**（`src/advanced_rag.rs`）：问题分解→子检索→汇总（复用已有
  Retrieval/Chunk 组件）。
- **doc_store 抽象**（`src/doc_store.rs` 新建）：DocStore trait（insert/delete/
  search/update/sql 全契约）+ Memory/Postgres/Zvec 三后端 + 连接池
  （es_conn_pool 语义：2 次重试/5s 间隔/按 URL 选后端）。
- **memory utils**（`src/memory.rs` 新建）：highlight_text（按句切分/<em> 标记）、
  aggregate_by_field。
- **admin 端点**：users CRUD/keys/activate/admin 授权/version/ping（对齐
  admin/server/routes.py）。

### Fixed

- `/api/v1/admin/users/{username}` 与既有 `{user_id}` 路由冲突 → 删除旧
  重复路由与 handler（新路由功能更全）。

## [0.2.0] — 2026-08-05

### Added

- **剩余 prompts 全部移植**（`src/prompts.rs` +1390 行）：26 个常量——
  related_question、resume 系列双语（system/basic_info/education/project_exp/
  work_exp）、structured_output/sufficiency_check/summary4memory/tool_call_summary、
  TOC 管线（detection/extraction/continue/index/relevance×2）、vision describe 系列
  （vision_llm_describe/figure_describe×2，Jinja 语法转 {var} 约定）。
- **问答模式分派**（`src/app_parsers.rs`）：naive/advanced/graph 模式选择逻辑
  （显式标记优先、全 KG 才走 graph）。
- **api/utils 服务层**（`src/api/utils.rs` ~600 行）：FileType/filename_type
  （扩展名逐字对齐）、sanitize_path、常量（255 限制/50MiB/100MiB）、
  hash128（xxh3）/md5 hex、get_parser_config（chunk_method 默认 + 递归合并）、
  init_dataset_defaults（parser_id=naive、llm_id 注入）。
- **解析器行为补齐**：`excel.rs`（多 sheet `表头:值;` 行格式、sheet_html 分块、
  >10000 行二分、row_number 计数）、`docx.rs`（blip rId→rels→media 图片提取）、
  `paddleocr_layout.rs`（table/figure/image 区域分组 + 位置 tag）。

### Fixed

- channels 测试断言 JSON map 遍历顺序不稳定 → 集合匹配（顺序无关）。

## [0.1.9] — 2026-08-05

### Added

- **连接器框架**（`src/connectors.rs` ~1480 行，对标 `common/data_source`）：
  Connector trait（load_credentials/list_files/fetch_file/normalize/fetch_all 分批
  runner）、RemoteFile/SyncBatch 契约、LocalConnector/WebDavConnector/BlobAdapter
  基础连接器 + FeishuConnector（tenant_access_token 授权、云空间递归列举、
  docx raw_content 导出）/ ConfluenceConnector（spaceKey 分页、HTML→markdown）；
  ConnectorRegistry 按类型分发；datasources 页面连接/断开/拉取按钮。
- **渠道框架**（`src/channels.rs` ~1200 行，对标 `api/channels`）：
  Channel trait（send_text/send_markdown/start/stop/dispatch 错误隔离）、
  ChannelRegistry（register/build_channels/start_all/stop_all/dispatch_inbound，
  bootstrap 语义）、webhook 渠道（OutgoingMessage 契约 POST）+ feishu 渠道
  （tenant_access_token 缓存、text/interactive-card 发送、v2.0 事件归一化 +
  URL verification challenge）；server.rs 启动/停止钩子。
- **rag/svr 语义补齐**：`src/api/file_mgr.rs` +236（DocFileCache：Redis 式
  `{kb_id}/{location}` 键、TTL 过期清理）；`src/task_executor.rs` +960
  （SyncTask 同步任务类型：fingerprint 跳过、poll_range_start 窗口推进、
  快照删除对账、超时永久 FAIL；ChunkEnrichment：auto_questions 两阶段 LLM
  缓存 + 离线 fallback、important_kwd/question_kwd 元数据、进度消息对齐）。

## [0.1.8] — 2026-08-05

### Added

- **TaskExecutor**（`src/task_executor.rs`，~680 行，对标 `task_executor.py`）：
  异步任务队列（tokio::mpsc + Semaphore 限流）、状态机 Pending→Processing→
  Done/Failed（对齐 RAGFlow doc 状态 UNSTARTED/RUNNING/DONE/FAILED）、
  进度回调（set_progress：`[ERROR]` 前缀/`Page(a~b):` 页前缀/HH:MM:SS 时间戳）、
  失败重试（retryable 标记 + 指数退避 + max_retries）、status/stats 上报、
  submit/shutdown 生命周期；接入 document.rs 上传队列。
- **ChatPipeline**（`src/chat.rs`，835 行，对标 `generator.py`）：
  问题改写（full_question：绝对 ISO 日期 + USER:/ASSISTANT: 会话，**ERROR** 回退）、
  多路查询（multi_queries_gen：<2 命中时生成 2-3 路补充查询，JSON 解析容错）、
  引用格式化（citation_plus 二次 LLM 生成 + repair_bad_citation_formats 规范化
  [ID:n] + references 按被引过滤）、format_knowledge（kb_prompt 树形渲染）。

## [0.1.7] — 2026-08-05

### Added

- **RAG 问答阶段 prompts**（`src/prompts.rs` +1194 行）：18 个 RAGFLOW_* 常量
  （citation/citation_plus/full_question/multi_queries_gen/next_step/reflect/
  rank_memory/content_tagging/cross_languages×2/analyze_task×2/ask_summary/
  assign_toc_levels/keyword/question_prompt/meta_data/meta_filter），注册
  PromptLibrary get()/list()。
- **graph_extractor NER 语义**（`src/structure_compile.rs` +1033 行）：实体/关系
  JSON 解析容错（LLM 非严格输出提取/修复）、upsert 语义（if not exists）、
  general/light extractor 剩余常量。
- **组件参数契约**（`src/runtime.rs`）：新增 iteration/iterationitem/exitloop/
  loopitem descriptor；扩展 categorize/listoperations/docsgenerator/excelprocessor
  参数（对齐 RAGFlow Param schema 的 camelCase 语义）。

## [0.1.6] — 2026-08-05

### Added

- **translate 工具**（`src/translate.rs` + `src/runtime.rs`）：注册 translate descriptor
  （对齐 RAGFlow deepl 参数契约），百度翻译 VIP 签名 API + 免 key sug 降级双路径
  （大陆可用）；3 个 mock-HTTP 测试（SUG 解析/VIP MD5 签名表单校验/错误码透传）。
- **KeyTerm Extractor**（`src/extractor.rs`，402 行）：RAGFlow KeyTermExtractor 语义的
  统计实现——词频/位置/POS 代理权重、CJK n-gram（跳过虚词开头、单字过滤）、
  停用词表；`extract_key_terms` / `extract_key_terms_with_top_n` / chunk 批量接口；
  pipeline 可选集成。
- **title_chunker.py 主文件语义**：标题连续性检查/章节序号处理补入 `src/chunk/title.rs`。

### Fixed

- extractor CJK 测试断言（默认 top-N 截断遮蔽 4 字短语 → 显式 wide top_n 验证）。

## [0.1.5] — 2026-08-05

### Added

- **Provider Model Catalog**（`src/model_meta.rs` + `src/api/fixtures/models/*.json`）：
  RAGFlow conf/models 全部 20 家厂商 / **157 个静态模型**种子化（include_str! 内嵌）；
  `list_models_for` / `resolve_model_factory`（zhipu→zhipu-ai、qwen→aliyun 别名）/
  `provider_models_payload`；providers 页面新增 Model Catalog 卡片 + 点击弹窗
  （模型名 + 能力 badge：chat/embedding/rerank/image2text/thinking）。
- **沙箱 executor_manager 契约补齐**（`src/sandbox.rs` +1031 行）：
  SupportLanguage/ResultStatus/ResourceLimitType/UnauthorizedAccessType/RuntimeErrorType
  枚举、失败协议（exit code + stderr 分类）、运行时环境约定（容器池/镜像/run 参数/
  bundle）、artifact 契约、安全校验契约。
- **GraphRAG 管线补齐**（`src/structure_compile.rs` +321 行、`src/prompts.rs` +116 行）：
  phase_markers 阶段标记（PHASE_RESOLUTION/PHASE_COMMUNITY、`graphrag:phase:{kb}:{phase}`
  键格式、7d TTL）、实体消解确定性门控（levenshtein/2gram 数字差/字符集重叠、
  entity_resolution_candidates 分组排序、parse_resolution_results）、light graph
  默认常量（LIGHTRAG_DEFAULT_LANGUAGE/TUPLE_DELIMITER/ENTITY_TYPES/FAIL_RESPONSE）。

### Fixed

- 子 agent 并发期间 prompts.rs 临时借用 E0716（I 已修，测试全绿）。

## [0.1.4] — 2026-08-05

### Added

- **RAGFlow 25 个 Agent 工作流模板种子化**（`src/agent_templates.rs` + `src/api/fixtures/agent_templates/*.json`）：
  include_str! 编译期嵌入（零转义风险），agents 页面模板库 3 → **28**（web_search_assistant、
  deep_research、text2sql_data_expert、trip_planner、ingestion_pipeline_* 系列等）。
- **GraphRAG prompts 移植**（`src/prompts.rs`，18 个常量 + PromptLibrary 方法）：
  GRAPH_EXTRACTION_PROMPT/CONTINUE/LOOP、GRAPH_SUMMARIZE_DESCRIPTIONS_PROMPT、
  GRAPH_COMMUNITY_REPORT_PROMPT、ENTITY_EXTRACTION_PROMPT 系列、KEYWORDS_EXTRACTION_PROMPT、
  LIGHTRAG_RESPONSE_PROMPT、NAIVE_RAG_RESPONSE_PROMPT、QUERY_KEYWORDS_EXTRACTION_PROMPT、
  MINIRAG_QUERY2KWD_PROMPT、ENTITY_RESOLUTION_PROMPT、MIND_MAP_EXTRACTION_PROMPT、
  QUESTION_PROPOSAL_PROMPT、KEYWORD_EXTRACTION_PROMPT。
- **paddleocr_layout 补齐**（`src/parser/paddleocr_layout.rs`，66 行 stub → 完整 layout 流程）：
  blocks→bbox→position tags、parse_methods、extract_positions（对齐 paddleocr_parser.py）。
- **figure.rs 补齐**：figure-describe prompts + 上下文感知 describe helper（对齐 figure_parser.py/picture.py）。

### Fixed

- `exact_dedup_by_key` canonical 名称保留组内首现（对齐 RAGFlow 语义，而非频次最高）。
- `parse_run_response` 缺省 result 时始终注入 result_present=false / result_value=null / result_type=json。

## [0.1.3] — 2026-08-05

### Added

- **Token Chunker**（`src/chunk/token.rs`，对标 RAGFlow `token_chunker.py`）：
  `one` / `token_size` / `delimiter` 三模式；反引号分隔符编译（最长优先）；
  分隔符并入段尾（re.split capture-group 语义）；重叠率 normalize（0<p<1→×100，
  clamp [0,90]）；children_delimiters 二次拆分（裸模式 escape，`mom` 挂父块原文）。
- **Title Chunker**（`src/chunk/title.rs`，对标 `title_chunker/{hierarchy,group,common}.py`）：
  hierarchy 目标层级合并、group 32/1024 token 常量合并、include_heading_content、
  root_chunk_as_heading；标题来源优先 PDF 大纲 `__outline__` 元数据，fallback 正文
  markdown/枚举标题行解析。
- **iwencai 问财工具**（`src/wencai.rs`，对齐 RAGFlow `agent/tools/wencai.py`）：
  get-robot-data 请求契约（perpage 字符串/page 整数）、xuangu_tableV1 landing
  分页数据接口、多组件 Markdown 渲染、`agent.rs` 工具注册与 provider 执行链。
- **沙箱 executor_manager 远程客户端**（`src/code_exec.rs`）：`parse_run_response`
  缺省 result 元数据兜底（result_present=false / result_value=null / result_type=json）；
  `process_result` 结构化结果优先、stderr-only 错误分支、canonical 渲染。
- **KB 详情页路由** `/kbs/{id}` → `kb_detail_page`（文档/配置/测试/知识图谱 tab，
  对齐 RAGFlow 数据集详情页；与旧 `/dataset/{id}` 并存）。

### Changed

- `Pipeline::new` 按 `config.chunk_method`（naive | token | title）选择 chunker。
- `ParserConfig` 新增 chunk_method / delimiter_mode / delimiters /
  children_delimiters / title_levels / hierarchy / include_heading_content /
  root_chunk_as_heading 字段（serde alias 兼容 RAGFlow camelCase）。
- 修复上一会话遗留 `src/structure_compile.rs` 编译错误（as_any ×2、
  semaphore 借用、Arc 包装、downcast and_then）。

### Fixed

- 子 agent 半成品编译错误（wencai `Map` 类型、code_exec `base64::Engine`、
  agent `SandboxResult` 缺字段）与测试缺陷（重复 bind listener、serde_json
  Map Index panic、process_result 断言语义）。

## [0.1.2] — 2026-08-04

### Added

- **UI 全面对标 RAGFlow**（16 页面 Rust 手写）：dashboard（统计卡片/最近会话）、
  knowledge base（批量上传/删除/清空/重命名）、search（多 KB 过滤/分页/图谱上下文
  卡片/导出 Markdown）、chat（模型选择器/流式/引用/会话管理）、agents（模板库
  3 种工作流/DSL 编辑器/画布拓扑/搜索过滤）、files（搜索）、admin（用户搜索）、
  status（Refresh/Copy Report）、api-docs、login（注册/cookie 会话）
- **Agent 画布完整闭环**：components 格式 DSL（RAGFlow 标准）、模板创建
  （RAG 问答/网络搜索/RAG 对话）、JSON 编辑保存、错误传播（11 个工具函数统一
  formalized_content 注入 + `{Provider} error:` 前缀）
- **OpenAI 兼容层**：`/api/v1/models`、`/api/v1/embeddings`（租户表空回落 env）、
  `/api/v1/chat/completions`（SSE 流式 + references 帧）、`/api/v1/rerank`
  （Jina 风格）
- **GraphRAG 生命周期**：Build（LLM 实体抽取）→ 持久化（graphrag.json 重启保留）
  → Query API → RAG 对话注入 → KB 删除清理
- **SearXNG 自托管**：Docker 部署 + cn.bing 引擎中国适配 + `SEARXNG_URL` 透传
- **性能优化**：zvec 增量 upsert（仅写变更）、`RAYRAG_NO_FSYNC` 开关
  （容器 overlay 上传 53s→37s）、docx 正则静态缓存 + HashMap tie-break
  （偶发测试失败根治）

### Fixed

- docx 并行偶发测试失败（OnceLock 缓存 + 确定性 tie-break）
- Agent 工具静默失败（错误传播到 formalized_content）
- 模板 JS 错位（showTemplates 误入 kbs_page）
- Retrieval 模板字段（formalized_content 非 result）
- 上传性能（容器 overlay fsync 瓶颈 + 增量 upsert）
- 知识图谱不落盘（build 只返回统计）

### Verified (container)

- 844 passed / 0 failed / 26 ignored
- GPU 四链路实连（embedding 8888 / LLM 8088 / rerank 8081 / OCR 8097）
- OpenAI 兼容 4 端点 + Agent 模板 3/3 + SearXNG 成功/失败路径
