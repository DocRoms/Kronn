// KT-609 — reading a `[src: …]` marker instead of stepping over it.
//
// Agents cite their sources with a formal marker the backend already verifies
// (`anti_halluc.rs`: the file exists, the lines are in range, the path stays
// under the project root). What was never handled is the marker LEFT IN THE
// PROSE: machine notation put in front of a reader, doubling a footer pill
// that already counts them.
//
// This only splits the text. It decides nothing about whether a citation is
// good — the backend did that, and its verdict travels on the message.

/** One `[src: …]` marker found in a run of text. */
export interface SourceCitation {
  /** Everything after `src:`, trimmed — the key that matches a `SourceCheck`. */
  raw: string;
  /** `file`, `commit`, `url`, `user`… lowercased. Empty when the marker has none. */
  kind: string;
  /** What the marker points at, with the kind removed. */
  reference: string;
  /** Short enough to read inside a sentence. */
  label: string;
}

/** A run of prose, or a citation standing in the middle of it. */
export type CitationPart = { text: string } | { citation: SourceCitation };

// Deliberately narrow: no nested brackets, no newlines. A marker that does not
// fit stays the text it is, because turning prose into a chip by accident is
// worse than leaving one marker unread.
const MARKER = /\[src:[ \t]*([^\][\n]+?)[ \t]*\]/g;

/** The last segment of a path, with its line or range kept. */
function fileLabel(reference: string): string {
  const [path, ...rest] = reference.split(':');
  const name = path.split('/').filter(Boolean).pop() ?? path;
  const lines = rest.join(':').trim();
  return lines ? `${name}:${lines}` : name;
}

function shorten(reference: string, max = 28): string {
  return reference.length <= max ? reference : `${reference.slice(0, max - 1)}…`;
}

function labelFor(kind: string, reference: string): string {
  if (!reference) return kind || 'src';
  switch (kind) {
    case 'file':
    case 'code-comment':
      return fileLabel(reference);
    // A sha is only ever read by its first characters, and the full one stays
    // one hover away.
    case 'commit':
      return /^[0-9a-f]{7,40}$/i.test(reference) ? reference.slice(0, 7) : shorten(reference);
    case 'url':
    case 'api':
      try {
        return new URL(reference).host;
      } catch {
        return shorten(reference);
      }
    default:
      return shorten(reference);
  }
}

/**
 * Split one string into prose and citations. A string with no marker comes
 * back as a single text part, so callers can render it unchanged.
 */
export function splitSourceCitations(text: string): CitationPart[] {
  const parts: CitationPart[] = [];
  let cursor = 0;
  MARKER.lastIndex = 0;
  let match = MARKER.exec(text);
  while (match) {
    if (match.index > cursor) parts.push({ text: text.slice(cursor, match.index) });
    const raw = match[1].trim();
    // `file: path:12` — the kind is what precedes the first colon, and only
    // when it looks like a kind rather than the start of a path.
    const separator = raw.indexOf(':');
    const head = separator === -1 ? '' : raw.slice(0, separator).trim().toLowerCase();
    const isKind = /^[a-z][a-z-]*$/.test(head);
    const kind = isKind ? head : '';
    const reference = isKind ? raw.slice(separator + 1).trim() : raw;
    parts.push({ citation: { raw, kind, reference, label: labelFor(kind, reference) } });
    cursor = match.index + match[0].length;
    match = MARKER.exec(text);
  }
  if (cursor < text.length) parts.push({ text: text.slice(cursor) });
  return parts;
}

/** Whether a string holds anything worth splitting. Cheap enough to ask first. */
export function hasSourceCitation(text: string): boolean {
  MARKER.lastIndex = 0;
  return MARKER.test(text);
}
