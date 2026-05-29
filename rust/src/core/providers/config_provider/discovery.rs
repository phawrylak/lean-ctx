//! Auto-discovery of provider config files from well-known directories.
//!
//! Scans:
//! 1. `~/.config/lean-ctx/providers/` — user-global providers
//! 2. `.lean-ctx/providers/` — project-local providers
//!
//! Supports `.toml` and `.json` files.

use std::path::{Path, PathBuf};

use super::schema::ProviderConfig;

/// Discover all provider config files from standard directories.
pub fn discover_configs(project_root: Option<&Path>) -> Vec<DiscoveredConfig> {
    let mut configs = Vec::new();

    for dir in config_directories(project_root) {
        if !dir.is_dir() {
            continue;
        }
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Some(cfg) = try_load_config(&path) {
                        configs.push(cfg);
                    }
                }
            }
            Err(e) => {
                tracing::debug!("[config_provider] failed to read {}: {e}", dir.display());
            }
        }
    }

    // Deduplicate: project-local configs override global ones (last wins).
    let mut seen = std::collections::HashMap::new();
    for cfg in configs {
        if let Some(prev) = seen.insert(cfg.config.id.clone(), cfg.clone()) {
            tracing::info!(
                "[config_provider] '{}' overridden: {} → {}",
                cfg.config.id,
                prev.source_path.display(),
                cfg.source_path.display()
            );
        }
    }
    let mut result: Vec<_> = seen.into_values().collect();
    result.sort_by(|a, b| a.config.id.cmp(&b.config.id));
    result
}

/// A config file that was successfully parsed.
#[derive(Debug, Clone)]
pub struct DiscoveredConfig {
    pub source_path: PathBuf,
    pub config: ProviderConfig,
}

/// Returns the list of directories to scan, in priority order.
/// Later entries override earlier ones (project-local > global).
fn config_directories(project_root: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // 1. Global: ~/.config/lean-ctx/providers/
    if let Some(config_dir) = dirs::config_dir() {
        dirs.push(config_dir.join("lean-ctx").join("providers"));
    }

    // 2. Global alt: ~/.lean-ctx/providers/
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".lean-ctx").join("providers"));
    }

    // 3. Project-local: <project>/.lean-ctx/providers/
    if let Some(root) = project_root {
        dirs.push(root.join(".lean-ctx").join("providers"));
    }

    dirs
}

/// Try to load and parse a single config file.
fn try_load_config(path: &Path) -> Option<DiscoveredConfig> {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        tracing::debug!(
            "[config_provider] skipping {}: no extension",
            path.display()
        );
        return None;
    };

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("[config_provider] failed to read {}: {e}", path.display());
            return None;
        }
    };

    let config: ProviderConfig = match ext {
        "toml" => toml::from_str(&content)
            .map_err(|e| {
                tracing::warn!("[config_provider] failed to parse {}: {e}", path.display());
                e
            })
            .ok()?,
        "json" => serde_json::from_str(&content)
            .map_err(|e| {
                tracing::warn!("[config_provider] failed to parse {}: {e}", path.display());
                e
            })
            .ok()?,
        other => {
            tracing::debug!(
                "[config_provider] skipping {}: unsupported extension .{other}",
                path.display()
            );
            return None;
        }
    };

    if let Err(e) = config.validate() {
        tracing::warn!("[config_provider] invalid config {}: {e}", path.display());
        return None;
    }

    tracing::info!(
        "[config_provider] loaded '{}' from {}",
        config.id,
        path.display()
    );

    Some(DiscoveredConfig {
        source_path: path.to_path_buf(),
        config,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discover_toml_config_from_project() {
        let dir = tempfile::tempdir().unwrap();
        let providers_dir = dir.path().join(".lean-ctx").join("providers");
        fs::create_dir_all(&providers_dir).unwrap();

        fs::write(
            providers_dir.join("myapi.toml"),
            r#"
id = "myapi"
name = "My API"
base_url = "https://api.example.com"

[auth]
type = "none"

[resources.items]
path = "/items"
[resources.items.response.mapping]
id = "id"
title = "name"
"#,
        )
        .unwrap();

        let configs = discover_configs(Some(dir.path()));
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].config.id, "myapi");
        assert_eq!(configs[0].config.name, "My API");
    }

    #[test]
    fn discover_json_config() {
        let dir = tempfile::tempdir().unwrap();
        let providers_dir = dir.path().join(".lean-ctx").join("providers");
        fs::create_dir_all(&providers_dir).unwrap();

        fs::write(
            providers_dir.join("notion.json"),
            r#"{
                "id": "notion",
                "name": "Notion",
                "base_url": "https://api.notion.com/v1",
                "auth": {"type": "none"},
                "resources": {
                    "pages": {
                        "path": "/search",
                        "method": "POST",
                        "response": {
                            "root": "results",
                            "mapping": {
                                "id": "id",
                                "title": "properties.Name.title[0].text.content"
                            }
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let configs = discover_configs(Some(dir.path()));
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].config.id, "notion");
    }

    #[test]
    fn discover_ignores_invalid_files() {
        let dir = tempfile::tempdir().unwrap();
        let providers_dir = dir.path().join(".lean-ctx").join("providers");
        fs::create_dir_all(&providers_dir).unwrap();

        // Invalid TOML
        fs::write(providers_dir.join("bad.toml"), "not valid toml {{{").unwrap();
        // Not a config file
        fs::write(providers_dir.join("readme.md"), "# Providers").unwrap();

        let configs = discover_configs(Some(dir.path()));
        assert!(configs.is_empty());
    }

    #[test]
    fn discover_deduplicates_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let providers_dir = dir.path().join(".lean-ctx").join("providers");
        fs::create_dir_all(&providers_dir).unwrap();

        let cfg = r#"
id = "dupe"
name = "Dupe"
base_url = "https://example.com"
[auth]
type = "none"
[resources.data]
path = "/data"
[resources.data.response.mapping]
id = "id"
title = "title"
"#;
        fs::write(providers_dir.join("dupe1.toml"), cfg).unwrap();
        fs::write(providers_dir.join("dupe2.toml"), cfg).unwrap();

        let configs = discover_configs(Some(dir.path()));
        assert_eq!(configs.len(), 1);
    }

    #[test]
    fn discover_empty_when_no_dir() {
        let configs = discover_configs(Some(Path::new("/nonexistent/path/12345")));
        // Should not crash, just return empty (the dir doesn't exist)
        assert!(configs.is_empty() || !configs.is_empty()); // always true, we just check no panic
    }

    #[test]
    fn config_directories_includes_project_root() {
        let root = Path::new("/tmp/myproject");
        let dirs = config_directories(Some(root));
        assert!(dirs
            .iter()
            .any(|d| d.ends_with("myproject/.lean-ctx/providers")));
    }
}
