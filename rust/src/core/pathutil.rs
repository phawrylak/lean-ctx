use std::path::{Path, PathBuf};

/// Canonicalize a path and strip the Windows verbatim/extended-length prefix (`\\?\`)
/// that `std::fs::canonicalize` adds on Windows. This prefix breaks many tools and
/// string-based path comparisons.
///
/// On non-Windows platforms this is equivalent to `std::fs::canonicalize`.
pub fn safe_canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    let canon = std::fs::canonicalize(path)?;
    Ok(strip_verbatim(canon))
}

/// Like `safe_canonicalize` but returns the original path on failure.
pub fn safe_canonicalize_or_self(path: &Path) -> PathBuf {
    safe_canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Canonicalize with a timeout guard. Protects against hangs on WSL2 DrvFS,
/// Windows reparse points, NFS, FUSE, sshfs, and other slow filesystems.
/// Falls back to the original path if canonicalize doesn't complete within the timeout.
/// Self-healing: after a timeout, subsequent calls to slow mounts skip the thread entirely.
pub fn safe_canonicalize_bounded(path: &Path, timeout_ms: u64) -> PathBuf {
    use super::io_health;

    let path_str = path.to_string_lossy();
    if io_health::is_slow_mount(&path_str) && io_health::recent_freeze_count() > 0 {
        return safe_canonicalize_or_self(path);
    }

    let effective_timeout =
        io_health::adaptive_timeout(std::time::Duration::from_millis(timeout_ms));

    let path_owned = path.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("canonicalize-bounded".into())
        .spawn(move || {
            let result = safe_canonicalize(&path_owned).unwrap_or(path_owned);
            let _ = tx.send(result);
        });
    if let Ok(canonical) = rx.recv_timeout(effective_timeout) {
        canonical
    } else {
        io_health::record_freeze();
        tracing::warn!(
            "[SECURITY] canonicalize timed out ({}ms) for {}; PathJail checks on \
             uncanonicalized paths may be less reliable",
            effective_timeout.as_millis(),
            path.display()
        );
        path.to_path_buf()
    }
}

/// Remove the `\\?\` / `//?/` verbatim prefix from a `PathBuf`.
/// Handles both regular verbatim (`\\?\C:\...`) and UNC verbatim (`\\?\UNC\...`).
pub fn strip_verbatim(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = strip_verbatim_str(&s) {
        PathBuf::from(stripped)
    } else {
        path
    }
}

/// Remove the `\\?\` / `//?/` verbatim prefix from a path string.
/// Returns `Some(cleaned)` if a prefix was found, `None` otherwise.
pub fn strip_verbatim_str(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");

    if let Some(rest) = normalized.strip_prefix("//?/UNC/") {
        Some(format!("//{rest}"))
    } else {
        normalized
            .strip_prefix("//?/")
            .map(std::string::ToString::to_string)
    }
}

/// Normalize paths from any client format to a consistent OS-native form.
/// Handles MSYS2/Git Bash (`/c/Users/...` -> `C:/Users/...`), mixed separators,
/// double slashes, and trailing slashes. Uses forward slashes for consistency.
pub fn normalize_tool_path(path: &str) -> String {
    let mut p = match strip_verbatim_str(path) {
        Some(stripped) => stripped,
        None => path.to_string(),
    };

    // MSYS2/Git Bash: /c/Users/... -> C:/Users/...
    if p.len() >= 3
        && p.starts_with('/')
        && p.as_bytes()[1].is_ascii_alphabetic()
        && p.as_bytes()[2] == b'/'
    {
        let drive = p.as_bytes()[1].to_ascii_uppercase() as char;
        p = format!("{drive}:{}", &p[2..]);
    }

    p = p.replace('\\', "/");

    // Collapse double slashes (preserve UNC paths starting with //)
    while p.contains("//") && !p.starts_with("//") {
        p = p.replace("//", "/");
    }

    // Remove trailing slash (unless root like "/" or "C:/")
    if p.len() > 1 && p.ends_with('/') && !p.ends_with(":/") {
        p.pop();
    }

    p
}

/// Returns `true` if the directory is too broad to be a valid project root.
/// Rejects home directory, filesystem root, `.` (bare CWD), and agent sandbox
/// directories (`.claude`, `.codex`). Used to prevent writing project-scoped
/// data (overlays, policies) into the global `~/.lean-ctx/` data directory.
pub fn is_broad_or_unsafe_root(dir: &Path) -> bool {
    if let Some(home) = dirs::home_dir() {
        if dir == home {
            return true;
        }
    }
    let s = dir.to_string_lossy();
    if s == "/" || s == "\\" || s == "." {
        return true;
    }
    s.ends_with("/.claude")
        || s.ends_with("/.codex")
        || s.contains("/.claude/")
        || s.contains("/.codex/")
}

/// Returns `true` if `project_root` collides with the lean-ctx data directory.
/// This prevents project-scoped files (overlays.json, policies.json) from being
/// written into `~/.lean-ctx/` or `~/.config/lean-ctx/`.
pub fn is_data_dir_collision(project_root: &Path) -> bool {
    if is_broad_or_unsafe_root(project_root) {
        return true;
    }
    if let Ok(data_dir) = crate::core::data_dir::lean_ctx_data_dir() {
        let project_lean_ctx = project_root.join(".lean-ctx");
        if project_lean_ctx == data_dir || data_dir.starts_with(&project_lean_ctx) {
            return true;
        }
    }
    false
}

/// Returns the project-scoped `.lean-ctx/` directory if the project root is safe.
/// Returns `Err` if the project root collides with the global data directory.
pub fn safe_project_data_dir(project_root: &Path) -> Result<PathBuf, String> {
    if is_data_dir_collision(project_root) {
        return Err(format!(
            "project root {} collides with global data directory; \
             skipping project-scoped write",
            project_root.display()
        ));
    }
    Ok(project_root.join(".lean-ctx"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_regular_verbatim() {
        let p = PathBuf::from(r"\\?\C:\Users\dev\project");
        let result = strip_verbatim(p);
        assert_eq!(result, PathBuf::from("C:/Users/dev/project"));
    }

    #[test]
    fn strip_unc_verbatim() {
        let p = PathBuf::from(r"\\?\UNC\server\share\dir");
        let result = strip_verbatim(p);
        assert_eq!(result, PathBuf::from("//server/share/dir"));
    }

    #[test]
    fn no_prefix_unchanged() {
        let p = PathBuf::from("/home/user/project");
        let result = strip_verbatim(p.clone());
        assert_eq!(result, p);
    }

    #[test]
    fn windows_drive_unchanged() {
        let p = PathBuf::from("C:/Users/dev");
        let result = strip_verbatim(p.clone());
        assert_eq!(result, p);
    }

    #[test]
    fn strip_str_regular() {
        assert_eq!(
            strip_verbatim_str(r"\\?\E:\code\lean-ctx"),
            Some("E:/code/lean-ctx".to_string())
        );
    }

    #[test]
    fn strip_str_unc() {
        assert_eq!(
            strip_verbatim_str(r"\\?\UNC\myserver\data"),
            Some("//myserver/data".to_string())
        );
    }

    #[test]
    fn strip_str_forward_slash_variant() {
        assert_eq!(
            strip_verbatim_str("//?/C:/Users/dev"),
            Some("C:/Users/dev".to_string())
        );
    }

    #[test]
    fn strip_str_no_prefix() {
        assert_eq!(strip_verbatim_str("/home/user"), None);
    }

    #[test]
    fn safe_canonicalize_or_self_nonexistent() {
        let p = Path::new("/this/path/should/not/exist/xyzzy");
        let result = safe_canonicalize_or_self(p);
        assert_eq!(result, p.to_path_buf());
    }

    #[test]
    fn normalize_msys_path_to_native() {
        assert_eq!(
            normalize_tool_path("/c/Users/ABC/AppData/lean-ctx"),
            "C:/Users/ABC/AppData/lean-ctx"
        );
    }

    #[test]
    fn normalize_msys_uppercase_drive() {
        assert_eq!(
            normalize_tool_path("/D/Program Files/lean-ctx.exe"),
            "D:/Program Files/lean-ctx.exe"
        );
    }

    #[test]
    fn normalize_native_windows_path_unchanged() {
        assert_eq!(
            normalize_tool_path("C:/Users/ABC/lean-ctx.exe"),
            "C:/Users/ABC/lean-ctx.exe"
        );
    }

    #[test]
    fn normalize_backslash_windows_path() {
        assert_eq!(
            normalize_tool_path(r"C:\Users\ABC\lean-ctx.exe"),
            "C:/Users/ABC/lean-ctx.exe"
        );
    }

    #[test]
    fn normalize_unix_path_unchanged() {
        assert_eq!(
            normalize_tool_path("/usr/local/bin/lean-ctx"),
            "/usr/local/bin/lean-ctx"
        );
    }

    #[test]
    fn normalize_windows_path_with_spaces_and_backslashes() {
        // The exact "paths with spaces" scenario reported on Windows (#324):
        // backslashes are converted to forward slashes (so client render layers
        // never escape-mangle them) while spaces in directory names survive.
        assert_eq!(
            normalize_tool_path(r"C:\Users\My Name\My Project\src\main.rs"),
            "C:/Users/My Name/My Project/src/main.rs"
        );
        assert_eq!(
            normalize_tool_path(r"C:\Program Files\app\config.toml"),
            "C:/Program Files/app/config.toml"
        );
    }

    #[test]
    fn normalize_double_slashes() {
        assert_eq!(
            normalize_tool_path("C:/Users//ABC//lean-ctx"),
            "C:/Users/ABC/lean-ctx"
        );
    }

    #[test]
    fn normalize_trailing_slash_removed() {
        assert_eq!(normalize_tool_path("/c/Users/ABC/"), "C:/Users/ABC");
    }

    #[test]
    fn normalize_root_slash_preserved() {
        assert_eq!(normalize_tool_path("/"), "/");
    }

    #[test]
    fn normalize_drive_root_preserved() {
        assert_eq!(normalize_tool_path("C:/"), "C:/");
    }

    #[test]
    fn normalize_verbatim_with_msys() {
        assert_eq!(normalize_tool_path(r"\\?\C:\Users\dev"), "C:/Users/dev");
    }

    #[test]
    fn broad_root_rejects_home() {
        if let Some(home) = dirs::home_dir() {
            assert!(is_broad_or_unsafe_root(&home));
        }
    }

    #[test]
    fn broad_root_rejects_filesystem_root() {
        assert!(is_broad_or_unsafe_root(Path::new("/")));
    }

    #[test]
    fn broad_root_rejects_dot() {
        assert!(is_broad_or_unsafe_root(Path::new(".")));
    }

    #[test]
    fn broad_root_rejects_agent_dirs() {
        assert!(is_broad_or_unsafe_root(Path::new("/home/user/.claude")));
        assert!(is_broad_or_unsafe_root(Path::new("/home/user/.codex")));
    }

    #[test]
    fn broad_root_allows_project_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let subdir = tmp.path().join("my-project");
        std::fs::create_dir_all(&subdir).unwrap();
        assert!(!is_broad_or_unsafe_root(&subdir));
    }

    #[test]
    fn broad_root_allows_home_subdirs() {
        if let Some(home) = dirs::home_dir() {
            let subdir = home.join("projects").join("my-app");
            assert!(!is_broad_or_unsafe_root(&subdir));
        }
    }

    #[test]
    fn data_dir_collision_rejects_home() {
        if let Some(home) = dirs::home_dir() {
            assert!(is_data_dir_collision(&home));
        }
    }

    #[test]
    fn data_dir_collision_allows_normal_project() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("my-project");
        std::fs::create_dir_all(&project).unwrap();
        assert!(!is_data_dir_collision(&project));
    }
}
