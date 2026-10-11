# REA 逆向分析产物（RAGFlow v0.27.2）

> 本目录为 **内部资产**（发布时硬拒排除，不入公开快照）。
> 来源：`morluto/rea` 对 v0.27.2 参考树（commit `a024bea0cd93`）+ 镜像前端产物（web-dist 707 JS）的逆向分析。
> 工作笔记与原始证据：`~/.cache/rea/ragflow-v0.27.2/`（notes/01–09、round3–9-forensics/、round8-ui/、web-dist/）。

## 模块分析（REA 轮次 → 蒸馏件）

| # | 模块 | 蒸馏件 | 核心结论 |
| --- | --- | --- | --- |
| 01 | 登录链 | （见运维记录续记③） | `/api/v1/auth/login`；未注册邮箱 109 |
| 02 | 接口面与迁移 | `api-surface-v0.27.2.md` | 三源 87/86/19；新面家族确认 |
| 03 | 检索与对话链 | `runtime-surface-v0.27.2.md` | 6 端点 200 形状；默认模型悬挂实证 |
| 04 | OpenAI 兼容面 | `openai-surface-v0.27.2.md` | 两段式错误；`model:"model"` 占位符；SSE 细节 |
| 05 | Search App | `search-surface-v0.27.2.md` | 净零生命周期；searchbots=AUTH_BETA |
| 06 | Memory 家族 | `memory-surface-v0.27.2.md` | 四类记忆；**实例检索缺陷**；提示词资产 |
| 07 | 文件提交面 | `file-commit-surface-v0.27.2.md` | 双域语义；错误三态；**RetCode 全表** |
| 08 | UI 面 | `ui-map-v0.27.2.md` | 侧栏 7 项；65 路由/55 块；en·zh 标签 |
| 09 | sessions/audio | `sessions-audio-surface-v0.27.2.md` | 109+data:false 统一形；双风格定型 |

## 资产

- `memory-default-system-prompt.txt` — 记忆默认提取系统提示词（3604 字符，源自实例真实配置）。

## 方法（已成范式）

逐模块四步：**静态地图 → 接口面提取 → 语义追踪 → 运行探针**；补齐三源（前端 API 表 / 官方文档 / 实例存活探针）+ 服务端日志取证；每轮产出工作笔记（~/.cache）+ 蒸馏件（本目录）+ 台账（运维记录.md）。

## 未决总表

- chat 成功形状（默认模型 MiniMax-M3 上游不可达）；
- 记忆语义检索（实例记录 `embd_id` 陈旧格式，待 Boss 确认 DB 修复）；
- 音频成功形状（实例未配置默认 TTS/ASR）；
- 文件提交 create/diff/tree 成功形状（需真实提交，未做写操作）。
