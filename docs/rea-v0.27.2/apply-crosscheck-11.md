# Sandbox executor sidecar（REA 轮次 22，v0.27.2）

> 轮次：2026-10-21（v0.55.0 切片）
> 基线：RAGFlow v0.27.2 固定树 ~/.cache/ragflow-v0.27.2（commit a024bea0cd93）
> 产物：本文件 + registry「Sandbox provider 执行」+ 台账最后 1 行 partial→aligned（**partial 归零**）

## 1. 本轮发现

- 上游 v0.27.2 的沙箱执行面：local / self_managed / ssh / tenki / ucloud 各有实现；**E2B provider 的 execute_code 仍是 TODO**（直接 raise "E2B provider is not yet fully implemented"），Aliyun Code Interpreter 用专有 agentrun-sdk（AgentRun 2025-09-10 OpenAPI）实现。
- RayRAG 现状：local/self_managed/ssh 已可真实执行；E2B/Aliyun 的 test endpoint 如实报 "not implemented"。对 E2B 而言这与上游一致；对 Aliyun 而言是真实缺口（上游可执行），但把专有 SDK/云 API 签名搬进 Rust 镜像不现实，也会让构建依赖不可控。

## 2. 落地（v0.3.10ar）

1. run_sidecar_probe(provider_type, config, executor_url)：
   - 先校验上游 initialize 的必填字段（e2b: api_key；aliyun: access_key_id/access_key_secret/account_id），缺失即返回上游风格错误，不发网络请求。
   - RAYRAG_SANDBOX_EXECUTOR_URL 配置时，把 {provider_type, config, code: SANDBOX_TEST_CODE, language: "python", timeout: min(config.timeout,30)} POST 给部署侧 executor（sidecar 内可用官方 agentrun-sdk / E2B SDK 真实执行）。
   - 解析 {success, exit_code, execution_time, stdout, stderr}；成功判据与上游一致（exit_code==0 && stdout 含 TEST_PASSED），消息/详情用同款 test_message/test_payload 格式。
   - 未配置 executor 时明确报 "Set RAYRAG_SANDBOX_EXECUTOR_URL..."，不伪造成功。
2. test_connection 路由把 e2b / aliyun_codeinterpreter 接到该 probe（其余 provider 行为不变）。
3. 测试：3 条单测（本地 one-shot HTTP executor 断言请求体含 provider/config/probe 代码、解析 stdout 并给出 Test PASSED；缺 executor 报 RAYRAG_SANDBOX_EXECUTOR_URL；缺凭据分别报 api_key / access_key_secret required）+ server 端点测试更新（e2b/aliyun 无 executor 报 executor 提示、e2b 缺 key 报凭据）。
4. mock model server 增加 POST /sandbox/execute（返回 TEST_PASSED stdout，记录请求）。
5. CDP 探针 sandbox-executor-cdp.mjs：/admin/sandbox-settings 选 E2B（填 api_key）与 Aliyun（填 AK/SK/account）→ 点 Test → modal 显示 "Successfully connected to sandbox provider" + Test PASSED + stdout。

## 3. 证据

- 单测：sandbox_admin 7 passed（新增 3 条 sidecar + 更新端点测试）。
- API E2E（debug + mock executor）：e2b 与 aliyun 的 POST /api/v1/admin/sandbox/test 均 success=true、message "Test PASSED | Exit code: 0 | Execution time: 0.01s | Output: ...TEST_PASSED..."、details.stdout 完整；mock 收到两条含完整 probe 代码与 config 的 JSON。
- CDP：E2B/Aliyun 均 summary "Successfully connected to sandbox provider"、stdout 含 TEST_PASSED，0 pageError；截图 sandbox-e2b-test / sandbox-aliyun-test。
- 全量 cargo test --locked：2296 passed / 0 failed / 27 ignored（+集成 6）。
- 台账：sandbox-settings → aligned；**partial 0 / unmapped 0**。

## 4. 在案（部署语义，不再是台账缺口）

- 部署方需提供 executor sidecar（wrapping agentrun-sdk / E2B SDK）并设置 RAYRAG_SANDBOX_EXECUTOR_URL；未配置时 console 如实报错。
- E2B 上游自身 v0.27.2 未实现执行，RayRAG 与之对齐；Aliyun 的真实执行由 sidecar 的官方 SDK 完成。
- 旧 CDP 脚本（search-related 等）仍按 v0.24 侧栏写法，属内部复核工具维护项。
