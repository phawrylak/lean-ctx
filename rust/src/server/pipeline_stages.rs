#![allow(dead_code)]

use rmcp::model::{CallToolResult, Content};
use rmcp::ErrorData;
use serde_json::{Map, Value};

use crate::tools::{CrpMode, LeanCtxServer};

use super::helpers;
use super::role_guard;

/// Stage 1: Resolve meta-tool (ctx -> ctx_*).
pub(super) fn resolve_meta_tool(
    original_name: String,
    arguments: Option<Map<String, Value>>,
) -> Result<(String, Option<Map<String, Value>>), ErrorData> {
    if original_name == "ctx" {
        let sub = arguments
            .as_ref()
            .and_then(|a| a.get("tool"))
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string)
            .ok_or_else(|| {
                ErrorData::invalid_params("'tool' is required for ctx meta-tool", None)
            })?;
        let tool_name = if sub.starts_with("ctx_") {
            sub
        } else {
            format!("ctx_{sub}")
        };
        let mut args = arguments.unwrap_or_default();
        args.remove("tool");
        Ok((tool_name, Some(args)))
    } else {
        Ok((original_name, arguments))
    }
}

/// Stage 2: Check role-based access control.
pub(super) fn check_role_access(name: &str) -> Option<CallToolResult> {
    let role_check = role_guard::check_tool_access(name);
    if let Some(denied) = role_guard::into_call_tool_result(&role_check) {
        tracing::warn!(
            tool = name,
            role = %role_check.role_name,
            "Tool blocked by role policy"
        );
        return Some(denied);
    }
    None
}

impl LeanCtxServer {
    /// Stage 3: Check workflow gate.
    pub(super) async fn check_workflow_gate(&self, name: &str) -> Option<CallToolResult> {
        if name == "ctx_workflow" || name == "ctx_call" {
            return None;
        }
        let active = self.workflow.read().await.clone();
        if let Some(run) = active {
            if let Some(state) = run.spec.state(&run.current) {
                if let Some(allowed) = &state.allowed_tools {
                    let allowed_ok = allowed.iter().any(|t| t == name) || name == "ctx";
                    if !allowed_ok {
                        let mut shown = allowed.clone();
                        shown.sort();
                        shown.truncate(30);
                        return Some(CallToolResult::success(vec![Content::text(format!(
                            "Tool '{name}' blocked by workflow '{}' (state: {}). Allowed ({} shown): {}",
                            run.spec.name,
                            run.current,
                            shown.len(),
                            shown.join(", ")
                        ))]));
                    }
                }
            }
        }
        None
    }

    /// Stage 4: Autonomy session lifecycle pre-hook.
    pub(super) async fn run_autonomy_pre_hook(&self, name: &str) -> Option<String> {
        let task = {
            let session = self.session.read().await;
            session.task.as_ref().map(|t| t.description.clone())
        };
        let project_root = {
            let session = self.session.read().await;
            session.project_root.clone()
        };
        let mut cache = self.cache.write().await;
        crate::tools::autonomy::session_lifecycle_pre_hook(
            &self.autonomy,
            name,
            &mut cache,
            task.as_deref(),
            project_root.as_deref(),
            CrpMode::effective(),
        )
    }

    /// Stage 5: Loop detection + throttle.
    pub(super) async fn check_loop_detection(
        &self,
        name: &str,
        args: Option<&Map<String, Value>>,
    ) -> Result<Option<String>, CallToolResult> {
        let fp = args
            .map(|a| {
                crate::core::loop_detection::LoopDetector::fingerprint(&serde_json::Value::Object(
                    a.clone(),
                ))
            })
            .unwrap_or_default();
        let mut detector = self.loop_detector.write().await;

        let is_search = crate::core::loop_detection::LoopDetector::is_search_tool(name);
        let is_search_shell = name == "ctx_shell" && {
            let cmd = args
                .and_then(|a| a.get("command"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            crate::core::loop_detection::LoopDetector::is_search_shell_command(cmd)
        };

        let throttle_result = if is_search || is_search_shell {
            let search_pattern = args.and_then(|a| {
                a.get("pattern")
                    .or_else(|| a.get("query"))
                    .and_then(|v| v.as_str())
            });
            let shell_pattern = if is_search_shell {
                args.and_then(|a| a.get("command"))
                    .and_then(|v| v.as_str())
                    .and_then(helpers::extract_search_pattern_from_command)
            } else {
                None
            };
            let pat = search_pattern.or(shell_pattern.as_deref());
            detector.record_search(name, &fp, pat)
        } else {
            detector.record_call(name, &fp)
        };

        if throttle_result.level == crate::core::loop_detection::ThrottleLevel::Blocked {
            let msg = throttle_result.message.unwrap_or_default();
            return Err(CallToolResult::success(vec![Content::text(msg)]));
        }

        let throttle_warning =
            if throttle_result.level == crate::core::loop_detection::ThrottleLevel::Reduced {
                throttle_result.message.clone()
            } else {
                None
            };
        Ok(throttle_warning)
    }

    /// Stage 6: Degradation policy + budget enforcement.
    pub(super) async fn check_degradation_policy(&self, name: &str) -> Option<CallToolResult> {
        let policy = crate::core::degradation_policy::evaluate_v1_for_tool(name, None);
        let governance_tool = matches!(name, "ctx_session" | "ctx_cost" | "ctx_metrics");

        if policy.decision.reason_code == "budget_exhausted" && !governance_tool {
            use crate::core::budget_tracker::BudgetLevel;
            let snap = &policy.budgets;
            for (dim, lvl, used, limit) in [
                (
                    "tokens",
                    &snap.tokens.level,
                    format!("{}", snap.tokens.used),
                    format!("{}", snap.tokens.limit),
                ),
                (
                    "shell",
                    &snap.shell.level,
                    format!("{}", snap.shell.used),
                    format!("{}", snap.shell.limit),
                ),
                (
                    "cost",
                    &snap.cost.level,
                    format!("${:.2}", snap.cost.used_usd),
                    format!("${:.2}", snap.cost.limit_usd),
                ),
            ] {
                if *lvl == BudgetLevel::Exhausted {
                    crate::core::events::emit_budget_exhausted(&snap.role, dim, &used, &limit);
                }
            }
            let msg = format!(
                "[BUDGET EXHAUSTED] {}\n\
                 Use `ctx_session action=role` to check/switch roles, \
                 or `ctx_session action=reset` to start fresh.",
                snap.format_compact()
            );
            tracing::warn!(tool = name, "{msg}");
            return Some(CallToolResult::success(vec![Content::text(msg)]));
        }

        let enforce_slo = policy.decision.enforced;
        if enforce_slo && !governance_tool {
            if policy.decision.reason_code == "slo_block" {
                let msg = format!(
                    "[SLO BLOCK] {}\n\
                     Use `ctx_session action=role` to check/switch roles, \
                     or lower load / budgets to continue.",
                    policy.slo.format_compact()
                );
                tracing::warn!(tool = name, "{msg}");
                return Some(CallToolResult::success(vec![Content::text(msg)]));
            }
            if policy.decision.reason_code == "slo_throttle" {
                if let Some(ms) = policy.decision.throttle_ms {
                    let ms = ms.clamp(1, 5_000);
                    tokio::time::sleep(tokio::time::Duration::from_millis(ms)).await;
                }
            }
        }
        None
    }

    /// Post-dispatch: budget warning check.
    pub(super) fn check_budget_warning() -> Option<String> {
        use crate::core::budget_tracker::{BudgetLevel, BudgetTracker};
        let snap = BudgetTracker::global().check();
        if *snap.worst_level() == BudgetLevel::Warning {
            for (dim, lvl, used, limit, pct) in [
                (
                    "tokens",
                    &snap.tokens.level,
                    format!("{}", snap.tokens.used),
                    format!("{}", snap.tokens.limit),
                    snap.tokens.percent,
                ),
                (
                    "shell",
                    &snap.shell.level,
                    format!("{}", snap.shell.used),
                    format!("{}", snap.shell.limit),
                    snap.shell.percent,
                ),
                (
                    "cost",
                    &snap.cost.level,
                    format!("${:.2}", snap.cost.used_usd),
                    format!("${:.2}", snap.cost.limit_usd),
                    snap.cost.percent,
                ),
            ] {
                if *lvl == BudgetLevel::Warning {
                    crate::core::events::emit_budget_warning(&snap.role, dim, &used, &limit, pct);
                }
            }
            if crate::core::protocol::meta_visible() {
                Some(format!("[BUDGET WARNING] {}", snap.format_compact()))
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Post-dispatch: archive large outputs.
    pub(super) async fn maybe_archive(
        &self,
        name: &str,
        args: Option<&Map<String, Value>>,
        result_text: &str,
        minimal: bool,
    ) -> Option<String> {
        if minimal {
            return None;
        }
        use crate::core::archive;
        let archivable = matches!(
            name,
            "ctx_shell"
                | "ctx_read"
                | "ctx_multi_read"
                | "ctx_smart_read"
                | "ctx_execute"
                | "ctx_search"
                | "ctx_tree"
        );
        if archivable && archive::should_archive(result_text) {
            let cmd = helpers::get_str(args, "command")
                .or_else(|| helpers::get_str(args, "path"))
                .unwrap_or_default();
            let session_id = self.session.read().await.id.clone();
            let to_store = crate::core::redaction::redact_text_if_enabled(result_text);
            let tokens = crate::core::tokens::count_tokens(&to_store);
            archive::store(name, &cmd, &to_store, Some(&session_id))
                .map(|id| archive::format_hint(&id, to_store.len(), tokens))
        } else {
            None
        }
    }

    /// Post-dispatch: ctx_read enrichment (related files, prefetch).
    pub(super) async fn enrich_after_read(
        &self,
        name: &str,
        args: Option<&Map<String, Value>>,
        result_text: &mut String,
        minimal: bool,
    ) {
        if name != "ctx_read" {
            return;
        }
        if minimal {
            let mut cache = self.cache.write().await;
            crate::tools::autonomy::maybe_auto_dedup(&self.autonomy, &mut cache, name);
        } else {
            let read_path = self
                .resolve_path_or_passthrough(&helpers::get_str(args, "path").unwrap_or_default())
                .await;
            let project_root = {
                let session = self.session.read().await;
                session.project_root.clone()
            };
            let task = {
                let session = self.session.read().await;
                session.task.as_ref().map(|t| t.description.clone())
            };
            let mut cache = self.cache.write().await;
            let enrich = crate::tools::autonomy::enrich_after_read(
                &self.autonomy,
                &mut cache,
                &read_path,
                project_root.as_deref(),
                task.as_deref(),
                CrpMode::effective(),
                minimal,
            );
            if let Some(hint) = enrich.related_hint {
                *result_text = format!("{result_text}\n{hint}");
            }
            if let Some(hint) = enrich.prefetch_hint {
                *result_text = format!("{result_text}\n{hint}");
            }
            if let Some(hint) = crate::tools::autonomy::large_ctx_read_full_hint(
                &self.autonomy,
                helpers::get_str(args, "mode").as_deref(),
                result_text.as_str(),
            ) {
                *result_text = format!("{result_text}\n{hint}");
            }
            crate::tools::autonomy::maybe_auto_dedup(&self.autonomy, &mut cache, name);
        }
    }

    /// Post-dispatch: autonomy auto-response compression.
    pub(super) async fn apply_auto_response(
        &self,
        name: &str,
        args: Option<&Map<String, Value>>,
        result_text: &str,
        minimal: bool,
    ) -> String {
        let action = helpers::get_str(args, "action");
        let before = result_text.to_string();
        let before_tokens = crate::core::tokens::count_tokens(&before);
        let start = std::time::Instant::now();
        let after = crate::tools::autonomy::maybe_auto_response(
            &self.autonomy,
            name,
            action.as_deref(),
            result_text,
            CrpMode::effective(),
            minimal,
        );
        let duration_us = start.elapsed().as_micros() as u64;
        if after != before {
            let after_tokens = crate::core::tokens::count_tokens(&after);
            let mut stats = self.pipeline_stats.write().await;
            stats.record(&[crate::core::pipeline::LayerMetrics::new(
                crate::core::pipeline::LayerKind::Autonomy,
                before_tokens,
                after_tokens,
                duration_us,
            )]);
            stats.save();
        }
        after
    }

    /// Post-dispatch: shell efficiency hint + sandbox archival for large outputs.
    pub(super) async fn shell_efficiency_hint(
        &self,
        name: &str,
        args: Option<&Map<String, Value>>,
        result_text: &mut String,
        output_token_count: usize,
        minimal: bool,
    ) {
        if minimal || name != "ctx_shell" {
            return;
        }
        let cmd = helpers::get_str(args, "command").unwrap_or_default();
        let output_bytes = result_text.len();

        const SANDBOX_THRESHOLD: usize = 5000;
        if output_bytes > SANDBOX_THRESHOLD {
            use crate::core::archive;
            let session_id = self.session.read().await.id.clone();
            let tokens = crate::core::tokens::count_tokens(result_text);
            let tail_lines: Vec<&str> = result_text.lines().rev().take(10).collect();
            let tail: String = tail_lines.into_iter().rev().collect::<Vec<_>>().join("\n");
            let redacted_for_archive = crate::core::redaction::redact_text_if_enabled(result_text);
            if let Some(id) =
                archive::store("ctx_shell", &cmd, &redacted_for_archive, Some(&session_id))
            {
                let hint = archive::format_hint(&id, output_bytes, tokens);
                *result_text =
                    format!("[sandbox] Output archived ({output_bytes} bytes, {tokens} tok).\n{hint}\n\nTail:\n{tail}");
                return;
            }
        }

        if let Some(hint) =
            crate::tools::autonomy::large_ctx_shell_output_hint(&self.autonomy, &cmd, output_bytes)
        {
            *result_text = format!("{result_text}\n{hint}");
        }
        let calls = self.tool_calls.read().await;
        let last_original = calls.last().map_or(0, |c| c.original_tokens);
        drop(calls);
        if let Some(hint) = crate::tools::autonomy::shell_efficiency_hint(
            &self.autonomy,
            &cmd,
            last_original,
            output_token_count,
        ) {
            *result_text = format!("{result_text}\n{hint}");
        }
    }

    /// Post-dispatch: session recording, intent protocol, evidence ledger, cost attribution.
    pub(super) async fn record_session_and_evidence(
        &self,
        name: &str,
        args: Option<&Map<String, Value>>,
        result_text: &str,
        output_token_count: usize,
    ) {
        let input = helpers::canonical_args_string(args);
        let input_md5 = helpers::hash_fast(&input);
        let output_md5 = helpers::hash_fast(result_text);
        let action = helpers::get_str(args, "action");
        let agent_id = self.agent_id.read().await.clone();
        let client_name = self.client_name.read().await.clone();
        let mut explicit_intent: Option<(
            crate::core::intent_protocol::IntentRecord,
            Option<String>,
            String,
        )> = None;

        let pending_session_save = {
            let empty_args = serde_json::Map::new();
            let args_map = args.unwrap_or(&empty_args);
            let mut session = self.session.write().await;
            session.record_tool_receipt(
                name,
                action.as_deref(),
                &input_md5,
                &output_md5,
                agent_id.as_deref(),
                Some(&client_name),
            );

            if let Some(intent) = crate::core::intent_protocol::infer_from_tool_call(
                name,
                action.as_deref(),
                args_map,
                session.project_root.as_deref(),
            ) {
                let is_explicit =
                    intent.source == crate::core::intent_protocol::IntentSource::Explicit;
                let root = session.project_root.clone();
                let sid = session.id.clone();
                session.record_intent(intent.clone());
                if is_explicit {
                    explicit_intent = Some((intent, root, sid));
                }
            }
            if session.should_save() {
                session.prepare_save().ok()
            } else {
                None
            }
        };

        if let Some(prepared) = pending_session_save {
            tokio::task::spawn_blocking(move || {
                let _ = prepared.write_to_disk();
            });
        }

        {
            let name = name.to_string();
            let action = action.clone();
            let input_md5 = input_md5.clone();
            let output_md5 = output_md5.clone();
            let agent_id = agent_id.clone();
            let client_name = client_name.clone();
            tokio::task::spawn_blocking(move || {
                let ts = chrono::Utc::now();
                let mut ledger = crate::core::evidence_ledger::EvidenceLedgerV1::load();
                ledger.record_tool_receipt(
                    &name,
                    action.as_deref(),
                    &input_md5,
                    &output_md5,
                    agent_id.as_deref(),
                    Some(&client_name),
                    ts,
                );
                let _ = ledger.save();
            });
        }

        if let Some((intent, root, session_id)) = explicit_intent {
            let _ = crate::core::intent_protocol::apply_side_effects(
                &intent,
                root.as_deref(),
                &session_id,
            );
        }

        if self.autonomy.is_enabled() {
            let (calls, project_root) = {
                let session = self.session.read().await;
                (session.stats.total_tool_calls, session.project_root.clone())
            };

            if let Some(root) = project_root {
                if crate::tools::autonomy::should_auto_consolidate(&self.autonomy, calls) {
                    let root_clone = root.clone();
                    tokio::task::spawn_blocking(move || {
                        let _ = crate::core::consolidation_engine::consolidate_latest(
                            &root_clone,
                            crate::core::consolidation_engine::ConsolidationBudgets::default(),
                        );
                    });
                }
            }
        }

        let agent_key = agent_id.unwrap_or_else(|| "unknown".to_string());
        let input_token_count = crate::core::tokens::count_tokens(&input) as u64;
        let output_token_count_u64 = output_token_count as u64;
        let cached_tokens = args
            .and_then(|a| a.get("cached_tokens"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let name_owned = name.to_string();
        tokio::task::spawn_blocking(move || {
            let pricing = crate::core::gain::model_pricing::ModelPricing::load();
            let quote = pricing.quote_from_env_or_agent_type(&client_name);
            let cost_usd = quote.cost.estimate_usd(
                input_token_count,
                output_token_count_u64,
                0,
                cached_tokens,
            );
            crate::core::budget_tracker::BudgetTracker::global().record_cost_usd(cost_usd);

            let mut store = crate::core::a2a::cost_attribution::CostStore::load();
            store.record_tool_call(
                &agent_key,
                &client_name,
                &name_owned,
                input_token_count,
                output_token_count_u64,
                cached_tokens,
            );
            let _ = store.save();
        });
    }
}
