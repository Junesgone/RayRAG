# RayRAG ↔ RAGFlow HTTP 接口对照工作清单（内部文档，禁止发布）

> 生成：`scripts/api-parity-audit.py --write`；数据源 = 固定 RAGFlow v0.27.2 参考树 `api/apps/restful_apis/*.py` 与 `src/server.rs` 的 `.route(...)` 注册表。
> 用途：把「每个上游接口 RayRAG 是否应答」变成可复核的数字，并按批收敛。

## 现状

- 上游路由：**195**；RayRAG `/api/v1` 路由：**345**
- 同名应答（exact）：**145**
- 别名应答（alias，上游拼写已注册但走 RayRAG 自己的命名）：**45**
- 形状不同但已被应答（covered）：**2**
- 尚未实现（missing，已登记）：**3**
- 路径已有但方法缺失（method）：**1**
- 未登记的缺口（mismatch，`--check` 视为失败）：**0**

## 未实现清单（按上游文件分组）

| 上游路径 | 来源文件 | 说明 |
| --- | --- | --- |
| `/datasets/<dataset_id>/skills` | `dataset_api.py` | 功能级缺口：数据集 Skills 面未实现（Skills 索引仅有全局 /api/v1/skills/*） |
| `/datasets/<dataset_id>/skills/<path:skill_kwd>` | `dataset_api.py` | 功能级缺口：同上（Skills 面未实现） |
| `/documents/artifact/<filename>` | `document_api.py` | 功能级缺口：文档产物文件读取未实现 |

## 别名对照（上游拼写 → RayRAG 路由）

| 上游路径 | RayRAG 路由 |
| --- | --- |
| `/compilation-template-groups` | `/api/v1/compilation_template_groups` |
| `/compilation-template-groups/<group_id>` | `/api/v1/compilation_template_groups/{id}` |
| `/compilation-templates/builtins` | `/api/v1/compilation_templates/builtins` |
| `/compilation-templates/wiki-presets` | `/api/v1/compilation_templates/wiki_presets` |
| `/searches/<search_id>/completions` | `/api/v1/searches/{search_id}/completions` |
| `/system/stats` | `/api/v1/stats` |

## 方法级缺口（路径已应答，但上游方法未实现）

| 上游路径 | 来源与缺失方法 |
| --- | --- |
| `/agents/<agent_id>/webhook/test` | agent_api.py [DELETE,GET,HEAD,PATCH,PUT] |
