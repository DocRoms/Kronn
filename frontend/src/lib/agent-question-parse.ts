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
 */
export function parseAgentQuestions(content: string): AgentQuestion[] {
  if (!content || !content.trim()) return [];

  // `\w+` is ASCII-only in JS regex by default (no `u` flag) — this is
  // what we want: `{{priorité}}` is NOT a valid var name, matching the
  // Quick Prompts rule.
  // `[ \t]*` (NOT `\s*`) after the colon — we must stay on the same line
  // so `{{orphan}}:\n{{next}}: body` doesn't capture `{{next}}: body` as
  // the orphan's question.
  const pattern = /\{\{(\w+)\}\}:[ \t]*([^\n]+)/g;
  const seen = new Set<string>();
  const out: AgentQuestion[] = [];

  for (const match of content.matchAll(pattern)) {
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
