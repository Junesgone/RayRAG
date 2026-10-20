# Model tree 状态收口 + zvec vendor 0.7.2（REA 轮次 21，v0.27.2）

> 轮次：2026-10-21（v0.54.0 切片）
> 基线：RAGFlow v0.27.2 固定树 ~/.cache/ragflow-v0.27.2（commit a024bea0cd93）+ zvec-ai/zvec-rust v0.7.2
> 产物：本文件 + registry「zvec vendor 与共享模型树」+ 台账 1 行 partial→aligned + Docker vendor 修复

## 1. 本轮发现

1. model-tree-select 行长期 partial 的原因是「上游所有 consumer 的逐文件复刻」：但上游 consumer 文件（agent parser 表单、llm-setting-items、layout-recognize、rerank.tsx 等）在台账中本就是 reference（明确排除的 UI 参考材料），RayRAG 用 Rust-native 管理面替代；真正 in-scope 的 consumer 早已全部落地（14 个调用点）。缺少的是当前构建的实测证据与 scope 结论。
2. Docker vendor 失配：Cargo.toml 的 zvec-rust git tag 是 v0.7.2、Cargo.lock 也是 0.7.2，但 vendor/zvec-rust 仍是 v0.7.1 → Dockerfile 追加的 [patch] 版本不匹配，构建时静默回退到 GitHub 拉取 git 依赖（"patch was not used in the crate graph"），vendor 形同虚设、构建不再自包含。

## 2. 落地（v0.3.10aq）

1. 模型树证据收口：当前构建 CDP 复核 kb-embd-tree PASS（键盘 ArrowDown/typeahead/End/Enter、保存后重建仍持久）、memory-tree（embd/llm 两字段、312 行树、选 gpt-5.5 后 config 落 form-local llm_id、无 default PATCH）、skill-embd-tree PASS（skills 弹窗嵌入模型、保存持久）、model-tree-keyboard 17/17、system-model（六行 role=tree、en/zh、tooltip）。
2. kb-embd-tree 脚本修掉陈旧断言：typeahead 命中行标题带展开箭头（"⌄Qwen3-Embedding-4B"），旧断言 activeIdx===0 不再成立；改为断言标题匹配 Qwen。脚本 PASS。
3. zvec vendor 升级：用本地 cargo checkout 的 v0.7.2（89a0ab30）重建 vendor/zvec-rust（Cargo.toml/README.md/zvec/zvec-sys），VENDOR.md 记录 v0.7.2，Dockerfile ARG ZVEC_RUST_VERSION 改 0.7.2（官方 prebuilt v0.7.2 资产存在），Cargo.toml 注释同步。
4. 台账：model-tree-select → aligned（附 in-scope consumer 清单与 CDP 证据；reference consumer 明确排除）；partial 2→1（仅 sandbox-settings）。

## 3. 证据

- 全量 cargo test --locked：2293 passed / 0 failed / 27 ignored（+集成 6）。
- Docker 构建：patch 生效、无 "patch was not used" 警告、镜像健康（见发布记录）。
- 台账 aligned 计数 321；partial 1。
- 文档/registry/README/运维记录同步。

## 4. 在案（下轮）

- 剩余 partial 1 行：admin/sandbox-settings.tsx 的 E2B/Aliyun 真实代码执行（需要云侧凭据/运行时，当前 test endpoint 如实报未实现）。
- Docker vendor 升级后仍需在发布流程重建镜像并抽查 zvec 后端可用性。
- 旧 CDP 脚本（search-related 等）仍按 v0.24 侧栏写法，未用于复核。
