# sessions / audio / mindmap / recommendation（RAGFlow v0.27.2 运行取证）

> REA 轮次⑨产出（2026-10-12）。来源：源码契约（`chat_api.py:826-1253`）+ 15 探针（只读/校验）。
> 工作笔记：`~/.cache/rea/ragflow-v0.27.2/notes/09-sessions-audio.md`；证据：`round9-forensics/`。

## 路由

`POST/GET /chats/{id}/sessions` · `GET/PATCH/DELETE /chats/{id}/sessions/{sid}` · `DELETE …/messages/{mid}` · `PUT …/messages/{mid}/feedback` · `POST /chat/audio/speech` · `POST /chat/audio/transcription` · `POST /chat/mindmap` · `POST /chat/recommendation`

## 要点

- **归属失败统一形**：`{code:109, data:false, message:"no authorization"}`（vs searchbots 域 102）。
- **会话细节**：prologue 注入（首条 assistant=prompt_config.prologue）；orderby 白名单 {create_time,update_time,name}；`messages`/`reference` 不可 PATCH；成对删除（assistant+user + reference 位）；批量删除支持 `{ids}`/`{delete_all:true}`、部分成功 `{success_count,errors}`、blob 清理。
- **反馈链**：thumbup 必 bool；联动 chunk 权重（先撤旧票再投新票）。
- **音频**：speech=按标点切分流式 audio/mpeg（流内错误 code:500 + `**ERROR**`）；transcription=multipart + 扩展名白名单 9 种 + SSE 可选；均用租户默认 TTS/ASR（本实例未配置 → 102 "No default … model is set."）。
- **`get_data_error_result` 双风格**：位置参 → 提示入 `code`；关键字 message → 102+提示。
- **未捕获异常** → `{code:100, message:"<Error>('<repr>')"}`。
- **回退对照**：recommendation 有默认模型回退；searches/completions 无（已知缺口）。
