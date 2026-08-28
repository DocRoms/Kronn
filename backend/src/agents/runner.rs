use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use crate::core::cmd::{async_cmd, sync_cmd};
use crate::models::{AgentType, ModelTier, ModelTiersConfig, TokensConfig};

const MAX_CALLS_PER_TOOL: usize = 12;
// Repository search is not an API probe. A real KT-404 worker needed thirteen
// distinct symbol searches to follow handoff callers, worktree persistence and
// test helpers; the generic twelve-call guard cut that honest trajectory off.
// Keep the larger allowance worker-only so a general/API agent still gets the
// stricter anti-loop policy.
const MAX_WORKER_SEARCH_TEXT_CALLS: usize = 24;
// 48, raised from 24 after a real delegation died of it: a task that crosses
// an 11 000-line file needed ~30 honest 120-line slices, spent the budget
// mid-exploration, and the run ended with no edit. The cap is a backstop now,
// not the primary guard — identical repeats, same-answer digests and error
// circuits catch an actual loop long before 48 reads do.
pub(crate) const MAX_READ_FILE_CALLS: usize = 48;
const MAX_ERRORS_PER_TOOL: usize = 3;
const MAX_ERROR_ONLY_TOOL_ROUNDS: usize = 6;
const WORKER_EXPLORATION_NUDGE_AT: usize = 24;
const WORKER_FINALIZATION_ITERATIONS: usize = 12;
const WORKER_FINALIZATION_READ_FILE_CALLS: usize = 3;
const WORKER_FINALIZATION_GIT_INSPECTION_CALLS: usize = 3;
// A checkpoint must retain enough recent tool trajectory for the model to
// continue its implementation plan. Resetting all the way to the brief was
// dogfooded and reverted in KT-407: the model went back to withdrawn
// exploration tools. Keep the same three-turn correction budget granted to a
// schema-invalid edit; on the exploration boundary the current call/result is
// appended after the checkpoint, yielding one additional live round.
const WORKER_CHECKPOINT_TOOL_ROUNDS: usize = WORKER_REPAIR_EDIT_ITERATIONS;
// Re-prefilling a 65K MLX slot on every finalization turn is the failure this
// checkpoint prevents. Trim the retained tail against a small phase-local
// target, then add explicit reply/tool headroom below.
const WORKER_FINALIZATION_CTX_TARGET: u64 = 16_384;
const WORKER_FINALIZATION_REPLY_HEADROOM: u64 = 4_096;
// One remembered tool call must not consume the only chance to execute the
// required repair read. Two responses allow exactly one corrective retry; the
// first successful read immediately advances the state, so at most one read is
// ever executed by this phase.
const WORKER_REPAIR_READ_ITERATIONS: usize = 2;
// Editing needs one more response than reading: a model may first repeat the
// now-withdrawn read, then make one schema-invalid edit, and must receive that
// executor error once to correct it. The first successful mutation advances
// immediately, so this never executes more edits after success.
const WORKER_REPAIR_EDIT_ITERATIONS: usize = 3;
const WORKER_REPAIR_COMMIT_ITERATIONS: usize = 3;
const WORKER_DELIVERY_ITERATIONS: usize = 3;
// A worker response such as "I'll inspect that now" is an intention, not a
// terminal result. Give it one corrective turn with the current bounded tool
// catalogue, then fail explicitly instead of accepting prose without a commit
// and DeliveryManifest or spending an open-ended local-model loop.
const WORKER_PROSE_ONLY_ITERATIONS: usize = 2;
// Ollama's MLX engine in 0.32.14 does not reuse the shared prompt prefix
// between `/api/chat` turns (ollama/ollama#17829) and retains cache memory
// across requests (#17875). A real qwen3.8:27b-mlx worker stayed fast through
// turn 32, then each observation grew to 1-6 minutes. Give that engine an
// earlier delivery boundary; this is a documented mitigation, not a claim
// that every MLX implementation has the same limitation.
const MLX_WORKER_EXPLORATION_ITERATIONS: usize = 32;
// A scoped local subtask must turn repository evidence into a mutation instead
// of paying for an open-ended repository tour. V15 of KT-404 had found the
// named function after two observations but reached eighteen without changing
// the workspace. Bound that exact signal for MLX only; finalization still
// grants three exact CAS refreshes before its one-shot repair path.
const MLX_WORKER_MAX_OBSERVATIONS_WITHOUT_MUTATION: usize = 12;
// The original 50% boundary was introduced while native MLX workers could
// still allocate the configured 65K slot: it moved finalization near 32K. Now
// that the whole run is capped at 32K, retaining 50% would compound the two
// mitigations and narrow the catalogue near 16K — a real KT-410 run crossed it
// on its very first tool call. Keep 25% for reply/tool-result headroom instead.
const MLX_WORKER_CONTEXT_PRESSURE_PERCENT: u64 = 75;
const DEFAULT_WORKER_CONTEXT_PRESSURE_PERCENT: u64 = 75;
// MLX fixes the slot when the model is loaded: a later request for 16K still
// used a live 8K slot, while a cold 32K request loaded the real qwen3.8 model
// in under eight seconds. The nominal 65K slot instead cost 31 GB and produced
// no first tool call after seventeen minutes. Keep exploration below that
// pathological tier; finalization still compacts its actual prompt to 16K.
const MLX_WORKER_EFFECTIVE_CTX_CAP: u64 = 32_768;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkerExplorationPolicy {
    max_iterations: usize,
    max_observations_without_mutation: Option<usize>,
    context_pressure_percent: u64,
    mlx_mitigation: bool,
    mlx_detection_source: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerExplorationBoundary {
    RoundLimit,
    ObservationLimit,
    ContextPressure(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerRepairStage {
    Inactive,
    Read,
    Edit,
    Commit,
}

impl WorkerRepairStage {
    fn iteration_limit(self) -> usize {
        match self {
            Self::Inactive => 0,
            Self::Read => WORKER_REPAIR_READ_ITERATIONS,
            Self::Edit => WORKER_REPAIR_EDIT_ITERATIONS,
            Self::Commit => WORKER_REPAIR_COMMIT_ITERATIONS,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Inactive => "inactive",
            Self::Read => "repair read",
            Self::Edit => "repair edit",
            Self::Commit => "repair commit",
        }
    }
}

fn http_turn_phase(
    worker_run: bool,
    prelocalized_worker: bool,
    repair_stage: WorkerRepairStage,
    delivery_phase: bool,
    finalization_phase: bool,
) -> crate::models::TaskExecutionHttpPhase {
    use crate::models::TaskExecutionHttpPhase;
    if delivery_phase {
        TaskExecutionHttpPhase::Delivery
    } else if prelocalized_worker {
        match repair_stage {
            WorkerRepairStage::Read => TaskExecutionHttpPhase::Read,
            WorkerRepairStage::Edit => TaskExecutionHttpPhase::Mutation,
            WorkerRepairStage::Commit => TaskExecutionHttpPhase::Commit,
            WorkerRepairStage::Inactive if finalization_phase => {
                TaskExecutionHttpPhase::Finalization
            }
            WorkerRepairStage::Inactive => TaskExecutionHttpPhase::Answer,
        }
    } else if worker_run && finalization_phase {
        TaskExecutionHttpPhase::Finalization
    } else if worker_run {
        TaskExecutionHttpPhase::Exploration
    } else {
        TaskExecutionHttpPhase::Answer
    }
}

fn ollama_model_has_mlx_tag(model: &str) -> bool {
    let tag = model.rsplit_once(':').map_or(model, |(_, tag)| tag);
    tag.split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|part| part.eq_ignore_ascii_case("mlx"))
}

fn worker_exploration_policy(
    model: &str,
    storage_format: Option<&str>,
    is_openai_wire: bool,
) -> WorkerExplorationPolicy {
    // A custom Ollama alias can hide the original `-mlx` tag. The native MLX
    // artefacts currently exposed by `/api/show` use safetensors, whereas the
    // classic llama.cpp path reports GGUF. Keep the tag as a fail-safe when an
    // older Ollama response omits `details.format`.
    let mlx_detection_source = if is_openai_wire {
        None
    } else if storage_format.is_some_and(|format| format.eq_ignore_ascii_case("safetensors")) {
        Some("model_format")
    } else if ollama_model_has_mlx_tag(model) {
        Some("model_tag_fallback")
    } else {
        None
    };
    let mlx_mitigation = mlx_detection_source.is_some();
    if mlx_mitigation {
        WorkerExplorationPolicy {
            max_iterations: MLX_WORKER_EXPLORATION_ITERATIONS,
            max_observations_without_mutation: Some(MLX_WORKER_MAX_OBSERVATIONS_WITHOUT_MUTATION),
            context_pressure_percent: MLX_WORKER_CONTEXT_PRESSURE_PERCENT,
            mlx_mitigation,
            mlx_detection_source,
        }
    } else {
        WorkerExplorationPolicy {
            max_iterations: crate::agents::tools::MAX_TOOL_ITERATIONS,
            max_observations_without_mutation: None,
            context_pressure_percent: DEFAULT_WORKER_CONTEXT_PRESSURE_PERCENT,
            mlx_mitigation,
            mlx_detection_source,
        }
    }
}

fn worker_effective_ctx_cap(
    configured_ctx_cap: u64,
    tool_run_mode: crate::agents::tools::ToolRunMode,
    policy: WorkerExplorationPolicy,
) -> u64 {
    if tool_run_mode == crate::agents::tools::ToolRunMode::Worker && policy.mlx_mitigation {
        configured_ctx_cap.min(MLX_WORKER_EFFECTIVE_CTX_CAP)
    } else {
        configured_ctx_cap
    }
}

fn worker_oversized_prompt_remedy(
    configured_ctx_cap: u64,
    policy: WorkerExplorationPolicy,
) -> &'static str {
    if policy.mlx_mitigation && configured_ctx_cap >= MLX_WORKER_EFFECTIVE_CTX_CAP {
        "reduce the task/tool surface, or use a GGUF/non-MLX model; native MLX workers are intentionally capped at 32768 tokens"
    } else if policy.mlx_mitigation {
        "increase the configured context cap up to 32768 tokens, reduce the task/tool surface, or use a GGUF/non-MLX model"
    } else {
        "increase the configured context cap or reduce the task/tool surface"
    }
}

fn worker_exploration_boundary(
    policy: WorkerExplorationPolicy,
    turn: usize,
    observations_without_mutation: usize,
    context_pressure_tokens: Option<u64>,
) -> Option<WorkerExplorationBoundary> {
    if let Some(tokens) = context_pressure_tokens {
        Some(WorkerExplorationBoundary::ContextPressure(tokens))
    } else if policy
        .max_observations_without_mutation
        .is_some_and(|limit| observations_without_mutation >= limit)
    {
        Some(WorkerExplorationBoundary::ObservationLimit)
    } else if turn >= policy.max_iterations {
        Some(WorkerExplorationBoundary::RoundLimit)
    } else {
        None
    }
}

fn estimated_chat_history_tokens(body: &serde_json::Value) -> u64 {
    let mut wire_bytes = body["messages"].to_string().len() as u64;
    for field in ["tools", "format"] {
        if let Some(value) = body.get(field) {
            wire_bytes = wire_bytes.saturating_add(value.to_string().len() as u64);
        }
    }
    (wire_bytes / 3) + 2048
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkerHistoryCheckpoint {
    before_messages: usize,
    after_messages: usize,
    before_tokens: u64,
    after_tokens: u64,
    seed_messages: usize,
    tail_messages: usize,
    compacted_tool_results: usize,
    final_num_ctx: u64,
}

#[derive(Debug, Clone)]
struct WorkerCheckpointSeed {
    messages: Vec<serde_json::Value>,
    source_message_count: usize,
}

impl WorkerCheckpointSeed {
    fn from_body(body: &serde_json::Value) -> Self {
        let messages = body["messages"].as_array().cloned().unwrap_or_default();
        let first_user = messages
            .iter()
            .position(|message| message["role"] == "user");
        let last_user = messages
            .iter()
            .rposition(|message| message["role"] == "user");
        let mut retained_indices = messages
            .iter()
            .enumerate()
            .filter_map(|(index, message)| (message["role"] == "system").then_some(index))
            .collect::<Vec<_>>();
        retained_indices.extend(first_user);
        retained_indices.extend(last_user);
        retained_indices.sort_unstable();
        retained_indices.dedup();
        Self {
            messages: retained_indices
                .into_iter()
                .filter_map(|index| messages.get(index).cloned())
                .collect(),
            source_message_count: messages.len(),
        }
    }
}

fn recent_worker_protocol_tail(
    messages: &[serde_json::Value],
    source_message_count: usize,
    available_tools: &std::collections::HashSet<String>,
) -> Vec<serde_json::Value> {
    let runtime = messages
        .get(source_message_count.min(messages.len())..)
        .unwrap_or_default();
    let mut compatible_rounds = Vec::<Vec<serde_json::Value>>::new();
    let mut index = 0;
    while index < runtime.len() {
        let Some(calls) = runtime[index]["tool_calls"]
            .as_array()
            .filter(|calls| runtime[index]["role"] == "assistant" && !calls.is_empty())
        else {
            index += 1;
            continue;
        };
        let call_ids = calls
            .iter()
            .filter_map(|call| call["id"].as_str())
            .collect::<std::collections::HashSet<_>>();
        let calls_are_available = calls.iter().all(|call| {
            call.pointer("/function/name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| available_tools.contains(name))
        });
        let mut end = index + 1;
        while end < runtime.len() && runtime[end]["role"] == "tool" {
            end += 1;
        }
        let results_are_correlated = !call_ids.is_empty()
            && runtime[index + 1..end].iter().all(|result| {
                result["tool_call_id"]
                    .as_str()
                    .is_some_and(|id| call_ids.contains(id))
            })
            && call_ids.iter().all(|id| {
                runtime[index + 1..end]
                    .iter()
                    .any(|result| result["tool_call_id"] == **id)
            });
        if calls_are_available && results_are_correlated {
            compatible_rounds.push(runtime[index..end].to_vec());
        }
        index = end;
    }
    compatible_rounds
        .into_iter()
        .rev()
        .take(WORKER_CHECKPOINT_TOOL_ROUNDS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .flatten()
        .collect()
}

const WORKER_CHECKPOINT_TOOL_RESULT_BYTES: usize = 4_096;
const WORKER_CHECKPOINT_EXCERPT_HEAD_BYTES: usize = 1_024;
const WORKER_CHECKPOINT_EXCERPT_TAIL_BYTES: usize = 256;
const WORKER_CHECKPOINT_MAX_FACTS: usize = 24;
const WORKER_CHECKPOINT_MAX_EXCERPTS: usize = 4;
const WORKER_CHECKPOINT_MAX_VISITED_VALUES: usize = 256;
const WORKER_CHECKPOINT_PRIORITY_FIELDS: &[&str] = &[
    "content_sha256",
    "previous_sha256",
    "path",
    "offset",
    "limit",
    "start_line",
    "end_line",
    "next_offset",
    "truncated",
    "found",
];

fn checkpoint_text_excerpt(text: &str) -> String {
    if text.len() <= WORKER_CHECKPOINT_EXCERPT_HEAD_BYTES + WORKER_CHECKPOINT_EXCERPT_TAIL_BYTES {
        return text.to_string();
    }
    let mut head_end = WORKER_CHECKPOINT_EXCERPT_HEAD_BYTES.min(text.len());
    while head_end > 0 && !text.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = text
        .len()
        .saturating_sub(WORKER_CHECKPOINT_EXCERPT_TAIL_BYTES);
    while tail_start < text.len() && !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    format!(
        "{}\n[... {} bytes omitted by Kronn checkpoint ...]\n{}",
        &text[..head_end],
        tail_start.saturating_sub(head_end),
        &text[tail_start..]
    )
}

fn collect_checkpoint_json_facts(
    value: &serde_json::Value,
    path: &str,
    facts: &mut Vec<serde_json::Value>,
    excerpts: &mut Vec<serde_json::Value>,
    visited: &mut usize,
) {
    if *visited >= WORKER_CHECKPOINT_MAX_VISITED_VALUES {
        return;
    }
    *visited += 1;
    match value {
        serde_json::Value::Object(map) => {
            // Receipts and workspace coordinates are protocol state, not an
            // arbitrary sample. Visit them first so a wide object cannot use
            // the bounded fact ledger before its CAS receipt is recorded.
            for key in WORKER_CHECKPOINT_PRIORITY_FIELDS {
                if let Some(child) = map.get(*key) {
                    let child_path = if path.is_empty() {
                        key.to_string()
                    } else {
                        format!("{path}.{key}")
                    };
                    collect_checkpoint_json_facts(child, &child_path, facts, excerpts, visited);
                }
            }
            for (key, child) in map {
                if WORKER_CHECKPOINT_PRIORITY_FIELDS.contains(&key.as_str()) {
                    continue;
                }
                let child_path = if path.is_empty() {
                    key.to_string()
                } else {
                    format!("{path}.{key}")
                };
                collect_checkpoint_json_facts(child, &child_path, facts, excerpts, visited);
            }
        }
        serde_json::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_checkpoint_json_facts(
                    child,
                    &format!("{path}[{index}]"),
                    facts,
                    excerpts,
                    visited,
                );
            }
        }
        serde_json::Value::String(text) if text.len() > 256 => {
            if excerpts.len() < WORKER_CHECKPOINT_MAX_EXCERPTS {
                excerpts.push(serde_json::json!({
                    "field": path,
                    "original_bytes": text.len(),
                    "excerpt": checkpoint_text_excerpt(text),
                }));
            }
        }
        scalar => {
            if facts.len() < WORKER_CHECKPOINT_MAX_FACTS {
                facts.push(serde_json::json!({"field": path, "value": scalar}));
            }
        }
    }
}

fn checkpoint_fact_is_priority(fact: &serde_json::Value) -> bool {
    fact["field"].as_str().is_some_and(|field| {
        let leaf = field.rsplit('.').next().unwrap_or(field);
        WORKER_CHECKPOINT_PRIORITY_FIELDS.contains(&leaf)
    })
}

fn render_checkpoint_tool_envelope(
    original_bytes: usize,
    facts: &[serde_json::Value],
    excerpts: &[serde_json::Value],
) -> String {
    serde_json::json!({
        "kronn_checkpoint_compacted": true,
        "original_bytes": original_bytes,
        "preserved_scalar_facts": facts,
        "large_field_excerpts": excerpts,
        "note": "The managed worktree is authoritative. Refresh the exact target with read_file before an edit if the retained excerpt is insufficient or its receipt may be stale."
    })
    .to_string()
}

/// Compact one large tool result as valid JSON while preserving every small
/// scalar fact (notably `content_sha256`, paths, offsets and truncation flags).
/// Blindly cutting the serialized payload can remove the CAS receipt or leave
/// malformed JSON, making the retained trajectory actively misleading.
fn compact_worker_checkpoint_tool_results(body: &mut serde_json::Value) -> usize {
    let Some(messages) = body["messages"].as_array_mut() else {
        return 0;
    };
    let mut compacted = 0;
    for message in messages
        .iter_mut()
        .filter(|message| message["role"] == "tool")
    {
        let Some(raw) = message["content"].as_str() else {
            continue;
        };
        if raw.len() <= WORKER_CHECKPOINT_TOOL_RESULT_BYTES {
            continue;
        }
        let original_bytes = raw.len();
        let parsed = serde_json::from_str::<serde_json::Value>(raw)
            .unwrap_or_else(|_| serde_json::Value::String(raw.to_string()));
        let mut facts = Vec::new();
        let mut excerpts = Vec::new();
        let mut visited = 0;
        collect_checkpoint_json_facts(&parsed, "", &mut facts, &mut excerpts, &mut visited);
        let mut rendered = render_checkpoint_tool_envelope(original_bytes, &facts, &excerpts);
        if rendered.len() > WORKER_CHECKPOINT_TOOL_RESULT_BYTES {
            // Wide API objects can have many small scalars. Fall back to the
            // protocol-critical subset plus one bounded large-field excerpt;
            // this keeps the envelope itself under the advertised per-result
            // limit without ever dropping a receipt in favour of trivia.
            facts.retain(checkpoint_fact_is_priority);
            excerpts.truncate(1);
            rendered = render_checkpoint_tool_envelope(original_bytes, &facts, &excerpts);
        }
        if rendered.len() > WORKER_CHECKPOINT_TOOL_RESULT_BYTES {
            // A pathological path/value may still be unusually long. Receipts
            // and scalar coordinates remain; the model can refresh content
            // with the retained read tool.
            excerpts.clear();
            rendered = render_checkpoint_tool_envelope(original_bytes, &facts, &excerpts);
        }
        while rendered.len() > WORKER_CHECKPOINT_TOOL_RESULT_BYTES && facts.len() > 1 {
            facts.pop();
            rendered = render_checkpoint_tool_envelope(original_bytes, &facts, &excerpts);
        }
        message["content"] = serde_json::json!(rendered);
        compacted += 1;
    }
    compacted
}

/// Start finalization from a compact, protocol-valid handoff. Immutable policy
/// and the authoritative initial prompt are retained, as are the most recent
/// complete tool-call/result rounds. Large observation payloads are clamped;
/// the code-generated facts below state only runner-observed mutations and the
/// catalogue that is actually declared for the next request.
fn checkpoint_worker_finalization_history(
    body: &mut serde_json::Value,
    seed: &WorkerCheckpointSeed,
    checkpoint_prompt: &str,
    ctx_cap: u64,
    workspace_mutated: bool,
    mutated_paths: &std::collections::BTreeSet<String>,
) -> WorkerHistoryCheckpoint {
    let before_messages = body["messages"].as_array().map_or(0, Vec::len);
    let before_tokens = estimated_chat_history_tokens(body);
    let available_tool_names = declared_tool_names(body);
    let tail = body["messages"]
        .as_array()
        .map(|messages| {
            recent_worker_protocol_tail(messages, seed.source_message_count, &available_tool_names)
        })
        .unwrap_or_default();
    let seed_messages = seed.messages.len();
    let tail_messages = tail.len();
    let mut kept = seed.messages.clone();
    kept.extend(tail);
    let mut available_tools = available_tool_names.into_iter().collect::<Vec<_>>();
    available_tools.sort_unstable();
    let mutation_paths = if mutated_paths.is_empty() {
        "none recorded in this provider run".to_string()
    } else {
        mutated_paths.iter().cloned().collect::<Vec<_>>().join(", ")
    };
    kept.push(serde_json::json!({
        "role": "user",
        "content": format!(
            "{checkpoint_prompt}\n\nKronn checkpoint facts (generated from executor state, not by the model):\n\
             - workspace mutation succeeded in this provider run: {workspace_mutated}\n\
             - paths successfully mutated in this provider run: {mutation_paths}\n\
             - tools declared for the next request: {}\n\
             Continue from the retained recent tool trajectory. Do not call a tool outside this exact list.",
            available_tools.join(", ")
        ),
    }));
    body["messages"] = serde_json::Value::Array(kept);
    let compacted_tool_results = compact_worker_checkpoint_tool_results(body);

    // Trim the retained raw results against a phase-local slot before sizing
    // the final request. `clamp_ollama_tool_results` preserves assistant/tool
    // correlation and adds an explicit truncation note to every shortened
    // payload.
    let phase_target = ctx_cap.clamp(OLLAMA_NUM_CTX_FLOOR, WORKER_FINALIZATION_CTX_TARGET);
    if let Some(options) = body["options"].as_object_mut() {
        options.insert("num_ctx".into(), serde_json::json!(phase_target));
    }
    clamp_ollama_tool_results(body, ctx_cap);
    let after_messages = body["messages"].as_array().map_or(0, Vec::len);
    let after_tokens = estimated_chat_history_tokens(body);
    let final_num_ctx = after_tokens
        .saturating_add(WORKER_FINALIZATION_REPLY_HEADROOM)
        .clamp(OLLAMA_NUM_CTX_FLOOR, ctx_cap.max(OLLAMA_NUM_CTX_FLOOR));
    if let Some(options) = body["options"].as_object_mut() {
        options.insert("num_ctx".into(), serde_json::json!(final_num_ctx));
    }
    WorkerHistoryCheckpoint {
        before_messages,
        after_messages,
        before_tokens,
        after_tokens,
        seed_messages,
        tail_messages,
        compacted_tool_results,
        final_num_ctx,
    }
}

fn worker_context_pressure(
    body: &serde_json::Value,
    ctx_cap: u64,
    pressure_percent: u64,
) -> Option<u64> {
    if ctx_cap == 0 || pressure_percent == 0 {
        return None;
    }
    let estimated = estimated_chat_history_tokens(body);
    (estimated.saturating_mul(100) >= ctx_cap.saturating_mul(pressure_percent)).then_some(estimated)
}

fn max_calls_for_tool(name: &str, run_mode: crate::agents::tools::ToolRunMode) -> usize {
    // Reading several small files is legitimate repository analysis. Keep the
    // stricter anti-loop cap for API/MCP calls, where varying arguments caused
    // the observed 47-call paid loop. Exact duplicate reads are still stopped
    // separately after one replay, and the global 50-round cap remains.
    match (run_mode, name) {
        (_, "read_file") => MAX_READ_FILE_CALLS,
        (crate::agents::tools::ToolRunMode::Worker, "search_text") => MAX_WORKER_SEARCH_TEXT_CALLS,
        _ => MAX_CALLS_PER_TOOL,
    }
}

fn is_workspace_observation_tool(name: &str) -> bool {
    matches!(
        name,
        "read_file"
            | "search_text"
            | "list_files"
            | "find_files"
            | "git_status"
            | "git_diff"
            | "git_log"
    )
}

fn is_workspace_progress_tool(name: &str) -> bool {
    matches!(
        name,
        "write_file"
            | "edit_file"
            | "edit_lines"
            | "insert_after_line"
            | "git_commit"
            | "task_exec_deliver"
    )
}

fn is_worker_finalization_tool(name: &str) -> bool {
    matches!(
        name,
        // `read_file` is required to refresh the exact CAS receipt before an
        // edit. Removing it here would leave edit_file/edit_lines advertised
        // but structurally unable to succeed.
        "read_file"
            | "write_file"
            | "edit_file"
            | "edit_lines"
            | "insert_after_line"
            | "git_status"
            | "git_diff"
            | "git_commit"
            | "task_exec_deliver"
    )
}

fn annotate_worker_exploration(
    outcome: &mut crate::agents::tools::ToolOutcome,
    explored_without_progress: usize,
) {
    if explored_without_progress < WORKER_EXPLORATION_NUDGE_AT
        || (explored_without_progress != WORKER_EXPLORATION_NUDGE_AT
            && !explored_without_progress.is_multiple_of(8))
    {
        return;
    }
    let Some(payload) = outcome.content.as_object_mut() else {
        return;
    };
    payload.insert(
        "kronn_worker_progress".into(),
        serde_json::json!(format!(
            "You have completed {explored_without_progress} successful repository observations \
             without changing the workspace. This may be legitimate analysis, so your read tools \
             remain available. Before exploring further, identify the evidence already acquired, \
             the exact evidence still missing, and the next action. If you already know the target, \
             edit it now; if you cannot proceed, state the concrete blocker instead of circling."
        )),
    );
}

/// A successful tool call that teaches the model nothing is invisible to every
/// other guard here: it is not an error, and varying one argument gives it a
/// fresh signature. Both loops measured on real delegations had that shape —
/// `task_list` twelve times with different filters, `git_log` twelve times with
/// a different `limit` — and the model was told nothing until the cap refused
/// the thirteenth call, eleven round-trips too late.
///
/// So annotate, never refuse: the payload the model just asked for stays intact
/// and the note rides along with it. A model that is converging reads it and
/// carries on; a model that is circling gets the only thing it was missing.
fn annotate_unproductive_repetition(
    outcome: &mut crate::agents::tools::ToolOutcome,
    tool: &str,
    canonical_args: &str,
    call_index: usize,
    tool_limit: usize,
    results_seen: &mut std::collections::HashMap<(String, u64), String>,
) {
    use std::hash::{Hash, Hasher};

    let Some(payload) = outcome.content.as_object_mut() else {
        // Non-object payloads would have to be wrapped to carry a note, and
        // wrapping changes the shape the model was promised. Leave them alone.
        return;
    };

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    outcome_digest_source(&serde_json::Value::Object(payload.clone())).hash(&mut hasher);
    let digest = hasher.finish();
    let key = (tool.to_string(), digest);
    // A digest collision would attach a note to an innocent result. That is the
    // worst it can do: the payload itself is never withheld.
    let already = results_seen.get(&key).cloned();
    results_seen
        .entry(key)
        .or_insert_with(|| canonical_args.to_string());

    if let Some(first_args) = already {
        if first_args != canonical_args {
            payload.insert(
                "kronn_same_answer_as_before".into(),
                serde_json::json!(format!(
                    "You already got this exact answer from `{tool}` in this turn, \
                     with different arguments ({first_args} then {canonical_args}). \
                     Rewording the question will not change it. Act on this result, \
                     or call a DIFFERENT tool."
                )),
            );
            return;
        }
    }

    // No pair of calls has coincided yet, but the tool may still be circling.
    // Warn from a third of its budget: late enough that legitimate paging never
    // sees it, early enough to save most of the round-trips a loop would burn.
    let warn_from = tool_limit.div_ceil(3);
    if call_index >= warn_from {
        // Naming the ceiling is not enough on its own: a model that pages a large
        // file has no alternative in mind, so it keeps paging and spends the turn.
        // Name the alternative with the warning.
        let alternative = if tool == "read_file" {
            " If you are looking for something in particular, `search_text` returns \
             its file and line in one call — read only that region afterwards."
        } else {
            " If it is not bringing you closer to an answer, use a different tool \
             or answer with what you already have."
        };
        payload.insert(
            "kronn_call_budget".into(),
            serde_json::json!(format!(
                "Call {call_index} of at most {tool_limit} to `{tool}` in this turn.{alternative}"
            )),
        );
    }
}

/// Hash the payload as the model sees it, minus the notes this guard itself
/// adds — otherwise the first annotated result could never match a later one.
fn outcome_digest_source(value: &serde_json::Value) -> String {
    match value.as_object() {
        Some(map) => {
            let mut pairs: Vec<_> = map
                .iter()
                .filter(|(key, _)| !key.starts_with("kronn_"))
                .map(|(key, item)| format!("{key}={item}"))
                .collect();
            pairs.sort();
            pairs.join(",")
        }
        None => value.to_string(),
    }
}

fn tool_convergence_diagnostic(
    calls: &std::collections::HashMap<String, usize>,
    errors: &std::collections::HashMap<String, usize>,
    refusals: &std::collections::HashMap<String, usize>,
) -> String {
    let mut failing = errors
        .iter()
        .filter(|(_, count)| **count > 0)
        .map(|(name, count)| {
            (
                name.as_str(),
                calls.get(name).copied().unwrap_or_default(),
                *count,
            )
        })
        .collect::<Vec<_>>();
    failing.sort_by(|left, right| right.2.cmp(&left.2).then_with(|| left.0.cmp(right.0)));
    let omitted = failing.len().saturating_sub(4);
    let mut summary = failing
        .into_iter()
        .take(4)
        .map(|(name, attempt_count, error_count)| {
            let refused_count = refusals.get(name).copied().unwrap_or_default();
            format!(
                "{name}: {attempt_count} attempts ({error_count} errors, {refused_count} refused)"
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    if omitted > 0 {
        summary.push_str(&format!("; +{omitted} other failing tools"));
    }
    if summary.is_empty() {
        "no tool error was recorded".to_string()
    } else {
        summary
    }
}

fn remove_tool_declarations(
    body: &mut serde_json::Value,
    blocked: &std::collections::HashSet<String>,
) {
    let emptied = body
        .get_mut("tools")
        .and_then(serde_json::Value::as_array_mut)
        .map(|tools| {
            tools.retain(|tool| {
                tool.pointer("/function/name")
                    .and_then(serde_json::Value::as_str)
                    .is_none_or(|name| !blocked.contains(name))
            });
            tools.is_empty()
        })
        .unwrap_or(false);
    if emptied {
        if let Some(map) = body.as_object_mut() {
            map.remove("tools");
        }
    }
}

fn retain_worker_finalization_tools(body: &mut serde_json::Value) {
    let emptied = body
        .get_mut("tools")
        .and_then(serde_json::Value::as_array_mut)
        .map(|tools| {
            tools.retain(|tool| {
                tool.pointer("/function/name")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(is_worker_finalization_tool)
            });
            tools.is_empty()
        })
        .unwrap_or(false);
    if emptied {
        if let Some(map) = body.as_object_mut() {
            map.remove("tools");
        }
    }
}

fn set_worker_tools_from_catalogue(
    body: &mut serde_json::Value,
    original_catalogue: &[serde_json::Value],
    names: &[&str],
) {
    let tools = original_catalogue
        .iter()
        .filter(|tool| {
            tool.pointer("/function/name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| names.contains(&name))
        })
        .cloned()
        .collect::<Vec<_>>();
    let Some(map) = body.as_object_mut() else {
        return;
    };
    if tools.is_empty() {
        map.remove("tools");
    } else {
        map.insert("tools".into(), serde_json::Value::Array(tools));
    }
}

fn prelocalized_scope_target(scope: &crate::models::TaskWorkerScope) -> (&str, u32, u32) {
    match scope {
        crate::models::TaskWorkerScope::PrelocalizedEdit {
            path,
            start_line,
            end_line,
        } => (path.as_str(), *start_line, *end_line),
        crate::models::TaskWorkerScope::PrelocalizedInsertAfter { path, anchor_line } => {
            (path.as_str(), *anchor_line, *anchor_line)
        }
    }
}

fn prelocalized_mutation_tool(scope: &crate::models::TaskWorkerScope) -> &'static str {
    match scope {
        crate::models::TaskWorkerScope::PrelocalizedEdit { .. } => "edit_lines",
        crate::models::TaskWorkerScope::PrelocalizedInsertAfter { .. } => "insert_after_line",
    }
}

/// Freeze the first provider turn to one real read around the principal's
/// authoritative target. The context padding is chosen by Kronn, not by the
/// model, so a vague offset cannot turn back into repository exploration.
fn constrain_prelocalized_read_tool(
    body: &mut serde_json::Value,
    original_catalogue: &[serde_json::Value],
    scope: &crate::models::TaskWorkerScope,
) {
    set_worker_tools_from_catalogue(body, original_catalogue, &["read_file"]);
    let (path, start_line, end_line) = prelocalized_scope_target(scope);
    let offset = start_line.saturating_sub(12).max(1);
    let last_line = end_line.saturating_add(12);
    let limit = last_line.saturating_sub(offset).saturating_add(1);
    let Some(tool) = body
        .get_mut("tools")
        .and_then(serde_json::Value::as_array_mut)
        .and_then(|tools| tools.first_mut())
    else {
        return;
    };
    tool["function"]["description"] = serde_json::json!(
        "Read the single Kronn-prelocalized context window. Use the exact path, offset and limit frozen in the schema. A successful read immediately withdraws this tool and exposes one exact CAS edit."
    );
    tool["function"]["parameters"]["properties"]["path"]["enum"] = serde_json::json!([path]);
    tool["function"]["parameters"]["properties"]["offset"]["enum"] = serde_json::json!([offset]);
    tool["function"]["parameters"]["properties"]["limit"]["enum"] = serde_json::json!([limit]);
    tool["function"]["parameters"]["required"] = serde_json::json!(["path", "offset", "limit"]);
    tool["function"]["parameters"]["additionalProperties"] = serde_json::json!(false);
}

fn constrain_prelocalized_edit_tool(
    body: &mut serde_json::Value,
    original_catalogue: &[serde_json::Value],
    scope: &crate::models::TaskWorkerScope,
    receipt: &str,
) {
    let tool_name = prelocalized_mutation_tool(scope);
    set_worker_tools_from_catalogue(body, original_catalogue, &[tool_name]);
    let Some(tool) = body
        .get_mut("tools")
        .and_then(serde_json::Value::as_array_mut)
        .and_then(|tools| tools.first_mut())
    else {
        return;
    };
    match scope {
        crate::models::TaskWorkerScope::PrelocalizedEdit {
            path,
            start_line,
            end_line,
        } => {
            tool["function"]["description"] = serde_json::json!(
                "Replace exactly the principal-prelocalized inclusive range. Path, range and fresh CAS receipt are frozen; only new_string is chosen by the worker."
            );
            for (argument, value) in [
                ("path", serde_json::json!(path)),
                ("start_line", serde_json::json!(start_line)),
                ("end_line", serde_json::json!(end_line)),
                ("expected_sha256", serde_json::json!(receipt)),
            ] {
                tool["function"]["parameters"]["properties"][argument]["enum"] =
                    serde_json::json!([value]);
            }
        }
        crate::models::TaskWorkerScope::PrelocalizedInsertAfter { path, anchor_line } => {
            tool["function"]["description"] = serde_json::json!(
                "Insert after the principal-prelocalized anchor. Path, anchor and fresh CAS receipt are frozen; the anchor is mechanically preserved and only new_string is chosen by the worker."
            );
            for (argument, value) in [
                ("path", serde_json::json!(path)),
                ("anchor_line", serde_json::json!(anchor_line)),
                ("expected_sha256", serde_json::json!(receipt)),
            ] {
                tool["function"]["parameters"]["properties"][argument]["enum"] =
                    serde_json::json!([value]);
            }
        }
    }
    tool["function"]["parameters"]["additionalProperties"] = serde_json::json!(false);
}

fn prelocalized_call_matches_scope(
    call: &crate::agents::tools::ToolCall,
    stage: WorkerRepairStage,
    scope: &crate::models::TaskWorkerScope,
    receipt: Option<&str>,
) -> bool {
    match stage {
        WorkerRepairStage::Read => {
            let (path, start_line, end_line) = prelocalized_scope_target(scope);
            let offset = start_line.saturating_sub(12).max(1);
            let limit = end_line
                .saturating_add(12)
                .saturating_sub(offset)
                .saturating_add(1);
            call.name == "read_file"
                && call
                    .arguments
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    == Some(path)
                && call
                    .arguments
                    .get("offset")
                    .and_then(serde_json::Value::as_u64)
                    == Some(u64::from(offset))
                && call
                    .arguments
                    .get("limit")
                    .and_then(serde_json::Value::as_u64)
                    == Some(u64::from(limit))
        }
        WorkerRepairStage::Edit => match scope {
            crate::models::TaskWorkerScope::PrelocalizedEdit {
                path,
                start_line,
                end_line,
            } => {
                call.name == "edit_lines"
                    && call
                        .arguments
                        .get("path")
                        .and_then(serde_json::Value::as_str)
                        == Some(path.as_str())
                    && call
                        .arguments
                        .get("start_line")
                        .and_then(serde_json::Value::as_u64)
                        == Some(u64::from(*start_line))
                    && call
                        .arguments
                        .get("end_line")
                        .and_then(serde_json::Value::as_u64)
                        == Some(u64::from(*end_line))
                    && receipt.is_some_and(|receipt| {
                        call.arguments
                            .get("expected_sha256")
                            .and_then(serde_json::Value::as_str)
                            == Some(receipt)
                    })
            }
            crate::models::TaskWorkerScope::PrelocalizedInsertAfter { path, anchor_line } => {
                call.name == "insert_after_line"
                    && call
                        .arguments
                        .get("path")
                        .and_then(serde_json::Value::as_str)
                        == Some(path.as_str())
                    && call
                        .arguments
                        .get("anchor_line")
                        .and_then(serde_json::Value::as_u64)
                        == Some(u64::from(*anchor_line))
                    && receipt.is_some_and(|receipt| {
                        call.arguments
                            .get("expected_sha256")
                            .and_then(serde_json::Value::as_str)
                            == Some(receipt)
                    })
            }
        },
        _ => true,
    }
}

fn constrain_worker_repair_tool(
    body: &mut serde_json::Value,
    failed_call: &crate::agents::tools::ToolCall,
) {
    let immutable_arguments: &[&str] = match failed_call.name.as_str() {
        "edit_lines" => &["path", "start_line", "end_line"],
        "insert_after_line" => &["path", "anchor_line"],
        "edit_file" => &["path", "old_string", "replace_all"],
        "write_file" => &["path"],
        _ => return,
    };
    let Some(tools) = body
        .get_mut("tools")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    let Some(tool) = tools.iter_mut().find(|tool| {
        tool.pointer("/function/name")
            .and_then(serde_json::Value::as_str)
            == Some(failed_call.name.as_str())
    }) else {
        return;
    };
    for argument in immutable_arguments {
        let Some(value) = failed_call.arguments.get(*argument).cloned() else {
            continue;
        };
        let pointer = format!("/function/parameters/properties/{argument}");
        if let Some(schema) = tool
            .pointer_mut(&pointer)
            .and_then(serde_json::Value::as_object_mut)
        {
            // A one-value enum is understood by Ollama and OpenAI-wire tool
            // schema validators; `const` is not consistently accepted by all
            // compatible gateways.
            schema.insert("enum".into(), serde_json::json!([value]));
        }
    }
}

fn worker_repair_call_matches_target(
    failed_call: &crate::agents::tools::ToolCall,
    repair_call: &crate::agents::tools::ToolCall,
) -> bool {
    if failed_call.name != repair_call.name {
        return false;
    }
    let immutable_arguments: &[&str] = match failed_call.name.as_str() {
        "edit_lines" => &["path", "start_line", "end_line"],
        "insert_after_line" => &["path", "anchor_line"],
        "edit_file" => &["path", "old_string"],
        "write_file" => &["path"],
        _ => return false,
    };
    let exact_arguments_match = immutable_arguments.iter().all(|argument| {
        failed_call.arguments.get(*argument) == repair_call.arguments.get(*argument)
    });
    exact_arguments_match
        && (failed_call.name != "edit_file"
            || failed_call
                .arguments
                .get("replace_all")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
                == repair_call
                    .arguments
                    .get("replace_all")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false))
}

fn rust_syntax_refusal(outcome: &crate::agents::tools::ToolOutcome) -> bool {
    outcome
        .content
        .get("error")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|error| {
            error.starts_with(crate::api::agent_workspace_tools::RUST_SYNTAX_REFUSAL_PREFIX)
        })
}

fn worker_repair_iteration_limit(
    stage: WorkerRepairStage,
    strict_syntax_repair: bool,
    prelocalized: bool,
) -> usize {
    if strict_syntax_repair && stage == WorkerRepairStage::Edit {
        1
    } else if prelocalized
        && matches!(
            stage,
            WorkerRepairStage::Read | WorkerRepairStage::Edit | WorkerRepairStage::Commit
        )
    {
        2
    } else {
        stage.iteration_limit()
    }
}

fn worker_repair_terminal_reason_code(
    stage: WorkerRepairStage,
    prelocalized: bool,
) -> &'static str {
    if prelocalized {
        match stage {
            WorkerRepairStage::Read => "prelocalized_read_exhausted",
            WorkerRepairStage::Edit => "prelocalized_edit_exhausted",
            WorkerRepairStage::Commit => "prelocalized_commit_exhausted",
            WorkerRepairStage::Inactive => "prelocalized_contract_exhausted",
        }
    } else {
        "worker_repair_exhausted"
    }
}

fn restore_worker_tools_from_catalogue(
    body: &mut serde_json::Value,
    original_catalogue: &[serde_json::Value],
    names: &[&str],
) {
    let mut wanted = declared_tool_names(body);
    wanted.extend(names.iter().map(|name| (*name).to_string()));
    let tools = original_catalogue
        .iter()
        .filter(|tool| {
            tool.pointer("/function/name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| wanted.contains(name))
        })
        .cloned()
        .collect::<Vec<_>>();
    let Some(map) = body.as_object_mut() else {
        return;
    };
    if tools.is_empty() {
        map.remove("tools");
    } else {
        map.insert("tools".into(), serde_json::Value::Array(tools));
    }
}

fn declared_tool_names(body: &serde_json::Value) -> std::collections::HashSet<String> {
    body.get("tools")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| {
            tool.pointer("/function/name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

fn invalidate_workspace_observation_cache(
    seen_calls: &mut std::collections::HashMap<String, (bool, serde_json::Value)>,
    repeated_calls: &mut std::collections::HashMap<String, usize>,
    results_seen: &mut std::collections::HashMap<(String, u64), String>,
) {
    let is_observation_signature = |signature: &str| {
        signature
            .split_once('|')
            .is_some_and(|(name, _)| is_workspace_observation_tool(name))
    };
    seen_calls.retain(|signature, _| !is_observation_signature(signature));
    repeated_calls.retain(|signature, _| !is_observation_signature(signature));
    results_seen.retain(|(name, _), _| !is_workspace_observation_tool(name));
}

/// Detect if we're running inside WSL (vs Windows native).
/// In WSL, /proc/version contains "microsoft" or "WSL".
#[allow(dead_code)]
fn is_wsl() -> bool {
    #[cfg(target_os = "linux")]
    {
        // WSL2 always sets WSL_DISTRO_NAME — most reliable check
        if std::env::var("WSL_DISTRO_NAME").is_ok() {
            return true;
        }
        std::fs::read_to_string("/proc/version")
            .map(|v| v.contains("microsoft") || v.contains("WSL"))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Convert a Windows path (C:\Users\...) to WSL path (/mnt/c/Users/...).
#[cfg(target_os = "windows")]
fn windows_to_wsl_path(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        // Extended-length path
        convert_drive_path(rest)
    } else if s.len() >= 3 && s.as_bytes()[1] == b':' {
        convert_drive_path(&s)
    } else {
        path.to_path_buf()
    }
}

#[cfg(target_os = "windows")]
fn convert_drive_path(s: &str) -> PathBuf {
    let drive = s.chars().next().unwrap().to_lowercase().next().unwrap();
    let rest = s[2..].replace('\\', "/");
    PathBuf::from(format!("/mnt/{}{}", drive, rest))
}

/// Output mode — how to interpret stdout from the agent
#[derive(Clone, Copy, PartialEq)]
pub enum OutputMode {
    /// Each line is plain text (default for most agents)
    Text,
    /// Each line is a JSON event (Claude Code --output-format stream-json)
    StreamJson,
}

/// Result of parsing a single stream-json line
#[derive(Debug)]
pub enum StreamJsonEvent {
    /// A text chunk to stream to the user
    Text(String),
    /// Token usage from a message_delta event (input_tokens, output_tokens, optional cost)
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: Option<f64>,
    },
    /// A terminal provider/CLI failure carried by Claude Code's final
    /// `result` event. Claude may write nothing to stderr and still exit 1,
    /// so dropping this object turns an actionable 429/quota error into the
    /// misleading "No output captured" fallback.
    TerminalError(StreamJsonFailure),
    /// Tool use started — name of the tool
    ToolStart(String),
    /// Partial JSON input for the current tool (accumulated to build full input)
    ToolInputDelta(String),
    /// Content block finished (tool input complete)
    ToolEnd,
    /// Nothing useful (metadata, start/stop events, etc.)
    Skip,
}

/// Structured fields retained from a failed Claude Code `result` event.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamJsonFailure {
    pub is_error: bool,
    pub text: String,
    pub api_error_status: Option<u16>,
    pub terminal_reason: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
}

impl StreamJsonFailure {
    /// Concise durable rendering used by discussions, workflows and audits.
    /// The original provider text remains verbatim while structured metadata
    /// stays visible enough for deterministic quota/error classification.
    pub fn user_message(&self) -> String {
        let mut details = Vec::new();
        if let Some(status) = self.api_error_status {
            details.push(format!("HTTP {status}"));
        }
        if let Some(reason) = self.terminal_reason.as_deref() {
            details.push(format!("terminal_reason={reason}"));
        }
        if details.is_empty() {
            format!("[Agent provider error]\n\n{}", self.text)
        } else {
            format!(
                "[Agent provider error]\n\n{}\n\n({})",
                self.text,
                details.join("; ")
            )
        }
    }
}

/// How to handle stderr from an agent process
#[derive(Clone, Copy)]
enum StderrMode {
    /// Merge stderr into output stream (default — agent puts useful output on both)
    Merge,
    /// Only use stdout; log stderr but don't stream it (agent puts noise on stderr)
    /// Stderr is still captured so it can be shown on failure.
    StdoutOnly,
}

/// Running agent process with streaming output
pub struct AgentProcess {
    pub child: tokio::process::Child,
    pub output_mode: OutputMode,
    pub work_dir: PathBuf,
    agent_type: AgentType,
    rx: mpsc::Receiver<String>,
    pub stderr_capture: Arc<Mutex<Vec<String>>>,
    stderr_task: Option<tokio::task::JoinHandle<()>>,
    /// HTTP agents run their provider/tool loop in a Tokio task rather than in
    /// `child`. Killing the lifeline process alone therefore does not stop the
    /// real work. Present only for HTTP agents; cancellation drops an in-flight
    /// request/tool future and prevents every later call in the same batch.
    http_cancel: Option<tokio_util::sync::CancellationToken>,
    /// On Unix, the process group ID of the spawned agent, used to terminate
    /// the entire process tree on cancellation. None on Windows.
    pgid: Option<i32>,
}

impl AgentProcess {
    /// True when `next_line()` yields RAW token fragments to concatenate as-is
    /// (Ollama HTTP streams model tokens), not whole lines. Line-based
    /// consumers must skip their '\n' separator for these — joining tokens
    /// with newlines shreds the message into one word per line (the
    /// 2026-07-01 Ollama formatting bug).
    pub fn raw_token_stream(&self) -> bool {
        is_http_chat_agent(&self.agent_type)
    }

    /// Get next output line. For Kiro, strips ANSI codes and filters noise.
    pub async fn next_line(&mut self) -> Option<String> {
        loop {
            let line = self.rx.recv().await?;
            if self.agent_type == AgentType::Kiro {
                if let Some(cleaned) = clean_kiro_line(&line) {
                    return Some(cleaned);
                }
                // Filtered noise line — try next
                continue;
            }
            return Some(line);
        }
    }

    /// Wait for stderr reader to finish, then return captured lines.
    /// Must be called after `child.wait()` to ensure all stderr is flushed.
    pub async fn captured_stderr_flushed(&mut self) -> Vec<String> {
        if let Some(handle) = self.stderr_task.take() {
            // Give stderr reader a brief window to finish after process exit
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        }
        self.stderr_capture.lock().unwrap().clone()
    }

    /// Return captured stderr lines (only populated in StdoutOnly mode)
    /// Note: may be incomplete if called before process exit. Prefer `captured_stderr_flushed`.
    pub fn captured_stderr(&self) -> Vec<String> {
        self.stderr_capture.lock().unwrap().clone()
    }

    /// Fix file ownership after agent execution.
    /// Files created by agents may have wrong ownership if container UID differs from host UID.
    pub fn fix_ownership(&self) {
        fix_file_ownership(&self.work_dir);
    }
}

impl Drop for AgentProcess {
    fn drop(&mut self) {
        // A caller may tear a stream down by dropping it without going through
        // `AgentIo::kill` (timeout/error paths have done both over time). The
        // receiver close unblocks a pending channel send; the token interrupts
        // work that is not currently sending output, notably provider retries
        // and an in-flight native tool.
        self.rx.close();
        if let Some(cancel) = &self.http_cancel {
            cancel.cancel();
        }
        // Kill the entire process group (CLI agents on Unix) to prevent zombies
        // when dropped without explicit kill(). Only act if child is still running (id exists).
        if self.child.id().is_some() {
            if let Some(pgid) = self.pgid {
                if pgid > 1 {
                    #[cfg(unix)]
                    {
                        // Synchronous fast path: SIGKILL the entire group.
                        // This is a best-effort cleanup; if the normal kill() path
                        // was already called, this group no longer exists.
                        unsafe {
                            let _ = libc::kill(-pgid, libc::SIGKILL);
                        }
                    }
                }
            }
        }
    }
}

/// Owned, portable exit status. The pipeline loops only ever read
/// `.success` / `.code`, so we avoid threading `std::process::ExitStatus`
/// (which can't be constructed portably in test fakes) through the trait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentExit {
    pub success: bool,
    pub code: Option<i32>,
}

impl AgentExit {
    fn from_status(status: std::process::ExitStatus) -> Self {
        Self {
            success: status.success(),
            code: status.code(),
        }
    }
}

/// Check if a process group has any living processes.
/// Returns true if at least one process exists, false only if confirmed dead (ESRCH).
/// Treats permission errors and other errors as "possibly alive / not confirmed".
#[cfg(unix)]
fn group_has_processes(pgid: i32) -> bool {
    if pgid <= 1 {
        return true;
    }
    use std::io;
    unsafe {
        match libc::kill(-pgid, 0) {
            0 => true, // Signal would be delivered; processes exist
            -1 => {
                // Check errno to distinguish ESRCH (group dead) from other errors
                match io::Error::last_os_error().raw_os_error() {
                    Some(libc::ESRCH) => false, // No such process; group confirmed dead
                    _ => true,                  // EPERM or other error; assume processes exist
                }
            }
            _ => true, // Shouldn't happen, assume alive
        }
    }
}

/// Abstraction over an agent's output stream + process lifecycle.
///
/// The pipeline loops (`run_agent_collect`, `run_agent_streaming`,
/// `make_agent_stream`, `full_audit`, the workflow Agent step) are written
/// against this trait so they can be driven by a real spawned
/// [`AgentProcess`] in production OR a `ScriptedProcess` (test-only, no real
/// subprocess) under test — covering the bug-prone consumption logic
/// (tool-call parsing, decoder-loop detection, checkpointing, cancellation,
/// stream-json vs raw) without burning tokens or needing a CLI binary.
///
/// Call sites use static dispatch (`impl AgentIo`), but `#[async_trait]`
/// keeps the returned futures `Send` so a loop can still be `tokio::spawn`ed
/// (the `make_agent_stream` path does). Mirrors the existing
/// `workflows::tracker::TrackerSource` convention in this codebase.
#[async_trait::async_trait]
pub trait AgentIo: Send {
    /// Next output line. `None` once the stream is exhausted.
    async fn next_line(&mut self) -> Option<String>;
    /// How to interpret stdout (StreamJson → parse events ; otherwise raw).
    fn output_mode(&self) -> OutputMode;
    /// True when `next_line()` yields raw token fragments (concatenate as-is)
    /// instead of whole lines. Only Ollama's HTTP stream does this.
    fn raw_token_stream(&self) -> bool {
        false
    }
    /// Best-effort kill of the underlying process.
    async fn kill(&mut self);
    /// Await process exit. `None` when nothing real backs it (scripted).
    async fn wait(&mut self) -> Option<AgentExit>;
    /// Non-blocking exit poll — used by the audit zombie-detector.
    fn try_wait(&mut self) -> Option<AgentExit>;
    /// Underlying OS pid, when a real process backs this (for cancellation
    /// registration). `None` for scripted fakes.
    fn child_id(&self) -> Option<u32>;
    /// Captured stderr, flushed after exit (StdoutOnly mode diagnostics).
    async fn captured_stderr_flushed(&mut self) -> Vec<String>;
    /// Fix file ownership after the run (Docker uid remap). No-op when there
    /// is no work dir (scripted).
    fn fix_ownership(&self);
}

#[async_trait::async_trait]
impl AgentIo for AgentProcess {
    async fn next_line(&mut self) -> Option<String> {
        AgentProcess::next_line(self).await
    }
    fn output_mode(&self) -> OutputMode {
        self.output_mode
    }
    fn raw_token_stream(&self) -> bool {
        AgentProcess::raw_token_stream(self)
    }
    async fn kill(&mut self) {
        self.rx.close();
        if let Some(cancel) = &self.http_cancel {
            cancel.cancel();
        }

        // Terminate the agent process (and its entire process tree on Unix).
        // Unix: kill the process group with SIGTERM, then SIGKILL if needed.
        // Windows: use taskkill /T /F to kill the process tree.
        let pid = self.child.id();
        if let Some(pid) = pid {
            let timeout = Duration::from_secs(5);
            let start = Instant::now();

            #[cfg(unix)]
            {
                // Try terminating the process group with SIGTERM
                if let Some(pgid) = self.pgid.filter(|pgid| *pgid > 1) {
                    tracing::debug!(
                        "Sending SIGTERM to process group {} for agent PID {}",
                        pgid,
                        pid
                    );
                    unsafe {
                        // Kill the entire process group, not just the parent.
                        // -pgid sends signal to all processes in the group.
                        libc::kill(-pgid, libc::SIGTERM);
                    }
                } else {
                    // Fallback: terminate just the process
                    tracing::debug!("No process group info, killing agent PID {} directly", pid);
                    let _ = self.child.kill().await;
                }

                // Wait with timeout for graceful termination of the group.
                // Poll group state periodically, not just the parent process.
                loop {
                    if start.elapsed() > timeout {
                        break;
                    }

                    // Check if child process has exited
                    match self.child.try_wait() {
                        Ok(Some(status)) => {
                            tracing::info!(
                                "Agent parent process terminated gracefully with status {}",
                                status
                            );
                            // Parent is gone, but verify group is also dead before returning
                            if let Some(pgid) = self.pgid.filter(|pgid| *pgid > 1) {
                                #[cfg(unix)]
                                {
                                    if !group_has_processes(pgid) {
                                        tracing::info!("Process group {} is empty", pgid);
                                        return;
                                    } else {
                                        tracing::debug!(
                                            "Parent exited but group {} still has processes",
                                            pgid
                                        );
                                        // Continue to SIGKILL loop below
                                        break;
                                    }
                                }
                                #[cfg(not(unix))]
                                {
                                    return;
                                }
                            } else {
                                return;
                            }
                        }
                        Ok(None) => {
                            // Parent still alive, sleep and retry
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                        Err(_) => {
                            tracing::debug!("Error checking child status, assuming terminated");
                            return;
                        }
                    }
                }

                // Forceful termination: send SIGKILL to process group
                if let Some(pgid) = self.pgid.filter(|pgid| *pgid > 1) {
                    tracing::warn!(
                        "SIGTERM timed out, sending SIGKILL to process group {}",
                        pgid
                    );
                    unsafe {
                        libc::kill(-pgid, libc::SIGKILL);
                    }

                    // Wait for SIGKILL to take effect
                    let sigkill_start = Instant::now();
                    let sigkill_timeout = Duration::from_secs(2);
                    loop {
                        if sigkill_start.elapsed() > sigkill_timeout {
                            tracing::warn!("SIGKILL timeout for process group {}", pgid);
                            break;
                        }

                        // Check if any processes remain in group
                        if !group_has_processes(pgid) {
                            tracing::info!("Process group {} terminated after SIGKILL", pgid);
                            break;
                        }

                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                } else {
                    tracing::warn!("SIGTERM timed out, killing agent PID {} with SIGKILL", pid);
                    let _ = self.child.kill().await;

                    // Final wait to reap the child
                    let mut final_attempts = 0;
                    loop {
                        if final_attempts > 20 {
                            tracing::warn!("Could not reap agent process after SIGKILL");
                            break;
                        }
                        match self.child.try_wait() {
                            Ok(Some(_)) => {
                                tracing::info!("Agent process terminated after SIGKILL");
                                break;
                            }
                            Ok(None) => {
                                tokio::time::sleep(Duration::from_millis(100)).await;
                                final_attempts += 1;
                            }
                            Err(_) => {
                                break;
                            }
                        }
                    }
                }

                // Final attempt to reap the child process to avoid zombies
                let _ = self.child.try_wait();
            }

            #[cfg(windows)]
            {
                // Windows: use taskkill to terminate the process tree
                tracing::debug!(
                    "Terminating process tree for agent PID {} with taskkill",
                    pid
                );
                if let Ok(output) = async_cmd("taskkill")
                    .args(&["/PID", &pid.to_string(), "/T", "/F"])
                    .output()
                    .await
                {
                    if !output.status.success() {
                        tracing::warn!("taskkill returned non-zero status: {:?}", output.status);
                    } else {
                        tracing::info!("Process tree terminated with taskkill");
                    }
                } else {
                    tracing::warn!("Failed to execute taskkill for PID {}", pid);
                    let _ = self.child.kill().await;
                }

                // Wait for process to actually exit
                let mut attempts = 0;
                while attempts < 50 {
                    match self.child.try_wait() {
                        Ok(Some(_)) => {
                            tracing::info!("Agent process exited");
                            return;
                        }
                        Ok(None) => {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            attempts += 1;
                        }
                        Err(_) => {
                            break;
                        }
                    }
                }
            }
        } else {
            tracing::debug!("No PID available for agent process termination");
            let _ = self.child.kill().await;
        }
    }
    async fn wait(&mut self) -> Option<AgentExit> {
        self.child.wait().await.ok().map(AgentExit::from_status)
    }
    fn try_wait(&mut self) -> Option<AgentExit> {
        // io::Result<Option<ExitStatus>> → Option<AgentExit> : both an error
        // and "still running" collapse to None (the caller treats both as
        // "not exited yet"), which matches the existing zombie-detector use.
        self.child
            .try_wait()
            .ok()
            .flatten()
            .map(AgentExit::from_status)
    }
    fn child_id(&self) -> Option<u32> {
        self.child.id()
    }
    async fn captured_stderr_flushed(&mut self) -> Vec<String> {
        AgentProcess::captured_stderr_flushed(self).await
    }
    fn fix_ownership(&self) {
        AgentProcess::fix_ownership(self)
    }
}

/// Test-only scripted [`AgentIo`] — yields a pre-canned sequence of output
/// lines with no real subprocess. Lets the pipeline loops be unit-tested
/// (line accumulation, stream-json parsing, teardown) without spawning a CLI
/// or burning tokens.
///
/// `#[cfg(test)]` so it never ships in the binary. Visible to unit tests
/// across the lib crate (cfg(test) is crate-wide); integration tests in
/// `tests/` can't see it — but loop logic is unit-level anyway.
#[cfg(test)]
pub struct ScriptedProcess {
    lines: std::collections::VecDeque<String>,
    output_mode: OutputMode,
    exit: AgentExit,
    /// Set by `kill()` so a test can assert the loop killed on timeout/cancel.
    pub killed: bool,
    /// Pre-canned stderr returned by `captured_stderr_flushed`.
    stderr: Vec<String>,
    /// `next_line` never resolves — the only way to reach a deadline branch in a
    /// test, since a drained scripted stream ends the loop normally.
    hangs_forever: bool,
}

#[cfg(test)]
impl ScriptedProcess {
    /// Scripted process in raw-line mode (each line emitted verbatim).
    pub fn raw(lines: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            lines: lines.into_iter().map(Into::into).collect(),
            output_mode: OutputMode::Text,
            exit: AgentExit {
                success: true,
                code: Some(0),
            },
            killed: false,
            stderr: Vec::new(),
            hangs_forever: false,
        }
    }

    /// Scripted process in StreamJson mode (lines are claude-stream JSON).
    pub fn stream_json(lines: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            lines: lines.into_iter().map(Into::into).collect(),
            output_mode: OutputMode::StreamJson,
            exit: AgentExit {
                success: true,
                code: Some(0),
            },
            killed: false,
            stderr: Vec::new(),
            hangs_forever: false,
        }
    }

    /// Override the exit status the loop sees on `wait()`.
    pub fn with_exit(mut self, success: bool, code: Option<i32>) -> Self {
        self.exit = AgentExit { success, code };
        self
    }

    /// Pre-load stderr lines for the StdoutOnly diagnostic path.
    pub fn with_stderr(mut self, lines: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.stderr = lines.into_iter().map(Into::into).collect();
        self
    }

    /// A process that produces nothing and never exits, so the caller's deadline
    /// is the only thing that can end the loop. Pair with a paused Tokio clock.
    pub fn hanging() -> Self {
        let mut process = Self::raw(Vec::<String>::new());
        process.hangs_forever = true;
        process.exit = AgentExit {
            success: false,
            code: None,
        };
        process
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl AgentIo for ScriptedProcess {
    async fn next_line(&mut self) -> Option<String> {
        if self.hangs_forever {
            std::future::pending::<()>().await;
        }
        self.lines.pop_front()
    }
    fn output_mode(&self) -> OutputMode {
        self.output_mode
    }
    async fn kill(&mut self) {
        self.killed = true;
    }
    async fn wait(&mut self) -> Option<AgentExit> {
        Some(self.exit)
    }
    fn try_wait(&mut self) -> Option<AgentExit> {
        // Mirror real semantics: "exited" only once the scripted stream is
        // drained ; otherwise "still running" (None). A hanging fake is still
        // running by definition, drained queue or not.
        if self.hangs_forever || !self.lines.is_empty() {
            None
        } else {
            Some(self.exit)
        }
    }
    fn child_id(&self) -> Option<u32> {
        None
    }
    async fn captured_stderr_flushed(&mut self) -> Vec<String> {
        std::mem::take(&mut self.stderr)
    }
    fn fix_ownership(&self) {}
}

/// Fix file ownership after agent execution or file operations.
/// Files created in Docker may have wrong ownership if container UID differs from host UID.
/// On macOS with VirtioFS, chown is silently ignored by the filesystem driver.
pub fn fix_file_ownership(work_dir: &Path) {
    // Only relevant in Docker — native apps own their own files
    if !crate::core::env::is_docker() {
        return;
    }
    let uid = std::env::var("KRONN_HOST_UID").unwrap_or_default();
    let gid = std::env::var("KRONN_HOST_GID").unwrap_or_default();
    if uid.is_empty() || gid.is_empty() {
        return;
    }

    // Skip if container user already matches the desired UID (expected when
    // APP_UID build arg matches KRONN_HOST_UID — the normal case after the fix).
    if let Ok(output) = sync_cmd("id")
        .arg("-u")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        let current_uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if current_uid == uid {
            return; // Already correct UID, no chown needed
        }
    }

    let ownership = format!("{}:{}", uid, gid);
    // Only fix files in the work directory, not system files
    let status = sync_cmd("chown")
        .args(["-R", &ownership])
        .arg(work_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    if let Ok(s) = status {
        if !s.success() {
            tracing::debug!(
                "chown failed (exit {}), likely non-root container or VirtioFS — skipping",
                s.code().unwrap_or(-1)
            );
        }
    }
}

/// Server-derived identity for one CLI-backed discussion agent executing a
/// durable task. The child process forwards this opaque context through the
/// Kronn MCP bridge; the backend still revalidates every field against the
/// execution and dispatch rows before accepting a delivery.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskWorkerBridgeContext {
    pub execution_id: String,
    pub discussion_id: String,
    pub agent_type: String,
    pub source_message_id: String,
}

/// Codex intentionally forwards only named parent-process variables to stdio
/// MCP children. Keep the allowlist narrow and shared with the global config
/// writer; without it a Kronn-launched Codex sees the bridge but loses its
/// discussion and task-worker capability at the process boundary.
pub(crate) const KRONN_INTERNAL_CODEX_ENV_VARS: &[&str] = &[
    "KRONN_DISCUSSION_ID",
    "KRONN_BACKEND_URL",
    "KRONN_AUTH_TOKEN",
    "KRONN_TASK_WORKER_CONTEXT",
    "KRONN_SESSION_ID",
    "KRONN_CALLER_SESSION_ID",
    "KRONN_AGENT_TYPE",
    "KRONN_CALLER_AGENT",
    "KRONN_AGENT_MODEL",
    "KRONN_WAIT_TOTAL_SECS",
];

fn codex_kronn_internal_env_override() -> String {
    format!(
        "mcp_servers.kronn-internal.env_vars={}",
        serde_json::to_string(KRONN_INTERNAL_CODEX_ENV_VARS)
            .expect("static Codex MCP env allowlist must serialize")
    )
}

/// Build the complete MCP table for an isolated Codex task worker.
///
/// `--ignore-user-config` deliberately removes every user MCP and policy, so
/// overriding only `...env_vars` leaves a partial server with no command. Codex
/// rejects that shape as `invalid transport` before the model starts. A worker
/// gets one self-contained server instead: the exact commit + delivery bridge
/// it needs, and nothing inherited from the user's global config.
fn codex_task_worker_mcp_override() -> Option<String> {
    let script = disc_introspection_mcp_path()?;
    render_codex_task_worker_mcp_override(Some(&script))
}

fn render_codex_task_worker_mcp_override(script: Option<&str>) -> Option<String> {
    let script = serde_json::to_string(script?).ok()?;
    let env_vars = serde_json::to_string(KRONN_INTERNAL_CODEX_ENV_VARS).ok()?;
    Some(format!(
        "mcp_servers={{\"kronn-internal\"={{command=\"python3\",args=[{script}],env_vars={env_vars},startup_timeout_sec=30,required=true,enabled_tools=[\"task_exec_status\",\"task_exec_commit\",\"task_exec_deliver\"],default_tools_approval_mode=\"prompt\",tools={{task_exec_status={{approval_mode=\"approve\"}},task_exec_commit={{approval_mode=\"approve\"}},task_exec_deliver={{approval_mode=\"approve\"}}}}}}}}"
    ))
}

/// Configuration for starting an agent process.
pub struct AgentStartConfig<'a> {
    pub agent_type: &'a AgentType,
    /// Used to read .mcp.json and resolve MCP context.
    pub project_path: &'a str,
    /// Working directory for the agent. If `None`, defaults to `project_path`.
    pub work_dir: Option<&'a str>,
    pub prompt: &'a str,
    pub tokens: &'a TokensConfig,
    pub full_access: bool,
    pub skill_ids: &'a [String],
    pub directive_ids: &'a [String],
    pub profile_ids: &'a [String],
    /// Override MCP context instead of reading from project filesystem.
    /// Used for general discussions to inject global MCP configs.
    pub mcp_context_override: Option<&'a str>,
    /// Model capability tier. Resolved to a --model flag per agent.
    /// Priority: explicit model string > tier > Default (no flag).
    pub tier: ModelTier,
    /// Per-agent model tier config (from global settings). Used to resolve tier to model name.
    pub model_tiers: Option<&'a ModelTiersConfig>,
    /// Executes Kronn primitives when the model asks for them. `None` = the
    /// agent gets no tools, which is the honest default for callers that have
    /// no `AppState` to execute against.
    pub tools: Option<std::sync::Arc<dyn crate::agents::tools::ToolExecutor>>,
    /// Endpoint slots of the OpenAI-wire providers. One value rather than one
    /// field per provider: as two fields, every call site set LiteLLM's and
    /// none set NVIDIA's, so a configured NVIDIA endpoint never arrived here
    /// (KT-337). `None` falls back to each provider's own default, and the
    /// matching key is read from `tokens` under that provider's slug.
    pub http_endpoints: Option<&'a crate::models::setup::HttpEndpoints>,
    /// Pre-built context files prompt (uploaded file contents for this discussion).
    pub context_files_prompt: &'a str,
    /// Discussion id this run targets, when known. Forwarded to the
    /// agent process as `KRONN_DISCUSSION_ID` so the in-process
    /// `kronn-internal` MCP bridge knows which discussion to introspect.
    /// `None` for one-off runs (e.g. workflow Agent steps that don't
    /// belong to a persistent discussion thread, or the auto-summary
    /// path itself).
    pub discussion_id: Option<&'a str>,
    /// CLI-only task-worker capability assembled by the discussion dispatcher.
    /// The MCP bridge receives this out-of-band through the child process
    /// environment; it is never rendered into the model prompt or accepted as
    /// a tool argument. HTTP workers keep using the in-process native executor.
    pub task_worker_context: Option<&'a TaskWorkerBridgeContext>,
    /// Ollama-only: a JSON Schema (a `TypedSchema` step's schema, already
    /// wrapped in the canonical envelope shape by the caller) forwarded as
    /// the `/api/chat` `format` param — grammar-constrained decoding +
    /// non-streaming. `None` for every other agent and for free-text steps;
    /// other agents get their schema via prompt injection, not here.
    pub ollama_format: Option<&'a serde_json::Value>,
    /// Explicit model, from a step's / QP's `AgentSettings.model`. When set it
    /// wins over `tier` (see `effective_model_flag`) for every agent that
    /// supports a `--model` flag (incl. both Ollama paths). `None` = resolve
    /// the model from `tier` as before.
    pub model_override: Option<&'a str>,
    /// KT-405 — persistent per-model context overrides (`ServerConfig`), keyed
    /// by exact model tag. `None` is reserved for callers with no server config
    /// in scope (mainly isolated tests); discussions and workflows pass it.
    /// The auto-derived cap still applies when the map has no matching key.
    /// The whole map rather than
    /// a pre-resolved value because the final model name — tier-resolved —
    /// is not always known until inside `start_ollama_http`.
    pub ollama_context_overrides: Option<&'a std::collections::HashMap<String, u64>>,
    /// Total timeout for one HTTP provider request, including the initial
    /// request that happens before an `AgentProcess` exists. Discussion paths
    /// pass the exact visible hosted/local wall-clock budget here.
    pub http_request_timeout: Option<std::time::Duration>,
    /// Optional lifecycle owned by the caller (discussion/workflow). HTTP
    /// agents derive a child token from it so cancellation also interrupts the
    /// initial request, before an `AgentProcess`/lifeline exists.
    pub cancel_token: Option<tokio_util::sync::CancellationToken>,
}

impl<'a> AgentStartConfig<'a> {
    /// Base config from the 4 always-required fields. Every optional field
    /// (work_dir / skills / directives / profiles / mcp override / tier /
    /// model_tiers / context files / discussion_id) defaults to empty/None.
    ///
    /// Call sites override only what differs via struct-update syntax:
    /// ```ignore
    /// AgentStartConfig {
    ///     full_access: true,
    ///     skill_ids: &skills,
    ///     tier: ModelTier::Reasoning,
    ///     ..AgentStartConfig::new(&agent_type, &project_path, &prompt, &tokens)
    /// }
    /// ```
    /// `Default` can't be derived because `agent_type` + `tokens` are
    /// required references with no sensible default (TokensConfig isn't
    /// const-constructible). This constructor is the equivalent ergonomics
    /// for the 11 spawn sites that previously repeated `mcp_context_override:
    /// None, model_tiers: None, context_files_prompt: "", discussion_id:
    /// None, …` verbatim.
    pub fn new(
        agent_type: &'a AgentType,
        project_path: &'a str,
        prompt: &'a str,
        tokens: &'a TokensConfig,
    ) -> Self {
        Self {
            agent_type,
            project_path,
            prompt,
            tokens,
            work_dir: None,
            full_access: false,
            skill_ids: &[],
            directive_ids: &[],
            profile_ids: &[],
            mcp_context_override: None,
            tier: ModelTier::Default,
            model_tiers: None,
            tools: None,
            http_endpoints: None,
            context_files_prompt: "",
            discussion_id: None,
            task_worker_context: None,
            ollama_format: None,
            model_override: None,
            ollama_context_overrides: None,
            http_request_timeout: None,
            cancel_token: None,
        }
    }
}

/// What an HTTP agent is told about its own capabilities.
///
/// Two different truths, and stating the wrong one silently defeats the tool
/// loop: with tools declared on the request, a prose "you have no tools" makes
/// the model refuse to call them (observed 2026-08-09 — `tools_declared=5` on
/// the wire, yet it answered "je ne peux pas exécuter d'outils ici"). File
/// access is absent either way.
pub(crate) fn http_agent_tools_notice(has_tools: bool) -> &'static str {
    if has_tools {
        "=== TOOLS ===\n\nYou have executable Kronn-native tools declared with \
         this request; use ONLY the ones declared. You have no shell and no MCP \
         server. You have a workspace: `find_files` with a glob (e.g. `**/*.rs`) to locate files in ONE call, \
         `list_files` to inspect a directory (pass `recursive` to walk it), \
         `search_text` to find a literal string across files by path and line — \
         prefer it to reading whole files. `read_file` reads one, sliced with \
         `offset`/`limit` for a large file. To CHANGE a file you have read: \
         `edit_file`/`edit_lines` replace an exact region (never guess an \
         edit you have not anchored to what you actually read); \
         `insert_after_line` preserves its anchor mechanically; `write_file` \
         creates a NEW file, or overwrites an existing one only with the exact \
         receipt (`expected_sha256` or equivalent) proving you read it first — \
         both refuse and tell you why on a path that escapes the workspace or a \
         stale/missing receipt. Never claim to have read, edited or written a \
         file you did not obtain through those tools. `disc_list` and `disc_read` read the OTHER discussions of this project (read-only, \
         and only this project: what you read leaves for whoever hosts this model). \
         `git_status`, `git_diff` and `git_log` read this workspace's repository, so you \
         can see WHAT changed instead of asking for a pasted diff; `truncated` marks a \
         diff too large to return whole. `git_commit` commits your own changes to the \
         local branch — no push, no merge — once the DoD is met, right before you \
         deliver. `web_fetch` retrieves an http(s) URL \
         server-side; private and loopback addresses are refused, and a `truncated` \
         flag tells you when you are seeing only part of a document — say so rather \
         than concluding from a partial read. Configured REST APIs are NOT MCP \
         servers: discover them with `mcp_list` (legacy name), inspect one with \
         `api_endpoints`, then execute it with `api_call`, or use `qa_list`/`qa_run` \
         for a saved Quick API. Do not search for or invent a vendor MCP when one of \
         those tools lists the requested API. When the answer depends on data you \
         were not given, CALL the matching tool instead of guessing or saying you \
         cannot. And a tool's output is something you RECEIVE, never something you \
         compose: never present, quote or summarise a result before the call has \
         returned it. Call, wait, then report what actually came back — a fabricated \
         result that merely has the right shape is worse than admitting you have not \
         looked yet."
    } else {
        "=== TOOLS ===\n\nYou have NO executable tools and NO file access in this \
         mode. You cannot read, write or modify ANY file (including docs/). Answer \
         strictly from the context provided above; never claim to run a tool, call \
         an API, or read a file."
    }
}

fn prelocalized_http_worker_notice() -> &'static str {
    "=== MANAGED WORKER ===\n\nKronn exposes exactly one phase-specific native tool at a time. \
     You have no shell, no MCP server and no capability outside the frozen target. \
     Call only the declared tool; its schema contains the complete arguments Kronn \
     will accept. Tool results are authoritative. Never invent a read, mutation, \
     commit or delivery, and state the exact blocker if the constrained evidence is insufficient."
}

/// Agents executed over the HTTP chat path instead of a CLI subprocess.
/// They share every consequence of that: no filesystem or stdio MCP, a
/// bounded native tool-execution loop when an executor is supplied, and
/// token-level (not line-level) streaming.
/// Speaks the OpenAI wire format on the shared HTTP path: same codec, same
/// bearer auth, same `/v1/chat/completions` shape. Only the endpoint and the
/// credential slot differ (LiteLLM proxy vs NVIDIA hosted) — KT-337.
pub(crate) fn is_openai_wire_agent(agent_type: &AgentType) -> bool {
    matches!(agent_type, AgentType::LiteLlm | AgentType::Nvidia)
}

/// How many runs of one agent may be in flight at once, as the JSON map the
/// dispatcher hands to the claim. An agent ABSENT from the map is unlimited.
///
/// Local agents are always capped, because the machine is the limit: Ollama
/// serves a single inference slot (default 1 — a second run does not compute
/// sooner, it queues and throws away the KV cache the first one warmed), and a
/// CLI agent is a process contending for its own auth files (default 1).
///
/// Remote providers are unlimited by default: LiteLLM and NVIDIA are endpoints
/// someone else scales, so holding them back only makes a batch slower. They
/// still accept a cap when the operator sets one — there the limit is spend and
/// provider rate limits, not this machine.
pub(crate) fn is_local_agent(agent: &AgentType) -> bool {
    matches!(
        agent,
        AgentType::ClaudeCode
            | AgentType::Codex
            | AgentType::GeminiCli
            | AgentType::Kiro
            | AgentType::Vibe
            | AgentType::CopilotCli
            | AgentType::Ollama
    )
}

pub(crate) fn agent_concurrency_limits(
    cfg: &crate::models::setup::AgentsConfig,
    local_global_limit: usize,
) -> String {
    const LOCAL: [AgentType; 7] = [
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::GeminiCli,
        AgentType::Kiro,
        AgentType::Vibe,
        AgentType::CopilotCli,
        AgentType::Ollama,
    ];
    const REMOTE: [AgentType; 2] = [AgentType::LiteLlm, AgentType::Nvidia];

    let per_agent = |agent: &AgentType| match agent {
        AgentType::ClaudeCode => &cfg.claude_code,
        AgentType::Codex => &cfg.codex,
        AgentType::GeminiCli => &cfg.gemini_cli,
        AgentType::Kiro => &cfg.kiro,
        AgentType::Vibe => &cfg.vibe,
        AgentType::CopilotCli => &cfg.copilot_cli,
        AgentType::Ollama => &cfg.ollama,
        AgentType::LiteLlm => &cfg.lite_llm,
        AgentType::Nvidia | AgentType::Custom => &cfg.nvidia,
    };

    let mut map = serde_json::Map::new();
    // Reserved admission key consumed by db::agent_dispatch. It is deliberately
    // not an AgentType: remote HTTP routes neither consume nor obey this pool.
    map.insert(
        "__local_global".into(),
        serde_json::json!(local_global_limit.max(1)),
    );
    for agent in LOCAL.iter() {
        let default = match agent {
            AgentType::Ollama => 1,
            _ => 1,
        };
        let limit = per_agent(agent).concurrency.unwrap_or(default).max(1);
        map.insert(format!("{agent:?}"), serde_json::json!(limit));
    }
    // Only an explicit operator choice caps a remote endpoint; otherwise it
    // stays out of the map, i.e. unlimited.
    for agent in REMOTE.iter() {
        if let Some(limit) = per_agent(agent).concurrency {
            map.insert(format!("{agent:?}"), serde_json::json!(limit.max(1)));
        }
    }
    serde_json::Value::Object(map).to_string()
}

pub(crate) fn is_http_chat_agent(agent_type: &AgentType) -> bool {
    matches!(
        agent_type,
        AgentType::Ollama | AgentType::LiteLlm | AgentType::Nvidia
    )
}

pub(crate) fn http_agent_identity_context(agent_type: &AgentType, model: &str) -> String {
    match agent_type {
        AgentType::Ollama => format!(
            "=== RUNTIME IDENTITY ===\nYou are the local model `{model}` served by Ollama. \
             You are not Claude, ChatGPT, or another agent mentioned in the conversation history. \
             Messages labelled `Ollama` and the `@ollama` alias address you; labels such as \
             `LiteLlm`, `ClaudeCode`, or their aliases address other participants. Never copy \
             another participant's self-identification. When asked who you are, identify yourself \
             as `{model}` running locally through Ollama."
        ),
        AgentType::LiteLlm => format!(
            "=== RUNTIME IDENTITY ===\nYou are the model route `{model}` served through the \
             LiteLLM proxy. Messages labelled `LiteLlm` and the `@litellm` alias address you. \
             LiteLLM is the transport, not your model name. Never copy another participant's \
             self-identification from the conversation history."
        ),
        // Without this arm the catch-all below returns an empty context and the
        // model invents an identity: asked who it was, it answered "Je suis Romu"
        // — the human's own first name, lifted from the conversation. Naming the
        // model and the alias is what stops that.
        AgentType::Nvidia => format!(
            "=== RUNTIME IDENTITY ===\nYou are the model `{model}`, served by NVIDIA over its \
             OpenAI-compatible API. NVIDIA is the host, not your model name, and it is not your \
             identity either. Messages labelled `Nvidia` and the `@nvidia` alias address you; \
             labels such as `LiteLlm`, `Ollama`, `ClaudeCode` or their aliases address other \
             participants, and a human's name in the history is never yours. When asked who you \
             are, answer `{model}` served by NVIDIA. Never copy another participant's \
             self-identification."
        ),
        _ => String::new(),
    }
}

/// Resolve a ModelTier to a concrete --model flag value for a given agent.
/// Returns None for Default tier or agents without --model support.
pub(crate) fn resolve_model_flag(
    agent_type: &AgentType,
    tier: ModelTier,
    overrides: Option<&ModelTiersConfig>,
) -> Option<String> {
    // Check user overrides first (all tiers including Default)
    if let Some(cfg) = overrides {
        let agent_cfg = match agent_type {
            AgentType::ClaudeCode => &cfg.claude_code,
            AgentType::Codex => &cfg.codex,
            AgentType::GeminiCli => &cfg.gemini_cli,
            AgentType::Kiro => &cfg.kiro,
            AgentType::Vibe => &cfg.vibe,
            AgentType::CopilotCli => &cfg.copilot_cli,
            AgentType::Ollama => &cfg.ollama,
            AgentType::LiteLlm => &cfg.lite_llm,
            AgentType::Nvidia => &cfg.nvidia,
            AgentType::Custom => return None,
        };
        let override_val = match tier {
            ModelTier::Economy => &agent_cfg.economy,
            ModelTier::Reasoning => &agent_cfg.reasoning,
            // `Default` tier now honors a user override too — primarily
            // for Ollama, where the OllamaCard picker writes here so the
            // user's preferred model wins over the built-in qwen3 fallback
            // below. Backward compatible: `None` (the common case) falls
            // through to the built-in match.
            ModelTier::Default => &agent_cfg.default,
        };
        if let Some(ref val) = override_val {
            if !val.is_empty() {
                return Some(val.clone());
            }
        }

        // Ollama has no built-in notion of tiers: the user picks ONE model in
        // the OllamaCard, which writes the `default` slot. So an empty
        // economy/reasoning slot must fall back to that single configured model
        // — NOT to a portability fallback the user never asked for. Without
        // this, someone who set "qwen3:32b" as their Ollama default but whose
        // discussions run at the reasoning tier would silently get
        // "qwen3:30b-a3b" instead. (Cloud agents keep distinct per-tier
        // built-ins below, since haiku/sonnet/opus are genuinely different.)
        if is_http_chat_agent(agent_type) {
            if let Some(ref d) = agent_cfg.default {
                if !d.is_empty() {
                    return Some(d.clone());
                }
            }
        }
    }

    // Built-in defaults — explicit model for each tier so tiers are always distinct.
    // Default maps to the "standard" model, not "no flag" (which depends on user subscription).
    match (agent_type, tier) {
        (AgentType::ClaudeCode, ModelTier::Economy) => Some("haiku".into()),
        (AgentType::ClaudeCode, ModelTier::Default) => Some("sonnet".into()),
        (AgentType::ClaudeCode, ModelTier::Reasoning) => Some("opus".into()),
        // 2026-07: gpt-5.6 generation (sol=frontier, terra=balanced, luna=fast).
        (AgentType::Codex, ModelTier::Economy) => Some("gpt-5.6-luna".into()),
        (AgentType::Codex, ModelTier::Default) => None, // Codex default is fine
        (AgentType::Codex, ModelTier::Reasoning) => Some("gpt-5.6-sol".into()),
        (AgentType::GeminiCli, ModelTier::Economy) => Some("gemini-2.5-flash".into()),
        (AgentType::GeminiCli, ModelTier::Default) => None, // Gemini default is fine
        (AgentType::GeminiCli, ModelTier::Reasoning) => Some("gemini-3.1-pro-preview".into()),
        // Copilot's available model set is account/policy-dependent. The old
        // hard-coded `gpt-4o-mini` / `o4-mini` values are no longer accepted
        // by Copilot CLI 1.0.x and made an otherwise valid prompt fail before
        // execution. Let the CLI select its current account-compatible model
        // unless the user explicitly configured a tier override.
        (AgentType::CopilotCli, _) => None,
        // Ollama: the user normally picks a model per tier via the OllamaCard
        // (override above). These are the pulled-tag fallbacks when none is set,
        // deliberately portability-first (NOT tuned for a beefy machine):
        // qwen3:8b (~5 GB) fits almost any box, is fast, multilingual, and — key
        // — reliably honors `/no_think` so its output stays clean+parseable.
        // Economy is ALSO qwen3:8b, not qwen3:4b: benchmarking (2026-07-02)
        // showed qwen3:4b ignores `/no_think`, leaking reasoning + `\boxed{}`
        // wrappers into `content` → unusable for a step that parses the output,
        // and it wasn't even faster (the thinking made it SLOWER). 8b is the
        // reliable small-model floor; users who want lighter can still pick
        // qwen3:4b explicitly in the OllamaCard economy slot. Reasoning is the
        // only heavy fallback (qwen3:30b-a3b MoE) — an explicit opt-in tier;
        // small machines should override it. Never bare tags like `qwen3` (not
        // pullable) or `llama3.2` (not pulled) → opaque Ollama 404.
        (AgentType::Ollama, ModelTier::Default) => Some("qwen3:8b".into()),
        (AgentType::Ollama, ModelTier::Economy) => Some("qwen3:8b".into()),
        (AgentType::Ollama, ModelTier::Reasoning) => Some("qwen3:30b-a3b".into()),
        // LiteLLM deliberately has no built-in: the model names come from the
        // operator's `config.yaml`, so any guess here would 404. The user sets
        // one in the LiteLLM card and it covers every tier (see above).
        // Kiro, Vibe: no --model flag support
        _ => None,
    }
}

/// Resolve the effective `--model` value for a run: an explicit per-step /
/// per-QP `model_override` wins outright (blank is treated as unset); otherwise
/// fall back to the tier → model mapping (`resolve_model_flag`, which itself
/// honors the global OllamaCard overrides). Kept pure + `pub(crate)` so the
/// precedence is unit-tested without spawning a process.
pub(crate) fn effective_model_flag(
    model_override: Option<&str>,
    agent_type: &AgentType,
    tier: ModelTier,
    model_tiers: Option<&ModelTiersConfig>,
) -> Option<String> {
    match model_override {
        Some(m) if !m.trim().is_empty() => Some(m.to_string()),
        _ => resolve_model_flag(agent_type, tier, model_tiers),
    }
}

/// Start an agent process with minimal config (no skills/directives/profiles).
pub async fn start_agent(
    agent_type: &AgentType,
    project_path: &str,
    prompt: &str,
    tokens: &TokensConfig,
    full_access: bool,
) -> Result<AgentProcess, String> {
    start_agent_with_config(AgentStartConfig {
        full_access,
        ..AgentStartConfig::new(agent_type, project_path, prompt, tokens)
    })
    .await
}

/// Start an agent process with full configuration.
pub async fn start_agent_with_config(config: AgentStartConfig<'_>) -> Result<AgentProcess, String> {
    // Read MCP context: use override if provided (general discussions),
    // otherwise read from project filesystem.
    let mcp_context = if let Some(override_ctx) = config.mcp_context_override {
        override_ctx.to_string()
    } else if !config.project_path.is_empty() {
        crate::core::mcp_scanner::read_all_mcp_contexts(config.project_path)
    } else {
        String::new()
    };

    // Use compact format for agents with small context windows (eco-design).
    // 0.8.11 — Ollama added: its num_ctx is auto-sized but CAPPED (default
    // 8192, see OLLAMA_NUM_CTX_CAP); full skill injection measured at ~6k
    // tokens for a real 6-skill step = 74% of that budget, crowding out the
    // actual task context (the N2 PR-review diff). Local models also exploit
    // long generic skill dumps poorly (bench: sharp instructions > bulk
    // context) — the ~150-char compact summary keeps the pointer without the
    // cost. Steps that NEED full rules inline them in the prompt template.
    let compact = matches!(
        config.agent_type,
        AgentType::Codex
            | AgentType::Kiro
            | AgentType::Vibe
            | AgentType::Ollama
            | AgentType::LiteLlm
    );

    // Ensure this run's skills/profiles exist as native files in the
    // directory the agent ACTUALLY runs in.
    //
    // This must target `work_dir`, not `project_path`. For a workflow Agent
    // step the agent runs in an isolated git WORKTREE (`work_dir`), which is a
    // fresh checkout that does NOT contain the untracked `.claude/skills/`
    // dir synced to the project root — so a custom skill named in `skill_ids`
    // was invisible to the agent's Skill tool ("the skill isn't registered").
    // Syncing to the effective cwd lands `SKILL.md` where the agent discovers
    // it. For a normal discussion `work_dir` is None → falls back to
    // `project_path` (unchanged behaviour). Additive: only creates missing
    // files, never removes others.
    let agent_cwd = config.work_dir.unwrap_or(config.project_path);
    let native_sync_ok = if !agent_cwd.is_empty()
        && (!config.skill_ids.is_empty() || !config.profile_ids.is_empty())
    {
        let profile_ids_vec: Vec<String> = config.profile_ids.to_vec();
        crate::core::native_files::sync_project_native_files(
            agent_cwd,
            config.skill_ids,
            &profile_ids_vec,
        )
        .is_ok()
    } else {
        false
    };

    // If native files exist AND the agent discovers them (not all do — Vibe/Kiro don't),
    // send a lightweight hint (~15 tokens) instead of full content (~500-800 tokens).
    // Probe the SAME dir we synced to (`agent_cwd`) — probing project_path
    // while the agent runs in a worktree would mis-detect and send a hint for
    // a file the agent can't actually see.
    let native_skills = native_sync_ok
        && crate::core::native_files::supports_native_skills(config.agent_type)
        && crate::core::native_files::has_native_skills(agent_cwd, config.agent_type);
    let native_profiles = native_sync_ok
        && config.profile_ids.len() == 1 // Multi-profile always needs prompt injection
        && crate::core::native_files::supports_native_profiles(config.agent_type)
        && crate::core::native_files::has_native_profiles(agent_cwd, config.agent_type);

    // Build skills prompt — native hint (~15 tokens) or full injection (~500-800 tokens)
    let force_full_skill = config.skill_ids.iter().any(|id| id == "compare-quality");
    let skills_prompt = if force_full_skill {
        // Compare judges cannot discover a skill through a tool, and compact
        // injection would omit the normative anchors after the first lines.
        crate::core::skills::build_skills_prompt(config.skill_ids)
    } else if native_skills {
        crate::core::native_files::build_skills_reference_prompt(config.skill_ids)
    } else if compact {
        crate::core::skills::build_skills_prompt_compact(config.skill_ids)
    } else {
        crate::core::skills::build_skills_prompt(config.skill_ids)
    };

    // 0.8.8 PR-B — enforce mode auto-attaches the `kronn-doc-author` cheat-sheet
    // when the agent's project carries a `docs/AGENTS.md`, so an agent that
    // edits docs gets the `[src:]` / `kronn:section` discipline even if the user
    // never attached the skill. Idempotent (skipped when already in skill_ids)
    // and inert outside enforce. The content is injected inline (the skill isn't
    // in skill_ids, so the native-files path wouldn't write it to disk).
    let project_has_agents_md = !config.project_path.is_empty()
        && std::path::Path::new(config.project_path)
            .join("docs/AGENTS.md")
            .exists();
    let doc_author_prompt = if crate::core::anti_halluc::should_auto_attach_doc_author(
        crate::core::anti_halluc::current_mode(),
        config.skill_ids,
        project_has_agents_md,
    ) {
        crate::core::skills::get_skill("kronn-doc-author")
            .map(|s| s.content.to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };

    // Build directives prompt (always injected — no native format)
    let directives_prompt = crate::core::directives::build_directives_prompt(config.directive_ids);

    // Build profiles prompt.
    //
    // Originally the `native_profiles` branch returned an empty string on
    // the assumption that Claude Code (and friends) auto-loaded the agent
    // file from `.claude/agents/` at runtime. That assumption does NOT hold
    // in `--print` / one-shot mode: agent files there are only consulted
    // after an explicit `@agent-name` mention or the interactive `/agents`
    // command. Observed on discussion EW-7189: `translator` profile
    // activated, file synced to `.claude/agents/translator.md`, persona
    // silently ignored — the email draft came back in a bland tone with
    // no hint of "Lin the translator". Inject at least the compact
    // summary so the persona actually reaches the model.
    let profiles_prompt = if config.profile_ids.is_empty() {
        String::new()
    } else if native_profiles || compact {
        // `native_profiles` true → the full persona also lives in
        // `.claude/agents/<id>.md` on disk; the compact injection here is
        // a token-saving fallback in case the agent's one-shot mode
        // doesn't auto-pick the file up (which was the EW-7189 failure).
        crate::core::profiles::build_profiles_prompt_compact(config.profile_ids)
    } else {
        crate::core::profiles::build_profiles_prompt(config.profile_ids)
    };

    // A prelocalized HTTP worker is not a smaller general-purpose agent. Its
    // executor exposes one frozen tool per phase, so injecting user memory,
    // project-wide AGENTS.md and the full general tool tutorial only makes the
    // model repeatedly pay for capabilities it cannot use. Explicit skills,
    // profiles, directives and context files remain honoured below.
    let prelocalized_http_worker = is_http_chat_agent(config.agent_type)
        && config
            .tools
            .as_ref()
            .and_then(|executor| executor.worker_scope())
            .is_some();

    // 0.7.1 — user-scoped cross-project context : the `~/.kronn/user-context/`
    // markdown directory. Universal across all CLIs (no per-tool format
    // proliferation), opt-in (user creates the files), and stable for
    // prompt cache (alphabetical ordering inside the helper).
    let user_context = crate::core::user_context::read_user_context();

    // 0.7.1 — agent memory prelude : encourages agents to update `docs/`
    // when they discover stable facts, names the writable subfolders,
    // forbids `docs/AGENTS.md` direct edits, references the anti-secret
    // filter. Universal text, no per-agent customisation.
    let memory_prelude = crate::core::user_context::build_memory_prelude_prompt();

    // Combine all context parts with explicit section markers
    // (helps non-Claude agents distinguish instructions from task)
    let mut parts = Vec::new();
    // 0.8.7 anti-hallucination P1 — the sourcing directive goes FIRST, before
    // any other context, so it frames everything the agent reads. Gated by the
    // global mode (off → nothing injected, zero added tokens). This single
    // chokepoint covers every agent surface (disc, audit, architect, QP
    // improver, batch, summarization, orchestration) — see core::anti_halluc.
    if let Some(preamble) = crate::core::anti_halluc::preamble_if_active() {
        parts.push(preamble.to_string());
    }
    if !prelocalized_http_worker && !user_context.is_empty() {
        parts.push(format!(
            "=== USER CONTEXT (cross-project) ===\n\n{}",
            user_context
        ));
    }
    if !profiles_prompt.is_empty() {
        parts.push(format!("=== YOUR ROLE ===\n\n{}", profiles_prompt));
    }
    if !skills_prompt.is_empty() {
        parts.push(format!("=== YOUR EXPERTISE ===\n\n{}", skills_prompt));
    }
    if !prelocalized_http_worker && !doc_author_prompt.is_empty() {
        parts.push(format!(
            "=== DOC AUTHORING DISCIPLINE (enforce) ===\n\n{}",
            doc_author_prompt
        ));
    }
    if !config.context_files_prompt.is_empty() {
        parts.push(format!(
            "=== CONTEXT FILES ===\n\n{}",
            config.context_files_prompt
        ));
    }
    // HTTP agents have no filesystem. Two consequences the CLI agents don't:
    //  1. CLI agents read `docs/AGENTS.md` themselves from the project CWD —
    //     an HTTP model can't, so inject the doc inline (capped) or it
    //     answers with zero project grounding.
    //  2. Never describe tools in prose here. Doing so taught the model to
    //     HALLUCINATE calls (2026-07-01: it presented `fastly_execute` as its
    //     own capability). Tools are DECLARED instead, on the request's
    //     `tools` field — see `agents::tools`.
    if is_http_chat_agent(config.agent_type) {
        if project_has_agents_md && !prelocalized_http_worker {
            if let Ok(mut doc) = std::fs::read_to_string(
                std::path::Path::new(config.project_path).join("docs/AGENTS.md"),
            ) {
                const MAX_INLINE_DOC: usize = 24_000;
                if doc.len() > MAX_INLINE_DOC {
                    let mut cut = MAX_INLINE_DOC;
                    while !doc.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    doc.truncate(cut);
                    doc.push_str(
                        "\n\n[… truncated — full doc exceeds the inline context budget …]",
                    );
                }
                // The doc INDEXES other files (docs/examples/*.md, …) the model
                // cannot open — without this note it claims "all of docs/ is
                // accessible" and cites files it never saw (observed 2026-07-01).
                parts.push(format!(
                    "=== PROJECT DOCUMENTATION ===\n\nThe following is the ONLY project file \
                     you have. Other files it mentions (docs/*, source code) are NOT available: \
                     you can NOT read or open them, and if asked you must say so. You may cite \
                     their paths as pointers for the USER to open, but never present their \
                     content as known to you.\n\n{}",
                    doc
                ));
            }
        }
        parts.push(if prelocalized_http_worker {
            prelocalized_http_worker_notice().to_string()
        } else {
            http_agent_tools_notice(config.tools.is_some()).to_string()
        });
    } else if !mcp_context.is_empty() {
        parts.push(format!("=== AVAILABLE TOOLS ===\n\n{}", mcp_context));
    }
    if !directives_prompt.is_empty() {
        parts.push(format!(
            "=== OUTPUT REQUIREMENTS ===\n\n{}",
            directives_prompt
        ));
    }
    // The memory prelude tells agents to WRITE learnings back into docs/ —
    // meaningless for Ollama (no file access) and actively harmful: it made the
    // model claim "I can modify docs/ files" (observed 2026-07-01).
    if !is_http_chat_agent(config.agent_type) {
        parts.push(format!(
            "=== PROJECT MEMORY (write back what you learn) ===\n\n{}",
            memory_prelude
        ));
    }
    let extra_context = parts.join("\n\n");

    // 0.6.0 — observability log : trace ce qui est INJECTÉ à chaque
    // spawn d'agent. Utile quand un user signale "ma directive ne fait
    // rien" — on peut lui dire de filtrer `kronn logs | grep injection`
    // pour vérifier que le payload est bien passé. INFO-level pour qu'il
    // reste visible sans flag de debug, mais sur un target dédié pour
    // pouvoir le couper si trop verbeux. Empty arrays sont volontairement
    // loggés (un user qui voit `directive_ids: []` comprend tout de suite
    // que sa sélection n'a pas été persistée).
    tracing::info!(
        target: "kronn::agent::injection",
        agent = ?config.agent_type,
        profile_ids = ?config.profile_ids,
        skill_ids = ?config.skill_ids,
        directive_ids = ?config.directive_ids,
        directives_prompt_len = directives_prompt.len(),
        extra_context_len = extra_context.len(),
        "agent prompt injection summary"
    );

    // ── Ollama HTTP path ────────────────────────────────────────────────
    // Ollama uses the HTTP API (/api/chat) instead of a CLI process.
    // This gives us: (1) separate system/user message roles — the model
    // doesn't confuse MCP context with the user's question, (2) token
    // counts in the response, (3) works without the ollama binary (Docker).
    if is_http_chat_agent(config.agent_type) {
        let model_flag = effective_model_flag(
            config.model_override,
            config.agent_type,
            config.tier,
            config.model_tiers,
        );
        // LiteLLM has no safe built-in default: model ids come from the
        // operator's `config.yaml`, so guessing one yields an opaque 404.
        let model = match (config.agent_type, model_flag.as_deref()) {
            (_, Some(m)) if !m.is_empty() => m,
            (AgentType::LiteLlm, _) => {
                return Err(
                    "No LiteLLM model configured. Pick one in Settings → Agents → \
                            LiteLLM, or set a model override on the step."
                        .into(),
                )
            }
            // Same reasoning as LiteLLM, for a different reason: the NVIDIA
            // catalogue lists ~100 ids the ACCOUNT may not be entitled to, so a
            // guess yields a 404 (or a request that never answers) instead of a
            // readable refusal. Never invent one (KT-337).
            (AgentType::Nvidia, _) => {
                return Err(
                    "No NVIDIA model configured. Pick one in Settings → Agents → \
                            NVIDIA, or set a model override on the step."
                        .into(),
                )
            }
            _ => "qwen3:8b",
        };
        // Both OpenAI-compatible providers read their endpoint and key from their
        // own slot: LiteLLM's proxy is operator-hosted (config or env), NVIDIA's
        // is the hosted service (env override, else the public endpoint).
        let http_base_url = config
            .http_endpoints
            .and_then(|endpoints| endpoints.for_agent(config.agent_type));
        let provider_slug = match config.agent_type {
            AgentType::Nvidia => crate::api::nvidia::PROVIDER,
            _ => "litellm",
        };
        return start_ollama_http(
            config.agent_type,
            config.prompt,
            &extra_context,
            model,
            config.ollama_format,
            http_base_url,
            config.tokens.active_key_for(provider_slug),
            config.tools.clone(),
            config.ollama_context_overrides,
            config.http_request_timeout,
            config.cancel_token.as_ref(),
        )
        .await;
    }

    // Resolve model: explicit per-step/per-QP override wins, else tier → model.
    let model_flag = effective_model_flag(
        config.model_override,
        config.agent_type,
        config.tier,
        config.model_tiers,
    );

    let task_worker = config.task_worker_context.is_some();
    if task_worker
        && *config.agent_type == AgentType::Codex
        && codex_task_worker_mcp_override().is_none()
    {
        return Err(
            "Codex task worker cannot start: the kronn-internal delivery bridge is unavailable"
                .to_string(),
        );
    }
    // Use work_dir (or project_path) for the agent's CWD
    let effective_work_dir = config.work_dir.unwrap_or(config.project_path);
    let work_dir = if effective_work_dir.is_empty() {
        // Global discussion: use a temp working directory
        std::env::temp_dir()
    } else {
        let container_path = crate::core::scanner::resolve_host_path(effective_work_dir);
        if container_path.exists() {
            container_path
        } else {
            let p = PathBuf::from(effective_work_dir);
            if !p.exists() {
                return Err(format!("Project path not found: {}", p.display()));
            }
            p
        }
    };
    let (binary, npx_pkg, mut args, env_key, stderr_mode, output_mode) =
        agent_command_with_task_worker_policy(
            config.agent_type,
            config.prompt,
            config.full_access,
            &extra_context,
            model_flag.as_deref(),
            task_worker,
            task_worker.then_some(work_dir.as_path()),
        );

    // Claude Code in --print mode does NOT auto-load .mcp.json from CWD.
    // Explicitly pass it via --mcp-config so MCP tools are available.
    // IMPORTANT: --mcp-config must come BEFORE --append-system-prompt and the
    // prompt argument, because --append-system-prompt consumes the next
    // positional arg. If --mcp-config is inserted between them, Claude Code
    // mis-parses the arguments and fails with "MCP config file not found".
    if *config.agent_type == AgentType::ClaudeCode {
        if task_worker {
            // Generated worktrees intentionally do not copy the project's
            // gitignored `.mcp.json`. Looking only in CWD let Claude edit and
            // commit successfully, then discover that the one mandatory
            // delivery tool did not exist. Build a fail-closed inline config
            // from the authoritative project config instead. Filtering to the
            // single internal server, together with `--strict-mcp-config`,
            // prevents an untrusted task from inheriting every project MCP.
            let project_root = crate::core::scanner::resolve_host_path(config.project_path);
            let worker_mcp = claude_task_worker_mcp_config(&project_root)?;
            insert_claude_mcp_config(&mut args, worker_mcp, true);
        } else {
            let mcp_json = work_dir.join(".mcp.json");
            if mcp_json.exists() {
                insert_claude_mcp_config(&mut args, mcp_json.to_string_lossy().to_string(), false);
            }
        }
    }

    // API key is optional — agents use their own local auth by default
    let api_key = get_api_key(env_key, config.tokens);

    // On macOS hosts, host-mounted kiro-cli is not runnable in Linux containers.
    // Ensure a Linux kiro-cli exists locally before spawning Kiro.
    if matches!(config.agent_type, AgentType::Kiro) {
        ensure_kiro_cli_available().await?;
    }

    // Claude Code accepts the positional prompt argument OR reads it from
    // stdin when absent. Writing large prompts to stdin side-steps the
    // kernel ARG_MAX cap (~128 KiB / arg on Linux) that broke EW-7189.
    // We also defensively truncate `--append-system-prompt` value since
    // that one still travels through argv.
    let stdin_prompt: Option<String> = if *config.agent_type == AgentType::ClaudeCode {
        // Pop the last arg — by construction of agent_command it is the
        // prompt (see the Claude branch there).
        let popped = args.pop();
        if let Some((original_bytes, truncated_bytes)) =
            truncate_claude_system_prompt_argument(&mut args)
        {
            tracing::warn!(
                "Truncating --append-system-prompt from {} bytes to {} to avoid ARG_MAX (E2BIG). \
                Consider trimming skills / MCP context.",
                original_bytes,
                truncated_bytes
            );
        }
        popped
    } else {
        None
    };

    if task_worker && *config.agent_type == AgentType::ClaudeCode {
        probe_claude_task_worker_auth(binary, npx_pkg, &work_dir).await?;
    }

    // Try direct binary first, then npx fallback
    let mut child = match try_spawn(
        binary,
        None,
        &args,
        &work_dir,
        env_key,
        api_key.as_deref(),
        stdin_prompt.as_deref(),
        config.discussion_id,
        config.task_worker_context,
    ) {
        Ok(c) => c,
        Err(e) => {
            tracing::info!("Direct binary '{}' failed ({}), trying npx...", binary, e);
            if let Some(pkg) = npx_pkg {
                try_spawn(
                    "npx",
                    Some(pkg),
                    &args,
                    &work_dir,
                    env_key,
                    api_key.as_deref(),
                    stdin_prompt.as_deref(),
                    config.discussion_id,
                    config.task_worker_context,
                )?
            } else {
                return Err(e);
            }
        }
    };

    let (tx, rx) = mpsc::channel::<String>(256);
    let stderr_capture = Arc::new(Mutex::new(Vec::new()));
    let mut stderr_handle: Option<tokio::task::JoinHandle<()>> = None;

    // Always stream stdout
    if let Some(stdout) = child.stdout.take() {
        let tx_out = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            // Don't conflate a read error (e.g. non-UTF-8 output) with EOF:
            // the stream is truncated either way, but truncation must be
            // visible in the logs.
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if tx_out.send(line).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("agent stdout read error (output truncated): {}", e);
                        break;
                    }
                }
            }
        });
    }

    if let Some(stderr) = child.stderr.take() {
        match stderr_mode {
            StderrMode::Merge => {
                let tx_err = tx;
                tokio::spawn(async move {
                    let mut lines = BufReader::new(stderr).lines();
                    loop {
                        match lines.next_line().await {
                            Ok(Some(line)) => {
                                if tx_err.send(line).await.is_err() {
                                    break;
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                tracing::warn!("agent stderr read error (output truncated): {}", e);
                                break;
                            }
                        }
                    }
                });
            }
            StderrMode::StdoutOnly => {
                // Log stderr for debugging but don't stream it to the user.
                // Capture it so we can show it on failure.
                let capture = stderr_capture.clone();
                stderr_handle = Some(tokio::spawn(async move {
                    let mut lines = BufReader::new(stderr).lines();
                    loop {
                        match lines.next_line().await {
                            Ok(Some(line)) => {
                                tracing::debug!("agent stderr: {}", line);
                                if let Ok(mut buf) = capture.lock() {
                                    buf.push(line);
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                tracing::warn!(
                                    "agent stderr read error (capture truncated): {}",
                                    e
                                );
                                break;
                            }
                        }
                    }
                }));
            }
        }
    }

    // Store process group ID on Unix for proper termination of the entire
    // process tree on cancellation. This is used by AgentIo::kill() to ensure
    // all descendants are terminated, not just the parent process.
    // We set setpgid(0,0) in pre_exec, so a representable child PID is the PGID.
    let pgid = if cfg!(unix) {
        child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|pgid| *pgid > 1)
    } else {
        None
    };

    Ok(AgentProcess {
        child,
        output_mode,
        work_dir,
        agent_type: config.agent_type.clone(),
        rx,
        stderr_capture,
        stderr_task: stderr_handle,
        http_cancel: None,
        pgid,
    })
}

/// Ensure kiro-cli is available inside the container.
/// Uses the official installer if missing.
pub(crate) async fn ensure_kiro_cli_available() -> Result<(), String> {
    if super::find_binary("kiro-cli").is_some() {
        return Ok(());
    }

    tracing::info!("kiro-cli not found, installing Linux kiro-cli...");
    let output = async_cmd("sh")
        .args([
            "-c",
            "command -v unzip >/dev/null 2>&1 || { echo 'Missing dependency: unzip' >&2; exit 127; }; \
             curl -fsSL --connect-timeout 10 --max-time 300 https://cli.kiro.dev/install | bash",
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to launch Kiro installer: {e}"))?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Kiro CLI install failed (exit {:?}): {}{}",
            output.status.code(),
            stderr.trim(),
            if stdout.trim().is_empty() {
                String::new()
            } else {
                format!("\n{stdout}")
            }
        ));
    }

    if super::find_binary("kiro-cli").is_none() {
        return Err(
            "Kiro CLI installed but not found in PATH. Ensure kiro-cli is accessible from your shell."
                .into(),
        );
    }

    Ok(())
}

/// Resolve the path to vibe-runner.py.
/// Searches: env override → Docker bundle → next to executable → Tauri resource → cargo manifest (dev).
fn vibe_runner_path() -> String {
    // 0. Explicit override (allows custom deployments)
    if let Ok(custom) = std::env::var("KRONN_VIBE_RUNNER") {
        if std::path::Path::new(&custom).exists() {
            return custom;
        }
    }
    // 1. Docker: scripts are copied into /app/scripts/
    let docker_path = "/app/scripts/vibe-runner.py";
    if std::path::Path::new(docker_path).exists() {
        return docker_path.to_string();
    }
    // 2. Native/Tauri: next to the running executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Candidate paths relative to the binary
            let candidates = [
                dir.join("scripts").join("vibe-runner.py"),
                dir.join("..").join("scripts").join("vibe-runner.py"),
                // macOS .app bundle: Contents/Resources/scripts/
                dir.join("..")
                    .join("Resources")
                    .join("scripts")
                    .join("vibe-runner.py"),
                // Windows: alongside the .exe
                dir.join("vibe-runner.py"),
            ];
            for candidate in &candidates {
                if candidate.exists() {
                    return candidate.to_string_lossy().to_string();
                }
            }
        }
    }
    // 3. Dev mode: relative to cargo manifest
    let dev_path = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/vibe-runner.py");
    dev_path.to_string()
}

/// Resolve `disc-introspection-mcp.py` using the same lookup chain as
/// `vibe_runner_path`. Returns `None` if the script can't be located —
/// the caller should skip the kronn-internal MCP entry rather than
/// inject a broken path that the agent will choke on.
///
/// **Container vs host path** — when Kronn runs in Docker, the script
/// lives at `/app/scripts/...` (built into the image). The user's host
/// CLI (`kiro-cli`, `claude`, …) cannot reach that path, so injecting it
/// into project-level config files (`.mcp.json`, `.kiro/settings/…`)
/// breaks the host CLI with `Broken pipe (os error 32)` on every
/// invocation. Use [`disc_introspection_mcp_path_for_shared_config`] in
/// any code path that writes to a file the host CLI may read.
pub(crate) fn disc_introspection_mcp_path() -> Option<String> {
    if let Ok(custom) = std::env::var("KRONN_DISC_INTROSPECTION_MCP") {
        if std::path::Path::new(&custom).exists() {
            return Some(custom);
        }
    }
    let docker_path = "/app/scripts/disc-introspection-mcp.py";
    if std::path::Path::new(docker_path).exists() {
        return Some(docker_path.to_string());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidates = [
                dir.join("scripts").join("disc-introspection-mcp.py"),
                dir.join("..")
                    .join("scripts")
                    .join("disc-introspection-mcp.py"),
                dir.join("..")
                    .join("Resources")
                    .join("scripts")
                    .join("disc-introspection-mcp.py"),
                dir.join("disc-introspection-mcp.py"),
            ];
            for c in &candidates {
                if c.exists() {
                    return Some(c.to_string_lossy().to_string());
                }
            }
        }
    }
    let dev_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/scripts/disc-introspection-mcp.py"
    );
    if std::path::Path::new(dev_path).exists() {
        return Some(dev_path.to_string());
    }
    None
}

/// Resolve a path to `disc-introspection-mcp.py` that's valid for **both**
/// Kronn-spawned (in-container) agents AND the user's host CLI sessions
/// (`kiro-cli`, `claude`, `gemini` run directly from the shell).
///
/// Used when writing entries to project-level config files (`.mcp.json`,
/// `.kiro/settings/mcp.json`, `.gemini/settings.json`, `~/.codex/config.toml`)
/// — those are read by every CLI the user has installed, not just the
/// container.
///
/// **Lookup order**:
///   1. `KRONN_INTROSPECTION_PUBLIC_PATH` env (set by `docker-compose.yml`
///      to the host path that's also self-mounted at the same absolute
///      path inside the container — same string resolves both sides).
///   2. The plain [`disc_introspection_mcp_path`] **if and only if**
///      we're not in Docker (= path is already host-valid).
///
/// Returns `None` when no shared path is reachable — in that case the
/// caller skips the `kronn-internal` injection. Kronn-spawned agents
/// then lose MCP-based introspection but still have the slash-marker
/// fallback (`KRONN:DISC_*` lines parsed in `slash_markers.rs`); the
/// trade-off keeps host CLIs working cleanly. See user report
/// 2026-05-10 (`kronn-internal Broken pipe (os error 32)` from
/// `kiro-cli` on the host).
pub(crate) fn disc_introspection_mcp_path_for_shared_config() -> Option<String> {
    if let Ok(public) = std::env::var("KRONN_INTROSPECTION_PUBLIC_PATH") {
        if std::path::Path::new(&public).exists() {
            return Some(public);
        }
    }
    // Native (non-Docker) Kronn — `disc_introspection_mcp_path()` already
    // returns a host-valid path. Detect Docker via the canonical
    // `/.dockerenv` marker file.
    if !std::path::Path::new("/.dockerenv").exists() {
        return disc_introspection_mcp_path();
    }
    None
}

/// Bounds for the local Ollama context window. The Ollama default is huge
/// (up to 256K tokens for some qwen3 tags); an oversized KV cache balloons
/// memory — e.g. llama3.3:70b at 128K ctx needs ~66 GB and spills onto the
/// CPU (measured 0.2 tok/s vs 12.5 at 8K, 100% GPU). We therefore cap the
/// window and never let a local step silently request a giant one.
///
/// The 8192 default is CPU-safe/portable. On a GPU box with RAM headroom a
/// larger window helps big-context steps (multi-file review, long docs) with no
/// CPU cliff — so the cap is overridable via `KRONN_OLLAMA_NUM_CTX_CAP`
/// (clamped to at least the floor; a bad value falls back to the default).
const OLLAMA_NUM_CTX_CAP: u64 = 8192;
pub(crate) const OLLAMA_NUM_CTX_FLOOR: u64 = 2048;
/// Persistent Settings overrides are bounded against accidental values that
/// would allocate an absurd KV cache. The process-global env override remains
/// the deliberately unbounded break-glass path.
pub(crate) const OLLAMA_NUM_CTX_OVERRIDE_MAX: u64 = 1_048_576;

/// Fallback ceiling when the machine's memory cannot be read. RAM-blind, so it
/// is the conservative figure that was the flat ceiling before KT-401.
const OLLAMA_NUM_CTX_BLIND_CEILING: u64 = 32768;

/// Ceiling for the AUTO-derived cap, derived from installed memory.
///
/// A model advertising 262 144 tokens is not an invitation to allocate them:
/// the KV cache for a 27B model is on the order of 200 KB per token, so a full
/// window is tens of gigabytes and spilling it onto the CPU is the 0.2 tok/s
/// cliff documented above. But a flat 32K was the opposite error — it silently
/// throttled a large model on a large machine, and said nothing about it.
///
/// Tiers, not a formula: the per-token cost depends on layers, KV heads and
/// quantisation, none of which `/api/show` reports reliably. A coarse ceiling
/// that is honest about being coarse beats a precise-looking number built on
/// figures we do not have. Installed memory, never free memory — free memory
/// changes minute to minute and would make two identical runs differ.
pub(crate) fn ram_derived_ceiling(total_bytes: Option<u64>) -> u64 {
    const GB: u64 = 1024 * 1024 * 1024;
    match total_bytes {
        None => OLLAMA_NUM_CTX_BLIND_CEILING,
        Some(bytes) if bytes < 16 * GB => 8_192,
        Some(bytes) if bytes < 32 * GB => 16_384,
        Some(bytes) if bytes < 64 * GB => 32_768,
        Some(bytes) if bytes < 128 * GB => 65_536,
        Some(_) => 131_072,
    }
}

/// Installed physical memory, or `None` when the platform will not say.
pub(crate) fn total_system_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let output = crate::core::cmd::sync_cmd("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        String::from_utf8(output.stdout).ok()?.trim().parse().ok()
    }
    #[cfg(target_os = "linux")]
    {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kilobytes: u64 = meminfo
            .lines()
            .find_map(|line| line.strip_prefix("MemTotal:"))?
            .split_whitespace()
            .next()?
            .parse()
            .ok()?;
        Some(kilobytes * 1024)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    None
}

/// Where the effective window came from. The remedy differs for each, and a
/// throttled model must be able to say so rather than look like its own limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CtxCapOrigin {
    /// `KRONN_OLLAMA_NUM_CTX_CAP` — the operator chose this number.
    OperatorOverride,
    /// A persistent per-model override, set once through the UI/API and
    /// remembered across restarts — see `resolve_ctx_cap_for_model`.
    ModelOverride,
    /// The model's own trained context, used in full.
    ModelWindow,
    /// Below what the model offers, because of what this machine can hold.
    MachineCeiling { model_limit: u64 },
    /// Ollama did not answer `/api/show`, so nothing better was known.
    PortableFallback,
}

#[derive(Debug, Clone)]
pub(crate) struct CtxCap {
    pub value: u64,
    pub origin: CtxCapOrigin,
}

impl CtxCap {
    /// The sentence a run owes the person reading it when the model is being
    /// held below its own capability. `None` when it is not.
    pub fn throttle_notice(&self, model: &str) -> Option<String> {
        match self.origin {
            CtxCapOrigin::MachineCeiling { model_limit } => Some(format!(
                "{model} supports a {model_limit}-token context; Kronn is running it at {} \
                 — the ceiling this machine's memory allows. Raise it with \
                 KRONN_OLLAMA_NUM_CTX_CAP if the RAM is there.",
                self.value
            )),
            // KT-405 — a prompt fitting inside 8192 is not proof the model was
            // given its real window: Ollama was simply never asked. Silence
            // here reads as "this model's context is 8192", which is only
            // ever true of the fallback that fires when /api/show did not
            // answer. Loud in every run, not just the ones that overflow it.
            CtxCapOrigin::PortableFallback => Some(format!(
                "Kronn could not learn {model}'s trained context from Ollama's \
                 /api/show and is running it at the portable fallback of {} \
                 tokens — almost certainly smaller than what the model actually \
                 supports. Check that Ollama is reachable and the model is \
                 pulled, then retry.",
                self.value
            )),
            CtxCapOrigin::OperatorOverride
            | CtxCapOrigin::ModelOverride
            | CtxCapOrigin::ModelWindow => None,
        }
    }
}
/// KT-382 — how many times to ask Ollama for a model's trained context before
/// falling back. Two, not more: the failure this covers is a cold load, and a
/// longer ladder would delay every genuinely-offline run by the same amount.
const OLLAMA_SHOW_ATTEMPTS: usize = 2;
const OLLAMA_SHOW_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_millis(750);

/// Pure parse of the ctx-cap override (split out so it's unit-testable without
/// mutating process env): a value below the floor or unparseable → None (the
/// auto/model-derived path decides).
pub(crate) fn parse_num_ctx_cap(raw: Option<String>) -> Option<u64> {
    let raw = raw?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<u64>() {
        Ok(v) if v >= OLLAMA_NUM_CTX_FLOOR => Some(v),
        _ => {
            // An operator who SET the override asked for a specific (usually
            // smaller) window; silently falling through to the auto cap can
            // hand them up to 32K — the opposite of what they wanted on a
            // RAM-constrained box. Loud, not silent.
            tracing::warn!(
                "KRONN_OLLAMA_NUM_CTX_CAP=\"{trimmed}\" ignored (not a number ≥ {OLLAMA_NUM_CTX_FLOOR}) — using the model-derived auto cap instead, which may be LARGER than intended"
            );
            None
        }
    }
}

/// Effective ctx cap (0.11.0 — product default, zero configuration):
///   1. `KRONN_OLLAMA_NUM_CTX_CAP` env — explicit operator override, wins.
///   2. The MODEL's own trained context (from `/api/show`), clamped to what
///      this machine's memory can hold — a user who pulled qwen3:32b gets its
///      real window automatically instead of a silent 8K truncation, and is
///      TOLD when the clamp is what decided the number.
///   3. Legacy portable default (8192) when Ollama can't be asked.
pub(crate) fn resolve_ctx_cap(env_raw: Option<String>, model_limit: Option<u64>) -> CtxCap {
    resolve_ctx_cap_within(
        env_raw,
        model_limit,
        ram_derived_ceiling(total_system_memory_bytes()),
    )
}

/// `resolve_ctx_cap`, with the persistent per-model override inserted between
/// the env break-glass and the auto-derived cap — see `CtxCapOrigin` for the
/// full precedence. `overrides` is the whole config map so ONE call resolves
/// one model without the caller needing to pre-look-up anything.
pub(crate) fn resolve_ctx_cap_for_model(
    env_raw: Option<String>,
    model: &str,
    overrides: &std::collections::HashMap<String, u64>,
    model_limit: Option<u64>,
    ceiling: u64,
) -> CtxCap {
    if let Some(value) = parse_num_ctx_cap(env_raw) {
        return CtxCap {
            value,
            origin: CtxCapOrigin::OperatorOverride,
        };
    }
    if let Some(&value) = overrides.get(model) {
        return CtxCap {
            // Config files can be hand-edited or copied from another host.
            // Loading remains backwards-compatible, but execution never trusts
            // a persisted value outside the same safety bounds as the API.
            value: value.clamp(OLLAMA_NUM_CTX_FLOOR, OLLAMA_NUM_CTX_OVERRIDE_MAX),
            origin: CtxCapOrigin::ModelOverride,
        };
    }
    resolve_ctx_cap_within(None, model_limit, ceiling)
}

/// The decision itself, with the machine's ceiling passed in so it is testable
/// without a machine.
pub(crate) fn resolve_ctx_cap_within(
    env_raw: Option<String>,
    model_limit: Option<u64>,
    ceiling: u64,
) -> CtxCap {
    if let Some(value) = parse_num_ctx_cap(env_raw) {
        return CtxCap {
            value,
            origin: CtxCapOrigin::OperatorOverride,
        };
    }
    match model_limit {
        Some(limit) => {
            let value = limit.clamp(OLLAMA_NUM_CTX_FLOOR, ceiling.max(OLLAMA_NUM_CTX_FLOOR));
            CtxCap {
                value,
                origin: if value < limit {
                    CtxCapOrigin::MachineCeiling { model_limit: limit }
                } else {
                    CtxCapOrigin::ModelWindow
                },
            }
        }
        None => CtxCap {
            value: OLLAMA_NUM_CTX_CAP,
            origin: CtxCapOrigin::PortableFallback,
        },
    }
}

/// Extract a model's trained context length from an Ollama `/api/show`
/// response: `model_info` carries an arch-prefixed key (`qwen3.context_length`,
/// `llama.context_length`, …) — match on the suffix. Pure + unit-tested.
pub(crate) fn parse_context_length(show_response: &serde_json::Value) -> Option<u64> {
    show_response
        .get("model_info")?
        .as_object()?
        .iter()
        .find(|(k, _)| k.ends_with(".context_length"))
        .and_then(|(_, v)| v.as_u64())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OllamaModelProfile {
    context_length: Option<u64>,
    storage_format: Option<String>,
}

pub(crate) fn parse_ollama_model_profile(show_response: &serde_json::Value) -> OllamaModelProfile {
    OllamaModelProfile {
        context_length: parse_context_length(show_response),
        storage_format: show_response
            .pointer("/details/format")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    }
}

/// Ask Ollama for the stable metadata Kronn needs while running `model`, with
/// a process-lifetime cache (one `/api/show` per model per boot).
/// `None` means transport failure and is deliberately not cached.
async fn ollama_model_profile(base: &str, model: &str) -> Option<OllamaModelProfile> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, OllamaModelProfile>>,
    > = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    // Key on base+model: switching the Ollama endpoint in config must not
    // serve the previous server's limits for the rest of the process.
    let cache_key = format!("{base}|{model}");
    if let Some(hit) = cache.lock().ok().and_then(|c| c.get(&cache_key).cloned()) {
        return Some(hit);
    }
    // Ok(_) = definitive answer from Ollama (cacheable — the profile is static
    // per model tag). Err = transport failure:
    // NOT cached. /api/show typically fires at the first Ollama step after
    // boot, exactly when Ollama may be busy cold-loading a 32b model — one
    // transient 5s miss must not pin the 8192 fallback (and its silent
    // truncation) for the whole process lifetime.
    // KT-382 — bounded retry. The probe fires at the first Ollama step after
    // boot, which is exactly when Ollama may be cold-loading a 27b model and
    // blowing past 5s. A single miss used to hand the whole call the 8192
    // fallback, and the caller then sent an 11k-token prompt into it. The cost
    // of one extra attempt is a few seconds at worst; the cost of being wrong
    // is an answer written from a silently truncated prompt.
    let mut fetched: Result<OllamaModelProfile, ()> = Err(());
    for attempt in 0..OLLAMA_SHOW_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(OLLAMA_SHOW_RETRY_BACKOFF).await;
        }
        fetched = async {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .map_err(|_| ())?;
            let resp = client
                .post(format!("{}/api/show", base))
                .json(&serde_json::json!({ "model": model }))
                .send()
                .await
                .map_err(|_| ())?
                .error_for_status()
                .map_err(|_| ())?;
            let v: serde_json::Value = resp.json().await.map_err(|_| ())?;
            Ok(parse_ollama_model_profile(&v))
        }
        .await;
        if fetched.is_ok() {
            break;
        }
    }
    match fetched {
        Ok(profile) => {
            if let Ok(mut c) = cache.lock() {
                c.insert(cache_key, profile.clone());
            }
            Some(profile)
        }
        Err(()) => {
            tracing::warn!(
                "Ollama /api/show failed for {model} — portable ctx fallback for this call only (will retry next step)"
            );
            None
        }
    }
}

/// Ask Ollama for `model`'s trained context length. This compatibility wrapper
/// shares the full `/api/show` profile cache with the HTTP worker policy.
pub(crate) async fn ollama_model_ctx_limit(base: &str, model: &str) -> Option<u64> {
    ollama_model_profile(base, model)
        .await
        .and_then(|profile| profile.context_length)
}

/// Size the context window to the prompt, bounded by [FLOOR, cap]. Coarse on
/// purpose (~3 chars/token + output headroom): this is a memory guard, not
/// fine-grained sizing.
pub(crate) fn ollama_num_ctx(system_context: &str, user_prompt: &str, ctx_cap: u64) -> u64 {
    let est = ((system_context.len() + user_prompt.len()) as u64 / 3) + 2048;
    est.clamp(OLLAMA_NUM_CTX_FLOOR, ctx_cap.max(OLLAMA_NUM_CTX_FLOOR))
}

/// Keep the messages within what `ctx_cap` can actually hold. Re-sizing the
/// window covers a tool result that fits the cap; past it the window cannot grow
/// any further, and Ollama would again drop history — the user turn included —
/// to make room. Trimming the biggest results first keeps every call visible and
/// says how much was dropped, so the model knows it is not seeing the whole file.
#[derive(Clone)]
struct CollectionSnapshot {
    original: serde_json::Value,
    shallow: Option<serde_json::Value>,
    identifiers: Option<serde_json::Value>,
    total: usize,
}

fn collection_array(value: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    if let Some(array) = value.as_array() {
        return Some(array);
    }
    let object = value.as_object()?;
    let mut arrays = object.values().filter_map(|v| v.as_array());
    let first = arrays.next()?;
    // Ambiguous when several arrays sit side by side: reporting one of them as
    // "the" count would be worse than saying nothing.
    arrays.next().is_none().then_some(first)
}

fn collection_array_mut(value: &mut serde_json::Value) -> Option<&mut Vec<serde_json::Value>> {
    if value.is_array() {
        return value.as_array_mut();
    }
    let key = {
        let object = value.as_object()?;
        let keys = object
            .iter()
            .filter(|(_, value)| value.is_array())
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        (keys.len() == 1).then(|| keys[0].clone())?
    };
    value.as_object_mut()?.get_mut(&key)?.as_array_mut()
}

fn shortened_string(value: &str) -> Option<String> {
    const MAX_CHARS: usize = 256;
    let mut chars = value.chars();
    let shortened = chars.by_ref().take(MAX_CHARS).collect::<String>();
    chars.next().map(|_| format!("{shortened}…"))
}

fn is_identifier_field(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key == "id"
        || key.ends_with("_id")
        || matches!(
            key.as_str(),
            "name"
                | "full_name"
                | "title"
                | "slug"
                | "key"
                | "uuid"
                | "url"
                | "html_url"
                | "language"
                | "version"
                | "active_version"
                | "status"
                | "state"
                | "type"
                | "created_at"
                | "updated_at"
        )
}

fn compact_collection_item(
    item: &serde_json::Value,
    identifiers_only: bool,
) -> (serde_json::Value, bool) {
    let Some(object) = item.as_object() else {
        if let Some(value) = item.as_str().and_then(shortened_string) {
            return (serde_json::Value::String(value), true);
        }
        return (item.clone(), false);
    };

    let mut compact = serde_json::Map::new();
    for (key, value) in object {
        if identifiers_only && !is_identifier_field(key) {
            continue;
        }
        match value {
            serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
                compact.insert(key.clone(), value.clone());
            }
            serde_json::Value::String(value) => {
                compact.insert(
                    key.clone(),
                    shortened_string(value)
                        .map(serde_json::Value::String)
                        .unwrap_or_else(|| serde_json::Value::String(value.clone())),
                );
            }
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {}
        }
    }

    // An object without scalar identifying data cannot be compacted honestly.
    if compact.is_empty() {
        return (item.clone(), false);
    }
    let compact = serde_json::Value::Object(compact);
    let changed = compact != *item;
    (compact, changed)
}

fn compact_collection(
    original: &serde_json::Value,
    identifiers_only: bool,
) -> Option<(serde_json::Value, bool)> {
    collection_array(original)?;
    let mut compact = original.clone();
    let mut changed = false;
    for item in collection_array_mut(&mut compact)? {
        let (replacement, item_changed) = compact_collection_item(item, identifiers_only);
        *item = replacement;
        changed |= item_changed;
    }
    Some((compact, changed))
}

fn collection_snapshot(raw: &str) -> Option<CollectionSnapshot> {
    let original = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    let total = collection_array(&original)?.len();
    let (shallow, shallow_changed) = compact_collection(&original, false)?;
    let (identifiers, identifiers_changed) = compact_collection(&original, true)?;
    Some(CollectionSnapshot {
        original,
        shallow: shallow_changed.then_some(shallow),
        identifiers: identifiers_changed.then_some(identifiers),
        total,
    })
}

fn complete_collection_note(total: usize) -> String {
    format!(
        "\n\n[compacted by Kronn: all {total} collection items are still present; nested fields \
         and long values were removed or shortened to fit the context window. Narrow the query \
         or use an `extract` if you need the omitted detail.]"
    )
}

fn incomplete_collection_note(kept: usize, total: usize) -> String {
    format!(
        "\n\n[truncated by Kronn: this list is INCOMPLETE — {kept} of {total} items kept to \
         fit the context window. Do NOT report the count as final; narrow the query (a filter, \
         a smaller page size, or an `extract` of the fields you need) and ask again.]"
    )
}

fn collection_replacement(
    snapshot: &CollectionSnapshot,
    max_encoded_len: usize,
    current_len: usize,
) -> Option<String> {
    for candidate in [&snapshot.shallow, &snapshot.identifiers]
        .into_iter()
        .flatten()
    {
        let serialized = serde_json::to_string(candidate).ok()?;
        let replacement = format!("{serialized}{}", complete_collection_note(snapshot.total));
        let encoded_len = serde_json::to_string(&replacement).ok()?.len();
        if encoded_len <= max_encoded_len && replacement.len() < current_len {
            return Some(replacement);
        }
    }

    // If every item cannot survive even after projection, keep a structurally
    // valid prefix and state its exact size. Prefer the identifier projection,
    // then the shallow one, then the untouched payload.
    let base = snapshot
        .identifiers
        .as_ref()
        .or(snapshot.shallow.as_ref())
        .unwrap_or(&snapshot.original);
    let render = |kept: usize| -> Option<String> {
        let mut candidate = base.clone();
        collection_array_mut(&mut candidate)?.truncate(kept);
        let serialized = serde_json::to_string(&candidate).ok()?;
        Some(format!(
            "{serialized}{}",
            incomplete_collection_note(kept, snapshot.total)
        ))
    };

    let mut low = 0;
    let mut high = snapshot.total;
    while low < high {
        let mid = (low + high).div_ceil(2);
        if render(mid).is_some_and(|value| {
            serde_json::to_string(&value).is_ok_and(|encoded| encoded.len() <= max_encoded_len)
        }) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let replacement = render(low)?;
    (low < snapshot.total && replacement.len() < current_len).then_some(replacement)
}

fn is_protected_checkpoint_tool_result(message: &serde_json::Value) -> bool {
    message["content"]
        .as_str()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(content).ok())
        .is_some_and(|content| content["kronn_checkpoint_compacted"] == true)
}

pub(crate) fn clamp_ollama_tool_results(body: &mut serde_json::Value, ctx_cap: u64) {
    // Trim against the window this request will ACTUALLY get, not the ceiling it
    // could have had. Ollama fixes `n_ctx_slot` when it loads the model and does
    // not grow it mid-conversation, so a later `num_ctx` bump buys nothing —
    // trimming to the cap while the slot sits at 4 864 leaves the prompt just as
    // oversized, and the history gets truncated until the user turn is gone.
    let ctx_cap = body["options"]["num_ctx"]
        .as_u64()
        .unwrap_or(ctx_cap)
        .min(ctx_cap);
    // Bytes per token, for turning the window back into a byte budget. NOT the
    // 3.0 the forward estimate assumes: that ratio holds for prose, while a tool
    // loop carries dense JSON — API payloads, file listings — which tokenises far
    // heavier. Measured on a real run, 10 KB of such a prompt came back as 4 592
    // tokens, i.e. ~2.2 bytes each. Budgeting at 3.0 let a prompt through that
    // was a third larger than the window, and Ollama then truncated the history
    // until the user turn was gone. 2 is deliberately pessimistic: over-trimming
    // costs the model some context, under-trimming costs the whole run.
    const BYTES_PER_TOKEN: usize = 2;
    // Headroom for the reply itself — the window has to hold both.
    const REPLY_HEADROOM_TOKENS: u64 = 2048;
    const MIN_KEPT: usize = 512;
    let budget =
        (ctx_cap.saturating_sub(REPLY_HEADROOM_TOKENS) as usize).saturating_mul(BYTES_PER_TOKEN);

    // Keep the untouched collection once. Every trimming pass can then rebuild
    // valid JSON from it instead of reparsing a previous diagnostic suffix.
    let collections: std::collections::HashMap<usize, CollectionSnapshot> = body["messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .enumerate()
                .filter(|(_, m)| m["role"] == "tool")
                .filter_map(|(i, m)| {
                    let raw = m["content"].as_str()?;
                    Some((i, collection_snapshot(raw)?))
                })
                .collect()
        })
        .unwrap_or_default();

    loop {
        let over = body["messages"].to_string().len().saturating_sub(budget);
        if over == 0 {
            return;
        }
        let Some(messages) = body["messages"].as_array_mut() else {
            return;
        };
        // Biggest tool result first: one huge file should be cut before several
        // small results that together cost less.
        let target = messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m["role"] == "tool")
            // These envelopes are already bounded, valid JSON and carry the
            // only retained CAS receipt. Blind truncation would corrupt the
            // very protocol state the checkpoint was designed to preserve.
            .filter(|(_, m)| !is_protected_checkpoint_tool_result(m))
            .map(|(i, m)| (i, m["content"].as_str().map_or(0, str::len)))
            .filter(|(_, len)| *len > MIN_KEPT)
            .max_by_key(|(_, len)| *len);
        let Some((idx, len)) = target else {
            return; // nothing left to trim; the re-size + diagnostic take over
        };
        // Owned: the write-back below borrows `messages` mutably.
        let content = messages[idx]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if let Some(snapshot) = collections.get(&idx) {
            // The tool content is itself a JSON string inside `messages`, so its
            // quotes and escapes cost more than its raw byte length. Derive the
            // exact encoded allowance; subtracting `over` from raw `len` can
            // otherwise decide that zero items fit when a compact list does.
            let body_len = budget + over;
            let encoded_content_len = serde_json::to_string(&content).map_or(len, |v| v.len());
            let other_messages_len = body_len.saturating_sub(encoded_content_len);
            let max_encoded_len = budget.saturating_sub(other_messages_len);
            if let Some(replacement) = collection_replacement(snapshot, max_encoded_len, len) {
                messages[idx]["content"] = serde_json::json!(replacement);
                continue;
            }
        }
        let note = |dropped: usize| {
            format!(
                "\n\n[truncated by Kronn: {dropped} bytes dropped so this result fits the \
                 context window. Ask for a narrower range if you need the rest.]"
            )
        };
        // The note itself costs bytes, so it has to come out of the budget too —
        // otherwise a small overflow trims a few bytes and adds more than it cut.
        let keep = len
            .saturating_sub(over + note(len).len())
            .max(MIN_KEPT)
            .min(len);
        let cut = content
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|i| *i <= keep)
            .last()
            .unwrap_or(0);
        let replacement = format!("{}{}", &content[..cut], note(len - cut));
        // Strict progress or stop: guarantees this loop terminates even when the
        // note is longer than what trimming saves.
        if replacement.len() >= len {
            return;
        }
        messages[idx]["content"] = serde_json::json!(replacement);
    }
}

/// Re-size `num_ctx` from the messages actually about to be sent. The first-turn
/// estimate covers only the system context and the user prompt, but the tool loop
/// appends results that are often far bigger (a file, a diff). Leaving the window
/// at its initial size makes Ollama truncate the history until the user message
/// itself is gone, which it then rejects with `no user query found in messages`.
pub(crate) fn resize_ollama_num_ctx(body: &mut serde_json::Value, ctx_cap: u64) {
    let est = estimated_chat_history_tokens(body);
    let sized = est.clamp(OLLAMA_NUM_CTX_FLOOR, ctx_cap.max(OLLAMA_NUM_CTX_FLOOR));
    if body["options"]["num_ctx"]
        .as_u64()
        .is_some_and(|cur| cur >= sized)
    {
        return;
    }
    body["options"]["num_ctx"] = serde_json::json!(sized);
}

/// qwen3 models are hybrid-reasoning; a step pays for every thinking token and
/// only `message.content` is ever read, so reasoning is switched off two ways:
/// `think:false` in the body (the effective one, honored since Ollama 0.19) and
/// the `/no_think` control token, kept for older runtimes that only had that.
pub(crate) fn ollama_disables_thinking(model: &str) -> bool {
    model.starts_with("qwen3")
}

/// Optional `keep_alive` for the Ollama request — how long the model stays
/// resident after the call. Ollama's own default is 5 min; set
/// `KRONN_OLLAMA_KEEP_ALIVE` to keep a model warm across a workflow's steps and
/// avoid paying the cold-reload latency each step. Accepts a duration string
/// (`"30m"`, `"1h"`) or seconds (`"1800"`, `"-1"` = forever, `"0"` = unload
/// now). Unset/blank ⇒ omit the field ⇒ Ollama uses its own default.
pub(crate) fn ollama_keep_alive() -> Option<serde_json::Value> {
    parse_keep_alive(std::env::var("KRONN_OLLAMA_KEEP_ALIVE").ok())
}

/// Pure parse of the keep_alive override (split out for unit tests without
/// mutating process env): blank/unset ⇒ None (omit); a bare integer ⇒ a number
/// (seconds, per Ollama); anything else ⇒ the raw duration string.
pub(crate) fn parse_keep_alive(raw: Option<String>) -> Option<serde_json::Value> {
    let raw = raw?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    Some(match raw.parse::<i64>() {
        Ok(secs) => serde_json::json!(secs),
        Err(_) => serde_json::json!(raw),
    })
}

/// Build the Ollama `/api/chat` request body. Pure + side-effect-free so it
/// can be unit-tested on the constructed request — we deliberately never
/// assert on generated text: on Metal, `temperature=0` + `seed` yields
/// *greedy-stable* output, NOT bit-exact reproducibility (float reduction
/// order isn't guaranteed; two logits within epsilon can flip the argmax,
/// more so under Q4 quant). Never build logic (output hash-caching, strict
/// text-equality) that presumes exact reproducibility. Ordered pillars:
/// fixed num_ctx > temp=0/top_k=1 > same model+quant > seed (near-inert
/// under greedy, kept only for a possible future temp>0).
///
/// `format` = an optional JSON Schema (from a workflow step's `TypedSchema`).
/// When present, Ollama constrains decoding to the schema (structurally-valid
/// JSON guaranteed) and we switch to a non-streaming request: a schema step
/// wants one validated blob, not progressive chunks.
pub(crate) fn build_ollama_chat_body(
    model: &str,
    system_context: &str,
    user_prompt: &str,
    format: Option<&serde_json::Value>,
    ctx_cap: u64,
) -> serde_json::Value {
    let mut messages = Vec::new();
    if ollama_disables_thinking(model) {
        messages.push(serde_json::json!({ "role": "system", "content": "/no_think" }));
    }
    if !system_context.is_empty() {
        messages.push(serde_json::json!({ "role": "system", "content": system_context }));
    }
    messages.push(serde_json::json!({ "role": "user", "content": user_prompt }));

    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
        // format present ⇒ one validated JSON blob (non-stream); else stream text.
        "stream": format.is_none(),
        "options": {
            "temperature": 0,
            "top_k": 1,
            "seed": 42,
            "num_ctx": ollama_num_ctx(system_context, user_prompt, ctx_cap),
        },
    });
    if let Some(fmt) = format {
        body["format"] = fmt.clone();
    }
    // Only ever sent to turn reasoning OFF; omitted otherwise so a model we
    // make no claim about keeps its own default.
    if ollama_disables_thinking(model) {
        body["think"] = serde_json::Value::Bool(false);
    }
    // Keep the model warm across steps when the operator opted in (env).
    if let Some(ka) = ollama_keep_alive() {
        body["keep_alive"] = ka;
    }
    body
}

/// Token counts seen so far. OpenAI emits them in a usage frame that arrives
/// *before* the `[DONE]` sentinel, so they must survive across lines.
#[derive(Default)]
pub(crate) struct TokenTally {
    prompt: u64,
    eval: u64,
}

const HTTP_TURN_TRACE_PREFIX: &str = "kronn_http_turn:";
const HTTP_TOOL_EXEC_TRACE_PREFIX: &str = "kronn_http_tool_exec:";
const MAX_HTTP_TELEMETRY_TURNS_PER_DISPATCH: usize = 64;
const MAX_HTTP_TELEMETRY_TOOLS_PER_TURN: usize = 8;
const MAX_HTTP_TELEMETRY_TOOL_NAME_BYTES: usize = 64;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct HttpTurnTrace {
    version: u8,
    turn: u32,
    provider: String,
    phase: crate::models::TaskExecutionHttpPhase,
    prompt_tokens: u64,
    eval_tokens: u64,
    duration_ms: u64,
    provider_ok: bool,
    requested_tools: Vec<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct HttpToolExecTrace {
    version: u8,
    turn: u32,
    name: String,
    ok: bool,
}

fn bounded_http_tool_name(name: &str) -> String {
    if !name.is_empty()
        && name.len() <= MAX_HTTP_TELEMETRY_TOOL_NAME_BYTES
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        name.to_string()
    } else {
        "invalid_tool_name".to_string()
    }
}

fn push_http_turn_trace(stderr: &Arc<Mutex<Vec<String>>>, trace: HttpTurnTrace) {
    if let (Ok(encoded), Ok(mut capture)) = (serde_json::to_string(&trace), stderr.lock()) {
        capture.push(format!("{HTTP_TURN_TRACE_PREFIX}{encoded}"));
    }
}

fn push_http_tool_exec_trace(stderr: &Arc<Mutex<Vec<String>>>, turn: u32, name: &str, ok: bool) {
    let trace = HttpToolExecTrace {
        version: 1,
        turn,
        name: bounded_http_tool_name(name),
        ok,
    };
    if let (Ok(encoded), Ok(mut capture)) = (serde_json::to_string(&trace), stderr.lock()) {
        capture.push(format!("{HTTP_TOOL_EXEC_TRACE_PREFIX}{encoded}"));
    }
}

/// Decode the internal, payload-free HTTP trace into the durable task usage
/// projection. Malformed or future-version lines are ignored independently so
/// one damaged diagnostic can never erase the rest of a run's accounting.
pub(crate) fn parse_http_turn_telemetry(
    stderr_lines: &[String],
) -> Vec<crate::models::TaskExecutionHttpTurnUsage> {
    let mut turns =
        std::collections::BTreeMap::<u32, crate::models::TaskExecutionHttpTurnUsage>::new();
    for line in stderr_lines {
        if let Some(raw) = line.strip_prefix(HTTP_TURN_TRACE_PREFIX) {
            let Ok(trace) = serde_json::from_str::<HttpTurnTrace>(raw) else {
                continue;
            };
            if trace.version != 1 {
                continue;
            }
            turns.insert(
                trace.turn,
                crate::models::TaskExecutionHttpTurnUsage {
                    turn: trace.turn,
                    dispatch_id: None,
                    provider: trace.provider,
                    phase: trace.phase,
                    prompt_tokens: trace.prompt_tokens,
                    eval_tokens: trace.eval_tokens,
                    duration_ms: trace.duration_ms,
                    provider_ok: trace.provider_ok,
                    requested_tools: trace
                        .requested_tools
                        .into_iter()
                        .take(MAX_HTTP_TELEMETRY_TOOLS_PER_TURN)
                        .map(|name| bounded_http_tool_name(&name))
                        .collect(),
                    executed_tools: Vec::new(),
                },
            );
            continue;
        }
        let Some(raw) = line.strip_prefix(HTTP_TOOL_EXEC_TRACE_PREFIX) else {
            continue;
        };
        let Ok(trace) = serde_json::from_str::<HttpToolExecTrace>(raw) else {
            continue;
        };
        if trace.version != 1 {
            continue;
        }
        if let Some(turn) = turns.get_mut(&trace.turn) {
            if turn.executed_tools.len() < MAX_HTTP_TELEMETRY_TOOLS_PER_TURN {
                turn.executed_tools
                    .push(crate::models::TaskExecutionHttpToolUsage {
                        name: bounded_http_tool_name(&trace.name),
                        ok: trace.ok,
                    });
            }
        }
    }
    let mut turns = turns.into_values().collect::<Vec<_>>();
    if turns.len() > MAX_HTTP_TELEMETRY_TURNS_PER_DISPATCH {
        turns.drain(..turns.len() - MAX_HTTP_TELEMETRY_TURNS_PER_DISPATCH);
    }
    turns
}

#[derive(Debug, Default, PartialEq, Eq)]
enum LeadingThinkingState {
    #[default]
    Probing,
    Suppressing,
    Passthrough,
}

/// Suppress a reasoning block only when it prefixes an HTTP model response.
///
/// Some OpenAI-compatible proxies return DeepSeek-style `<think>...</think>`
/// scratchpads in `content` instead of a dedicated reasoning field. Deltas can
/// split either tag at any byte boundary, so a per-chunk regex cannot prevent
/// the leak. Once real answer text starts, the filter becomes a passthrough so
/// legitimate code samples mentioning these tags remain visible.
#[derive(Debug, Default)]
pub(crate) struct LeadingThinkingFilter {
    state: LeadingThinkingState,
    pending: String,
}

impl LeadingThinkingFilter {
    const OPEN_TAGS: [&'static str; 2] = ["<think>", "<thinking>"];
    const CLOSE_TAGS: [&'static str; 2] = ["</think>", "</thinking>"];

    pub(crate) fn push(&mut self, input: &str) -> String {
        if self.state == LeadingThinkingState::Passthrough {
            return input.to_string();
        }

        self.pending.push_str(input);
        let mut visible = String::new();

        loop {
            match self.state {
                LeadingThinkingState::Passthrough => {
                    visible.push_str(&std::mem::take(&mut self.pending));
                    break;
                }
                LeadingThinkingState::Probing => {
                    let Some(first_content) = self
                        .pending
                        .char_indices()
                        .find_map(|(index, ch)| (!ch.is_whitespace()).then_some(index))
                    else {
                        break;
                    };
                    let candidate = self.pending[first_content..].to_ascii_lowercase();
                    if let Some(tag) = Self::OPEN_TAGS
                        .iter()
                        .find(|tag| candidate.starts_with(**tag))
                    {
                        self.pending.drain(..first_content + tag.len());
                        self.state = LeadingThinkingState::Suppressing;
                        continue;
                    }
                    if Self::OPEN_TAGS
                        .iter()
                        .any(|tag| tag.starts_with(&candidate))
                    {
                        break;
                    }
                    self.state = LeadingThinkingState::Passthrough;
                }
                LeadingThinkingState::Suppressing => {
                    let lower = self.pending.to_ascii_lowercase();
                    let closing = Self::CLOSE_TAGS
                        .iter()
                        .filter_map(|tag| lower.find(tag).map(|index| (index, tag.len())))
                        .min_by_key(|(index, _)| *index);
                    if let Some((index, tag_len)) = closing {
                        self.pending.drain(..index + tag_len);
                        self.state = LeadingThinkingState::Probing;
                        continue;
                    }

                    let keep = Self::longest_possible_tag_suffix(&lower, &Self::CLOSE_TAGS);
                    self.pending.drain(..self.pending.len() - keep);
                    break;
                }
            }
        }

        visible
    }

    pub(crate) fn finish(&mut self) -> String {
        match self.state {
            LeadingThinkingState::Suppressing => {
                self.pending.clear();
                String::new()
            }
            LeadingThinkingState::Probing | LeadingThinkingState::Passthrough => {
                std::mem::take(&mut self.pending)
            }
        }
    }

    fn longest_possible_tag_suffix(input: &str, tags: &[&str]) -> usize {
        tags.iter()
            .flat_map(|tag| (1..tag.len()).map(move |length| &tag[..length]))
            .filter(|prefix| input.ends_with(prefix))
            .map(str::len)
            .max()
            .unwrap_or(0)
    }
}

pub(crate) fn strip_leading_thinking_blocks(input: &str) -> String {
    let mut filter = LeadingThinkingFilter::default();
    let mut visible = filter.push(input);
    visible.push_str(&filter.finish());
    visible
}

/// Apply one decoded stream line: forward the text delta, record errors, and
/// on the terminal chunk stash token counts for `parse_token_usage`. The
/// stderr lock is only held across synchronous work — never across
/// `tx.send().await`.
///
/// Returns `false` once the consumer has dropped the receiver (user cancelled
/// / stream aborted) so the caller can stop draining the response into the
/// void.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn forward_chat_line(
    codec: &dyn crate::agents::chat_codec::ChatCodec,
    // Which HTTP backend produced this line. Passed in because an in-band error
    // used to be labelled "Ollama error" whatever the provider: a room testing
    // @nvidia showed "Ollama error: ResourceExhausted", which sends the reader
    // hunting through the wrong provider's settings.
    backend: &str,
    line: &str,
    tx: &tokio::sync::mpsc::Sender<String>,
    stderr: &Arc<Mutex<Vec<String>>>,
    got_done: &mut bool,
    got_error: &mut bool,
    // Exact provider error for retry classification. `got_error` remains the
    // cheap terminal flag; this preserves the signal that says whether the
    // failure was transient (for example NVIDIA worker saturation).
    provider_error: &mut Option<String>,
    num_ctx: u64,
    tally: &mut TokenTally,
    thinking_filter: &mut LeadingThinkingFilter,
    // Tool-call fragments seen so far this turn. Merged here rather than in
    // the codec because one call can span several frames.
    pending_tools: &mut crate::agents::tools::ToolCallAccumulator,
    // Set as soon as any visible text is forwarded, so the loop can tell a silent
    // finish from a real answer.
    emitted_any: &mut bool,
) -> bool {
    let Some(chunk) = codec.parse_line(line) else {
        return true;
    };
    if !chunk.tool_calls.is_empty() {
        pending_tools.push(chunk.tool_calls);
    }
    // In-band error on a 200 stream (model crashed mid-generation).
    // Swallowing it made the step SUCCEED with empty or truncated output —
    // surface it so the step fails with the reason.
    // A stop reason other than the normal ones is the single most useful fact about
    // a run that produced nothing: `length` says the model exhausted its output
    // budget (a reasoning model can spend all of it thinking), which no amount of
    // retrying fixes. Recorded here so the failure notice can state it instead of
    // listing candidate causes.
    if let Some(reason) = chunk.finish_reason.as_deref() {
        if !matches!(reason, "stop" | "tool_calls") {
            if let Ok(mut stderr) = stderr.lock() {
                stderr.push(format!("{backend} stopped with finish_reason: {reason}"));
            }
        }
    }
    if let Some(err) = &chunk.error {
        tracing::warn!("{backend} in-band error: {err}");
        if let Ok(mut stderr) = stderr.lock() {
            stderr.push(format!("{backend} error: {err}"));
        }
        *provider_error = Some(err.clone());
        *got_error = true;
    }
    if let Some(text) = chunk.delta {
        let visible = thinking_filter.push(&text);
        if !visible.is_empty() {
            *emitted_any = true;
        }
        if !visible.is_empty() && tx.send(visible).await.is_err() {
            return false;
        }
    }
    if chunk.prompt_tokens > 0 {
        tally.prompt = chunk.prompt_tokens;
    }
    if chunk.eval_tokens > 0 {
        tally.eval = chunk.eval_tokens;
    }
    if chunk.done {
        *got_done = true;
        if let Ok(mut stderr) = stderr.lock() {
            stderr.push(format!("ollama_tokens:{}:{}", tally.prompt, tally.eval));
        }
        // A prompt that FILLED the window was almost certainly cut by
        // Ollama (it silently drops the overflow) — exact signal, unlike
        // the pre-flight estimate.
        if num_ctx > 0 && tally.prompt >= num_ctx.saturating_sub(64) {
            tracing::warn!(
                target: "kronn::ollama",
                prompt_tokens = tally.prompt, num_ctx,
                "prompt filled the context window — input was silently TRUNCATED by Ollama; \
                 reduce the step's input or raise KRONN_OLLAMA_NUM_CTX_CAP"
            );
            if let Ok(mut stderr) = stderr.lock() {
                stderr.push(format!(
                    "Ollama truncation: prompt_eval_count {} filled num_ctx {num_ctx}",
                    tally.prompt
                ));
            }
        }
    }
    true
}

// One provider request may be replayed only while it is still a pure model
// invocation. Once a tool has executed, the caller forces the budget to one:
// even a read-looking tool can hide a remote side effect, so fail closed until
// the tool contract carries an explicit idempotency guarantee.
const HTTP_PROVIDER_MAX_ATTEMPTS: usize = 3;

#[derive(Debug)]
struct HttpProviderFailure {
    status: Option<reqwest::StatusCode>,
    detail: String,
    attempts: usize,
}

fn is_permanent_provider_failure(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    [
        "invalid api key",
        "incorrect api key",
        "authentication failed",
        "unauthorized",
        "model_not_found",
        "model not found",
        "unknown model",
        "insufficient_quota",
        "quota exceeded",
        "quota exhausted",
        "out of credits",
        "credit balance",
        "billing limit",
    ]
    .iter()
    .any(|needle| detail.contains(needle))
}

fn is_transient_provider_failure(status: Option<reqwest::StatusCode>, detail: &str) -> bool {
    if is_permanent_provider_failure(detail) {
        return false;
    }
    if status.is_some_and(|status| {
        status.is_server_error()
            || matches!(
                status,
                reqwest::StatusCode::REQUEST_TIMEOUT
                    | reqwest::StatusCode::TOO_EARLY
                    | reqwest::StatusCode::TOO_MANY_REQUESTS
            )
    }) {
        return true;
    }

    let detail = detail.to_ascii_lowercase();
    [
        // Exact NVIDIA NIM capacity signal observed in production. Bare
        // `ResourceExhausted` is deliberately absent: providers also use it
        // for permanent account quota exhaustion.
        "worker local total request limit reached",
        "server is overloaded",
        "temporarily unavailable",
        "service unavailable",
        "gateway timeout",
        "upstream timeout",
        "try again later",
        "no capacity available",
        "transport error:",
    ]
    .iter()
    .any(|needle| detail.contains(needle))
}

fn provider_retry_delay(failed_attempt: usize) -> std::time::Duration {
    #[cfg(test)]
    {
        let _ = failed_attempt;
        std::time::Duration::from_millis(1)
    }
    #[cfg(not(test))]
    {
        // Long enough for a saturated worker slot to clear, still bounded so
        // the user is never left behind an invisible minute-long retry loop.
        std::time::Duration::from_secs(if failed_attempt == 1 { 2 } else { 5 })
    }
}

fn push_provider_retry_trace(stderr: &Arc<Mutex<Vec<String>>>, line: String) {
    tracing::info!(target: "kronn::agent::provider_retry", "{line}");
    if let Ok(mut stderr) = stderr.lock() {
        stderr.push(format!("[provider-retry: {line}]"));
    }
}

fn provider_failure_label(status: Option<reqwest::StatusCode>, detail: &str) -> String {
    if detail
        .to_ascii_lowercase()
        .contains("worker local total request limit reached")
    {
        "worker saturation".to_string()
    } else if let Some(status) = status {
        format!("HTTP {status}")
    } else {
        "transport error".to_string()
    }
}

#[allow(clippy::too_many_arguments)]
async fn send_http_agent_request(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
    auth_key: Option<&str>,
    backend: &str,
    first_attempt: usize,
    max_attempts: usize,
    retry_allowed: bool,
    stderr: &Arc<Mutex<Vec<String>>>,
) -> Result<(reqwest::Response, usize), HttpProviderFailure> {
    let mut attempt = first_attempt;
    loop {
        let mut request = client.post(url).json(body);
        if let Some(key) = auth_key {
            request = request.bearer_auth(key);
        }
        match request.send().await {
            Ok(response) if response.status().is_success() => return Ok((response, attempt)),
            Ok(response) => {
                let status = response.status();
                let detail = response.text().await.unwrap_or_default();
                if retry_allowed
                    && attempt < max_attempts
                    && is_transient_provider_failure(Some(status), &detail)
                {
                    let delay = provider_retry_delay(attempt);
                    push_provider_retry_trace(
                        stderr,
                        format!(
                            "{backend} attempt {attempt}/{max_attempts} failed ({}); retrying attempt {}/{} in {} ms",
                            provider_failure_label(Some(status), &detail),
                            attempt + 1,
                            max_attempts,
                            delay.as_millis()
                        ),
                    );
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                    continue;
                }
                return Err(HttpProviderFailure {
                    status: Some(status),
                    detail,
                    attempts: attempt,
                });
            }
            Err(error) => {
                let detail = error.to_string();
                if retry_allowed && attempt < max_attempts {
                    let delay = provider_retry_delay(attempt);
                    push_provider_retry_trace(
                        stderr,
                        format!(
                            "{backend} attempt {attempt}/{max_attempts} failed (transport error); retrying attempt {}/{} in {} ms",
                            attempt + 1,
                            max_attempts,
                            delay.as_millis()
                        ),
                    );
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                    continue;
                }
                return Err(HttpProviderFailure {
                    status: None,
                    detail,
                    attempts: attempt,
                });
            }
        }
    }
}

fn format_provider_failure(
    backend: &str,
    base: &str,
    failure: &HttpProviderFailure,
    suffix: &str,
) -> String {
    let attempts = if failure.attempts > 1 {
        format!(" after {} attempts", failure.attempts)
    } else {
        String::new()
    };
    match failure.status {
        Some(status) => format!(
            "{backend} error {status}{attempts}:{suffix} Provider response: {}",
            failure.detail
        ),
        None => format!(
            "{backend} unreachable at {base}{attempts}: {}",
            failure.detail
        ),
    }
}

/// Start Ollama via HTTP API (/api/chat) instead of a CLI process.
/// Returns an AgentProcess with a dummy child process and an rx fed by
/// the HTTP response. System context and user prompt are sent as separate
/// messages (role: system, role: user) so the model doesn't confuse MCP
/// instructions with the user's question.
///
/// `format` = an optional JSON Schema (a `TypedSchema` step's schema, already
/// wrapped in the canonical envelope shape by the caller). When set, decoding
/// is grammar-constrained and the request is non-streaming (one JSON object).
#[allow(clippy::too_many_arguments)]
async fn start_ollama_http(
    agent_type: &AgentType,
    user_prompt: &str,
    system_context: &str,
    model: &str,
    format: Option<&serde_json::Value>,
    http_base_url: Option<&str>,
    http_api_key: Option<&str>,
    executor: Option<std::sync::Arc<dyn crate::agents::tools::ToolExecutor>>,
    ollama_context_overrides: Option<&std::collections::HashMap<String, u64>>,
    http_request_timeout: Option<std::time::Duration>,
    parent_cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<AgentProcess, String> {
    use crate::agents::chat_codec::{ChatCodec, OllamaCodec, OpenAiCodec};
    let identity_context = http_agent_identity_context(agent_type, model);
    let system_context = if system_context.trim().is_empty() {
        identity_context
    } else {
        format!("{identity_context}\n\n{system_context}")
    };
    // Endpoint, request body and line decoding are the only per-backend parts;
    // everything below this block is shared transport.
    let codec: Box<dyn ChatCodec> = if is_openai_wire_agent(agent_type) {
        Box::new(OpenAiCodec)
    } else {
        Box::new(OllamaCodec)
    };
    // A throttled local model must say so where the run is read, not only in a
    // log file nobody has open. Collected here so the Ollama arm can set it.
    let mut ctx_notice: Option<String> = None;
    let (base, body, ctx_cap, model_trained_context, model_storage_format, ctx_cap_origin) =
        if is_openai_wire_agent(agent_type) {
            let base = if *agent_type == AgentType::Nvidia {
                crate::api::nvidia::resolve_base_url_pub(http_base_url)
            } else {
                crate::api::lite_llm::resolve_base_url_pub(http_base_url)
            };
            // No `num_ctx` equivalent: the window belongs to whatever upstream the
            // proxy fronts, so there is nothing here to cap or to warn about.
            let body = crate::agents::chat_codec::build_openai_chat_body(
                model,
                &system_context,
                user_prompt,
                format,
                format.is_none(),
            );
            tracing::info!(
                target: "kronn::lite_llm",
                model = %model,
                constrained_format = format.is_some(),
                stream = body["stream"].as_bool().unwrap_or(true),
                "litellm run starting"
            );
            (base, body, 0, None, None, None)
        } else {
            let base = crate::api::ollama::ollama_base_url_pub();
            // 0.11.0 — ctx cap auto-derived from THE MODEL (its trained context via
            // /api/show, cached), clamped to a RAM-safe ceiling. Zero configuration:
            // a user who pulled qwen3:32b gets its real window instead of a silent 8K
            // truncation. Env override still wins for experts.
            let model_profile = ollama_model_profile(&base, model).await;
            let model_limit = model_profile
                .as_ref()
                .and_then(|profile| profile.context_length);
            let model_storage_format = model_profile
                .as_ref()
                .and_then(|profile| profile.storage_format.clone());
            let cap = match ollama_context_overrides {
                Some(overrides) => resolve_ctx_cap_for_model(
                    std::env::var("KRONN_OLLAMA_NUM_CTX_CAP").ok(),
                    model,
                    overrides,
                    model_limit,
                    ram_derived_ceiling(total_system_memory_bytes()),
                ),
                None => {
                    resolve_ctx_cap(std::env::var("KRONN_OLLAMA_NUM_CTX_CAP").ok(), model_limit)
                }
            };
            let ctx_cap = cap.value;
            ctx_notice = cap.throttle_notice(model);
            if let Some(notice) = ctx_notice.as_deref() {
                tracing::warn!(target: "kronn::ollama", model = %model, ctx_cap, "{notice}");
            }
            let est = ((system_context.len() + user_prompt.len()) as u64 / 3) + 2048;
            if est > ctx_cap {
                // KT-382 — refuse, do not announce and send anyway. Ollama does not
                // reject an oversized prompt: it silently drops the head of the
                // conversation to make room, and answers confidently from whatever
                // survived. That is strictly worse than an error — the run looks
                // successful, the reply is grounded in a prompt nobody chose, and
                // whoever reads it has no way to know. A refusal costs one message.
                //
                // Naming where the cap came from matters: the remedy is different
                // for each, and an operator staring at "8192" cannot tell whether
                // they picked that number or Ollama failed to answer.
                // Naming the ceiling as "this model's window" when the model in
                // fact offers eight times more sends the operator to change models
                // instead of raising a ceiling. The origin now knows the difference.
                let (origin, remedy) = match cap.origin {
                    CtxCapOrigin::OperatorOverride => (
                        "your KRONN_OLLAMA_NUM_CTX_CAP setting".to_string(),
                        "raise it if the machine has the RAM, or shorten the step's input",
                    ),
                    CtxCapOrigin::ModelOverride => (
                        "the persistent per-model context override set for this model".to_string(),
                        "raise the override for this model if the machine has the RAM, or \
                     shorten the step's input",
                    ),
                    CtxCapOrigin::ModelWindow => (
                        "this model's own trained context window".to_string(),
                        "shorten the step's input, or use a model with a larger window",
                    ),
                    CtxCapOrigin::MachineCeiling { model_limit } => (
                        format!(
                            "the ceiling this machine's memory allows — the model itself \
                         supports {model_limit}"
                        ),
                        "raise KRONN_OLLAMA_NUM_CTX_CAP if the RAM is there, or shorten the \
                     step's input",
                    ),
                    CtxCapOrigin::PortableFallback => (
                        "the portable fallback, because Ollama did not answer /api/show"
                            .to_string(),
                        "check that Ollama is reachable and the model is pulled, then retry — \
                     the real window is almost certainly larger than this fallback",
                    ),
                };
                tracing::warn!(
                    target: "kronn::ollama",
                    model = %model,
                    estimated_tokens = est,
                    ctx_cap = ctx_cap,
                    model_limit = ?model_limit,
                    "refusing an oversized prompt rather than letting Ollama truncate it"
                );
                return Err(format!(
                    "prompt is about {est} tokens but the context window is {ctx_cap} ({origin}). \
                 Sending it would make Ollama silently drop part of the conversation and answer \
                 from the remainder, so Kronn refuses instead: {remedy}."
                ));
            }
            let body = build_ollama_chat_body(model, &system_context, user_prompt, format, ctx_cap);
            (
                base,
                body,
                ctx_cap,
                model_limit,
                model_storage_format,
                Some(cap.origin),
            )
        };
    let url = codec.endpoint(&base);

    let client = reqwest::Client::builder()
        .timeout(http_request_timeout.unwrap_or_else(|| {
            // Direct/test callers without a server config still get a finite
            // policy. Production discussion paths always pass the Settings
            // value, including the initial cold-load request.
            if *agent_type == AgentType::Ollama {
                std::time::Duration::from_secs(240 * 60)
            } else {
                std::time::Duration::from_secs(30 * 60)
            }
        }))
        // Connect is bounded separately: without this, a black-holed OLLAMA_HOST
        // (firewall DROP, wrong host.docker.internal) sits in TCP connect for the
        // full request budget before "unreachable" surfaces, instead of failing in 5s like
        // the /api/show probe above.
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let backend = match agent_type {
        AgentType::LiteLlm => "LiteLLM",
        AgentType::Nvidia => "NVIDIA",
        _ => "Ollama",
    };
    // Ollama takes no credential; a LiteLLM proxy usually sits behind a
    // master key. Blank means "no auth configured", not "send an empty key".
    // Captured before the stream task, which cannot borrow `agent_type`.
    let is_openai_wire = is_openai_wire_agent(agent_type);
    let auth_key: Option<String> = http_api_key
        .filter(|k| !k.trim().is_empty() && is_openai_wire_agent(agent_type))
        .map(str::to_string);

    // Declaring tools is what turns a text generator into something that can
    // act. Only advertise them when there is an executor to honour the calls.
    let mut body = body;
    let mut tools_declared = 0usize;
    let tool_run_mode = executor
        .as_ref()
        .map(|exec| exec.run_mode())
        .unwrap_or_default();
    let worker_scope = executor.as_ref().and_then(|exec| exec.worker_scope());
    if worker_scope.is_some() && tool_run_mode != crate::agents::tools::ToolRunMode::Worker {
        return Err(format!(
            "{backend} refused a worker_scope outside a durable worker execution room"
        ));
    }
    let worker_policy =
        worker_exploration_policy(model, model_storage_format.as_deref(), is_openai_wire);
    let configured_ctx_cap = ctx_cap;
    let ctx_cap = worker_effective_ctx_cap(configured_ctx_cap, tool_run_mode, worker_policy);
    let mut worker_original_catalogue_seed = Vec::new();
    if let Some(exec) = executor.as_ref() {
        let catalogue = exec.catalogue();
        if tool_run_mode == crate::agents::tools::ToolRunMode::Worker {
            worker_original_catalogue_seed = catalogue.clone();
        }
        tools_declared = catalogue.len();
        if !catalogue.is_empty() {
            body["tools"] = serde_json::Value::Array(catalogue);
        }
    }
    if let Some(scope) = worker_scope.as_ref() {
        constrain_prelocalized_read_tool(&mut body, &worker_original_catalogue_seed, scope);
        tools_declared = body["tools"].as_array().map(Vec::len).unwrap_or(0);
    }
    // A turn that declares tools will grow by whatever they return, and Ollama
    // fixes the window when it loads the model — it does not grow mid-run. Sizing
    // it on the prompt alone gives a one-line question a tiny slot that the first
    // tool result blows past, and the history then gets truncated until the user
    // turn itself is gone. Ask for the effective cap up front when tools are on
    // the table. Native MLX workers have already reduced that cap above because
    // their engine does pay heavily for a nominal 65K slot; other engines keep
    // the configured/model-derived ceiling.
    if tools_declared > 0 && !is_openai_wire {
        let required =
            estimated_chat_history_tokens(&body).saturating_add(WORKER_FINALIZATION_REPLY_HEADROOM);
        if required > ctx_cap {
            let remedy = worker_oversized_prompt_remedy(configured_ctx_cap, worker_policy);
            return Err(format!(
                "{backend} worker prompt and tool catalogue need about {required} tokens, but the effective context window is {ctx_cap}. To continue, {remedy}; Kronn refuses to let Ollama truncate the brief."
            ));
        }
        if let Some(options) = body["options"].as_object_mut() {
            options.insert(
                "num_ctx".into(),
                serde_json::json!(ctx_cap.max(OLLAMA_NUM_CTX_FLOOR)),
            );
        }
    }
    tracing::info!(
        target: "kronn::agent::tools",
        backend, model = %model, tools_declared,
        configured_context_ceiling = configured_ctx_cap,
        effective_context_ceiling = ctx_cap,
        worker_exploration_limit = worker_policy.max_iterations,
        prelocalized_worker = worker_scope.is_some(),
        worker_context_pressure_percent = worker_policy.context_pressure_percent,
        mlx_mitigation = worker_policy.mlx_mitigation,
        mlx_detection_source = ?worker_policy.mlx_detection_source,
        "HTTP agent starting"
    );
    // The definitive Ollama sizing event is emitted only after tool
    // declaration, because a tooled run raises the request to the full
    // ceiling above. One event therefore reports one non-stale truth.
    if !is_openai_wire {
        tracing::info!(
            target: "kronn::ollama",
            model = %model,
            requested_num_ctx = body["options"]["num_ctx"].as_u64().unwrap_or(0),
            context_ceiling = ctx_cap,
            configured_context_ceiling = configured_ctx_cap,
            model_trained_context = ?model_trained_context,
            model_storage_format = ?model_storage_format,
            ctx_cap_origin = ?ctx_cap_origin,
            tools_declared,
            no_think = ollama_disables_thinking(model),
            constrained_format = format.is_some(),
            stream = body["stream"].as_bool().unwrap_or(true),
            "ollama run starting"
        );
    }

    let stderr_capture: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // Romu's rule, and it is the right one: if we hold a model below what it can
    // do, say it where the run is read — not only in a log.
    if let Some(notice) = ctx_notice {
        if let Ok(mut capture) = stderr_capture.lock() {
            capture.push(notice);
        }
    }
    // Capture the immutable request seed before any provider/tool turn is
    // appended. The checkpoint later uses this instead of guessing which user
    // prompts in the live loop are principal-authored.
    let worker_checkpoint_seed = (tool_run_mode == crate::agents::tools::ToolRunMode::Worker)
        .then(|| WorkerCheckpointSeed::from_body(&body));
    // Derive before the first request: until response headers arrive there is
    // no `AgentProcess` and therefore no child pid for generic cancellation to
    // kill. The caller-owned parent closes that blind spot.
    let http_cancel = parent_cancel
        .map(tokio_util::sync::CancellationToken::child_token)
        .unwrap_or_default();
    let initial_request_started_at = std::time::Instant::now();
    let initial = tokio::select! {
        biased;
        _ = http_cancel.cancelled() => {
            return Err(format!("{backend} run cancelled before the provider accepted the initial request"));
        }
        response = send_http_agent_request(
            &client,
            &url,
            &body,
            auth_key.as_deref(),
            backend,
            1,
            HTTP_PROVIDER_MAX_ATTEMPTS,
            true,
            &stderr_capture,
        ) => response,
    };
    let (response, initial_provider_attempt) = initial.map_err(|failure| {
        let tool_hint = if tools_declared > 0 {
            " Kronn declared native tools on this request; this provider route/model may not support tool calling. Choose a tool-capable model or move the call to an ApiCall step."
        } else {
            ""
        };
        format_provider_failure(backend, &base, &failure, tool_hint)
    })?;

    // Stream the response — each line is a JSON object with a `message.content` field.
    // The last chunk has `done: true` and includes token counts.
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(256);
    let stderr_clone = stderr_capture.clone();

    // AgentProcess requires a child, but Ollama's execution is HTTP-based. The
    // child must mirror the STREAM's lifetime exactly: consumers `child.wait()`
    // after the stream to finalize (a `sleep 3600` placeholder blocked there
    // for an hour — the UI spinner never stopped, 2026-07-01 bug), and the
    // audit zombie-probe `try_wait()`s after 60s idle (an already-exited child
    // would be a false zombie during a slow model cold-load).
    //
    // Solution: a tiny sh lifeline that blocks on `read` from a piped stdin
    // held by the streaming task, then exits with the STATUS the task writes.
    // The old `cat` variant exited 0 on every path — a mid-stream HTTP error
    // or an in-band {"error":…} object still looked like a SUCCESSFUL step
    // with truncated/empty output. Now: task writes "0\n" on a clean `done`,
    // "1\n" on stream/in-band errors, and if the task dies without writing
    // anything, `read` hits EOF and the lifeline exits 1 — fail-safe.
    let mut dummy_child = crate::core::cmd::async_cmd("sh")
        .args(["-c", r#"read -r s; exit "${s:-1}""#])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn stream-lifetime process: {}", e))?;
    let stdin_guard = dummy_child.stdin.take();

    // This token is the real lifecycle of an HTTP agent. The dummy child below
    // only lets generic callers await an exit status; cancelling/killing that
    // child cannot by itself stop a Tokio provider/tool loop.
    let task_cancel = http_cancel.clone();

    // Spawn a background task to read the HTTP stream and forward text to the channel.
    tokio::spawn(async move {
        // Holds the lifeline's stdin open for the duration of the stream.
        let mut lifeline = stdin_guard;
        // Report the stream's outcome as the lifeline child's exit code.
        // `ok=false` also covers "consumer dropped" (cancel) — the run is
        // being torn down anyway, so the failed wait() is never surfaced.
        async fn finish(lifeline: &mut Option<tokio::process::ChildStdin>, ok: bool) {
            if let Some(mut stdin) = lifeline.take() {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(if ok { b"0\n" } else { b"1\n" }).await;
            }
        }
        use crate::agents::tools::{
            assistant_tool_call_message, tool_result_message, trace_line, ToolCallAccumulator,
            MAX_TOOL_ITERATIONS,
        };
        use futures::StreamExt;
        let mut response = response;
        let mut provider_attempt = initial_provider_attempt;
        let mut request_started_at = initial_request_started_at;
        let mut http_turn_index = 0u32;
        // Be deliberately stricter than the minimum invariant: after ANY tool
        // execution, not only a known write tool, a model request is no longer
        // replayable. Tool catalogues do not yet expose idempotency metadata.
        let mut external_effect_observed = false;
        let mut turn: usize = 0;

        // Tool calls already executed this run, keyed by name + exact arguments, with
        // the result they returned. Not a cache for speed: it exists so a repeated
        // identical call gets an answer that tells the model to move on.
        // How many times one tool may run in a single turn-chain before Kronn stops
        // feeding it. `seen_calls` below only catches a call repeated ARGUMENT FOR
        // ARGUMENT; a model that varies one parameter each time slips past it and
        // spins — observed: 47 `api_call`s in a row, each slightly different, until
        // the round cap fired 47 minutes later. Twelve leaves room for honest work
        // (reading a dozen files, paging through a result set) while catching a
        // model that is circling instead of answering.
        let mut calls_per_tool: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut seen_calls: std::collections::HashMap<String, (bool, serde_json::Value)> =
            std::collections::HashMap::new();
        // One replay is useful: it gives a weaker model an explicit nudge with the
        // original result still attached. A second identical repeat proves that
        // the nudge did not converge, so refuse it and withdraw tools for that
        // turn instead of spending the full 12-call per-tool budget on model
        // round-trips that cannot produce new information.
        let mut repeated_calls: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        // Argument variation must not reset an error loop. The production failure
        // alternated `find_files` and `list_files` with a different path each time,
        // so exact-call deduplication never fired. Count failures by tool identity,
        // open that tool's circuit after repeated errors, and force a prose answer
        // after a bounded run of error-only rounds.
        let mut errors_per_tool: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        // Circuits react to a consecutive failure streak, not lifetime failures.
        // Repository discovery legitimately probes several absent paths; a later
        // successful read proves that exploration is progressing and closes that
        // streak. Keep total errors separately for the final diagnostic.
        let mut consecutive_errors_per_tool: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        // Argument variation must not reset a SUCCESS loop either. Measured on a
        // real delegation: `git_log` twelve times running with limit 100, 30, 50,
        // 10, 15 … — twelve distinct signatures, so exact-call deduplication never
        // fired, and the model got no signal at all until the cap refused the
        // thirteenth. Remember which payload each tool has already produced, so a
        // reworded question that yields the same answer is named as such.
        let mut results_seen_per_tool: std::collections::HashMap<(String, u64), String> =
            std::collections::HashMap::new();
        let mut refusals_per_tool: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut open_tool_circuits = std::collections::HashSet::<String>::new();
        // A tool this turn has refused, for whatever reason. Its DECLARATION
        // goes away, which is the only reliable way to stop a model asking for
        // it again — but the rest of the toolbox stays. Measured: a worker paged
        // one file until its read budget was gone, lost every tool including
        // `write_file`, and could only write a report about the fix it had
        // correctly worked out.
        let mut withdrawn_tools = std::collections::HashSet::<String>::new();
        let mut consecutive_error_only_rounds = 0usize;
        let mut useful_tool_results = 0usize;
        let mut forced_synthesis = false;
        // A worker gets one bounded exploration phase and then a small
        // finalization phase. The boundary adapts to the provider engine and
        // context pressure, turning the old 50th-turn crash into a bounded
        // chance to refresh a CAS receipt, edit, inspect, commit and deliver.
        // General/API agents keep the existing policy.
        let worker_run = tool_run_mode == crate::agents::tools::ToolRunMode::Worker;
        let worker_original_catalogue = worker_original_catalogue_seed;
        let prelocalized_worker = worker_scope.is_some();
        let mut explored_without_progress = 0usize;
        let mut worker_finalization_phase = false;
        let mut worker_finalization_turns = 0usize;
        let mut worker_finalization_read_calls = 0usize;
        let mut worker_finalization_git_inspection_calls = 0usize;
        let mut worker_finalization_git_tools_withdrawn =
            std::collections::HashSet::<String>::new();
        let mut worker_repair_stage = if prelocalized_worker {
            WorkerRepairStage::Read
        } else {
            WorkerRepairStage::Inactive
        };
        let mut worker_repair_turns = 0usize;
        let mut worker_strict_syntax_repair = false;
        let mut worker_repair_target: Option<crate::agents::tools::ToolCall> = None;
        let mut prelocalized_read_receipt: Option<String> = None;
        let mut worker_delivery_phase = false;
        let mut worker_delivery_turns = 0usize;
        let mut worker_prose_only_turns = 0usize;
        let mut worker_mutated_paths = std::collections::BTreeSet::<String>::new();
        // A successful edit is a state transition, not another observation.
        // Remember it independently so a weaker local model that subsequently
        // says "let me re-read" can be moved into bounded finalization instead
        // of being killed by the generic pre-action prose guard.
        let mut worker_workspace_mutated = false;
        // Whether this run has produced ANY visible text, and whether we have already
        // asked for a final answer once. A reasoning model that used tools sometimes
        // stops with `finish_reason: stop` and no prose at all — it considers itself
        // done. Reproduced against a real proxy: with a long system prompt and 24 tools
        // declared, the reply came back with zero text. The user then saw a failed run
        // with none of the work, though the tools HAD run.
        let mut emitted_text = false;
        let mut asked_for_answer = false;
        let ok = loop {
            http_turn_index = http_turn_index.saturating_add(1);
            let current_http_turn = http_turn_index;
            let current_http_phase = http_turn_phase(
                worker_run,
                prelocalized_worker,
                worker_repair_stage,
                worker_delivery_phase,
                worker_finalization_phase,
            );
            // Provider usage is per response. Resetting here prevents a clean
            // zero-usage/error frame from inheriting the preceding turn's
            // counts; parse_token_usage later sums the independent markers.
            let mut tally = TokenTally::default();
            // The response below was generated from this exact catalogue. A
            // model can remember a tool that was withdrawn on a previous turn
            // and still emit its name; declaration removal is not an execution
            // boundary unless Kronn verifies it before calling the executor.
            let declared_tools_for_turn = declared_tool_names(&body);
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut got_done = false;
            let mut got_error = false;
            let mut provider_error: Option<String> = None;
            let mut emitted_this_turn = false;
            let mut pending_tools = ToolCallAccumulator::default();
            let mut thinking_filter = LeadingThinkingFilter::default();

            loop {
                let chunk = tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => {
                        if let Ok(mut se) = stderr_clone.lock() {
                            se.push(format!("{backend} run cancelled while reading the provider stream"));
                        }
                        finish(&mut lifeline, false).await;
                        return;
                    }
                    chunk = stream.next() => chunk,
                };
                let Some(chunk) = chunk else { break };
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!("{} stream error: {}", backend, e);
                        provider_error = Some(format!("transport error: {e}"));
                        if let Ok(mut se) = stderr_clone.lock() {
                            se.push(format!(
                                "{backend} stream error (connection lost mid-generation): {e}"
                            ));
                        }
                        got_error = true;
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&bytes));

                // Process complete JSON lines (newline-delimited stream chunks).
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].to_string();
                    buffer = buffer[newline_pos + 1..].to_string();
                    // Consumer gone (cancel) → stop reading the HTTP body.
                    if !forward_chat_line(
                        codec.as_ref(),
                        backend,
                        &line,
                        &tx,
                        &stderr_clone,
                        &mut got_done,
                        &mut got_error,
                        &mut provider_error,
                        ctx_cap,
                        &mut tally,
                        &mut thinking_filter,
                        &mut pending_tools,
                        &mut emitted_this_turn,
                    )
                    .await
                    {
                        finish(&mut lifeline, false).await;
                        return;
                    }
                }
            }

            // Non-streaming responses (format-constrained / TypedSchema steps
            // set stream:false) arrive as a single JSON object with no trailing
            // newline, so the line loop above never fires — flush the remainder.
            let _ = forward_chat_line(
                codec.as_ref(),
                backend,
                buffer.trim(),
                &tx,
                &stderr_clone,
                &mut got_done,
                &mut got_error,
                &mut provider_error,
                ctx_cap,
                &mut tally,
                &mut thinking_filter,
                &mut pending_tools,
                &mut emitted_this_turn,
            )
            .await;

            let trailing = thinking_filter.finish();
            let had_trailing = !trailing.is_empty();
            if had_trailing && tx.send(trailing).await.is_err() {
                finish(&mut lifeline, false).await;
                return;
            }
            if had_trailing {
                emitted_this_turn = true;
            }
            emitted_text |= emitted_this_turn;

            let calls = pending_tools.finish();
            let worker_repair_stage_for_turn = worker_repair_stage;
            push_http_turn_trace(
                &stderr_clone,
                HttpTurnTrace {
                    version: 1,
                    turn: current_http_turn,
                    provider: backend.to_ascii_lowercase(),
                    phase: current_http_phase,
                    prompt_tokens: tally.prompt,
                    eval_tokens: tally.eval,
                    duration_ms: request_started_at
                        .elapsed()
                        .as_millis()
                        .min(u64::MAX as u128) as u64,
                    provider_ok: got_done && !got_error,
                    requested_tools: calls
                        .iter()
                        .take(MAX_HTTP_TELEMETRY_TOOLS_PER_TURN)
                        .map(|call| bounded_http_tool_name(&call.name))
                        .collect(),
                },
            );
            // A 2xx only means that the provider accepted the request; NVIDIA
            // may still put ResourceExhausted inside the SSE body. Call an
            // attempt successful only after its terminal frame was decoded.
            if got_done && !got_error && provider_attempt > 1 {
                push_provider_retry_trace(
                    &stderr_clone,
                    format!(
                        "{backend} completed on attempt {provider_attempt}/{HTTP_PROVIDER_MAX_ATTEMPTS}"
                    ),
                );
            }
            // NVIDIA can report worker saturation inside a HTTP-200 SSE frame.
            // A clean-but-terminal-less empty stream is the equivalent proxy
            // failure. Re-send the exact request only while nothing escaped to
            // the user and no tool has run; the same AgentProcess and dispatch
            // remain in place, so one run still persists one agent message.
            let retryable_stream_failure = if let Some(detail) = provider_error.as_deref() {
                is_transient_provider_failure(None, detail)
            } else {
                !got_done && !got_error && calls.is_empty()
            };
            if retryable_stream_failure
                && !external_effect_observed
                && !emitted_this_turn
                && provider_attempt < HTTP_PROVIDER_MAX_ATTEMPTS
            {
                let detail = provider_error
                    .as_deref()
                    .unwrap_or("stream ended before its terminal frame");
                let delay = provider_retry_delay(provider_attempt);
                push_provider_retry_trace(
                    &stderr_clone,
                    format!(
                        "{backend} attempt {provider_attempt}/{HTTP_PROVIDER_MAX_ATTEMPTS} failed ({}); retrying attempt {}/{} in {} ms",
                        provider_failure_label(None, detail),
                        provider_attempt + 1,
                        HTTP_PROVIDER_MAX_ATTEMPTS,
                        delay.as_millis()
                    ),
                );
                tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => {
                        finish(&mut lifeline, false).await;
                        return;
                    }
                    _ = tokio::time::sleep(delay) => {}
                }
                request_started_at = std::time::Instant::now();
                let retried = tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => {
                        finish(&mut lifeline, false).await;
                        return;
                    }
                    result = send_http_agent_request(
                        &client,
                        &url,
                        &body,
                        auth_key.as_deref(),
                        backend,
                        provider_attempt + 1,
                        HTTP_PROVIDER_MAX_ATTEMPTS,
                        true,
                        &stderr_clone,
                    ) => result,
                };
                match retried {
                    Ok((next_response, attempt)) => {
                        response = next_response;
                        provider_attempt = attempt;
                        continue;
                    }
                    Err(failure) => {
                        let msg = format_provider_failure(backend, &base, &failure, "");
                        if let Ok(mut se) = stderr_clone.lock() {
                            se.push(msg);
                        }
                        break false;
                    }
                }
            }
            if retryable_stream_failure
                && !external_effect_observed
                && !emitted_this_turn
                && provider_attempt >= HTTP_PROVIDER_MAX_ATTEMPTS
            {
                let detail = provider_error
                    .as_deref()
                    .unwrap_or("stream ended before its terminal frame");
                push_provider_retry_trace(
                    &stderr_clone,
                    format!(
                        "{backend} attempt {provider_attempt}/{HTTP_PROVIDER_MAX_ATTEMPTS} failed ({}); retry budget exhausted",
                        provider_failure_label(None, detail)
                    ),
                );
            }
            if forced_synthesis && !calls.is_empty() && !got_error {
                // The model ignored a tool-free synthesis turn and emitted another
                // tool call anyway. Do not give it 40 more chances: return one
                // bounded, honest diagnostic instead of ending as a generic agent
                // error after the global cap.
                let diagnostic = tool_convergence_diagnostic(
                    &calls_per_tool,
                    &errors_per_tool,
                    &refusals_per_tool,
                );
                let fallback = format!(
                    "Kronn stopped a non-progressing tool loop ({diagnostic}). \
                     {useful_tool_results} useful tool result(s) were obtained, but \
                     the model did not produce the requested synthesis. Treat the \
                     answer as partial and retry with a narrower request if needed."
                );
                if tx.send(fallback).await.is_err() {
                    finish(&mut lifeline, false).await;
                    return;
                }
                if let Ok(mut se) = stderr_clone.lock() {
                    se.push(format!(
                        "{backend} forced convergence fallback: {diagnostic}; useful_results={useful_tool_results}"
                    ));
                }
                break true;
            }

            if worker_delivery_phase {
                worker_delivery_turns += 1;
                if worker_delivery_turns > WORKER_DELIVERY_ITERATIONS {
                    let msg = format!(
                        "{backend} worker committed but did not submit its DeliveryManifest after \
                         {WORKER_DELIVERY_ITERATIONS} delivery-only rounds — giving up"
                    );
                    tracing::warn!(target: "kronn::agent::tools", "{}", msg);
                    if let Ok(mut se) = stderr_clone.lock() {
                        se.push(msg);
                    }
                    break false;
                }
                if calls.is_empty() && got_done && !got_error {
                    if worker_delivery_turns == WORKER_DELIVERY_ITERATIONS {
                        let msg = format!(
                            "{backend} worker committed but answered without `task_exec_deliver` \
                             on all {WORKER_DELIVERY_ITERATIONS} delivery-only rounds"
                        );
                        tracing::warn!(target: "kronn::agent::tools", "{}", msg);
                        if let Ok(mut se) = stderr_clone.lock() {
                            se.push(msg);
                        }
                        break false;
                    }
                    if let Some(messages) = body["messages"].as_array_mut() {
                        messages.push(serde_json::json!({
                            "role": "user",
                            "content": format!(
                                "A Git commit is not a Kronn delivery. Submit the required \
                                 DeliveryManifest v1 now with `task_exec_deliver`; it is the only \
                                 available tool. {} delivery-only attempt(s) remain.",
                                WORKER_DELIVERY_ITERATIONS - worker_delivery_turns
                            ),
                        }));
                    }
                    if !is_openai_wire {
                        clamp_ollama_tool_results(&mut body, ctx_cap);
                        resize_ollama_num_ctx(&mut body, ctx_cap);
                    }
                    request_started_at = std::time::Instant::now();
                    let delivery_retry = tokio::select! {
                        biased;
                        _ = task_cancel.cancelled() => {
                            finish(&mut lifeline, false).await;
                            return;
                        }
                        result = send_http_agent_request(
                            &client,
                            &url,
                            &body,
                            auth_key.as_deref(),
                            backend,
                            1,
                            1,
                            false,
                            &stderr_clone,
                        ) => result,
                    };
                    response = match delivery_retry {
                        Ok((response, _)) => response,
                        Err(failure) => {
                            if let Ok(mut se) = stderr_clone.lock() {
                                se.push(format!(
                                    "{backend} delivery-only retry failed: {}",
                                    failure.detail
                                ));
                            }
                            break false;
                        }
                    };
                    buffer.clear();
                    continue;
                }
            }

            if worker_repair_stage != WorkerRepairStage::Inactive {
                worker_repair_turns += 1;
                let repair_limit = worker_repair_iteration_limit(
                    worker_repair_stage,
                    worker_strict_syntax_repair,
                    prelocalized_worker,
                );
                if worker_repair_turns > repair_limit {
                    let reason_code = worker_repair_terminal_reason_code(
                        worker_repair_stage,
                        prelocalized_worker,
                    );
                    let msg = format!(
                        "{backend} worker exhausted its bounded {} phase after {repair_limit} rounds; reason_code={reason_code}",
                        worker_repair_stage.label()
                    );
                    tracing::warn!(target: "kronn::agent::tools", "{}", msg);
                    if let Ok(mut se) = stderr_clone.lock() {
                        se.push(msg);
                    }
                    break false;
                }
                if calls.is_empty() && got_done && !got_error {
                    if worker_repair_turns == repair_limit {
                        let reason_code = worker_repair_terminal_reason_code(
                            worker_repair_stage,
                            prelocalized_worker,
                        );
                        let msg = format!(
                            "{backend} worker answered without the required tool during its bounded {} phase; reason_code={reason_code}",
                            worker_repair_stage.label()
                        );
                        tracing::warn!(target: "kronn::agent::tools", "{}", msg);
                        if let Ok(mut se) = stderr_clone.lock() {
                            se.push(msg);
                        }
                        break false;
                    }
                    if let Some(messages) = body["messages"].as_array_mut() {
                        messages.push(serde_json::json!({
                            "role": "user",
                            "content": format!(
                                "The bounded {} phase is not complete. Use the only declared \
                                 tool(s) now; prose is not a mutation. {} attempt(s) remain.",
                                worker_repair_stage.label(),
                                repair_limit - worker_repair_turns
                            ),
                        }));
                    }
                    if !is_openai_wire {
                        clamp_ollama_tool_results(&mut body, ctx_cap);
                        resize_ollama_num_ctx(&mut body, ctx_cap);
                    }
                    request_started_at = std::time::Instant::now();
                    let repair_retry = tokio::select! {
                        biased;
                        _ = task_cancel.cancelled() => {
                            finish(&mut lifeline, false).await;
                            return;
                        }
                        result = send_http_agent_request(
                            &client,
                            &url,
                            &body,
                            auth_key.as_deref(),
                            backend,
                            1,
                            1,
                            false,
                            &stderr_clone,
                        ) => result,
                    };
                    response = match repair_retry {
                        Ok((response, _)) => response,
                        Err(failure) => {
                            if let Ok(mut se) = stderr_clone.lock() {
                                se.push(format!(
                                    "{backend} {} retry failed: {}",
                                    worker_repair_stage.label(),
                                    failure.detail
                                ));
                            }
                            break false;
                        }
                    };
                    buffer.clear();
                    continue;
                }
            }

            if worker_run
                && worker_finalization_phase
                && calls.is_empty()
                && got_done
                && !got_error
                && !forced_synthesis
                && body.get("tools").is_some()
                && !worker_delivery_phase
                && worker_repair_stage == WorkerRepairStage::Inactive
            {
                worker_finalization_turns += 1;
                if worker_finalization_turns >= WORKER_FINALIZATION_ITERATIONS {
                    let msg = format!(
                        "{backend} worker answered in prose without using a finalization tool on \
                         all {WORKER_FINALIZATION_ITERATIONS} bounded finalization rounds"
                    );
                    tracing::warn!(target: "kronn::agent::tools", "{}", msg);
                    if let Ok(mut se) = stderr_clone.lock() {
                        se.push(msg);
                    }
                    break false;
                }
                if let Some(messages) = body["messages"].as_array_mut() {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": format!(
                            "The workspace has already been mutated and this worker is now in \
                             bounded finalization. Prose is not progress: use one of the declared \
                             read/edit/inspect/commit/deliver tools now. {} finalization round(s) \
                             remain.",
                            WORKER_FINALIZATION_ITERATIONS - worker_finalization_turns
                        ),
                    }));
                }
                if !is_openai_wire {
                    clamp_ollama_tool_results(&mut body, ctx_cap);
                    resize_ollama_num_ctx(&mut body, ctx_cap);
                }
                request_started_at = std::time::Instant::now();
                let finalization_retry = tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => {
                        finish(&mut lifeline, false).await;
                        return;
                    }
                    result = send_http_agent_request(
                        &client,
                        &url,
                        &body,
                        auth_key.as_deref(),
                        backend,
                        1,
                        1,
                        false,
                        &stderr_clone,
                    ) => result,
                };
                response = match finalization_retry {
                    Ok((response, _)) => response,
                    Err(failure) => {
                        if let Ok(mut se) = stderr_clone.lock() {
                            se.push(format!(
                                "{backend} prose-only finalization retry failed: {}",
                                failure.detail
                            ));
                        }
                        break false;
                    }
                };
                buffer.clear();
                continue;
            }

            if worker_run
                && calls.is_empty()
                && got_done
                && !got_error
                && !forced_synthesis
                && body.get("tools").is_some()
                && !worker_delivery_phase
                && worker_repair_stage == WorkerRepairStage::Inactive
                && !worker_finalization_phase
            {
                if worker_workspace_mutated {
                    // The model has crossed the exploration/mutation boundary
                    // already. A prose intention here is evidence that its
                    // next request needs less choice, not that the task is
                    // complete. Enter the same fail-bounded finalization used
                    // by round/context pressure, while restoring one exact CAS
                    // refresh even if exploration exhausted read_file.
                    worker_finalization_phase = true;
                    worker_finalization_turns = 1;
                    retain_worker_finalization_tools(&mut body);
                    let checkpoint = checkpoint_worker_finalization_history(
                        &mut body,
                        worker_checkpoint_seed
                            .as_ref()
                            .expect("worker checkpoint seed"),
                        "A workspace mutation already succeeded. The managed worktree is the \
                         authoritative state; the earlier exploration transcript was checkpointed \
                         to prevent local-model context collapse. Inspect the current diff/files \
                         with the remaining tools, repair if needed, commit, then deliver.",
                        ctx_cap,
                        worker_workspace_mutated,
                        &worker_mutated_paths,
                    );
                    invalidate_workspace_observation_cache(
                        &mut seen_calls,
                        &mut repeated_calls,
                        &mut results_seen_per_tool,
                    );
                    calls_per_tool.remove("read_file");
                    errors_per_tool.remove("read_file");
                    consecutive_errors_per_tool.remove("read_file");
                    open_tool_circuits.remove("read_file");
                    withdrawn_tools.remove("read_file");
                    if let Some(messages) = body["messages"].as_array_mut() {
                        messages.push(serde_json::json!({
                            "role": "user",
                            "content": format!(
                                "A workspace mutation already succeeded. Kronn has entered bounded \
                                 finalization and withdrawn broad exploration tools. Use the \
                                 remaining read/edit/inspect/commit/deliver tools now. After each \
                                 successful edit, at most \
                                 {WORKER_FINALIZATION_GIT_INSPECTION_CALLS} combined \
                                 `git_status`/`git_diff` inspections are available before you must \
                                 commit, edit from the evidence, or state a blocker; prose about the \
                                 next action is not progress. {} finalization round(s) remain.",
                                WORKER_FINALIZATION_ITERATIONS - worker_finalization_turns,
                            ),
                        }));
                    }
                    if !is_openai_wire {
                        clamp_ollama_tool_results(&mut body, ctx_cap);
                        resize_ollama_num_ctx(&mut body, ctx_cap);
                    }
                    if let Ok(mut se) = stderr_clone.lock() {
                        se.push(format!(
                            "{backend} worker entered bounded finalization after a successful \
                             workspace mutation followed by prose-only intent; context checkpoint \
                             {}→{} messages (seed {}, recent tail {}, compacted tool results {}), \
                             approximately {}→{} tokens, final num_ctx {}",
                            checkpoint.before_messages,
                            checkpoint.after_messages,
                            checkpoint.seed_messages,
                            checkpoint.tail_messages,
                            checkpoint.compacted_tool_results,
                            checkpoint.before_tokens,
                            checkpoint.after_tokens,
                            checkpoint.final_num_ctx,
                        ));
                    }
                    request_started_at = std::time::Instant::now();
                    let finalization_retry = tokio::select! {
                        biased;
                        _ = task_cancel.cancelled() => {
                            finish(&mut lifeline, false).await;
                            return;
                        }
                        result = send_http_agent_request(
                            &client,
                            &url,
                            &body,
                            auth_key.as_deref(),
                            backend,
                            1,
                            1,
                            false,
                            &stderr_clone,
                        ) => result,
                    };
                    response = match finalization_retry {
                        Ok((response, _)) => response,
                        Err(failure) => {
                            if let Ok(mut se) = stderr_clone.lock() {
                                se.push(format!(
                                    "{backend} post-mutation finalization retry failed: {}",
                                    failure.detail
                                ));
                            }
                            break false;
                        }
                    };
                    buffer.clear();
                    continue;
                }
                worker_prose_only_turns += 1;
                if worker_prose_only_turns >= WORKER_PROSE_ONLY_ITERATIONS {
                    let msg = format!(
                        "{backend} worker answered in prose without using an available tool on \
                         all {WORKER_PROSE_ONLY_ITERATIONS} bounded attempts — no commit or \
                         DeliveryManifest was produced"
                    );
                    tracing::warn!(target: "kronn::agent::tools", "{}", msg);
                    if let Ok(mut se) = stderr_clone.lock() {
                        se.push(msg);
                    }
                    break false;
                }
                if let Some(messages) = body["messages"].as_array_mut() {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": format!(
                            "A worker task is not complete when you only describe the next action. \
                             Use one of the currently declared tools now and continue toward \
                             `git_commit` then `task_exec_deliver`. If no declared tool can make \
                             progress, state the exact blocker on the final attempt. {} corrective \
                             attempt(s) remain.",
                            WORKER_PROSE_ONLY_ITERATIONS - worker_prose_only_turns
                        ),
                    }));
                }
                if !is_openai_wire {
                    clamp_ollama_tool_results(&mut body, ctx_cap);
                    resize_ollama_num_ctx(&mut body, ctx_cap);
                }
                request_started_at = std::time::Instant::now();
                let worker_retry = tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => {
                        finish(&mut lifeline, false).await;
                        return;
                    }
                    result = send_http_agent_request(
                        &client,
                        &url,
                        &body,
                        auth_key.as_deref(),
                        backend,
                        1,
                        1,
                        false,
                        &stderr_clone,
                    ) => result,
                };
                response = match worker_retry {
                    Ok((response, _)) => response,
                    Err(failure) => {
                        if let Ok(mut se) = stderr_clone.lock() {
                            se.push(format!(
                                "{backend} prose-only worker retry failed: {}",
                                failure.detail
                            ));
                        }
                        break false;
                    }
                };
                buffer.clear();
                continue;
            }

            let Some(exec) = executor.clone().filter(|_| !calls.is_empty() && !got_error) else {
                // A model that used tools and then finished with no prose at all
                // considers itself done, but the user gets nothing — the tool
                // results are not an answer. Ask once, explicitly, for the written
                // reply. Bounded to a single extra turn and only when tools really
                // ran, so it cannot become a loop of its own.
                if got_done && !got_error && !emitted_text && !asked_for_answer && turn > 0 {
                    asked_for_answer = true;
                    if let Ok(mut se) = stderr_clone.lock() {
                        se.push(format!(
                            "{backend} finished without any text after using tools — asking once for the answer"
                        ));
                    }
                    if let Some(messages) = body["messages"].as_array_mut() {
                        messages.push(serde_json::json!({
                            "role": "user",
                            "content": "You used tools but wrote no answer. Reply now, in prose, \
                                        using what the tools returned. Do not call any more tools.",
                        }));
                    }
                    // Drop the tool declarations for this turn: the instruction is to
                    // write, and removing them makes another tool call impossible
                    // rather than merely discouraged.
                    if let Some(map) = body.as_object_mut() {
                        map.remove("tools");
                    }
                    if !is_openai_wire {
                        resize_ollama_num_ctx(&mut body, ctx_cap);
                    }
                    request_started_at = std::time::Instant::now();
                    let final_answer = tokio::select! {
                        biased;
                        _ = task_cancel.cancelled() => {
                            finish(&mut lifeline, false).await;
                            return;
                        }
                        result = send_http_agent_request(
                            &client,
                            &url,
                            &body,
                            auth_key.as_deref(),
                            backend,
                            1,
                            1,
                            false,
                            &stderr_clone,
                        ) => result,
                    };
                    response = match final_answer {
                        Ok((response, _)) => response,
                        Err(failure) => {
                            if let Ok(mut se) = stderr_clone.lock() {
                                se.push(format!(
                                    "{backend} refused the final-answer turn: {}. Automatic retry was skipped because a tool had already executed",
                                    match failure.status {
                                        Some(status) => format!("HTTP {status}"),
                                        None => failure.detail,
                                    }
                                ));
                            }
                            break false;
                        }
                    };
                    buffer.clear();
                    continue;
                }
                // No tool round-trip to do: this turn is the answer.
                //
                // A stream that ends without the terminal `done` chunk was
                // truncated (server closed, proxy cut it, model unloaded) —
                // that must not pass as a successful step.
                if !got_done && !got_error {
                    if let Ok(mut se) = stderr_clone.lock() {
                        se.push(format!(
                            "{backend} stream ended without a terminal 'done' chunk — output is likely truncated"
                        ));
                    }
                    got_error = true;
                }
                break got_done && !got_error;
            };

            worker_prose_only_turns = 0;
            turn += 1;
            // KT-403 — the consumer IS the run. When streaming.rs times out,
            // is stopped, or tears down, it drops the receiver; before this
            // check the loop kept executing tools for another 40+ minutes as a
            // zombie (measured), while the watchdog requeued a SECOND worker
            // onto the same worktree. A closed channel means nobody will ever
            // read another byte of this run: stop before spending a model turn
            // or an effectful tool call on it.
            if tx.is_closed() {
                let msg = format!(
                    "{backend} run abandoned by its consumer (timeout, stop or teardown) — \
                     halting the tool loop after turn {turn}"
                );
                tracing::warn!(target: "kronn::agent::tools", "{}", msg);
                if let Ok(mut se) = stderr_clone.lock() {
                    se.push(msg);
                }
                break false;
            }
            let mut entered_worker_finalization = false;
            let mut entered_worker_delivery = false;
            let mut entered_worker_repair = false;
            let mut worker_repair_prompt: Option<String> = None;
            let context_pressure_tokens = worker_run
                .then(|| {
                    worker_context_pressure(&body, ctx_cap, worker_policy.context_pressure_percent)
                })
                .flatten();
            let worker_boundary = worker_run
                .then(|| {
                    worker_exploration_boundary(
                        worker_policy,
                        turn,
                        explored_without_progress,
                        context_pressure_tokens,
                    )
                })
                .flatten();
            if worker_delivery_phase {
                // Delivery-only attempts are counted when their response is
                // decoded above. They must not consume the finalization budget
                // that produced the commit which made this phase possible.
            } else if worker_repair_stage_for_turn != WorkerRepairStage::Inactive {
                // A repair is armed by one failed edit and has its own strict
                // read/edit/commit budgets. It must remain usable even when
                // that edit was finalization round 12.
            } else if worker_finalization_phase {
                worker_finalization_turns += 1;
                if worker_finalization_turns > WORKER_FINALIZATION_ITERATIONS {
                    let msg = format!(
                        "{backend} worker did not finalize after its bounded exploration phase \
                         and {WORKER_FINALIZATION_ITERATIONS} finalization rounds — giving up"
                    );
                    tracing::warn!(target: "kronn::agent::tools", "{}", msg);
                    if let Ok(mut se) = stderr_clone.lock() {
                        se.push(msg);
                    }
                    break false;
                }
            } else if worker_boundary.is_some() {
                // Narrow the *next* turn to the tools that can turn gathered
                // evidence into a durable delivery. MLX-backed Ollama workers
                // transition earlier because that engine currently re-prefills
                // their whole growing history on every request. Every Ollama
                // worker also transitions before its request body approaches
                // the configured context ceiling.
                worker_finalization_phase = true;
                entered_worker_finalization = true;
                retain_worker_finalization_tools(&mut body);
                invalidate_workspace_observation_cache(
                    &mut seen_calls,
                    &mut repeated_calls,
                    &mut results_seen_per_tool,
                );
                // A CAS refresh must remain executable even when the worker
                // spent its whole exploration allowance paging the target.
                calls_per_tool.remove("read_file");
                errors_per_tool.remove("read_file");
                consecutive_errors_per_tool.remove("read_file");
                open_tool_circuits.remove("read_file");
                withdrawn_tools.remove("read_file");
            } else if turn > MAX_TOOL_ITERATIONS {
                // Refusing to converge is a failure, not a silent
                // truncation: surface it so the step fails with a reason.
                let msg = format!(
                    "{backend} kept requesting tools after {MAX_TOOL_ITERATIONS} rounds — giving up"
                );
                tracing::warn!(target: "kronn::agent::tools", "{}", msg);
                if let Ok(mut se) = stderr_clone.lock() {
                    se.push(msg);
                }
                break false;
            }

            // Execute, then feed the results back as the next turn's history.
            let mut results = Vec::with_capacity(calls.len());
            let mut budget_refusals = 0usize;
            let mut delivered_this_turn = false;
            let mut failed_edit_this_turn = false;
            let mut syntax_refused_edit_this_turn = false;
            let mut successful_edit_this_turn = false;
            let mut refused_finalization_read_this_turn = false;
            let mut repair_read_succeeded = false;
            let mut repair_edit_succeeded = false;
            for call in &calls {
                if !declared_tools_for_turn.contains(&call.name) {
                    if worker_run
                        && worker_finalization_phase
                        && !worker_delivery_phase
                        && worker_repair_stage_for_turn == WorkerRepairStage::Inactive
                        && call.name == "read_file"
                    {
                        refused_finalization_read_this_turn = true;
                    }
                    *refusals_per_tool.entry(call.name.clone()).or_insert(0) += 1;
                    tracing::warn!(
                        target: "kronn::agent::tools",
                        tool = %call.name,
                        turn,
                        "undeclared tool call refused before executor"
                    );
                    let refusal = crate::agents::tools::ToolOutcome {
                        call: call.clone(),
                        ok: false,
                        content: serde_json::json!({
                            "undeclared_tool": true,
                            "note": format!(
                                "`{}` is not available in the current tool catalogue and was not executed. Use only a tool declared on this turn; if none can satisfy the request, state the exact blocker.",
                                call.name
                            ),
                        }),
                    };
                    if let Ok(mut se) = stderr_clone.lock() {
                        se.push(trace_line(&refusal));
                        se.push(format!(
                            "{backend} refused undeclared tool `{}` at turn {turn}; it was absent from the request catalogue and the executor was not called",
                            call.name
                        ));
                    }
                    results.push(refusal);
                    budget_refusals += 1;
                    continue;
                }
                if let Some(scope) = worker_scope.as_ref() {
                    if matches!(
                        worker_repair_stage_for_turn,
                        WorkerRepairStage::Read | WorkerRepairStage::Edit
                    ) && !prelocalized_call_matches_scope(
                        call,
                        worker_repair_stage_for_turn,
                        scope,
                        prelocalized_read_receipt.as_deref(),
                    ) {
                        if worker_repair_stage_for_turn == WorkerRepairStage::Edit {
                            failed_edit_this_turn = true;
                        }
                        *refusals_per_tool.entry(call.name.clone()).or_insert(0) += 1;
                        let refusal = crate::agents::tools::ToolOutcome {
                            call: call.clone(),
                            ok: false,
                            content: serde_json::json!({
                                "prelocalized_scope_mismatch": true,
                                "note": "This tiny worker is mechanically frozen to the exact tool arguments declared in the current schema. Nothing was executed. Use those exact path/target/receipt values; only the declared mutation tool's new_string is yours to choose.",
                            }),
                        };
                        if let Ok(mut se) = stderr_clone.lock() {
                            se.push(trace_line(&refusal));
                            se.push(format!(
                                "{backend} refused a prelocalized {} call outside its frozen target at turn {turn}",
                                worker_repair_stage_for_turn.label()
                            ));
                        }
                        results.push(refusal);
                        budget_refusals += 1;
                        continue;
                    }
                }
                if worker_repair_stage_for_turn == WorkerRepairStage::Edit
                    && worker_strict_syntax_repair
                    && worker_repair_target
                        .as_ref()
                        .is_some_and(|target| !worker_repair_call_matches_target(target, call))
                {
                    failed_edit_this_turn = true;
                    let refusal = crate::agents::tools::ToolOutcome {
                        call: call.clone(),
                        ok: false,
                        content: serde_json::json!({
                            "strict_repair_target_mismatch": true,
                            "note": "The single syntax-repair attempt must keep the exact tool, path and anchor/range of the refused edit. Only the replacement bytes may change. Nothing was executed.",
                        }),
                    };
                    if let Ok(mut se) = stderr_clone.lock() {
                        se.push(trace_line(&refusal));
                        se.push(format!(
                            "{backend} refused syntax repair outside its preconstructed target at turn {turn}"
                        ));
                    }
                    results.push(refusal);
                    budget_refusals += 1;
                    continue;
                }
                let finalization_read = worker_finalization_phase
                    && !entered_worker_finalization
                    && worker_repair_stage_for_turn == WorkerRepairStage::Inactive
                    && call.name == "read_file";
                if finalization_read
                    && worker_finalization_read_calls >= WORKER_FINALIZATION_READ_FILE_CALLS
                {
                    refused_finalization_read_this_turn = true;
                    *refusals_per_tool.entry(call.name.clone()).or_insert(0) += 1;
                    let refusal = crate::agents::tools::ToolOutcome {
                        call: call.clone(),
                        ok: false,
                        content: serde_json::json!({
                            "finalization_read_budget_exhausted": true,
                            "note": format!(
                                "The bounded finalization phase already used its {} exact `read_file` refreshes. No further repository exploration is allowed. Edit from the evidence already collected, then inspect, commit and deliver; otherwise state the exact blocker.",
                                WORKER_FINALIZATION_READ_FILE_CALLS
                            ),
                        }),
                    };
                    if let Ok(mut se) = stderr_clone.lock() {
                        se.push(trace_line(&refusal));
                        se.push(format!(
                            "{backend} refused finalization read_file beyond the {}-call refresh budget at turn {turn}",
                            WORKER_FINALIZATION_READ_FILE_CALLS
                        ));
                    }
                    results.push(refusal);
                    budget_refusals += 1;
                    continue;
                }
                let exhausts_finalization_reads = finalization_read
                    && worker_finalization_read_calls + 1 == WORKER_FINALIZATION_READ_FILE_CALLS;
                if finalization_read {
                    worker_finalization_read_calls += 1;
                    if exhausts_finalization_reads {
                        // Remove it from the next request. The current call was
                        // declared and remains valid; a same-batch fourth read
                        // is refused by the explicit budget above.
                        withdrawn_tools.insert("read_file".into());
                    }
                }
                let finalization_git_inspection = worker_finalization_phase
                    && !entered_worker_finalization
                    && !worker_delivery_phase
                    && worker_repair_stage_for_turn == WorkerRepairStage::Inactive
                    && matches!(call.name.as_str(), "git_status" | "git_diff");
                if finalization_git_inspection
                    && worker_finalization_git_inspection_calls
                        >= WORKER_FINALIZATION_GIT_INSPECTION_CALLS
                {
                    *refusals_per_tool.entry(call.name.clone()).or_insert(0) += 1;
                    let refusal = crate::agents::tools::ToolOutcome {
                        call: call.clone(),
                        ok: false,
                        content: serde_json::json!({
                            "finalization_git_inspection_budget_exhausted": true,
                            "note": format!(
                                "The bounded finalization phase already used its {} combined `git_status`/`git_diff` inspections since the last successful edit. Those tools cannot add more evidence in this mutation epoch. Commit the inspected changes now, make a justified edit from the evidence already collected, or state the exact blocker.",
                                WORKER_FINALIZATION_GIT_INSPECTION_CALLS
                            ),
                        }),
                    };
                    if let Ok(mut se) = stderr_clone.lock() {
                        se.push(trace_line(&refusal));
                        se.push(format!(
                            "{backend} refused finalization `{}` beyond the {}-call combined Git inspection budget at turn {turn}",
                            call.name, WORKER_FINALIZATION_GIT_INSPECTION_CALLS
                        ));
                    }
                    results.push(refusal);
                    budget_refusals += 1;
                    continue;
                }
                let exhausts_finalization_git_inspections = finalization_git_inspection
                    && worker_finalization_git_inspection_calls + 1
                        == WORKER_FINALIZATION_GIT_INSPECTION_CALLS;
                if finalization_git_inspection {
                    worker_finalization_git_inspection_calls += 1;
                    if exhausts_finalization_git_inspections {
                        for name in ["git_status", "git_diff"] {
                            if !withdrawn_tools.contains(name) {
                                worker_finalization_git_tools_withdrawn.insert(name.to_string());
                            }
                            withdrawn_tools.insert(name.to_string());
                        }
                    }
                }
                // A weaker model can ask for the SAME call over and over: observed
                // here, task_list() seven times running until the round cap fired,
                // 10 185 tokens for nothing. Re-running it would return the same
                // bytes and teach the model nothing, so answer from the first result
                // and say plainly that repeating will not change it. This breaks the
                // loop several rounds before the cap, and the cap stays as backstop.
                // Canonical: serde_json preserves insertion order, so the same
                // call re-emitted with its keys in another order would otherwise
                // look new. Sorting the pairs makes the signature about content.
                let canonical_args = match call.arguments.as_object() {
                    Some(map) => {
                        let mut pairs: Vec<_> =
                            map.iter().map(|(k, v)| format!("{k}={v}")).collect();
                        pairs.sort();
                        pairs.join(",")
                    }
                    None => call.arguments.to_string(),
                };
                let signature = format!("{}|{}", call.name, canonical_args);

                let used = calls_per_tool.entry(call.name.clone()).or_insert(0);
                *used += 1;
                let call_index = *used;
                if open_tool_circuits.contains(&call.name) {
                    *refusals_per_tool.entry(call.name.clone()).or_insert(0) += 1;
                    let refusal = crate::agents::tools::ToolOutcome {
                        call: call.clone(),
                        ok: false,
                        content: serde_json::json!({
                            "tool_circuit_open": true,
                            "note": format!(
                                "`{}` was disabled for this turn after {} failed calls. \
                                 Do not vary its arguments and retry; answer from the \
                                 evidence already available.",
                                call.name,
                                consecutive_errors_per_tool
                                    .get(&call.name)
                                    .copied()
                                    .unwrap_or_default(),
                            ),
                        }),
                    };
                    if let Ok(mut se) = stderr_clone.lock() {
                        se.push(trace_line(&refusal));
                    }
                    results.push(refusal);
                    budget_refusals += 1;
                    continue;
                }
                let tool_limit = max_calls_for_tool(&call.name, tool_run_mode);
                if *used > tool_limit {
                    *refusals_per_tool.entry(call.name.clone()).or_insert(0) += 1;
                    withdrawn_tools.insert(call.name.clone());
                    tracing::warn!(
                        target: "kronn::agent::tools",
                        tool = %call.name, turn, used = *used,
                        "tool call budget exhausted — refusing to run it again"
                    );
                    let refusal = crate::agents::tools::ToolOutcome {
                        call: call.clone(),
                        ok: false,
                        content: serde_json::json!({
                            "budget_exhausted": true,
                            "note": format!(
                                "You have called `{}` {} times in this turn without \
                                 producing an answer. It will not run again. Answer now \
                                 with what you already have, and say plainly what is \
                                 still missing.",
                                call.name, *used - 1
                            ),
                        }),
                    };
                    if let Ok(mut se) = stderr_clone.lock() {
                        se.push(crate::agents::tools::trace_line(&refusal));
                    }
                    results.push(refusal);
                    budget_refusals += 1;
                    continue;
                }

                let mut fresh_execution = false;
                let outcome = match seen_calls.get(&signature) {
                    Some((previous_ok, previous)) => {
                        let repeats = repeated_calls.entry(signature.clone()).or_insert(0);
                        *repeats += 1;
                        if *repeats > 1 {
                            *refusals_per_tool.entry(call.name.clone()).or_insert(0) += 1;
                            tracing::warn!(
                                target: "kronn::agent::tools",
                                tool = %call.name, turn, repeats = *repeats,
                                "identical tool call repeated after replay — refusing and forcing synthesis"
                            );
                            withdrawn_tools.insert(call.name.clone());
                            let refusal = crate::agents::tools::ToolOutcome {
                                call: call.clone(),
                                ok: false,
                                content: serde_json::json!({
                                    "repeated_call_refused": true,
                                    "note": "This exact call was already executed and its result was replayed once. It cannot produce new information. Answer now from the available result; say plainly what remains unknown.",
                                }),
                            };
                            budget_refusals += 1;
                            refusal
                        } else {
                            tracing::info!(
                                target: "kronn::agent::tools",
                                tool = %call.name, turn,
                                "identical tool call repeated — replaying the first result"
                            );
                            crate::agents::tools::ToolOutcome {
                                call: call.clone(),
                                ok: *previous_ok,
                                content: serde_json::json!({
                                    "repeated_call": true,
                                    "note": "You already made this exact call with these exact \
                                             arguments. This is the same result as before — calling it \
                                             again cannot change it. Use it and answer, or call a \
                                             DIFFERENT tool.",
                                    "result": previous,
                                }),
                            }
                        }
                    }
                    None => {
                        // Set before awaiting the tool: a timeout or panic may
                        // happen after the external system accepted the call.
                        // From this point onward no provider retry is safe.
                        external_effect_observed = true;
                        fresh_execution = true;
                        let fresh = tokio::select! {
                            biased;
                            _ = task_cancel.cancelled() => {
                                if let Ok(mut se) = stderr_clone.lock() {
                                    se.push(format!(
                                        "{backend} run cancelled while `{}` was in flight; no later tool in this batch was started",
                                        call.name
                                    ));
                                }
                                finish(&mut lifeline, false).await;
                                return;
                            }
                            outcome = exec.execute(call) => outcome,
                        };
                        seen_calls.insert(signature, (fresh.ok, fresh.content.clone()));
                        fresh
                    }
                };
                let mut outcome = outcome;
                let edit_tool = matches!(
                    call.name.as_str(),
                    "write_file" | "edit_file" | "edit_lines" | "insert_after_line"
                );
                if edit_tool {
                    if outcome.ok {
                        successful_edit_this_turn = true;
                        repair_edit_succeeded = true;
                        if let Some(path) =
                            call.arguments.get("path").and_then(|path| path.as_str())
                        {
                            worker_mutated_paths.insert(path.to_string());
                        }
                    } else {
                        failed_edit_this_turn = true;
                        if rust_syntax_refusal(&outcome) {
                            syntax_refused_edit_this_turn = true;
                            worker_repair_target.get_or_insert_with(|| call.clone());
                        }
                        if let Some(payload) = outcome.content.as_object_mut() {
                            payload.insert(
                                "kronn_edit_recovery".into(),
                                serde_json::json!(
                                    "No mutation occurred. If Kronn grants a repair read, use it on the exact target slice. Then retry with one declared edit tool. `edit_lines` requires path, positive start_line/end_line, new_string, and expected_sha256; `insert_after_line` requires path, positive anchor_line, non-empty new_string, and expected_sha256; `edit_file` requires path, byte-exact old_string, new_string, and expected_sha256."
                                ),
                            );
                        }
                    }
                }
                if worker_repair_stage_for_turn == WorkerRepairStage::Read
                    && call.name == "read_file"
                    && outcome.ok
                {
                    repair_read_succeeded = true;
                    if prelocalized_worker {
                        prelocalized_read_receipt = outcome
                            .content
                            .get("content_sha256")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string);
                        if prelocalized_read_receipt.is_none() {
                            repair_read_succeeded = false;
                            outcome.ok = false;
                            outcome.content = serde_json::json!({
                                "prelocalized_receipt_missing": true,
                                "note": "The authoritative read did not return content_sha256, so Kronn cannot arm the CAS edit. Nothing was written; this bounded attempt will fail closed."
                            });
                        }
                    }
                }
                if outcome.ok && call.name == "task_exec_deliver" {
                    delivered_this_turn = true;
                }
                if worker_run && fresh_execution && outcome.ok && call.name == "git_commit" {
                    worker_delivery_phase = true;
                    worker_repair_stage = WorkerRepairStage::Inactive;
                    worker_strict_syntax_repair = false;
                    worker_repair_target = None;
                    entered_worker_delivery = true;
                    set_worker_tools_from_catalogue(
                        &mut body,
                        &worker_original_catalogue,
                        &["task_exec_deliver"],
                    );

                    // Delivery is a fresh protocol phase. Exploration or an
                    // early invalid delivery attempt must not leave its replay,
                    // budget or circuit state attached to the only remaining
                    // tool.
                    calls_per_tool.remove("task_exec_deliver");
                    errors_per_tool.remove("task_exec_deliver");
                    consecutive_errors_per_tool.remove("task_exec_deliver");
                    refusals_per_tool.remove("task_exec_deliver");
                    open_tool_circuits.remove("task_exec_deliver");
                    withdrawn_tools.remove("task_exec_deliver");
                    seen_calls.retain(|signature, _| !signature.starts_with("task_exec_deliver|"));
                    repeated_calls
                        .retain(|signature, _| !signature.starts_with("task_exec_deliver|"));
                    results_seen_per_tool.retain(|(tool, _), _| tool != "task_exec_deliver");

                    tracing::info!(
                        target: "kronn::agent::tools",
                        turn,
                        delivery_rounds = WORKER_DELIVERY_ITERATIONS,
                        "worker commit recorded — entering delivery-only phase"
                    );
                    if let Ok(mut se) = stderr_clone.lock() {
                        se.push(format!(
                            "{backend} worker commit succeeded; only `task_exec_deliver` remains for {WORKER_DELIVERY_ITERATIONS} bounded delivery rounds"
                        ));
                    }
                }
                if exhausts_finalization_reads {
                    if let Some(payload) = outcome.content.as_object_mut() {
                        payload.insert(
                            "kronn_finalization_read_limit".into(),
                            serde_json::json!(format!(
                                "This was the last of {} exact read refreshes allowed during bounded finalization. `read_file` is now withdrawn; edit, inspect, commit and deliver from the evidence already collected.",
                                WORKER_FINALIZATION_READ_FILE_CALLS
                            )),
                        );
                    }
                    tracing::info!(
                        target: "kronn::agent::tools",
                        turn,
                        finalization_read_calls = worker_finalization_read_calls,
                        "worker finalization read budget reached — withdrawing read_file"
                    );
                }
                if exhausts_finalization_git_inspections {
                    if let Some(payload) = outcome.content.as_object_mut() {
                        payload.insert(
                            "kronn_finalization_git_inspection_limit".into(),
                            serde_json::json!(format!(
                                "This was the last of {} combined `git_status`/`git_diff` inspections allowed since the last successful edit. Both inspection tools are now withdrawn for this mutation epoch; commit, make a justified edit, or state the exact blocker.",
                                WORKER_FINALIZATION_GIT_INSPECTION_CALLS
                            )),
                        );
                    }
                    tracing::info!(
                        target: "kronn::agent::tools",
                        turn,
                        finalization_git_inspection_calls =
                            worker_finalization_git_inspection_calls,
                        "worker finalization Git inspection budget reached — withdrawing git_status and git_diff"
                    );
                }
                if fresh_execution {
                    if outcome.ok {
                        useful_tool_results += 1;
                        consecutive_errors_per_tool.insert(call.name.clone(), 0);
                        annotate_unproductive_repetition(
                            &mut outcome,
                            &call.name,
                            &canonical_args,
                            call_index,
                            tool_limit,
                            &mut results_seen_per_tool,
                        );
                        if worker_run && is_workspace_observation_tool(&call.name) {
                            explored_without_progress += 1;
                            annotate_worker_exploration(&mut outcome, explored_without_progress);
                        } else if worker_run && is_workspace_progress_tool(&call.name) {
                            explored_without_progress = 0;
                            worker_workspace_mutated = true;
                            // Repository observations are snapshots. Replaying a
                            // pre-edit read_file/git_status result after a write
                            // can hand the worker a stale CAS receipt or falsely
                            // claim its worktree is still clean. Effectful/API
                            // calls deliberately stay cached.
                            invalidate_workspace_observation_cache(
                                &mut seen_calls,
                                &mut repeated_calls,
                                &mut results_seen_per_tool,
                            );
                        }
                    } else {
                        *errors_per_tool.entry(call.name.clone()).or_insert(0) += 1;
                        let consecutive = consecutive_errors_per_tool
                            .entry(call.name.clone())
                            .or_insert(0);
                        *consecutive += 1;
                        if *consecutive >= MAX_ERRORS_PER_TOOL {
                            open_tool_circuits.insert(call.name.clone());
                        }
                    }
                    push_http_tool_exec_trace(
                        &stderr_clone,
                        current_http_turn,
                        &call.name,
                        outcome.ok,
                    );
                }
                tracing::info!(
                    target: "kronn::agent::tools",
                    tool = %call.name, ok = outcome.ok, turn,
                    "HTTP agent tool call"
                );
                if let Ok(mut se) = stderr_clone.lock() {
                    se.push(trace_line(&outcome));
                }
                results.push(outcome);
            }

            if worker_run
                && syntax_refused_edit_this_turn
                && !worker_delivery_phase
                && worker_repair_stage_for_turn == WorkerRepairStage::Inactive
                && worker_repair_stage == WorkerRepairStage::Inactive
                && !successful_edit_this_turn
            {
                // A parser refusal is stronger evidence than an exploration
                // boundary: the target and receipt are already known and no
                // bytes changed. End exploration immediately and spend the
                // one local correction on that exact target.
                worker_finalization_phase = true;
                worker_finalization_turns = 0;
            }

            if worker_run
                && worker_finalization_phase
                && !worker_delivery_phase
                && worker_repair_stage_for_turn == WorkerRepairStage::Inactive
                && successful_edit_this_turn
            {
                worker_finalization_git_inspection_calls = 0;
                let restorable_git_tools = worker_finalization_git_tools_withdrawn
                    .drain()
                    .filter(|name| {
                        !open_tool_circuits.contains(name)
                            && calls_per_tool.get(name).copied().unwrap_or_default()
                                < max_calls_for_tool(name, tool_run_mode)
                    })
                    .collect::<Vec<_>>();
                for name in &restorable_git_tools {
                    withdrawn_tools.remove(name);
                }
                if !restorable_git_tools.is_empty() {
                    let names = restorable_git_tools
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>();
                    restore_worker_tools_from_catalogue(
                        &mut body,
                        &worker_original_catalogue,
                        &names,
                    );
                    tracing::info!(
                        target: "kronn::agent::tools",
                        turn,
                        "worker edit started a new finalization Git inspection epoch"
                    );
                }
            }

            let mut reset_tool_loop_state = |names: &[&str]| {
                for name in names {
                    calls_per_tool.remove(*name);
                    errors_per_tool.remove(*name);
                    consecutive_errors_per_tool.remove(*name);
                    refusals_per_tool.remove(*name);
                    open_tool_circuits.remove(*name);
                    withdrawn_tools.remove(*name);
                }
                seen_calls.retain(|signature, _| {
                    !signature
                        .split_once('|')
                        .is_some_and(|(name, _)| names.contains(&name))
                });
                repeated_calls.retain(|signature, _| {
                    !signature
                        .split_once('|')
                        .is_some_and(|(name, _)| names.contains(&name))
                });
                results_seen_per_tool.retain(|(name, _), _| !names.contains(&name.as_str()));
            };

            if worker_run
                && worker_finalization_phase
                && !worker_delivery_phase
                && worker_repair_stage_for_turn == WorkerRepairStage::Inactive
                && worker_repair_stage == WorkerRepairStage::Inactive
                && (failed_edit_this_turn || refused_finalization_read_this_turn)
                && !successful_edit_this_turn
            {
                worker_repair_stage = if syntax_refused_edit_this_turn {
                    WorkerRepairStage::Edit
                } else {
                    WorkerRepairStage::Read
                };
                worker_repair_turns = 0;
                entered_worker_repair = true;
                forced_synthesis = false;
                let (repair_prompt, transition) = if syntax_refused_edit_this_turn {
                    worker_strict_syntax_repair = true;
                    let target = worker_repair_target
                        .as_ref()
                        .expect("syntax refusal records its edit target");
                    let tool_name = target.name.as_str();
                    reset_tool_loop_state(&[tool_name]);
                    set_worker_tools_from_catalogue(
                        &mut body,
                        &worker_original_catalogue,
                        &[tool_name],
                    );
                    constrain_worker_repair_tool(&mut body, target);
                    (
                        format!(
                            "Rust syntax validation refused the edit and wrote nothing. The prior \
                             receipt is still authoritative. One strict correction remains: use \
                             only `{tool_name}` on the exact preconstructed path and anchor/range \
                             frozen in its schema; change only the replacement bytes using the \
                             parser error above. Any prose, exploration, different target or \
                             second invalid proposal ends this local attempt and hands the task \
                             back for a stronger worker."
                        ),
                        "worker Rust syntax refusal — entering one strict repair edit",
                    )
                } else {
                    reset_tool_loop_state(&["read_file"]);
                    set_worker_tools_from_catalogue(
                        &mut body,
                        &worker_original_catalogue,
                        &["read_file"],
                    );
                    let (repair_intro, transition) = if failed_edit_this_turn {
                        (
                            "The edit was refused and nothing changed.",
                            "worker edit refusal — entering one-shot repair read",
                        )
                    } else {
                        (
                            "The bounded finalization read was refused after the normal refresh budget. The last receipt may now be stale.",
                            "worker finalization read refusal — entering one-shot repair read",
                        )
                    };
                    (
                        format!(
                            "{repair_intro} Kronn has armed one non-renewable repair sequence outside the \
                             {WORKER_FINALIZATION_ITERATIONS}-round finalization budget. Repair read: call \
                             `read_file` once on only the exact target slice and keep its `content_sha256`. \
                             It is the only available tool; one invalid response gets one corrective retry."
                        ),
                        transition,
                    )
                };
                worker_repair_prompt = Some(repair_prompt);
                tracing::info!(target: "kronn::agent::tools", turn, "{transition}");
                if let Ok(mut se) = stderr_clone.lock() {
                    se.push(format!("{backend} {transition}"));
                }
            } else if worker_repair_stage_for_turn == WorkerRepairStage::Read
                && repair_read_succeeded
            {
                worker_repair_stage = WorkerRepairStage::Edit;
                worker_repair_turns = 0;
                reset_tool_loop_state(&[
                    "write_file",
                    "edit_file",
                    "edit_lines",
                    "insert_after_line",
                ]);
                if let Some(scope) = worker_scope.as_ref() {
                    let receipt = prelocalized_read_receipt
                        .as_deref()
                        .expect("successful prelocalized read has a CAS receipt");
                    let mutation_tool = prelocalized_mutation_tool(scope);
                    constrain_prelocalized_edit_tool(
                        &mut body,
                        &worker_original_catalogue,
                        scope,
                        receipt,
                    );
                    worker_repair_prompt = Some(format!(
                        "The authoritative prelocalized read succeeded. `read_file` is now \
                         permanently withdrawn. Call the only declared `{mutation_tool}` tool on \
                         its frozen target and CAS receipt; choose only `new_string`. You have \
                         this attempt plus one correction. Do not search, re-read, broaden the \
                         target or answer with an implementation plan."
                    ));
                } else {
                    set_worker_tools_from_catalogue(
                        &mut body,
                        &worker_original_catalogue,
                        &["write_file", "edit_file", "edit_lines", "insert_after_line"],
                    );
                    worker_repair_prompt = Some(format!(
                        "The single repair read succeeded and `read_file` is withdrawn again. Repair \
                         edit ({WORKER_REPAIR_EDIT_ITERATIONS} rounds maximum): mutate the target now \
                         with one declared edit tool and the fresh receipt. `edit_lines` requires path, \
                         positive start_line/end_line, new_string, expected_sha256; \
                         `insert_after_line` requires path, positive anchor_line, non-empty new_string, \
                         expected_sha256; `edit_file` requires path, byte-exact old_string, new_string, \
                         expected_sha256."
                    ));
                }
                tracing::info!(
                    target: "kronn::agent::tools",
                    turn,
                    "worker repair read succeeded — entering repair edit"
                );
                if let Ok(mut se) = stderr_clone.lock() {
                    se.push(format!(
                        "{backend} worker repair read succeeded — entering repair edit"
                    ));
                }
            } else if worker_repair_stage_for_turn == WorkerRepairStage::Edit
                && repair_edit_succeeded
            {
                worker_repair_stage = WorkerRepairStage::Commit;
                worker_repair_turns = 0;
                reset_tool_loop_state(&["git_status", "git_diff", "git_commit"]);
                let commit_tools = if prelocalized_worker {
                    &["git_commit"][..]
                } else {
                    &["git_status", "git_diff", "git_commit"][..]
                };
                set_worker_tools_from_catalogue(
                    &mut body,
                    &worker_original_catalogue,
                    commit_tools,
                );
                worker_repair_prompt = Some(if prelocalized_worker {
                    "The exact prelocalized edit succeeded. Only `git_commit` remains: call it now \
                     with the frozen target file and a concise message. You have this attempt plus \
                     one correction. Kronn derives and verifies the committed inventory; no read, \
                     diff, status or edit tool remains."
                        .into()
                } else {
                    format!(
                        "The repair edit succeeded. Repair commit ({WORKER_REPAIR_COMMIT_ITERATIONS} \
                         rounds maximum): inspect with `git_status`/`git_diff` as needed, then call \
                         `git_commit` with explicit files and message. No read or edit tool remains."
                    )
                });
                tracing::info!(
                    target: "kronn::agent::tools",
                    turn,
                    "worker repair edit succeeded — entering repair commit"
                );
                if let Ok(mut se) = stderr_clone.lock() {
                    se.push(format!(
                        "{backend} worker repair edit succeeded — entering repair commit"
                    ));
                }
            }

            if let Some(messages) = body["messages"].as_array_mut() {
                // OpenAI wants JSON-encoded arguments; Ollama wants a real object.
                messages.push(assistant_tool_call_message(&calls, is_openai_wire));
                for outcome in &results {
                    messages.push(tool_result_message(outcome));
                }
            }

            // The phase boundary applies to the NEXT provider request. Execute
            // and record the already-authorized call first, then checkpoint the
            // resulting evidence. Doing this before execution left the in-flight
            // call outside the checkpoint; a withdrawn `search_text` was appended
            // afterwards and immediately taught Qwen to request it again.
            let finalization_checkpoint = entered_worker_finalization.then(|| {
                checkpoint_worker_finalization_history(
                    &mut body,
                    worker_checkpoint_seed
                        .as_ref()
                        .expect("worker checkpoint seed"),
                    "Exploration is complete. The managed worktree is the authoritative state; \
                     the earlier observation transcript was checkpointed to keep this local-model \
                     phase small and tool-capable. Inspect only the current diff/files as needed, \
                     finish the mutation, commit, then deliver.",
                    ctx_cap,
                    worker_workspace_mutated,
                    &worker_mutated_paths,
                )
            });
            if let (Some(checkpoint), Some(worker_boundary)) =
                (finalization_checkpoint, worker_boundary)
            {
                tracing::info!(
                    target: "kronn::agent::tools",
                    turn,
                    explored_without_progress,
                    exploration_limit = worker_policy.max_iterations,
                    observation_limit = ?worker_policy.max_observations_without_mutation,
                    boundary = ?worker_boundary,
                    estimated_prompt_tokens = ?context_pressure_tokens,
                    context_ceiling = ctx_cap,
                    context_pressure_percent = worker_policy.context_pressure_percent,
                    mlx_mitigation = worker_policy.mlx_mitigation,
                    finalization_rounds = WORKER_FINALIZATION_ITERATIONS,
                    messages_before_checkpoint = checkpoint.before_messages,
                    messages_after_checkpoint = checkpoint.after_messages,
                    seed_messages_retained = checkpoint.seed_messages,
                    tail_messages_retained = checkpoint.tail_messages,
                    compacted_tool_results = checkpoint.compacted_tool_results,
                    tokens_before_checkpoint = checkpoint.before_tokens,
                    tokens_after_checkpoint = checkpoint.after_tokens,
                    final_num_ctx = checkpoint.final_num_ctx,
                    "worker exploration boundary reached — entering bounded finalization"
                );
                if let Ok(mut se) = stderr_clone.lock() {
                    let reason = match worker_boundary {
                        WorkerExplorationBoundary::ContextPressure(estimated) => format!(
                            "estimated prompt pressure ({estimated}/{ctx_cap} tokens, {}%)",
                            worker_policy.context_pressure_percent
                        ),
                        WorkerExplorationBoundary::ObservationLimit => format!(
                            "{explored_without_progress} successful repository observations without a workspace mutation"
                        ),
                        WorkerExplorationBoundary::RoundLimit => format!(
                            "the {}-round exploration limit",
                            worker_policy.max_iterations
                        ),
                    };
                    se.push(format!(
                        "{backend} worker entered bounded finalization after {reason}; \
                         {WORKER_FINALIZATION_ITERATIONS} rounds remain; context checkpoint \
                         {}→{} messages (seed {}, compatible recent tail {}, compacted tool results {}), \
                         approximately {}→{} tokens, final num_ctx {}",
                        checkpoint.before_messages,
                        checkpoint.after_messages,
                        checkpoint.seed_messages,
                        checkpoint.tail_messages,
                        checkpoint.compacted_tool_results,
                        checkpoint.before_tokens,
                        checkpoint.after_tokens,
                        checkpoint.final_num_ctx,
                    ));
                }
            }

            if let Some(messages) = body["messages"].as_array_mut() {
                if let Some(prompt) = worker_repair_prompt.take() {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": prompt,
                    }));
                } else if entered_worker_delivery {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": format!(
                            "The Git commit succeeded. A commit is not a Kronn delivery. \
                             Submit the required DeliveryManifest v1 now with \
                             `task_exec_deliver`; it is the only available tool. \
                             You have {WORKER_DELIVERY_ITERATIONS} bounded delivery-only attempts."
                        ),
                    }));
                } else if entered_worker_finalization {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": format!(
                            "Kronn's bounded exploration window is complete. \
                             Your evidence is preserved. \
                             Finalize within {WORKER_FINALIZATION_ITERATIONS} bounded rounds: \
                             refresh the exact target with `read_file` if its CAS receipt is \
                             missing or stale (at most {WORKER_FINALIZATION_READ_FILE_CALLS} \
                             exact refreshes remain), then edit, use at most \
                             {WORKER_FINALIZATION_GIT_INSPECTION_CALLS} combined \
                             `git_status`/`git_diff` inspections after each successful edit, \
                             `git_commit`, and `task_exec_deliver`. Broad discovery tools are \
                             no longer available. If the evidence is still insufficient, stop \
                             circling and state the exact blocker plainly."
                        ),
                    }));
                }
            }

            if delivered_this_turn {
                // The durable manifest is the worker's terminal result. Do not
                // pay for another full MLX re-prefill merely to ask the model
                // to paraphrase a successful protocol action. Keep one small,
                // non-model acknowledgement so child-run accounting does not
                // misclassify this successful worker as an empty response.
                let _ = tx
                    .send("DeliveryManifest submitted to Kronn.".to_string())
                    .await;
                break true;
            }

            let error_only_round = !results.is_empty() && results.iter().all(|result| !result.ok);
            if entered_worker_repair {
                // The failed edit armed a new, non-renewable recovery path.
                // Do not let earlier error-only rounds force synthesis before
                // its first read request is sent.
                consecutive_error_only_rounds = 0;
            } else if error_only_round {
                consecutive_error_only_rounds += 1;
            } else {
                consecutive_error_only_rounds = 0;
            }
            let withdrawn: std::collections::HashSet<String> = open_tool_circuits
                .union(&withdrawn_tools)
                .cloned()
                .collect();
            if !withdrawn.is_empty() {
                remove_tool_declarations(&mut body, &withdrawn);
            }

            // Every call this turn was refused for budget: the model is asking for
            // a tool it can no longer have. Telling it so is not enough — it just
            // asks again, and did, eleven times over. Drop the declarations so
            // writing the answer is the only move left, same as the
            // "you used tools but wrote nothing" retry above.
            let error_loop_exhausted = consecutive_error_only_rounds >= MAX_ERROR_ONLY_TOOL_ROUNDS;
            // Withdrawing the offending declarations above is what actually stops
            // the loop. Forcing prose on top of it is only right once there is
            // nothing left to call: a worker that overspent ONE budget still has
            // a change to write and a commit to make. When the offending tool was
            // the only one, the catalogue empties and this fires exactly as before.
            let nothing_left_to_call = body.get("tools").is_none();
            if (budget_refusals > 0 && budget_refusals == calls.len() && nothing_left_to_call)
                || error_loop_exhausted
            {
                if let Some(map) = body.as_object_mut() {
                    map.remove("tools");
                }
                let diagnostic = tool_convergence_diagnostic(
                    &calls_per_tool,
                    &errors_per_tool,
                    &refusals_per_tool,
                );
                if !forced_synthesis {
                    if let Some(messages) = body["messages"].as_array_mut() {
                        messages.push(serde_json::json!({
                            "role": "user",
                            "content": format!(
                                "Kronn stopped a non-progressing tool loop: {diagnostic}. \
                                 Do not call any more tools. Answer now with the useful \
                                 evidence already obtained, clearly separate confirmed \
                                 facts from missing information, and mention the failed \
                                 tools only once in a concise limitation note."
                            ),
                        }));
                    }
                    forced_synthesis = true;
                }
                tracing::info!(
                    target: "kronn::agent::tools",
                    turn,
                    refused = budget_refusals,
                    consecutive_error_only_rounds,
                    diagnostic = %diagnostic,
                    "tool convergence forced — withdrawing tools and requesting synthesis"
                );
                if let Ok(mut se) = stderr_clone.lock() {
                    se.push(format!(
                        "{backend} forced tool convergence after {consecutive_error_only_rounds} error-only rounds: {diagnostic}; useful_results={useful_tool_results}"
                    ));
                }
            }
            // The messages just grew by the tool results: the window sized for turn
            // one no longer fits them. OpenAI-wire providers own their own window.
            if !is_openai_wire {
                clamp_ollama_tool_results(&mut body, ctx_cap);
                resize_ollama_num_ctx(&mut body, ctx_cap);
            }
            if worker_delivery_phase && worker_delivery_turns >= WORKER_DELIVERY_ITERATIONS {
                let msg = format!(
                    "{backend} worker committed but did not submit its DeliveryManifest after \
                     {WORKER_DELIVERY_ITERATIONS} delivery-only rounds — giving up"
                );
                tracing::warn!(target: "kronn::agent::tools", "{}", msg);
                if let Ok(mut se) = stderr_clone.lock() {
                    se.push(msg);
                }
                break false;
            }
            let repair_iteration_limit = worker_repair_iteration_limit(
                worker_repair_stage,
                worker_strict_syntax_repair,
                prelocalized_worker,
            );
            if worker_repair_stage != WorkerRepairStage::Inactive
                && worker_repair_turns >= repair_iteration_limit
            {
                let reason_code =
                    worker_repair_terminal_reason_code(worker_repair_stage, prelocalized_worker);
                let msg = format!(
                    "{backend} worker exhausted its bounded {} phase after {} rounds; reason_code={reason_code}",
                    worker_repair_stage.label(),
                    repair_iteration_limit
                );
                tracing::warn!(target: "kronn::agent::tools", "{}", msg);
                if let Ok(mut se) = stderr_clone.lock() {
                    se.push(msg);
                }
                break false;
            }
            if worker_finalization_phase
                && !worker_delivery_phase
                && worker_repair_stage == WorkerRepairStage::Inactive
                && worker_finalization_turns >= WORKER_FINALIZATION_ITERATIONS
            {
                let msg = format!(
                    "{backend} worker did not finalize after its bounded exploration phase \
                     and {WORKER_FINALIZATION_ITERATIONS} finalization rounds — giving up"
                );
                tracing::warn!(target: "kronn::agent::tools", "{}", msg);
                if let Ok(mut se) = stderr_clone.lock() {
                    se.push(msg);
                }
                break false;
            }
            request_started_at = std::time::Instant::now();
            let next_turn = tokio::select! {
                biased;
                _ = task_cancel.cancelled() => {
                    finish(&mut lifeline, false).await;
                    return;
                }
                result = send_http_agent_request(
                    &client,
                    &url,
                    &body,
                    auth_key.as_deref(),
                    backend,
                    1,
                    1,
                    false,
                    &stderr_clone,
                ) => result,
            };
            response = match next_turn {
                Ok((response, _)) => {
                    provider_attempt = 1;
                    response
                }
                Err(failure) if failure.status.is_some() => {
                    let status = failure.status.expect("guarded above");
                    let provider_body = failure.detail;
                    // `no user query found in messages` is not a tool-calling
                    // problem: Ollama dropped the whole history, user turn
                    // included, to fit a window too small for the tool results.
                    let msg = if provider_body.contains("no user query found") {
                        format!(
                            "{backend} error {status} on tool round-trip {turn}: the prompt outgrew the context window, so the provider truncated the history until the user turn itself was gone. Reduce what the tools return, or raise KRONN_OLLAMA_NUM_CTX_CAP. Provider response: {}",
                            provider_body.trim()
                        )
                    } else {
                        format!(
                            "{backend} error {status} on tool round-trip {turn}. The provider accepted the initial tool declaration but rejected the follow-up; verify that this route/model supports native tool calling. Automatic retry was skipped because a tool had already executed. Provider response: {}",
                            provider_body.trim()
                        )
                    };
                    tracing::warn!(target: "kronn::agent::tools", "{}", msg);
                    if let Ok(mut se) = stderr_clone.lock() {
                        se.push(msg);
                    }
                    break false;
                }
                Err(failure) => {
                    let msg = format!(
                        "{backend} unreachable on tool round-trip {turn}: {}. Automatic retry was skipped because a tool had already executed",
                        failure.detail
                    );
                    tracing::warn!(target: "kronn::agent::tools", "{}", msg);
                    if let Ok(mut se) = stderr_clone.lock() {
                        se.push(msg);
                    }
                    break false;
                }
            };
        };

        finish(&mut lifeline, ok).await;
    });

    Ok(AgentProcess {
        child: dummy_child,
        output_mode: OutputMode::Text,
        work_dir: std::env::temp_dir(),
        agent_type: agent_type.clone(),
        rx,
        stderr_capture,
        stderr_task: None,
        http_cancel: Some(http_cancel),
        pgid: None,
    })
}

/// MCP context is injected via --append-system-prompt for Claude Code,
/// or prepended to the prompt for other agents.
/// Returns: (binary, npx_package, args, env_key, stderr_mode, output_mode)
/// Build the complete Claude sandbox policy for one task worktree.
///
/// Keep this invocation-local and bounded regardless of how many unrelated
/// worktrees exist on the host. The current task worktree is the only explicit
/// write root; sandbox availability and unsandboxed fallback remain fail-closed.
fn claude_task_worker_settings(work_dir: &Path) -> Result<String, String> {
    let work_dir = work_dir.canonicalize().map_err(|error| {
        format!(
            "Claude task worker cannot start: unable to canonicalize managed worktree {}: {error}",
            work_dir.display()
        )
    })?;
    serde_json::to_string(&serde_json::json!({
        "sandbox": {
            "enabled": true,
            "failIfUnavailable": true,
            "autoAllowBashIfSandboxed": true,
            "allowUnsandboxedCommands": false,
            "excludedCommands": [],
            "filesystem": { "allowWrite": [work_dir] }
        }
    }))
    .map_err(|error| format!("Unable to encode the Claude task-worker sandbox policy: {error}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeTaskWorkerAuthStatus {
    LoggedIn,
    LoggedOut,
    Malformed,
}

fn parse_claude_task_worker_auth_status(
    stdout: &[u8],
    exit_success: bool,
) -> ClaudeTaskWorkerAuthStatus {
    let logged_in = serde_json::from_slice::<serde_json::Value>(stdout)
        .ok()
        .and_then(|status| status.get("loggedIn").and_then(serde_json::Value::as_bool));
    match (logged_in, exit_success) {
        (Some(true), true) => ClaudeTaskWorkerAuthStatus::LoggedIn,
        (Some(false), _) => ClaudeTaskWorkerAuthStatus::LoggedOut,
        _ => ClaudeTaskWorkerAuthStatus::Malformed,
    }
}

fn claude_task_worker_auth_result(stdout: &[u8], exit_success: bool) -> Result<(), String> {
    match parse_claude_task_worker_auth_status(stdout, exit_success) {
        ClaudeTaskWorkerAuthStatus::LoggedIn => Ok(()),
        ClaudeTaskWorkerAuthStatus::LoggedOut => Err(
            "Claude task worker cannot start: `claude auth status` reports `loggedIn=false`. \
             Run `claude auth login`, or use `task_exec_reassign` to move this execution to \
             another available worker."
                .into(),
        ),
        ClaudeTaskWorkerAuthStatus::Malformed => Err(
            "Claude task worker cannot start: `claude auth status` returned an unrecognized \
             response. Verify the local Claude CLI with `claude auth status`; if it remains \
             unavailable, use `task_exec_reassign` to move this execution to another \
             available worker."
                .into(),
        ),
    }
}

async fn run_claude_task_worker_auth_probe(
    resolved: (String, Vec<String>, bool),
    work_dir: &Path,
) -> std::io::Result<std::process::Output> {
    let (command, args, via_wsl) = resolved;
    let (command, args, effective_work_dir) =
        platform_agent_invocation(command, args, via_wsl, work_dir);
    async_cmd(command)
        .args(args)
        .current_dir(effective_work_dir)
        .stdin(Stdio::null())
        .output()
        .await
}

async fn probe_claude_task_worker_auth(
    binary: &str,
    npx_package: Option<&str>,
    work_dir: &Path,
) -> Result<(), String> {
    let auth_args = vec!["auth".to_string(), "status".to_string()];
    let direct = resolve_agent_invocation(binary, None, &auth_args);
    let output = match direct {
        Ok(resolved) => match run_claude_task_worker_auth_probe(resolved, work_dir).await {
            Ok(output) => output,
            Err(direct_error) => {
                let Some(package) = npx_package else {
                    return Err(claude_task_worker_auth_spawn_diagnostic(&direct_error));
                };
                let fallback = resolve_agent_invocation(binary, Some(package), &auth_args)
                    .map_err(|_| claude_task_worker_auth_spawn_diagnostic(&direct_error))?;
                run_claude_task_worker_auth_probe(fallback, work_dir)
                    .await
                    .map_err(|error| claude_task_worker_auth_spawn_diagnostic(&error))?
            }
        },
        Err(direct_error) => {
            let Some(package) = npx_package else {
                return Err(format!(
                    "{direct_error}. Use `task_exec_reassign` to move this execution to another \
                     available worker."
                ));
            };
            let fallback =
                resolve_agent_invocation(binary, Some(package), &auth_args).map_err(|error| {
                    format!(
                        "{error}. Use `task_exec_reassign` to move this execution to another \
                         available worker."
                    )
                })?;
            run_claude_task_worker_auth_probe(fallback, work_dir)
                .await
                .map_err(|error| claude_task_worker_auth_spawn_diagnostic(&error))?
        }
    };

    claude_task_worker_auth_result(&output.stdout, output.status.success())
}

fn claude_task_worker_auth_spawn_diagnostic(error: &std::io::Error) -> String {
    format!(
        "Claude task worker cannot start: the `claude auth status` preflight could not run \
         ({error}). Use `task_exec_reassign` to move this execution to another available worker."
    )
}

fn claude_task_worker_mcp_config(project_root: &Path) -> Result<String, String> {
    let source = project_root.join(".mcp.json");
    let raw = std::fs::read_to_string(&source).map_err(|error| {
        format!(
            "Claude task worker cannot start: unable to read authoritative MCP config {}: {error}",
            source.display()
        )
    })?;
    let config: serde_json::Value = serde_json::from_str(&raw).map_err(|error| {
        format!(
            "Claude task worker cannot start: invalid authoritative MCP config {}: {error}",
            source.display()
        )
    })?;
    let mut internal = config
        .get("mcpServers")
        .and_then(serde_json::Value::as_object)
        .and_then(|servers| servers.get("kronn-internal"))
        .cloned()
        .ok_or_else(|| {
            format!(
                "Claude task worker cannot start: {} does not define the required `kronn-internal` MCP server",
                source.display()
            )
        })?;
    let internal_object = internal.as_object_mut().ok_or_else(|| {
        format!(
            "Claude task worker cannot start: `kronn-internal` in {} is not an object",
            source.display()
        )
    })?;
    // Claude Code does not forward session-level variables to stdio MCP
    // children. Expand the capability from the Claude process environment into
    // this one server explicitly. The inline config contains placeholders, not
    // their values, so the opaque delivery capability never appears in argv.
    // Replace (rather than merge) the source env to keep the worker bridge
    // fail-closed and limited to the four values it actually needs.
    internal_object.insert(
        "env".into(),
        serde_json::json!({
            "KRONN_TASK_WORKER_CONTEXT": "${KRONN_TASK_WORKER_CONTEXT}",
            "KRONN_DISCUSSION_ID": "${KRONN_DISCUSSION_ID}",
            "KRONN_BACKEND_URL": "${KRONN_BACKEND_URL:-http://127.0.0.1:3140}",
            "KRONN_AUTH_TOKEN": "${KRONN_AUTH_TOKEN:-}",
        }),
    );
    serde_json::to_string(&serde_json::json!({
        "mcpServers": {"kronn-internal": internal}
    }))
    .map_err(|error| format!("Unable to encode the Claude task-worker MCP config: {error}"))
}

fn insert_claude_mcp_config(args: &mut Vec<String>, config: String, strict: bool) {
    // The user prompt is always the last positional argument. MCP config must
    // precede --append-system-prompt when present, otherwise it can be consumed
    // as the system-prompt value. Locate the flag explicitly: inspecting the
    // previous value is unsafe now that worker policy adds other value options.
    let prompt_index = args.len().saturating_sub(1);
    let insert_at = args
        .iter()
        .position(|arg| arg == "--append-system-prompt")
        .unwrap_or(prompt_index);
    if strict {
        args.insert(insert_at, "--strict-mcp-config".into());
    }
    let mcp_index = insert_at + usize::from(strict);
    args.insert(mcp_index, "--mcp-config".into());
    args.insert(mcp_index + 1, config);
}

#[cfg(test)]
fn agent_command(
    agent_type: &AgentType,
    prompt: &str,
    full_access: bool,
    mcp_context: &str,
    model_flag: Option<&str>,
) -> (
    &'static str,
    Option<&'static str>,
    Vec<String>,
    &'static str,
    StderrMode,
    OutputMode,
) {
    agent_command_with_task_worker_policy(
        agent_type,
        prompt,
        full_access,
        mcp_context,
        model_flag,
        false,
        None,
    )
}

/// Build a provider command with the stricter policy required by a spawned
/// task worker. A worktree is an ownership boundary, not a sandbox by itself:
/// every CLI that offers a global bypass must ignore it for worker runs.
fn agent_command_with_task_worker_policy(
    agent_type: &AgentType,
    prompt: &str,
    full_access: bool,
    mcp_context: &str,
    model_flag: Option<&str>,
    task_worker: bool,
    task_work_dir: Option<&Path>,
) -> (
    &'static str,
    Option<&'static str>,
    Vec<String>,
    &'static str,
    StderrMode,
    OutputMode,
) {
    match agent_type {
        AgentType::ClaudeCode => {
            let mut args = vec![
                "--print".into(),
                "--output-format".into(),
                "stream-json".into(),
                "--verbose".into(),
                "--include-partial-messages".into(),
            ];
            if let Some(model) = model_flag {
                args.push("--model".into());
                args.push(model.into());
            }
            if task_worker {
                // Ignore user/project settings so a previously configured
                // allowWrite or excluded command cannot silently widen this
                // worker's boundary. The explicit settings make sandbox
                // availability a hard gate and remove Claude's unsandboxed
                // escape hatch. `acceptEdits` keeps normal edits inside CWD
                // autonomous; attempts outside CWD remain permission-gated.
                args.push("--setting-sources".into());
                args.push(String::new());
                args.push("--settings".into());
                let settings = task_work_dir
                    .and_then(|path| claude_task_worker_settings(path).ok())
                    .unwrap_or_else(|| {
                        // Production task-worker launches always provide the
                        // managed worktree. This invalid path keeps direct
                        // builder calls fail-closed and deterministic.
                        r#"{"sandbox":{"enabled":true,"failIfUnavailable":true,"autoAllowBashIfSandboxed":true,"allowUnsandboxedCommands":false,"excludedCommands":[],"filesystem":{"allowWrite":[]}}}"#.into()
                    });
                args.push(settings);
                // Task workers need repository I/O, not nested agents,
                // browsers, worktree creation or arbitrary MCP capabilities.
                args.push("--tools".into());
                args.push("Bash,Edit,Read,Write,Glob,Grep".into());
                args.push("--allowedTools".into());
                args.push("mcp__kronn-internal__task_exec_commit".into());
                args.push("mcp__kronn-internal__task_exec_deliver".into());
                args.push("--permission-mode".into());
                args.push("acceptEdits".into());
            } else if full_access {
                args.push("--dangerously-skip-permissions".into());
            }
            // Inject MCP context via --append-system-prompt (separate from user prompt)
            if !mcp_context.is_empty() {
                args.push("--append-system-prompt".into());
                args.push(mcp_context.into());
            }
            args.push(prompt.into());
            (
                "claude",
                Some("@anthropic-ai/claude-code"),
                args,
                "ANTHROPIC_API_KEY",
                StderrMode::StdoutOnly,
                OutputMode::StreamJson,
            )
        }
        AgentType::Codex => {
            let mut args: Vec<String> = vec!["exec".into()];
            if let Some(model) = model_flag {
                args.push("--model".into());
                args.push(model.into());
            }
            // Codex does not inherit arbitrary parent env into stdio MCP
            // children. Pin the same narrow allowlist per invocation so the
            // current discussion/task capability cannot depend on a later
            // global config sync and concurrent workers stay isolated.
            args.push("-c".into());
            args.push(if task_worker {
                // `start_agent_with_config` validates availability before this
                // builder is reached. The fallback keeps direct unit calls
                // deterministic without ever broadening the worker surface.
                codex_task_worker_mcp_override().unwrap_or_else(|| "mcp_servers={}".into())
            } else {
                codex_kronn_internal_env_override()
            });
            // Codex requires a trusted git directory by default.
            // Inside Docker the paths are mapped, so skip the check.
            args.push("--skip-git-repo-check".into());
            // In Docker, container paths don't match host trusted paths,
            // causing "Permission denied" on CWD listing with default sandbox.
            // On macOS Docker, workspace-write can block shell/apply_patch writes
            // despite rw mounts; prefer danger-full-access there.
            if task_worker {
                // The command-line sandbox is authoritative for a worker.
                // Ignore user config/rules so neither added write roots nor an
                // exec-policy can broaden the managed worktree. Git commit is
                // mediated by Kronn; shared objects/refs stay outside this
                // process. Auth is deliberately still read from CODEX_HOME.
                args.push("--ignore-user-config".into());
                args.push("--ignore-rules".into());
                args.push("--sandbox=workspace-write".into());
            } else if std::env::var("KRONN_HOST_HOME").is_ok() || full_access {
                // Inside the Docker container, Codex's bwrap sandbox can NEVER
                // initialize: unprivileged user namespaces are blocked
                // (`bwrap: No permissions to create new namespace`, verified
                // 2026-06-13 — run-9's plan review couldn't read ANY file and
                // emitted a false NEEDS_RETRIAGE). workspace-write is therefore
                // structurally broken in Docker on every OS, not just macOS;
                // the container + git worktree ARE the isolation boundary.
                // On native installs, honour the user's explicit full-access
                // setting instead of silently leaving Codex in its default
                // read-only sandbox.
                args.push("--sandbox=danger-full-access".into());
            }
            // Codex has no system prompt flag — prepend context to the prompt
            let full_prompt = if mcp_context.is_empty() {
                prompt.into()
            } else {
                format!("{}\n\n{}", mcp_context, prompt)
            };
            args.push(full_prompt);
            (
                "codex",
                Some("@openai/codex"),
                args,
                "OPENAI_API_KEY",
                StderrMode::StdoutOnly,
                OutputMode::Text,
            )
        }
        AgentType::Vibe => {
            // Vibe CLI hangs: get_prompt_from_stdin() blocks on sys.stdin.read()
            // when stdin is not a tty, and 429 rate limits cause infinite hangs.
            // vibe-runner.py bypasses the CLI and calls run_programmatic() directly,
            // giving a real agent (bash, file I/O, grep, etc. + MCP if configured).
            // Falls back to direct Mistral API streaming if vibe is not installed.
            let full_prompt = if mcp_context.is_empty() {
                prompt.into()
            } else {
                format!("{}\n\n{}", mcp_context, prompt)
            };
            let runner_script = vibe_runner_path();
            let mut args = vec![runner_script];
            if let Some(model) = model_flag {
                args.push("--model".into());
                args.push(model.into());
            }
            args.push("--max-turns".into());
            args.push("30".into());
            args.push(full_prompt);
            (
                "python3",
                None,
                args,
                "MISTRAL_API_KEY",
                StderrMode::StdoutOnly,
                OutputMode::Text,
            )
        }
        AgentType::GeminiCli => {
            // Gemini CLI requires -p <prompt> as the LAST args.
            // Options (--model, --yolo) must come BEFORE -p, otherwise
            // Gemini interprets them as the prompt value and fails.
            let mut args: Vec<String> = Vec::new();
            if let Some(model) = model_flag {
                args.push("--model".into());
                args.push(model.into());
            }
            if full_access && !task_worker {
                args.push("--yolo".into());
            }
            // Gemini CLI has no system prompt flag — prepend context to prompt
            let full_prompt = if mcp_context.is_empty() {
                prompt.into()
            } else {
                format!("{}\n\n{}", mcp_context, prompt)
            };
            args.push("-p".into());
            args.push(full_prompt);
            (
                "gemini",
                Some("@google/gemini-cli"),
                args,
                "GEMINI_API_KEY",
                StderrMode::StdoutOnly,
                OutputMode::Text,
            )
        }
        AgentType::Kiro => {
            // --trust-all-tools is REQUIRED in --no-interactive mode,
            // otherwise Kiro blocks waiting for tool confirmation that never comes.
            let mut args: Vec<String> = vec!["chat".into(), "--no-interactive".into()];
            if !task_worker {
                args.push("--trust-all-tools".into());
            }
            args.push("--wrap".into());
            args.push("never".into());
            let _ = full_access; // Kiro has no narrower full-access flag.
            let full_prompt = if mcp_context.is_empty() {
                prompt.into()
            } else {
                format!("{}\n\n{}", mcp_context, prompt)
            };
            args.push(full_prompt);
            (
                "kiro-cli",
                None, // No npx package
                args,
                "AWS_BUILDER_ID", // Not really used, but placeholder
                StderrMode::StdoutOnly,
                OutputMode::Text,
            )
        }
        AgentType::CopilotCli => {
            let mut args: Vec<String> = Vec::new();
            if let Some(model) = model_flag {
                args.push("--model".into());
                args.push(model.into());
            }
            if full_access && !task_worker {
                args.push("--allow-all-tools".into());
            }
            // Copilot has no system prompt flag — prepend context to prompt
            let full_prompt = if mcp_context.is_empty() {
                prompt.into()
            } else {
                format!("{}\n\n{}", mcp_context, prompt)
            };
            // Keep `-p <prompt>` adjacent and last. Copilot consumes the
            // argument immediately after `-p` as the prompt; putting options
            // between them makes the CLI reject the real prompt as extra
            // unquoted words.
            args.push("-p".into());
            args.push(full_prompt);
            (
                "copilot",
                Some("@github/copilot"),
                args,
                "GH_TOKEN",
                StderrMode::StdoutOnly,
                OutputMode::Text,
            )
        }
        AgentType::Ollama => {
            // Ollama: local LLM inference via `ollama run <model> <prompt>`
            let model = model_flag.unwrap_or("qwen3:8b");
            let full_prompt = if mcp_context.is_empty() {
                prompt.into()
            } else {
                format!("{}\n\n{}", mcp_context, prompt)
            };
            let args = vec![
                "run".into(),
                "--nowordwrap".into(),
                model.into(),
                full_prompt,
            ];
            (
                "ollama",
                None,
                args,
                "OLLAMA_HOST",
                StderrMode::StdoutOnly,
                OutputMode::Text,
            )
        }
        // Unreachable in practice: `start_agent_with_config` returns via the
        // HTTP path before building a command line. Kept explicit (rather than
        // folded into a catch-all) so a future CLI-mode LiteLLM has to make a
        // deliberate decision here instead of silently inheriting `echo`.
        AgentType::LiteLlm => (
            "echo",
            None,
            vec!["LiteLLM runs over HTTP, not as a CLI process".into()],
            "NONE",
            StderrMode::Merge,
            OutputMode::Text,
        ),
        AgentType::Nvidia => (
            "echo",
            None,
            vec!["NVIDIA runs over HTTP, not as a CLI process".into()],
            "NONE",
            StderrMode::Merge,
            OutputMode::Text,
        ),
        AgentType::Custom => (
            "echo",
            None,
            vec!["Custom agent not configured".into()],
            "NONE",
            StderrMode::Merge,
            OutputMode::Text,
        ),
    }
}

/// Linux `ARG_MAX` per-argument limit. Above ~128 KiB a single argv element
/// causes `execve` to return `E2BIG` ("Argument list too long"). Bump of
/// `PAGE_SIZE * 32` picks the 128 KiB figure with a conservative margin.
/// Seen in the wild (EW-7189 analysis): large MCP + skills + prompt combined
/// pushed past the limit, spawn_failed with `os error 7`.
pub(crate) const MAX_SINGLE_ARG_BYTES: usize = 100 * 1024;
const CLAUDE_SYSTEM_PROMPT_TRUNCATION_MARKER: &str =
    "\n\n[... system prompt truncated by Kronn to fit ARG_MAX ...]";

fn truncate_claude_system_prompt_argument(args: &mut [String]) -> Option<(usize, usize)> {
    let index = args
        .iter()
        .position(|argument| argument == "--append-system-prompt")?;
    let value = args.get_mut(index + 1)?;
    let original_bytes = value.len();
    if original_bytes <= MAX_SINGLE_ARG_BYTES {
        return None;
    }

    let mut cut = MAX_SINGLE_ARG_BYTES.saturating_sub(CLAUDE_SYSTEM_PROMPT_TRUNCATION_MARKER.len());
    while cut > 0 && !value.is_char_boundary(cut) {
        cut -= 1;
    }
    value.truncate(cut);
    value.push_str(CLAUDE_SYSTEM_PROMPT_TRUNCATION_MARKER);
    Some((original_bytes, value.len()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InvocationSizeReceipt {
    argv_payload_bytes: usize,
    environment_payload_bytes: usize,
    argument_count: usize,
    max_argument_bytes: usize,
    environment_entry_count: usize,
    max_environment_entry_bytes: usize,
    settings_bytes: usize,
    mcp_config_bytes: usize,
    system_prompt_bytes: usize,
    stdin_bytes: usize,
}

impl InvocationSizeReceipt {
    fn compact(self) -> String {
        format!(
            "argv_payload_bytes={}, environment_payload_bytes={}, argument_count={}, \
             max_argument_bytes={}, environment_entry_count={}, \
             max_environment_entry_bytes={}, settings_bytes={}, mcp_config_bytes={}, \
             system_prompt_bytes={}, stdin_bytes={}",
            self.argv_payload_bytes,
            self.environment_payload_bytes,
            self.argument_count,
            self.max_argument_bytes,
            self.environment_entry_count,
            self.max_environment_entry_bytes,
            self.settings_bytes,
            self.mcp_config_bytes,
            self.system_prompt_bytes,
            self.stdin_bytes,
        )
    }

    fn validate_single_argument_limit(self) -> Result<(), String> {
        if self.max_argument_bytes <= MAX_SINGLE_ARG_BYTES {
            return Ok(());
        }

        let oversized_components: Vec<&str> = [
            ("settings_bytes", self.settings_bytes),
            ("mcp_config_bytes", self.mcp_config_bytes),
            ("system_prompt_bytes", self.system_prompt_bytes),
        ]
        .into_iter()
        .filter_map(|(name, bytes)| (bytes > MAX_SINGLE_ARG_BYTES).then_some(name))
        .collect();
        let oversized_components = if oversized_components.is_empty() {
            "other_argument_bytes".to_string()
        } else {
            oversized_components.join(",")
        };

        Err(format!(
            "Claude task worker invocation refused before spawn: max_argument_bytes={} exceeds \
             max_single_arg_bytes={MAX_SINGLE_ARG_BYTES}; oversized_components={oversized_components}; \
             settings_bytes={}, mcp_config_bytes={}, system_prompt_bytes={}. No argument content \
             was logged. Use `task_exec_reassign` to move this execution to another available worker.",
            self.max_argument_bytes,
            self.settings_bytes,
            self.mcp_config_bytes,
            self.system_prompt_bytes,
        ))
    }
}

#[cfg(unix)]
fn os_str_bytes(value: &std::ffi::OsStr) -> usize {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes().len()
}

#[cfg(windows)]
fn os_str_bytes(value: &std::ffi::OsStr) -> usize {
    use std::os::windows::ffi::OsStrExt;
    value.encode_wide().count() * std::mem::size_of::<u16>()
}

fn invocation_size_receipt(
    program: &std::ffi::OsStr,
    args: &[&std::ffi::OsStr],
    environment: &[(&std::ffi::OsStr, &std::ffi::OsStr)],
    stdin_bytes: usize,
) -> InvocationSizeReceipt {
    let program_bytes = os_str_bytes(program);
    let argument_lengths: Vec<usize> = std::iter::once(program_bytes)
        .chain(args.iter().map(|arg| os_str_bytes(arg)))
        .collect();
    let environment_lengths: Vec<usize> = environment
        .iter()
        .map(|(key, value)| os_str_bytes(key) + 1 + os_str_bytes(value))
        .collect();
    let value_after = |flag: &str| {
        args.windows(2)
            .find(|pair| pair[0] == std::ffi::OsStr::new(flag))
            .map_or(0, |pair| os_str_bytes(pair[1]))
    };

    InvocationSizeReceipt {
        argv_payload_bytes: argument_lengths.iter().map(|bytes| bytes + 1).sum(),
        environment_payload_bytes: environment_lengths.iter().map(|bytes| bytes + 1).sum(),
        argument_count: argument_lengths.len(),
        max_argument_bytes: argument_lengths.into_iter().max().unwrap_or(0),
        environment_entry_count: environment_lengths.len(),
        max_environment_entry_bytes: environment_lengths.into_iter().max().unwrap_or(0),
        settings_bytes: value_after("--settings"),
        mcp_config_bytes: value_after("--mcp-config"),
        system_prompt_bytes: value_after("--append-system-prompt"),
        stdin_bytes,
    }
}

fn command_invocation_size_receipt(
    command: &tokio::process::Command,
    stdin_payload: Option<&str>,
) -> InvocationSizeReceipt {
    let command = command.as_std();
    let mut environment: std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString> =
        std::env::vars_os().collect();
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            environment.insert(key.to_os_string(), value.to_os_string());
        } else {
            environment.remove(key);
        }
    }
    let args: Vec<&std::ffi::OsStr> = command.get_args().collect();
    let environment: Vec<(&std::ffi::OsStr, &std::ffi::OsStr)> = environment
        .iter()
        .map(|(key, value)| (key.as_os_str(), value.as_os_str()))
        .collect();
    invocation_size_receipt(
        command.get_program(),
        &args,
        &environment,
        stdin_payload.map_or(0, str::len),
    )
}

fn resolve_agent_invocation(
    binary: &str,
    npx_package: Option<&str>,
    args: &[String],
) -> Result<(String, Vec<String>, bool), String> {
    if let Some(package) = npx_package {
        let mut npx_args = vec!["--yes".to_string(), package.to_string()];
        npx_args.extend_from_slice(args);
        let via_wsl = super::find_binary("npx")
            .map(|location| location.via_wsl)
            .unwrap_or(false);
        Ok(("npx".to_string(), npx_args, via_wsl))
    } else {
        let location =
            super::find_binary(binary).ok_or_else(|| format!("Binary '{binary}' not found"))?;
        Ok((location.path, args.to_vec(), location.via_wsl))
    }
}

fn platform_agent_invocation(
    command: String,
    args: Vec<String>,
    resolved_via_wsl: bool,
    work_dir: &Path,
) -> (String, Vec<String>, PathBuf) {
    #[cfg(target_os = "windows")]
    let use_wsl = !is_wsl() && (resolved_via_wsl || command.starts_with('/'));
    #[cfg(not(target_os = "windows"))]
    {
        let _ = resolved_via_wsl;
    }
    #[cfg(not(target_os = "windows"))]
    let use_wsl = false;

    if use_wsl {
        #[cfg(target_os = "windows")]
        let wsl_work_dir = windows_to_wsl_path(work_dir);
        #[cfg(not(target_os = "windows"))]
        let wsl_work_dir = work_dir.to_path_buf();

        let mut wsl_args = vec![
            "--cd".to_string(),
            wsl_work_dir.display().to_string(),
            "-e".to_string(),
            command,
        ];
        wsl_args.extend(args);
        ("wsl.exe".to_string(), wsl_args, work_dir.to_path_buf())
    } else {
        (command, args, work_dir.to_path_buf())
    }
}

/// True iff the agent's `HOME` should NOT be overridden to
/// `KRONN_HOST_HOME` before spawn. Pure decision so we can test the
/// policy table without mounting filesystem state.
///
/// Rationale: every Kronn-managed CLI agent has its config dir
/// mounted at `/home/kronn/<agent>` by docker-compose, so `HOME` is
/// already correct as-is in the container. The override would
/// misdirect them to `/home/<host-user>/<agent>` which doesn't exist
/// in the container — silent hang waiting for a missing token
/// (cf. TD-20260507-home-override-breaks-claude-creds; reported on
/// WSL2 + Claude in issue #81 bug 5, generalised to all CLI agents
/// after code-level audit).
///
/// Unknown binaries keep the override — they may legitimately need
/// a host-rooted HOME (arbitrary user-installed tools).
pub(crate) fn should_skip_home_override(binary: &str, npx_package: Option<&str>) -> bool {
    matches!(
        binary,
        "claude" | "codex" | "vibe" | "gemini" | "kiro-cli" | "copilot"
    ) || matches!(
        npx_package,
        Some("@anthropic-ai/claude-code")
            | Some("@openai/codex")
            | Some("@google/gemini-cli")
            | Some("@github/copilot"),
    )
}

/// Spawn an agent process. If npx_package is Some, uses npx to run.
///
/// `stdin_payload`: when present, the string is written to the child's stdin
/// and stdin is then closed (EOF). Used for agents that accept their prompt
/// via stdin (currently: Claude Code with `--print` and no positional prompt
/// arg), letting us side-step the kernel's per-argv size cap.
///
/// 9 args: each is genuinely independent — bundling them into a config
/// struct would just shuffle the verbosity from the call sites
/// (already only 2) into the struct definition + builder. The
/// arguments map 1:1 to the spawn primitives the OS expects (binary,
/// args, env, cwd, stdin) plus four Kronn-specific feed-throughs
/// (npx_package, api_key, discussion_id, task-worker capability). Allow-listed rather than
/// refactored.
#[allow(clippy::too_many_arguments)]
fn try_spawn(
    binary: &str,
    npx_package: Option<&str>,
    args: &[String],
    work_dir: &Path,
    env_key: &str,
    api_key: Option<&str>,
    stdin_payload: Option<&str>,
    discussion_id: Option<&str>,
    task_worker_context: Option<&TaskWorkerBridgeContext>,
) -> Result<tokio::process::Child, String> {
    // Resolve the final command. We also remember whether the resolved binary
    // lives inside WSL (`via_wsl`) so we can pick the right exec strategy
    // below — sending a Linux path to a Windows-native spawn would just fail.
    let (cmd_name, mut cmd_args, resolved_via_wsl) =
        resolve_agent_invocation(binary, npx_package, args)?;

    // Force current workspace as trusted for Codex sessions inside Docker.
    // This avoids path-style mismatch issues (/Users/... vs /host-home/...).
    let is_codex = binary == "codex" || npx_package == Some("@openai/codex");
    if is_codex {
        if let Some(exec_idx) = cmd_args.iter().position(|a| a == "exec") {
            let workdir_s = work_dir.display().to_string();
            let mut overrides = vec![
                "-c".to_string(),
                format!("projects.\"{}\".trust_level=\"trusted\"", workdir_s),
            ];
            if let Ok(host_home) = std::env::var("KRONN_HOST_HOME") {
                if let Some(relative) = workdir_s.strip_prefix("/host-home") {
                    overrides.push("-c".to_string());
                    let host_path = format!("{}{}", host_home, relative);
                    overrides.push(format!(
                        "projects.\"{}\".trust_level=\"trusted\"",
                        host_path,
                    ));
                }
            }
            cmd_args.splice(exec_idx + 1..exec_idx + 1, overrides);
        }
    }
    // Never log argv: prompts may contain user data and, historically, API
    // credentials. Besides the persistent log, argv is already visible to
    // the child process; duplicating it at INFO turns a transient exposure
    // into a durable one. Operational diagnostics only need the executable,
    // argument count, workdir and auth mode.
    tracing::info!(
        "Spawning agent: {} ({} args) in {} (key: {})",
        cmd_name,
        cmd_args.len(),
        work_dir.display(),
        if api_key.is_some() {
            "override"
        } else {
            "local auth"
        }
    );

    let (final_cmd, final_args, effective_work_dir) = platform_agent_invocation(
        cmd_name.clone(),
        cmd_args.clone(),
        resolved_via_wsl,
        work_dir,
    );

    let mut cmd = async_cmd(&final_cmd);
    cmd.args(&final_args)
        .current_dir(&effective_work_dir)
        .stdin(if stdin_payload.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // SIGKILL the agent process if its `Child` is dropped before
        // `wait()` returns. This is what makes workflow-run cancellation
        // actually stop in-flight Agent steps: when the runner drops the
        // step-dispatch future on `cancel_token.cancelled()`, the
        // `AgentProcess` (and the child it owns) is dropped, and
        // kill_on_drop turns that drop into a SIGKILL. Without this, the
        // child would be reparented to PID 1 and keep burning tokens.
        .kill_on_drop(true);

    // On Unix, create a new process group for the agent so we can terminate
    // the entire process tree (agent + its descendants) on cancellation.
    #[cfg(unix)]
    {
        unsafe {
            cmd.pre_exec(|| {
                // setpgid(0, 0) makes the process its own process group leader.
                // Returns 0 on success, -1 on error (errno set).
                let ret = libc::setpgid(0, 0);
                if ret == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
    }

    // Set TMPDIR to a directory on the same filesystem as work_dir.
    // Prevents EXDEV (cross-device link) errors when agents like Codex do
    // os.rename() from temp files to the work directory (macOS Docker + VirtioFS).
    let agent_tmpdir = work_dir.join(".kronn/tmp");
    let _ = std::fs::create_dir_all(&agent_tmpdir);
    // Ensure .kronn/tmp/ is gitignored in the project (once per project, idempotent)
    if let Some(project_path) = work_dir.to_str() {
        crate::core::mcp_scanner::ensure_gitignore_public(project_path, ".kronn/tmp/");
    }
    cmd.env("TMPDIR", &agent_tmpdir);
    cmd.env("TEMP", &agent_tmpdir);
    cmd.env("TMP", &agent_tmpdir);

    // In Docker, HOME=/home/kronn (the container user) and EVERY agent's auth
    // dir is mounted there by docker-compose: `${HOME}/.claude → /home/kronn/.claude`,
    // `${HOME}/.codex → /home/kronn/.codex`, `${HOME}/.vibe → /home/kronn/.vibe`,
    // `${HOME}/.gemini → /home/kronn/.gemini`, `${HOME}/.kiro → /home/kronn/.kiro`,
    // `${HOME}/.local/share/kiro-cli → /home/kronn/.local/share/kiro-cli`.
    // The pre-2026-05-07 code overrode HOME=KRONN_HOST_HOME for all agents
    // — agents would then look for `$HOME/.<agent>` at the host path
    // (e.g. `/home/<user>/.claude`) which doesn't exist inside the
    // container, and hang silently waiting for an auth token
    // (TD-20260507-home-override-breaks-claude-creds, reported in issue
    // #81 bug 5 on WSL2 for Claude — every other CLI agent had the same
    // shape and was likely silently broken too).
    //
    // Current policy: skip the override for ALL Kronn-managed CLI
    // agents whose config is mounted at /home/kronn/<dir>. Copilot is
    // fine either way because it has an explicit COPILOT_HOME override
    // a few lines down. Ollama doesn't read $HOME (uses HTTP API).
    // Unknown binaries keep the override — they may legitimately need
    // a host-rooted HOME (e.g. arbitrary user-installed tools).
    // Forward the discussion id to the agent process so the
    // `kronn-internal` MCP bridge (auto-injected into .mcp.json by the
    // disc setup paths) can call back into Kronn's introspection
    // endpoints with the right disc context. Set unconditionally when
    // we have one — non-MCP-aware agents simply ignore the env var.
    if let Some(disc_id) = discussion_id {
        cmd.env("KRONN_DISCUSSION_ID", disc_id);
        // Backend URL: the agent process runs on the same host as the
        // Kronn backend (Docker bridge or native), default 127.0.0.1
        // unless an operator override is set in the system env.
        let backend_url = std::env::var("KRONN_BACKEND_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:3140".to_string());
        cmd.env("KRONN_BACKEND_URL", backend_url);
    }
    if let Some(context) = task_worker_context {
        let encoded = serde_json::to_string(context)
            .map_err(|error| format!("Unable to encode task worker context: {error}"))?;
        cmd.env("KRONN_TASK_WORKER_CONTEXT", encoded);
    }

    let real_home = std::env::var("KRONN_HOST_HOME").ok().filter(|rh| {
        let exists = std::path::Path::new(rh).is_dir();
        if !exists {
            tracing::warn!("KRONN_HOST_HOME={} does not exist, ignoring", rh);
        }
        exists
    });
    let skip_home_override = should_skip_home_override(binary, npx_package);
    if let Some(ref rh) = real_home {
        if !skip_home_override {
            cmd.env("HOME", rh);
            cmd.env("USERPROFILE", rh); // Windows agents use USERPROFILE
        }
    }

    // Resolve the effective home for agent config lookups (cross-platform).
    let effective_home = real_home
        .clone()
        .or_else(|| std::env::var("HOME").ok())
        .or_else(|| std::env::var("USERPROFILE").ok());

    // Copilot CLI supports COPILOT_HOME to override config location.
    // Set it explicitly as a safety net (works on all platforms).
    if (binary == "copilot" || npx_package == Some("@github/copilot"))
        && std::env::var("COPILOT_HOME").is_err()
    {
        if let Some(ref home) = effective_home {
            let copilot_dir = std::path::Path::new(home).join(".copilot");
            if copilot_dir.exists() {
                cmd.env("COPILOT_HOME", &copilot_dir);
            }
        }
    }

    // Historical full-access discussions may still need Claude's container
    // marker to pass its root check. A spawned task worker must never inherit
    // that marker: Claude's own fail-closed OS sandbox is the boundary there,
    // and pretending an outer sandbox exists would undermine that guarantee.
    // Note: use CLAUDE_CODE_BUBBLEWRAP, not IS_SANDBOX — IS_SANDBOX also
    // suppresses 529 overloaded errors causing infinite silent retries.
    if task_worker_context.is_none() {
        cmd.env("CLAUDE_CODE_BUBBLEWRAP", "1");
    }
    // Hint shell-aware tools to use bash (dash does not support `-l`).
    // Only on Unix — Windows doesn't use SHELL env var.
    #[cfg(unix)]
    cmd.env("SHELL", "/bin/bash");

    // Only set API key env var if explicitly configured (override)
    // Otherwise let the agent use its own local auth
    if let Some(key) = api_key {
        cmd.env(env_key, key);
    }

    // Forward GitHub token so agents can create branches, PRs, etc.
    // Priority: env var GH_TOKEN/GITHUB_TOKEN > `gh auth token` (gh CLI config).
    // Also sets COPILOT_GITHUB_TOKEN for GitHub Copilot CLI.
    let gh_token = std::env::var("GH_TOKEN")
        .or_else(|_| std::env::var("GITHUB_TOKEN"))
        .or_else(|_| {
            // Fallback: extract token from `gh auth token` (stored in ~/.config/gh/hosts.yml).
            // Use sync_cmd so the gh subprocess does not flash a console window on Windows.
            crate::core::cmd::sync_cmd("gh")
                .args(["auth", "token"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| {
                    let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if t.is_empty() {
                        None
                    } else {
                        Some(t)
                    }
                })
                .ok_or(std::env::VarError::NotPresent)
        });
    if let Ok(ref token) = gh_token {
        cmd.env("GH_TOKEN", token);
        cmd.env("GITHUB_TOKEN", token);
        cmd.env("COPILOT_GITHUB_TOKEN", token);
    }
    // If an API key was explicitly set (e.g. for CopilotCli), also set COPILOT_GITHUB_TOKEN
    if let Some(key) = api_key {
        if env_key == "GH_TOKEN" {
            cmd.env("COPILOT_GITHUB_TOKEN", key);
        }
    }

    let invocation_receipt = (task_worker_context.is_some()
        && (binary == "claude" || npx_package == Some("@anthropic-ai/claude-code")))
    .then(|| command_invocation_size_receipt(&cmd, stdin_payload));
    if let Some(receipt) = invocation_receipt {
        tracing::info!(
            target: "kronn::agent::claude_task_worker",
            "Claude task-worker invocation receipt: {}",
            receipt.compact()
        );
        receipt.validate_single_argument_limit()?;
    }

    let mut child = cmd.spawn().map_err(|error| {
        if let Some(receipt) = invocation_receipt {
            format!(
                "Spawn failed for {cmd_name}: {error}; Claude task-worker invocation receipt: {}. \
                 Use `task_exec_reassign` to move this execution to another available worker.",
                receipt.compact()
            )
        } else {
            format!("Spawn failed for {cmd_name}: {error}")
        }
    })?;

    // Feed the prompt over stdin when requested. The caller uses this path
    // for Claude Code to keep large prompts off the argv size cap (~128 KiB
    // on Linux, hit by the initial Phase-1 prompt + skills injection on
    // EW-7189, producing `os error 7` / E2BIG before this fix).
    if let Some(payload) = stdin_payload {
        use tokio::io::AsyncWriteExt;
        if let Some(mut stdin) = child.stdin.take() {
            let owned = payload.to_string();
            tokio::spawn(async move {
                if let Err(e) = stdin.write_all(owned.as_bytes()).await {
                    tracing::warn!("Failed to write prompt to agent stdin: {}", e);
                }
                // Explicit close so the agent sees EOF and starts streaming.
                let _ = stdin.shutdown().await;
                drop(stdin);
            });
        }
    }

    Ok(child)
}

/// Parse a single line from Claude Code's --output-format stream-json output.
///
/// With `--verbose --include-partial-messages`, stream-json emits wrapped Anthropic API events:
/// ```json
/// {"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}}
/// ```
///
/// The final result line contains cost/token info:
/// ```json
/// {"type":"result","subtype":"success","cost_usd":0.01,"duration_ms":1234,"session_id":"...","usage":{"input_tokens":100,"output_tokens":50}}
/// ```
pub fn parse_claude_stream_line(line: &str) -> StreamJsonEvent {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return StreamJsonEvent::Skip;
    }

    let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        // Not valid JSON — pass through as plain text
        return StreamJsonEvent::Text(line.to_string());
    };

    let event_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match event_type {
        // Wrapped Anthropic streaming events
        "stream_event" => {
            let Some(event) = json.get("event") else {
                return StreamJsonEvent::Skip;
            };

            // Text delta: event.delta.type == "text_delta"
            if let Some(delta) = event.get("delta") {
                if delta.get("type").and_then(|v| v.as_str()) == Some("text_delta") {
                    if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                        let cleaned = strip_thinking_leaks(text);
                        // Entire delta was a leaked tag → skip to avoid
                        // streaming empty chunks that still bump the
                        // loop-repeat counter downstream.
                        if cleaned.is_empty() {
                            return StreamJsonEvent::Skip;
                        }
                        return StreamJsonEvent::Text(cleaned);
                    }
                }
            }

            // message_delta may carry usage
            if event.get("type").and_then(|v| v.as_str()) == Some("message_delta") {
                if let Some(usage) = event.get("usage") {
                    let input = usage
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let output = usage
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    if input > 0 || output > 0 {
                        return StreamJsonEvent::Usage {
                            input_tokens: input,
                            output_tokens: output,
                            cost_usd: None,
                        };
                    }
                }
            }

            // Tool input delta — accumulate partial JSON
            if let Some(delta) = event.get("delta") {
                if delta.get("type").and_then(|v| v.as_str()) == Some("input_json_delta") {
                    if let Some(partial) = delta.get("partial_json").and_then(|v| v.as_str()) {
                        return StreamJsonEvent::ToolInputDelta(partial.to_string());
                    }
                }
            }

            // Content block start — tool use or thinking
            if let Some(content_block) = event.get("content_block") {
                let block_type = content_block
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if block_type == "tool_use" {
                    if let Some(name) = content_block.get("name").and_then(|v| v.as_str()) {
                        return StreamJsonEvent::ToolStart(name.to_string());
                    }
                }
            }

            // Content block stop — tool input complete
            let event_type_inner = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if event_type_inner == "content_block_stop" {
                return StreamJsonEvent::ToolEnd;
            }

            StreamJsonEvent::Skip
        }

        // Final result line — contains token usage and cost
        "result" => {
            let cost = json
                .get("cost_usd")
                .and_then(|v| v.as_f64())
                .filter(|c| *c > 0.0);
            let input = json
                .pointer("/usage/input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let output = json
                .pointer("/usage/output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let is_error = json
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if is_error {
                let text = json
                    .get("result")
                    .and_then(|v| v.as_str())
                    .or_else(|| json.get("message").and_then(|v| v.as_str()))
                    .or_else(|| json.pointer("/error/message").and_then(|v| v.as_str()))
                    .unwrap_or("Claude Code reported an unsuccessful result")
                    .to_string();
                return StreamJsonEvent::TerminalError(StreamJsonFailure {
                    is_error,
                    text,
                    api_error_status: json
                        .get("api_error_status")
                        .and_then(|v| v.as_u64())
                        .and_then(|status| u16::try_from(status).ok()),
                    terminal_reason: json
                        .get("terminal_reason")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    input_tokens: input,
                    output_tokens: output,
                    cost_usd: cost,
                });
            }
            if json.get("usage").is_some() && (input > 0 || output > 0) {
                return StreamJsonEvent::Usage {
                    input_tokens: input,
                    output_tokens: output,
                    cost_usd: cost,
                };
            }
            StreamJsonEvent::Skip
        }

        // "assistant" messages with --include-partial-messages are cumulative snapshots
        // (they contain the full text so far, not a delta). We skip them to avoid
        // duplicating text already received via stream_event deltas.
        "assistant" => StreamJsonEvent::Skip,

        // Everything else (system, init, etc.)
        _ => StreamJsonEvent::Skip,
    }
}

/// Remove literal `<thinking>` / `</thinking>` tags from a text delta.
///
/// When Claude Opus runs with extended thinking enabled, the internal
/// reasoning tokens are supposed to travel through a dedicated
/// `thinking_delta` stream event — never inside `text_delta`. In practice
/// the decoder can leak a closing tag into text output and, worse, get
/// stuck repeating it (observed 6349× on EW-7189). Stripping is pragmatic:
/// the tags carry zero user-facing meaning, so removing them is lossless
/// for legitimate content and kills the visual noise from the leak. The
/// loop detection in `discussions.rs` handles the runaway case itself —
/// this helper just prevents the visible pollution from reaching the UI.
///
/// Case-insensitive on the tag name so a model quirk like `<Thinking>` is
/// also caught. Also matches the shorter `<think>` / `</think>` form emitted
/// by qwen3 (hybrid-reasoning) — the primary guard against qwen3 reasoning
/// leaks is `/no_think` (see `ollama_disables_thinking`), which keeps
/// `message.content` clean; this is only a secondary net for the tagged case.
/// We do NOT strip more generic HTML-ish tags — legitimate user-facing content
/// may contain other `<...>` patterns (code samples, XML docs, etc.), and
/// over-stripping would be worse than the leak.
pub fn strip_thinking_leaks(s: &str) -> String {
    static RE: std::sync::LazyLock<regex_lite::Regex> =
        std::sync::LazyLock::new(|| regex_lite::Regex::new(r"(?i)</?think(ing)?>").unwrap());
    RE.replace_all(s, "").to_string()
}

/// Strip ANSI escape codes from a string.
/// Handles CSI sequences (\x1b[...m), OSC, and other common escape patterns.
pub fn strip_ansi(s: &str) -> String {
    static RE: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(r"\x1b\[[0-9;?]*[a-zA-Z]|\x1b\][^\x07]*\x07|\x1b[()][0-9A-B]")
            .unwrap()
    });
    RE.replace_all(s, "").to_string()
}

/// Clean Kiro CLI output: strip ANSI codes, remove the "> " prefix, and filter noise lines.
/// Kiro mixes tool execution logs with actual response text. Filter out tool noise.
pub fn clean_kiro_line(line: &str) -> Option<String> {
    let clean = strip_ansi(line);
    let trimmed = clean.trim();
    // Skip empty lines, cursor control artifacts, and the Kiro banner/spinner
    if trimmed.is_empty()
        || trimmed.chars().all(|c| c.is_whitespace() || c == '\u{2800}') // braille blank chars in banner
        || trimmed.starts_with("Credits:")
        || trimmed.starts_with("▸ Credits:")
        // ── Kiro tool execution logs (structural patterns, language-independent) ──
        // Unicode marker lines
        || trimmed.starts_with("✓ ")       // ✓ Successfully read/found/etc.
        || trimmed.starts_with("↱ ")       // ↱ Operation N: ...
        || trimmed.starts_with("⋮")        // truncation marker
        || trimmed.starts_with("❗ ")       // ❗ No matches found ...
        // Tool invocation patterns (always in English — Kiro CLI log format)
        || trimmed.contains("(using tool:")           // "Reading file: X (using tool: read)"
        || trimmed.contains("(from mcp server:")      // "Running tool X ... (from mcp server: Y)"
        // Structured result lines (start with "- " followed by keyword)
        || trimmed.starts_with("- Completed in ")
        || trimmed.starts_with("- Summary: ")
        // Batch operation headers
        || trimmed.starts_with("Batch fs_read")
        || trimmed.starts_with("Batch ")
    {
        return None;
    }
    // Strip the "> " prefix Kiro adds to responses
    let result = if let Some(stripped) = trimmed.strip_prefix("> ") {
        stripped.to_string()
    } else {
        trimmed.to_string()
    };
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Parse token usage from agent output.
/// Codex outputs "tokens used\nN,NNN" on stderr (StdoutOnly mode captures it).
/// Kiro outputs "Credits: 0.05 • Time: 3s" on stderr.
/// Returns (cleaned_response, tokens_used) — token lines are stripped if found in response.
pub fn parse_token_usage(
    agent_type: &AgentType,
    response: &str,
    stderr_lines: &[String],
) -> (String, u64) {
    match agent_type {
        AgentType::Kiro => {
            // Kiro outputs "Credits: X.XX" or "▸ Credits: X.XX" on stderr.
            // Format observed: "Credits: 0.05 • Time: 3s" (may vary across versions).
            // We parse the float after "Credits:" and before the next "•" or EOL.
            // Store as integer: credits × 10000 for precision (0.05 → 500).
            for line in stderr_lines {
                let clean = strip_ansi(line);
                if let Some(credits_part) = clean.split("Credits:").nth(1) {
                    let credits_str = credits_part
                        .split('•')
                        .next()
                        .unwrap_or(credits_part)
                        .trim();
                    if let Ok(credits) = credits_str.parse::<f64>() {
                        let tokens = (credits * 10000.0) as u64;
                        return (response.to_string(), tokens);
                    } else {
                        tracing::warn!("Kiro credits parse failed for: {:?}", credits_str);
                    }
                }
            }
            if !stderr_lines.is_empty() {
                tracing::debug!(
                    "Kiro stderr ({} lines), no Credits found",
                    stderr_lines.len()
                );
            }
            (response.to_string(), 0)
        }
        AgentType::Codex => {
            // Codex writes `tokens used` followed by the count. Newer releases
            // may append diagnostics afterwards, so the marker is not reliably
            // the penultimate stderr line anymore. Scan every adjacent pair and
            // prefer the latest valid measurement.
            if let Some(count) = codex_token_usage(stderr_lines.iter().map(String::as_str)) {
                return (response.to_string(), count);
            }
            // Fallback: check stdout (some versions may put it there)
            let lines: Vec<&str> = response.lines().collect();
            for marker_index in (0..lines.len().saturating_sub(1)).rev() {
                if let Some(count) = codex_token_count(lines[marker_index], lines[marker_index + 1])
                {
                    let clean = lines
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| *index != marker_index && *index != marker_index + 1)
                        .map(|(_, line)| *line)
                        .collect::<Vec<_>>()
                        .join("\n");
                    return (clean, count);
                }
            }
            (response.to_string(), 0)
        }
        AgentType::Ollama | AgentType::LiteLlm | AgentType::Nvidia => {
            // Both HTTP backends put "ollama_tokens:prompt:eval" in stderr_capture
            // (written by `forward_chat_line` on the terminal chunk); the marker
            // keeps its original name because it is an internal wire detail. One
            // marker is pushed per TURN (on that turn's own `chunk.done`, with
            // `tally` reassigned — not accumulated — from that turn's own
            // response), so a multi-turn tool loop leaves one independent
            // marker per turn. KT-408 — this used to `return` on the FIRST
            // match, silently reporting only turn one's cost for any run that
            // used tools; the real total is the SUM across every turn.
            let mut total = 0u64;
            for line in stderr_lines {
                let Some(rest) = line.strip_prefix("ollama_tokens:") else {
                    continue;
                };
                let parts: Vec<&str> = rest.split(':').collect();
                // A malformed marker must not erase what valid ones already
                // contributed — skip it and keep accumulating.
                let [prompt_str, eval_str] = parts.as_slice() else {
                    continue;
                };
                let (Ok(prompt), Ok(eval)) = (prompt_str.parse::<u64>(), eval_str.parse::<u64>())
                else {
                    continue;
                };
                total += prompt + eval;
            }
            (response.to_string(), total)
        }
        AgentType::GeminiCli => {
            // Gemini CLI prepends `MCP issues detected. Run /mcp list for status.`
            // to its reply whenever ANY MCP server fails handshake (auth gone
            // stale, network blocked, etc.) — even when the reply is otherwise
            // fine. Surfacing the prefix in the saved transcript pollutes the
            // disc title generator and confuses the user (they assume Gemini
            // failed when it didn't). Strip it. Also drop the noisy
            // `Server 'X' supports tool updates...` and `[MCP error] ... ` debug
            // lines that occasionally leak into stdout. The MCP failure itself
            // is still logged via stderr_capture for debugging.
            //
            // Token usage isn't available on Gemini CLI 0.32 stdout (no
            // `tokens used` marker) — return 0, the auth_mode field still
            // disambiguates `override` vs `local auth` in the UI.
            const MCP_ISSUES_MARKER: &str = "MCP issues detected. Run /mcp list for status.";
            // Step 1 — drop debug noise lines. We keep blank lines so paragraph
            // breaks in the agent's actual reply survive intact.
            let filtered: Vec<&str> = response
                .lines()
                .filter(|line| {
                    let t = line.trim_start();
                    !t.starts_with("Server '")
                        && !t.starts_with("[MCP error]")
                        && !t.starts_with("[WARN] Skipping unreadable")
                })
                .collect();
            let cleaned = filtered.join("\n");
            // Step 2 — strip the `MCP issues detected.` marker once, wherever
            // it lands. Gemini sometimes emits it on its own line (filtered
            // here as a leading-prefix replacement), sometimes glued inline
            // to the next chunk (handled by the same `replacen` since the
            // marker is unique).
            let cleaned = cleaned.replacen(MCP_ISSUES_MARKER, "", 1);
            (cleaned.trim_start().to_string(), 0)
        }
        // Claude Code: tokens parsed inline via parse_claude_stream_line() in discussions.rs
        // TODO: Vibe — not yet supported
        _ => (response.to_string(), 0),
    }
}

fn codex_token_usage<'a>(lines: impl Iterator<Item = &'a str>) -> Option<u64> {
    let lines = lines.collect::<Vec<_>>();
    lines
        .windows(2)
        .rev()
        .find_map(|pair| codex_token_count(pair[0], pair[1]))
}

fn codex_token_count(marker: &str, count: &str) -> Option<u64> {
    if strip_ansi(marker).trim() != "tokens used" {
        return None;
    }
    let digits = strip_ansi(count)
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

#[cfg(test)]
#[path = "runner_test.rs"]
mod runner_test;

fn get_api_key(env_key: &str, tokens: &TokensConfig) -> Option<String> {
    let provider = match env_key {
        "ANTHROPIC_API_KEY" => "anthropic",
        "OPENAI_API_KEY" => "openai",
        "GEMINI_API_KEY" => "google",
        "MISTRAL_API_KEY" => "mistral",
        "OLLAMA_HOST" => return std::env::var(env_key).ok(), // Ollama: no API key, just host URL
        _ => return None,
    };

    // If override is disabled for this provider, fall back to env var.
    if tokens.disabled_overrides.iter().any(|d| d == provider) {
        // For Google specifically, also try the gemini-cli settings.json
        // fallback before giving up — see comment in the main return below.
        return std::env::var(env_key).ok().or_else(|| {
            if provider == "google" {
                read_gemini_settings_api_key()
            } else {
                None
            }
        });
    }

    // Use active key from multi-key system, then env var, then a final
    // Gemini-specific settings.json fallback. Why the last one:
    // gemini-cli 0.32.x does NOT honour the `apiKey` field in
    // `~/.gemini/settings.json` despite documenting it — it requires
    // `GEMINI_API_KEY` set in the process env. Without this fallback,
    // users who configured the key via `gemini auth login` (which writes
    // settings.json) hit `"You must specify the GEMINI_API_KEY
    // environment variable."` on every Kronn-spawned run, even though
    // the CLI works fine when invoked from the user's shell where the
    // env IS set. User report 2026-05-10 — the agent surfaced the
    // confusing fallback message `MCP issues detected. Run /mcp list
    // for status.` followed by the real `Network error. Unable to reach
    // the API.` because no API key meant no API call.
    tokens
        .active_key_for(provider)
        .map(|s| s.to_string())
        .or_else(|| std::env::var(env_key).ok())
        .or_else(|| {
            if provider == "google" {
                read_gemini_settings_api_key()
            } else {
                None
            }
        })
}

/// Read `apiKey` from `~/.gemini/settings.json` as a last-resort fallback
/// for `GEMINI_API_KEY`. Returns `None` on missing file, parse error, or
/// missing/empty `apiKey` field — caller handles None as "no key
/// available" the same way as before this fallback existed.
fn read_gemini_settings_api_key() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let path = std::path::Path::new(&home)
        .join(".gemini")
        .join("settings.json");
    let raw = std::fs::read_to_string(&path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let key = val.get("apiKey")?.as_str()?.trim();
    if key.is_empty() {
        None
    } else {
        Some(key.to_string())
    }
}
