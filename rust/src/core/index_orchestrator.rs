use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::core::bm25_index::BM25Index;
use crate::core::graph_index::{self, ProjectIndex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Building,
    Ready,
    Failed,
}

#[derive(Debug, Clone)]
struct Component {
    state: State,
    started_ms: Option<u64>,
    finished_ms: Option<u64>,
    duration_ms: Option<u64>,
    last_error: Option<String>,
}

impl Component {
    fn new() -> Self {
        Self {
            state: State::Idle,
            started_ms: None,
            finished_ms: None,
            duration_ms: None,
            last_error: None,
        }
    }
}

#[derive(Debug)]
struct ProjectBuild {
    worker_running: bool,
    graph: Component,
    bm25: Component,
}

impl ProjectBuild {
    fn new() -> Self {
        Self {
            worker_running: false,
            graph: Component::new(),
            bm25: Component::new(),
        }
    }
}

// Lock ordering (see rust/LOCK_ORDERING.md):
//   L1 = REGISTRY outer Mutex  (the HashMap guard)
//   L2 = per-project Arc<Mutex<ProjectBuild>>  (inner guard)
//
// Invariant: L1 must NEVER be held while locking L2.
// `entry_for()` enforces this by cloning the Arc and dropping L1 before
// the caller acquires L2.
static REGISTRY: OnceLock<Mutex<HashMap<String, Arc<Mutex<ProjectBuild>>>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, Arc<Mutex<ProjectBuild>>>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn entry_for(project_root: &str) -> Arc<Mutex<ProjectBuild>> {
    let mut map = registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.entry(project_root.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(ProjectBuild::new())))
        .clone()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn start_component(c: &mut Component) {
    c.state = State::Building;
    c.started_ms = Some(now_ms());
    c.finished_ms = None;
    c.duration_ms = None;
    c.last_error = None;
}

fn finish_ok(c: &mut Component) {
    c.state = State::Ready;
    let end = now_ms();
    c.finished_ms = Some(end);
    c.duration_ms = c.started_ms.map(|s| end.saturating_sub(s));
}

fn finish_err(c: &mut Component, e: String) {
    c.state = State::Failed;
    let end = now_ms();
    c.finished_ms = Some(end);
    c.duration_ms = c.started_ms.map(|s| end.saturating_sub(s));
    c.last_error = Some(e);
}

pub fn ensure_all_background(project_root: &str) {
    let state = entry_for(project_root);
    let should_spawn = {
        let mut s = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if s.worker_running {
            false
        } else {
            s.worker_running = true;
            true
        }
    };

    if !should_spawn {
        return;
    }

    let root = project_root.to_string();
    std::thread::spawn(move || {
        let state = entry_for(&root);

        {
            let mut s = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            start_component(&mut s.graph);
        }
        let idx = std::panic::catch_unwind(|| {
            let idx = graph_index::load_or_build(&root);
            let _ = idx.save();
            idx
        });
        if let Ok(_i) = idx {
            let mut s = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            finish_ok(&mut s.graph);
        } else {
            let mut s = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            finish_err(&mut s.graph, "graph index build panicked".to_string());
        }

        {
            let mut s = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            start_component(&mut s.bm25);
        }
        let bm = std::panic::catch_unwind(|| {
            let root_pb = Path::new(&root);
            let idx = BM25Index::load_or_build(root_pb);
            let _ = idx.save(root_pb);
        });
        if let Ok(()) = bm {
            let mut s = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            finish_ok(&mut s.bm25);
        } else {
            let mut s = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            finish_err(&mut s.bm25, "bm25 build panicked".to_string());
        }

        let mut s = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        s.worker_running = false;
    });
}

pub fn try_load_graph_index(project_root: &str) -> Option<ProjectIndex> {
    ProjectIndex::load(project_root).filter(|idx| !idx.files.is_empty())
}

pub fn try_load_bm25_index(project_root: &str) -> Option<BM25Index> {
    BM25Index::load(Path::new(project_root))
}

#[derive(Debug, Serialize)]
struct ComponentStatus<'a> {
    state: &'a str,
    started_ms: Option<u64>,
    finished_ms: Option<u64>,
    duration_ms: Option<u64>,
    last_error: Option<&'a str>,
}

fn component_status(c: &Component) -> ComponentStatus<'_> {
    ComponentStatus {
        state: match c.state {
            State::Idle => "idle",
            State::Building => "building",
            State::Ready => "ready",
            State::Failed => "failed",
        },
        started_ms: c.started_ms,
        finished_ms: c.finished_ms,
        duration_ms: c.duration_ms,
        last_error: c.last_error.as_deref(),
    }
}

#[derive(Debug, Serialize)]
struct StatusResponse<'a> {
    project_root: &'a str,
    graph_index: ComponentStatus<'a>,
    bm25_index: ComponentStatus<'a>,
    disk: DiskStatusAll,
}

#[derive(Debug, Serialize, Default)]
pub struct DiskStatus {
    pub exists: bool,
    pub size_bytes: Option<u64>,
    pub file_count: Option<u64>,
    pub modified_at: Option<String>,
}

#[derive(Debug, Serialize, Default)]
pub struct DiskStatusAll {
    pub graph_index: DiskStatus,
    pub bm25_index: DiskStatus,
    pub code_graph: DiskStatus,
}

fn disk_status_for_graph(project_root: &str) -> DiskStatus {
    let Some(dir) = graph_index::ProjectIndex::index_dir(project_root) else {
        return DiskStatus::default();
    };
    let zst = dir.join("index.json.zst");
    let json = dir.join("index.json");
    let path = if zst.exists() {
        zst
    } else if json.exists() {
        json
    } else {
        return DiskStatus::default();
    };
    let meta = std::fs::metadata(&path).ok();
    let file_count =
        graph_index::ProjectIndex::load(project_root).map(|idx| idx.files.len() as u64);
    DiskStatus {
        exists: true,
        size_bytes: meta.as_ref().map(std::fs::Metadata::len),
        file_count,
        modified_at: meta.and_then(|m| m.modified().ok()).map(format_time),
    }
}

fn disk_status_for_bm25(project_root: &str) -> DiskStatus {
    let root = Path::new(project_root);
    let path = BM25Index::index_file_path(root);
    if !path.exists() {
        return DiskStatus::default();
    }
    let meta = std::fs::metadata(&path).ok();
    DiskStatus {
        exists: true,
        size_bytes: meta.as_ref().map(std::fs::Metadata::len),
        file_count: None,
        modified_at: meta.and_then(|m| m.modified().ok()).map(format_time),
    }
}

fn disk_status_for_code_graph(project_root: &str) -> DiskStatus {
    let dir = crate::core::property_graph::graph_dir(project_root);
    let db_path = dir.join("graph.db");
    if !db_path.exists() {
        return DiskStatus::default();
    }
    let meta = std::fs::metadata(&db_path).ok();
    let node_count = crate::core::property_graph::CodeGraph::open(project_root)
        .ok()
        .and_then(|g| {
            g.connection()
                .query_row("SELECT count(*) FROM nodes", [], |r| r.get::<_, i64>(0))
                .ok()
                .map(|c| c as u64)
        });
    DiskStatus {
        exists: true,
        size_bytes: meta.as_ref().map(std::fs::Metadata::len),
        file_count: node_count,
        modified_at: meta.and_then(|m| m.modified().ok()).map(format_time),
    }
}

fn format_time(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let dt = chrono::DateTime::from_timestamp(secs as i64, 0);
    dt.map_or_else(
        || format!("{secs}"),
        |d| d.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
    )
}

pub fn disk_status(project_root: &str) -> DiskStatusAll {
    DiskStatusAll {
        graph_index: disk_status_for_graph(project_root),
        bm25_index: disk_status_for_bm25(project_root),
        code_graph: disk_status_for_code_graph(project_root),
    }
}

pub fn status_json(project_root: &str) -> String {
    let state = entry_for(project_root);
    let s = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let res = StatusResponse {
        project_root,
        graph_index: component_status(&s.graph),
        bm25_index: component_status(&s.bm25),
        disk: disk_status(project_root),
    };
    serde_json::to_string(&res).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_json_is_valid_json() {
        let s = status_json("/tmp");
        let _: serde_json::Value = serde_json::from_str(&s).unwrap();
    }
}
