# 数据源 OAuth token 字段真实兑现（REA 轮次 20，v0.27.2）

> 轮次：2026-10-21（v0.53.0 切片）
> 基线：RAGFlow v0.27.2 固定树 ~/.cache/ragflow-v0.27.2（commit a024bea0cd93）
> 产物：本文件 + registry「数据源 OAuth token 兑现」+ 台账 2 行 partial→aligned + 单测 3 条 + mock token 端点 + CDP 探针

## 1. 本轮发现

- Box/Gmail token 字段的后端兑现链路（start → callback → result）在 v0.3.10ap 前只在「离线中止 popup」下验证过：callback 里的真实 token 兑换分支（BOX_OAUTH_TOKEN_URL / GOOGLE_OAUTH_TOKEN_URL）没有测试也没有端到端证据，台账因此留 partial。
- 代码层面兑换分支是内联的重复块，不可直接单测（读 env）。

## 2. 落地（v0.3.10ap）

1. 抽出 exchange_authorization_code(client_id, client_secret, redirect_uri, code, token_url) 与 box_token_exchange / google_token_exchange 包装；callback 只负责读取 env 并调用。
2. 3 条单测（本地 one-shot TcpListener mock 端点）：
   - box_token_exchange 配置端点时 POST 表单（grant_type/code/client_id/client_secret/redirect_uri）并保存 access/refresh token；
   - 无端点时保持 code、不写 token；
   - google_token_exchange 同上。
3. mock model server 增加 POST /box/token 与 /google/token：接受 form、记录到 /tmp/mock-oauth.log、返回固定 access/refresh token。
4. 新增 CDP 探针 oauth-token-exchange-cdp.mjs：Box start→callback→result；Gmail start→callback→result→用 minted credentials POST /api/v1/data_sources 创建 gmail connector；并回读 mock 请求日志。

## 3. 证据

- 探针（debug :9490，BOX_OAUTH_TOKEN_URL=http://127.0.0.1:18089/box/token、GOOGLE_OAUTH_TOKEN_URL=…/google/token）：
  - box started=true、callbackCompleted=true、resultCode=0、credentials.access_token=box-access-from-mock、refresh_token=box-refresh-from-mock；
  - gmail started=true、callbackCompleted=true、resultCode=0、access_token=google-access-from-mock、refresh_token=google-refresh-from-mock；connector code=0、rowType=gmail；
  - mock 日志两行 form：grant_type=authorization_code&code=…&client_id=…&client_secret=…&redirect_uri=…；0 pageError。
- 单测 oauth_web 7 passed（新增 3 条兑换测试）。
- 全量 cargo test --locked：2293 passed / 0 failed / 27 ignored（+集成 6）。
- 台账：box-token-field、gmail-token-field → aligned；**partial 4→2**（剩 model-tree-select、sandbox-settings）。

## 4. 在案（下轮）

- 剩余 partial 2 行：model-tree-select（agent parser 表单 / chat LLM setting items / layout-recognize 等跨页 consumer）、admin/sandbox-settings（E2B/Aliyun 真实代码执行）。
- token 兑换端点需部署时配置（BOX_OAUTH_TOKEN_URL / GOOGLE_OAUTH_TOKEN_URL）；未配置时保持原语义（回调存 code，result 返回 code）。
- Docker vendor zvec 0.7.1/0.7.2 patch 失效仍待修。
