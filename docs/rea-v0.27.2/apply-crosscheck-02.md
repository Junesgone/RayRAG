# 文档面应用交叉核对（RAGFlow v0.27.2 × RayRAG）

> 上游证据：`document-surface-v0.27.2.md`（19 路由 + 39 探针，`~/.cache/rea/ragflow-v0.27.2/round10-forensics/`）。
> 本件：逐项对照 RayRAG 实现（`src/api/document.rs` / `doc_assets.rs` / `artifacts.rs` / `document_metadata.rs` / `metadata_config.rs` / `attachments.rs` / `server.rs`），全部代码级核对（RayRAG 实例探针留待修正批后复测）。
> 日期：2026-10-12。

**判定图例**：`✓` 可观察等价（文案+码值+形状）；`T` 文案级（引号/大小写/空格/单词）；`C` 码值 / HTTP 状态级（并入 file-commit 信封决策项）；`S` 形状级（JSON 键/结构；落地前需查 RayRAG UI 依赖）；`F` 功能面缺失（参数/分支，入实施队列）。

## 逐项对照

| # | 上游行为（实测/源） | RayRAG 现状 | 判定 |
|---|---|---|---|
| 1 | `GET …/documents` 成功：`data {total, docs}`；条目 28 键（含 `chunk_method`/`dataset_id`/`suffix`/`thumbnail`/`token_count`/`parser_config`/`pipeline_*`/`nickname`/`process_*`） | `data {docs, total_datasets}`；条目基础键 + alias（`create_time`/`location`/`created_by`/`type`/`source_type`），缺上述键 | **S** |
| 2 | 列表过滤/分页/排序（orderby 校验文案 `invalid orderby field: {v}` 101）、`type=filter` 聚合分支、`id`+`ids` 冲突文案 | 无参数面（列全量） | **F** |
| 3 | 未拥有数据集 → 102 `You don't own the dataset {id}. `（尾空格）+ HTTP 200 | HTTP 404 + `Not found` | **C+T** |
| 4 | `GET /thumbnails` 无参 → 101 + `data:false` + `Lack of "Document ID"` | 一致（含 101/`data:false`） | **✓** |
| 5 | `GET /thumbnails` 伪 ID → `{code:0, data:{}}` | `data:{id: null}`（按请求键回填） | **S**（minor） |
| 6 | 缩略图 URL `{kb}-{thumbnail}`；图片端点首连字符分割 | `{kb_id}-{doc_id}.jpg`；最长前缀匹配（UUID 含破折号，doc_assets.rs 注释在案） | **✓**（机制等价） |
| 7 | `GET /documents/images/{id}` 畸形/不存在 → 102 `Image not found.` + HTTP 200 | HTTP 404 + 102 + `document not found` | **C+T** |
| 8 | `GET /documents/artifact/{f}` 坏扩展/不存在/非法名 → 200 + 102 + `invalid file type`/`Artifact not found.`/`Invalid filename.` | 逐字一致（artifacts.rs 已对齐） | **✓** |
| 9 | 产物访问门 = 消息引用会话 + canvas 可访问性 | ArtifactStore owner/admin 记账 | 备注（访问模型差异，无探针冲突） |
| 10 | preview / download(`/documents/{id}`) / download(数据集域) 未找到 → 102 `document not found` + 200 | 404 + `Document not found` / `Document not found!` | **C+T** |
| 11 | `The dataset not own the document {id}.`（跨数据集分支） | 无该分支 | **T+F** |
| 12 | 空文件 → `This file is empty.`（preview 与 download 均同文） | download ✓；preview 用 `File content missing` | **T** |
| 13 | DELETE `{}` → 102 `should either provide doc ids or set delete_all(true), dataset: {id}. `（尾空格） | 400 + 同文（无尾空格） | **C+T** |
| 14 | DELETE 并存 → 102 `should not provide both doc ids and delete_all(true), dataset: {id}. ` | 400 + 同文（无尾空格） | **C+T** |
| 15 | DELETE 无效 ID → 102 `These documents do not belong to dataset {id} or Document not found: {join}` | 400 + 同文（join 格式已一致） | **C** |
| 16 | DELETE 成功 → `data {deleted: n}` | `data {success_count: n}` | **S** |
| 17 | DELETE 失败 → 102 + `str(errors)` | 500 `Partial failure` + data | **S** |
| 18 | parse/stop `{}` → 102 `` `document_ids` is required ``（反引号） | 400 + `'document_ids' is required`（单引号） | **C+T** |
| 19 | parse/stop 伪 ID → 102 `Documents not found: ['{id}']`（引号） | 102 ✓ + `[id]`（无引号） | **T** |
| 20 | parse/stop 成功 → `{success_count, errors?}` | 同形（errors 元素为 `{id}: {err}`，上游为字符串集） | **✓**（微差备注） |
| 21 | stop 非运行 → errors 追加 `Can't stop parsing document that has not started or already completed` | 直接尝试取消；无该判定文案 | 备注 |
| 22 | batch 空 → 101 `"doc_ids" must be a non-empty list.` | 400 + `'doc_ids' …`（单引号） | **C+T** |
| 23 | batch 坏 status → 101 `"Status" must be either 0 or 1:{v}!` | 400 + `'status' must be either 0 or 1: {v}!`（大小写/空格） | **C+T** |
| 24 | batch 每文档映射 + 部分失败 500 `Partial failure` + data | 同构（错误条文案微差） | **✓** |
| 25 | summary 伪数据集 → 102 `You don't own the dataset {id}. ` + 200 | 404 + `Not found` | **C+T** |
| 26 | summary 成功 → `{summary}` | 一致 | **✓** |
| 27 | metadatas/metadata-update 空、空选择器 → 成功 `{updated:0, matched_docs:0}`；**`selector:[]` 上游也成功**（falsy 归一） | `selector:[]` 报 `selector must be an object.`（无 falsy 归一） | **T**（语义） |
| 28 | metadatas 字段校验文案（`selector must be an object.` 等） | 逐字一致（document_metadata.rs） | **✓** |
| 29 | metadata/config 伪数据集 → `you don't own the dataset`（102） | 404 `The dataset doesn't exist` | **C+T** |
| 30 | metadata/config 伪文档 → 102 `document {doc} not found in dataset {ds}` | 404 `The dataset does not have the document.` | **C+T** |
| 31 | metadata/config 缺 `metadata` 键 → 101 `metadata is required` | serde 默认空列表 → 静默接受 | **T**（校验） |
| 32 | `POST /documents/upload` both/none/SSRF 文案 101（HTTP 200） | 文案逐字 ✓、code 101 ✓；HTTP 400 | **C** |
| 33 | `POST /datasets/{id}/documents` 无文件 → 101 `No file part!`（另有 `No file selected!`） | 400 `multipart field 'file' is required` | **C+T** |
| 34 | 单文件成功 → 200 + `data`（map_doc_keys_with_run_status） | HTTP 202 + `Uploaded {name}`（自有形状） | **C+S** |
| 35 | 多文件部分成功 → 200 + code 500 + `data`（混和信封） | 207/400 + 自有 `{succeeded, failed}` | **C+S** |
| 36 | `type=web`/`type=empty`、`parent_path`、`return_raw_files`、parser_config 覆写 | 未实现 | **F** |
| 37 | ingest 成功 → `data: true`；错误 → `get_json_result(error_code, message)` | `data: [queued]` / `Cancelled` 形状；自定校验文案（`'run'…`） | **记录***（上游成功形状未实测） |
| 38 | PATCH update `{name:null}` → 102 `Field: <name> - Message: <Input should be a valid string> - Value: <None>` | 400 `name is required` | **C+T** |
| 39 | PATCH update 伪文档 → 102 `the dataset doesn't own the document` | 404 `Not found` | **C+T** |
| 40 | PATCH update 全字段面（`chunk_method`/`enabled`/`parser_config`/`pipeline_id`/`meta_fields`） | 仅重命名 | **F** |

\* ingest 行：上游空/伪体行为已实测（2026-10-12 round11 探针：{}→100 KeyError('doc_ids')、伪 ID→109 no authorization、空表→成功 data:true，均 HTTP 200）。

## 修正批计划

**批① 文案级（可直接落地；测试断言同步：`src/server.rs:18189/18220/18292/18294/21175` 及附近）**
- `src/api/document.rs:478/489` batch 两处引号与大小写（`"doc_ids"`、`"Status" must be either 0 or 1:{v}!`）；
- `:837/892` parse/stop `'document_ids'` → `` `document_ids` ``；
- `:844/899` `Documents not found: [{}]` 每 ID 加单引号；
- `:579/585` delete 两处句尾补尾空格（上游 `. ` 结尾）；
- `:1030/1034` 与 preview/download 系（`Document not found`/`!` → `document not found`）、preview 空文件 → `This file is empty.`；
- `doc_assets.rs` images 未找到文案 → `Image not found.`；
- `metadata_config.rs:323` → `document {doc} not found in dataset {ds}`；缺 `metadata` 键校验（101 `metadata is required`）；
- `document_metadata.rs` summary 系文案（`You don't own the dataset {id}. `）与 `selector:[]` falsy 归一。

**批② 码值/状态级（并入 `apply-crosscheck-01.md` 的信封决策项后统一执行）**
- document.rs 族 400 → 101（参数）/ 102（数据）收敛；404 族 → 200 + RetCode（与 metadata/artifacts 族一致）；
- `POST /documents/upload` HTTP 400 → 200。

**批③ 形状级（先查 RayRAG UI 依赖，随后随版本发布）**
- DELETE `{deleted}` 兼容键（或双键）、list `total` 双键（保留 `total_datasets`）；
- thumbnails 伪 ID 省略键（`{}`）；upload 202 族与 ingest `data` 形状。

**批④ 功能级（入实施队列，按序推进）**
- 列表过滤/分页/排序 + `type=filter`（含 `orderby` 校验文案）；
- `type=web`/`type=empty` 上传 + `parent_path`/`return_raw_files`/parser_config 覆写；
- PATCH update 字段面扩展（`chunk_method`/`enabled`/`parser_config`/`pipeline_id`/`meta_fields`）；
- `The dataset not own the document {id}.` 分支。

## 台账关系

- `document_api.py` 行「aligned / 复核通过」已精细化：追加本件指针与三级差异分类（见 coverage 行注）。
- 未决总表追加：文档面写路径成功形状（既有）+ 四级差异修正批队列（本件）。

## 批① 执行记录（2026-10-12）

已落地（仓库提交 `api: document-surface batch 1`）：批① 清单全部执行——list/delete 鉴权文案（`You don't own the dataset {id}. ` 含尾空格）、batch 引号与大小写（`"doc_ids"`、`"Status" …:{status}!`）、parse/stop 反引号与 ID 单引号、rename 文案族（`you don't own the dataset` / `the dataset doesn't own the document` / `Field: <name> … <None>`）、download/preview 文案族（`document not found`）与 `The dataset not own the document {id}.` 分支、delete 尾空格、images `Image not found.`、summary GET 鉴权（同族 `data_error` 200+102）、`selector` falsy 归一（`[]`/`""`/`0`/`false` → `{}`）。
随批修复：`env_surface` 补 `RAYRAG_POSTGRES_USER` 豁免条目（`9288bbf` 漏登记 → 单测转红）。
延后：metadata/config `metadata is required` 校验（需换 raw-body 提取器；归入批④同批）。
验证：`cargo test --locked` **2271 passed / 0 failed / 27 ignored**；`cargo fmt --check` 干净。

## 批② 执行记录（2026-10-12，提交 f9584d2）

已落地（仓库提交 `api: document-surface batch 2 — business codes converge to the upstream envelope`）：

- **document.rs（30 处）**：list/delete 鉴权 404→102；get_doc/reparse/rename/delete_doc 的 `Not found` →102（8 处；ingest 1 处冻结保留）；batch/rename 鉴权→102；rename 空名分支 400→101（推定参数族，字段面归批④）；download/preview 族 404→102（6 处）；跨数据集分支→102；sheets/slides：404→102 且 `Document not found`→`document not found`、`File content missing`→`This file is empty.`；download_doc 流文案→`This file is empty.`；batch 校验 400→101（2 处）；delete 校验 400→102（3 处）；parse/stop 空选择 400→102（2 处）。
- **server.rs create 路由**：kb 不存在 → `Can't find the dataset with ID {id}!`（102）；存在但无管理权 → `no authorization`（109）——均 HTTP 200；无文件 → `No file part!`（101）HTTP 200。
- **attachments.rs**：`/documents/upload` 文档路由业务错误统一 HTTP 200（新增 `document_route_error` 与 `reject` 分流、`upload_by_url` 增 `document_route` 参数）；canvas 路由保持 400（合同未审计）。
- **doc_assets.rs**：images `not_found` 闭包 404→200+102（`img_fake` / `img_nohyphen` 证据）。
- **metadata_config.rs**：doc 级 metadata/config 两条（`you don't own the dataset` / `document … not found…`）404→200+102（`mcfg_fake` 证据）；dataset 级保留（上游 service 文案未审计，后补）。

未随批（记录在案）：

- **ingest 四处冻结**（668/677/689/694）：round11 探针实测（`{}`→100 `KeyError('doc_ids')`；伪 ID→109 `no authorization`；空表→成功 `data:true`，均 HTTP 200）；语义/形状差异归批④/批③。
- **upload 成功族**（单文件 202 → `Uploaded {name}`；多文件 207 与全失败 400）与 ingest `data` 形状 → 批③（信封统一随形状落）。
- **update_dataset / delete_dataset** 的 `Knowledge base not found`（404 块 ×2，同型代码）属数据集面，未随批②。

测试同步（7 断言 / 6 用例）：bulk batch 校验 101、bulk delete 校验 102、stop 空选择 102、preview 缺失 102、kb-isolation list 102、doc-config 缺失 200+102、thumbnails images 200+102。

验证：`cargo test --locked`（zvec 0.7.2 + 清代理）**2271 passed / 0 failed / 27 ignored**；`cargo fmt --check` 干净；build ✓。UI 依赖核查：RayRAG 前端仅判 `code!==0`，无数字分支，收敛安全。

## 批③ 执行记录（2026-10-12，提交 ac16591）

已落地（仓库提交 `api: document-surface batch 3 — response shapes converge to the upstream envelope`）：

- **list**：`data {docs, total, total_datasets}`（补 `total` 双键；探针 doc_list_filter / page_neg / numeric_run）+ `message:"success"`；行形状抽为 `doc_list_entry`（列表与上传共型）。
- **delete**：成功 `{deleted, success_count}`（双键）；失败 → `{code:102, message:"<errors 连接>"}`（`FileService.delete_docs` 形态，无 data 键）。
- **upload（create 路由 `/datasets/{id}/documents`）**：成功 = `{code:0, data:[doc…]}`（数组、无 message，`get_result(data=doc_data)`）；全失败 = `{code:500, message:"{name}: {msg}" 换行连接}`（`file_service.py` err 拼接逐字）；部分成功 = `{code:500, message, data:[ok…]}`（`construct_json_result` 混合信封）；HTTP 200 恒态。
- **ingest**：成功/取消 = `{code:0, message:"success", data:true}`（消息语义差异仍冻结，归批④）。
- **thumbnails**：未解析 / 不可解码 ID 出图外（省略键；探针 `thumb_fake`）；成功补 `message:"success"`。
- **batch-update-status**：成功补 `message:"success"`（`get_json_result(data=result)`）。
- **错误信封族（批中新增发现）**：新增 `api_error_result`（`get_error_data_result` 形态：`{code, message}` 无 `data` 键）并切换文档面错误族——create 路由 102/109、`No file part!`、upload SERVER_ERROR、`document_route_error` 全族（up_both / up_none / SSRF）、images `not_found`、artifacts 三拒绝。此前 `api_error_code` 会序列化 `"data": null`，与探针体（up_both / up_none / img_fake / art_badext / art_missing / del_nobody）不符。
- **测试同步**：12 断言（upload 单 / 多 / 部分 / 拒收族、thumbnails、ingest ×2、delete 契约、list total、traversal）+ MIME 用例补漏 1 处。

验证：`cargo test --locked`（zvec 0.7.2 + 清代理）**2271 passed / 0 failed / 27 ignored**；`cargo fmt --check` 干净；build ✓。

未随批（在案）：ingest 消息语义归批④；附件上传（`/documents/upload`）成功形状为 RayRAG 附件语义——上游 `upload_info` 成功形状未取证（观察项）；batch 逐文档 docStore 错误串（内部不可触发分支）。

## 批④ 执行记录（2026-10-12）

功能级修正落地（五批，均含测试同步与全量验证）：

- **④a 列表查询面（提交 `b379c03`）**：`list_docs` 按上游查询面重写——分页/排序/过滤（`orderby` 白名单、`types`/`run` 校验收敛到上游文案）、`type=filter` → `{total, filter}` 双键；行值本地化（`create_date`/`update_date` → 本地 `YYYY-MM-DDTHH:MM:SS`；`run` 上报短名 `UNSTART/RUNNING/CANCEL/DONE/FAIL` 与数值 `0..4`；`token_count` 按文档聚合（`SearchEngine::token_counts_for`）；`type` 由 `filename_type` 推导）；UI 引入分页循环助手 `fetchAllDocs`（`loadDocs`/`schedulePoll`/`emptyAllDocs`/`exportKb` 四处消费点，`page_size=100` 累加至 `total`）。4 个新测试；全量 **2275 passed / 0 failed**。
- **④b ingest 语义（提交 `2608e78`）**：`doc_ids` 四臂（缺键 → 100 `KeyError('doc_ids')`；null → 100 `TypeError`；标量 → 109；空表 → 成功 `data:true`，不触达鉴权）；`run` 缺键 → 100 `KeyError('run')`；取消守卫（非 RUNNING/CANCEL 且无未完成任务 → 102）；取消成功追加 `"stopped by user"` 标记且 progress 归 0；逐文档首错即中止；成功体 `{code:0, message:"success", data:true}`；`ingest_error` 三键助手（含 `data:null`，≠ 文档面 `api_error_result`）。1 个新测试；全量 **2276 passed / 0 failed**。
- **④c metadata/config raw-body 校验（提交 `726d9c9`）**：doc 级 `get_request_json` 五臂复刻——空体 / `{}` / 无体 → 101 `metadata is required`（两键无 data）；畸形体 → 100 `{data:null, message:"<BadRequest '400: Bad Request'>"}`；JSON 非对象 → 100 `TypeError('JSON payload must be an object.')`；`fields_from_value` pydantic 措辞守卫（list/字段类型/必填）；`Bytes` 裸体提取器替换 `Json<T>`。dataset 级 `validate_and_parse_json_request` 前置链——CT 检查 → `Malformed JSON syntax…`（空体即此）→ `Invalid request payload: expected object, got {type}`；未知数据集改 service 文案 `User '{id}' lacks permission for dataset '{ds}'`（废止 `The dataset doesn't exist` 404）；成功体去 `message` 键、`config_json` 键序字母序；`data_error` 修为无-data 两键（`mcfg_fake` 证据；原 `data:null` 为批② 残留偏差）。round12 实测四态归证（`round12-forensics/`）。测试同批扩展（未知集 404→200+102、非法 schema 400→200、doc 级四臂、dataset 级三臂、无 message、失败体不污染存储）。全量 **2276 passed / 0 failed**。
- **④d file-commit 信封逐字复刻（提交 `31815fe`，owner 2026-10-12 决策）**：`string_code_error`（字符串 code + `Sorry! Data missing!` 恒文案 + HTTP 200）、`raised_value_error`（`{code:100, data:null, message:"ValueError(\"…\")"}`）、`missing_arguments`（缺参合集单答案、声明序、尾 `"; "`）、`find_any` 先按 id 后归域（no-such-commit / belongs-elsewhere 两文案）、versions 先判文件可见性（"File not found" quirk；可见无历史 → 空列表）、`page_size` 缺省 30 / >100 → ValueError repr；三个提交构建器按上游键集拆分（create 八键 / row 九键 / detail 八键）并按 jsonify 字母序书写；diff/versions 行键序字母序、versions `hash = new or old or ""`。测试两处扩展；全量 **2276 passed / 0 failed**。
- **④e PATCH 字段面（提交 `21e994f`）**：`rename_doc` 升级为上游 `update_document` 全字段面——round13 构造性零写取证（**26 键成功体** + 17 校验臂 + raised 信封逐字）后逐字复刻：裸体读取（空体归一 `{}`；畸形体 → 100 `<BadRequest '400: Bad Request'>`；非对象 → 100 `TypeError('JSON payload must be an object.')`）；`name:null` 类型错误先行（所有权之前）；`UpdateDocumentReq` pydantic 措辞逐字（102；`PydanticCustomError` 无「Value error, 」前缀）；`validate_document_update_fields`（不可变三值 / 255 字节·扩展名（双方 `.lower()` 比较）·重名 / `chunk_method` 空串与自动检测面守卫）；更新分支按服务层序（meta_fields → name → `parser_id` 守卫 → parser_config → pipeline_id/chunk_method → enabled），方法变更或 pipeline 残留 → `reset_document_for_reparse`（progress 归零 + 计数器释放 + docStore 清块），缺省 `parser_config` 用 `get_parser_config` 部署默认值替换；响应 `{code:0, data:<26 键全文档体>}`（`map_doc_keys` 字母序）；UI 两处 rename 忽略响应体。新增集成测试 1（26 键键序 + 22 校验臂逐字 + raised 信封 + 写臂全链）；全量 **2278 passed / 0 failed**。

在案（未随批，按队列推进）：

- `type=web`/`type=empty` 上传 + `parent_path`/`return_raw_files`/`parser_config` 覆写；`The dataset not own the document {id}.` 分支复核。
- ④e 已审计差异（在案）：`meta_fields=null` 臂为零写跳过（上游进 `update_document_metadata(None)`，HTTP 面同为成功，未以写取证）；`meta_fields` 值的 `_split_combined_values` 分隔符切分未复刻（内部存储语义，HTTP 面不可观察）；`parser_id` 守卫的类型由当前文件名推导（上游用上传时存储列，改名后边缘分歧）。
- file-commit 家族成功形状（create 之外 tree/diff 属写路径，零污染政策下留观察）。

批④ 全清（④a–④e）——合并 **v0.46.0** 发版（含 `.gitignore` 修正）。

