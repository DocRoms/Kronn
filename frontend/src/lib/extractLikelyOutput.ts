// Heuristic to extract the "useful data" from a raw agent response.
//
// Used by the step test panel to auto-fill the mock input for the next
// step: instead of pasting 3 pages of agent blabla, this function picks
// the structured envelope (if present), the last data-like line, or
// falls back to the full text.
//
// Priority:
//   1. STEP_OUTPUT envelope (Structured mode) → take its JSON contents
//   2. Last non-empty line if it looks like data (short, or contains
//      commas, semicolons, JSON brackets)
//   3. Full text (let the user clean it up)

export interface ExtractResult {
  /** The extracted or full value. */
  value: string;
  /** True if the value was extracted (envelope or last-line heuristic).
   *  False if it's the raw input unchanged (single line or full fallback). */
  extracted: boolean;
}

export function extractLikelyOutput(raw: string): ExtractResult {
  if (!raw) return { value: '', extracted: false };

  // 1. Structured envelope
  const envMatch = raw.match(/---STEP_OUTPUT---([\s\S]+?)---END_STEP_OUTPUT---/);
  if (envMatch) return { value: envMatch[1].trim(), extracted: true };

  // 2. Line-based heuristic
  const lines = raw.split('\n').map(l => l.trim()).filter(Boolean);
  if (lines.length === 0) return { value: '', extracted: false };
  if (lines.length === 1) return { value: lines[0], extracted: false };

  const last = lines[lines.length - 1];
  const looksLikeData = (
    last.length < 200 ||
    last.includes(',') ||
    last.includes(';') ||
    last.startsWith('[') ||
    last.startsWith('{')
  );
  if (looksLikeData) return { value: last, extracted: true };

  // 3. Full fallback
  return { value: raw, extracted: false };
}
