//! Orchestration strategies for the agentic graph — RAGFlow v0.27.2
//! `rag/advanced_rag/harness/orchestrator/`.
//!
//! Each module here is one strategy the graph can run; they are imported
//! lazily by `agentic_rag_graph` at the point of use:
//!
//! * `direct` — single-pass retrieval (the low path).
//! * `sufficient_context` — sufficiency review + gap-driven rewrites.
//! * `query_rewriter` — turns a reported gap into a follow-up query.
//!
//! Which strategies a mode runs is decided in [`crate::harness::config`]
//! (`THINKING_MODES`), not here — see `ModeSpec::enable_sca` / `use_fanout`.
//!
pub mod direct;
pub mod query_rewriter;
pub mod sufficient_context;
