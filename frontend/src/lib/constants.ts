// ─── Shared constants across pages ──────────────────────────────────────────

import type { AgentType, AgentsConfig } from '../types/generated';

export const AGENT_COLORS: Record<string, string> = {
  ClaudeCode: '#D4714E',
  'Claude Code': '#D4714E',
  Codex: '#10a37f',
  Vibe: '#FF7000',
  GeminiCli: '#4285f4',
  'Gemini CLI': '#4285f4',
  Kiro: '#7B61FF',
  CopilotCli: '#238636',
  'GitHub Copilot': '#238636',
  Ollama: '#60A5FA',
  LiteLlm: '#14B8A6',
};

export const AGENT_LABELS: Record<string, string> = {
  ClaudeCode: 'Claude Code',
  Codex: 'Codex',
  Vibe: 'Vibe',
  GeminiCli: 'Gemini CLI',
  Kiro: 'Kiro',
  CopilotCli: 'GitHub Copilot',
  Ollama: 'Ollama',
  LiteLlm: 'LiteLLM',
};

export const ALL_AGENT_TYPES: AgentType[] = ['ClaudeCode', 'Codex', 'Vibe', 'GeminiCli', 'Kiro', 'CopilotCli', 'Ollama', 'LiteLlm'];

export const AGENT_MENTIONS: ReadonlyArray<{
  trigger: string;
  type: AgentType;
  label: string;
}> = [
  { trigger: '@claude', type: 'ClaudeCode', label: 'Claude Code' },
  { trigger: '@codex', type: 'Codex', label: 'Codex' },
  { trigger: '@vibe', type: 'Vibe', label: 'Vibe' },
  { trigger: '@gemini', type: 'GeminiCli', label: 'Gemini CLI' },
  { trigger: '@kiro', type: 'Kiro', label: 'Kiro' },
  { trigger: '@copilot', type: 'CopilotCli', label: 'GitHub Copilot' },
  { trigger: '@ollama', type: 'Ollama', label: 'Ollama' },
  { trigger: '@litellm', type: 'LiteLlm', label: 'LiteLLM' },
];

/** Return every canonical agent mention found in text, once each and in the
 * order the human wrote them.
 *
 * Shared by every composer that launches explicitly mentioned agents. Keeping
 * the boundary rule here prevents discussion creation, send, and edit/resend
 * from drifting.
 */
export function mentionedAgents(text: string): AgentType[] {
  const matches: Array<{ type: AgentType; index: number }> = [];
  for (const mention of AGENT_MENTIONS) {
    const escaped = mention.trigger.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const matcher = new RegExp(
      `(^|[\\s([{])${escaped}(?=$|[\\s\\])},.!?;:])`,
      'gi',
    );
    let match: RegExpExecArray | null;
    while ((match = matcher.exec(text)) !== null) {
      matches.push({
        type: mention.type,
        index: match.index + match[1].length,
      });
    }
  }
  matches.sort((left, right) => left.index - right.index);
  return matches
    .filter((match, index, all) => (
      all.findIndex(candidate => candidate.type === match.type) === index
    ))
    .map(match => match.type);
}

/** Canonical mention for the human of this Kronn instance. Agents must use
 *  this exact trigger instead of inventing a pseudo, so a message addressed to
 *  the human renders like any other mention. */
export const USER_MENTION_TRIGGER = '@user';

/** Agents RTK can hook to compress shell output (token savings). Kiro/Copilot/
 *  Vibe/Ollama are out of scope (not in RTK's list / no shell to hook / local).
 *  Single source of truth — shared by the RTK Settings card and the
 *  new-discussion "no RTK" cost warning. */
export const RTK_APPLICABLE: ReadonlySet<AgentType> = new Set<AgentType>([
  'ClaudeCode', 'Codex', 'GeminiCli',
]);

/** RTK is "active" for an agent when the binary is installed AND the agent's
 *  hook is wired. `rtk_available`/`rtk_hook_configured` come from agent
 *  detection (`AgentDetection`). */
export function isRtkActive(agent: { rtk_available: boolean; rtk_hook_configured: boolean }): boolean {
  return agent.rtk_available && agent.rtk_hook_configured;
}

export type AgentMentionColors = Partial<Record<AgentType, string>>;

const AGENT_CONFIG_KEY: Partial<Record<AgentType, keyof AgentsConfig>> = {
  ClaudeCode: 'claude_code',
  Codex: 'codex',
  Vibe: 'vibe',
  GeminiCli: 'gemini_cli',
  Kiro: 'kiro',
  CopilotCli: 'copilot_cli',
  Ollama: 'ollama',
};

const isRgbHex = (value: string | null | undefined): value is string =>
  typeof value === 'string' && /^#[0-9a-f]{6}$/i.test(value);

/** Extract validated per-agent mention colors from persisted config. Invalid
 *  hand-edited TOML values are ignored and fall back to the built-in palette. */
export function agentMentionColors(agentAccess: AgentsConfig | null | undefined): AgentMentionColors {
  if (!agentAccess) return {};
  const colors: AgentMentionColors = {};
  for (const agentType of ALL_AGENT_TYPES) {
    const key = AGENT_CONFIG_KEY[agentType];
    if (!key || key === 'model_tiers') continue;
    const config = agentAccess[key];
    if (config && typeof config === 'object' && 'mention_color' in config) {
      const color = config.mention_color;
      if (isRgbHex(color)) colors[agentType] = color;
    }
  }
  return colors;
}

export const agentColor = (
  agentType: string | null | undefined,
  overrides?: AgentMentionColors,
): string => {
  const override = agentType ? overrides?.[agentType as AgentType] : undefined;
  return isRgbHex(override) ? override : AGENT_COLORS[agentType ?? ''] ?? '#8b5cf6';
};

/** Check if an agent has full_access disabled (restricted mode). */
export function isAgentRestricted(agentAccess: AgentsConfig | undefined, agentType: AgentType): boolean {
  if (!agentAccess) return false;
  const map: Record<string, boolean | undefined> = {
    ClaudeCode: agentAccess.claude_code?.full_access,
    Codex: agentAccess.codex?.full_access,
    GeminiCli: agentAccess.gemini_cli?.full_access,
    Vibe: agentAccess.vibe?.full_access,
    Kiro: undefined,
    CopilotCli: agentAccess.copilot_cli?.full_access,
    Ollama: agentAccess.ollama?.full_access,
  };
  return map[agentType] === false;
}

/** Extract org/owner from a project's repo_url for grouping.
 *  Returns the org name (e.g. "acme-org") or a fallback label. */
export function getProjectGroup(p: { repo_url: string | null }, localLabel = 'Local', otherLabel = 'Other'): string {
  if (!p.repo_url) return localLabel;
  try {
    const url = p.repo_url.replace('git@github.com:', 'https://github.com/')
      .replace('git@gitlab.com:', 'https://gitlab.com/');
    const parts = new URL(url).pathname.split('/').filter(Boolean);
    return parts[0] || otherLabel;
  } catch { return otherLabel; }
}

/** Whether the agent can introspect the discussion it's running in
 *  (`disc_meta`, `disc_get_message`, `disc_summarize`).
 *
 *  Two paths exist on the backend (cf. `disc_prompts.rs` —
 *  `agent_speaks_mcp` / `agent_uses_slash_markers` gates):
 *    - MCP tools (single-turn, fast) for Claude Code, Kiro, Gemini,
 *      Copilot, Codex (since 0.132) — see
 *      `mcp_scanner::inject_kronn_internal`.
 *    - Slash markers (multi-turn: agent emits `KRONN:DISC_*`, Kronn
 *      resolves on next turn) for Vibe + Ollama — see
 *      `slash_markers.rs`.
 *
 *  Every concrete `AgentType` now has at least one path → returns
 *  `true` unconditionally. Kept as a function rather than inlined
 *  so we still have a single canonical name to grep for when a
 *  future agent breaks the assumption.
 *
 *  History — 0.8.6 (2026-05-20) : Codex flipped to supporting after
 *  upstream Codex 0.132 fixed the exec-mode sandbox that cancelled
 *  MCP tool calls in 0.121. Confirmed by a `tools/call` smoke test
 *  through Codex itself. The earlier `TD-20260510-codex-mcp-sandbox-
 *  block` is closed. */
export function agentSupportsIntrospection(_agentType: AgentType): boolean {
  return true;
}

/** Check if an agent has full_access enabled. */
export function hasAgentFullAccess(agentAccess: AgentsConfig | undefined, agentType: AgentType): boolean {
  if (!agentAccess) return false;
  const map: Record<string, boolean | undefined> = {
    ClaudeCode: agentAccess.claude_code?.full_access,
    Codex: agentAccess.codex?.full_access,
    GeminiCli: agentAccess.gemini_cli?.full_access,
    Vibe: agentAccess.vibe?.full_access,
    Kiro: undefined,
    CopilotCli: agentAccess.copilot_cli?.full_access,
    Ollama: agentAccess.ollama?.full_access,
  };
  return map[agentType] === true;
}

// ─── Shared predicates (used by Dashboard, DiscussionsPage, McpPage) ────────

/** Check if a path contains a hidden segment (starts with '.') */
export const isHiddenPath = (path: string) => path.split('/').some(s => s.startsWith('.'));

/** Agent is usable: runnable, enabled, and not known to be missing auth. */
export const isUsable = (a: {
  installed: boolean;
  runtime_available: boolean;
  enabled: boolean;
  auth_ready?: boolean;
}) => (a.installed || a.runtime_available) && a.enabled && a.auth_ready !== false;

/** Check if a discussion title matches the validation audit title */
export const isValidationDisc = (title: string) => title === 'Validation audit AI';

/** 0.8.2 — Tracker MCP name needles. Must stay in sync with backend
 *  `detect_issue_tracker_mcp` in `api/audit/helpers.rs`. The audit
 *  Phase 3 + AutoPilot preset both leverage these to offer ticket
 *  creation; the ProjectCard tracker-hint banner shows when none of
 *  these match any wired MCP server. */
export const TRACKER_MCP_NEEDLES = [
  'github', 'gitlab', 'jira', 'atlassian', 'linear', 'youtrack',
] as const;

/** Substring-match a server name/id against the tracker needles.
 *  Case-insensitive, matches the backend heuristic exactly. */
export const isTrackerMcp = (serverNameOrId: string): boolean => {
  const lower = serverNameOrId.toLowerCase();
  return TRACKER_MCP_NEEDLES.some(n => lower.includes(n));
};

/** 0.8.2 — Parse a GitHub / GitLab remote URL into `{owner, repo}`.
 *  Handles all four common shapes observed in the field:
 *    - `git@github.com:OWNER/REPO.git`        (SSH)
 *    - `https://github.com/OWNER/REPO.git`    (HTTPS + .git)
 *    - `https://github.com/OWNER/REPO`        (HTTPS clean)
 *    - `https://github.com/OWNER/REPO/`       (HTTPS trailing slash)
 *  Returns `null` for non-GitHub/GitLab hosts and malformed inputs.
 *  Powers the AutoPilot deep-link auto-fill of `{owner}/{repo}` in
 *  the `fetch_issue` step path. */
/** 0.8.2 — "Oldest open issue" REST request descriptor per tracker plugin.
 *  Returned by `buildOldestIssueRequest`; consumed by the AutoPilot deep-link
 *  to pre-fill the `fetch_issue` step. Generic by design — adding a new
 *  tracker (e.g. Linear) is a single switch case + matching test. */
export interface OldestIssueRequest {
  endpoint: string;
  query: Record<string, string>;
  path_params?: Record<string, string>;
  extract_path: string;
}

/** Build the per-tracker "list oldest open issue" request shape.
 *  Each ecosystem has its own URL/query conventions, captured here:
 *    - GitHub REST v3:  `/repos/{owner}/{repo}/issues?state=open&sort=created&direction=asc&per_page=1` → `$[0]`
 *    - GitLab API v4:   `/api/v4/projects/{project_id}/issues?state=opened&order_by=created_at&sort=asc&per_page=1` → `$[0]`
 *      where `{project_id}` is `<owner>/<repo>` (percent-encoded by Kronn's
 *      path-param resolver at runtime — no manual %2F needed here).
 *    - Jira Cloud:      `/rest/api/3/search/jql?jql=…ORDER BY created ASC&maxResults=1` → `$.issues[0]`
 *      (no owner/repo concept; the project key lives in the JQL itself).
 *  Returns `null` for unknown plugins or when the GitHub/GitLab branch
 *  has no parsed `{owner, repo}` AND we still want to surface placeholder
 *  fields (we return the request with empty values — the user fills them). */
export const buildOldestIssueRequest = (
  pluginSlug: string,
  repo: { owner: string; repo: string } | null,
): OldestIssueRequest | null => {
  switch (pluginSlug) {
    case 'mcp-github':
      return {
        endpoint: '/repos/{owner}/{repo}/issues',
        query: { state: 'open', sort: 'created', direction: 'asc', per_page: '1' },
        path_params: { owner: repo?.owner ?? '', repo: repo?.repo ?? '' },
        extract_path: '$[0]',
      };
    case 'mcp-gitlab':
      return {
        endpoint: '/api/v4/projects/{project_id}/issues',
        query: { state: 'opened', order_by: 'created_at', sort: 'asc', per_page: '1' },
        path_params: { project_id: repo ? `${repo.owner}/${repo.repo}` : '' },
        extract_path: '$[0]',
      };
    case 'mcp-jira':
    case 'mcp-atlassian':
      return {
        endpoint: '/rest/api/3/search/jql',
        query: { jql: 'statusCategory != Done ORDER BY created ASC', maxResults: '1' },
        path_params: undefined,
        extract_path: '$.issues[0]',
      };
    default:
      return null;
  }
};

export const parseRepoUrl = (url: string | null | undefined): { owner: string; repo: string } | null => {
  if (!url) return null;
  // Match `(github|gitlab|codeberg).com[:/]<owner>/<repo>` then strip trailing
  // `.git` and slashes. The character classes intentionally reject `/` and
  // `:` inside owner/repo so a malformed URL like `github.com/foo/bar/baz`
  // gives `{owner: foo, repo: bar}` rather than producing junk.
  const m = url.match(/(?:github|gitlab|codeberg)\.(?:com|io|org)[:/]([^/:]+)\/([^/:]+?)(?:\.git)?\/?$/i);
  if (!m) return null;
  return { owner: m[1], repo: m[2] };
};

/** 0.8.2 — Infer the MCP tracker slug from the project's `repo_url`.
 *  Used by the AutoPilot deep-link to pick GitHub vs GitLab vs Jira
 *  WITHOUT relying on which tracker MCP happens to be `is_global`.
 *  Without this, a globally-wired Jira would shadow a project-specific
 *  GitHub config because both match the "actively wired" filter. */
export const inferTrackerSlugFromRepoUrl = (url: string | null | undefined): string | null => {
  if (!url) return null;
  const lower = url.toLowerCase();
  if (lower.includes('github.com')) return 'mcp-github';
  if (lower.includes('gitlab.com') || lower.includes('gitlab.')) return 'mcp-gitlab';
  // Codeberg/Forgejo has its own API (/api/v1/...) — leave null until
  // a dedicated mcp-forgejo plugin lands.
  return null;
};

/** A briefing discussion is created by the backend with a localized
 *  title. Pre-fix the per-page detector used `startsWith('Briefing')`
 *  which only matched FR (`Briefing projet`) and ES (`Briefing del
 *  proyecto`); EN's `Project Briefing` was missed, so English users
 *  saw none of the briefing-specific UI (Zap icon, completion CTA,
 *  refetch-on-open effect). `includes` covers all three localized
 *  shapes and is safe — no other system-created title contains the
 *  word "Briefing". */
export const isBriefingDisc = (title: string) => title.includes('Briefing');

/** A bootstrap discussion always opens with the literal `Bootstrap: `
 *  prefix on every locale (the backend hard-codes the string and
 *  appends the project name). Using `startsWith` keeps user-named
 *  discussions like "About bootstrap testing" out of this branch. */
export const isBootstrapDisc = (title: string) => title.startsWith('Bootstrap: ');
