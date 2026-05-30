pub mod executor;
pub mod manifest;
pub mod registry;

use executor::{execute_hooks_for_point, HookPoint, HookResult};
use registry::PluginRegistry;
use std::sync::Mutex;
use std::sync::OnceLock;

static GLOBAL_REGISTRY: OnceLock<Mutex<PluginRegistry>> = OnceLock::new();

pub struct PluginManager;

impl PluginManager {
    pub fn init() {
        let _ = GLOBAL_REGISTRY.get_or_init(|| {
            let mut reg = PluginRegistry::from_default_dir();
            let errors = reg.discover();
            for err in &errors {
                tracing::warn!(
                    "plugin discovery error at {}: {}",
                    err.path.display(),
                    err.error
                );
            }
            Mutex::new(reg)
        });
    }

    pub fn with_registry<F, R>(f: F) -> Option<R>
    where
        F: FnOnce(&PluginRegistry) -> R,
    {
        GLOBAL_REGISTRY
            .get()
            .and_then(|m| m.lock().ok())
            .map(|reg| f(&reg))
    }

    pub fn with_registry_mut<F, R>(f: F) -> Option<R>
    where
        F: FnOnce(&mut PluginRegistry) -> R,
    {
        GLOBAL_REGISTRY
            .get()
            .and_then(|m| m.lock().ok())
            .map(|mut reg| f(&mut reg))
    }

    pub fn fire_hook(hook: &HookPoint) -> Vec<HookResult> {
        Self::with_registry(|reg| {
            let plugins: Vec<_> = reg.enabled_plugins();
            execute_hooks_for_point(&plugins, hook)
        })
        .unwrap_or_default()
    }

    pub fn fire_hook_background(hook: HookPoint) {
        std::thread::spawn(move || {
            let results = Self::fire_hook(&hook);
            for r in &results {
                if !r.success {
                    tracing::warn!(
                        "plugin hook failed: {} - {}",
                        r.plugin_name,
                        r.error.as_deref().unwrap_or("unknown")
                    );
                }
            }
        });
    }
}

pub fn init_plugin_template(name: &str, dir: &std::path::Path) -> std::io::Result<()> {
    let plugin_dir = dir.join(name);
    std::fs::create_dir_all(&plugin_dir)?;

    let manifest = format!(
        r#"[plugin]
name = "{name}"
version = "0.1.0"
description = "Description of what this plugin does"
author = "Your Name"

[hooks.on_session_start]
command = "{name} start"
timeout_ms = 5000

[hooks.on_session_end]
command = "{name} stop"

# [hooks.pre_read]
# command = "{name} pre-read"
# timeout_ms = 2000

# [hooks.post_compress]
# command = "{name} post-compress"

# [hooks.on_knowledge_update]
# command = "{name} knowledge-updated"
"#
    );

    std::fs::write(plugin_dir.join("plugin.toml"), manifest)?;

    let readme = format!(
        "# {name}\n\n\
         A lean-ctx plugin.\n\n\
         ## Installation\n\n\
         Copy this directory to `~/.config/lean-ctx/plugins/{name}/`\n\n\
         ## Hook Points\n\n\
         - `on_session_start` — Called when a new session begins\n\
         - `on_session_end` — Called when a session ends\n\
         - `pre_read` — Called before a file is read (receives path via stdin JSON)\n\
         - `post_compress` — Called after compression (receives stats via stdin JSON)\n\
         - `on_knowledge_update` — Called when knowledge is updated (receives fact_id via stdin JSON)\n\n\
         ## Protocol\n\n\
         Hook data is passed as JSON via stdin. Your command should:\n\
         1. Read JSON from stdin\n\
         2. Process the hook\n\
         3. Write optional JSON response to stdout\n\
         4. Exit with code 0 on success, non-zero on failure\n"
    );

    std::fs::write(plugin_dir.join("README.md"), readme)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_template_creates_files() {
        let dir = tempfile::tempdir().unwrap();
        init_plugin_template("test-plugin", dir.path()).unwrap();
        let plugin_dir = dir.path().join("test-plugin");
        assert!(plugin_dir.join("plugin.toml").exists());
        assert!(plugin_dir.join("README.md").exists());

        let manifest = manifest::PluginManifest::from_file(&plugin_dir.join("plugin.toml"));
        assert!(manifest.is_ok());
        let m = manifest.unwrap();
        assert_eq!(m.plugin.name, "test-plugin");
    }

    #[test]
    fn fire_hook_with_no_plugins_returns_empty() {
        let results = PluginManager::fire_hook(&HookPoint::OnSessionStart);
        assert!(results.is_empty());
    }
}
