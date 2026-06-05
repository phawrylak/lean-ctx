use std::sync::Arc;

use rmcp::model::Tool;
use serde_json::{Map, Value};

mod granular;
pub use granular::{granular_tool_defs, unified_tool_defs};

pub fn tool_def(name: &'static str, description: &'static str, schema_value: Value) -> Tool {
    let schema: Map<String, Value> = match schema_value {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    Tool::new(name, description, Arc::new(schema))
}

pub const CORE_TOOL_NAMES: &[&str] = &[
    "ctx_read",
    "ctx_search",
    "ctx_shell",
    "shell",
    "ctx_tree",
    "ctx_edit",
    "ctx_session",
    "ctx_knowledge",
    "ctx_overview",
    "ctx_graph",
    "ctx_call",
    "ctx_provider",
    "ctx_expand",
];

pub fn core_tool_names() -> &'static [&'static str] {
    CORE_TOOL_NAMES
}

pub fn lazy_tool_defs() -> Vec<Tool> {
    let all = granular_tool_defs();
    all.into_iter()
        .filter(|t| CORE_TOOL_NAMES.contains(&t.name.as_ref()))
        .collect()
}

pub fn discover_tools(query: &str) -> String {
    // Derived from the registry (single source of truth) so discovery results
    // never drift from the advertised tool schemas (#141).
    let all = crate::server::registry::build_registry().tool_defs();
    let query_lower = query.to_lowercase();
    let matches: Vec<(String, String)> = all
        .iter()
        .filter_map(|t| {
            let name = t.name.as_ref();
            let desc = t.description.as_deref().unwrap_or("");
            if name.to_lowercase().contains(&query_lower)
                || desc.to_lowercase().contains(&query_lower)
            {
                Some((name.to_string(), desc.to_string()))
            } else {
                None
            }
        })
        .collect();

    if matches.is_empty() {
        return format!("No tools found matching '{query}'. Try broader terms like: graph, cost, session, search, compress, agent, workflow, gain.");
    }

    let mut out = format!("{} tools matching '{query}':\n", matches.len());
    for (name, desc) in &matches {
        // First line only — registry descriptions can be multi-line.
        let first = desc.lines().next().unwrap_or(desc);
        let short = if first.len() > 80 {
            &first[..first.floor_char_boundary(80)]
        } else {
            first
        };
        out.push_str(&format!("  {name} — {short}\n"));
    }
    out.push_str(
        "\nIf your MCP client registers tools only once at startup (static tools/list), \
use ctx_call (available in lazy mode) to invoke discovered tools:\n\
  ctx_call {\"name\":\"ctx_graph\",\"arguments\":{\"action\":\"status\"}}\n",
    );
    out
}

pub fn is_full_mode() -> bool {
    std::env::var("LEAN_CTX_FULL_TOOLS").is_ok_and(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
        || std::env::var("LEAN_CTX_LAZY_TOOLS")
            .is_ok_and(|v| v == "0" || v.eq_ignore_ascii_case("false"))
}
