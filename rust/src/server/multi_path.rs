use serde_json::{Map, Value};

use crate::server::tool_trait::{get_str, get_str_array, ToolContext};

pub struct ResolvedPaths {
    pub roots: Vec<String>,
    pub is_multi: bool,
}

/// Resolve tool paths with multi-root support.
///
/// Priority:
/// 0. `repo` argument (multi-repo alias → specific root)
/// 1. `paths` array argument (explicit multi-root)
/// 2. `path` string argument (single root, pre-resolved by dispatch)
/// 3. Session `extra_roots` (default multi-root from config/MCP)
/// 4. Fallback to `"."` (project root)
pub fn resolve_tool_paths(args: &Map<String, Value>, ctx: &ToolContext) -> ResolvedPaths {
    if let Some(repo) = get_str(args, "repo") {
        if let Some(root) = crate::core::multi_repo::resolve_repo_root(&repo) {
            return ResolvedPaths {
                roots: vec![root],
                is_multi: false,
            };
        }
    }

    if let Some(paths) = get_str_array(args, "paths") {
        if !paths.is_empty() {
            let resolved = resolve_paths_sync(ctx, &paths);
            if !resolved.is_empty() {
                return ResolvedPaths {
                    is_multi: resolved.len() > 1,
                    roots: resolved,
                };
            }
        }
    }

    if let Some(path) = ctx.resolved_path("path") {
        return ResolvedPaths {
            roots: vec![path.to_string()],
            is_multi: false,
        };
    }

    if let Some(session_lock) = ctx.session.as_ref() {
        let (extra, jail_root) = tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let session = session_lock.read().await;
                let root = session
                    .project_root
                    .clone()
                    .unwrap_or_else(|| ".".to_string());
                (session.extra_roots.clone(), root)
            })
        });
        if !extra.is_empty() {
            let jail = std::path::Path::new(&jail_root);
            let mut roots = vec![ctx.project_root.clone()];
            for r in &extra {
                let p = std::path::Path::new(r);
                if !p.is_dir() {
                    continue;
                }
                match crate::core::pathjail::jail_path(p, jail) {
                    Ok(_) => roots.push(r.clone()),
                    Err(e) => tracing::warn!("extra_root rejected by PathJail: {e}"),
                }
            }
            if roots.len() > 1 {
                return ResolvedPaths {
                    is_multi: true,
                    roots,
                };
            }
        }
    }

    ResolvedPaths {
        roots: vec![".".to_string()],
        is_multi: false,
    }
}

fn resolve_paths_sync(ctx: &ToolContext, raw: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(raw.len());
    for p in raw {
        match ctx.resolve_path_sync(p) {
            Ok(resolved) => out.push(resolved),
            Err(e) => {
                tracing::warn!("multi-path resolve failed for {p}: {e}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_ctx() -> ToolContext {
        ToolContext {
            project_root: "/test/project".to_string(),
            minimal: false,
            resolved_paths: std::collections::HashMap::new(),
            crp_mode: crate::tools::CrpMode::Off,
            cache: None,
            session: None,
            tool_calls: None,
            agent_id: None,
            workflow: None,
            ledger: None,
            client_name: None,
            pipeline_stats: None,
            call_count: None,
            autonomy: None,
            pressure_snapshot: None,
            path_errors: std::collections::HashMap::new(),
            bm25_cache: None,
            progress_sender: None,
        }
    }

    #[test]
    fn fallback_to_dot_when_nothing_set() {
        let args = Map::new();
        let ctx = test_ctx();
        let result = resolve_tool_paths(&args, &ctx);
        assert_eq!(result.roots, vec!["."]);
        assert!(!result.is_multi);
    }

    #[test]
    fn uses_resolved_path_when_present() {
        let args = Map::new();
        let mut ctx = test_ctx();
        ctx.resolved_paths
            .insert("path".to_string(), "/resolved/dir".to_string());
        let result = resolve_tool_paths(&args, &ctx);
        assert_eq!(result.roots, vec!["/resolved/dir"]);
        assert!(!result.is_multi);
    }

    #[test]
    fn empty_paths_array_falls_back() {
        let mut args = Map::new();
        args.insert("paths".to_string(), json!([]));
        let mut ctx = test_ctx();
        ctx.resolved_paths
            .insert("path".to_string(), "/fallback".to_string());
        let result = resolve_tool_paths(&args, &ctx);
        assert_eq!(result.roots, vec!["/fallback"]);
        assert!(!result.is_multi);
    }
}
