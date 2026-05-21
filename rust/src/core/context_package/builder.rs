use chrono::Utc;
use sha2::{Digest, Sha256};

use super::content::{
    GotchaExport, GotchasLayer, GraphEdgeExport, GraphLayer, GraphNodeExport, KnowledgeLayer,
    PackageContent, SessionDecision, SessionFinding, SessionLayer,
};
use super::manifest::{
    CompatibilitySpec, PackageIntegrity, PackageLayer, PackageManifest, PackageProvenance,
    PackageStats,
};

pub struct PackageBuilder {
    name: String,
    version: String,
    description: String,
    author: Option<String>,
    tags: Vec<String>,
    compatibility: CompatibilitySpec,
    content: PackageContent,
    project_hash: Option<String>,
    session_id: Option<String>,
}

impl PackageBuilder {
    pub fn new(name: &str, version: &str) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            description: String::new(),
            author: None,
            tags: Vec::new(),
            compatibility: CompatibilitySpec::default(),
            content: PackageContent::default(),
            project_hash: None,
            session_id: None,
        }
    }

    pub fn description(mut self, desc: &str) -> Self {
        self.description = desc.to_string();
        self
    }

    pub fn author(mut self, author: &str) -> Self {
        self.author = Some(author.to_string());
        self
    }

    pub fn tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    pub fn compatibility(mut self, spec: CompatibilitySpec) -> Self {
        self.compatibility = spec;
        self
    }

    pub fn project_hash(mut self, hash: &str) -> Self {
        self.project_hash = Some(hash.to_string());
        self
    }

    pub fn session_id(mut self, id: &str) -> Self {
        self.session_id = Some(id.to_string());
        self
    }

    pub fn add_knowledge_from_project(mut self, project_root: &str) -> Self {
        let knowledge = crate::core::knowledge::ProjectKnowledge::load_or_create(project_root);

        if knowledge.facts.is_empty()
            && knowledge.patterns.is_empty()
            && knowledge.history.is_empty()
        {
            return self;
        }

        self.content.knowledge = Some(KnowledgeLayer {
            facts: knowledge.facts,
            patterns: knowledge.patterns,
            insights: knowledge.history,
            exported_at: Utc::now(),
        });

        self
    }

    pub fn add_graph_from_project(mut self, project_root: &str) -> Self {
        let Ok(graph) = crate::core::property_graph::CodeGraph::open(project_root) else {
            return self;
        };

        let nodes = export_graph_nodes(&graph);
        let edges = export_graph_edges(&graph);

        if nodes.is_empty() && edges.is_empty() {
            return self;
        }

        self.content.graph = Some(GraphLayer {
            nodes,
            edges,
            exported_at: Utc::now(),
        });

        self
    }

    pub fn add_session(mut self, session: &crate::core::session::SessionState) -> Self {
        let has_content = session.task.is_some()
            || !session.findings.is_empty()
            || !session.decisions.is_empty()
            || !session.next_steps.is_empty()
            || !session.files_touched.is_empty();

        if !has_content {
            return self;
        }

        let layer = SessionLayer {
            task_description: session.task.as_ref().map(|t| t.description.clone()),
            findings: session
                .findings
                .iter()
                .map(|f| SessionFinding {
                    summary: f.summary.clone(),
                    file: f.file.clone(),
                    line: f.line,
                })
                .collect(),
            decisions: session
                .decisions
                .iter()
                .map(|d| SessionDecision {
                    summary: d.summary.clone(),
                    rationale: d.rationale.clone(),
                })
                .collect(),
            next_steps: session.next_steps.clone(),
            files_touched: session
                .files_touched
                .iter()
                .map(|f| f.path.clone())
                .collect(),
            exported_at: Utc::now(),
        };

        self.content.session = Some(layer);
        self
    }

    pub fn add_gotchas_from_project(mut self, project_root: &str) -> Self {
        let store = crate::core::gotcha_tracker::GotchaStore::load(project_root);
        if store.gotchas.is_empty() {
            return self;
        }

        self.content.gotchas = Some(GotchasLayer {
            gotchas: store
                .gotchas
                .iter()
                .map(|g| GotchaExport {
                    id: g.id.clone(),
                    category: g.category.short_label().to_string(),
                    severity: match g.severity {
                        crate::core::gotcha_tracker::GotchaSeverity::Critical => "critical".into(),
                        crate::core::gotcha_tracker::GotchaSeverity::Warning => "warning".into(),
                        crate::core::gotcha_tracker::GotchaSeverity::Info => "info".into(),
                    },
                    trigger: g.trigger.clone(),
                    resolution: g.resolution.clone(),
                    file_patterns: g.file_patterns.clone(),
                    confidence: g.confidence,
                })
                .collect(),
            exported_at: Utc::now(),
        });

        self
    }

    pub fn build(self) -> Result<(PackageManifest, PackageContent), String> {
        if self.name.is_empty() {
            return Err("package name is required".into());
        }
        if self.version.is_empty() {
            return Err("package version is required".into());
        }
        if self.content.is_empty() {
            return Err("package has no content — add at least one layer".into());
        }

        let mut layers = Vec::new();
        if self.content.knowledge.is_some() {
            layers.push(PackageLayer::Knowledge);
        }
        if self.content.graph.is_some() {
            layers.push(PackageLayer::Graph);
        }
        if self.content.session.is_some() {
            layers.push(PackageLayer::Session);
        }
        if self.content.patterns.is_some() {
            layers.push(PackageLayer::Patterns);
        }
        if self.content.gotchas.is_some() {
            layers.push(PackageLayer::Gotchas);
        }

        let content_json = serde_json::to_string(&self.content).map_err(|e| e.to_string())?;
        let content_bytes = content_json.as_bytes();

        let content_hash = sha256_hex(content_bytes);
        let sha256 =
            sha256_hex(format!("{}:{}:{}", self.name, self.version, content_hash).as_bytes());

        let stats = compute_stats(&self.content);

        let manifest = PackageManifest {
            schema_version: crate::core::contracts::CONTEXT_PACKAGE_V1_SCHEMA_VERSION,
            name: self.name,
            version: self.version,
            description: self.description,
            author: self.author,
            created_at: Utc::now(),
            updated_at: None,
            layers,
            dependencies: Vec::new(),
            tags: self.tags,
            integrity: PackageIntegrity {
                sha256,
                content_hash,
                byte_size: content_bytes.len() as u64,
            },
            provenance: PackageProvenance {
                tool: "lean-ctx".into(),
                tool_version: env!("CARGO_PKG_VERSION").into(),
                project_hash: self.project_hash,
                source_session_id: self.session_id,
            },
            compatibility: self.compatibility,
            stats,
        };

        manifest.validate().map_err(|errs| errs.join("; "))?;

        Ok((manifest, self.content))
    }
}

fn export_graph_nodes(graph: &crate::core::property_graph::CodeGraph) -> Vec<GraphNodeExport> {
    let conn = graph.connection();
    let Ok(mut stmt) =
        conn.prepare("SELECT kind, name, file_path, line_start, line_end, metadata FROM nodes")
    else {
        tracing::warn!("ctxpkg: failed to prepare graph nodes query");
        return Vec::new();
    };

    let Ok(rows) = stmt.query_map([], |row| {
        let line_start: Option<i64> = row.get(3)?;
        let line_end: Option<i64> = row.get(4)?;
        Ok(GraphNodeExport {
            kind: row.get(0)?,
            name: row.get(1)?,
            file_path: row.get(2)?,
            line_start: line_start.map(|v| v as usize),
            line_end: line_end.map(|v| v as usize),
            metadata: row.get(5)?,
        })
    }) else {
        tracing::warn!("ctxpkg: failed to query graph nodes");
        return Vec::new();
    };

    let mut nodes = Vec::new();
    for row in rows {
        match row {
            Ok(n) => nodes.push(n),
            Err(e) => tracing::warn!("ctxpkg: skipping graph node: {e}"),
        }
    }
    nodes
}

fn export_graph_edges(graph: &crate::core::property_graph::CodeGraph) -> Vec<GraphEdgeExport> {
    let conn = graph.connection();
    let sql = "
        SELECT n1.file_path, n1.name, n2.file_path, n2.name, e.kind, e.metadata
        FROM edges e
        JOIN nodes n1 ON e.source_id = n1.id
        JOIN nodes n2 ON e.target_id = n2.id
    ";
    let Ok(mut stmt) = conn.prepare(sql) else {
        tracing::warn!("ctxpkg: failed to prepare graph edges query");
        return Vec::new();
    };

    let Ok(rows) = stmt.query_map([], |row| {
        Ok(GraphEdgeExport {
            source_path: row.get(0)?,
            source_name: row.get(1)?,
            target_path: row.get(2)?,
            target_name: row.get(3)?,
            kind: row.get(4)?,
            metadata: row.get(5)?,
        })
    }) else {
        tracing::warn!("ctxpkg: failed to query graph edges");
        return Vec::new();
    };

    let mut edges = Vec::new();
    for row in rows {
        match row {
            Ok(e) => edges.push(e),
            Err(e) => tracing::warn!("ctxpkg: skipping graph edge: {e}"),
        }
    }
    edges
}

fn compute_stats(content: &PackageContent) -> PackageStats {
    let knowledge_facts = content
        .knowledge
        .as_ref()
        .map_or(0, |k| k.facts.len() as u32);
    let graph_nodes = content.graph.as_ref().map_or(0, |g| g.nodes.len() as u32);
    let graph_edges = content.graph.as_ref().map_or(0, |g| g.edges.len() as u32);
    let pattern_count = content
        .patterns
        .as_ref()
        .map_or(0, |p| p.patterns.len() as u32);
    let gotcha_count = content
        .gotchas
        .as_ref()
        .map_or(0, |g| g.gotchas.len() as u32);

    let raw_json = serde_json::to_string(content).unwrap_or_default();
    let compression_ratio = {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        let _ = encoder.write_all(raw_json.as_bytes());
        let compressed = encoder.finish().unwrap_or_default();
        if raw_json.is_empty() {
            1.0
        } else {
            compressed.len() as f64 / raw_json.len() as f64
        }
    };

    PackageStats {
        knowledge_facts,
        graph_nodes,
        graph_edges,
        pattern_count,
        gotcha_count,
        compression_ratio,
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_builder_fails() {
        let result = PackageBuilder::new("test", "1.0.0").build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no content"));
    }

    #[test]
    fn sha256_is_deterministic() {
        let a = sha256_hex(b"hello world");
        let b = sha256_hex(b"hello world");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }
}
