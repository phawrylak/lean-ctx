use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::Response,
};
use serde_json::Value;

use super::compress::compress_tool_result;
use super::forward;
use super::tool_kind::{self, should_protect, ToolResultKind};
use super::ProxyState;

/// Conversation turns kept fully intact at the tail of the history; older
/// tool results are summarized by `history_prune`.
const KEEP_RECENT: usize = 6;

pub async fn handler(
    State(state): State<ProxyState>,
    req: Request<Body>,
) -> Result<Response, StatusCode> {
    let upstream = state.anthropic_upstream.clone();
    forward::forward_request(
        State(state),
        req,
        &upstream,
        "/v1/messages",
        compress_request_body,
        "Anthropic",
        &[],
    )
    .await
}

fn compress_request_body(parsed: Value, original_size: usize) -> (Vec<u8>, usize, usize) {
    let mut doc = parsed;
    let mut modified = false;

    if let Some(messages) = doc.get_mut("messages").and_then(|m| m.as_array_mut()) {
        // Resolve tool-call id → tool name so file/source reads can be protected
        // from lossy compression that would force the model to re-read mid-task.
        let tool_names = tool_kind::anthropic_tool_names(messages);

        super::history_prune::prune_history(messages, KEEP_RECENT, &tool_names);
        modified = true;

        for msg in messages.iter_mut() {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if role != "user" {
                continue;
            }

            if let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) {
                for block in content.iter_mut() {
                    if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                        continue;
                    }

                    let name = block
                        .get("tool_use_id")
                        .and_then(|v| v.as_str())
                        .and_then(|id| tool_names.get(id))
                        .map(String::as_str);
                    let kind = name.map_or(ToolResultKind::Other, tool_kind::classify_tool_name);

                    if let Some(inner_content) = block.get_mut("content") {
                        modified |= compress_content_field(inner_content, name, kind);
                    }
                }
            }
        }
    }

    let out = serde_json::to_vec(&doc).unwrap_or_default();
    let compressed_size = if modified { out.len() } else { original_size };
    (out, original_size, compressed_size)
}

/// Compresses a tool_result `content` field unless it is a protected file/source
/// read, which must reach the model intact (it is what gets edited).
fn compress_content_field(
    content: &mut Value,
    tool_name: Option<&str>,
    kind: ToolResultKind,
) -> bool {
    match content {
        Value::String(s) => {
            if should_protect(kind, s) {
                return false;
            }
            let compressed = compress_tool_result(s, tool_name);
            if compressed.len() < s.len() {
                *s = compressed;
                return true;
            }
            false
        }
        Value::Array(arr) => {
            let mut modified = false;
            for item in arr.iter_mut() {
                if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(text) = item
                        .get_mut("text")
                        .and_then(|t| t.as_str().map(String::from))
                    {
                        if should_protect(kind, &text) {
                            continue;
                        }
                        let compressed = compress_tool_result(&text, tool_name);
                        if compressed.len() < text.len() {
                            item["text"] = Value::String(compressed);
                            modified = true;
                        }
                    }
                }
            }
            modified
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_file_body() -> Vec<u8> {
        let code = (0..60)
            .map(|i| format!("    let binding_{i} = compute_value_{i}(context, options);"))
            .collect::<Vec<_>>()
            .join("\n");
        let body = serde_json::json!({
            "model": "claude-opus-4-8",
            "messages": [
                {
                    "role": "assistant",
                    "content": [{"type": "tool_use", "id": "toolu_1", "name": "Read", "input": {"file_path": "src/app.rs"}}]
                },
                {
                    "role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": code}]
                }
            ]
        });
        serde_json::to_vec(&body).unwrap()
    }

    #[test]
    fn read_tool_result_is_never_truncated() {
        let bytes = source_file_body();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let (out, _orig, _comp) = compress_request_body(body, bytes.len());
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        let content = parsed["messages"][1]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(
            content.contains("binding_59"),
            "the full source body must survive — refactors need it intact"
        );
        assert!(!content.contains("lines omitted"));
    }

    #[test]
    fn bash_tool_result_still_compresses() {
        let log = {
            let mut s = String::from(
                "$ git status\nOn branch main\nYour branch is up to date with 'origin/main'.\n\nChanges not staged for commit:\n  (use \"git add <file>...\" to update what will be committed)\n",
            );
            for i in 0..90 {
                s.push_str(&format!("\tmodified:   src/module_{i}/file_{i}.rs\n"));
            }
            s.push_str("\nno changes added to commit (use \"git add\" and/or \"git commit -a\")\n");
            s
        };
        let body = serde_json::json!({
            "messages": [
                {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": log}]}
            ]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let (_out, orig, comp) = compress_request_body(body, bytes.len());
        assert!(comp < orig, "shell output must still be compressed");
    }
}
