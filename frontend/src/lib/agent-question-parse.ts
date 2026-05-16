// Parser for structured questions in agent messages.
//
// When an agent replies with questions formatted as
//   {{var_name}}: question text
// the frontend surfaces them as a mini-form above ChatInput so the user
// can answer each one in its own field instead of copy-pasting bullet
// points into the chat.
//
// Design notes:
// - Syntax is lossless: a human reader sees "{{priority}}: …" and it still
//   makes sense. If we ever disable the feature, the raw message remains
//   readable.
// - We reuse the Quick Prompts `{{var}}` ASCII-only rule (`\w+`). Accented
//   or unicode identifiers are intentionally rejected to match the Quick
//   Prompts renderer — same grammar on both sides.
// - The reply is sent as plain `key: value` lines (not YAML/JSON) so the
//   next agent turn parses it naturally.

/** A single parsed question from an agent message. */
export interface AgentQuestion {
  /** Variable name — ASCII `\w+` only. Unique within a message (first wins). */
  var: string;
  /** Question text the agent wrote after `:`. Trimmed. */
  question: string;
}

/**
 * Strip fenced code blocks (```…```) and inline code (`…`) by replacing
 * them with same-length whitespace runs. We keep the byte length and
 * newlines intact so downstream line-anchored regexes still match at
 * the right offsets — only the *content* of the code regions becomes
 * unparseable.
 *
 * Without this, agents emitting QP refactors / git commands containing
 * placeholders like `--after="{{date}}T{{h1}}:00"` would trip the
 * question parser into reading `h1` and `h2` as questions with the
 * trailing `:00"` as the question body. Cf. user-reported UX bug in
 * the QP Improver flow, 0.8.4 dogfooding.
 */
function stripCodeRegions(content: string): string {
  const fence = /```[\s\S]*?```/g;
  // Inline code: backtick-delimited, no newline inside. We're loose
  // here — markdown actually allows `` `…` `` (double backticks to
  // embed a backtick inside) but agents don't emit that in practice
  // for our case (questions vs. code disambiguation).
  const inline = /`[^`\n]*`/g;
  const blank = (m: string) => m.replace(/[^\n]/g, ' ');
  return content.replace(fence, blank).replace(inline, blank);
}

/**
 * Extract structured `{{var}}: question` entries from an agent message.
 *
 * Returns them in source order. Duplicate `var` names: first occurrence wins
 * (silently drops later ones — a reply form with two inputs named the same
 * would be confusing).
 *
 * Returns an empty array if:
 * - `content` is empty / whitespace-only
 * - No matching pattern is found
 * - The only matches have empty question text (e.g. `{{foo}}:` alone)
 *
 * Anchoring rules (0.8.4 fix): the `{{var}}:` token MUST appear at the
 * start of a line — optionally preceded by whitespace, a bullet marker
 * (`- ` / `* ` / `+ ` / `• `), or a markdown ordered list prefix
 * (`1. `). A `{{var}}:` token sitting mid-sentence (e.g. inside
 * `--after="{{date}}T{{h1}}:00"`) is rejected. Same goes for tokens
 * inside fenced or inline code — those regions are blanked out before
 * matching (see `stripCodeRegions`).
 */
export function parseAgentQuestions(content: string): AgentQuestion[] {
  if (!content || !content.trim()) return [];

  const sanitized = stripCodeRegions(content);

  // Anchored at start-of-line (`m` flag → `^` matches per line).
  // Optional prefix: indentation, then optional bullet (`-`, `*`, `+`,
  // `•`) or ordered-list marker (digits + `.` or `)`), then one
  // mandatory space/tab after the marker. Followed immediately by
  // `{{var}}:` then `[ \t]*` then the question body.
  // `\w+` stays ASCII-only (no `u` flag) — matches the Quick Prompts
  // renderer rule.
  const pattern = /^[ \t]*(?:(?:[-*+•]|\d+[.)])[ \t]+)?\{\{(\w+)\}\}:[ \t]*([^\n]+)/gm;
  const seen = new Set<string>();
  const out: AgentQuestion[] = [];

  for (const match of sanitized.matchAll(pattern)) {
    const varName = match[1];
    const question = match[2].trim();
    if (!question) continue;           // empty body → skip
    if (seen.has(varName)) continue;   // duplicate → first wins
    seen.add(varName);
    out.push({ var: varName, question });
  }

  return out;
}

/**
 * Compose the user reply from a set of answers.
 *
 * Output format: one `var: value` line per question, in the order the
 * agent asked. Skips empty / whitespace-only answers so the agent sees
 * only actually-answered fields on the next turn.
 */
export function composeAnswers(
  questions: AgentQuestion[],
  answers: Record<string, string>,
): string {
  const lines: string[] = [];
  for (const q of questions) {
    const value = (answers[q.var] ?? '').trim();
    if (!value) continue;
    lines.push(`${q.var}: ${value}`);
  }
  return lines.join('\n');
}
