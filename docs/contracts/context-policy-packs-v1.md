# Context Policy Packs v1 (GL #489)

Declarative, versioned governance presets — "Context-Policies as Code". A pack
pins a team's context-governance expectations in reviewable TOML: default read
mode, allowed/denied tools, redaction patterns, an audit-retention expectation
and a context-budget cap. The reduced, solo-viable slice of #377/#403/#404.

v1 ships the **format, validation, resolution, five curated built-ins and the
`lean-ctx policy` CLI**; **runtime enforcement is wired as of #673** (see
*Enforcement*). Pack signing and central org distribution remain explicit
follow-ups (see *Out of scope*).

## Format

A pack is one TOML file. The project pack lives at `.lean-ctx/policy.toml`.

```toml
name = "acme-internal"          # lowercase letters, digits, hyphens
version = "1.0.0"               # MAJOR.MINOR.PATCH (digits only)
description = "ACME engineering baseline"
extends = "strict-redaction"    # optional: single inheritance, built-in parent

[context]                       # all fields optional
default_read_mode = "map"       # auto|full|map|signatures|diff|task|reference|aggressive|entropy
allow_tools = ["ctx_read", "ctx_search"]   # when set: only these
deny_tools = ["ctx_url_read"]   # always additive down the chain
max_context_tokens = 12000      # > 0
audit_retention_days = 365      # governance intent (hosted plane enforces its plan window)

[redaction]                     # name -> regex, matched before content enters context
employee_id = 'EMP-\d{6}'
```

Unknown keys are **rejected** (`deny_unknown_fields`) so a typo like
`alow_tools` fails validation instead of silently weakening a policy.

## Inheritance (`extends`)

Single inheritance against the built-in registry, max depth 8, cycles
rejected. Semantics are security-first and predictable:

| Field | Rule |
|---|---|
| `default_read_mode`, `max_context_tokens`, `audit_retention_days` | child **overrides** when set |
| `deny_tools` | **accumulates** (parent restrictions can never be dropped) |
| `[redaction]` | **accumulates**; a child entry with the same name re-points that pattern |
| `allow_tools` | child **overrides** when set (an allowlist is a posture choice, not a set union) |

After folding, a resolved `allow_tools` colliding with an accumulated deny is
an error (`AllowDenyOverlap`) — a pack cannot both allow and deny a tool.

## Built-in packs

| Pack | Extends | Posture |
|---|---|---|
| `baseline` | — | secret redaction (PEM keys, AWS, credential assignments, bearer tokens), `auto` mode, 90-day audit expectation |
| `strict-redaction` | baseline | + JWT/GitHub/GitLab/Slack/OpenAI/Anthropic/Stripe/DB-URL coverage, `map` mode, 180 days |
| `finance-eu` | strict-redaction | + IBAN/payment-card/EU-VAT/SWIFT, denies `ctx_url_read`, 12 k token cap, 365 days |
| `healthcare` | strict-redaction | + SSN/MRN/member-id/DOB/NPI (HIPAA-aligned), denies `ctx_url_read`, 12 k cap, 2 190 days |
| `open-source` | baseline | permissive, keeps secret coverage, 30 days |

Built-ins are embedded at compile time (`include_str!`) and covered by tests:
every pack must parse, validate, resolve and retain the baseline secret
coverage; the regulated packs must deny web fetches and pin budgets.

## CLI

```
lean-ctx policy list                  # built-ins + project pack (if any)
lean-ctx policy show <name> [--toml]  # resolved effective policy / raw TOML
lean-ctx policy show project          # the .lean-ctx/policy.toml pack
lean-ctx policy show ./custom.toml    # any pack file
lean-ctx policy validate [path]       # lint (default .lean-ctx/policy.toml); exit 1 on INVALID
lean-ctx policy coverage [name] [--benchmark cgb] [--json]
                                      # automated PARTIAL CGB assessment; exit 1 on any FAIL
```

`coverage` statically checks a resolved pack against the Context Governance
Benchmark v1.0-draft: credential fixtures vs. redaction patterns (CGB-1.1),
named declarative rules (1.2), regulated-identifier classes (1.3), budget
cap (3.2), retention expectation (4.3), tool posture (5.4), egress
restriction (5.5). It prints PASS/FAIL/INCONCLUSIVE per aspect and an
honesty line — never a maturity grade (7 of 32 controls are statically
checkable; see `docs/compliance/cgb-self-assessment.md`).

`show --toml` prints the **unresolved** pack definition — the natural starting
point for an org-specific pack:

```
lean-ctx policy show baseline --toml > .lean-ctx/policy.toml
```

## Error vocabulary

`PolicyError` names the offending field and value; the CLI prints it verbatim:
`Toml`, `InvalidName`, `InvalidVersion`, `EmptyDescription`,
`UnknownReadMode`, `BadRegex{pattern_name}`, `ZeroMaxTokens`,
`AllowDenyOverlap`, `UnknownParent`, `ExtendsCycle`, `ExtendsTooDeep`.

## Enforcement (#673)

The resolved project pack (`.lean-ctx/policy.toml`) is applied at the MCP
server hot path. Enforcement is **opt-in**: with no project pack present nothing
is gated and behavior is identical to a pack-less install.

| Field | Where it is enforced | Effect |
|---|---|---|
| `deny_tools` / `allow_tools` | `server::policy_guard` in `call_tool_guarded`, right after the role guard | a denied tool returns a `[POLICY DENIED]` result and is audited (`ToolDenied`); an `allow_tools` allowlist is exclusive |
| `[redaction]` | `call_tool_guarded`, before the result reaches the model and the out-of-band archive | each match becomes `[REDACTED:<name>]`, on top of the built-in secret rules |
| `default_read_mode` | `ctx_read`, only when the caller omits `mode` | the pack default replaces auto/profile selection (an explicit `mode` always wins; line windows may still narrow it) |
| `max_context_tokens` | `core::budget_tracker::check` | tightens (never loosens) the per-session token ceiling; the agent hits the normal budget warning/exhausted path |

Invariants:

- **No self-lockout** — the meta tools `ctx`, `ctx_session`, `ctx_policy` can
  never be policy-denied, so an operator can always switch policy back out.
- **Fail-open on a broken pack** — an invalid `.lean-ctx/policy.toml` is logged
  and ignored (no enforcement), never bricking the agent; `lean-ctx policy
  validate` surfaces the same error.
- **Local-Free Invariant** — enforcement only constrains the *agent* pipeline
  (the tools the model calls); it never gates a human's own local reads or CLI.
- The active pack is loaded once and cached (`core::policy::runtime`); call
  `runtime::reload()` after editing the pack.

## Out of scope (follow-ups)

1. **Central signed org policy distribution + admin** (#674) — v1 enforcement
   (#673) reads a *project-local* pack only; org-wide rollout and tamper-evident
   signing land next.
2. **Signing + trust pipeline**, registry/marketplace distribution (#403/MKT).
3. **Conformance scoring against live telemetry** — `policy coverage` (v1) is
   static pack analysis. Runtime evidence is now *emitted* (denials audited as
   `ToolDenied`, redaction counts logged); aggregating it into a score is the
   follow-up.
4. Multi-file packs, non-built-in parents (`extends` against local files).

## Module map

| Piece | Path |
|---|---|
| Types, parse, validate, resolve | `rust/src/core/policy/mod.rs` |
| Runtime view (load + cache active pack) | `rust/src/core/policy/runtime.rs` |
| Server-side tool gating + redaction | `rust/src/server/policy_guard.rs` |
| CGB coverage checks | `rust/src/core/policy/coverage.rs` |
| Built-in registry | `rust/src/core/policy/builtin.rs` |
| Built-in pack sources | `rust/src/core/policy/builtin/*.toml` |
| CLI | `rust/src/cli/policy_cmd.rs` (dispatch key `policy`) |
| Authoring guide | `docs/guides/policy-packs.md` |
