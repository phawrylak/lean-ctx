use std::collections::HashMap;
use std::sync::OnceLock;

static BUNDLED_REGISTRY: &str = include_str!("../../data/model_registry.json");

static PARSED_BUNDLED: OnceLock<Registry> = OnceLock::new();
static PARSED_LOCAL: OnceLock<Option<Registry>> = OnceLock::new();

#[derive(Debug, Clone)]
struct ModelEntry {
    context_window: usize,
}

#[derive(Debug, Clone, Default)]
struct Registry {
    models: HashMap<String, ModelEntry>,
    families: HashMap<String, usize>,
}

fn parse_registry(json: &str) -> Option<Registry> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let mut models = HashMap::new();
    if let Some(obj) = v.get("models").and_then(|m| m.as_object()) {
        for (key, entry) in obj {
            if let Some(window) = entry
                .get("context_window")
                .and_then(serde_json::Value::as_u64)
            {
                models.insert(
                    key.to_lowercase(),
                    ModelEntry {
                        context_window: window as usize,
                    },
                );
            }
        }
    }
    let mut families = HashMap::new();
    if let Some(obj) = v.get("families").and_then(|f| f.as_object()) {
        for (key, val) in obj {
            if let Some(window) = val.as_u64() {
                families.insert(key.to_lowercase(), window as usize);
            }
        }
    }
    Some(Registry { models, families })
}

fn bundled() -> &'static Registry {
    PARSED_BUNDLED.get_or_init(|| parse_registry(BUNDLED_REGISTRY).unwrap_or_default())
}

fn local_registry() -> Option<&'static Registry> {
    PARSED_LOCAL
        .get_or_init(|| {
            let data_dir = crate::core::data_dir::lean_ctx_data_dir().ok()?;
            let path = data_dir.join("model_registry.json");
            let content = std::fs::read_to_string(path).ok()?;
            parse_registry(&content)
        })
        .as_ref()
}

fn user_config_override(model: &str) -> Option<usize> {
    let cfg = crate::core::config::Config::load();
    cfg.model_context_windows
        .get(model)
        .or_else(|| cfg.model_context_windows.get(&model.to_lowercase()))
        .copied()
}

/// Exact, then delimiter-anchored prefix match (e.g. "gpt-5.5-0513" → "gpt-5.5").
fn exact_or_prefix(m: &str, registry: &Registry) -> Option<usize> {
    // Exact match
    if let Some(entry) = registry.models.get(m) {
        return Some(entry.context_window);
    }

    // Prefix match: "gpt-5.5-0513" should match "gpt-5.5"
    let mut best_match: Option<(usize, usize)> = None; // (key_len, window)
    for (key, entry) in &registry.models {
        if m.starts_with(key.as_str()) && m[key.len()..].starts_with(['-', '_', '.'])
            || m == key.as_str()
        {
            let key_len = key.len();
            if best_match.is_none_or(|(bl, _)| key_len > bl) {
                best_match = Some((key_len, entry.context_window));
            }
        }
    }
    best_match.map(|(_, w)| w)
}

/// '-'-delimited tokens, sorted — a multiset key for order-independent comparison.
fn sorted_tokens(s: &str) -> Vec<&str> {
    let mut t: Vec<&str> = s.split('-').filter(|p| !p.is_empty()).collect();
    t.sort_unstable();
    t
}

/// Match a Claude id regardless of element order. The family/version order flipped
/// between generations (version-first `claude-3-5-sonnet` vs family-first
/// `claude-opus-4-8`), so e.g. `claude-4-6-opus` should still resolve to
/// `claude-opus-4-6`. Every Claude key has a distinct token multiset, so a match is
/// unambiguous. Caller guarantees `m` starts with "claude".
fn claude_reordered_match(m: &str, registry: &Registry) -> Option<usize> {
    let want = sorted_tokens(m);
    registry
        .models
        .iter()
        .find(|(key, _)| key.starts_with("claude") && sorted_tokens(key) == want)
        .map(|(_, entry)| entry.context_window)
}

fn registry_lookup(model: &str, registry: &Registry) -> Option<usize> {
    let m = model.to_lowercase();

    if let Some(window) = exact_or_prefix(&m, registry) {
        return Some(window);
    }

    // Two robustness passes for Claude, beyond the canonical exact/prefix match above.
    // Scoped to "claude" so GPT/Gemini keys that legitimately use dots (gpt-4.1,
    // gemini-1.5-pro) are left untouched.
    if m.starts_with("claude") {
        // 1. Dotted variants ("claude-opus-4.8") — normalize dots to hyphens. Keeps
        //    prefix matching for dotted dated snapshots ("claude-opus-4.8-20260601").
        let normalized = m.replace('.', "-");
        if normalized != m
            && let Some(window) = exact_or_prefix(&normalized, registry)
        {
            return Some(window);
        }
        // 2. Element order flipped between generations — accept either ordering
        //    ("claude-4-6-opus" == "claude-opus-4-6") via token-multiset match.
        if let Some(window) = claude_reordered_match(&normalized, registry) {
            return Some(window);
        }
    }

    // Family match (substring)
    let mut best_family: Option<(usize, usize)> = None;
    for (family, window) in &registry.families {
        if m.contains(family.as_str()) {
            let flen = family.len();
            if best_family.is_none_or(|(bl, _)| flen > bl) {
                best_family = Some((flen, *window));
            }
        }
    }
    best_family.map(|(_, w)| w)
}

/// Look up context window for a model name.
/// Layers: User Config → Local Registry → Bundled Registry → 200k default.
pub fn context_window_for_model(model: &str) -> usize {
    // Layer 1: User config override
    if let Some(w) = user_config_override(model) {
        return w;
    }

    // Layer 2: Local registry (auto-updated via lean-ctx update)
    if let Some(local) = local_registry()
        && let Some(w) = registry_lookup(model, local)
    {
        return w;
    }

    // Layer 3: Bundled registry (compiled into binary)
    if let Some(w) = registry_lookup(model, bundled()) {
        return w;
    }

    // Fallback
    200_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_registry_parses() {
        let reg = bundled();
        assert!(!reg.models.is_empty());
        assert!(!reg.families.is_empty());
    }

    #[test]
    fn exact_match_gpt55() {
        assert_eq!(context_window_for_model("gpt-5.5"), 1_048_576);
    }

    #[test]
    fn prefix_match_gpt55_variant() {
        assert_eq!(context_window_for_model("gpt-5.5-0513"), 1_048_576);
    }

    #[test]
    fn exact_match_gpt41() {
        assert_eq!(context_window_for_model("gpt-4.1"), 1_047_576);
    }

    #[test]
    fn family_match_gpt5() {
        assert_eq!(context_window_for_model("gpt-5.3-turbo"), 128_000);
    }

    #[test]
    fn family_match_claude() {
        assert_eq!(context_window_for_model("claude-unknown-version"), 200_000);
    }

    #[test]
    fn family_match_gemini() {
        assert_eq!(context_window_for_model("gemini-future-model"), 1_048_576);
    }

    #[test]
    fn unknown_model_returns_default() {
        assert_eq!(
            context_window_for_model("totally-unknown-model-xyz"),
            200_000
        );
    }

    // Regression: the modern Claude family must report its real 1M window, not
    // the stale 200k that the old reversed-convention / prefix-trap keys forced.
    #[test]
    fn claude_opus_48_is_1m() {
        assert_eq!(context_window_for_model("claude-opus-4-8"), 1_000_000);
    }

    #[test]
    fn claude_opus_46_is_1m() {
        assert_eq!(context_window_for_model("claude-opus-4-6"), 1_000_000);
    }

    #[test]
    fn claude_sonnet_46_is_1m() {
        assert_eq!(context_window_for_model("claude-sonnet-4-6"), 1_000_000);
    }

    #[test]
    fn claude_fable_5_is_1m() {
        assert_eq!(context_window_for_model("claude-fable-5"), 1_000_000);
    }

    #[test]
    fn claude_haiku_45_is_200k() {
        assert_eq!(context_window_for_model("claude-haiku-4-5"), 200_000);
    }

    // 4-5 must stay 200k even though 4-8/4-6 are now 1M (no prefix bleed).
    #[test]
    fn claude_opus_45_is_200k() {
        assert_eq!(context_window_for_model("claude-opus-4-5"), 200_000);
    }

    // Dated/snapshot variants resolve via prefix match to the base id.
    #[test]
    fn claude_opus_48_dated_variant_is_1m() {
        assert_eq!(
            context_window_for_model("claude-opus-4-8-20260601"),
            1_000_000
        );
    }

    // Dotted, non-canonical Claude ids normalize to their hyphenated key.
    // family-first ordering (4.x): "claude-opus-4.8" → "claude-opus-4-8".
    #[test]
    fn claude_opus_48_dotted_is_1m() {
        assert_eq!(context_window_for_model("claude-opus-4.8"), 1_000_000);
    }

    #[test]
    fn claude_sonnet_46_dotted_is_1m() {
        assert_eq!(context_window_for_model("claude-sonnet-4.6"), 1_000_000);
    }

    #[test]
    fn claude_haiku_45_dotted_is_200k() {
        assert_eq!(context_window_for_model("claude-haiku-4.5"), 200_000);
    }

    // version-first ordering (3.x): "claude-3.5-sonnet" → "claude-3-5-sonnet".
    #[test]
    fn claude_35_sonnet_dotted_is_200k() {
        assert_eq!(context_window_for_model("claude-3.5-sonnet"), 200_000);
    }

    // Dot normalization is scoped to claude: GPT keys that legitimately use dots
    // must NOT be hyphen-normalized (gpt-4.1 stays gpt-4.1, not gpt-4-1).
    #[test]
    fn gpt_41_dotted_unaffected_by_claude_normalization() {
        assert_eq!(context_window_for_model("gpt-4.1"), 1_047_576);
    }

    // Order-independent: element order flipped between generations, so a reversed
    // ordering must still resolve. family-first id written version-first:
    // "claude-4-8-opus" == "claude-opus-4-8".
    #[test]
    fn claude_opus_48_reversed_order_is_1m() {
        assert_eq!(context_window_for_model("claude-4-8-opus"), 1_000_000);
    }

    // version-first id written family-first: "claude-sonnet-3-5" == "claude-3-5-sonnet".
    #[test]
    fn claude_35_sonnet_reversed_order_is_200k() {
        assert_eq!(context_window_for_model("claude-sonnet-3-5"), 200_000);
    }

    // The old registry's own convention (reversed AND dotted) now resolves correctly:
    // "claude-4.6-opus" -> normalize dots -> reorder -> "claude-opus-4-6" -> 1M.
    #[test]
    fn claude_46_opus_reversed_dotted_is_1m() {
        assert_eq!(context_window_for_model("claude-4.6-opus"), 1_000_000);
    }
}
