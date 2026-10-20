//! Thinking-mode configuration: the single authority for mode behaviour —
//! RAGFlow v0.27.2 `rag/advanced_rag/harness/config.py`.
//!
//! Every mode-dependent decision in the harness reads from this module instead
//! of re-deriving it from a `thinking_mode` string: graph usage (`agentic`),
//! the sufficiency-check loop (`enable_sca` / `sca_max_rounds`), planner
//! fan-out (`use_fanout`), the session turn budget (`action_max_turns`) and the
//! visible tool surface (`tools`).
//!
//! Unknown labels fall back to [`naive`] rather than raising: the label comes
//! from user input, and a naive (non-agentic) answer degrades gracefully.

use std::collections::{BTreeMap, BTreeSet};

/// Tools the action session can bind, in declaration order.
pub const ALL_TOOLS: [&str; 7] = [
    "retrieve",
    "search_chunks",
    "list_chunks",
    "navigate_tree",
    "navigate_structure",
    "calculate",
    "web_search",
];
/// Relational exploration is the extra depth reserved for ultra.
pub const GRAPH_EXPLORE: &str = "graph_explore";

/// One thinking mode's behaviour.
#[derive(Debug, Clone, PartialEq)]
pub struct ModeSpec {
    pub label: String,
    pub agentic: bool,
    pub enable_sca: bool,
    pub sca_max_rounds: usize,
    pub use_fanout: bool,
    pub action_max_turns: usize,
    pub tools: BTreeSet<String>,
}

fn tools_from(names: impl IntoIterator<Item = &'static str>) -> BTreeSet<String> {
    names.into_iter().map(str::to_string).collect()
}

fn spec(label: &str) -> ModeSpec {
    ModeSpec {
        label: label.to_string(),
        agentic: true,
        enable_sca: true,
        sca_max_rounds: 3,
        use_fanout: false,
        action_max_turns: 4,
        tools: tools_from(ALL_TOOLS),
    }
}

/// `THINKING_MODES`.
pub fn thinking_modes() -> BTreeMap<String, ModeSpec> {
    let mut modes = BTreeMap::new();

    // low: one hybrid-search pass through `direct_search`. No action session,
    // so no tool loop — the model never sees tools in this mode.
    modes.insert(
        "low".to_string(),
        ModeSpec {
            label: "low".to_string(),
            agentic: false,
            enable_sca: false,
            sca_max_rounds: 0,
            use_fanout: false,
            action_max_turns: 4,
            tools: BTreeSet::new(),
        },
    );
    // medium: agentic, SCA review on, no planner decomposition.
    modes.insert("medium".to_string(), spec("medium"));
    // high: adds planner + prefetch fan-out over the same tool surface.
    modes.insert(
        "high".to_string(),
        ModeSpec {
            use_fanout: true,
            ..spec("high")
        },
    );
    // ultra: deeper sessions, more SCA rounds, and the relational tool.
    modes.insert(
        "ultra".to_string(),
        ModeSpec {
            sca_max_rounds: 5,
            use_fanout: true,
            action_max_turns: 6,
            tools: tools_from(ALL_TOOLS.into_iter().chain([GRAPH_EXPLORE])),
            ..spec("ultra")
        },
    );
    modes
}

/// `NAIVE`: fallback for an unrecognised mode label — not agentic.
pub fn naive() -> ModeSpec {
    ModeSpec {
        label: "naive".to_string(),
        agentic: false,
        enable_sca: false,
        sca_max_rounds: 0,
        use_fanout: false,
        action_max_turns: 4,
        tools: BTreeSet::new(),
    }
}

/// `get_mode`: the spec for `label`, falling back to [`naive`].
pub fn get_mode(label: &str) -> ModeSpec {
    thinking_modes()
        .get(label.trim().to_lowercase().as_str())
        .cloned()
        .unwrap_or_else(naive)
}

/// `resolve_mode`: read a `RAGTools`-like object's `thinking_mode`.
pub fn resolve_mode(thinking_mode: &str) -> ModeSpec {
    get_mode(thinking_mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_mirror_upstream_tables() {
        let low = get_mode("low");
        assert!(!low.agentic);
        assert!(!low.enable_sca);
        assert_eq!(low.sca_max_rounds, 0);
        assert!(low.tools.is_empty());

        let medium = get_mode("Medium");
        assert!(medium.agentic);
        assert_eq!(medium.sca_max_rounds, 3);
        assert!(!medium.use_fanout);
        assert_eq!(medium.tools.len(), ALL_TOOLS.len());

        let high = get_mode("high");
        assert!(high.use_fanout);
        assert_eq!(high.action_max_turns, 4);

        let ultra = get_mode("ultra");
        assert_eq!(ultra.sca_max_rounds, 5);
        assert_eq!(ultra.action_max_turns, 6);
        assert!(ultra.use_fanout);
        assert!(ultra.tools.contains(GRAPH_EXPLORE));
        assert_eq!(ultra.tools.len(), ALL_TOOLS.len() + 1);

        // Unknown labels degrade to the non-agentic fallback.
        let unknown = resolve_mode("  fantasy  ");
        assert_eq!(unknown.label, "naive");
        assert!(!unknown.agentic);
        assert!(unknown.tools.is_empty());
    }
}
