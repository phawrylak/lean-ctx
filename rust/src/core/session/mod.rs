mod compaction;
mod heuristics;
mod paths;
mod persistence;
pub mod playbook;
mod state;
mod types;

pub use playbook::{DeltaOutcome, EntryKind, Playbook, PlaybookEntry};
pub use types::{
    Decision, EvidenceKind, EvidenceRecord, FileTouched, Finding, ManifestEntry, PreparedSave,
    ProgressEntry, SessionState, SessionStats, SessionSummary, TaskInfo, TestSnapshot,
};

#[cfg(test)]
mod tests {
    use super::paths::extract_cd_target;
    use super::types::*;

    #[test]
    fn extract_cd_absolute_path() {
        let result = extract_cd_target("cd /usr/local/bin", "/home/user");
        assert_eq!(result, Some("/usr/local/bin".to_string()));
    }

    #[test]
    fn extract_cd_relative_path() {
        let result = extract_cd_target("cd subdir", "/home/user");
        assert_eq!(result, Some("/home/user/subdir".to_string()));
    }

    #[test]
    fn extract_cd_with_chained_command() {
        let result = extract_cd_target("cd /tmp && ls", "/home/user");
        assert_eq!(result, Some("/tmp".to_string()));
    }

    #[test]
    fn extract_cd_with_semicolon() {
        let result = extract_cd_target("cd /tmp; ls", "/home/user");
        assert_eq!(result, Some("/tmp".to_string()));
    }

    #[test]
    fn extract_cd_parent_dir() {
        let result = extract_cd_target("cd ..", "/home/user/project");
        assert_eq!(result, Some("/home/user/project/..".to_string()));
    }

    #[test]
    fn extract_cd_no_cd_returns_none() {
        let result = extract_cd_target("ls -la", "/home/user");
        assert!(result.is_none());
    }

    #[test]
    fn extract_cd_bare_cd_goes_home() {
        let result = extract_cd_target("cd", "/home/user");
        assert!(result.is_some());
    }

    #[test]
    fn effective_cwd_explicit_takes_priority() {
        let tmp = std::env::temp_dir().join("lean-ctx-test-cwd-explicit");
        let sub = tmp.join("sub");
        let _ = std::fs::create_dir_all(&sub);
        let root_canon = crate::core::pathutil::safe_canonicalize_or_self(&tmp)
            .to_string_lossy()
            .to_string();
        let sub_canon = crate::core::pathutil::safe_canonicalize_or_self(&sub)
            .to_string_lossy()
            .to_string();

        let mut session = SessionState::new();
        session.project_root = Some(root_canon);
        let result = session.effective_cwd(Some(&sub_canon));
        assert_eq!(result, sub_canon);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(not(feature = "no-jail"))]
    #[test]
    fn effective_cwd_explicit_outside_root_is_jailed() {
        let tmp = std::env::temp_dir().join("lean-ctx-test-cwd-jail");
        let _ = std::fs::create_dir_all(&tmp);
        let root_canon = crate::core::pathutil::safe_canonicalize_or_self(&tmp)
            .to_string_lossy()
            .to_string();

        let mut session = SessionState::new();
        session.project_root = Some(root_canon.clone());
        let result = session.effective_cwd(Some("/nonexistent-outside-path"));
        assert_eq!(result, root_canon);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn effective_cwd_shell_cwd_second_priority() {
        let mut session = SessionState::new();
        session.project_root = Some("/project".to_string());
        session.shell_cwd = Some("/project/src".to_string());
        assert_eq!(session.effective_cwd(None), "/project/src");
    }

    #[test]
    fn effective_cwd_project_root_third_priority() {
        let mut session = SessionState::new();
        session.project_root = Some("/project".to_string());
        assert_eq!(session.effective_cwd(None), "/project");
    }

    #[test]
    fn effective_cwd_dot_ignored() {
        let mut session = SessionState::new();
        session.project_root = Some("/project".to_string());
        assert_eq!(session.effective_cwd(Some(".")), "/project");
    }

    #[test]
    fn compaction_snapshot_includes_compression_config_when_enabled() {
        let mut session = SessionState::new();
        session.compression_level = "standard".to_string();
        session.terse_mode = true;
        session.set_task("x", None);
        let snapshot = session.build_compaction_snapshot();
        assert!(snapshot.contains("<config compression=\"standard\" />"));
    }

    #[test]
    fn resume_block_prefixes_compression_hint_when_enabled() {
        let mut session = SessionState::new();
        session.compression_level = "lite".to_string();
        session.terse_mode = true;
        let block = session.build_resume_block();
        assert!(block.contains("[COMPRESSION: lite]"));
    }

    #[test]
    fn compaction_snapshot_includes_task() {
        let mut session = SessionState::new();
        session.set_task("fix auth bug", None);
        let snapshot = session.build_compaction_snapshot();
        assert!(snapshot.contains("<task>fix auth bug</task>"));
        assert!(snapshot.contains("<session_snapshot>"));
        assert!(snapshot.contains("</session_snapshot>"));
    }

    #[test]
    fn compaction_snapshot_includes_files() {
        let mut session = SessionState::new();
        session.touch_file("src/auth.rs", None, "full", 500);
        session.files_touched[0].modified = true;
        session.touch_file("src/main.rs", None, "map", 100);
        let snapshot = session.build_compaction_snapshot();
        assert!(snapshot.contains("auth.rs"));
        assert!(snapshot.contains("<files>"));
    }

    #[test]
    fn compaction_snapshot_includes_decisions() {
        let mut session = SessionState::new();
        session.add_decision("Use JWT RS256", None);
        let snapshot = session.build_compaction_snapshot();
        assert!(snapshot.contains("JWT RS256"));
        assert!(snapshot.contains("<decisions>"));
    }

    #[test]
    fn compaction_snapshot_respects_size_limit() {
        let mut session = SessionState::new();
        session.set_task("a]task", None);
        for i in 0..100 {
            session.add_finding(
                Some(&format!("file{i}.rs")),
                Some(i),
                &format!("Finding number {i} with some detail text here"),
            );
        }
        let snapshot = session.build_compaction_snapshot();
        assert!(snapshot.len() <= 2200);
    }

    #[test]
    fn compaction_snapshot_includes_stats() {
        let mut session = SessionState::new();
        session.stats.total_tool_calls = 42;
        session.stats.total_tokens_saved = 10000;
        let snapshot = session.build_compaction_snapshot();
        assert!(snapshot.contains("calls=42"));
        assert!(snapshot.contains("saved=10000"));
    }
}
