//! Retrieval evaluation harness for lean-ctx hybrid search.
//!
//! Runs a standardized query→expected_file benchmark to measure Recall@k,
//! MRR (Mean Reciprocal Rank), and latency. Outputs NDJSON scorecards.
//!
//! Usage: `lean-ctx benchmark --eval [path]`

use std::path::Path;
use std::time::Instant;

use crate::core::bm25_index::BM25Index;
use crate::core::hybrid_search::HybridConfig;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EvalQuery {
    pub query: String,
    pub expected_files: Vec<String>,
    #[serde(default)]
    pub category: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EvalResult {
    pub query: String,
    pub category: String,
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    pub mrr: f64,
    pub latency_us: u64,
    pub retrieved_files: Vec<String>,
    pub expected_files: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EvalScorecard {
    pub project: String,
    pub total_queries: usize,
    pub avg_recall_at_5: f64,
    pub avg_recall_at_10: f64,
    pub avg_mrr: f64,
    pub avg_latency_us: u64,
    pub per_category: Vec<CategoryScore>,
    pub results: Vec<EvalResult>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CategoryScore {
    pub category: String,
    pub count: usize,
    pub avg_recall_at_5: f64,
    pub avg_mrr: f64,
}

impl std::fmt::Display for EvalScorecard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Eval: {} ({} queries)", self.project, self.total_queries)?;
        writeln!(f, "  R@5:  {:.1}%", self.avg_recall_at_5 * 100.0)?;
        writeln!(f, "  R@10: {:.1}%", self.avg_recall_at_10 * 100.0)?;
        writeln!(f, "  MRR:  {:.3}", self.avg_mrr)?;
        writeln!(f, "  Latency: {}µs avg", self.avg_latency_us)?;
        for cat in &self.per_category {
            writeln!(
                f,
                "  [{:12}] R@5={:.1}% MRR={:.3} (n={})",
                cat.category,
                cat.avg_recall_at_5 * 100.0,
                cat.avg_mrr,
                cat.count
            )?;
        }
        Ok(())
    }
}

/// Run evaluation using the full hybrid search pipeline (BM25 + embeddings + SPLADE).
/// Falls back to BM25-only if embeddings are not available.
pub fn run_eval(
    project_root: &Path,
    queries: &[EvalQuery],
    index: &BM25Index,
    config: &HybridConfig,
) -> EvalScorecard {
    let label = project_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let mut results = Vec::with_capacity(queries.len());

    for q in queries {
        let start = Instant::now();
        let retrieved = hybrid_eval_search(project_root, &q.query, index, config);
        let latency = start.elapsed().as_micros() as u64;

        let recall_5 = recall_at_k(&retrieved, &q.expected_files, 5);
        let recall_10 = recall_at_k(&retrieved, &q.expected_files, 10);
        let mrr = mean_reciprocal_rank(&retrieved, &q.expected_files);

        results.push(EvalResult {
            query: q.query.clone(),
            category: q.category.clone(),
            recall_at_5: recall_5,
            recall_at_10: recall_10,
            mrr,
            latency_us: latency,
            retrieved_files: retrieved.into_iter().take(10).collect(),
            expected_files: q.expected_files.clone(),
        });
    }

    let total = results.len();
    let avg_r5 = results.iter().map(|r| r.recall_at_5).sum::<f64>() / total.max(1) as f64;
    let avg_r10 = results.iter().map(|r| r.recall_at_10).sum::<f64>() / total.max(1) as f64;
    let avg_mrr = results.iter().map(|r| r.mrr).sum::<f64>() / total.max(1) as f64;
    let avg_lat = results.iter().map(|r| r.latency_us).sum::<u64>() / total.max(1) as u64;

    let per_category = build_category_scores(&results);

    EvalScorecard {
        project: label,
        total_queries: total,
        avg_recall_at_5: avg_r5,
        avg_recall_at_10: avg_r10,
        avg_mrr,
        avg_latency_us: avg_lat,
        per_category,
        results,
    }
}

/// Full hybrid search for eval: BM25 + dense embeddings + SPLADE + RRF.
/// Falls back to BM25-only when embeddings are unavailable.
fn hybrid_eval_search(
    project_root: &Path,
    query: &str,
    index: &BM25Index,
    config: &HybridConfig,
) -> Vec<String> {
    #[cfg(feature = "embeddings")]
    {
        if let Ok(results) = try_hybrid_search(project_root, query, index, config) {
            return results;
        }
    }
    let _ = project_root;
    index
        .search(query, config.bm25_candidates)
        .iter()
        .map(|r| r.file_path.clone())
        .collect()
}

#[cfg(feature = "embeddings")]
fn try_hybrid_search(
    project_root: &Path,
    query: &str,
    index: &BM25Index,
    config: &HybridConfig,
) -> Result<Vec<String>, String> {
    use crate::core::dense_backend;
    use crate::tools::ctx_semantic_search;

    let (engine, mut embed_idx) = ctx_semantic_search::load_engine_and_index_pub(project_root)?;

    let (aligned, _coverage, changed_files) = ctx_semantic_search::ensure_embeddings_for_eval(
        project_root,
        index,
        engine,
        &mut embed_idx,
    )?;

    let backend = dense_backend::DenseBackendKind::try_from_env()?;
    let candidate_k = config.bm25_candidates.max(config.dense_candidates);

    let mut results = dense_backend::hybrid_results(
        backend,
        project_root,
        index,
        engine,
        &aligned,
        &changed_files,
        query,
        candidate_k,
        config,
        None,
        None,
    )?;

    if config.splade_weight > 0.0 {
        let splade = crate::core::splade_retrieval::hybrid_retrieve(query, index, candidate_k);
        if !splade.is_empty() {
            ctx_semantic_search::boost_with_splade_pub(&mut results, &splade, config.splade_weight);
        }
    }

    results.truncate(10);
    Ok(results.iter().map(|r| r.file_path.clone()).collect())
}

/// Generate self-eval queries from an indexed codebase.
/// Picks random symbols/files and constructs retrieval queries.
pub fn generate_self_eval(index: &BM25Index, max_queries: usize) -> Vec<EvalQuery> {
    let mut queries = Vec::new();

    for chunk in index.chunks.iter().take(max_queries * 2) {
        if queries.len() >= max_queries {
            break;
        }
        if chunk.symbol_name.is_empty() || chunk.file_path.is_empty() {
            continue;
        }

        let category = if chunk.symbol_name.starts_with("fn ") || chunk.symbol_name.contains("()") {
            "function"
        } else if chunk.symbol_name.starts_with("struct ")
            || chunk.symbol_name.starts_with("class ")
        {
            "type"
        } else {
            "symbol"
        };

        let clean_name = chunk
            .symbol_name
            .replace("fn ", "")
            .replace("struct ", "")
            .replace("class ", "")
            .replace("()", "");

        queries.push(EvalQuery {
            query: format!("where is {clean_name} defined"),
            expected_files: vec![chunk.file_path.clone()],
            category: category.to_string(),
        });
    }

    queries
}

fn recall_at_k(retrieved: &[String], expected: &[String], k: usize) -> f64 {
    if expected.is_empty() {
        return 0.0;
    }
    let top_k: Vec<&str> = retrieved
        .iter()
        .take(k)
        .map(std::string::String::as_str)
        .collect();
    let hits = expected
        .iter()
        .filter(|e| {
            top_k
                .iter()
                .any(|r| r.ends_with(e.as_str()) || e.ends_with(r))
        })
        .count();
    hits as f64 / expected.len() as f64
}

fn mean_reciprocal_rank(retrieved: &[String], expected: &[String]) -> f64 {
    for (rank, r) in retrieved.iter().enumerate() {
        if expected
            .iter()
            .any(|e| r.ends_with(e.as_str()) || e.ends_with(r.as_str()))
        {
            return 1.0 / (rank as f64 + 1.0);
        }
    }
    0.0
}

fn build_category_scores(results: &[EvalResult]) -> Vec<CategoryScore> {
    use std::collections::HashMap;
    let mut cat_map: HashMap<&str, Vec<&EvalResult>> = HashMap::new();
    for r in results {
        cat_map.entry(r.category.as_str()).or_default().push(r);
    }

    let mut scores: Vec<CategoryScore> = cat_map
        .into_iter()
        .map(|(cat, items)| {
            let n = items.len();
            CategoryScore {
                category: cat.to_string(),
                count: n,
                avg_recall_at_5: items.iter().map(|r| r.recall_at_5).sum::<f64>() / n as f64,
                avg_mrr: items.iter().map(|r| r.mrr).sum::<f64>() / n as f64,
            }
        })
        .collect();
    scores.sort_by(|a, b| a.category.cmp(&b.category));
    scores
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recall_at_k_full_match() {
        let retrieved = vec!["a.rs".into(), "b.rs".into(), "c.rs".into()];
        let expected = vec!["a.rs".into()];
        assert_eq!(recall_at_k(&retrieved, &expected, 5), 1.0);
    }

    #[test]
    fn recall_at_k_no_match() {
        let retrieved = vec!["x.rs".into(), "y.rs".into()];
        let expected = vec!["a.rs".into()];
        assert_eq!(recall_at_k(&retrieved, &expected, 5), 0.0);
    }

    #[test]
    fn recall_at_k_partial() {
        let retrieved = vec!["a.rs".into(), "x.rs".into()];
        let expected = vec!["a.rs".into(), "b.rs".into()];
        assert_eq!(recall_at_k(&retrieved, &expected, 5), 0.5);
    }

    #[test]
    fn mrr_first_hit() {
        let retrieved = vec!["a.rs".into(), "b.rs".into()];
        let expected = vec!["a.rs".into()];
        assert_eq!(mean_reciprocal_rank(&retrieved, &expected), 1.0);
    }

    #[test]
    fn mrr_second_hit() {
        let retrieved = vec!["x.rs".into(), "a.rs".into()];
        let expected = vec!["a.rs".into()];
        assert_eq!(mean_reciprocal_rank(&retrieved, &expected), 0.5);
    }

    #[test]
    fn mrr_no_hit() {
        let retrieved = vec!["x.rs".into()];
        let expected = vec!["a.rs".into()];
        assert_eq!(mean_reciprocal_rank(&retrieved, &expected), 0.0);
    }

    #[test]
    fn empty_expected() {
        assert_eq!(recall_at_k(&["a.rs".into()], &[], 5), 0.0);
    }

    #[test]
    fn scorecard_display() {
        let sc = EvalScorecard {
            project: "test".into(),
            total_queries: 10,
            avg_recall_at_5: 0.8,
            avg_recall_at_10: 0.9,
            avg_mrr: 0.75,
            avg_latency_us: 100,
            per_category: vec![],
            results: vec![],
        };
        let s = format!("{sc}");
        assert!(s.contains("80.0%"));
        assert!(s.contains("0.750"));
    }
}
