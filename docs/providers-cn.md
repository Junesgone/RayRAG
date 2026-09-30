# 国内直连模型端点推荐（中国大陆网络适配）

RayRAG 的模型接入统一走 OpenAI 兼容协议：

- **LLM / 视觉**：配置里 `model@provider`（如 `Qwen/Qwen2.5-VL-7B-Instruct@SILICONFLOW`），提供商目录在 `src/data/providers/llm_factories.json`（63 提供商 / 942 模型，含类型标签 `t`：chat / embedding / rerank / image2text / tts / speech2text）。
- **Embedding**：环境变量 `EMBED_API_BASE` + `EMBED_API_KEY` + `EMBED_MODEL`（OpenAI 兼容 `/embeddings`）。
- **Rerank**：环境变量 `RERANK_API_BASE` + `RERANK_API_KEY` + `RERANK_MODEL`（OpenAI 兼容 `/rerank`）。
- 本机 GPU（llama.cpp 已实测 8/8）优先：
  - LLM/视觉：`http://localhost:8088`，`Qwen3.5-9B-Q4_K_M.gguf`
  - Embedding：`http://localhost:8888`，`Qwen3-Embedding-4B`
  - Rerank：`http://localhost:8081`，`mxbai-rerank-large-v2`（服务须带 `--reranking`）

---

## 一、Embedding 推荐（EMBED_API_BASE / EMBED_MODEL）

| 提供商 | OpenAI 兼容端点 | 推荐模型 | 备注 |
|---|---|---|---|
| 硅基流动 SILICONFLOW | `https://api.siliconflow.cn/v1` | `BAAI/bge-m3`、`Qwen/Qwen3-Embedding-4B`、`Qwen/Qwen3-Embedding-8B` | 免费额度友好，中文最优 |
| 通义 Tongyi-Qianwen | `https://dashscope.aliyuncs.com/compatible-mode/v1` | `text-embedding-v3`、`text-embedding-v4` | 阿里云百炼兼容模式 |
| 智谱 ZHIPU-AI | `https://open.bigmodel.cn/api/paas/v4` | `embedding-3` | |
| GiteeAI | `https://ai.gitee.com/v1/` | `Qwen3-Embedding-4B`、`bce-embedding-base_v1` | |
| 302.AI | 见官网（聚合器） | `jina-clip-v2` | 聚合多家 |
| Jiekou.AI | 见官网（聚合器） | `baai/bge-m3` | 聚合多家 |

## 二、Rerank 推荐（RERANK_API_BASE / RERANK_MODEL）

| 提供商 | 端点 | 推荐模型 |
|---|---|---|
| 硅基流动 | `https://api.siliconflow.cn/v1` | `BAAI/bge-reranker-v2-m3`、`netease-youdao/bce-reranker-base_v1` |
| 通义 | `https://dashscope.aliyuncs.com/compatible-mode/v1` | `gte-rerank-v2`、`qwen3-rerank` |
| GiteeAI | `https://ai.gitee.com/v1/` | `Qwen3-Reranker-4B`、`bge-reranker-v2-m3` |
| Jiekou.AI | 见官网 | `baai/bge-reranker-v2-m3` |

## 三、视觉 / 多模态推荐（LLM 工厂 image2text 类型）

| 提供商 | 端点 | 推荐模型 |
|---|---|---|
| 硅基流动 | `https://api.siliconflow.cn/v1` | `Qwen/Qwen2.5-VL-72B-Instruct`、`Qwen/QVQ-72B-Preview` |
| 通义 | `https://dashscope.aliyuncs.com/compatible-mode/v1` | `qwen3-vl-plus`、`qwen-vl-max` |
| 智谱 | `https://open.bigmodel.cn/api/paas/v4` | `glm-4.5v` |
| GiteeAI | `https://ai.gitee.com/v1/` | `Qwen2.5-VL-32B-Instruct`、`ERNIE-4.5-Turbo-VL` |
| 月之暗面 Moonshot | `https://api.moonshot.cn/v1` | `moonshot-v1-128k-vision-preview` |

## 四、Chat / 推理推荐（LLM 工厂 chat 类型，国内直连）

- **DeepSeek**：`https://api.deepseek.com/v1`（`deepseek-chat` / `deepseek-reasoner`）
- **硅基流动**：`Qwen/Qwen3-...`、`deepseek-ai/DeepSeek-V3` 等 66 模型
- **通义**：`qwen3-*` 系列 61 模型
- **智谱**：`glm-4.5` 等 21 模型
- **月之暗面**：`kimi-*` 15 模型
- **MiniMax**：`https://api.minimaxi.com/v1`（国内版用 `https://api.minimax.chat/v1`）

## 五、外网工具的国内替代（agent/tools 对照）

| RAGFlow 外网工具 | RayRAG 国内替代 |
|---|---|
| Wikipedia | `baike`（百度百科） |
| Google / Googlescholar | `baidu` / `bing` / `bocha` / `baidu_scholar` |
| YahooFinance | `eastmoney` / `tencent_finance` / `jin10` |
| akshare / tushare | `akshare`（东财新闻）/ `tushare`（快讯，postgres-only 决策除外） |
| DeepL | `translate`（百度翻译） |
| Tavily / SearXNG | `searxng`（自托管）+ `bing`/`bocha` |
| crawl4ai | `crawler`（SSRF 防护 + html/markdown/content） |
