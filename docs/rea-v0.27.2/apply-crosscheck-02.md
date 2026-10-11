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

\* ingest 行：上游空/伪体行为未实测（源分析为 KeyError→500 族），待实例具备条件后补探针。

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
