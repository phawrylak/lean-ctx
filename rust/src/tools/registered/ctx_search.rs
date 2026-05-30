use rmcp::model::Tool;
use rmcp::ErrorData;
use serde_json::{json, Map, Value};

use crate::server::tool_trait::{get_bool, get_int, get_str, McpTool, ToolContext, ToolOutput};
use crate::tool_defs::tool_def;

pub struct CtxSearchTool;

impl McpTool for CtxSearchTool {
    fn name(&self) -> &'static str {
        "ctx_search"
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "ctx_search",
            "Regex code search (.gitignore aware, compact results). Supports multi-root via `paths` array. Secret-like files skipped unless role allows.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regex pattern" },
                    "path": { "type": "string", "description": "Directory to search" },
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Multiple directories to search (alternative to path)"
                    },
                    "ext": { "type": "string", "description": "File extension filter" },
                    "max_results": { "type": "integer", "description": "Max results (default: 20)" },
                    "ignore_gitignore": { "type": "boolean", "description": "Set true to scan ALL files including .gitignore'd paths (default: false). Requires role policy (e.g. admin)." }
                },
                "required": ["pattern"]
            }),
        )
    }

    fn handle(
        &self,
        args: &Map<String, Value>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ErrorData> {
        let pattern = get_str(args, "pattern")
            .ok_or_else(|| ErrorData::invalid_params("pattern is required", None))?;
        let resolved = crate::server::multi_path::resolve_tool_paths(args, ctx);
        let ext = get_str(args, "ext");
        let max = (get_int(args, "max_results").unwrap_or(20) as usize).min(500);
        let no_gitignore = get_bool(args, "ignore_gitignore").unwrap_or(false);

        if no_gitignore {
            if let Err(e) = crate::core::io_boundary::ensure_ignore_gitignore_allowed("ctx_search")
            {
                return Ok(ToolOutput::simple(e));
            }
        }

        let crp = ctx.crp_mode;
        let respect = !no_gitignore;
        let allow_secret_paths = crate::core::roles::active_role().io.allow_secret_paths;

        if !resolved.is_multi {
            return search_single(
                &pattern,
                &resolved.roots[0],
                ext.as_deref(),
                max,
                crp,
                respect,
                allow_secret_paths,
            );
        }

        let _mode_guard = crate::core::savings_footer::ModeGuard::new("search");
        let per_root_max = (max / resolved.roots.len()).max(5);
        let mut combined = String::new();
        let mut total_original: usize = 0;
        let mut total_sent: usize = 0;

        for root in &resolved.roots {
            let pat = pattern.clone();
            let r = root.clone();
            let e = ext.clone();

            let search_result = tokio::task::block_in_place(|| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::tools::ctx_search::handle(
                        &pat,
                        &r,
                        e.as_deref(),
                        per_root_max,
                        crp,
                        respect,
                        allow_secret_paths,
                    )
                }))
                .ok()
            });

            let Some((result, original)) = search_result else {
                combined.push_str(&format!("── {root} ──\nERROR: search panicked\n\n"));
                continue;
            };

            if result.starts_with("ERROR:") || result.trim().is_empty() {
                if !result.trim().is_empty() {
                    combined.push_str(&format!("── {root} ──\n{result}\n\n"));
                }
                continue;
            }

            combined.push_str(&format!("── {root} ──\n{result}\n\n"));
            total_original += original;
            total_sent += crate::core::tokens::count_tokens(&result);
        }

        if combined.is_empty() {
            combined = "No matches found across any root.".to_string();
        }

        let final_out =
            crate::core::protocol::append_savings(&combined, total_original, total_sent);
        let saved = total_original.saturating_sub(total_sent);

        Ok(ToolOutput {
            text: final_out,
            original_tokens: total_original,
            saved_tokens: saved,
            mode: None,
            path: None,
            changed: false,
        })
    }
}

fn search_single(
    pattern: &str,
    path: &str,
    ext: Option<&str>,
    max: usize,
    crp: crate::tools::CrpMode,
    respect_gitignore: bool,
    allow_secret_paths: bool,
) -> Result<ToolOutput, ErrorData> {
    let _mode_guard = crate::core::savings_footer::ModeGuard::new("search");
    let pattern_clone = pattern.to_string();
    let path_clone = path.to_string();

    let search_result = tokio::task::block_in_place(|| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::tools::ctx_search::handle(
                &pattern_clone,
                &path_clone,
                ext,
                max,
                crp,
                respect_gitignore,
                allow_secret_paths,
            )
        }));
        match result {
            Ok(r) => Ok(r),
            Err(_) => Err("search task panicked"),
        }
    });

    let (result, original) = match search_result {
        Ok(r) => r,
        Err(e) => {
            return Err(ErrorData::internal_error(
                format!("search task failed: {e}"),
                None,
            ));
        }
    };

    if result.starts_with("ERROR:") {
        return Err(ErrorData::invalid_params(result, None));
    }

    let sent = crate::core::tokens::count_tokens(&result);
    let saved = original.saturating_sub(sent);
    let final_out = crate::core::protocol::append_savings(&result, original, sent);

    Ok(ToolOutput {
        text: final_out,
        original_tokens: original,
        saved_tokens: saved,
        mode: None,
        path: Some(path.to_string()),
        changed: false,
    })
}
