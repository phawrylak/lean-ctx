use rmcp::model::Tool;
use rmcp::ErrorData;
use serde_json::{json, Map, Value};

use crate::server::tool_trait::{McpTool, ToolContext, ToolOutput};
use crate::tool_defs::tool_def;

pub struct CtxProviderTool;

impl McpTool for CtxProviderTool {
    fn name(&self) -> &'static str {
        "ctx_provider"
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "ctx_provider",
            "External context providers (GitHub, GitLab, Jira, Postgres, custom). \
             action=discover|list: list registered providers. \
             action=status: provider health + cache metrics. \
             action=refresh: invalidate cache + re-fetch (provider= optional). \
             action=configure: show config (resource=paths|template for details). \
             action=query: provider+resource for data access. \
             Legacy GitLab actions still supported.",
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": [
                            "discover",
                            "list",
                            "status",
                            "refresh",
                            "configure",
                            "query",
                            "mcp_resources",
                            "gitlab_issues",
                            "gitlab_issue",
                            "gitlab_mrs",
                            "gitlab_pipelines"
                        ],
                        "description": "Provider action. 'discover'/'list' lists all. 'status' shows health+cache. 'refresh' invalidates+re-fetches. 'configure' shows config. 'query' uses registry routing."
                    },
                    "provider": {
                        "type": "string",
                        "description": "Provider ID (e.g. 'github', 'gitlab', 'jira', 'mcp:my-kb'). Required for query, optional for refresh (omit to refresh all)."
                    },
                    "resource": {
                        "type": "string",
                        "description": "Resource type for action=query (e.g. 'issues', 'pull_requests'). For configure: 'paths'|'template'|'show'."
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["compact", "chunks"],
                        "description": "Output mode for action=query. 'compact' (default) or 'chunks' for BM25/embedding ingest."
                    },
                    "state": {
                        "type": "string",
                        "description": "Filter by state (open, closed, merged, all)"
                    },
                    "labels": {
                        "type": "string",
                        "description": "Comma-separated labels filter (GitLab)"
                    },
                    "iid": {
                        "type": "integer",
                        "description": "Issue/MR IID for single-item lookup (GitLab)"
                    },
                    "status": {
                        "type": "string",
                        "description": "Pipeline/Actions status filter (running, success, failed)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max results (default 20, max 100)"
                    }
                },
                "required": ["action"]
            }),
        )
    }

    fn handle(
        &self,
        args: &Map<String, Value>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ErrorData> {
        let result = crate::tools::ctx_provider::handle(args, ctx);
        Ok(ToolOutput::simple(result))
    }
}
