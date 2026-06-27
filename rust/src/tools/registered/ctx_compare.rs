use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::core::compress_preview;
use crate::server::tool_trait::{McpTool, ToolContext, ToolOutput, get_str, require_resolved_path};
use crate::tool_defs::tool_def;

/// `ctx_compare` — read-only compression preview (#984).
///
/// Surfaces the production compressors' effect side by side so an agent (or a
/// human) can see *exactly* what lean-ctx would emit and how many tokens it
/// saves, without re-deriving the pipeline.
pub struct CtxCompareTool;

impl McpTool for CtxCompareTool {
    fn name(&self) -> &'static str {
        "ctx_compare"
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "ctx_compare",
            "Preview compression — original vs the bytes lean-ctx would emit, with token counts + line diff.\n\
             INPUT (pick one): path=<file> (read pipeline) | content=<text> [+ ext=rs|json|csv] (read pipeline) | command=<cmd> + output=<text> (shell pipeline).\n\
             Read-only: never changes files, cache, or session. Use to decide whether a mode/pipeline is worth it.\n\
             ANTIPATTERN: not for reading files (use ctx_read) or restoring archived output (use ctx_expand).",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File to preview via the read/aggressive pipeline" },
                    "content": { "type": "string", "description": "Inline content to preview (read pipeline)" },
                    "ext": { "type": "string", "description": "Extension for inline content, e.g. rs, json, csv" },
                    "command": { "type": "string", "description": "Shell command for the shell pipeline (pair with output)" },
                    "output": { "type": "string", "description": "Command output to preview (shell pipeline)" }
                }
            }),
        )
    }

    fn handle(
        &self,
        args: &Map<String, Value>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ErrorData> {
        // Precedence: path > inline content > shell command/output.
        if args.contains_key("path") {
            let resolved = require_resolved_path(ctx, args, "path")?;
            let content = match std::fs::read_to_string(&resolved) {
                Ok(c) => c,
                Err(e) => {
                    return Ok(ToolOutput::simple(format!(
                        "ctx_compare: cannot read {resolved}: {e}"
                    )));
                }
            };
            let ext = compress_preview::ext_of(&resolved);
            let preview = compress_preview::preview_read(&content, ext.as_deref());
            return Ok(ToolOutput::simple(preview.render()));
        }

        if let Some(content) = get_str(args, "content") {
            let ext = get_str(args, "ext");
            let preview = compress_preview::preview_read(&content, ext.as_deref());
            return Ok(ToolOutput::simple(preview.render()));
        }

        if let Some(command) = get_str(args, "command") {
            let output = get_str(args, "output").unwrap_or_default();
            let preview = compress_preview::preview_shell(&command, &output);
            return Ok(ToolOutput::simple(preview.render()));
        }

        Err(ErrorData::invalid_params(
            "ctx_compare needs one of: path, content, or command (+output)".to_string(),
            None,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ToolContext {
        ToolContext::default()
    }

    #[test]
    fn previews_inline_content_with_token_accounting() {
        let mut args = Map::new();
        args.insert(
            "content".to_string(),
            Value::String("// drop me\nfn a() {}\n".to_string()),
        );
        args.insert("ext".to_string(), Value::String("rs".to_string()));
        let out = CtxCompareTool.handle(&args, &ctx()).unwrap();
        assert!(out.text.contains("compress preview"));
        assert!(out.text.contains("read/aggressive"));
        // The comment survives only as a diff *deletion* (`-N: …`), proving it was
        // stripped from the compressed form.
        assert!(
            out.text.contains("-1: // drop me"),
            "comment should appear as a removal in the diff: {}",
            out.text
        );
    }

    #[test]
    fn previews_shell_pipeline() {
        let mut args = Map::new();
        args.insert(
            "command".to_string(),
            Value::String("cargo build".to_string()),
        );
        args.insert(
            "output".to_string(),
            Value::String("Compiling foo\n".repeat(40)),
        );
        let out = CtxCompareTool.handle(&args, &ctx()).unwrap();
        assert!(out.text.contains("pipeline: shell"));
    }

    #[test]
    fn errors_without_any_input() {
        let args = Map::new();
        let msg = match CtxCompareTool.handle(&args, &ctx()) {
            Err(e) => format!("{e:?}"),
            Ok(_) => panic!("expected an error when no input is given"),
        };
        assert!(msg.contains("needs one of"), "got: {msg}");
    }
}
