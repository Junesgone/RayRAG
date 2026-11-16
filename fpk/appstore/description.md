# RayRAG — App Center listing material

This directory holds the copy and screenshots used when submitting RayRAG to the
自托管主机 App Center. The package itself is built from the repository root by
`fpk/build-fpk.sh` and published as `dist/rayrag_<version>_x86_64.fpk`.

## English

**Name** RayRAG

**Tagline** RAGFlow-compatible RAG server in Rust, with built-in PostgreSQL and zvec.

**Overview**
RayRAG is a self-hosted retrieval-augmented generation server for your NAS. It speaks the
RAGFlow v0.27.2 APIs and ships the same dataset, document, chunk, retrieval, chat and agent
workflows in a single Rust binary. PostgreSQL and the zvec vector backend start automatically
inside the package, so there is no extra database or vector service to install. The first-run
guide detects the NAS resources, prefills the defaults and writes them to an editable env file.

**Features**
- Dataset workspace: upload, parse, chunk preview, retrieval testing and parse logs.
- Built-in ingestion pipelines plus the classic chunking methods, with the upstream DSL.
- Chat assistants, search apps and agents with citation answers.
- Model providers, embedding/reranker selection and an OpenAI-compatible model catalogue lookup.
- Data stays on the NAS: PostgreSQL and zvec run locally under the app's data share.
- English and Chinese UI, responsive layout for desktop and mobile browsers.

**First login** admin / admin (change the password after signing in; the first-run guide
opens automatically and saves the configuration to the environment file).

**Ports and data** Web UI: 9395 (can be changed in the manifest). Data: the `rayrag` shared
folder (`pgdata`, `data`, `rayrag.env`).

**Support** https://github.com/Junesgone/RayRAG

## 中文

**名称** RayRAG

**一句话介绍** Rust 编写的 RAGFlow 兼容 RAG 服务，内置 PostgreSQL 与 zvec。

**应用简介**
RayRAG 是运行在飞牛 NAS 上的自托管检索增强生成服务。它兼容 RAGFlow v0.27.2 的接口，
以单个 Rust 二进制提供数据集、文档解析、分块预览、检索测试、聊天助手与智能体等完整流程。
安装包内置 PostgreSQL 与 zvec 向量后端并自动启动，无需额外安装数据库或向量服务。
首次启动的引导界面会自动检测 NAS 资源、预填默认配置，并写入可随时编辑的 env 文件。

**主要功能**
- 数据集工作台：上传、解析、分块预览、检索测试与解析日志。
- 内置数据管道与通用切片方式，兼容上游 DSL。
- 聊天助手、搜索应用与智能体，回答带引用。
- 模型提供商、Embedding/Reranker 选择，支持模型目录查询。
- 数据全部保存在 NAS 本地：PostgreSQL 与 zvec 运行在应用数据共享目录中。
- 中英文界面，桌面与手机浏览器自适应布局。

**首次登录** admin / admin（登录后请修改密码；首次引导会自动打开，并把配置写入 env 文件）。

**端口与数据** Web 界面：9395（可在 manifest 中调整）。数据：`rayrag` 共享目录（`pgdata`、`data`、`rayrag.env`）。

**支持** https://github.com/Junesgone/RayRAG

## Screenshots

The `screenshots/` folder contains images captured from the installed NAS app:

- `home-1440.png`, `home-390.png` — dashboard
- `datasets-1440.png`, `datasets-390.png` — dataset cards
- `dataset-files-1440.png`, `dataset-files-390.png` — files table with parsing controls
- `retrieval-1440.png` — retrieval testing
- `logs-1440.png` — parse logs and statistics
- `configuration-1440.png` — dataset configuration
- `models-1440.png` — model providers
