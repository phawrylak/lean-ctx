//! Honest accounting of the fixed per-turn context lean-ctx injects (GitHub #361).
//!
//! Three components ride every request and — on a provider WITHOUT prompt caching
//! — are re-billed on every turn:
//!  - the exposed MCP **tool schemas** (description + input schema of each tool),
//!  - the MCP **server instructions** block, and
//!  - the **rules block** lean-ctx writes into the host's instruction file
//!    (`CLAUDE.md` / `AGENTS.md`).
//!
//! `lean-ctx gain` measures *compression on lean-ctx-touched reads* — its
//! denominator is lean-ctx traffic, not the provider bill. On a phase-isolated /
//! non-caching workload (separate process per phase, no provider cache) the
//! cached-re-read lever has no surface, so the headline can read net-positive
//! while the bill moved net-negative. Surfacing this overhead — and stating the
//! denominator — keeps the meter honest.
//!
//! Net bill impact ≈ `gross_saved_tokens − total_tokens() × turns`.

use std::sync::OnceLock;

use crate::core::tokens::count_tokens;

/// A measured breakdown, in tokens, of the per-turn context lean-ctx adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContextOverhead {
    /// Number of MCP tools exposed (the schema-bearing surface).
    pub tool_count: usize,
    /// Tokens for all exposed tool descriptions + input schemas.
    pub tool_schema_tokens: usize,
    /// Tokens for the MCP server instructions block (capped at the instruction budget).
    pub instruction_tokens: usize,
    /// Tokens for the rules block injected into the host instruction file.
    pub rules_block_tokens: usize,
}

impl ContextOverhead {
    /// Total per-turn overhead in tokens.
    #[must_use]
    pub fn total_tokens(&self) -> usize {
        self.tool_schema_tokens + self.instruction_tokens + self.rules_block_tokens
    }

    /// Process-cached overhead. The tool surface and rules block are static and
    /// the instruction block varies only with slow-moving session state, so a
    /// once-per-process measurement is the right tradeoff for callers that render
    /// repeatedly (the `gain` dashboard re-renders every second in `--live`) —
    /// it avoids per-tick disk I/O and re-tokenization.
    #[must_use]
    pub fn cached() -> Self {
        static CACHE: OnceLock<ContextOverhead> = OnceLock::new();
        *CACHE.get_or_init(Self::measure)
    }

    /// Measure the overhead for the currently-configured MCP surface. Reads the
    /// effective tool profile (minimal vs full) and CRP mode from config, so the
    /// number reflects what this install actually advertises.
    #[must_use]
    pub fn measure() -> Self {
        let cfg = crate::core::config::Config::load();
        let tools = if cfg.minimal_overhead_effective() {
            crate::tool_defs::lazy_tool_defs()
        } else {
            crate::tool_defs::granular_tool_defs()
        };
        let tool_count = tools.len();
        let tool_schema_tokens = tools.iter().map(tool_tokens).sum();

        let instructions =
            crate::instructions::build_instructions(crate::tools::CrpMode::effective());
        let instruction_tokens = count_tokens(&instructions);

        let rules_block_tokens = count_tokens(crate::rules_inject::canonical_rules_block());

        Self {
            tool_count,
            tool_schema_tokens,
            instruction_tokens,
            rules_block_tokens,
        }
    }
}

/// Description + input-schema tokens for one tool definition — exactly the two
/// fields a client re-sends in every request's tool list.
fn tool_tokens(t: &rmcp::model::Tool) -> usize {
    let desc = t
        .description
        .as_ref()
        .map_or(0, |d| count_tokens(d.as_ref()));
    let schema = count_tokens(&serde_json::to_string(&t.input_schema).unwrap_or_default());
    desc + schema
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_reports_nonzero_components() {
        let o = ContextOverhead::measure();
        assert!(o.tool_count > 0, "must expose at least one tool");
        assert!(o.tool_schema_tokens > 0, "tool schemas carry tokens");
        assert!(o.instruction_tokens > 0, "instructions carry tokens");
        assert!(o.rules_block_tokens > 0, "rules block carries tokens");
        assert_eq!(
            o.total_tokens(),
            o.tool_schema_tokens + o.instruction_tokens + o.rules_block_tokens
        );
    }

    #[test]
    fn total_is_sum_of_parts() {
        let o = ContextOverhead {
            tool_count: 10,
            tool_schema_tokens: 100,
            instruction_tokens: 200,
            rules_block_tokens: 50,
        };
        assert_eq!(o.total_tokens(), 350);
    }
}
