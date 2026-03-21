/** Clean markdown for natural speech, keeping meaning intact */
export function stripMarkdown(md: string): string {
  return md
    // Remove code blocks entirely (unreadable as speech)
    .replace(/```[\s\S]*?```/g, '')
    // Inline code: keep content
    .replace(/`([^`]+)`/g, '$1')
    // Headings → add period after heading text for a natural pause
    .replace(/^#{1,6}\s+(.+)$/gm, '$1.')
    // Bold / italic
    .replace(/\*\*([^*]+)\*\*/g, '$1')
    .replace(/\*([^*]+)\*/g, '$1')
    // Links
    .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
    // Bullet list items → end with comma (natural list reading), last-like items get period
    .replace(/^\s*[-*+]\s+(.+)$/gm, '$1,')
    // Numbered list items → end with period
    .replace(/^\s*\d+\.\s+(.+)$/gm, '$1.')
    // Blockquotes → add period
    .replace(/^>\s+(.+)$/gm, '$1.')
    // Tables
    .replace(/\|[^|\n]*(?:\|[^|\n]*)*/g, '')
    // Horizontal rules → pause
    .replace(/---+/g, '.')
    .replace(/KRONN:[A-Z_]+/g, '')
    // URLs → just say "lien"
    .replace(/https?:\/\/\S+/g, 'lien')
    // File paths → keep just the filename
    .replace(/(?:\.\/|\/)[a-zA-Z0-9_\-/]*\/([a-zA-Z0-9_\-.]+)/g, '$1')
    // snake_case → spaces (compute_step_checksums → "compute step checksums")
    .replace(/\b([a-zA-Z][a-zA-Z0-9]*(?:_[a-zA-Z0-9]+)+)\b/g, (_, id: string) => id.replace(/_/g, ' '))
    // camelCase/PascalCase → spaces (getDiscussionById → "get Discussion By Id")
    .replace(/\b([a-z][a-zA-Z0-9]*[A-Z][a-zA-Z0-9]*)\b/g, (_, id: string) =>
      id.replace(/([a-z])([A-Z])/g, '$1 $2').replace(/([A-Z]+)([A-Z][a-z])/g, '$1 $2'))
    // JSON-like fragments { ... }
    .replace(/\{[^}]{0,200}\}/g, '')
    // Collapse newlines to spaces
    .replace(/\n/g, ' ')
    // Clean up trailing commas before periods (list items followed by heading)
    .replace(/,\s*\./g, '.')
    // Collapse stray double periods and whitespace
    .replace(/\.\s*\./g, '.')
    .replace(/,\s*,/g, ',')
    .replace(/\s{2,}/g, ' ')
    .trim();
}

/** Split text into sentences for pipelined TTS */
export function splitSentences(text: string): string[] {
  // Split on sentence-ending punctuation followed by space or end
  const raw = text.match(/[^.!?:;]+[.!?:;]+[\s]?|[^.!?:;]+$/g) || [text];
  // Merge very short fragments with the previous sentence
  const merged: string[] = [];
  for (const s of raw) {
    const trimmed = s.trim();
    if (!trimmed) continue;
    if (merged.length > 0 && trimmed.length < 10) {
      merged[merged.length - 1] += ' ' + trimmed;
    } else {
      merged.push(trimmed);
    }
  }
  return merged.filter(s => s.length > 2);
}
