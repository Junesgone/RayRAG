# RayRAG

[English](README.md) · **简体中文**

**一句话：把你的一堆文档（PDF / Word / Excel / PPT / 图片 / 网页）丢进去，就能用大白话提问，它从你的文档里找答案。**

- 用 **Rust** 写，启动后只占 **几十 MB 内存**（RAGFlow 启动约 1 GB）。
- 界面和功能对标开源项目 RAGFlow，但**不需要** MySQL、Elasticsearch、Redis、MinIO：
  向量库用 [zvec](https://github.com/zvec-ai/zvec-rust)，元数据用 **PostgreSQL 18.4**。
- 全程**国内网络可用**：镜像、依赖源、联网搜索都用国内可达的地址。
- 支持 **65 家模型厂商**（DeepSeek、通义千问、智谱、月之暗面、硅基流动、豆包、百川……）
  以及本机跑的 Ollama / vLLM / LM Studio / llama.cpp。
- 不想用 Docker 也行：直接跑一个二进制文件（纯 Linux 模式）。

---

## 目录

- [1. 五分钟跑起来（傻瓜式）](#1-五分钟跑起来傻瓜式)
- [2. 打开网页，第一次使用](#2-打开网页第一次使用)
- [3. 接入大模型（让问答变聪明）](#3-接入大模型让问答变聪明)
- [4. 常见问题（遇到问题先看这里）](#4-常见问题遇到问题先看这里)
- [5. 日常维护：升级 / 备份 / 卸载](#5-日常维护升级--备份--卸载)
- [6. 不用 Docker：纯 Linux 安装](#6-不用-docker纯-linux-安装)
- [7. 这个项目有什么](#7-这个项目有什么)
- [8. 更多文档](#8-更多文档)

---

## 1. 五分钟跑起来（傻瓜式）

### 第一步：确认电脑上装了 Docker

打开终端（命令行），输入：

```bash
docker compose version
```

- **看到版本号**（例如 `Docker Compose version v2.24.0`）→ 继续下一步。
- **提示 command not found** → 先装 Docker：
  - Windows / macOS：装 [Docker Desktop](https://www.docker.com/products/docker-desktop/)，装完重启。
  - Linux：`curl -fsSL https://get.docker.com | sh`

> 没装 Docker、或者不想用 Docker？直接跳到 [第 6 节：纯 Linux 安装](#6-不用-docker纯-linux-安装)。

### 第二步：下载并一键部署

```bash
git clone https://github.com/Junesgone/RayRAG.git
cd RayRAG
./install.sh
```

`install.sh` 会自动帮你做完剩下的事：

1. 生成配置文件 `.env`，并**随机生成两个密码**（PostgreSQL 密码、管理员登录密码）；
2. 用**国内镜像源**拉取基础镜像、下载依赖（首次约 5～15 分钟，取决于网速）；
3. 启动数据库和应用，并等待服务真正健康；
4. 在屏幕上打印访问地址和登录账号密码。

跑完后你会看到类似这样的提示（脚本会跟随系统语言，英文环境输出英文）：

```
============================================================
  RayRAG 已经跑起来了 🎉
------------------------------------------------------------
  网页地址 : http://192.168.1.10:9380
  本机访问 : http://127.0.0.1:9380
  登录邮箱 : admin@rayrag.local
  登录密码 : xxxxxxxxxxxxxxxxxxxxxxxx   （已写入 .env，请妥善保存）
------------------------------------------------------------
  查看日志 : docker compose logs -f rayrag
  停止服务 : docker compose down
  重启服务 : docker compose restart
============================================================
```

> **重要：把这个地址和密码记下来，登录密码只存在 `.env` 文件里。**

### 常用参数（可选）

```bash
./install.sh --port 8080        # 换一个网页端口（默认 9380）
./install.sh --global-mirror    # 构建时直连国外源（国内一般不需要）
./install.sh --no-build         # 只启动，不重新构建镜像
./install.sh --dry-run          # 只做检查和生成 .env，不构建不启动
./install.sh --help             # 看全部参数
```

### 不想用脚本？手动三条命令也行

```bash
cp .env.example .env
# 用编辑器打开 .env，把 RAYRAG_ADMIN_PASSWORD 和 RAYRAG_POSTGRES_PASSWORD
# 改成你自己的密码（各至少 12 位），保存
docker compose up -d --build
```

---

## 2. 打开网页，第一次使用

1. **打开浏览器**，访问 `http://你的主机IP:9380`
   （本机就是 `http://127.0.0.1:9380`）。
2. **登录**：邮箱 `admin@rayrag.local`，密码是 `.env` 里的 `RAYRAG_ADMIN_PASSWORD`。
3. **建知识库**：左侧菜单 `Knowledge Base` → `Create knowledge base` → 填名字 → 保存。
4. **传文档**：进入刚建的知识库 → `Files` 页签 → `+ Add file` 拖入 PDF/Word/Excel/图片 → 确定。
5. **解析**：勾选文件 → 点 `Parse`（绿色播放按钮）→ 等状态变成 `DONE`。
   > 大文件解析慢是正常的，可以在 `Logs` 页签看进度。
6. **开始提问**：
   - 想快速试：左侧 `Search` → 输入问题 → 回车，会列出命中的原文片段；
   - 想要 ChatGPT 式问答：左侧 `Chat` → `Create chat assistant` → 选知识库 → 开始对话。

界面上几乎所有按钮都做了**中文/英文双语**，右上角头像 → `Language` 可切换。

---

## 3. 接入大模型（让问答变聪明）

**不接模型也能用**：文档解析、分块、关键词检索都能跑；但"对话回答"和"向量语义检索"
必须要有一个大模型和一个向量模型。RayRAG **不自带模型**，也不绑定任何一家厂商。

### 3.1 在网页上接（推荐，最简单）

1. 右上角头像 → **`Model providers`**（模型供应商）
2. 在 **Available models** 里找到你要用的厂商（例如 `DeepSeek`），点卡片上的 `Add`
3. 填三样东西：
   - **Instance name**：随便起个名，例如 `my-deepseek`
   - **API-Key**：在厂商控制台申请的 key
   - **Base-Url**：一般不用改，除非你用中转/自建服务
4. 点 **`Verify`** 测试连通 → 通过后点 **`Ok`**
5. 回到页面顶部 **Set default models**，把 `Chat model` / `Embedding model` 选成刚添加的模型
   （`Rerank model`、`Vision model`、`OCR`、`TTS`/`ASR` 按需选）

### 3.2 国内常见厂商对照表

| 厂商 | 在 RayRAG 里选 | Base-Url（一般自动填好） | 备注 |
|---|---|---|---|
| 深度求索 DeepSeek | `DeepSeek` | `https://api.deepseek.com/v1` | 便宜好用，chat 首选 |
| 阿里通义千问 | `Tongyi-Qianwen` | DashScope 兼容地址 | chat + embedding 都有 |
| 硅基流动 | `SILICONFLOW` | `https://api.siliconflow.cn/v1` | 一家搞定 chat/embedding/rerank |
| 智谱 AI | `ZHIPU-AI` | `https://open.bigmodel.cn/api/paas/v4` | 有免费额度 |
| 月之暗面 Kimi | `Moonshot` | `https://api.moonshot.cn/v1` | 长文档友好 |
| 火山方舟（豆包） | `VolcEngine` | 方舟控制台给的地址 | 需先创建推理接入点 |
| 百度千帆 | `BaiduYiyan` | 千帆控制台给的地址 | — |
| 讯飞星火 | `XunFei Spark` | — | — |
| 腾讯混元 | `Tencent Hunyuan` | — | — |
| 任何"OpenAI 兼容"服务 | `OpenAI-API-Compatible` | 你的服务地址 | 中转站、自建网关都用这个 |

### 3.3 本机显卡跑模型（免费、数据不出门）

| 你用的工具 | 在 RayRAG 里选 | 默认地址 |
|---|---|---|
| Ollama | `Ollama` | `http://127.0.0.1:11434` |
| vLLM | `VLLM` | `http://127.0.0.1:8000/v1` |
| LM Studio | `LM-Studio` | `http://127.0.0.1:1234/v1` |
| llama.cpp (llama-server) | `OpenAI-API-Compatible` | `http://127.0.0.1:8080/v1` |
| Xinference | `Xinference` | `http://127.0.0.1:9997` |
| GPUStack | `GPUStack` | 见 GPUStack 面板 |

> Docker 里访问宿主机上的模型服务，地址要写 `http://host.docker.internal:11434`
> （Linux 上写宿主机内网 IP，例如 `http://192.168.1.10:11434`）。

### 3.4 用环境变量接（适合批量部署）

编辑 `.env`，填这几行，然后 `docker compose up -d`：

```ini
LLM_API_BASE=http://192.168.1.10:8088/v1     # 大语言模型地址
LLM_API_KEY=sk-xxxx
LLM_MODEL=Qwen3.5-9B-Q4_K_M.gguf

EMBED_API_BASE=http://192.168.1.10:8888/v1   # 向量模型地址
EMBED_API_KEY=
EMBED_MODEL=Qwen3-Embedding-4B-Q4_K_M.gguf

RERANK_API_BASE=http://192.168.1.10:8899/v1  # 重排模型（可选）
RERANK_MODEL=bge-reranker-v2-m3
```

> 划重点：**本地模型服务如果和 RayRAG 不在同一台机器，"127.0.0.1" 一定不通**，
> 要写实际 IP。装完可以在 `System` 页面看各链路是否连通。

---

## 4. 常见问题（遇到问题先看这里）

**Q1：`install.sh` 报 `docker: command not found` 或 `docker compose` 不识别？**
先装 Docker（见第 1 节第一步）。Linux 上还要确认当前用户在 docker 组里：
`sudo usermod -aG docker $USER`，然后**退出重新登录**。

**Q2：9380 端口被占用，起不来？**
换端口：`./install.sh --port 8080`。已经装过的，改 `.env` 里的 `RAYRAG_PORT` 再
`docker compose up -d`。

**Q3：管理员密码有要求吗？**
至少 **12 位**。太短会启动失败并提示
`RAYRAG_ADMIN_PASSWORD must be at least 12 characters`。

**Q4：忘记登录密码 / 密码写错了？**
改 `.env` 里的 `RAYRAG_ADMIN_PASSWORD` 后执行：
```bash
docker compose down && docker volume rm rayrag-state && ./install.sh
```
> 注意：这样会**清空已有数据**。只想改密码请到网页里 头像 → `Profile` → `Password`。

**Q5：构建/拉镜像太慢，或者卡在下载？**
默认已经走国内镜像（DaoCloud + 清华 apt 源 + 国内 crates 源）。
如果仍然慢，可以显式指定国内档位后重试：
```bash
RAYRAG_MIRROR_PROFILE=cn docker compose build rayrag
```

**Q6：问答说"没有找到相关内容"？**
按顺序检查：① 文档状态是不是 `DONE`（`Files` 页签看）；
② `Set default models` 里 embedding 模型有没有选；③ `System` 页看 embedding 链路是否连通。

**Q7：上传的文档去哪了？我的数据存在哪？**
全部在 Docker 卷里：`rayrag-state`（文件与业务数据）、`rayrag-zvec-data`（向量索引）、
`rayrag-postgres-data`（数据库）。不会上传到任何第三方服务器。

**Q8：能处理图片、扫描件 PDF 吗？**
能，需要配一个 OCR / 视觉模型（`OCR` 页签或 `Vision` 模型）。
国内可选 PaddleOCR、MinerU、SoMark，也可以接本机的视觉大模型。

**Q9：为什么我的 reranker 用不了？**
本机显卡被大语言模型占满时，reranker 会加载失败。要么把 reranker 放到另一台机器/另一个端口，
要么在 `Set default models` 里**不选** rerank 模型（检索照常工作，只是不做重排）。

**Q10：怎么确认服务真的好了？**
```bash
docker compose ps                      # 两个服务都应显示 healthy
curl http://127.0.0.1:9380/api/v1/system/healthz
# {"code":0,"data":{"status":"healthy","postgres":"healthy","zvec":"healthy"}}
```

**Q11：能不能不用 Docker？**
可以，见下一节。

**Q12：支持 Windows / macOS 吗？**
Docker 部署两个系统都能跑（Apple Silicon / arm64 也支持）。纯二进制模式目前只提供 Linux。

---

## 5. 日常维护：升级 / 备份 / 卸载

```bash
# 升级到最新代码
git pull
./install.sh                     # 重新构建并启动

# 看日志（排错必备）
docker compose logs -f rayrag

# 停止 / 启动
docker compose stop
docker compose start

# 备份（把数据卷打包到当前目录）
docker run --rm -v rayrag-state:/data -v "$PWD":/backup alpine \
  tar czf /backup/rayrag-backup-$(date +%F).tar.gz -C /data .

# 卸载（保留数据）
docker compose down

# 卸载并删除全部数据（不可恢复！）
docker compose down -v
```

---

## 6. 不用 Docker：纯 Linux 安装

需要三样：**Rust 1.97+**、一个 **PostgreSQL 18.4**、**zvec 动态库**。

```bash
# 1) 装 Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"

# 2) 编译（PostgreSQL 元数据 + zvec 原生向量后端，两者都是默认 feature）
#    ZVEC_LIB_DIR 指向含 libzvec_c_api.so 与 TARGET 的目录
export ZVEC_LIB_DIR="$HOME/.local/lib/zvec/0.7.1"
cargo build --release --locked

# 3) 配置
export RAYRAG_POSTGRES_URL=postgresql://rayrag:你的密码@127.0.0.1:5432/rayrag
export RAYRAG_ADMIN_EMAIL=admin@rayrag.local
export RAYRAG_ADMIN_PASSWORD='至少12位的密码'

# 4) 启动
./target/release/rayrag serve --port 9380
```

`postgres-backend` 与 `zvec-backend` 都是默认 feature，所以上面的命令就是发行配置，二进制**默认用 zvec**：
用 `RAYRAG_ZVEC_DIR=/var/lib/rayrag/zvec` 指定向量数据目录（默认 `./zvec-data`）。构建时解析到的
`ZVEC_LIB_DIR` 会写进二进制的 runpath，运行时不需要再设 `LD_LIBRARY_PATH`。若环境里没有原生库，可用
`cargo build --release --locked --no-default-features --features postgres-backend` 退回可移植的 JSON 索引
（并把 `RAYRAG_VECTOR_BACKEND=json`）。

> 详细参数、systemd 写法与私有化部署注意事项见 [`docs/advanced.md`](docs/advanced.md)。

---

## 7. 这个项目有什么

| 页面 | 能干什么 |
|---|---|
| `Knowledge Base` | 建知识库、上传/解析文档、切片查看与编辑、知识图谱、检索测试、日志 |
| `Chat` | 建对话助手、流式回答、引用溯源、会话管理 |
| `Search` | 多知识库联合检索、重排、相关搜索推荐 |
| `Agent` | 可视化画布编排：检索、联网搜索、代码执行、条件分支、循环；每个智能体都有运行日志，可检索、排序、导出 CSV |
| `Files` | 统一管理所有已上传文件 |
| `Skills` | 技能检索配置、技能索引 |
| `Memories` | 长期记忆（对话记忆写入与召回） |
| `Model providers` | 65 家厂商 / 本机模型的接入与默认模型设置 |
| `Data sources` | 从 S3、Notion、语雀、飞书、GitLab 等 35 种来源同步文档 |
| `System` / `Admin` | 运行状态、用户与团队、权限、监控 |
| `API` | OpenAI 兼容接口：`/api/v1/chat/completions`、`/embeddings`、`/rerank`，可直接接 LangChain、OpenWebUI、Dify |

**技术栈**：Rust（Axum + Tokio）· zvec 向量库 · PostgreSQL 18.4 · 单二进制部署。

**与 RAGFlow 的关系**：界面层级、按钮、交互对齐 RAGFlow v0.26.4；
解析管线用 Rust 重写；把 MySQL / Elasticsearch / Redis / MinIO 换成了
PostgreSQL + zvec，部署更轻。

---

## 8. 更多文档

| 文档 | 内容 |
|---|---|
| [`README.md`](README.md) | 本 README 的英文版 |
| [`docs/advanced.md`](docs/advanced.md) | 架构、完整环境变量、密码与 TLS、Agent 画布执行器、各解析器接入 |
| [`docs/providers-cn.md`](docs/providers-cn.md) | 国内网络环境下的连接器与搜索源选择 |
| [`CHANGELOG.md`](CHANGELOG.md) | 每个版本改了什么 |
| [`NOTICE`](NOTICE) | 对标来源与第三方资产说明 |

---

## 许可证

Apache License 2.0，见 [`LICENSE`](LICENSE)。
