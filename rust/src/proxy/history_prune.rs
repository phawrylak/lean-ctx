use serde_json::Value;

/// Summarize old tool_result blocks in conversation history to reduce token count.
/// Only prunes results older than `keep_recent` messages from the end.
pub fn prune_history(messages: &mut [Value], keep_recent: usize) {
    let len = messages.len();
    if len <= keep_recent {
        return;
    }
    let prune_end = len - keep_recent;

    for msg in &mut messages[..prune_end] {
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");

        match role {
            // Anthropic: user messages with tool_result content blocks
            "user" => {
                if let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) {
                    for block in content.iter_mut() {
                        if block.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                            summarize_anthropic_tool_result(block);
                        }
                    }
                }
            }
            // OpenAI: tool role messages
            "tool" => {
                if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                    if content.len() > 200 {
                        let summary = summarize_text(content);
                        msg["content"] = Value::String(summary);
                    }
                }
            }
            _ => {}
        }
    }
}

fn summarize_anthropic_tool_result(block: &mut Value) {
    if let Some(inner) = block.get_mut("content") {
        match inner {
            Value::String(s) if s.len() > 200 => {
                *s = summarize_text(s);
            }
            Value::Array(arr) => {
                for item in arr.iter_mut() {
                    if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                        if let Some(Value::String(s)) = item.get_mut("text") {
                            if s.len() > 200 {
                                *s = summarize_text(s);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn summarize_text(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= 5 {
        return text.to_string();
    }

    let first_3: Vec<&str> = lines.iter().take(3).copied().collect();
    let last_2: Vec<&str> = lines.iter().rev().take(2).rev().copied().collect();

    format!(
        "{}\n[...{} lines pruned by lean-ctx...]\n{}",
        first_3.join("\n"),
        lines.len() - 5,
        last_2.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_skips_recent_messages() {
        let long_content = (0..40).map(|i| format!("line {i}: this is a longer line to ensure content exceeds the 200 character threshold for pruning")).collect::<Vec<_>>().join("\n");
        let mut messages = vec![
            serde_json::json!({"role": "tool", "content": long_content}),
            serde_json::json!({"role": "assistant", "content": "ok"}),
            serde_json::json!({"role": "tool", "content": long_content}),
        ];
        prune_history(&mut messages, 2);
        let first = messages[0]["content"].as_str().unwrap();
        assert!(first.contains("pruned"), "old message should be pruned");
        let last = messages[2]["content"].as_str().unwrap();
        assert!(!last.contains("pruned"), "recent message should be kept");
    }

    #[test]
    fn prune_handles_short_content() {
        let mut messages = vec![serde_json::json!({"role": "tool", "content": "short"})];
        prune_history(&mut messages, 0);
        assert_eq!(messages[0]["content"].as_str().unwrap(), "short");
    }
}
