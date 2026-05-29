use crate::core::consolidation;
use crate::core::providers::config::GitLabConfig;
use crate::core::providers::provider_trait::ProviderParams;
use crate::core::providers::registry::global_registry;
use crate::core::providers::{gitlab, ProviderResult};
use crate::server::tool_trait::ToolContext;

pub fn handle(args: &serde_json::Map<String, serde_json::Value>, ctx: &ToolContext) -> String {
    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");

    match action {
        // -- Discovery --
        "discover" => handle_discover(ctx),

        // -- Registry-based routing (provider_id + resource) --
        "query" => handle_registry_query(args, ctx),

        // -- MCP Bridge convenience actions --
        "mcp_resources" => handle_mcp_resources(args, ctx),

        // -- Legacy GitLab actions (backward-compatible) --
        "gitlab_issues" => handle_gitlab_issues(args),
        "gitlab_issue" => handle_gitlab_issue(args),
        "gitlab_mrs" => handle_gitlab_mrs(args),
        "gitlab_pipelines" => handle_gitlab_pipelines(args),

        _ => {
            let available =
                "discover, query, mcp_resources, gitlab_issues, gitlab_issue, gitlab_mrs, gitlab_pipelines";
            format!("Unknown action: {action}. Available: {available}")
        }
    }
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

fn handle_discover(ctx: &ToolContext) -> String {
    crate::core::providers::init::init_with_project_root(Some(std::path::Path::new(
        &ctx.project_root,
    )));
    let infos = global_registry().discover();
    if infos.is_empty() {
        return "No providers registered. Set GITHUB_TOKEN or GITLAB_TOKEN.".to_string();
    }

    let mut out = format!("Registered providers ({}):\n", infos.len());
    for info in &infos {
        let status = if info.available {
            "ready"
        } else {
            "unavailable"
        };
        out.push_str(&format!(
            "  {} ({}) [{}] actions: {}\n",
            info.id,
            info.display_name,
            status,
            info.actions.join(", "),
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// MCP Bridge convenience: list resources from a specific MCP bridge
// ---------------------------------------------------------------------------

fn handle_mcp_resources(
    args: &serde_json::Map<String, serde_json::Value>,
    ctx: &ToolContext,
) -> String {
    crate::core::providers::init::init_with_project_root(Some(std::path::Path::new(
        &ctx.project_root,
    )));

    let Some(provider_id) = args.get("provider").and_then(|v| v.as_str()) else {
        let registry = global_registry();
        let mcp_providers: Vec<_> = registry
            .discover()
            .into_iter()
            .filter(|p| p.id.starts_with("mcp:"))
            .collect();

        if mcp_providers.is_empty() {
            return "No MCP bridges configured. Add [providers.mcp_bridges] to config.toml."
                .to_string();
        }

        let mut out = format!("Available MCP bridges ({}):\n", mcp_providers.len());
        for p in &mcp_providers {
            let status = if p.available { "ready" } else { "unavailable" };
            out.push_str(&format!("  {} ({}) [{}]\n", p.id, p.display_name, status));
        }
        out.push_str("\nUse provider=\"mcp:<name>\" to list resources from a specific bridge.");
        return out;
    };

    let provider_id = if provider_id.starts_with("mcp:") {
        provider_id.to_string()
    } else {
        format!("mcp:{provider_id}")
    };

    let params = ProviderParams {
        limit: args
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .map(|n| n as usize),
        ..Default::default()
    };

    match global_registry().execute(&provider_id, "resources", &params) {
        Ok(result) => format_result(&result),
        Err(e) => format!("Error: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Registry-based query (new unified interface)
// ---------------------------------------------------------------------------

fn handle_registry_query(
    args: &serde_json::Map<String, serde_json::Value>,
    ctx: &ToolContext,
) -> String {
    crate::core::providers::init::init_with_project_root(Some(std::path::Path::new(
        &ctx.project_root,
    )));

    let Some(provider_id) = args.get("provider").and_then(|v| v.as_str()) else {
        return "Error: 'provider' is required for action=query".to_string();
    };
    let Some(resource) = args.get("resource").and_then(|v| v.as_str()) else {
        return "Error: 'resource' is required for action=query".to_string();
    };

    let params = ProviderParams {
        project: args
            .get("project")
            .and_then(|v| v.as_str())
            .map(String::from),
        state: args.get("state").and_then(|v| v.as_str()).map(String::from),
        limit: args
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .map(|n| n as usize),
        query: args.get("query").and_then(|v| v.as_str()).map(String::from),
        id: args.get("id").and_then(|v| v.as_str()).map(String::from),
    };

    let mode = args
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("compact");

    match mode {
        "chunks" => handle_registry_chunks(provider_id, resource, &params, ctx),
        _ => handle_registry_compact(provider_id, resource, &params, ctx),
    }
}

fn handle_registry_compact(
    provider_id: &str,
    resource: &str,
    params: &ProviderParams,
    ctx: &ToolContext,
) -> String {
    match global_registry().execute_as_chunks(provider_id, resource, params) {
        Ok(chunks) => {
            consolidate_to_session(&chunks, ctx);
            let result = global_registry().execute(provider_id, resource, params);
            match result {
                Ok(r) => format_result(&r),
                Err(_) => format_chunks_compact(&chunks, provider_id, resource),
            }
        }
        Err(e) => format!("Error: {e}"),
    }
}

fn handle_registry_chunks(
    provider_id: &str,
    resource: &str,
    params: &ProviderParams,
    ctx: &ToolContext,
) -> String {
    match global_registry().execute_as_chunks(provider_id, resource, params) {
        Ok(chunks) => {
            consolidate_to_session(&chunks, ctx);
            let mut out = format!(
                "{} content chunks from {provider_id}/{resource}:\n",
                chunks.len()
            );
            for c in &chunks {
                let refs = if c.references.is_empty() {
                    String::new()
                } else {
                    format!(" refs:[{}]", c.references.join(","))
                };
                out.push_str(&format!(
                    "  {} {:?} ({}tok){}\n",
                    c.file_path, c.kind, c.token_count, refs
                ));
            }
            out
        }
        Err(e) => format!("Error: {e}"),
    }
}

/// Consolidate provider chunks into ALL long-term stores:
///   1. Session cache (fast re-reads at ~13 tokens)
///   2. BM25 index (searchable via ctx_semantic_search)
///   3. Graph index (cross-source edges for ctx_read hints)
///   4. Knowledge (extracted facts for ctx_knowledge)
///
/// Cache writes happen synchronously (fast). BM25/Graph/Knowledge
/// writes happen in a background thread to avoid blocking the tool
/// response — the "hippocampal sleep replay" pattern.
fn consolidate_to_session(chunks: &[crate::core::content_chunk::ContentChunk], ctx: &ToolContext) {
    if chunks.is_empty() {
        return;
    }

    let artifacts = consolidation::consolidate(chunks);
    if artifacts.is_empty() {
        return;
    }

    // Phase 1: Session cache (synchronous, fast)
    if let Some(cache_lock) = ctx.cache.as_ref() {
        if let Ok(mut cache) = cache_lock.try_write() {
            for entry in &artifacts.cache_entries {
                cache.store(&entry.uri, &entry.content);
            }
        }
    }

    let external_count = artifacts
        .bm25_chunks
        .iter()
        .filter(|c| c.is_external())
        .count();
    let edge_count = artifacts.edges.len();
    let fact_count = artifacts.facts.len();
    let cache_count = artifacts.cache_entries.len();

    tracing::debug!(
        "[ctx_provider] consolidated {} chunks → {} edges, {} facts, {} cached",
        external_count,
        edge_count,
        fact_count,
        cache_count,
    );

    // Phase 2: Deep indexing (background thread — BM25, Graph, Knowledge)
    let cfg = crate::core::config::Config::load();
    if !cfg.providers.auto_index {
        return;
    }

    let project_root = ctx.project_root.clone();
    std::thread::spawn(move || {
        apply_artifacts_to_stores(&artifacts, &project_root);
    });
}

/// Apply consolidation artifacts to BM25, Graph, and Knowledge stores.
/// Called from a background thread after provider queries.
pub fn apply_artifacts_to_stores(
    artifacts: &consolidation::ConsolidationArtifacts,
    project_root: &str,
) {
    let root_path = std::path::Path::new(project_root);

    // BM25: load existing index, ingest provider chunks, save
    if !artifacts.bm25_chunks.is_empty() {
        let mut index = crate::core::bm25_index::BM25Index::load_or_build(root_path);
        let ingested = index.ingest_content_chunks(artifacts.bm25_chunks.clone());
        if ingested > 0 {
            if let Err(e) = index.save(root_path) {
                tracing::warn!("[ctx_provider] BM25 save failed: {e}");
            } else {
                tracing::info!("[ctx_provider] indexed {ingested} provider chunks into BM25");
            }
        }
    }

    // Graph: load existing index, merge cross-source edges, save
    if !artifacts.edges.is_empty() {
        let mut graph = crate::core::graph_index::load_or_build(project_root);
        let added =
            crate::core::cross_source_edges::merge_edges(&mut graph.edges, artifacts.edges.clone());
        if added > 0 {
            if let Err(e) = graph.save() {
                tracing::warn!("[ctx_provider] graph save failed: {e}");
            } else {
                tracing::info!("[ctx_provider] added {added} cross-source edges to graph");
            }
        }
    }

    // Knowledge: load or create, remember extracted facts, save
    if !artifacts.facts.is_empty() {
        let policy = crate::core::memory_policy::MemoryPolicy::default();
        let mut knowledge = crate::core::knowledge::ProjectKnowledge::load(project_root)
            .unwrap_or_else(|| crate::core::knowledge::ProjectKnowledge::new(project_root));

        let session_id = format!("provider-ingest-{}", chrono::Utc::now().timestamp());
        for fact in &artifacts.facts {
            knowledge.remember(
                &fact.category,
                &fact.key,
                &fact.value,
                &session_id,
                fact.confidence,
                &policy,
            );
        }

        if let Err(e) = knowledge.save() {
            tracing::warn!("[ctx_provider] knowledge save failed: {e}");
        } else {
            tracing::info!(
                "[ctx_provider] remembered {} facts from provider data",
                artifacts.facts.len()
            );
        }
    }
}

fn format_chunks_compact(
    chunks: &[crate::core::content_chunk::ContentChunk],
    provider_id: &str,
    resource: &str,
) -> String {
    let mut out = format!("{} results from {provider_id}/{resource}:\n", chunks.len());
    for c in chunks {
        out.push_str(&format!(
            "  #{} {}\n",
            c.file_path.rsplit('/').next().unwrap_or("?"),
            c.symbol_name
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Legacy GitLab handlers (unchanged)
// ---------------------------------------------------------------------------

fn handle_gitlab_issues(args: &serde_json::Map<String, serde_json::Value>) -> String {
    let config = match GitLabConfig::from_env() {
        Ok(c) => c,
        Err(e) => return format!("Error: {e}"),
    };
    let state = args.get("state").and_then(|v| v.as_str());
    let labels = args.get("labels").and_then(|v| v.as_str());
    let limit = args
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .map(|n| n as usize);

    match gitlab::list_issues(&config, state, labels, limit) {
        Ok(result) => format_result(&result),
        Err(e) => format!("Error: {e}"),
    }
}

fn handle_gitlab_issue(args: &serde_json::Map<String, serde_json::Value>) -> String {
    let config = match GitLabConfig::from_env() {
        Ok(c) => c,
        Err(e) => return format!("Error: {e}"),
    };
    let iid = args
        .get("iid")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if iid == 0 {
        return "Error: iid is required for gitlab_issue".to_string();
    }

    match gitlab::show_issue(&config, iid) {
        Ok(result) => format_result(&result),
        Err(e) => format!("Error: {e}"),
    }
}

fn handle_gitlab_mrs(args: &serde_json::Map<String, serde_json::Value>) -> String {
    let config = match GitLabConfig::from_env() {
        Ok(c) => c,
        Err(e) => return format!("Error: {e}"),
    };
    let state = args.get("state").and_then(|v| v.as_str());
    let limit = args
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .map(|n| n as usize);

    match gitlab::list_mrs(&config, state, limit) {
        Ok(result) => format_result(&result),
        Err(e) => format!("Error: {e}"),
    }
}

fn handle_gitlab_pipelines(args: &serde_json::Map<String, serde_json::Value>) -> String {
    let config = match GitLabConfig::from_env() {
        Ok(c) => c,
        Err(e) => return format!("Error: {e}"),
    };
    let status = args.get("status").and_then(|v| v.as_str());
    let limit = args
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .map(|n| n as usize);

    match gitlab::list_pipelines(&config, status, limit) {
        Ok(result) => format_result(&result),
        Err(e) => format!("Error: {e}"),
    }
}

fn format_result(result: &ProviderResult) -> String {
    crate::core::redaction::redact_text_if_enabled(&result.format_compact())
}
