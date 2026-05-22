// 0.8.6 phase 4 — Tool-call System-message parser.
//
// The backend persists every tool call as a `System` message with a
// fixed text format. Two sources :
//
//   `[kronn-internal: <tool>(<args>?) → <result>?]`
//     → Kronn's own MCP exposed to the agent (qa_run, api_call,
//       disc_*, workflow_*, etc.). Deagentified primitives.
//
//   `[agent-native: <tool>(<args>?) → <result>?]`
//     → Everything else : Claude Code's own Read / Bash / Edit /
//       Write / Grep / Glob, third-party MCP servers wired in the
//       project, etc.
//
// Examples (real-world) :
//   [kronn-internal: qa_list()]
//   [kronn-internal: api_call({"endpoint_path": "/v1/sites"})]
//   [agent-native: Read({"file_path":"/tmp/foo.rs"})]
//   [agent-native: Bash({"command":"ls -la"})]
//
// Two consumers parse this :
//   - `MessageBubble.tsx` — renders ONE call as a yellow badge inline
//     (only the kronn-internal branch ; agent-native goes through the
//     grouping path only).
//   - `ToolCallsGroup.tsx` (0.8.6 phase 4) — groups N consecutive calls
//     into TWO sub-banners (Kronn-MCP vs Native) above the agent reply.
//
// Centralising the regex here means both consumers stay in sync if a
// backend format change ships in a future release.

export type ToolCallSource = 'kronn-internal' | 'agent-native';

export interface KronnToolCall {
  /** Where the tool comes from — drives banner grouping in the UI. */
  source: ToolCallSource;
  /** Full tool name, e.g. `qa_run`, `api_call`, `Read`, `Bash`. */
  name: string;
  /** Raw args string (the inside of the parentheses). `null` when the
   *  tool was called with no args, e.g. `qa_list()`. */
  args: string | null;
  /** Raw result string when present, `null` otherwise. */
  result: string | null;
}

// Tool names :
//   - Kronn-internal : lowercase + underscores only (qa_run, api_call).
//     Strict regex matches the backend tool catalog exactly.
//   - Agent-native : may use PascalCase (`Read`, `Bash`, `Edit`) OR
//     namespace prefixes (`mcp__github__create_issue`). Permissive
//     regex accepts any non-paren, non-bracket sequence.
const KRONN_INTERNAL_RE = /^\[kronn-internal: ([a-z_]+)(?:\(([\s\S]*?)\))?(?: → ([\s\S]*))?\]$/;
const AGENT_NATIVE_RE = /^\[agent-native: ([^()\[\]]+?)(?:\(([\s\S]*?)\))?(?: → ([\s\S]*))?\]$/;

/** Parse a System message's content. Returns `null` when the content
 *  isn't a tool trace — callers fall back to the default System-message
 *  rendering for non-tool entries (summary cache notices, error
 *  blocks, etc.). */
export function parseKronnToolMessage(content: string): KronnToolCall | null {
  if (content.startsWith('[kronn-internal:')) {
    const m = KRONN_INTERNAL_RE.exec(content.trim());
    if (!m) return null;
    return {
      source: 'kronn-internal',
      name: m[1],
      args: m[2] ?? null,
      result: m[3] ?? null,
    };
  }
  if (content.startsWith('[agent-native:')) {
    const m = AGENT_NATIVE_RE.exec(content.trim());
    if (!m) return null;
    return {
      source: 'agent-native',
      name: m[1].trim(),
      args: m[2] ?? null,
      result: m[3] ?? null,
    };
  }
  return null;
}

/** Quick predicate — true when a System message is any tool trace (Kronn
 *  or native). Cheaper than calling `parseKronnToolMessage` when the
 *  caller only needs to decide grouping. */
export function isKronnToolMessage(content: string): boolean {
  return content.startsWith('[kronn-internal:') || content.startsWith('[agent-native:');
}

/** Aggregate a list of tool calls into per-tool counts, sorted by
 *  descending count then ascending name (deterministic across renders).
 *  Used by the collapsible banner's compact summary line :
 *  `qa_run ×5, api_call ×2, mcp_list ×1`. */
export function groupToolCallsByName(
  calls: KronnToolCall[],
): Array<{ name: string; count: number }> {
  const counts = new Map<string, number>();
  for (const c of calls) {
    counts.set(c.name, (counts.get(c.name) ?? 0) + 1);
  }
  return [...counts.entries()]
    .map(([name, count]) => ({ name, count }))
    .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));
}

/** Format a duration in milliseconds compactly. < 1s → `<1s` (the
 *  precision below a second isn't useful for tool-call timing) ;
 *  1-60s → `Ns` ; > 60s → `MmSs`. */
export function formatDurationCompact(ms: number): string {
  if (ms < 1000) return '<1s';
  const totalSeconds = Math.round(ms / 1000);
  if (totalSeconds < 60) return `${totalSeconds}s`;
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return seconds > 0 ? `${minutes}m${seconds}s` : `${minutes}m`;
}
