//! Retrieval and navigation tool implementations — RAGFlow v0.27.2
//! `rag/advanced_rag/harness/tools/`.
//!
//! This package holds the *implementations* the action-session runtime calls:
//!
//! * `search` — hybrid / BM25 / grep retrieval plus compiled-structure expansion.
//! * `navigation` — dataset navigation tree, document structure navigation and
//!   knowledge-graph exploration.
//! * `exploration` — re-exports `graph_explore` and hosts `wiki_query`.
//!
//! Tool *schemas* and per-turn dispatch live in the action-session module
//! (`_TOOL_MAP` / `execute_tool`), which is what the model actually sees.
//!
//! Note: upstream's declarative `TOOL_REGISTRY` (registry.py + gating.py +
//! pipeline.py) previously lived here and registered 17 tools on import; it was
//! never read by any live path and has been removed upstream — `_active_tool_specs`
//! is the single place that decides the visible tool surface.
//!
//! The implementations land with the action-session batch; this package root
//! already carries the upstream layout.
