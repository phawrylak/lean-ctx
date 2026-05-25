use crate::dashboard::routes::helpers::detect_project_root_for_dashboard;

pub(super) fn get_route(
    path: &str,
    query_str: &str,
) -> Option<(&'static str, &'static str, String)> {
    match path {
        "/api/heatmap" => Some(heatmap()),
        "/api/graph" => Some(graph()),
        "/api/graph/enrich" => Some(enrich()),
        "/api/graph/stats" => Some(stats()),
        "/api/graph-files" => Some(graph_files()),
        "/api/routes" => Some(routes(query_str)),
        _ => None,
    }
}

fn heatmap() -> (&'static str, &'static str, String) {
    let project_root = detect_project_root_for_dashboard();
    let index = crate::core::graph_index::load_or_build(&project_root);
    let entries = build_heatmap_json(&index);
    ("200 OK", "application/json", entries)
}

fn graph() -> (&'static str, &'static str, String) {
    let root = detect_project_root_for_dashboard();
    let index = crate::core::graph_index::load_or_build(&root);

    let mut edge_stats: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for edge in &index.edges {
        *edge_stats.entry(edge.kind.as_str()).or_default() += 1;
    }
    let connected: std::collections::HashSet<&str> = index
        .edges
        .iter()
        .flat_map(|e| [e.from.as_str(), e.to.as_str()])
        .collect();
    let isolated_count = index.files.len() - connected.len().min(index.files.len());

    let mut val = serde_json::to_value(&index).unwrap_or_default();
    if let Some(obj) = val.as_object_mut() {
        obj.insert(
            "project_root".to_string(),
            serde_json::Value::String(super::project_basename(&root)),
        );
        obj.insert(
            "project_root_full".to_string(),
            serde_json::Value::String(root.clone()),
        );
        obj.insert(
            "edge_stats".to_string(),
            serde_json::to_value(&edge_stats).unwrap_or_default(),
        );
        obj.insert(
            "isolated_node_count".to_string(),
            serde_json::Value::Number(isolated_count.into()),
        );
        let total = index.files.len();
        let orphan_rate = if total > 0 {
            (isolated_count as f64 / total as f64 * 100.0).round() / 100.0
        } else {
            0.0
        };
        obj.insert("orphan_rate".to_string(), serde_json::json!(orphan_rate));
    }
    let json = serde_json::to_string(&val)
        .unwrap_or_else(|_| "{\"error\":\"failed to serialize project index\"}".to_string());
    ("200 OK", "application/json", json)
}

fn enrich() -> (&'static str, &'static str, String) {
    let root = detect_project_root_for_dashboard();
    let project_path = std::path::Path::new(&root);
    let result = match crate::core::property_graph::CodeGraph::open(&root) {
        Ok(graph) => match crate::core::graph_enricher::enrich_graph(&graph, project_path, 500) {
            Ok(stats) => {
                let nc = graph.node_count().unwrap_or(0);
                let ec = graph.edge_count().unwrap_or(0);
                serde_json::json!({
                    "commits_indexed": stats.commits_indexed,
                    "tests_indexed": stats.tests_indexed,
                    "knowledge_indexed": stats.knowledge_indexed,
                    "edges_created": stats.edges_created,
                    "total_nodes": nc,
                    "total_edges": ec,
                })
            }
            Err(e) => {
                tracing::warn!("graph enrich error: {e}");
                serde_json::json!({"error": "enrichment_failed"})
            }
        },
        Err(e) => {
            tracing::warn!("graph open error: {e}");
            serde_json::json!({"error": "graph_unavailable"})
        }
    };
    let json = serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string());
    ("200 OK", "application/json", json)
}

fn stats() -> (&'static str, &'static str, String) {
    let root = detect_project_root_for_dashboard();
    let result = if let Some(open) = crate::core::graph_provider::open_best_effort(&root) {
        let nc = open.provider.node_count().unwrap_or(0);
        let ec = open.provider.edge_count().unwrap_or(0);
        match open.source {
            crate::core::graph_provider::GraphProviderSource::PropertyGraph => {
                serde_json::json!({
                    "source": "property_graph",
                    "node_count": nc,
                    "edge_count": ec,
                })
            }
            crate::core::graph_provider::GraphProviderSource::GraphIndex => {
                serde_json::json!({
                    "source": "graph_index",
                    "node_count": nc,
                    "edge_count": ec,
                })
            }
        }
    } else {
        serde_json::json!({
            "source": "none",
            "node_count": 0,
            "edge_count": 0,
        })
    };
    let json = serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string());
    ("200 OK", "application/json", json)
}

fn graph_files() -> (&'static str, &'static str, String) {
    let root = detect_project_root_for_dashboard();
    let index = crate::core::graph_index::load_or_build(&root);
    let mut files: Vec<serde_json::Value> = index
        .files
        .values()
        .map(|f| {
            serde_json::json!({
                "path": f.path,
                "language": f.language,
                "token_count": f.token_count,
                "line_count": f.line_count,
            })
        })
        .collect();
    files.sort_by(|a, b| {
        b["token_count"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["token_count"].as_u64().unwrap_or(0))
    });
    files.truncate(500);
    let json = serde_json::json!({ "files": files, "project_root_full": root });
    let out = serde_json::to_string(&json).unwrap_or_else(|_| "{\"files\":[]}".to_string());
    ("200 OK", "application/json", out)
}

fn routes(_query_str: &str) -> (&'static str, &'static str, String) {
    let root = detect_project_root_for_dashboard();
    let index = crate::core::graph_index::load_or_build(&root);
    let routes = crate::core::route_extractor::extract_routes_from_project(&root, &index.files);
    let route_candidate_count = index
        .files
        .keys()
        .filter(|p| {
            std::path::Path::new(p.as_str())
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| {
                    matches!(e, "js" | "ts" | "py" | "rs" | "java" | "rb" | "go" | "kt")
                })
        })
        .count();
    let payload = serde_json::json!({
        "routes": routes,
        "indexed_file_count": index.files.len(),
        "route_candidate_count": route_candidate_count,
    });
    let json = serde_json::to_string(&payload).unwrap_or_else(|_| "{\"routes\":[]}".to_string());
    ("200 OK", "application/json", json)
}

fn build_heatmap_json(index: &crate::core::graph_index::ProjectIndex) -> String {
    let mut connection_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for edge in &index.edges {
        *connection_counts.entry(edge.from.clone()).or_default() += 1;
        *connection_counts.entry(edge.to.clone()).or_default() += 1;
    }

    let max_tokens = index
        .files
        .values()
        .map(|f| f.token_count)
        .max()
        .unwrap_or(1) as f64;
    let max_connections = connection_counts.values().max().copied().unwrap_or(1) as f64;

    let mut entries: Vec<serde_json::Value> = index
        .files
        .values()
        .map(|f| {
            let connections = connection_counts.get(&f.path).copied().unwrap_or(0);
            let token_norm = f.token_count as f64 / max_tokens;
            let conn_norm = connections as f64 / max_connections;
            let heat = token_norm * 0.4 + conn_norm * 0.6;
            serde_json::json!({
                "path": f.path,
                "tokens": f.token_count,
                "connections": connections,
                "language": f.language,
                "heat": (heat * 100.0).round() / 100.0,
            })
        })
        .collect();

    entries.sort_by(|a, b| {
        b["heat"]
            .as_f64()
            .unwrap_or(0.0)
            .partial_cmp(&a["heat"].as_f64().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string())
}
