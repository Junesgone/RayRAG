# 文件提交面（RAGFlow v0.27.2 运行取证）

> REA 轮次⑦产出（2026-10-12）。来源：源码契约（`file_commit_api.py` 全文）+ 12 探针（全只读）+ RetCode 总表。
> 工作笔记：`~/.cache/rea/ragflow-v0.27.2/notes/07-file-commits.md`；证据：`round7-forensics/`。

## 路由（`{prefix}` = `/datasets/<id>` 或 `/workspaces/<id>`）

`POST {prefix}/commits` · `GET {prefix}/commits[?slug]` · `GET {prefix}/commits/{id}` · `GET {prefix}/commits/{id}/files` · `GET {prefix}/commits/diff?from&to` · `GET {prefix}/changes` · `GET {prefix}/commits/{id}/tree` · `GET {prefix}/commits/{id}/files/{file_id}/content` · `GET /workspace-files/{file_id}/versions`

## 要点

- **双域**：`/datasets/*` = artifact 页面历史（folder_id=kb_id，title/comments 扩展）；`/workspaces/*` = 工作区文件提交；`/folders/*`、memories/skills 均未挂载。
- **错误三态**：101 缺参；**字符串 code**——`get_data_error_result("<提示>")` 位置参把提示写进 `code` 而 `message` 恒为 `"Sorry! Data missing!"`；100=EXCEPTION_ERROR（异常 repr 直出）。
- **RetCode 全表**：0/10/100/101/102/103/105/106/108/109 + HTTP 400/401/403/404/409/500（100=EXCEPTION_ERROR、102=DATA_ERROR、103=OPERATING_ERROR、109=AUTHENTICATION_ERROR）。
- **分页信封**：`{total,page,page_size,commits[]}`（默认 page_size 30）。
- **frontend**：UI 以 `listWikiCommits/getWikiCommit/getArtifactGraph` 称呼 datasets 提交（Wiki 页面历史）。
- 实例实证：`.knowledgebase` 文件夹挂载点（32GB）；未提交变更 30,329 条（全 add）；无已提交记录。

## 对标增量（待入实施队列）

file-commit git-like 模型（parent/tree_state）、三态错误信封与 RetCode 全表、分页信封、artifact/workspace 域判别。
