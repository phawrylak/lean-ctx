use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncReport {
    pub synced: Vec<String>,
    pub skipped: Vec<String>,
    pub errors: Vec<String>,
}

pub fn sync_all(home: &Path) -> SyncReport {
    let inject_result = crate::rules_inject::inject_all_rules(home);

    let mut synced = Vec::new();
    synced.extend(inject_result.injected.iter().cloned());
    synced.extend(inject_result.updated.iter().cloned());

    SyncReport {
        synced,
        skipped: inject_result.already,
        errors: inject_result.errors,
    }
}

pub fn sync_agent(home: &Path, agent: &str) -> SyncReport {
    let inject_result = crate::rules_inject::inject_rules_for_agent(home, agent);

    let mut synced = Vec::new();
    synced.extend(inject_result.injected.iter().cloned());
    synced.extend(inject_result.updated.iter().cloned());

    SyncReport {
        synced,
        skipped: inject_result.already,
        errors: inject_result.errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_nonexistent_home() {
        let home = Path::new("/tmp/nonexistent_sync_test_home");
        let report = sync_all(home);
        assert!(report.synced.is_empty());
        assert!(report.errors.is_empty());
    }

    #[test]
    fn sync_agent_unknown() {
        let home = Path::new("/tmp/nonexistent_sync_test_home");
        let report = sync_agent(home, "unknown_xyz");
        assert!(report.synced.is_empty());
        assert!(report.errors.is_empty());
    }
}
