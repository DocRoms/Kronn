// KT-1518 — what a room or a run says when a turn or a step continued on the
// next agent of a fallback chain:
//   « Codex (gpt-5.6-sol): quota exhausted until 14:00 UTC -> continued with Claude Code (sonnet) »
// The backend writes it (`core::agent_fallback::notice`); this reads it back so
// the view can show from which agent/model, why, until when, and to which
// agent/model. Kept out of the component module so that module exports only components.

export interface FallbackNotice {
  fromAgent: string;
  fromModel: string;
  /** « quota exhausted », « rate limited », « provider unavailable », « not authenticated ». */
  reason: string;
  /** « 14:00 UTC », when the provider announced its reset. */
  until: string | null;
  toAgent: string;
  toModel: string;
}

const NOTICE = /^(.+?) \((.+?)\): (.+?)(?: until (\d{2}:\d{2} UTC))? -> continued with (.+?) \((.+)\)$/;

/** The notice a line holds, or `null` for anything else (never throws). */
export function parseFallbackNotice(line: string): FallbackNotice | null {
  const match = NOTICE.exec(line.trim());
  if (!match) return null;
  return {
    fromAgent: match[1],
    fromModel: match[2],
    reason: match[3],
    until: match[4] ?? null,
    toAgent: match[5],
    toModel: match[6],
  };
}

/** A step's output starts with the notices of its switches as quoted lines
 *  (« > … »): they are split off, the rest is the step's own output. */
export function splitStepNotices(output: string): { notices: FallbackNotice[]; rest: string } {
  const lines = output.split('\n');
  const notices: FallbackNotice[] = [];
  let at = 0;
  while (at < lines.length && lines[at].startsWith('> ')) {
    const notice = parseFallbackNotice(lines[at].slice(2));
    if (!notice) break;
    notices.push(notice);
    at += 1;
  }
  if (notices.length === 0) return { notices, rest: output };
  return { notices, rest: lines.slice(at).join('\n').replace(/^\n+/, '') };
}
