- **ID**: TD-20260417-models-monolith
- **Area**: Backend
- **Severity**: Medium (size-driven friction, no runtime cost)

## Problem (fact)
`backend/src/models/mod.rs` is ~2225 lines containing **147 structs/enums** + ~15 `fn default_*()` helpers across 21 logical domains. A single edit triggers a full recompile of every downstream module that imports `crate::models::*`. The file is the source of truth for ts-rs generation, so it can't be trimmed by moving types to their usage sites — everything has to stay visible under `crate::models`.

## Impact
- Compile-time friction: every model tweak recompiles ~3/4 of the backend.
- Merge-conflict magnet: 147 types in one file → frequent conflicts on feature branches.
- Navigation: hard to find the right struct for a given domain when doing model work.
- Review cost: PRs touching models get long diffs that mix unrelated domains.

## Why we can't fix now (constraint)
Mechanical split touches 147 types and scatter of `default_*` helpers. Too risky to bundle with other work — single typo in a `pub use` or misplaced `default_` fn cascades into 50+ compile errors. Needs a dedicated session with only this refactor.

## Where (pointers)
- `backend/src/models/mod.rs` — the monolith
- Every `use crate::models::{X, Y}` in the backend relies on the flat namespace
- `make typegen` (ts-rs) reads `#[derive(TS)]` by module resolution — sub-modules work as long as the barrel re-exports

## Suggested direction (non-binding)
Convert `models/mod.rs` into a `models/` directory with one sub-file per domain (existing `// ═══ Domain ═══` comment headers already delimit the boundaries precisely):

| Sub-module | Lines in current mod.rs | Key types |
|------------|------------------------|-----------|
| `config.rs` | 17-379 | AppConfig, ServerConfig, TokensConfig, ApiKey, ScanConfig, SetupStatus, SetupStep, ModelTier, AgentsConfig, AgentType, AgentDetection |
| `projects.rs` | 381-444 + 2041-2185 | Project, TokenOverride, AiConfigStatus, DetectedRepo, BootstrapProjectRequest, CloneProjectRequest, RemoteRepo, GitStatusResponse, GitDiff* |
| `audit.rs` | 447-603 + 1789-1876 | AiAuditStatus, AuditProgress, LaunchAuditRequest, AuditInfo, TechDebtItem, AuditFileInfo, AuditTodo, DriftCheckResponse, DriftSection, PartialAuditRequest |
| `mcps.rs` | 605-713 + 1972-1989 | McpServer, McpTransport, McpSource, McpConfig, McpConfigDisplay, McpEnvEntry, McpDefinition, McpContextEntry, McpIncompatibility, McpOverview |
| `workflows.rs` | 781-1139 | Workflow, WorkflowStep, WorkflowTrigger, NotifyConfig, StepType, StepMode, AgentSettings, StepConditionRule, ConditionAction, RetryConfig, WorkflowAction, WorkflowSafety, WorkspaceConfig, WorkspaceHooks, WorkflowRun, RunStatus, StepResult, WorkflowSummary, WorkflowRunSummary, WorkflowSuggestion |
| `quick_prompts.rs` | 715-778 | PromptVariable, QuickPrompt, CreateQuickPromptRequest |
| `agents_custom.rs` | 1141-1270 | Skill, SkillCategory, CreateSkillRequest, AgentProfile, ProfileCategory, CreateProfileRequest, Directive, DirectiveCategory, CreateDirectiveRequest |
| `ai_docs.rs` | 1273-1298 | AiFileNode, AiFileContent, AiSearchResult |
| `stats.rs` | 1301-1376 | TokenUsageSummary, ProviderUsage, ProjectUsage, DailyUsage, AgentUsageSummary, AgentProjectUsage |
| `contacts.rs` | 1379-1423 | Contact, DetectedIp, NetworkInfo |
| `ws.rs` | 1426-1496 | WsMessage (big enum with ~15 variants) |
| `discussions.rs` | 1499-1594 + 2186-end | Discussion, DiscussionMessage, MessageRole, ContextFile |
| `api.rs` | 1595-1788 | CreateDiscussionRequest, UpdateDiscussionRequest, SendMessageRequest, OrchestrationRequest, …all HTTP DTOs |
| `ollama.rs` | 1877-1971 | OllamaStatus, OllamaModel, …Ollama health/listing |
| `db.rs` | 1990-2040 | DbInfo, DbExport, ImportWorkflowRequest |

### `mod.rs` after split
```rust
mod config;
mod projects;
mod audit;
// …
pub use config::*;
pub use projects::*;
pub use audit::*;
// …

// Shared helpers used across modules (keep here):
pub(crate) fn deserialize_optional_field<'de, D>(…) -> …
pub(crate) fn default_true() -> bool { true }
pub(crate) fn default_language() -> String { … }
```

### Helpers to keep in `mod.rs`
`default_true`, `default_language`, `default_ui_language`, `deserialize_optional_field` — used across multiple domains. Everything else (`default_global_context_mode`, `default_max_agents`, etc.) moves with its owning struct.

### Safety
- `#[derive(TS)]` does not care about sub-modules as long as each file has `use ts_rs::TS;` — the `#[ts(export)]` attribute writes to `backend/bindings/<TypeName>.ts` regardless of module path.
- Every existing `use crate::models::Foo` keeps working thanks to `pub use * from sub`.
- `make typegen` must produce the same output after split — diff `backend/bindings/` before/after to confirm.

## Next step
Dedicated session (~45 min). Plan:
1. Create empty sub-files with copied section ranges.
2. Rewrite `mod.rs` as barrel.
3. `cargo check` — fix any missing `pub(crate)` on shared helpers.
4. `cargo test --lib` — expect 0 behavioural diff.
5. `make typegen` + diff `backend/bindings/` — expect identical output.
6. Update `ai/repo-map.md` and this TD entry status to FIXED.
