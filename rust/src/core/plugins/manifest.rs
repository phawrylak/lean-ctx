use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct PluginManifest {
    pub plugin: PluginMeta,
    #[serde(default)]
    pub hooks: HashMap<String, HookEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginMeta {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HookEntry {
    pub command: String,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_timeout_ms() -> u64 {
    5000
}

impl PluginManifest {
    pub fn from_file(path: &Path) -> Result<Self, ManifestError> {
        let content = std::fs::read_to_string(path).map_err(|e| ManifestError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        Self::from_str(&content, path)
    }

    pub fn from_str(content: &str, path: &Path) -> Result<Self, ManifestError> {
        let manifest: Self = toml::from_str(content).map_err(|e| ManifestError::Parse {
            path: path.to_path_buf(),
            source: e,
        })?;
        manifest.validate(path)?;
        Ok(manifest)
    }

    fn validate(&self, path: &Path) -> Result<(), ManifestError> {
        if self.plugin.name.is_empty() {
            return Err(ManifestError::Validation {
                path: path.to_path_buf(),
                field: "plugin.name".to_string(),
                reason: "must not be empty".to_string(),
            });
        }
        if self.plugin.version.is_empty() {
            return Err(ManifestError::Validation {
                path: path.to_path_buf(),
                field: "plugin.version".to_string(),
                reason: "must not be empty".to_string(),
            });
        }
        for (hook_name, entry) in &self.hooks {
            if entry.command.is_empty() {
                return Err(ManifestError::Validation {
                    path: path.to_path_buf(),
                    field: format!("hooks.{hook_name}.command"),
                    reason: "must not be empty".to_string(),
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("failed to read plugin manifest at {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse plugin manifest at {path}: {source}")]
    Parse {
        path: std::path::PathBuf,
        source: toml::de::Error,
    },
    #[error("invalid plugin manifest at {path}: {field} {reason}")]
    Validation {
        path: std::path::PathBuf,
        field: String,
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_valid_manifest() {
        let toml = r#"
[plugin]
name = "test-plugin"
version = "0.1.0"
description = "A test plugin"
author = "Test Author"

[hooks.on_session_start]
command = "test-binary start"
timeout_ms = 3000

[hooks.pre_read]
command = "test-binary pre-read"
"#;
        let manifest = PluginManifest::from_str(toml, &PathBuf::from("test.toml")).unwrap();
        assert_eq!(manifest.plugin.name, "test-plugin");
        assert_eq!(manifest.plugin.version, "0.1.0");
        assert_eq!(manifest.hooks.len(), 2);
        assert_eq!(manifest.hooks["on_session_start"].timeout_ms, 3000);
        assert_eq!(manifest.hooks["pre_read"].timeout_ms, 5000);
    }

    #[test]
    fn reject_empty_name() {
        let toml = r#"
[plugin]
name = ""
version = "0.1.0"
"#;
        let err = PluginManifest::from_str(toml, &PathBuf::from("bad.toml")).unwrap_err();
        assert!(err.to_string().contains("plugin.name"));
    }

    #[test]
    fn reject_empty_version() {
        let toml = r#"
[plugin]
name = "test"
version = ""
"#;
        let err = PluginManifest::from_str(toml, &PathBuf::from("bad.toml")).unwrap_err();
        assert!(err.to_string().contains("plugin.version"));
    }

    #[test]
    fn reject_empty_command() {
        let toml = r#"
[plugin]
name = "test"
version = "0.1.0"

[hooks.pre_read]
command = ""
"#;
        let err = PluginManifest::from_str(toml, &PathBuf::from("bad.toml")).unwrap_err();
        assert!(err.to_string().contains("hooks.pre_read.command"));
    }

    #[test]
    fn minimal_manifest_no_hooks() {
        let toml = r#"
[plugin]
name = "minimal"
version = "1.0.0"
"#;
        let manifest = PluginManifest::from_str(toml, &PathBuf::from("minimal.toml")).unwrap();
        assert_eq!(manifest.plugin.name, "minimal");
        assert!(manifest.hooks.is_empty());
    }

    #[test]
    fn default_timeout_applied() {
        let toml = r#"
[plugin]
name = "defaults"
version = "0.1.0"

[hooks.on_session_end]
command = "plugin-bin stop"
"#;
        let manifest = PluginManifest::from_str(toml, &PathBuf::from("test.toml")).unwrap();
        assert_eq!(manifest.hooks["on_session_end"].timeout_ms, 5000);
    }
}
