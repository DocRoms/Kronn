import { ALL_AGENT_TYPES } from './constants';

/** Matches a tool call an ACP runtime wrote into the middle of a reply.
 *
 *  Between 2026-08-31 and 0.13.0 the ACP transport forwarded tool calls on the
 *  channel carrying the reply itself, so messages read
 *  `Je vais vérifier.[ClaudeCode tool: ToolSearch][ClaudeCode tool: WebFetch]
 *  Réponse courte :` — one message held 93 of them. The backend no longer
 *  produces these, but the messages already written keep them, and they are
 *  unreadable for anyone who does not know what they are.
 *
 *  Anchored on a known agent name and closed on the same line, because prose
 *  legitimately contains the words: two messages on the reporting instance say
 *  things like `(using tool: read, max depth: 1)`, and those must survive
 *  untouched. */
const MARKER = new RegExp(
  `\\[(?:${ALL_AGENT_TYPES.join('|')}) tool: ([^\\]\\n]{1,200})\\]`,
  'g',
);

export interface StrippedToolMarkers {
  /** The reply as it was meant to read. */
  content: string;
  /** The tools that were called, in order, deduplicated consecutively — a run
   *  of five identical calls is five calls, but reading `ToolSearch ×5` is the
   *  point of removing them from the prose in the first place. */
  tools: { name: string; count: number }[];
}

/** Lifts the markers out of a reply so it reads as prose again.
 *
 *  Returns the content unchanged and an empty list when there is nothing to
 *  strip, so a caller can keep the original object identity and skip the work. */
export function stripAcpToolMarkers(content: string): StrippedToolMarkers {
  MARKER.lastIndex = 0;
  if (!MARKER.test(content)) return { content, tools: [] };

  const tools: { name: string; count: number }[] = [];
  MARKER.lastIndex = 0;
  const stripped = content.replace(MARKER, (_match, name: string) => {
    const trimmed = name.trim();
    const last = tools[tools.length - 1];
    if (last && last.name === trimmed) last.count += 1;
    else tools.push({ name: trimmed, count: 1 });
    return '';
  });

  // Removing a marker from between two sentences leaves them welded together;
  // removing one that sat on its own line leaves a blank line behind.
  const content2 = stripped.replace(/[ \t]+\n/g, '\n').replace(/\n{3,}/g, '\n\n');

  return { content: content2, tools };
}
