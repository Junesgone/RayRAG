# Provider 固定图标资产 + UI 复核批次（REA 轮次 17，v0.27.2）

> 轮次：2026-10-21（v0.50.0 切片）
> 基线：RAGFlow v0.27.2 固定树 ~/.cache/ragflow-v0.27.2（commit a024bea0cd93）
> 产物：本文件 + registry「Provider 固定图标资产」+ 台账 8 行 partial→aligned + 模型树行补证（仍 partial）

## 1. 本轮范围

1. web/src/components/model-tree-select.tsx / tree-select.tsx residual：**固定 provider 图片资产**。上游 v0.27.2 的 components/svg-icon.tsx 用 IconMap + svgIcons 把一部分 factory 映射到 assets/svg/llm/*.svg 固定图片，其余走 iconfont；RayRAG 只有 13 个旧资产（其中 jiekouai/wenxin 为旧版遗留），mapping 仅覆盖 10 个 factory。
2. 一批「实现已完整但状态保守」的 UI 行（login-next、skills、chat-channel、setting-model、dataset testing、profile）需要当前构建的 CDP 复核实证。

## 2. 静态对照（v0.27.2）

- 上游 web/src/assets/svg/llm/ 共 **29** 个 SVG；svgIcons 列表含 LocalAI/Gemini/StepFun/MinerU/PaddleOCR/N1n/Avian/RAGcon/SoMark/NewAPI/Astraflow(CN)/FuturMix/Xiaomi/YouDao/BAAI/NomicAI/SentenceTransformers/Grok/FastEmbed/HuaweiCloud/OrcaRouter/Qiniu/TokenHub/FunASR/AIMLAPI/GreenPT/Synthorai/MWS（FishAudio/TogetherAI/Meituan/Longcat/Jiekou/Perplexity/MinerU 走 themeIcons：-bright/-dark）。
- 上游 IconMap：factory→图标 id；iconfont 走 #icon-<id>，svg 走 assets/svg/llm/<id>.svg。
- RayRAG provider_icon(&entry.name) 已按同语义返回 ProviderIcon::Svg/Font，但 svg 分支只映射 10 个名字，且静态树只有 13 个文件（8 个是上游 v0.27.2 子集 + 5 个旧版遗留）。

## 3. 落地（v0.3.10am）

1. **资产入库**：复制上游 web/src/assets/svg/llm/*.svg 全量到 web/static/assets/svg/llm/（现 31 个，含保留的 jiekouai/wenxin 旧资产；新增 aimlapi/astraflow/baai/fastembed/funasr/futurmix/greenpt/grok/huaweicloud/mws/nomic-ai/orcarouter/qiniu/sentence-transformers/synthorai/tokenhub/xiaomi/youdao 等）。
2. **mapping 补齐**：provider_icon 新增 aimlapi.com→aimlapi.svg、Astraflow(-CN)→astraflow.svg、FastEmbed/FunASR/FuturMix/GreenPT/MWS/Synthorai/Xiaomi 等；仅 factory RAGFLOW_LLM_FACTORIES_JSON 中实际存在的名字才映射（BAAI/NomicAI/YouDao 等未在 v0.27.2 factory 列表，保持 font fallback）。
3. **测试**：provider_icons_resolve_svg_font_and_fallback 扩展为表驱动——对每个 svg factory 断言返回 Svg(file) 且 web/static/assets/svg/llm/<file> 存在；iconfont/未知 factory 仍返回 Font。
4. **CDP 脚本修整**：profile-page-cdp.mjs 对齐 v0.49.0 的共享裁剪弹窗（pick→#avatarCropOk→64×64 PATCH），编辑按钮/取消改 DOM click 规避固定 header 拦截，新增姓名两条客户端规则断言（101 字符→Name must be at most 100 characters、bad<name>→Only letters, numbers and . _ '- are allowed）。
5. **复核批次**：login-bg、login-next-form(7/7)、skills-config、cc-dialog(PASS)/cc-zh(PASS)、model-page-layout、dataset-testing、profile-page（0 error / 0 pageError）全部通过；据此翻正 8 行并逐行写证据。

## 4. 证据

- 全量 cargo test --locked：2290 passed / 0 failed / 27 ignored（+集成 6）。
- 台账：8 行 partial→aligned（login-next/bg、login-next/index、skills search-config-modal、chat-channel constant、setting-model index、dataset testing-form、profile constants、profile index），**partial 19→11**；model-tree-select 行补资产证据但保持 partial（跨页 consumer 未全接）。
- CDP 日志（内部）：/tmp/ui2-*.log；profile 结果含 nameMaxError/nameCharsetError/uploadedIsPng=true/serverAvatarAfterRemove=""。
- registry 新增「Provider 固定图标资产」：映射表、静态路径、验证方式。

## 5. 在案（未随批）

- model-tree-select 剩余跨页 consumer：agent parser 表单（word/pdf/spreadsheet/video/text-html）、chat/agent LLM setting items、layout-recognize-form-field；tree-select 的 Radix 视觉 focus 仍为近似。
- search-related-cdp.mjs、search-page-rerank-cdp.mjs 等旧脚本仍指向 v0.24 版侧栏/旧 Search App 详情页选择器，v0.27.2 抽屉化后这些脚本不再适用（本轮未用于翻正）。
- data-source（3 行）仍缺 per-source search/真实 token minting；box/gmail 真实兑换需外部 OAuth 凭据。
- sandbox-settings 的 E2B/Aliyun 真实执行仍在案；admin sandbox 两 CDP 脚本需 /tmp/rr-sshd fixture。
- /search?sapp=... 页面在带 seed 的特定路径偶发 searchRerankToggle is not defined（本轮重跑未复现，2/3 次干净），已记入待观察，暂不翻动相关行。
