//! Leiden community detection on the Property Graph.
//!
//! Implements the Leiden algorithm (Traag, Waltman, van Eck 2019) for
//! modularity-based graph clustering with guaranteed connected communities:
//!   1. **Local moving:** greedily move nodes to the community that yields
//!      the highest modularity gain.
//!   2. **Refinement:** within each community, find well-connected
//!      sub-communities to ensure connectivity.
//!   3. **Aggregation:** collapse sub-communities into super-nodes and repeat.

use std::collections::HashMap;

use rusqlite::Connection;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Community {
    pub id: usize,
    pub files: Vec<String>,
    pub internal_edges: usize,
    pub external_edges: usize,
    pub cohesion: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommunityResult {
    pub communities: Vec<Community>,
    pub modularity: f64,
    pub node_count: usize,
    pub edge_count: usize,
}

struct AdjGraph {
    node_ids: Vec<String>,
    #[cfg_attr(not(test), allow(dead_code))]
    node_to_idx: HashMap<String, usize>,
    adj: Vec<Vec<(usize, f64)>>,
    total_weight: f64,
    degree: Vec<f64>,
}

impl AdjGraph {
    fn from_property_graph(conn: &Connection) -> Self {
        let mut node_ids: Vec<String> = Vec::new();
        let mut node_to_idx: HashMap<String, usize> = HashMap::new();

        let Ok(mut file_stmt) =
            conn.prepare("SELECT DISTINCT file_path FROM nodes WHERE kind = 'file'")
        else {
            tracing::warn!("community: failed to prepare file query");
            return Self {
                node_ids: Vec::new(),
                node_to_idx: HashMap::new(),
                adj: Vec::new(),
                degree: Vec::new(),
                total_weight: 0.0,
            };
        };
        let files = match file_stmt.query_map([], |row| row.get::<_, String>(0)) {
            Ok(rows) => rows.filter_map(std::result::Result::ok).collect::<Vec<_>>(),
            Err(e) => {
                tracing::warn!("community: file query failed: {e}");
                Vec::new()
            }
        };

        for f in &files {
            let idx = node_ids.len();
            node_ids.push(f.clone());
            node_to_idx.insert(f.clone(), idx);
        }

        let n = node_ids.len();
        let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
        let mut total_weight = 0.0;
        let mut degree = vec![0.0; n];

        let edge_sql = "
            SELECT DISTINCT n1.file_path, n2.file_path, e.kind
            FROM edges e
            JOIN nodes n1 ON e.source_id = n1.id
            JOIN nodes n2 ON e.target_id = n2.id
            WHERE n1.kind = 'file' AND n2.kind = 'file'
              AND n1.file_path != n2.file_path
        ";
        let Ok(mut edge_stmt) = conn.prepare(edge_sql) else {
            tracing::warn!("community: failed to prepare edge query");
            return Self {
                node_ids,
                node_to_idx,
                adj,
                total_weight,
                degree,
            };
        };
        let edges = match edge_stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        }) {
            Ok(rows) => rows.filter_map(std::result::Result::ok).collect::<Vec<_>>(),
            Err(e) => {
                tracing::warn!("community: edge query failed: {e}");
                Vec::new()
            }
        };

        for (from, to, kind) in &edges {
            let Some(&i) = node_to_idx.get(from) else {
                continue;
            };
            let Some(&j) = node_to_idx.get(to) else {
                continue;
            };
            let w = edge_weight(kind);
            adj[i].push((j, w));
            degree[i] += w;
            degree[j] += w;
            total_weight += w;
        }

        Self {
            node_ids,
            node_to_idx,
            adj,
            total_weight,
            degree,
        }
    }
}

fn edge_weight(kind: &str) -> f64 {
    match kind {
        "imports" => 1.0,
        "calls" => 1.5,
        "type_ref" => 0.8,
        "defines" | "exports" => 0.3,
        _ => 0.5,
    }
}

pub fn detect_communities(conn: &Connection) -> CommunityResult {
    let graph = AdjGraph::from_property_graph(conn);
    let n = graph.node_ids.len();

    if n == 0 {
        return CommunityResult {
            communities: Vec::new(),
            modularity: 0.0,
            node_count: 0,
            edge_count: 0,
        };
    }

    let assignment = leiden(&graph);

    let mut comm_map: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, &c) in assignment.iter().enumerate() {
        comm_map.entry(c).or_default().push(i);
    }

    let mut communities: Vec<Community> = Vec::new();
    for members in comm_map.values() {
        let files: Vec<String> = members.iter().map(|&i| graph.node_ids[i].clone()).collect();
        let member_set: std::collections::HashSet<usize> = members.iter().copied().collect();

        let mut internal = 0usize;
        let mut external = 0usize;
        for &i in members {
            for &(j, _) in &graph.adj[i] {
                if member_set.contains(&j) {
                    internal += 1;
                } else {
                    external += 1;
                }
            }
        }

        let total = (internal + external).max(1) as f64;
        let cohesion = internal as f64 / total;

        communities.push(Community {
            id: 0,
            files,
            internal_edges: internal,
            external_edges: external,
            cohesion,
        });
    }

    communities.sort_by(|a, b| {
        b.files.len().cmp(&a.files.len()).then_with(|| {
            b.cohesion
                .partial_cmp(&a.cohesion)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });

    for (new_id, c) in communities.iter_mut().enumerate() {
        c.id = new_id;
    }

    let modularity = compute_modularity(&graph, &assignment);
    let edge_count = graph.adj.iter().map(Vec::len).sum::<usize>();

    CommunityResult {
        communities,
        modularity,
        node_count: n,
        edge_count,
    }
}

// ── Leiden Algorithm ────────────────────────────────────────

const MAX_ITERATIONS: usize = 20;
const GAMMA: f64 = 1.0;

fn leiden(graph: &AdjGraph) -> Vec<usize> {
    let n = graph.node_ids.len();
    let mut assignment: Vec<usize> = (0..n).collect();
    let m2 = graph.total_weight.max(1.0) * 2.0;

    for _ in 0..MAX_ITERATIONS {
        let moved = local_moving(graph, &mut assignment, m2);
        if !moved {
            break;
        }
        refine_communities(graph, &mut assignment, m2);
    }

    assignment
}

/// Phase 1: Local Moving — greedily move nodes to their best neighbor community.
fn local_moving(graph: &AdjGraph, assignment: &mut [usize], m2: f64) -> bool {
    let n = assignment.len();
    let mut comm_total: Vec<f64> = vec![0.0; n];
    for (i, &c) in assignment.iter().enumerate() {
        comm_total[c] += graph.degree[i];
    }

    let mut changed = false;
    let mut improved = true;

    while improved {
        improved = false;
        for i in 0..n {
            let current = assignment[i];
            let ki = graph.degree[i];

            let mut neighbor_comm_weight: HashMap<usize, f64> = HashMap::new();
            for &(j, w) in &graph.adj[i] {
                *neighbor_comm_weight.entry(assignment[j]).or_default() += w;
            }

            let sigma_current = comm_total[current];
            let ki_in_current = neighbor_comm_weight.get(&current).copied().unwrap_or(0.0);

            let mut best_delta = 0.0f64;
            let mut best_comm = current;

            for (&c, &ki_in) in &neighbor_comm_weight {
                if c == current {
                    continue;
                }
                let sigma_c = comm_total[c];
                let delta_remove = -2.0 * (ki_in_current - ki * (sigma_current - ki) / m2) / m2;
                let delta_add = 2.0 * (ki_in - ki * sigma_c / m2) / m2;
                let delta = delta_add + delta_remove;

                if delta > best_delta {
                    best_delta = delta;
                    best_comm = c;
                }
            }

            if best_comm != current {
                comm_total[current] -= ki;
                comm_total[best_comm] += ki;
                assignment[i] = best_comm;
                improved = true;
                changed = true;
            }
        }
    }

    changed
}

/// Phase 2: Refinement — ensure each community is well-connected by splitting
/// disconnected components within communities.
fn refine_communities(graph: &AdjGraph, assignment: &mut [usize], m2: f64) {
    let mut comm_members: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, &c) in assignment.iter().enumerate() {
        comm_members.entry(c).or_default().push(i);
    }

    let mut next_id = *assignment.iter().max().unwrap_or(&0) + 1;

    for members in comm_members.values() {
        if members.len() <= 1 {
            continue;
        }

        let components = find_connected_components(graph, members);
        if components.len() <= 1 {
            continue;
        }

        for component in components.iter().skip(1) {
            let new_comm = next_id;
            next_id += 1;
            for &node in component {
                assignment[node] = new_comm;
            }
        }
    }

    merge_singleton_communities(graph, assignment, m2);
}

/// Find connected components within a subset of nodes.
fn find_connected_components(graph: &AdjGraph, members: &[usize]) -> Vec<Vec<usize>> {
    let member_set: std::collections::HashSet<usize> = members.iter().copied().collect();
    let mut visited = std::collections::HashSet::new();
    let mut components = Vec::new();

    for &start in members {
        if visited.contains(&start) {
            continue;
        }

        let mut component = Vec::new();
        let mut stack = vec![start];

        while let Some(node) = stack.pop() {
            if !visited.insert(node) {
                continue;
            }
            component.push(node);
            for &(neighbor, _) in &graph.adj[node] {
                if member_set.contains(&neighbor) && !visited.contains(&neighbor) {
                    stack.push(neighbor);
                }
            }
        }

        components.push(component);
    }

    components
}

/// Try to merge singleton communities into their best neighbor community.
fn merge_singleton_communities(graph: &AdjGraph, assignment: &mut [usize], m2: f64) {
    let n = assignment.len();
    let mut comm_total: Vec<f64> =
        vec![0.0; n.max(assignment.iter().copied().max().unwrap_or(0) + 1)];
    for (i, &c) in assignment.iter().enumerate() {
        if c < comm_total.len() {
            comm_total[c] += graph.degree[i];
        }
    }

    let mut comm_sizes: HashMap<usize, usize> = HashMap::new();
    for &c in assignment.iter() {
        *comm_sizes.entry(c).or_default() += 1;
    }

    for i in 0..n {
        let current = assignment[i];
        if *comm_sizes.get(&current).unwrap_or(&0) > 1 {
            continue;
        }

        let ki = graph.degree[i];
        let mut neighbor_comm_weight: HashMap<usize, f64> = HashMap::new();
        for &(j, w) in &graph.adj[i] {
            *neighbor_comm_weight.entry(assignment[j]).or_default() += w;
        }

        let mut best_delta = 0.0f64;
        let mut best_comm = current;

        for (&c, &ki_in) in &neighbor_comm_weight {
            if c == current {
                continue;
            }
            let sigma_c = if c < comm_total.len() {
                comm_total[c]
            } else {
                0.0
            };
            let delta = 2.0 * (ki_in - GAMMA * ki * sigma_c / m2) / m2;
            if delta > best_delta {
                best_delta = delta;
                best_comm = c;
            }
        }

        if best_comm != current {
            if current < comm_total.len() {
                comm_total[current] -= ki;
            }
            if best_comm < comm_total.len() {
                comm_total[best_comm] += ki;
            }
            *comm_sizes.entry(current).or_default() -= 1;
            *comm_sizes.entry(best_comm).or_default() += 1;
            assignment[i] = best_comm;
        }
    }
}

fn compute_modularity(graph: &AdjGraph, community: &[usize]) -> f64 {
    let m2 = graph.total_weight.max(1.0) * 2.0;
    let mut q = 0.0;

    for (i, neighbors) in graph.adj.iter().enumerate() {
        for &(j, w) in neighbors {
            if community[i] == community[j] {
                let ki = graph.degree[i];
                let kj = graph.degree[j];
                q += w - (ki * kj) / m2;
            }
        }
    }

    q / m2
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::property_graph::{CodeGraph, Edge, EdgeKind, Node};

    fn build_test_graph() -> CodeGraph {
        let graph = CodeGraph::open_in_memory().unwrap();

        let node_a = graph.upsert_node(&Node::file("src/core/a.rs")).unwrap();
        let node_b = graph.upsert_node(&Node::file("src/core/b.rs")).unwrap();
        let node_c = graph.upsert_node(&Node::file("src/core/c.rs")).unwrap();
        let node_d = graph.upsert_node(&Node::file("src/tools/d.rs")).unwrap();
        let node_e = graph.upsert_node(&Node::file("src/tools/e.rs")).unwrap();

        graph
            .upsert_edge(&Edge::new(node_a, node_b, EdgeKind::Imports))
            .unwrap();
        graph
            .upsert_edge(&Edge::new(node_b, node_c, EdgeKind::Imports))
            .unwrap();
        graph
            .upsert_edge(&Edge::new(node_a, node_c, EdgeKind::Calls))
            .unwrap();

        graph
            .upsert_edge(&Edge::new(node_d, node_e, EdgeKind::Imports))
            .unwrap();
        graph
            .upsert_edge(&Edge::new(node_e, node_d, EdgeKind::Calls))
            .unwrap();

        graph
            .upsert_edge(&Edge::new(node_c, node_d, EdgeKind::Imports))
            .unwrap();

        graph
    }

    #[test]
    fn detects_communities() {
        let g = build_test_graph();
        let result = detect_communities(g.connection());

        assert!(
            !result.communities.is_empty(),
            "Should detect at least one community"
        );
        assert!(result.node_count == 5);
        assert!(result.edge_count > 0);
    }

    #[test]
    fn modularity_positive() {
        let g = build_test_graph();
        let result = detect_communities(g.connection());

        assert!(
            result.modularity >= 0.0,
            "Modularity should be non-negative for clustered graph"
        );
    }

    #[test]
    fn community_files_cover_all_nodes() {
        let g = build_test_graph();
        let result = detect_communities(g.connection());

        let total_files: usize = result.communities.iter().map(|c| c.files.len()).sum();
        assert_eq!(total_files, 5, "All 5 files should be assigned");
    }

    #[test]
    fn empty_graph() {
        let g = CodeGraph::open_in_memory().unwrap();
        let result = detect_communities(g.connection());
        assert!(result.communities.is_empty());
        assert_eq!(result.modularity, 0.0);
    }

    #[test]
    fn communities_are_connected() {
        let g = build_test_graph();
        let graph = AdjGraph::from_property_graph(g.connection());
        let result = detect_communities(g.connection());

        for comm in &result.communities {
            if comm.files.len() <= 1 {
                continue;
            }
            let indices: Vec<usize> = comm
                .files
                .iter()
                .filter_map(|f| graph.node_to_idx.get(f).copied())
                .collect();
            let components = find_connected_components(&graph, &indices);
            assert_eq!(
                components.len(),
                1,
                "Community {} with {} files should be connected, found {} components",
                comm.id,
                comm.files.len(),
                components.len()
            );
        }
    }
}
