//! Jira provider — issues, sprints, and boards via the Jira REST API.
//!
//! Configuration via environment variables:
//!   - `JIRA_URL`: Base URL (e.g., `https://company.atlassian.net`)
//!   - `JIRA_EMAIL`: User email for Basic Auth
//!   - `JIRA_TOKEN`: API token
//!   - `JIRA_PROJECT`: Default project key (e.g., "PROJ")
//!   - `JIRA_DEPLOYMENT`: `cloud` (default) or `server` for Data Center/Server

use crate::core::providers::{ContextProvider, ProviderItem, ProviderParams, ProviderResult};

const B64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn simple_base64(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64_CHARS[((n >> 18) & 63) as usize] as char);
        out.push(B64_CHARS[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(B64_CHARS[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(B64_CHARS[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JiraDeployment {
    Cloud,
    Server,
}

pub struct JiraConfig {
    pub base_url: String,
    pub email: String,
    pub token: String,
    pub project: Option<String>,
    pub deployment: JiraDeployment,
}

impl JiraConfig {
    pub fn from_env() -> Result<Self, String> {
        let base_url = std::env::var("JIRA_URL").map_err(|_| "JIRA_URL not set")?;
        let email = std::env::var("JIRA_EMAIL").map_err(|_| "JIRA_EMAIL not set")?;
        let token = std::env::var("JIRA_TOKEN").map_err(|_| "JIRA_TOKEN not set")?;
        let project = std::env::var("JIRA_PROJECT").ok();

        let deployment = match std::env::var("JIRA_DEPLOYMENT")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "server" | "dc" | "datacenter" => JiraDeployment::Server,
            _ => JiraDeployment::Cloud,
        };

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            email,
            token,
            project,
            deployment,
        })
    }

    fn auth_header(&self) -> String {
        let credentials = format!("{}:{}", self.email, self.token);
        let encoded = simple_base64(credentials.as_bytes());
        format!("Basic {encoded}")
    }
}

pub struct JiraProvider {
    config: Result<JiraConfig, String>,
}

impl Default for JiraProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl JiraProvider {
    pub fn new() -> Self {
        Self {
            config: JiraConfig::from_env(),
        }
    }
}

impl ContextProvider for JiraProvider {
    fn id(&self) -> &'static str {
        "jira"
    }

    fn display_name(&self) -> &'static str {
        "Jira"
    }

    fn supported_actions(&self) -> &[&str] {
        &["issues", "sprints"]
    }

    fn execute(&self, action: &str, params: &ProviderParams) -> Result<ProviderResult, String> {
        let config = self.config.as_ref().map_err(std::clone::Clone::clone)?;
        match action {
            "issues" => list_issues(config, params),
            "sprints" => list_sprints(config, params),
            _ => Err(format!("Unsupported action: {action}")),
        }
    }

    fn cache_ttl_secs(&self) -> u64 {
        120
    }

    fn requires_auth(&self) -> bool {
        true
    }

    fn is_available(&self) -> bool {
        self.config.is_ok()
    }
}

// ---------------------------------------------------------------------------
// HTTP helper with status-code-aware error messages
// ---------------------------------------------------------------------------

fn jira_request(
    config: &JiraConfig,
    method: &str,
    url: &str,
    body: Option<&[u8]>,
) -> Result<String, String> {
    let resp = match method {
        "POST" => ureq::post(url)
            .header("Authorization", &config.auth_header())
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send(body.unwrap_or(&[]))
            .map_err(|ref e| jira_error_with_hint(e))?,
        _ => ureq::get(url)
            .header("Authorization", &config.auth_header())
            .header("Accept", "application/json")
            .call()
            .map_err(|ref e| jira_error_with_hint(e))?,
    };

    resp.into_body()
        .read_to_string()
        .map_err(|e| format!("Jira read error: {e}"))
}

fn jira_error_with_hint(e: &ureq::Error) -> String {
    let hint = match e {
        ureq::Error::StatusCode(410) => {
            " (endpoint removed — update lean-ctx or check Jira Cloud API version)"
        }
        ureq::Error::StatusCode(401) => " (check JIRA_EMAIL + JIRA_TOKEN credentials)",
        ureq::Error::StatusCode(403) => " (insufficient permissions for this resource)",
        ureq::Error::StatusCode(404) => {
            " (endpoint not found — check JIRA_URL and JIRA_DEPLOYMENT setting)"
        }
        _ => "",
    };
    format!("Jira API error: {e}{hint}")
}

// ---------------------------------------------------------------------------
// Issues — Cloud: POST /rest/api/3/search/jql  |  Server: GET /rest/api/2/search
// ---------------------------------------------------------------------------

fn list_issues(config: &JiraConfig, params: &ProviderParams) -> Result<ProviderResult, String> {
    match config.deployment {
        JiraDeployment::Cloud => list_issues_cloud(config, params),
        JiraDeployment::Server => list_issues_server(config, params),
    }
}

fn build_jql(config: &JiraConfig, params: &ProviderParams) -> String {
    let project = params
        .state
        .as_deref()
        .or(config.project.as_deref())
        .unwrap_or("*");

    if project == "*" {
        "ORDER BY updated DESC".to_string()
    } else {
        format!("project={project} ORDER BY updated DESC")
    }
}

fn list_issues_cloud(
    config: &JiraConfig,
    params: &ProviderParams,
) -> Result<ProviderResult, String> {
    let limit = params.limit.unwrap_or(20);
    let jql = build_jql(config, params);
    let url = format!("{}/rest/api/3/search/jql", config.base_url);

    let mut all_items = Vec::new();
    let mut next_page_token: Option<String> = None;
    loop {
        let page_size = (limit - all_items.len()).min(100);
        let mut body = serde_json::json!({
            "jql": jql,
            "maxResults": page_size,
            "fields": ["summary", "status", "reporter", "created", "updated", "labels", "description"]
        });
        if let Some(ref token) = next_page_token {
            body["nextPageToken"] = serde_json::json!(token);
        }

        let body_bytes = serde_json::to_vec(&body).unwrap_or_default();
        let text = jira_request(config, "POST", &url, Some(&body_bytes))?;
        let resp: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("Jira JSON parse error: {e}"))?;

        let issues = resp["issues"].as_array().cloned().unwrap_or_default();
        all_items.extend(issues.iter().map(|issue| parse_issue(issue, config)));

        next_page_token = resp["nextPageToken"].as_str().map(String::from);

        if next_page_token.is_none() || all_items.len() >= limit {
            break;
        }
    }

    let truncated = next_page_token.is_some();
    all_items.truncate(limit);

    Ok(ProviderResult {
        provider: "jira".into(),
        resource_type: "issues".into(),
        total_count: Some(all_items.len()),
        truncated,
        items: all_items,
    })
}

fn list_issues_server(
    config: &JiraConfig,
    params: &ProviderParams,
) -> Result<ProviderResult, String> {
    let limit = params.limit.unwrap_or(20);
    let jql = build_jql(config, params);

    let url = format!(
        "{}/rest/api/2/search?jql={}&maxResults={limit}",
        config.base_url,
        urlencoding::encode(&jql)
    );

    let text = jira_request(config, "GET", &url, None)?;
    let body: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Jira JSON parse error: {e}"))?;

    let total = body["total"].as_u64().unwrap_or(0) as usize;
    let issues = body["issues"].as_array().cloned().unwrap_or_default();

    let items: Vec<ProviderItem> = issues
        .iter()
        .map(|issue| parse_issue(issue, config))
        .collect();

    Ok(ProviderResult {
        provider: "jira".into(),
        resource_type: "issues".into(),
        items,
        total_count: Some(total),
        truncated: total > limit,
    })
}

fn parse_issue(issue: &serde_json::Value, config: &JiraConfig) -> ProviderItem {
    let fields = &issue["fields"];
    ProviderItem {
        id: issue["key"].as_str().unwrap_or_default().to_string(),
        title: fields["summary"].as_str().unwrap_or_default().to_string(),
        state: fields["status"]["name"].as_str().map(String::from),
        author: fields["reporter"]["displayName"].as_str().map(String::from),
        created_at: fields["created"].as_str().map(String::from),
        updated_at: fields["updated"].as_str().map(String::from),
        url: Some(format!(
            "{}/browse/{}",
            config.base_url,
            issue["key"].as_str().unwrap_or_default()
        )),
        labels: fields["labels"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        body: fields["description"]
            .as_str()
            .map(String::from)
            .or_else(|| {
                fields["description"]["content"]
                    .as_array()
                    .map(|_| "[Jira rich text — see web UI]".to_string())
            }),
    }
}

// ---------------------------------------------------------------------------
// Sprints — /rest/agile/1.0/board/{id}/sprint
// ---------------------------------------------------------------------------

fn list_sprints(config: &JiraConfig, params: &ProviderParams) -> Result<ProviderResult, String> {
    let board_id = params
        .state
        .as_deref()
        .ok_or("Sprint listing requires a board ID via the 'state' parameter")?;

    let limit = params.limit.unwrap_or(5);
    let url = format!(
        "{}/rest/agile/1.0/board/{board_id}/sprint?state=active,future&maxResults={limit}",
        config.base_url
    );

    let text = jira_request(config, "GET", &url, None)?;
    let body: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Jira JSON parse error: {e}"))?;

    let sprints = body["values"].as_array().cloned().unwrap_or_default();
    let items: Vec<ProviderItem> = sprints
        .iter()
        .map(|s| ProviderItem {
            id: s["id"].as_u64().map_or_else(String::new, |n| n.to_string()),
            title: s["name"].as_str().unwrap_or_default().to_string(),
            state: s["state"].as_str().map(String::from),
            author: None,
            created_at: s["startDate"].as_str().map(String::from),
            updated_at: s["endDate"].as_str().map(String::from),
            url: None,
            labels: vec![],
            body: s["goal"].as_str().map(String::from),
        })
        .collect();

    Ok(ProviderResult {
        provider: "jira".into(),
        resource_type: "sprints".into(),
        items,
        total_count: Some(sprints.len()),
        truncated: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jira_provider_is_unavailable_without_env() {
        let _orig_url = std::env::var("JIRA_URL");
        std::env::remove_var("JIRA_URL");
        std::env::remove_var("JIRA_EMAIL");
        std::env::remove_var("JIRA_TOKEN");

        let provider = JiraProvider::new();
        assert!(!provider.is_available());
        assert_eq!(provider.id(), "jira");
        assert!(provider.requires_auth());
    }

    #[test]
    fn jira_provider_supported_actions() {
        let provider = JiraProvider::new();
        assert!(provider.supported_actions().contains(&"issues"));
        assert!(provider.supported_actions().contains(&"sprints"));
    }

    #[test]
    fn deployment_defaults_to_cloud() {
        std::env::remove_var("JIRA_DEPLOYMENT");
        std::env::set_var("JIRA_URL", "https://test.atlassian.net");
        std::env::set_var("JIRA_EMAIL", "test@test.com");
        std::env::set_var("JIRA_TOKEN", "token");
        let cfg = JiraConfig::from_env().unwrap();
        assert_eq!(cfg.deployment, JiraDeployment::Cloud);
        std::env::remove_var("JIRA_URL");
        std::env::remove_var("JIRA_EMAIL");
        std::env::remove_var("JIRA_TOKEN");
    }

    #[test]
    fn deployment_server_variants() {
        for val in &["server", "dc", "datacenter", "SERVER", "DC"] {
            std::env::set_var("JIRA_URL", "https://jira.internal");
            std::env::set_var("JIRA_EMAIL", "u@e.com");
            std::env::set_var("JIRA_TOKEN", "t");
            std::env::set_var("JIRA_DEPLOYMENT", val);
            let cfg = JiraConfig::from_env().unwrap();
            assert_eq!(cfg.deployment, JiraDeployment::Server, "failed for {val}");
        }
        std::env::remove_var("JIRA_URL");
        std::env::remove_var("JIRA_EMAIL");
        std::env::remove_var("JIRA_TOKEN");
        std::env::remove_var("JIRA_DEPLOYMENT");
    }

    #[test]
    fn build_jql_with_project() {
        let cfg = JiraConfig {
            base_url: "https://x.atlassian.net".into(),
            email: String::new(),
            token: String::new(),
            project: Some("PROJ".into()),
            deployment: JiraDeployment::Cloud,
        };
        let params = ProviderParams::default();
        assert_eq!(
            build_jql(&cfg, &params),
            "project=PROJ ORDER BY updated DESC"
        );
    }

    #[test]
    fn build_jql_wildcard() {
        let cfg = JiraConfig {
            base_url: String::new(),
            email: String::new(),
            token: String::new(),
            project: None,
            deployment: JiraDeployment::Cloud,
        };
        let params = ProviderParams::default();
        assert_eq!(build_jql(&cfg, &params), "ORDER BY updated DESC");
    }

    #[test]
    fn error_hint_410() {
        let msg = jira_error_with_hint(&ureq::Error::StatusCode(410));
        assert!(msg.contains("endpoint removed"), "{msg}");
    }

    #[test]
    fn error_hint_401() {
        let msg = jira_error_with_hint(&ureq::Error::StatusCode(401));
        assert!(msg.contains("JIRA_EMAIL"), "{msg}");
    }

    #[test]
    fn error_hint_403() {
        let msg = jira_error_with_hint(&ureq::Error::StatusCode(403));
        assert!(msg.contains("permissions"), "{msg}");
    }

    #[test]
    fn error_hint_404() {
        let msg = jira_error_with_hint(&ureq::Error::StatusCode(404));
        assert!(msg.contains("JIRA_DEPLOYMENT"), "{msg}");
    }

    #[test]
    fn parse_issue_extracts_fields() {
        let issue = serde_json::json!({
            "key": "PROJ-123",
            "fields": {
                "summary": "Test issue",
                "status": { "name": "Open" },
                "reporter": { "displayName": "Alice" },
                "created": "2026-01-01T00:00:00Z",
                "updated": "2026-05-01T00:00:00Z",
                "labels": ["bug", "urgent"],
                "description": "Fix the thing"
            }
        });
        let cfg = JiraConfig {
            base_url: "https://x.atlassian.net".into(),
            email: String::new(),
            token: String::new(),
            project: None,
            deployment: JiraDeployment::Cloud,
        };
        let item = parse_issue(&issue, &cfg);
        assert_eq!(item.id, "PROJ-123");
        assert_eq!(item.title, "Test issue");
        assert_eq!(item.state.as_deref(), Some("Open"));
        assert_eq!(item.author.as_deref(), Some("Alice"));
        assert_eq!(item.labels, vec!["bug", "urgent"]);
        assert_eq!(item.body.as_deref(), Some("Fix the thing"));
        assert!(item.url.as_deref().unwrap().contains("/browse/PROJ-123"));
    }

    #[test]
    fn base64_encoding() {
        assert_eq!(simple_base64(b"user:token"), "dXNlcjp0b2tlbg==");
        assert_eq!(simple_base64(b"a"), "YQ==");
        assert_eq!(simple_base64(b"ab"), "YWI=");
        assert_eq!(simple_base64(b"abc"), "YWJj");
    }
}
