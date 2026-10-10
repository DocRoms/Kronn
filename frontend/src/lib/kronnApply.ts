// Shared parser for the `KRONN:APPLY` proposal blocks emitted by the AI
// helpers (custom API config and workflow ApiCall step).

export interface ApplySuggestion {
  signature: string;
  parsed: Record<string, unknown>;
  applied: boolean;
}

export interface KronnApplyParse {
  /** Proposals whose JSON parsed to a plain object. */
  blocks: ApplySuggestion[];
  /** Message text with every proposal chunk removed. */
  prose: string;
  /** Raw content of the blocks that failed to parse, or of a marker no block followed; null otherwise. */
  unreadable: string | null;
}

const MARKER = 'KRONN:APPLY';
const JSON_FENCE = '```[ \\t]*(?:json)?[ \\t]*\\n?([\\s\\S]*?)```';

// Order matters: the marker in its own fence must win over "marker inside the
// json fence", which would otherwise capture an empty body.
const APPLY_SOURCE = [
  // Marker wrapped in its own fence, then a json fence.
  `\`\`\`[^\\n]*\\n[ \\t]*${MARKER}[ \\t]*\\n?[ \\t]*\`\`\`\\s*${JSON_FENCE}`,
  // Bare marker alone on its line, then a json fence (the documented format).
  // The line anchor keeps a sentence ending with the word from becoming a proposal.
  `^[ \\t]*${MARKER}[ \\t]*\\n\\s*${JSON_FENCE}`,
  // Marker as the first line inside the json fence. The lookahead stops a
  // previous block's closing fence from swallowing a bare marker.
  `\`\`\`[ \\t]*(?:json)?[ \\t]*\\n[ \\t]*${MARKER}[ \\t]*\\n(?![ \\t]*\`\`\`)([\\s\\S]*?)\`\`\``,
].join('|');

function applyRegex(): RegExp {
  return new RegExp(APPLY_SOURCE, 'gim');
}

// A marker on a line of its own (optionally in backticks or bold): mentions
// inside a sentence are not proposals and must not raise the warning.
const MARKER_LINE_RX = /^[ \t>*_`]*kronn:apply[ \t*_`:]*$/im;

function parseObject(raw: string): Record<string, unknown> | null {
  try {
    const value: unknown = JSON.parse(raw);
    if (value && typeof value === 'object' && !Array.isArray(value)) {
      return value as Record<string, unknown>;
    }
  } catch {
    // Streaming may expose an incomplete block, or the agent emitted bad JSON.
  }
  return null;
}

export function parseKronnApply(input: string): KronnApplyParse {
  // Every pattern expects LF; agents on Windows hosts may emit CRLF.
  const text = input.replace(/\r\n?/g, '\n');
  const blocks: ApplySuggestion[] = [];
  const failed: string[] = [];
  for (const match of text.matchAll(applyRegex())) {
    const body = match[1] ?? match[2] ?? match[3] ?? '';
    const parsed = parseObject(body);
    if (parsed) blocks.push({ signature: body.trim(), parsed, applied: false });
    else failed.push(body.trim());
  }
  const prose = text.replace(applyRegex(), '').trim();

  // A broken block is reported even next to valid ones, so its JSON is never lost.
  let unreadable: string | null = failed.length > 0 ? failed.join('\n\n') : null;
  if (unreadable === null && blocks.length === 0) {
    const marker = MARKER_LINE_RX.exec(text);
    if (marker) unreadable = text.slice(marker.index).trim();
  }
  return { blocks, prose, unreadable };
}

export function parseApplyBlocks(text: string): ApplySuggestion[] {
  return parseKronnApply(text).blocks;
}
