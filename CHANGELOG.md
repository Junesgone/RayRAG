# Changelog

All notable changes to RayRAG are documented in this file.

## [0.4.21] — 2026-09-20（对标批次 v0.3.7y）

### Changed

- **zvec 成为默认配置（三个入口统一）**：`zvec-backend` 与 `postgres-backend` 一起进入 Cargo 默认 feature，
  因此纯 Linux 的 `cargo build --release --locked` 就是发行配置（原生 zvec + PostgreSQL 元数据），
  Dockerfile 的 `RAYRAG_FEATURES` 默认值也从 `postgres-backend` 改为 `postgres-backend,zvec-backend`
  （compose、`.env.example`、`install.sh` 早就默认 zvec）。README（中英）与 `docs/advanced.md` 的纯 Linux
  步骤改为先导出 `ZVEC_LIB_DIR` 再 `cargo build --release --locked`，并说明无原生库环境可用
  `--no-default-features --features postgres-backend` 退回 JSON 索引。
- **运行期默认后端改为「编译进来就用 zvec」**：此前 `RAYRAG_VECTOR_BACKEND` 不设时一律走 JSON，即使
  二进制带了原生后端。现在默认值由编译期决定（`default_vector_backend()`：有 `zvec-backend` 用 `zvec`，
  否则 `json`），显式设置仍然优先。新增单测覆盖两条分支。
- **构建时把 zvec 库目录写进 runpath**：`build.rs` 在 `ZVEC_LIB_DIR` 指向含 `libzvec_c_api.so` 的目录时
  注入 `-Wl,-rpath`（依赖 crate 的 `rustc-link-arg` 不会传到本包二进制，此前运行必须手动
  `LD_LIBRARY_PATH`），纯 Linux 与测试因此可直接运行。全量 `cargo test --locked --lib` 现在默认即带
  zvec，1512 passed（比无 feature 时多 7 项原生后端用例）。

## [0.4.20] — 2026-09-20（对标批次 v0.3.7x）

### Added

- **OAuth 回调落地处理对齐上游 `useOAuthCallback`**：上游回调成功后跳 `/?auth={user_id}`，失败跳
  `/?error=…`，由 `hooks/auth-hooks.ts` 在落地页消费（`auth` 写入本地会话并清掉查询参数，`error` 先弹出提示、
  1 秒后回到 `/login` 并清掉参数）。RayRAG 之前回调端已实现（state 校验、换 token、拉用户信息、注册/登录、
  下发 cookie），但落地页不读这两个参数。现在共享外壳（无导航的 `layout_plain` 与带导航的 `layout`）
  都注入同一段 `OAUTH_LANDING_JS`：`auth` 写入 `localStorage.rayrag_token` 并补一次 cookie、清参数后停留；
  `error` 显示一个 `role='alert'` 的浮层提示（`data-testid='oauth-error-toast'`）并跳回 `/login`。

## [0.4.19] — 2026-09-20（对标批次 v0.3.7w）

### Added

- **默认模型选择树支持 Radix 键盘操作**：上游 `components/tree-select.tsx` 基于 Radix `Tree`，
  除已有的 ↑/↓（循环）、Home/End、首字母跳转、Enter 选中、Escape 关闭外，本轮补齐
  **→**（折叠的分支→展开；已展开→进入第一个子项）、**←**（已展开的分支→折叠；否则→跳到父项），
  以及**带 1 秒缓冲区的循环 typeahead**（连续输入累积匹配，同一字母可继续跳到下一个匹配项）。
- **行内 roving tabindex 与 `aria-level`**：树里始终只有一个可 Tab 到达的行（其余 `tabindex=-1`），
  焦点移动时同步更新，因此整棵树是一个 Tab 站点（与 Radix 的树一致），每行还带上 `aria-level`。

## [0.4.18] — 2026-09-20（对标批次 v0.3.7v）

### Added

- **后台「服务状态」页的详情弹层按上游组件渲染**：上游有两套渲染——`admin/service-detail.tsx`
  按载荷形状分支（对象数组 → 以首个对象的键为表头的表格；普通对象 → `键/JSON.stringify(值)` 的定义列表；
  字符串 → 卡片内 `<pre><code>`；其他 → 原样），`admin/task-executor-detail.tsx` 对 `task_executor`
  按执行器分组渲染卡片（标题 + `Lag:`/`Pending:` + done/failed 柱状图 + 图例 + 点击柱子的 JSON 提示）。
  RayRAG 之前把两种载荷都塞进一个 JSON 树里，现在按上游落地：标题也随之变成
  `Task executor detail` 或 `Service {name} detail`。
- **拆成两个弹层并补上行内图标动作**：上游的行操作是悬停显示的 `Settings2`（Extra information，渲染
  `item.extra` 的 JsonView）与 `ClipboardList`（服务详情，`GET /api/v1/admin/services/{id}`）两个图标按钮，
  RayRAG 之前合并成一个 "Show details" 按钮。

## [0.4.17] — 2026-09-20（对标批次 v0.3.7u）

### Added

- **搜索空结果用上游插画**：上游 `components/empty/empty.tsx` 在 `EmptyType.SearchData` 时渲染
  `assets/svg/empty/no-search-data-{dark,bri}.svg`（84×104，`iconWidth={80}`，配 `common.noResults` 文案）。
  RayRAG 之前只有一行 "No results found." 文字；现在把两个主题变体都内联进页面，按现有 `body.light`
  主题类切换（暗色 `no-search-data-dark`／亮色 `no-search-data-bri`），文案沿用 `common.noResults`。

## [0.4.16] — 2026-09-20（对标批次 v0.3.7t）

### Added

- **搜索页的文件筛选改成上游的 `RetrievalDocuments` 浮层**：上游 `next-search/retrieval-documents/index.tsx` 里，
  侧栏的「文件」控件是一个触发器（lucide `Files` 图标 + `已选/总数` + `knowledgeDetails.subbarFiles` 文案，
  右侧 X 清空、竖分隔线、ChevronDown），点开是带搜索框的浮层，逐文档复选框（选中打勾），底部
  `common.clear` / `common.close`；有文档时才渲染。RayRAG 之前只有一个「Document Type」下拉，现在按上游实现，
  并把选中的 `doc_ids` 直接送进 `POST /api/v1/retrieval`（勾选即生效，与上游 `onTesting` 一致）。
  上游没有文档类型过滤，因此该下拉一并移除（结果不再按扩展名二次过滤）。
- 新增 `t()` 命名空间键 `knowledgeDetails.subbarFiles`：它的中文是「文件列表」，与导航 `header.fileManager`
  的「文件」不同，按仓内既有约定用命名空间键区分。

## [0.4.15] — 2026-09-20（对标批次 v0.3.7s）

### Added

- **补齐上游的「用户资产」管理接口**：上游 `admin/server/routes.py` 的
  `GET /api/v1/admin/users/{username}/datasets` 与 `/agents` 此前缺失（详情页的 Agent 页签永远是空的）。
  现在两个接口按 `UserServiceMgr.get_user_datasets` / `get_user_agents` 的语义返回：数据集给出
  id/name/avatar/doc_num/chunk_num/token_num/language/permission/create_date/update_date，智能体给出
  `title` / `permission` / `canvas_category`（只取 `_` 前的第一段）/ `avatar`；未知账号 404 `User not found`，非管理员 403。
- **用户详情接口改为上游的列表形态**：上游 `UserMgr.get_user_details` 返回的是「匹配账号数组」，控制台读
  `data.data[0]`，而 RayRAG 之前返回单个对象。现在同样返回数组，字段名与上游一致
  （avatar/email/language/last_login_time/is_active/is_anonymous/login_channel/status/is_superuser/create_date/update_date），
  另外保留 RayRAG 用户表需要的 id/nickname/role/api_key_count。
- **`/admin/users/{id}` 页面按上游并行取数并分页**：与上游一样 `Promise.all([detail, datasets, agents])` 同时请求三个接口；
  Dataset / Agent 两个页签的表格带上了头像+名称（Agent 为标题）列与页脚分页（`Total N` + 10/20/50/100 + Previous/Next）。

## [0.4.14] — 2026-09-20（对标批次 v0.3.7r）

### Added

- **补齐上游的沙箱提供方管理接口与页面**：上游 `admin/server/routes.py` 的
  `/api/v1/admin/sandbox/{providers,providers/{id}/schema,config,test}` 五个动作此前未实现（RayRAG 只有本机沙箱与
  executor-manager 远程执行）。本轮按 `admin/server/services.py::SandboxMgr` 与 `agent/sandbox/providers/*.py`
  逐字段落地：5 个提供方注册表（local / self_managed / ssh / aliyun_codeinterpreter / e2b，含名称、描述、
  标签）与各自的配置 schema（字段名、标签、说明、默认值、最小/最大值、`secret`、`multiline`、只读部署项，
  共 local 8 / ssh 14 / self_managed 9 / aliyun 6 / e2b 3 项）。配置持久化到系统设置
  `sandbox.provider_type` 与 `sandbox.<provider>`（执行器读的就是这里），密钥沿用 `<redacted>` 哨兵：读接口不回显，
  原样回传即保留原值。
- **连接测试是真的跑代码**：`POST /sandbox/test` 使用上游同一段探针代码（打印 `2 + 2`、`json.dumps`、
  `math.sqrt` 后输出 `TEST_PASSED`），local 走配置的 Python 解释器、self_managed 走 executor-manager 的
  `GET /healthz` + `POST /run`，返回与上游相同的 `{success,message,details:{exit_code,execution_time,stdout,stderr}}`
  与 `Test PASSED | Exit code: 0 | Execution time: …` 文案；ssh 只做 TCP 可达性检查并如实说明本构建不执行远程
  SSH 代码；e2b / aliyun 明确报「本构建未实现该提供方执行」。**不做假成功**。
- **运行镜像内置 python3 与 nodejs**：Code 组件在本机沙箱执行用户代码需要解释器，此前运行镜像（Debian bookworm slim）只有 `curl/libssl3/zlib1g`，
  任何 Python/JavaScript 执行都会以 `No such file or directory` 失败（本机沙箱与 local 提供方的连接测试都会如实报这个错）。现在运行阶段安装
  `python3` 与 `nodejs`（镜像 183MB → 310MB），本机沙箱与 local 提供方开箱可用；也可以在提供方配置里把 `python_bin`/`node_bin` 指向别的解释器。
- **`/admin/sandbox-settings` 页面结构对齐上游**：标题与描述、`Provider selection` 单选卡片网格（图标/名称/
  描述/标签）、`{name} configuration` 卡片（Test connection + Save 配置按钮）、按 schema 生成的字段
  （必填星号、说明、数值上下界、布尔开关、`multiline` 文本域、`secret` 密码框、只读项进
  `Deployment Defaults` 折叠区；SSH 额外有 `Runtime Settings`／`Authentication`（Password / Private Key 切换）／
  `Execution` 分组）以及 `Connection test result` 弹层（成功/失败摘要、退出码、耗时、stdout/stderr）。
  RayRAG 原有的持久变量表保留为独立卡片。

## [0.4.13] — 2026-09-20（对标批次 v0.3.7q）

### Fixed

- **权限字段改用与语言字段同一个上游 `SelectWithSearch`**：上游 `dataset-setting/permission-form-field.tsx`
  与 `general-form.tsx` 用的是同一个组件，而 RayRAG 之前是另一套 `input-select` 实现。现在两处共用一个
  组件（新增 `SWS_JS` 脚本常量 + `push_select_with_search` / `push_permission_field` 渲染函数），
  触发器的 `data-testid='ds-settings-basic-permissions-select'`、`role=combobox` 与选中态 Check 图标一致；
  因为只有 2 个选项（`PermissionRole` 的 `me`/`team`），按上游 `showSearch` 规则**不渲染搜索行**。
- **权限提示文案取自正确的命名空间**：改为上游 `knowledgeConfiguration.permissionsTip`
  （"If it is set to 'Team', all your team members will be able to manage the dataset." /
  "如果把知识库权限设为“团队”，则所有团队成员都可以操作该知识库。"），并补上 `knowledgeConfiguration.me`
  的中文「只有我」；此前用的是另一命名空间（agent 设置）里的旧句。
- **修掉旧版 `/kbs/{id}` 页面的权限控件**：该页仍渲染旧的 `input-select` 标记，但其脚本里
  `openPermPicker` 早已不存在（点击即 `ReferenceError`）。现在该页也注入共享组件并注册实例。

## [0.4.12] — 2026-09-20（对标批次 v0.3.7p）

### Fixed

- **数据集语言字段改为上游 `SelectWithSearch` 组件**：上一轮误把该字段对齐到 `constants/common.ts::LanguageList`，
  但上游 `dataset-setting/general-form.tsx` 实际取的是 `Object.keys(LanguageTranslationMap)`——共 27 项，
  **包含 Korean**，并且 `Indonesian/Indonesia`、`Portuguese BR/pt-br/pt-BR` 这类近重复键都在其中，
  选项文案就是键本身（该字段是“文档语言”而不是界面语言）。本轮把原生 `<select>` 换成上游的
  `role=combobox` 触发器 + cmdk 浮层：选项超过 5 个因此渲染搜索行（`搜索...` / `Search...`），
  选中项右侧显示 `Check` 图标，无匹配时显示 `CommandEmpty`（`common.noDataFound`：没有找到数据。/ No data found.），
  点击空白或 Escape 关闭且不改值。
- **语言值现在真的会保存与回填**：`PUT /api/v1/datasets/{id}` 的请求体补上 `language`，
  `loadConfig` 用 `kb.language` 回填，此前该字段只是装饰性控件。
- **补齐上游的集合删除接口**：上游 `web/src/utils/api.ts::rmKb` 是 `DELETE /api/v1/datasets` +
  `{ids:[...]}`（`ids` 为 `null` 时需配合 `delete_all`，空数组不删任何东西，返回 `success_count`），
  RayRAG 此前只有 `DELETE /api/v1/datasets/{id}`。本轮新增该集合路由（含跨租户 id 的
  `lacks permission for datasets` 报错与 `delete_all`），并把首页卡片、知识库列表卡片、详情页删除弹层
  三处调用改到该路由；单条路由保留兼容。
- **后端补上 `Knowledgebase.language` 顶层列**：上游 `db_models.py` 里 `language` 是
  `knowledgebase` 表的列（`CharField(max_length=32)`，默认按进程 locale 取 `Chinese`/`English`），
  不在 `parser_config` 里。RayRAG 的 `KnowledgeBase` 结构体新增该字段（`#[serde(default)]` 保证旧记录照常加载），
  `POST/PUT /api/v1/datasets/{id}` 接受顶层 `language` 并写入该列，`parser_config` 不再夹带该键；
  空值与超过 32 字符的值按上游约束拒绝。

## [0.4.11] — 2026-09-20（对标批次 v0.3.7o，未单独发版，随 0.4.12 一起发布）

### Fixed

- **数据集配置的语言选项与上游 `LanguageList` 逐项一致**：删除了 RayRAG 自行添加的 `Korean / 한국어`
  （上游 `constants/common.ts::LanguageList` 只有 16 项，且没有韩语），顺序与显示名（`LanguageMap`：
  简体中文 / Русский / Bahasa Indonesia /Tiếng việt / 日本語 / Português BR / Deutsch / Français /
  Italiano / Български /العربية / Türkçe / Nederlands）保持上游原样，新增回归测试锁定值与顺序。

## [0.4.10] — 2026-09-20（对标批次 v0.3.7n）

### Fixed

- **头像裁剪弹层按钮本地化**：共享外壳里的 `Cancel` / `OK` 改由 `t()` 注入（新增 zh 词条 `确定`），
  中文界面不再出现英文按钮，与上游 `common.cancel` / `common.ok` 一致。

## [0.4.9] — 2026-09-19（对标批次 v0.3.7m）

### Fixed

- **`/user-setting/profile` 的头像裁剪改为拖拽弹层**：上一轮发现该页并未引入共享脚本块，
  本轮把裁剪交互抽成模块常量 `AVATAR_CROP_JS`（`<script>` 整块）并在 `user_setting_layout` 外壳中统一注入，
  profile 上传路径切换为 `openAvatarCropModal(...)`，与数据集页共用同一套「80% 初始选区 + 拖拽夹紧 + 64×64 输出」行为。

## [0.4.8] — 2026-09-19（对标批次 v0.3.7l）

### Added

- **头像裁剪弹层支持拖拽选择（对齐上游 `components/avatar-upload.tsx`）**：打开裁剪弹层时按图片较短边的
  80% 生成初始选区并居中，鼠标按住选区即可拖动（越界自动夹紧到图片范围），OK 时把该区域绘制成 64×64 PNG。
  此前是「居中固定方形裁剪」的近似实现。

### Note

- 发布说明自此以**英文为默认**（GitHub Release 记录同样）。

## [0.4.7] — 2026-09-19（对标批次 v0.3.7k）

### Changed

- **后台用户表格的状态单元格对齐上游**：非本人行改为「Active / Inactive」下拉框（改动即保存），
  当前登录管理员自己的那一行改为 Active/Inactive 徽章（上游对本人行不提供状态控件）；
  原先动作列里的 Enable/Disable 按钮随之移除（上游动作列只有改密/删除等操作）。

### Note

- **审计结论（撤回一条非特性）**：`admin/users.tsx` 里没有 Excel 导入弹层（全文件无 xlsx/import 相关实现），
  台账中该残留项据此撤回；「行内角色编辑」的角色下拉由 `IS_ENTERPRISE` 控制（开源版不加载 roleList、
  更新接口也是企业版），按企业专属处理、不计入开源对标缺口。

## [0.4.6] — 2026-09-19（对标批次 v0.3.7j）

### Added

- **后台「用户管理」表格补齐上游能力**：新增全局搜索框（按邮箱/用户名/昵称/角色/状态过滤）、
  可排序列头（Email / Nickname / Role / Status，点击切换升/降序并显示 ↑↓）与页脚分页
  （`Total N` + 页码 + 每页 10/20/50/100）；`Reset` 同时清空角色/状态筛选、搜索词与排序状态。
  与 `service-status.tsx` 不同，`users.tsx` 上游并未禁用排序，故本页保留排序。

## [0.4.5] — 2026-09-19（对标批次 v0.3.7i）

### Changed

- **后台「额外信息」弹层改为上游 `JsonView` 风格的树形渲染**：不再把 JSON 直接 `JSON.stringify` 成一段文本，
  而是递归渲染键/值树——对象与数组带 `{n}`/`[n]` 折叠按钮（`aria-expanded` 可切），字符串/数字/布尔/null
  分别着色，等宽字体、逐层缩进，与 RAGFlow 后台详情弹层的 `JsonView` 展示一致。

## [0.4.4] — 2026-09-19（对标批次 v0.3.7h）

### Added

- **管理后台「服务状态」表格补齐上游工具栏与页脚**：新增全局搜索框（上游 `header.search`，按 ID/名称/类型/主机/端口过滤）
  与卡片页脚的 `RAGFlowPagination` 等价控件（`Total N` + 页码 + 每页 10/20/50/100），`Reset` 同时清空类型筛选与搜索词。

### Note

- **审计结论（撤回两条非特性）**：上游 `service-status.tsx` 里 `enableSorting: false` 全局禁用列排序、
  `useQuery` 未设置 `refetchInterval`（即没有自动轮询）；台账中“tanstack 列排序 / 自动刷新轮询”两项残留据此撤回，
  真正的缺口只有分页与全局搜索（本版已补）。

## [0.4.3] — 2026-09-19（对标批次 v0.3.7g）

### Fixed

- **「可用模型」面板元素语义与标题字号对齐上游**：`un-add-model.tsx` 的根节点是
  `<aside className="text-text-primary h-full flex flex-col">`、标题是 16px 的 `<h3>`；
  RayRAG 之前用 `<section>` + 默认字号 `<h2>`，现改为 `aside` + `.available-models-title`（16px/400），
  同时保留 `id='availableModels'`、`data-testid='available-models-section'` 与新增 `aria-label`。

## [0.4.2] — 2026-09-19（对标批次 v0.3.7f）

### Fixed

- **`/user-setting/model` 页面布局对齐上游**：改为上游 `setting-model/index.tsx` 的两栏卡片——
  外框 `.5px` 边框 + 圆角，左栏 3/5（设置默认模型 + 已添加模型，间距 16px、左右 20px 内边距、右侧 `.5px` 分隔线），
  右栏 2/5（可用模型列表），两栏各自滚动；卡片内加入上游的单个 `<Spotlight />` 背景光晕
  （opacity .8、coverage 60%、中心 50%/190%、深色白/浅色 rgb(194,221,243)）。

## [0.4.1] — 2026-09-19（对标批次 v0.3.7e）

### Fixed

- **模型列表选择器对齐上游 `ToggleList`**：触发器图标改为 lucide `ChevronsDown`（16px），展开时
  以 200ms 过渡旋转 180°（`transition: transform .2s`），不再是无动效的 `⌄` 文本；
  搜索行的放大镜改为 lucide `Search`、清除按钮改为 lucide `X`，并在模型列表加载期间显示
  `Loader2` 同款旋转指示器（加载中隐藏清除按钮，加载完成且有输入时才显示），与上游
  `searchLoading && <Loader2 className="size-4 shrink-0 animate-spin" />` 的互斥关系一致。

### Note

- **审计结论**：上游 `used-model.tsx` 的 viewMode 编辑实例入口（设置齿轮 + `handleSettingsClick`）在
  v0.26.4 中是注释掉的，`onEditInstance` 只被转递、从未被调用，因此“viewMode 立即持久化路径”在
  当前 UI 中不可达；RayRAG 的“新增实例”弹层即为行为等价实现。

## [0.4.0] — 2026-09-19（对标批次 v0.3.7d）

### Changed

- **版本号进入 0.4.0**：`Cargo.toml version = 0.4.0`，对标切片常量同步为 `v0.3.7d`；
  自此每次公开发布都会带上新的版本号与 GitHub Release 记录。

### Fixed

- **模型供应商图标回归上游 `LlmIcon` 画法**：去掉 RayRAG 自加的字母色块（`background:var(--nab)` + 圆角），
  统一 32px 品牌字形、颜色继承上下文（Available 卡片 `text-text-primary`，Added 行 `text-text-secondary`），
  与上游 `un-add-model.tsx`/`used-model.tsx` 的 `<LlmIcon width={32}>` 一致。
- **已添加模型列表几何**：列表容器底色改回上游 `bg-bg-card`，行分隔线 `.5px`（上游 `border-b-[0.5px]`）。
- **实例删除确认弹层对齐 `ConfirmDeleteDialog`**：标题 `common.deleteModalTitle`、右上角 lucide `X` 关闭、
  底部 `common.cancel` / `common.delete`，容器与按钮带上游 testid
  （`confirm-delete-dialog` / `-cancel-btn` / `-confirm-btn`）。

## [0.3.4] — 2026-09-19（对标批次 v0.3.7c）

### Added

- **模型类型多选（`MultiSelect`）补齐 cmdk 行为**：`(Select all)` 全选行（`common.selectAll`）、
  搜索时按 cmdk 1.1.1 的 `command-score` 打分并重新排序、空结果提示改为上游 `common.noDataFound`
  （"No data found." / 没有找到数据。，中文另有 `common.searching` 文案备用）。

### Fixed

- **自定义模型对话框校验对齐 `DynamicForm`**：必填提示改为上游拼接的 `"{label} is required"`
  （英文 "Model name is required"，中文 "模型名称 is required"）；`model_types` 恢复为可选
  （上游 zod schema 中多选字段 `optional()`，`modelTypeRequired` 只用于 switch-group）；
  未勾选特性时不再发送 `extra`（对齐 `extra: item.features ? {is_tools} : undefined`）。

## [0.3.4] — 2026-09-19（对标批次 v0.3.7b）

### Fixed

- **模型供应商卡片的标签顺序**（上游 `un-add-model.tsx` 用 `sortModelTypes`/`orderMap` 排序，而非后端返回的字母序）：
  14 张卡片的标签顺序与上游逐字对齐（如 OpenAI `LLM Embedding TTS ASR`、StepFun `LLM TTS ASR VLM`、
  Tongyi-Qianwen `LLM Embedding Rerank TTS ASR VLM OCR`）。
- **文档链接范围**：卡片右上角的 `↗` 只对上游 `constants/llm.ts::APIMapUrl` 的 47 个厂商渲染
  （Ollama/VLLM/LM-Studio/LocalAI/Xinference/New API 上游没有该链接，RayRAG 之前多渲染了 6 个）。
- **默认模型行标签**（`system-setting.tsx`）：`Embedding`→`Embedding Model`（zh 嵌入模型）、
  `Rerank`→`Rerank model`（zh Rerank模型），并按语言本地化。

## [0.3.4] — 2026-09-19（对标批次 v0.3.7a）

### Added

- **智能体日志页 `/agent-log-page/:id`**（上游 `pages/agents/agent-log-page.tsx` 417 行、
  `agent-log-detail-modal.tsx` 151 行、`hooks/use-export-agent-log.ts` 86 行）：
  面包屑（Agent › 画布标题 › Log）、工具栏（Export、`ID/Title` 关键字框、`Latest date` 区间、
  Search/Reset）、八列可排序表格（ID / User ID / Title / State 圆点 / Number / Latest date /
  Create date / Version）、RAGFlow 分页（10/20/50/100）、click-through 详情弹层（CJK 15 字截断标题、
  逐条消息与引用块）与带 BOM 的 CSV 导出（`agent-logs-{canvasId}-{date}.csv`）。
- **会话列表 API**：`GET /api/v1/agents/{id}/sessions`、`/sessions/{session_id}`，以及浏览器侧
  `api.ts::fetchAgentLogs` 使用的 `GET /v1/canvas/{id}/sessions` 别名（同一处理函数）。查询语义对齐
  `API4ConversationService.get_list`（keywords/日期区间/orderby+desc/分页上限 100/`dsl=false`/
  `exp_user_id`），行结构对齐 `_normalize_agent_session`。
- **会话运行元数据**：`Conversation` 新增 `dsl`/`errors`/`version_title`；智能体运行后写入画布快照、
  版本标题与错误状态，日志页的 Version 列与红/绿状态点取自真实运行结果。
- `/agent/{id}` 编辑器页新增 Log 入口（对应上游编辑器下拉菜单的 `flow.log`）。

### Fixed

- **容器时区与浏览器不一致导致日期筛选为空**：新增 `RAYRAG_TIMEZONE`（`docker-compose.yml` 注入容器 `TZ`，默认 `Asia/Shanghai`，与 RAGFlow 的 `TIMEZONE` 同一语义），日志行时间与日期区间比较改用服务端本地时钟（对齐上游 `DataBaseModel` 的 `datetime.now` 本地时间与 `CustomJSONEncoder`）。

### Note

- 上游 v0.26.4 的日志页自身请求了已随 `sdk` blueprint 迁移而消失的 `/v1/canvas/{id}/sessions`，
  即官方页面在该版本 404；RayRAG 同时提供新旧两种拼写，页面可用且 REST 面保持一致。

## [0.3.4] — 2026-09-18（对标批次 v0.3.4e–v0.3.6d）

### Changed

- **检索请求改为上游形状（`search_id` 驱动）**：`/api/v1/retrieval` 支持 `search_id` —— 由应用记录
  提供 `kb_ids`/`top_k`/`similarity_threshold`/`vector_similarity_weight`/`rerank_id`/`doc_ids`，
  调用方显式给出的字段优先；同时接受上游单数 `kb_id` 与 `size` 别名。`/search` 页面改为发送
  `{question, highlight, page, size, search_id, kb_id, doc_ids}`，侧栏控件仅在用户实际修改后才覆盖
  对应参数（`window.__SEARCH_TOUCHED`）。

### Fixed

- **`BM25+Vector` 关闭时被强制 `vector_similarity_weight=0`**：该行为是 RayRAG 自造，导致未命中关键词的
  提问检索不到任何结果（上一轮 CDP 需手动打开开关才能复现结果）。现在权重由应用配置或用户显式修改决定，
  与上游 `SimilaritySliderFormField` 一致；探针在不触碰侧栏的情况下即可拿到真实结果。

### Added

- **`/search` 的 AI 总结流与思维导图**（上游 `next-search/search-view.tsx` + `mindmap-sheet.tsx` 残留收口）：
  新增 `POST /api/v1/searches/{search_id}/completions`（按应用 `kb_ids`/`chat_id`/`llm_setting` 生成，
  以 `chat.completion.chunk` 增量 + `chat.completion.references` + `[DONE]` 的 SSE 输出，与聊天页同一套解析）
  与 `POST /api/v1/chat/mindmap`（LLM 生成 `{name,children}` 树）。
  前端：问候语输入行（`search.searchGreeting` 占位、lucide `X` 清空、分隔线、圆形提交按钮，
  流式期间切换为停止控件并可中断）、`AI summary` 区块（24px 标题、流式骨架屏、边框内滚动
  208px 的 markdown 答案 + 引用 chip + 分隔线）、`Total: N` 结果条与思维导图按钮/抽屉
  （`chunk.mind` 标题、进度条、缩进树、关闭按钮），标签全部按语言注入并修正了注入时机。

### Fixed

- **`/search` 检索在知识库 chip 尚未加载时丢参数**：`runSearch` 只从 DOM chip 取 kb_ids，
  页面初始化竞态下会发出无 `kb_ids` 的请求并被服务端以 400 拒绝；现回退到应用的
  `search_config.kb_ids`（与上游直接读应用配置一致）。
- **搜索页脚本语法错误**：新增 JS 在 Rust 普通字符串中的转义（`\'`、`\n`）与一处多余花括号
  导致整段脚本不可解析；已修复并以 `node --check` 逐 `<script>` 块复验 5/5 通过。

### Added

- **`/chunk` 切片工作台子树**：新增上游 `Routes.Chunk` 的四个路由——`/chunk`（仅头部外壳）、
  `/chunk/parsed/chunks`、`/chunk/chunk/{doc}`、`/chunk/result/{doc}`。头部按上游
  `pages/chunk/index.tsx` 复刻：面包屑（`Agent` 链接到知识库 + 当前文档名）、`Segmented`
  三选项（`Parsed results` / `Chunk result` / `Result view`，活动态与 `data-section` 对齐）、
  ghost 省略号菜单（Refresh/Copy）与带 lucide `Save` 图标的 Save；面板按上游
  `chunk-toolbar.tsx`（30px 粗体标题、ghost `Copy` 图标按钮、outline `Export`）与
  `chunk-card.tsx`（`ParsedPageCard` 页面标签+正文；`ChunkCard` 含 lucide `Annoyed`、
  `Switch` + `Active` 文案、四行截断正文）渲染。
- **数据为真实切片**：数据集/文档由 `?knowledgeId=`（上游 `QueryStringMap.KnowledgeId`）解析，
  切片来自 `/api/v1/datasets/{id}/documents/{did}/chunks`；`Active` 开关经 `Save` 以
  `PATCH …/chunks/{chunk_id}`（`{available}`）持久化，Copy 复制到剪贴板、Export 下载 JSON，
  文档仍在解析时状态行给出提示。
- **台账更正**：上游该子树实为**静态原型**（面包屑 `Agent`/`xxx`、Save 无处理函数、两个面板
  都是 `new Array(10).fill(<同一段示例文本>)`），故 8 个文件由 `reference` 纠正为 `replaced`
  并逐行记录“路由对齐 + 真实数据替代”的取舍（replaced 101→109）。

### Fixed

- **`/chunk` 面板在文档未解析完成时无反馈**：现在状态行显示“文档仍在解析中，请稍后刷新”，
  避免空面板被误认为功能缺失。

### Changed

- **Added models 实例行与实例模型编辑对齐上游 `used-model.tsx`**：实例折叠按钮在
  `View models`/`Hide models` 文案切换时同步替换 lucide `ChevronsDown`/`ChevronsUp`（16px）图标；
  删除按钮由 emoji 改为 lucide `Trash2`（16px）图标按钮；模型行铅笔改为 lucide `Pencil`（12px）
  并使用按语言注入的 `Edit model` 可访问名。
- **`MultiSelect` 组件化复用**：把 v0.3.5z 的 Model type 组合控件抽成可注册前缀的共享实现
  （`piMsRegister/piMsToggleOpen/piMsRender/...`），Add custom model 弹层与**实例模型编辑弹层**
  共用同一控件与同一份上游选项顺序；实例编辑弹层的 Name / Max tokens 置为禁用（仅类型可改），
  标题 `Edit model`、底部 `Cancel`/`Confirm` 与上游 `AddCustomModelDialog` 一致。

### Fixed

- **模型供应商页脚本语法错误**：重构时 `toggleAddedInstance` 丢失 `async` 关键字，导致该页
  整段脚本解析失败、供应商弹层无法打开。已修复并用 node --check 逐 `<script>` 块复验（7/7 通过）。

### Changed

- **模型供应商弹层的 List models / Add custom model 逐行对齐**：模型行改为上游渲染——类型 chip 直接显示
  原始 `model_types` 字符串（不再经 `mapModelKey` 映射）、`All models` 哨兵行同款左右分组、空搜索行使用
  上游 `setting.noMatchingResults`（两地语言均为字面量 "None"）；行内编辑改为上游 lucide `Pencil`（12px，
  可访问名 `Edit model`）；底部 "Add custom model" 改为 `role=button tabindex=0` + lucide `Plus` +
  键盘 Enter/Space 与上游内边距。
- **Add custom model 弹层的 Model type 改为上游 `MultiSelect` 组合控件**：徽章触发器（含逐个 ×移除、
  `Select value` 占位、悬停清除）、搜索框（`common.search` + "..."）、带 `h-4 w-4 rounded-sm border
  border-primary` 勾选方块的选项行、底部 `common.clear` / `common.close`；`maxCount=100` 保持全部可见；
  `Model features` 改为上游 `rounded-md border p-3` 容器。顺带修正两处 i18n 取值：`common.clear` 由
  「清空选择」改为上游「清空」，`Select value` 由错误值改为「请选择」。

### Fixed

- **List models 目录请求会覆盖用户刚添加的自定义模型**：发现请求返回时整表替换，正在打开弹层期间新增/
  编辑的模型会被静默丢弃。改为**合并**（保留本地自定义项）并对追加按名称去重，等价上游
  `setModels(prev => prev.some(m => m.name === item.name) ? prev : [...prev, item])`。

### Added

- **RAGFlow 管理控制台（`/admin` 子树）**：`/admin` 由旧的 RayRAG 仪表盘改为上游语义的
  **管理端登录页**（`pages/admin/login.tsx`：共享登录背景 + 三束 spotlight、logo + `RAGFlow`、
  `Admin console` 标题、邮箱/密码/记住我 + 全宽登录按钮、卡片下方主题开关），登录走
  `POST /api/v1/admin/login`（仅超级管理员，非管理员返回上游文案
  `Only superuser can login admin system`，令牌写入 `Authorization` 响应头）。
  新增导航布局（288px 侧栏：logo + `admin.title`、三个社区入口 `Service status` /
  `User management` / `Sandbox settings` 配 lucide `ServerCrash`/`UserCog`/`Zap` 图标、
  版本号 + 主题开关 + 登出）：`/admin/services`（组件表 + 类型筛选 + 详情弹层）、
  `/admin/users`（用户表 + 角色/状态筛选 + 新建/改密/删除弹层）、
  `/admin/users/{id}`（账号详情 + 状态/密码/删除操作）、`/admin/sandbox-settings`
  （沙箱变量表）。新增 API：`POST /api/v1/admin/login`、`GET /api/v1/admin/logout`、
  `GET /api/v1/admin/services`、`GET /api/v1/admin/services/{id}`。
  未登录/非管理员访问控制台页面由服务端 307 回 `/admin`（对齐上游 `AdminAuthorizedLayout`）。

### Added

- **`install.sh` 一键部署脚本**：面向第一次使用的用户，检查 Docker/Compose、幂等生成 `.env`
  （随机 24 位 PostgreSQL 与管理员密码，重复执行不覆盖既有配置）、支持
  `--port/--no-build/--cn-mirror/--global-mirror/--dry-run/--help`、统一
  `RAYRAG_CMD_TIMEOUT`（默认且上限 2 小时）、校验 compose 配置、构建并启动、等待健康检查、
  打印局域网访问地址与登录账号；输出跟随系统语言（zh*/en）。
- **双语 README**：英文默认 `README.md` + 中文 `README.zh-CN.md`，面向零基础用户
  （五分钟部署、首次使用六步、65 家 provider 与本地模型对照表、12 条 FAQ、升级/备份/卸载、
  纯 Linux 安装）；原技术长文档迁至 `docs/advanced.md`。

### Changed

- **`/user-setting/model` 的 Set default models 段落逐行对齐上游 `system-setting.tsx`**：
  `article > header`（24px/500 标题 + 14px 说明）、`px-7 py-6 gap-6 max-h-[70vh] overflow-y-auto`
  边框滚动容器、六行 `flex gap-3`（label `w-1/4` / control `w-3/4`，仅 LLM 行带 `*`）；
  placeholder 改为上游 `setting.selectModelPlaceholder`；六个 tooltip 走 `t()` 并补 zh.ts 逐字译文；
  触发图标改为 lucide `CircleQuestionMark`（12px 描边圆 + 问号，保留悬停弹窗）。
- **同名异译 i18n 键分离**：上游 `setting.selectModelPlaceholder`（en `Select model` /
  zh `请选择模型`）与 `memories.selectModel`（en `Select model` / zh `选择模型`）英文同文、
  中文异译，扁平 `t()` 表改用上游 i18n 路径作为名字空间键区分。

### Added

- **上游路由表对齐（`routes.tsx` 第一批）**：新增 `/login-next`（上游 `/login` 与 `/login-next` 同页双入口）、
  上游规范的单数详情路径 `/agent/:id` 与 `/search/:id`（保留 RayRAG 既有 `/agents/{id}`、`/searches/{id}` 兼容别名）、
  上游独立文档查看器 `/document/:id`（按文档记录反查知识库渲染，未知 id 返回真实 404）、
  以及公开嵌入页 `/search/share`（`?tenantId=` 解析检索应用、`visible_avatar` 控制应用头像行，
  与 `/search` 共用 `search_page_markup`）。

### Fixed

- **Docker 构建可能无限挂起**：zvec 预编译库下载只有 `--retry` 而没有连接/总超时，国内镜像站卡死时
  构建会静默等待（本轮实测挂起 20+ 分钟）。改为 `--connect-timeout 20` +
  `--max-time ${ZVEC_DOWNLOAD_TIMEOUT}`（默认 900 秒，`docker-compose.yml`/`.env.example` 可覆盖）
  `--retry 3 --retry-all-errors`，主镜像超时后清理半包并自动切换备用镜像。
- **一条从未执行的测试**：`available_models_pills_and_local_provider_fields_match_upstream`
  缺少 `#[tokio::test]` 属性，长期被当作死代码；补属性后首次真实执行并通过。
- **`t()` 死分支**：删除被 `"混合搜索"` 永久遮蔽的 `"Hybrid" => "混合检索"`。

## [0.3.4] — 2026-09-18（对标批次 v0.3.4e–v0.3.5v）

### Added

- **Apache-2.0 `LICENSE` 与 `NOTICE`**：注明对标来源 RAGFlow v0.26.4 与第三方资产许可。
- **净化发布流程**：当前提交导出为净化快照（剔除开发脚本、抓取产物、模型权重与运行时状态，并对机器标识脱敏）后推送 GitHub；脚本保留在内部工程仓。

### Changed

- **相关搜索端到端对齐上游 next-search**：检索应用配置补齐 `search_config` 全字段（doc_ids/chat_id/llm_setting/
  cross_languages/use_kg/highlight/keyword/web_search/related_search/query_mindmap/summary，`top_k` 默认 1024）；
  `/search` 设置栏与 `/searches/{id}` 编辑页新增 `Enable related search` 开关；检索结果下方按上游渲染
  `Related search` 标题与最多 5 个 chip，点击即以该问题重跑检索。
- `POST /api/v1/chat/recommendation`：命中 `search_id` 时使用该应用的 `chat_id` 实例与 `llm_setting`（剔除
  `parameter`，默认 temperature 0.9），非法设置返回 400。
- **浏览器端验证**：开关置位、chip 数量与文本、请求体、点击重跑、
  关闭后不再请求、编辑页保存后 GET 复核、探针应用清理。

- **Joined teams 表逐列对齐上游 `tenant-table.tsx`**：四列 Name（头像+昵称）/ Update date（三态排序按钮，↕/↑/↓）/
  Email / Action；`invite` 行渲染链接式 Agree/Refuse，`normal` 且非本人租户渲染纯图标 ghost Quit，owner 行留空；
  补 loading 转圈行与 `No data` 空行；日期统一走上游 `utils/date.ts::formatDate` 的 `DD/MM/YYYY HH:mm:ss`，
  Quit 用 lucide `LogOut` 图标。后端 `/api/v1/tenants` 关联 owner 的 nickname/email/avatar 与 `update_date`
  （自持租户合成行回退到账号创建时间），成员接口同步补 avatar。
- **浏览器端验证**：live payload 键、loading 行、四态排序顺序与图标、
  搜索过滤、空数据行、恢复默认。

- **技能检索配置弹层对齐上游 `search-config-modal.tsx`**：补 `configDesc` 说明、权重/阈值改滑杆并带实时读数
  （`Hybrid (N% Keyword + M% Vector)` / `.toFixed(1)`）、三档说明与 Top K(1..100)、新增 Index Fields 四字段编辑器
  （Name/Tags/Description/Content 各带开关与权重，保存写入 `field_config` 并支持回填），全量文案中英双语。
- **浏览器端验证**：滑块实时文案、字段编辑、保存后 GET 复核与重开回填。


### Added

- **数据集标签接口**：`GET /api/v1/datasets/{id}/tags`（按文档元数据聚合 `[[tag, count], …]`，count 降序）、
  `DELETE`（`{tags:[…]}`）与 `PUT`（`{from_tag,to_tag}` 重命名并去重），校验文案与未知数据集 404 对齐上游
  `dataset_api`；实现位于 `src/api/dataset_tags.rs`。
- **配置页标签云 / 标签表**：Tag sets 行下新增上游 `tag-tabs.tsx` 形态的分段切换——CSS 词云（字号随 count 变化、
  hover 显示计数、点击重命名）与标签表（Tag/Count/重命名/删除、空态与状态行）。
- **浏览器端验证**：种子标签 → 云/表视图 → 重命名 → 删除，逐步复核 GET 结果。

### Fixed

- **标签重命名会残留旧标签**：`batch_update` 对数组是合并语义，改用整条记录 `replace()` 重写。
- **标签脚本跨页引用** `PI_PICKER_LABELS` 导致 `ReferenceError`，改为数据集页自带标签表；
  并清理 `t()` 中重复的 `"Rerank Model"`/`"Cancel"` 分支（unreachable pattern）。


### Changed

- **检索页重排字段对齐上游 `search-setting.tsx`**：开关文案改为 `search.rerankModel`（Rerank Model/重排模型），
  打开后显示共享重排模型树（必填），无模型时提交被上游文案
  「Rerank model is required when rerank is enabled」拦截且不发请求；关闭开关把 Top K 复位为上游默认 1024；
  检索统一走 `/api/v1/retrieval`，`rerank_id` 仅在开关打开时随包发送。

### Added

- **浏览器端验证**：home→results 视图、无模型校验、选中模型后的
  请求体断言、关闭后 top_k=1024，逐步截图。


### Changed

- **数据集权限字段对齐上游 `PermissionFormField`**：改为可搜索组合控件（`SelectWithSearch` 形态）+
  `PermissionRole` 两项（`me`=Only me / `team`=Team）+ 上游 label/tooltip + `ds-settings-basic-permissions-select`。
- **权限值统一为 `me`/`team`**：`normalize_permission()` 在存储加载、创建、更新与数据集 API 四处归一，
  历史 `private` 自动纠正；`GET /api/v1/datasets` 现在返回 `me`/`team`。
- **数据集头像改用上游 `AvatarUpload` 组合**：空态「＋ Upload」、64×64 预览 + 铅笔重选、圆形 `Remove image`、
  扩展名白名单与 64×64 canvas 裁剪，带 `ds-settings-basic-avatar-upload`。
- **浏览器端验证**：覆盖权限弹层过滤/选择/保存回读与头像三态。

### Fixed

- **配置页横向溢出（28px）**：设置表格的固有宽度把 `.ds-content` 轨道撑出视口；现允许轨道收缩
  （`.ds-content{min-width:0}`）并让卡片内部横向滚动（`.ds-content>.card{overflow-x:auto}`），实测 `docScrollW == clientW`。
- **数据集头像脚本跨页引用**：初版误用 Profile 页的 `PROFILE_AVATAR_EXT`/`profileAvatarCrop`（页面脚本各自独立
  作用域）导致 `ReferenceError`，现改为数据集页自带的 `DATASET_AVATAR_EXT`/`cfgAvatarCrop`。


### Fixed

- **Logs 面板偶发空白（竞态）**：`loadLogs()` 在页面 `badge()` 定义之前就可能拿到响应，触发
  `ReferenceError: badge is not defined` 使日志表不渲染；现把 `badge()` 同时定义在 Logs 脚本内，并新增
  `dataset_logs_script_defines_badge_before_use` 断言定义早于首次调用。

### Added

- **OAuth 登录回调全链路**：`GET /api/v1/auth/oauth/{channel}/callback` —— 单次消费 `state`（10 分钟 TTL）、
  `token_url` 换码、`userinfo_url` 取用户（github 另取 primary email）、未知邮箱自动注册（昵称/头像落库）、
  签发会话并通过 `rayrag_token` cookie 跳 `/?auth=<id>`；失败按上游语义 302 到
  `/?error=invalid_state|missing_code|token_failed|userinfo_failed|email_missing|register_failed`。
- **`StateStore`/`UserStore::issue_token_for`**：前者为无状态服务提供 OAuth state 生命周期，后者为已认证用户签发 24h 会话。
- **回归护栏** `oauth_callback_flow_matches_upstream`：本地假 provider 驱动真实换码/userinfo 请求，并覆盖
  成功注册 → 发会话 → cookie 校验的完整链路。
- **配置透传**：`docker-compose.yml` 增加 `REGISTER_ENABLED` / `DISABLE_PASSWORD_LOGIN` / `RAYRAG_OAUTH_CONFIG`，`.env.example` 同步说明与示例。


### Added

- **登录渠道端点**：`GET /api/v1/auth/login/channels`（`[{channel, display_name, icon}]`，无配置返回空数组）与
  `GET /api/v1/auth/login/{channel}`（302 跳转授权地址，未知渠道 400 `Invalid channel name: …`），由新增的
  `src/oauth_config.rs` 注册表驱动（`RAYRAG_OAUTH_CONFIG`/`OAUTH` JSON，按上游 `settings.OAUTH_CONFIG` 形状，
  支持 OIDC `issuer` 推导端点），两条均免登录。
- **SSO-only 模式**：登录页遵循 `disablePasswordLogin`——隐藏两个密码表单与两个切换链接，渠道区块切换为上游
  `py-8`/`w-full` 单列布局；渠道按钮改为上游的 `GET /api/v1/auth/login/{channel}` 跳转。

### Fixed

- **`public_api_path` 动态段失效**：占位符（`{channel}`、`{account_id}`）此前按字面量匹配真实请求路径，导致
  OAuth 入口与既有的渠道路由回调一律 401；改为逐段匹配并加单测（含必须仍需鉴权的反向用例）。
- **登录页脚本 `SyntaxError`**：渠道按钮 `innerHTML` 模板中的嵌套双引号让整段登录脚本失效，改为
  `data-channel` + `addEventListener` 绑定。
- **`/api/v1/auth/login/channels` 此前不存在**（页面静默失败、渠道区永不渲染），现补实现。
- **`/api/v1/version` 短暂 401**：v0.3.5o 重写 `public_api_path` 时漏掉该自建端点，已补回并纳入白名单单测。


### Added

- **`/login` 上游 3D 背景**：`login-next/bg.tsx` 的三条霓虹弧带（viewBox 240/466/704、`#00BEB4` 底描边、
  `#80FFF8` 辉光渐变 + dash 遮罩、`#FFD700` 高光、`feGaussianBlur` 5.2/5.5、16s dash 流动）与
  `components/spotlight.tsx` 的三个径向辉光（0.4/0.3/0.3、coverage 60/12/12、`backdrop-filter: blur(30px)`）全部落地。
- **`registerEnabled` 闸门**：登录页读取 `GET /api/v1/system/config`，为 0 时隐藏注册入口并禁止翻面。
- **两个上游系统端点**（免登录）：`GET /api/v1/system/version`（返回版本字符串）与
  `GET /api/v1/system/config`（`registerEnabled` / `disablePasswordLogin`，取自 `REGISTER_ENABLED` /
  `DISABLE_PASSWORD_LOGIN`），并加入 `public_api_path` 白名单。
- **浏览器端验证**：校验弧带高度/viewBox、spotlight 数量、动画名与时长、
  闸门开关与两个端点响应。


### Added

- **user-setting Profile 页对齐上游**：头部 `setting.profile` + `setting.profileDescription`，190px 标签列 + 边框值框 +
  `PenLine` 编辑按钮（用户名/头像/时区/邮箱/密码），`setting.emailDescription` 与 `setting.avatarTip` 说明文案。
- **AvatarUpload 交互**：无头像时 64×64 虚线上传按钮；有头像时 64×64 预览（hover 铅笔可重选）+ 右上角圆形
  `Remove image` 按钮清空；文件名白名单 `jpg|jpeg|png|webp|bmp`；选中后 canvas 居中裁剪为 64×64 PNG 再上传。
- **Profile 双语**：页面标签/说明/按钮与 JS 校验文案全部中英双语（概要/用户名/头像/时区/邮箱/密码/编辑、
  请选择图片文件、请确认新密码…），弹窗标题按上游保持英文硬编码。
- **浏览器端验证**：验证空态→上传→预览→删除→非法扩展名拒绝→三个编辑弹窗→zh 文案。

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
- **浏览器端验证**：逐个打开六个本地厂与 OpenRouter 模态，
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
- **浏览器端验证**：用 `page.route` 桩出 12 个模型，逐步验证展开、
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
- **浏览器端验证**：真实点击卡片 → 弹窗 → 打开 Base-Url 弹层 → 切 region → 清除 →
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
- **侧栏/导航全量实点脚本** 浏览器端验证脚本：顶部导航 7 项、
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
- **RAGFlow UI 遍历工具链**（浏览器端验证脚本）：`ragflow-auth-cdp.mjs`
  （翻转卡片登录/注册自举，会话持久化于 Chrome profile）、
  `ragflow-ui-dump.mjs`（逐路由导出按钮/链接/字段/表头/标签页 + 截图，产物
  内部 UI 抓取产物），作为 RayRAG UI 一比一对标的结构化基准。

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
- **RAGFlow Web UI 清单** 内部界面盘点：真实浏览器
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

- **覆盖矩阵最终收尾**（纯文档，内部对标台账）：矩阵终态
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
- **Helm chart**（Kubernetes 部署脚手架 新建）：最小可用 chart（deployment/service/configmap，
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
