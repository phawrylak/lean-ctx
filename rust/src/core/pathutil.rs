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

/// Canonicalize with a timeout guard. On Windows, `std::fs::canonicalize` can hang
/// indefinitely on cloud-synced paths, reparse points, or network drives.
/// Falls back to the original path if canonicalize doesn't complete within the timeout.
pub fn safe_canonicalize_bounded(path: &Path, timeout_ms: u64) -> PathBuf {
    #[cfg(windows)]
    {
        let path_owned = path.to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("canonicalize-bounded".into())
            .spawn(move || {
                let result = safe_canonicalize(&path_owned).unwrap_or_else(|_| path_owned);
                let _ = tx.send(result);
            });
        match rx.recv_timeout(std::time::Duration::from_millis(timeout_ms)) {
            Ok(canonical) => canonical,
            Err(_) => {
                tracing::debug!(
                    "canonicalize timed out ({}ms) for {}; using original path",
                    timeout_ms,
                    path.display()
                );
                path.to_path_buf()
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = timeout_ms;
        safe_canonicalize_or_self(path)
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
}
