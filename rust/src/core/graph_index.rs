use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::import_resolver;
use crate::core::signatures;

const INDEX_VERSION: u32 = 6;

pub fn is_safe_scan_root_public(path: &str) -> bool {
    is_safe_scan_root(path)
}

fn is_filesystem_root(path: &str) -> bool {
    let p = Path::new(path);
    p.parent().is_none() || (cfg!(windows) && p.parent() == Some(Path::new("")))
}

fn is_safe_scan_root(path: &str) -> bool {
    let normalized = normalize_project_root(path);
    let p = Path::new(&normalized);

    if normalized == "/" || normalized == "\\" || is_filesystem_root(&normalized) {
        tracing::warn!("[graph_index: refusing to scan filesystem root]");
        return false;
    }

    if normalized == "." || normalized.is_empty() {
        tracing::warn!("[graph_index: refusing to scan relative/empty root]");
        return false;
    }

    if let Some(home) = dirs::home_dir() {
        let home_norm = normalize_project_root(&home.to_string_lossy());
        if normalized == home_norm {
            use std::sync::Once;
            static HOME_WARN: Once = Once::new();
            HOME_WARN.call_once(|| {
                tracing::warn!(
                    "[graph_index: skipping — cannot index home directory {normalized}.\n  \
                     Run from inside a project, or set LEAN_CTX_PROJECT_ROOT=/path/to/project]"
                );
            });
            return false;
        }
        // Block common broad home subdirectories that are never valid project roots
        let home_path = Path::new(&home_norm);
        const BLOCKED_HOME_SUBDIRS: &[&str] = &[
            "Desktop",
            "Documents",
            "Downloads",
            "Pictures",
            "Music",
            "Videos",
            "Movies",
            "Library",
            ".local",
            ".cache",
            ".config",
            "snap",
            "Applications",
        ];
        for blocked in BLOCKED_HOME_SUBDIRS {
            let blocked_path = home_path.join(blocked);
            let is_inside_blocked = p == blocked_path || p.starts_with(&blocked_path);
            let has_project_marker = p.join(".git").exists()
                || p.join("Cargo.toml").exists()
                || p.join("package.json").exists();
            if is_inside_blocked && !has_project_marker {
                tracing::warn!(
                    "[graph_index: refusing to scan {normalized} — \
                     inside home/{blocked} without project markers]"
                );
                return false;
            }
        }

        // Block directories that are direct children of home without project markers
        if p.parent() == Some(home_path) {
            let has_marker = p.join(".git").exists()
                || p.join("Cargo.toml").exists()
                || p.join("package.json").exists()
                || p.join("go.mod").exists()
                || p.join("pyproject.toml").exists();
            if !has_marker {
                tracing::warn!(
                    "[graph_index: refusing to scan {normalized} — \
                     direct child of home without project markers]"
                );
                return false;
            }
        }
    }

    let breadth_markers = [
        ".git",
        "Cargo.toml",
        "package.json",
        "go.mod",
        "pyproject.toml",
        "setup.py",
        "Makefile",
        "CMakeLists.txt",
        "pnpm-workspace.yaml",
        ".projectile",
        "BUILD.bazel",
        "go.work",
    ];

    if !breadth_markers.iter().any(|m| p.join(m).exists()) {
        let child_count = std::fs::read_dir(p).map_or(0, |rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .count()
        });
        if child_count > 50 {
            tracing::warn!(
                "[graph_index: {normalized} has no project markers and {child_count} subdirectories — \
                 skipping scan to avoid indexing broad directories]"
            );
            return false;
        }
    }

    true
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectIndex {
    pub version: u32,
    pub project_root: String,
    pub last_scan: String,
    pub files: HashMap<String, FileEntry>,
    pub edges: Vec<IndexEdge>,
    pub symbols: HashMap<String, SymbolEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub hash: String,
    pub language: String,
    pub line_count: usize,
    pub token_count: usize,
    pub exports: Vec<String>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolEntry {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub start_line: usize,
    pub end_line: usize,
    pub is_exported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
    #[serde(default = "default_edge_weight")]
    pub weight: f32,
}

fn default_edge_weight() -> f32 {
    1.0
}

impl ProjectIndex {
    pub fn new(project_root: &str) -> Self {
        Self {
            version: INDEX_VERSION,
            project_root: normalize_project_root(project_root),
            last_scan: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            files: HashMap::new(),
            edges: Vec::new(),
            symbols: HashMap::new(),
        }
    }

    pub fn index_dir(project_root: &str) -> Option<std::path::PathBuf> {
        let normalized = normalize_project_root(project_root);
        let hash = crate::core::project_hash::hash_project_root(&normalized);
        crate::core::data_dir::lean_ctx_data_dir()
            .ok()
            .map(|d| d.join("graphs").join(hash))
    }

    pub fn load(project_root: &str) -> Option<Self> {
        let dir = Self::index_dir(project_root)?;

        let zst_path = dir.join("index.json.zst");
        if zst_path.exists() {
            let compressed = std::fs::read(&zst_path).ok()?;
            let data = zstd::decode_all(compressed.as_slice()).ok()?;
            let content = String::from_utf8(data).ok()?;
            let index: Self = serde_json::from_str(&content).ok()?;
            if index.version != INDEX_VERSION {
                return None;
            }
            return Some(index);
        }

        let json_path = dir.join("index.json");
        let content = std::fs::read_to_string(&json_path)
            .or_else(|_| -> std::io::Result<String> {
                let legacy_hash = short_hash(&normalize_project_root(project_root));
                let legacy_dir = crate::core::data_dir::lean_ctx_data_dir()
                    .map_err(|_| std::io::Error::new(std::io::ErrorKind::NotFound, "no data dir"))?
                    .join("graphs")
                    .join(legacy_hash);
                let legacy_path = legacy_dir.join("index.json");
                let data = std::fs::read_to_string(&legacy_path)?;
                if let Err(e) = copy_dir_fallible(&legacy_dir, &dir) {
                    tracing::debug!("graph index migration: {e}");
                }
                Ok(data)
            })
            .ok()?;
        let index: Self = serde_json::from_str(&content).ok()?;
        if index.version != INDEX_VERSION {
            return None;
        }
        // Auto-migrate: compress legacy JSON to zstd
        if let Ok(compressed) = zstd::encode_all(content.as_bytes(), 9) {
            let zst_tmp = zst_path.with_extension("zst.tmp");
            if std::fs::write(&zst_tmp, &compressed).is_ok()
                && std::fs::rename(&zst_tmp, &zst_path).is_ok()
            {
                let _ = std::fs::remove_file(&json_path);
            }
        }
        Some(index)
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = Self::index_dir(&self.project_root)
            .ok_or_else(|| "Cannot determine data directory".to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let json = serde_json::to_string(self).map_err(|e| e.to_string())?;
        let compressed = zstd::encode_all(json.as_bytes(), 9).map_err(|e| format!("zstd: {e}"))?;
        let target = dir.join("index.json.zst");
        let tmp = target.with_extension("zst.tmp");
        std::fs::write(&tmp, &compressed).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &target).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(dir.join("index.json"));
        Ok(())
    }

    /// Remove all cached graph indices that are older than max_age_hours.
    /// Called on startup/update to prevent stale data from persisting.
    pub fn purge_stale_indices() {
        let Ok(data_dir) = crate::core::data_dir::lean_ctx_data_dir() else {
            return;
        };
        let graphs_dir = data_dir.join("graphs");
        let Ok(entries) = std::fs::read_dir(&graphs_dir) else {
            return;
        };
        let cfg = crate::core::config::Config::load();
        let max_age_secs = cfg.archive.max_age_hours * 3600;

        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let zst = path.join("index.json.zst");
            let json = path.join("index.json");
            let index_file = if zst.exists() {
                &zst
            } else if json.exists() {
                &json
            } else {
                continue;
            };

            let is_old = index_file
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|mtime| {
                    mtime
                        .elapsed()
                        .is_ok_and(|age| age.as_secs() > max_age_secs)
                });

            if is_old {
                tracing::info!("[graph_index: purging stale index at {}]", path.display());
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn symbol_count(&self) -> usize {
        self.symbols.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn get_symbol(&self, key: &str) -> Option<&SymbolEntry> {
        self.symbols.get(key)
    }

    pub fn get_reverse_deps(&self, path: &str, depth: usize) -> Vec<String> {
        let mut result = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut queue: Vec<(String, usize)> = vec![(path.to_string(), 0)];

        while let Some((current, d)) = queue.pop() {
            if d > depth || visited.contains(&current) {
                continue;
            }
            visited.insert(current.clone());
            if current != path {
                result.push(current.clone());
            }

            for edge in &self.edges {
                if edge.to == current && edge.kind == "import" && !visited.contains(&edge.from) {
                    queue.push((edge.from.clone(), d + 1));
                }
            }
        }
        result
    }

    pub fn get_related(&self, path: &str, depth: usize) -> Vec<String> {
        let mut result = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut queue: Vec<(String, usize)> = vec![(path.to_string(), 0)];

        while let Some((current, d)) = queue.pop() {
            if d > depth || visited.contains(&current) {
                continue;
            }
            visited.insert(current.clone());
            if current != path {
                result.push(current.clone());
            }

            for edge in &self.edges {
                if edge.from == current && !visited.contains(&edge.to) {
                    queue.push((edge.to.clone(), d + 1));
                }
                if edge.to == current && !visited.contains(&edge.from) {
                    queue.push((edge.from.clone(), d + 1));
                }
            }
        }
        result
    }
}

/// Load the best available graph index, trying multiple root path variants.
/// If no valid index exists, automatically scans the project to build one.
/// This is the primary entry point — ensures zero-config usage.
pub fn load_or_build(project_root: &str) -> ProjectIndex {
    if std::env::var("LEAN_CTX_NO_INDEX").is_ok() {
        return ProjectIndex::load(project_root).unwrap_or_else(|| ProjectIndex::new(project_root));
    }

    // Prefer stable absolute roots. Using "." as a cache key is fragile because
    // it depends on the process cwd and can accidentally load the wrong project.
    let root_abs = if project_root.trim().is_empty() || project_root == "." {
        std::env::current_dir().ok().map_or_else(
            || ".".to_string(),
            |p| normalize_project_root(&p.to_string_lossy()),
        )
    } else {
        normalize_project_root(project_root)
    };

    if !is_safe_scan_root(&root_abs) {
        return ProjectIndex::new(&root_abs);
    }

    // Try the absolute/root-normalized path first.
    if let Some(idx) = ProjectIndex::load(&root_abs) {
        if !idx.files.is_empty() {
            if index_looks_stale(&idx, &root_abs) {
                tracing::warn!("[graph_index: stale index detected for {root_abs}; rebuilding]");
                return scan(&root_abs);
            }
            return idx;
        }
    }

    // CWD fallback: only use if CWD is a subdirectory of root_abs (same project)
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_str = normalize_project_root(&cwd.to_string_lossy());
        if cwd_str != root_abs && cwd_str.starts_with(&root_abs) {
            if let Some(idx) = ProjectIndex::load(&cwd_str) {
                if !idx.files.is_empty() {
                    if index_looks_stale(&idx, &cwd_str) {
                        return scan(&cwd_str);
                    }
                    return idx;
                }
            }
        }
    }

    scan(&root_abs)
}

fn index_looks_stale(index: &ProjectIndex, root_abs: &str) -> bool {
    if index.files.is_empty() {
        return true;
    }

    // TTL check: rebuild if index is older than configured max_age_hours
    if let Ok(scan_time) =
        chrono::NaiveDateTime::parse_from_str(&index.last_scan, "%Y-%m-%d %H:%M:%S")
    {
        let cfg = crate::core::config::Config::load();
        let max_age = chrono::Duration::hours(cfg.archive.max_age_hours as i64);
        let now = chrono::Local::now().naive_local();
        if now.signed_duration_since(scan_time) > max_age {
            tracing::info!(
                "[graph_index: index is older than {}h — marking stale]",
                cfg.archive.max_age_hours
            );
            return true;
        }
    }

    // Contamination check: if index contains paths from common user directories,
    // it was built from a too-broad root and must be rebuilt
    const CONTAMINATION_MARKERS: &[&str] = &[
        "Desktop/",
        "Documents/",
        "Downloads/",
        "Pictures/",
        "Music/",
        "Videos/",
        "Movies/",
        "Library/",
        ".cache/",
        "snap/",
    ];
    let contaminated = index.files.keys().take(200).any(|rel| {
        CONTAMINATION_MARKERS
            .iter()
            .any(|m| rel.starts_with(m) || rel.contains(&format!("/{m}")))
    });
    if contaminated {
        tracing::warn!(
            "[graph_index: index contains files from user directories (Desktop/Documents/...) — \
             marking stale to force clean rebuild]"
        );
        return true;
    }

    let root_path = Path::new(root_abs);
    // Sample up to 20 files for existence check (avoid scanning all files in large indices)
    let sample_size = index.files.len().min(20);
    for rel in index.files.keys().take(sample_size) {
        let rel = rel.trim_start_matches(['/', '\\']);
        if rel.is_empty() {
            continue;
        }
        let abs = root_path.join(rel);
        if !abs.exists() {
            return true;
        }
    }

    false
}

pub fn scan(project_root: &str) -> ProjectIndex {
    if std::env::var("LEAN_CTX_NO_INDEX").is_ok() {
        tracing::info!("[graph_index: LEAN_CTX_NO_INDEX set — skipping scan]");
        return ProjectIndex::new(project_root);
    }

    let project_root = normalize_project_root(project_root);

    if !is_safe_scan_root(&project_root) {
        tracing::warn!("[graph_index: scan aborted for unsafe root {project_root}]");
        return ProjectIndex::new(&project_root);
    }

    let lock_name = format!(
        "graph-idx-{}",
        &crate::core::index_namespace::namespace_hash(Path::new(&project_root))[..8]
    );
    let _lock = crate::core::startup_guard::try_acquire_lock(
        &lock_name,
        std::time::Duration::from_millis(800),
        std::time::Duration::from_mins(3),
    );
    if _lock.is_none() {
        tracing::info!(
            "[graph_index: another process is scanning {project_root} — returning cached or empty]"
        );
        return ProjectIndex::load(&project_root)
            .unwrap_or_else(|| ProjectIndex::new(&project_root));
    }

    let existing = ProjectIndex::load(&project_root);
    let mut index = ProjectIndex::new(&project_root);

    let old_files: HashMap<String, (String, Vec<(String, SymbolEntry)>)> =
        if let Some(ref prev) = existing {
            prev.files
                .iter()
                .map(|(path, entry)| {
                    let syms: Vec<(String, SymbolEntry)> = prev
                        .symbols
                        .iter()
                        .filter(|(_, s)| s.file == *path)
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    (path.clone(), (entry.hash.clone(), syms))
                })
                .collect()
        } else {
            HashMap::new()
        };

    let walker = ignore::WalkBuilder::new(&project_root)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .max_depth(Some(20))
        .build();

    let cfg = crate::core::config::Config::load();
    let extra_ignores: Vec<glob::Pattern> = cfg
        .extra_ignore_patterns
        .iter()
        .filter_map(|p| glob::Pattern::new(p).ok())
        .collect();

    let mut scanned = 0usize;
    let mut reused = 0usize;
    let mut entries_visited = 0usize;
    let max_files = if cfg.graph_index_max_files == 0 {
        usize::MAX // unlimited
    } else {
        cfg.graph_index_max_files as usize
    };
    const MAX_ENTRIES_VISITED: usize = 500_000;
    const MAX_FILE_SIZE_BYTES: u64 = 2 * 1024 * 1024; // 2 MB per file
    let scan_deadline = std::time::Instant::now() + std::time::Duration::from_mins(5);

    for entry in walker.filter_map(std::result::Result::ok) {
        entries_visited += 1;
        if entries_visited > MAX_ENTRIES_VISITED {
            tracing::warn!(
                "[graph_index: walked {entries_visited} entries — aborting scan to prevent \
                 runaway traversal. Indexed {} files so far.]",
                index.files.len()
            );
            break;
        }
        if entries_visited.is_multiple_of(5000) {
            if std::time::Instant::now() > scan_deadline {
                tracing::warn!(
                    "[graph_index: scan timeout (120s) after {entries_visited} entries — \
                     saving partial index with {} files]",
                    index.files.len()
                );
                break;
            }
            if crate::core::memory_guard::abort_requested() {
                tracing::warn!(
                    "[graph_index: memory pressure abort after {entries_visited} entries — \
                     saving partial index with {} files]",
                    index.files.len()
                );
                break;
            }
            if crate::core::memory_guard::is_under_pressure() {
                tracing::warn!(
                    "[graph_index: memory pressure detected at {entries_visited} entries — \
                     stopping scan with {} files]",
                    index.files.len()
                );
                break;
            }
            if let Some(ref g) = _lock {
                g.touch();
            }
        }

        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let file_path = normalize_absolute_path(&entry.path().to_string_lossy());

        // Prevent indexing files that escaped the project root (symlinks, mount points)
        if !file_path.starts_with(&project_root) {
            continue;
        }

        // Skip special files (devices, FIFOs, sockets) that can stream infinite data
        if let Ok(meta) = std::fs::metadata(&file_path) {
            if !meta.is_file() {
                continue;
            }
            if meta.len() > MAX_FILE_SIZE_BYTES {
                tracing::debug!(
                    "[graph_index: skipping {file_path} — {:.1}MB exceeds {}MB limit]",
                    meta.len() as f64 / 1_048_576.0,
                    MAX_FILE_SIZE_BYTES / (1024 * 1024),
                );
                continue;
            }
        }

        let ext = Path::new(&file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        if !is_indexable_ext(ext) {
            continue;
        }

        let rel = make_relative(&file_path, &project_root);
        if extra_ignores.iter().any(|p| p.matches(&rel)) {
            continue;
        }

        if max_files != usize::MAX && index.files.len() >= max_files {
            tracing::info!(
                "[graph_index: reached configured limit of {} files. Set graph_index_max_files = 0 for unlimited.]",
                max_files
            );
            break;
        }

        let Ok(content) = std::fs::read_to_string(&file_path) else {
            continue;
        };

        let hash = compute_hash(&content);
        let rel_path = make_relative(&file_path, &project_root);

        if let Some((old_hash, old_syms)) = old_files.get(&rel_path) {
            if *old_hash == hash {
                if let Some(old_entry) = existing.as_ref().and_then(|p| p.files.get(&rel_path)) {
                    index.files.insert(rel_path.clone(), old_entry.clone());
                    for (key, sym) in old_syms {
                        index.symbols.insert(key.clone(), sym.clone());
                    }
                    reused += 1;
                    continue;
                }
            }
        }

        let sigs = signatures::extract_signatures(&content, ext);
        let line_count = content.lines().count();
        let token_count = crate::core::tokens::count_tokens(&content);
        let summary = extract_summary(&content);

        let exports: Vec<String> = sigs
            .iter()
            .filter(|s| s.is_exported)
            .map(|s| s.name.clone())
            .collect();

        index.files.insert(
            rel_path.clone(),
            FileEntry {
                path: rel_path.clone(),
                hash,
                language: ext.to_string(),
                line_count,
                token_count,
                exports,
                summary,
            },
        );

        for sig in &sigs {
            let (start, end) = sig
                .start_line
                .zip(sig.end_line)
                .unwrap_or_else(|| find_symbol_range(&content, sig));
            let key = format!("{}::{}", rel_path, sig.name);
            index.symbols.insert(
                key,
                SymbolEntry {
                    file: rel_path.clone(),
                    name: sig.name.clone(),
                    kind: sig.kind.to_string(),
                    start_line: start,
                    end_line: end,
                    is_exported: sig.is_exported,
                },
            );
        }

        scanned += 1;
    }

    build_edges(&mut index);

    if let Err(e) = index.save() {
        tracing::warn!("could not save graph index: {e}");
    }

    tracing::warn!(
        "[graph_index: {} files ({} scanned, {} reused), {} symbols, {} edges]",
        index.file_count(),
        scanned,
        reused,
        index.symbol_count(),
        index.edge_count()
    );

    index
}

fn build_edges(index: &mut ProjectIndex) {
    build_edges_with_cache(index, &HashMap::new());
    build_implicit_edges(index);
    build_cochange_edges(index);
    build_sibling_edges(index);
}

fn build_edges_with_cache(index: &mut ProjectIndex, content_cache: &HashMap<String, String>) {
    index.edges.clear();

    if crate::core::memory_guard::abort_requested() {
        tracing::warn!("[graph_index: skipping edge-building due to memory pressure]");
        return;
    }

    let root = normalize_project_root(&index.project_root);
    let root_path = Path::new(&root);

    let mut file_paths: Vec<String> = index.files.keys().cloned().collect();
    file_paths.sort();

    let resolver_ctx = import_resolver::ResolverContext::new(root_path, file_paths.clone());

    const MAX_FILE_SIZE_FOR_EDGES: u64 = 2 * 1024 * 1024;

    for (i, rel_path) in file_paths.iter().enumerate() {
        if i.is_multiple_of(1000) && crate::core::memory_guard::is_under_pressure() {
            tracing::warn!(
                "[graph_index: stopping edge-building at file {i}/{} due to memory pressure]",
                file_paths.len()
            );
            break;
        }

        let content = if let Some(cached) = content_cache.get(rel_path) {
            std::borrow::Cow::Borrowed(cached.as_str())
        } else {
            let abs_path = root_path.join(rel_path.trim_start_matches(['/', '\\']));
            if let Ok(meta) = abs_path.metadata() {
                if meta.len() > MAX_FILE_SIZE_FOR_EDGES {
                    continue;
                }
            }
            match std::fs::read_to_string(&abs_path) {
                Ok(c) => std::borrow::Cow::Owned(c),
                Err(_) => continue,
            }
        };

        let ext = Path::new(rel_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let resolve_ext = match ext {
            "vue" | "svelte" => "ts",
            _ => ext,
        };

        let analysis_content = if ext == "vue" || ext == "svelte" {
            if let Some(script) = crate::core::signatures_ts::sfc::extract_script_block(&content) {
                std::borrow::Cow::Owned(script)
            } else {
                content
            }
        } else {
            content
        };

        let imports = crate::core::deep_queries::analyze(&analysis_content, resolve_ext).imports;
        if imports.is_empty() {
            continue;
        }

        let resolved =
            import_resolver::resolve_imports(&imports, rel_path, resolve_ext, &resolver_ctx);
        for r in resolved {
            if r.is_external {
                continue;
            }
            if let Some(to) = r.resolved_path {
                index.edges.push(IndexEdge {
                    from: rel_path.clone(),
                    to,
                    kind: "import".to_string(),
                    weight: 1.0,
                });
            }
        }
    }

    index.edges.sort_by(|a, b| {
        a.from
            .cmp(&b.from)
            .then_with(|| a.to.cmp(&b.to))
            .then_with(|| a.kind.cmp(&b.kind))
    });
    index
        .edges
        .dedup_by(|a, b| a.from == b.from && a.to == b.to && a.kind == b.kind);
}

// ---------------------------------------------------------------------------
// Layer 2: Implicit Language Edges (weight 0.8)
// ---------------------------------------------------------------------------

fn build_implicit_edges(index: &mut ProjectIndex) {
    let file_paths: Vec<String> = index.files.keys().cloned().collect();
    let file_set: std::collections::HashSet<&str> = file_paths.iter().map(String::as_str).collect();

    let mut new_edges: Vec<IndexEdge> = Vec::new();

    for file in &file_paths {
        let ext = Path::new(file.as_str())
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        match ext {
            "rs" => collect_rust_mod_edges(file, &file_set, index, &mut new_edges),
            "go" => collect_go_package_edges(file, &file_paths, &mut new_edges),
            "py" => collect_python_init_edges(file, &file_paths, &mut new_edges),
            "ts" | "js" | "tsx" | "jsx" => {
                collect_barrel_edges(file, &file_set, index, &mut new_edges);
            }
            _ => {}
        }
    }

    index.edges.extend(new_edges);
}

fn collect_rust_mod_edges(
    file: &str,
    file_set: &std::collections::HashSet<&str>,
    index: &ProjectIndex,
    edges: &mut Vec<IndexEdge>,
) {
    if !index.files.contains_key(file) {
        return;
    }

    let full_path = Path::new(&index.project_root).join(file);
    let Ok(content) = std::fs::read_to_string(&full_path) else {
        return;
    };

    let dir = Path::new(file)
        .parent()
        .map(|p| p.to_string_lossy().to_string());

    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("mod ") || trimmed.contains('{') {
            continue;
        }
        let mod_name = trimmed
            .trim_start_matches("mod ")
            .trim_start_matches("pub mod ")
            .trim_start_matches("pub(crate) mod ")
            .trim_end_matches(';')
            .trim();

        if mod_name.is_empty() || mod_name.contains(' ') {
            continue;
        }

        let candidates = if let Some(ref d) = dir {
            vec![
                format!("{d}/{mod_name}.rs"),
                format!("{d}/{mod_name}/mod.rs"),
            ]
        } else {
            vec![format!("{mod_name}.rs"), format!("{mod_name}/mod.rs")]
        };

        for candidate in candidates {
            if file_set.contains(candidate.as_str()) {
                edges.push(IndexEdge {
                    from: file.to_string(),
                    to: candidate,
                    kind: "module".to_string(),
                    weight: 0.8,
                });
                break;
            }
        }
    }
}

fn collect_go_package_edges(file: &str, file_paths: &[String], edges: &mut Vec<IndexEdge>) {
    let p = Path::new(file);
    if p.extension().and_then(|e| e.to_str()) != Some("go") {
        return;
    }
    if file.ends_with("_test.go") {
        return;
    }

    let Some(dir) = p.parent().map(|d| d.to_string_lossy().to_string()) else {
        return;
    };

    for other in file_paths {
        if other == file {
            continue;
        }
        let op = Path::new(other.as_str());
        if op.extension().and_then(|e| e.to_str()) != Some("go") {
            continue;
        }
        if other.ends_with("_test.go") {
            continue;
        }
        let other_dir = op
            .parent()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_default();
        if other_dir == dir {
            edges.push(IndexEdge {
                from: file.to_string(),
                to: other.clone(),
                kind: "package".to_string(),
                weight: 0.5,
            });
            break;
        }
    }
}

fn collect_python_init_edges(file: &str, file_paths: &[String], edges: &mut Vec<IndexEdge>) {
    let p = Path::new(file);
    if p.file_name().and_then(|n| n.to_str()) != Some("__init__.py") {
        return;
    }

    let Some(dir) = p.parent().map(|d| d.to_string_lossy().to_string()) else {
        return;
    };

    for other in file_paths {
        if other == file {
            continue;
        }
        let op = Path::new(other.as_str());
        if op.extension().and_then(|e| e.to_str()) != Some("py") {
            continue;
        }
        let other_dir = op
            .parent()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_default();
        if other_dir == dir {
            edges.push(IndexEdge {
                from: file.to_string(),
                to: other.clone(),
                kind: "module".to_string(),
                weight: 0.8,
            });
        }
    }
}

fn collect_barrel_edges(
    file: &str,
    file_set: &std::collections::HashSet<&str>,
    index: &ProjectIndex,
    edges: &mut Vec<IndexEdge>,
) {
    let basename = Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if basename != "index" {
        return;
    }

    let full_path = Path::new(&index.project_root).join(file);
    let Ok(content) = std::fs::read_to_string(&full_path) else {
        return;
    };

    let dir = Path::new(file)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let ext = Path::new(file)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("ts");

    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("export") || !trimmed.contains("from") {
            continue;
        }
        if let Some(from_pos) = trimmed.find("from") {
            let after = &trimmed[from_pos + 4..];
            let source = after
                .trim()
                .trim_start_matches(['\'', '"'])
                .trim_end_matches([';', '\'', '"'])
                .trim_end_matches(['\'', '"']);

            if source.starts_with("./") || source.starts_with("../") {
                let resolved = if dir.is_empty() {
                    source.trim_start_matches("./").to_string()
                } else {
                    format!("{dir}/{}", source.trim_start_matches("./"))
                };

                let candidates = vec![
                    format!("{resolved}.{ext}"),
                    format!("{resolved}/index.{ext}"),
                    resolved.clone(),
                ];

                for candidate in candidates {
                    if file_set.contains(candidate.as_str()) {
                        edges.push(IndexEdge {
                            from: file.to_string(),
                            to: candidate,
                            kind: "reexport".to_string(),
                            weight: 0.8,
                        });
                        break;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Layer 3: Co-Change Edges (weight 0.5)
// ---------------------------------------------------------------------------

fn build_cochange_edges(index: &mut ProjectIndex) {
    let project_root = &index.project_root;

    let output = match std::process::Command::new("git")
        .args([
            "log",
            "--name-only",
            "--pretty=format:---",
            "--since=6 months",
            "--",
            ".",
        ])
        .current_dir(project_root)
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return,
    };

    let file_set: std::collections::HashSet<&str> =
        index.files.keys().map(String::as_str).collect();

    let connected: std::collections::HashSet<&str> = index
        .edges
        .iter()
        .flat_map(|e| [e.from.as_str(), e.to.as_str()])
        .collect();

    // Parse commits into groups of files
    let mut cooccurrence: HashMap<(String, String), u32> = HashMap::new();
    let mut current_commit: Vec<&str> = Vec::new();

    for line in output.lines() {
        if line == "---" {
            if current_commit.len() >= 2 && current_commit.len() <= 20 {
                for i in 0..current_commit.len() {
                    for j in (i + 1)..current_commit.len() {
                        let a = current_commit[i];
                        let b = current_commit[j];
                        if !file_set.contains(a) || !file_set.contains(b) {
                            continue;
                        }
                        // Only add if at least one is currently isolated
                        if connected.contains(a) && connected.contains(b) {
                            continue;
                        }
                        let key = if a < b {
                            (a.to_string(), b.to_string())
                        } else {
                            (b.to_string(), a.to_string())
                        };
                        *cooccurrence.entry(key).or_insert(0) += 1;
                    }
                }
            }
            current_commit.clear();
        } else if !line.is_empty() {
            current_commit.push(line.trim());
        }
    }

    // Filter: min 5 shared commits
    let mut cochange_edges: Vec<IndexEdge> = cooccurrence
        .into_iter()
        .filter(|(_, count)| *count >= 5)
        .map(|((from, to), _)| IndexEdge {
            from,
            to,
            kind: "cochange".to_string(),
            weight: 0.5,
        })
        .collect();

    // Cap at 500 to prevent noise
    cochange_edges.sort_by(|a, b| a.from.cmp(&b.from).then_with(|| a.to.cmp(&b.to)));
    cochange_edges.truncate(500);

    index.edges.extend(cochange_edges);
}

// ---------------------------------------------------------------------------
// Layer 4: Sibling Edges (weight 0.2)
// ---------------------------------------------------------------------------

fn build_sibling_edges(index: &mut ProjectIndex) {
    let connected: std::collections::HashSet<&str> = index
        .edges
        .iter()
        .flat_map(|e| [e.from.as_str(), e.to.as_str()])
        .collect();

    let file_paths: Vec<String> = index.files.keys().cloned().collect();
    let mut new_edges: Vec<IndexEdge> = Vec::new();

    for file in &file_paths {
        if connected.contains(file.as_str()) {
            continue;
        }

        let ext = Path::new(file.as_str())
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let dir = Path::new(file.as_str())
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        // Find one sibling with same extension
        for other in &file_paths {
            if other == file {
                continue;
            }
            let other_ext = Path::new(other.as_str())
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let other_dir = Path::new(other.as_str())
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            if other_ext == ext && other_dir == dir {
                new_edges.push(IndexEdge {
                    from: file.clone(),
                    to: other.clone(),
                    kind: "sibling".to_string(),
                    weight: 0.2,
                });
                break; // Max 1 sibling edge per isolate
            }
        }
    }

    index.edges.extend(new_edges);
}

fn find_symbol_range(content: &str, sig: &signatures::Signature) -> (usize, usize) {
    let lines: Vec<&str> = content.lines().collect();
    let mut start = 0;

    for (i, line) in lines.iter().enumerate() {
        if line.contains(&sig.name) {
            let trimmed = line.trim();
            let is_def = trimmed.starts_with("fn ")
                || trimmed.starts_with("pub fn ")
                || trimmed.starts_with("pub(crate) fn ")
                || trimmed.starts_with("async fn ")
                || trimmed.starts_with("pub async fn ")
                || trimmed.starts_with("struct ")
                || trimmed.starts_with("pub struct ")
                || trimmed.starts_with("enum ")
                || trimmed.starts_with("pub enum ")
                || trimmed.starts_with("trait ")
                || trimmed.starts_with("pub trait ")
                || trimmed.starts_with("impl ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("export class ")
                || trimmed.starts_with("export function ")
                || trimmed.starts_with("export async function ")
                || trimmed.starts_with("function ")
                || trimmed.starts_with("async function ")
                || trimmed.starts_with("def ")
                || trimmed.starts_with("async def ")
                || trimmed.starts_with("func ")
                || trimmed.starts_with("interface ")
                || trimmed.starts_with("export interface ")
                || trimmed.starts_with("type ")
                || trimmed.starts_with("export type ")
                || trimmed.starts_with("const ")
                || trimmed.starts_with("export const ")
                || trimmed.starts_with("fun ")
                || trimmed.starts_with("private fun ")
                || trimmed.starts_with("public fun ")
                || trimmed.starts_with("internal fun ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("data class ")
                || trimmed.starts_with("sealed class ")
                || trimmed.starts_with("sealed interface ")
                || trimmed.starts_with("enum class ")
                || trimmed.starts_with("object ")
                || trimmed.starts_with("private object ")
                || trimmed.starts_with("interface ")
                || trimmed.starts_with("typealias ")
                || trimmed.starts_with("private typealias ");
            if is_def {
                start = i + 1;
                break;
            }
        }
    }

    if start == 0 {
        return (1, lines.len().min(20));
    }

    let base_indent = lines
        .get(start - 1)
        .map_or(0, |l| l.len() - l.trim_start().len());

    let mut end = start;
    let mut brace_depth: i32 = 0;
    let mut found_open = false;

    for (i, line) in lines.iter().enumerate().skip(start - 1) {
        for ch in line.chars() {
            if ch == '{' {
                brace_depth += 1;
                found_open = true;
            } else if ch == '}' {
                brace_depth -= 1;
            }
        }

        end = i + 1;

        if found_open && brace_depth <= 0 {
            break;
        }

        if !found_open && i > start {
            let indent = line.len() - line.trim_start().len();
            if indent <= base_indent && !line.trim().is_empty() && i > start {
                end = i;
                break;
            }
        }

        if end - start > 200 {
            break;
        }
    }

    (start, end)
}

fn extract_summary(content: &str) -> String {
    for line in content.lines().take(20) {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with('#')
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
            || trimmed.starts_with("use ")
            || trimmed.starts_with("import ")
            || trimmed.starts_with("from ")
            || trimmed.starts_with("require(")
            || trimmed.starts_with("package ")
        {
            continue;
        }
        return trimmed.chars().take(120).collect();
    }
    String::new()
}

fn compute_hash(content: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn short_hash(input: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{:08x}", hasher.finish() & 0xFFFF_FFFF)
}

fn copy_dir_fallible(src: &std::path::Path, dst: &std::path::Path) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_fallible(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn normalize_absolute_path(path: &str) -> String {
    if let Ok(canon) = crate::core::pathutil::safe_canonicalize(std::path::Path::new(path)) {
        return canon.to_string_lossy().to_string();
    }

    let mut normalized = path.to_string();
    while normalized.ends_with("\\.") || normalized.ends_with("/.") {
        normalized.truncate(normalized.len() - 2);
    }
    while normalized.len() > 1
        && (normalized.ends_with('\\') || normalized.ends_with('/'))
        && !normalized.ends_with(":\\")
        && !normalized.ends_with(":/")
        && normalized != "\\"
        && normalized != "/"
    {
        normalized.pop();
    }
    normalized
}

pub fn normalize_project_root(path: &str) -> String {
    normalize_absolute_path(path)
}

pub fn graph_match_key(path: &str) -> String {
    let stripped =
        crate::core::pathutil::strip_verbatim_str(path).unwrap_or_else(|| path.replace('\\', "/"));
    stripped.trim_start_matches('/').to_string()
}

pub fn graph_relative_key(path: &str, root: &str) -> String {
    let root_norm = normalize_project_root(root);
    let path_norm = normalize_absolute_path(path);
    let root_path = Path::new(&root_norm);
    let path_path = Path::new(&path_norm);

    if let Ok(rel) = path_path.strip_prefix(root_path) {
        let rel = rel.to_string_lossy().to_string();
        return rel.trim_start_matches(['/', '\\']).to_string();
    }

    path.trim_start_matches(['/', '\\'])
        .replace('/', std::path::MAIN_SEPARATOR_STR)
}

fn make_relative(path: &str, root: &str) -> String {
    graph_relative_key(path, root)
}

fn is_indexable_ext(ext: &str) -> bool {
    crate::core::language_capabilities::is_indexable_ext(ext)
}

#[cfg(test)]
fn kotlin_package_name(content: &str) -> Option<String> {
    content.lines().map(str::trim).find_map(|line| {
        line.strip_prefix("package ")
            .map(|rest| rest.trim().trim_end_matches(';').to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_short_hash_deterministic() {
        let h1 = short_hash("/Users/test/project");
        let h2 = short_hash("/Users/test/project");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 8);
    }

    #[test]
    fn test_make_relative() {
        assert_eq!(
            make_relative("/foo/bar/src/main.rs", "/foo/bar"),
            graph_relative_key("/foo/bar/src/main.rs", "/foo/bar")
        );
        assert_eq!(
            make_relative("src/main.rs", "/foo/bar"),
            graph_relative_key("src/main.rs", "/foo/bar")
        );
        assert_eq!(
            make_relative("C:\\repo\\src\\main\\kotlin\\Example.kt", "C:\\repo"),
            graph_relative_key("C:\\repo\\src\\main\\kotlin\\Example.kt", "C:\\repo")
        );
        assert_eq!(
            make_relative("//?/C:/repo/src/main/kotlin/Example.kt", "//?/C:/repo"),
            graph_relative_key("//?/C:/repo/src/main/kotlin/Example.kt", "//?/C:/repo")
        );
    }

    #[test]
    fn test_normalize_project_root() {
        assert_eq!(normalize_project_root("C:\\repo\\"), "C:\\repo");
        assert_eq!(normalize_project_root("C:\\repo\\."), "C:\\repo");
        assert_eq!(normalize_project_root("//?/C:/repo/"), "//?/C:/repo");
    }

    #[test]
    fn test_graph_match_key_normalizes_windows_forms() {
        assert_eq!(
            graph_match_key(r"C:\repo\src\main.rs"),
            "C:/repo/src/main.rs"
        );
        assert_eq!(
            graph_match_key(r"\\?\C:\repo\src\main.rs"),
            "C:/repo/src/main.rs"
        );
        assert_eq!(graph_match_key(r"\src\main.rs"), "src/main.rs");
    }

    #[test]
    fn test_extract_summary() {
        let content = "// comment\nuse std::io;\n\npub fn main() {\n    println!(\"hello\");\n}";
        let summary = extract_summary(content);
        assert_eq!(summary, "pub fn main() {");
    }

    #[test]
    fn test_compute_hash_deterministic() {
        let h1 = compute_hash("hello world");
        let h2 = compute_hash("hello world");
        assert_eq!(h1, h2);
        assert_ne!(h1, compute_hash("hello world!"));
    }

    #[test]
    fn test_project_index_new() {
        let idx = ProjectIndex::new("/test");
        assert_eq!(idx.version, INDEX_VERSION);
        assert_eq!(idx.project_root, "/test");
        assert!(idx.files.is_empty());
    }

    fn fe(path: &str, content: &str, language: &str) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            hash: compute_hash(content),
            language: language.to_string(),
            line_count: content.lines().count(),
            token_count: crate::core::tokens::count_tokens(content),
            exports: Vec::new(),
            summary: extract_summary(content),
        }
    }

    #[test]
    fn test_index_looks_stale_when_any_file_missing() {
        let td = tempdir().expect("tempdir");
        let root = td.path();
        std::fs::write(root.join("a.rs"), "pub fn a() {}\n").expect("write a.rs");

        let root_s = normalize_project_root(&root.to_string_lossy());
        let mut idx = ProjectIndex::new(&root_s);
        idx.files
            .insert("a.rs".to_string(), fe("a.rs", "pub fn a() {}\n", "rs"));
        idx.files.insert(
            "missing.rs".to_string(),
            fe("missing.rs", "pub fn m() {}\n", "rs"),
        );

        assert!(index_looks_stale(&idx, &root_s));
    }

    #[test]
    fn test_index_looks_fresh_when_all_files_exist() {
        let td = tempdir().expect("tempdir");
        let root = td.path();
        std::fs::write(root.join("a.rs"), "pub fn a() {}\n").expect("write a.rs");

        let root_s = normalize_project_root(&root.to_string_lossy());
        let mut idx = ProjectIndex::new(&root_s);
        idx.files
            .insert("a.rs".to_string(), fe("a.rs", "pub fn a() {}\n", "rs"));

        assert!(!index_looks_stale(&idx, &root_s));
    }

    #[test]
    fn test_reverse_deps() {
        let mut idx = ProjectIndex::new("/test");
        idx.edges.push(IndexEdge {
            from: "a.rs".to_string(),
            to: "b.rs".to_string(),
            kind: "import".to_string(),
            weight: 1.0,
        });
        idx.edges.push(IndexEdge {
            from: "c.rs".to_string(),
            to: "b.rs".to_string(),
            kind: "import".to_string(),
            weight: 1.0,
        });

        let deps = idx.get_reverse_deps("b.rs", 1);
        assert_eq!(deps.len(), 2);
        assert!(deps.contains(&"a.rs".to_string()));
        assert!(deps.contains(&"c.rs".to_string()));
    }

    #[test]
    fn test_find_symbol_range_kotlin_function() {
        let content = r#"
package com.example

class UserService {
    fun greet(name: String): String {
        return "hi $name"
    }
}
"#;
        let sig = signatures::Signature {
            kind: "method",
            name: "greet".to_string(),
            params: "name:String".to_string(),
            return_type: "String".to_string(),
            is_async: false,
            is_exported: true,
            indent: 2,
            ..signatures::Signature::no_span()
        };
        let (start, end) = find_symbol_range(content, &sig);
        assert_eq!(start, 5);
        assert!(end >= start);
    }

    #[test]
    fn test_signature_spans_override_fallback_range() {
        let sig = signatures::Signature {
            kind: "method",
            name: "release".to_string(),
            params: "id:String".to_string(),
            return_type: "Boolean".to_string(),
            is_async: true,
            is_exported: true,
            indent: 2,
            start_line: Some(42),
            end_line: Some(43),
        };

        let (start, end) = sig
            .start_line
            .zip(sig.end_line)
            .unwrap_or_else(|| find_symbol_range("ignored", &sig));
        assert_eq!((start, end), (42, 43));
    }

    #[test]
    fn test_parse_stale_index_version() {
        let json = format!(
            r#"{{"version":{},"project_root":"/test","last_scan":"now","files":{{}},"edges":[],"symbols":{{}}}}"#,
            INDEX_VERSION - 1
        );
        let parsed: ProjectIndex = serde_json::from_str(&json).unwrap();
        assert_ne!(parsed.version, INDEX_VERSION);
    }

    #[test]
    fn test_kotlin_package_name() {
        let content = "package com.example.feature\n\nclass UserService";
        assert_eq!(
            kotlin_package_name(content).as_deref(),
            Some("com.example.feature")
        );
    }

    #[test]
    fn safe_scan_root_rejects_fs_root() {
        assert!(!is_safe_scan_root("/"));
        assert!(!is_safe_scan_root("\\"));
        #[cfg(windows)]
        {
            assert!(!is_safe_scan_root("C:\\"));
            assert!(!is_safe_scan_root("D:\\"));
        }
    }

    #[test]
    fn safe_scan_root_rejects_home() {
        if let Some(home) = dirs::home_dir() {
            let home_str = home.to_string_lossy().to_string();
            assert!(
                !is_safe_scan_root(&home_str),
                "home dir should be rejected: {home_str}"
            );
        }
    }

    #[test]
    fn safe_scan_root_accepts_project_dir() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\n",
        )
        .unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        assert!(is_safe_scan_root(&root));
    }

    #[test]
    fn safe_scan_root_rejects_broad_dir() {
        let tmp = tempdir().unwrap();
        for i in 0..55 {
            std::fs::create_dir(tmp.path().join(format!("dir{i}"))).unwrap();
        }
        let root = tmp.path().to_string_lossy().to_string();
        assert!(!is_safe_scan_root(&root));
    }

    #[test]
    fn no_index_env_skips_scan() {
        let _env = crate::core::data_dir::test_env_lock();
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "").unwrap();
        std::fs::write(tmp.path().join("main.rs"), "fn main() {}").unwrap();

        std::env::set_var("LEAN_CTX_NO_INDEX", "1");
        let idx = scan(&tmp.path().to_string_lossy());
        std::env::remove_var("LEAN_CTX_NO_INDEX");
        assert!(idx.files.is_empty(), "LEAN_CTX_NO_INDEX should skip scan");
    }

    #[test]
    fn stale_index_detected_by_contamination() {
        let root_s = "/home/testuser/myproject";
        let mut idx = ProjectIndex::new(root_s);
        // Simulate a contaminated index with Desktop files
        idx.files.insert(
            "Desktop/random.py".to_string(),
            fe("Desktop/random.py", "x = 1\n", "py"),
        );
        idx.files.insert(
            "src/main.rs".to_string(),
            fe("src/main.rs", "fn main() {}\n", "rs"),
        );
        assert!(
            index_looks_stale(&idx, root_s),
            "Index with Desktop/ files should be considered stale"
        );
    }

    #[test]
    fn stale_index_detected_by_age() {
        let td = tempdir().expect("tempdir");
        let root = td.path();
        std::fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();

        let root_s = normalize_project_root(&root.to_string_lossy());
        let mut idx = ProjectIndex::new(&root_s);
        idx.files
            .insert("a.rs".to_string(), fe("a.rs", "fn a() {}\n", "rs"));
        // Set last_scan to 100 hours ago (default max_age_hours is 48)
        let old_time = chrono::Local::now().naive_local() - chrono::Duration::hours(100);
        idx.last_scan = old_time.format("%Y-%m-%d %H:%M:%S").to_string();

        assert!(
            index_looks_stale(&idx, &root_s),
            "Index older than max_age_hours should be stale"
        );
    }

    #[test]
    fn safe_scan_root_rejects_home_downloads() {
        if let Some(home) = dirs::home_dir() {
            let downloads = home.join("Downloads");
            // Only test if Downloads doesn't contain a .git (unlikely but possible)
            if !downloads.join(".git").exists() {
                let downloads_str = downloads.to_string_lossy().to_string();
                assert!(
                    !is_safe_scan_root(&downloads_str),
                    "~/Downloads should be rejected without project markers"
                );
            }
        }
    }
}
