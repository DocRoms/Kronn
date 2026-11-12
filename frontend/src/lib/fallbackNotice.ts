// KT-1518 / KT-1693 — what a room or a run says when a turn or a step continued on
// a fallback:
//   « Codex · gpt-5.6-sol: quota exhausted until 14:00 UTC → continued with Claude Code · sonnet [CLI] »
// The backend writes it (`core::agent_fallback::notice`); this reads it back so
// the view can show from which agent/model, why, until when, and to which
// agent/model over which transport. A line written by 0.15.3
// (« Codex (gpt-5.6-sol): … -> continued with Claude Code (sonnet) ») is still
// read, with no transport. Kept out of the component module so that module
// exports only components.

import type { FallbackTransport } from '../types/generated';

export interface FallbackNotice {
  fromAgent: string;
  fromModel: string;
  /** « quota exhausted », « rate limited », « provider unavailable », « not authenticated ». */
  reason: string;
  /** « 14:00 UTC », when the provider announced its reset. */
  until: string | null;
  toAgent: string;
  toModel: string;
  /** How the fallback is reached; `null` on a line written before transports were named. */
  toTransport: FallbackTransport | null;
}

const NOTICE = /^(.+?) · (.+?): (.+?)(?: until (\d{2}:\d{2} UTC))? → continued with (.+?) · (.+) \[(CLI|HTTP|Local)\]$/;
const LEGACY_NOTICE = /^(.+?) \((.+?)\): (.+?)(?: until (\d{2}:\d{2} UTC))? -> continued with (.+?) \((.+)\)$/;

const TRANSPORTS: Record<string, FallbackTransport> = { CLI: 'cli', HTTP: 'http', Local: 'local' };

/** The notice a line holds, or `null` for anything else (never throws). */
export function parseFallbackNotice(line: string): FallbackNotice | null {
  const text = line.trim();
  const match = NOTICE.exec(text);
  if (match) {
    return {
      fromAgent: match[1],
      fromModel: match[2],
      reason: match[3],
      until: match[4] ?? null,
      toAgent: match[5],
      toModel: match[6],
      toTransport: TRANSPORTS[match[7]] ?? null,
    };
  }
  const legacy = LEGACY_NOTICE.exec(text);
  if (!legacy) return null;
  return {
    fromAgent: legacy[1],
    fromModel: legacy[2],
    reason: legacy[3],
    until: legacy[4] ?? null,
    toAgent: legacy[5],
    toModel: legacy[6],
    toTransport: null,
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
