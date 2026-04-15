/**
 * One-click bug-report helper — builds a GitHub "new issue" URL with the
 * title/body pre-filled from live backend info + the last N log lines.
 *
 * Design:
 *  - Pure functions only — the component calls `fetch` for raw data, then
 *    `buildIssueUrl(...)` with the pieces. Testable in isolation.
 *  - Secrets are redacted client-side BEFORE the URL is built: users
 *    occasionally paste API keys into prompts that end up logged
 *    (`echo $OPENAI_API_KEY`-style missteps). Better to scrub here than
 *    hope no one copies the wrong line.
 *  - Output URL caps at a conservative 6000 chars (GitHub supports ~8k
 *    but some browsers truncate earlier). If the encoded body would
 *    overflow, we trim the oldest log lines first — the most recent
 *    ones are what the bug reporter actually saw.
 */

/** Canonical upstream repo. Change here if Kronn ever forks. */
export const KRONN_REPO_URL = 'https://github.com/DocRoms/Kronn';

/** Hard URL-length budget. GitHub says 8k; browsers are safer at ~6k. */
const MAX_URL_LENGTH = 6000;

export interface BugReportContext {
  /** `env!("CARGO_PKG_VERSION")` from `/api/health`. */
  kronnVersion?: string | null;
  /** `KRONN_HOST_OS` or heuristic value from `/api/health`. */
  hostOs?: string | null;
  /** Short one-line-per-agent summary (e.g. `"Claude Code: ok v2.3.0"`). */
  agentsSummary?: string[];
  /** Raw log lines (oldest-first) — sanitised and truncated before use. */
  logLines: string[];
  /** Optional free-text the user entered into the "What happened" field. */
  userContext?: string;
  /** `navigator.userAgent` — captured by the caller (DOM-only). */
  userAgent?: string;
}

/**
 * Redact common secret-ish patterns from a single log line. Conservative
 * by design — false positives are fine (a redacted line still conveys
 * that an env var was read), false negatives would leak real keys.
 */
export function redactSecrets(line: string): string {
  return line
    // Anthropic / OpenAI-style keys (sk-...)
    .replace(/\bsk-[a-zA-Z0-9_-]{20,}\b/g, 'sk-***REDACTED***')
    // GitHub personal / fine-grained / app tokens
    .replace(/\bgh[opsu]_[A-Za-z0-9_]{30,}\b/g, 'gh*_***REDACTED***')
    // Google API keys
    .replace(/\bAIza[0-9A-Za-z_-]{30,}\b/g, 'AIza***REDACTED***')
    // Bearer tokens in headers / logs
    .replace(/\b([Bb]earer\s+)[A-Za-z0-9._-]{20,}/g, '$1***REDACTED***')
    // Generic JSON "password" / "token" fields
    .replace(/("(?:password|token|api_key|apiKey|secret)"\s*:\s*")[^"]+(")/g,
      '$1***REDACTED***$2');
}

export function sanitizeLogLines(lines: string[]): string[] {
  return lines.map(redactSecrets);
}

export function buildIssueTitle(ctx: BugReportContext): string {
  const host = ctx.hostOs ? ` on ${ctx.hostOs}` : '';
  const ver = ctx.kronnVersion ? ` v${ctx.kronnVersion}` : '';
  return `[Bug] Kronn${ver}${host}`;
}

/**
 * Assemble the markdown body. Uses GitHub-flavoured details/summary so
 * the logs collapse by default — keeps the issue readable while still
 * including the full captured tail.
 */
export function buildIssueBody(ctx: BugReportContext, maxLogLines = 200): string {
  const sanitized = sanitizeLogLines(ctx.logLines);
  const capped = sanitized.slice(-maxLogLines);
  const logs = capped.length > 0 ? capped.join('\n') : '(no logs captured)';
  const agents = ctx.agentsSummary && ctx.agentsSummary.length > 0
    ? ctx.agentsSummary.join('\n')
    : '(no agents detected)';

  const preamble = ctx.userContext?.trim() || '<!-- Please describe what you were doing when the bug occurred -->';

  return `## Environment
- Kronn version: ${ctx.kronnVersion ?? 'unknown'}
- Host OS: ${ctx.hostOs ?? 'unknown'}
- Browser: ${ctx.userAgent ?? 'unknown'}

## What happened
${preamble}

## Agent detection
\`\`\`
${agents}
\`\`\`

## Backend logs (last ${capped.length} lines)
<details>
<summary>Expand</summary>

\`\`\`
${logs}
\`\`\`

</details>

<!-- Submitted from Kronn Settings > Debug > Report a bug. Secrets matching
     common patterns (sk-*, gh?_*, AIza*, Bearer *, JSON password/token) have
     been redacted client-side. Review before submitting. -->
`;
}

/**
 * Build the final `https://github.com/.../issues/new?...` URL.
 *
 * If the encoded URL would exceed `MAX_URL_LENGTH`, log lines are
 * trimmed from the oldest side (keeping the most recent events) and
 * the body is rebuilt. Guarantees the returned URL fits in a browser
 * address bar / `window.open` call.
 */
export function buildIssueUrl(ctx: BugReportContext): string {
  // Start with a generous log window; shrink if needed.
  for (const cap of [200, 100, 50, 20, 10, 0]) {
    const body = buildIssueBody(ctx, cap);
    const url = formatUrl(buildIssueTitle(ctx), body);
    if (url.length <= MAX_URL_LENGTH) return url;
  }
  // Fallback: title-only (logs dropped entirely, user fills body in GitHub).
  return formatUrl(buildIssueTitle(ctx), '');
}

function formatUrl(title: string, body: string): string {
  const params = new URLSearchParams();
  params.set('title', title);
  if (body) params.set('body', body);
  params.set('labels', 'bug');
  return `${KRONN_REPO_URL}/issues/new?${params.toString()}`;
}
