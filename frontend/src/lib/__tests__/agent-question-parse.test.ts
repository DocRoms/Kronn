import { describe, it, expect } from 'vitest';
import { parseAgentQuestions, composeAnswers } from '../agent-question-parse';

describe('parseAgentQuestions', () => {
  it('returns empty for empty / whitespace-only input', () => {
    expect(parseAgentQuestions('')).toEqual([]);
    expect(parseAgentQuestions('   \n  ')).toEqual([]);
  });

  it('returns empty when no pattern is present', () => {
    const md = `Voici mon analyse :\n- point 1\n- point 2\nPas de question ici.`;
    expect(parseAgentQuestions(md)).toEqual([]);
  });

  it('parses a single question', () => {
    const content = `Pour avancer j'ai besoin d'une info.\n\n{{priority}}: Quelle est la priorité ?`;
    expect(parseAgentQuestions(content)).toEqual([
      { var: 'priority', question: 'Quelle est la priorité ?' },
    ]);
  });

  it('parses multiple questions and preserves order', () => {
    const content = `
Quelques précisions :

{{priority}}: Priorité ? (low/medium/high)
{{scope}}: Backend seul ou full-stack ?
{{deadline}}: Y a-t-il une deadline ?
    `;
    expect(parseAgentQuestions(content)).toEqual([
      { var: 'priority', question: 'Priorité ? (low/medium/high)' },
      { var: 'scope', question: 'Backend seul ou full-stack ?' },
      { var: 'deadline', question: 'Y a-t-il une deadline ?' },
    ]);
  });

  it('ignores a matching var with an empty body', () => {
    const content = `{{orphan}}:\n{{real}}: Une vraie question ?`;
    expect(parseAgentQuestions(content)).toEqual([
      { var: 'real', question: 'Une vraie question ?' },
    ]);
  });

  it('ignores lines missing the colon separator', () => {
    const content = `{{var}} pas de colon\n{{ok}}: OK ?`;
    expect(parseAgentQuestions(content)).toEqual([
      { var: 'ok', question: 'OK ?' },
    ]);
  });

  it('extracts only the pattern from mixed markdown', () => {
    const content = `
## Analyse

Le ticket touche 3 modules.

**Détails techniques :**
- auth
- API
- UI

{{risk}}: Risque acceptable ?

Je continue dès que tu me dis.
    `;
    expect(parseAgentQuestions(content)).toEqual([
      { var: 'risk', question: 'Risque acceptable ?' },
    ]);
  });

  it('rejects accented variable names (ASCII-only rule)', () => {
    // `\w+` without `u` flag is ASCII-only — `é` breaks the match.
    const content = `{{priorité}}: Invalid ?\n{{ok}}: Valid ?`;
    expect(parseAgentQuestions(content)).toEqual([
      { var: 'ok', question: 'Valid ?' },
    ]);
  });

  it('deduplicates on first occurrence', () => {
    const content = `{{x}}: Première\n{{x}}: Deuxième`;
    expect(parseAgentQuestions(content)).toEqual([
      { var: 'x', question: 'Première' },
    ]);
  });

  it('keeps only one line per question (does not bleed across newlines)', () => {
    const content = `{{q}}: Question sur une seule ligne.\nRemarque hors pattern.`;
    expect(parseAgentQuestions(content)).toEqual([
      { var: 'q', question: 'Question sur une seule ligne.' },
    ]);
  });

  it('handles extra spaces after the colon', () => {
    const content = `{{v}}:    Quelle valeur ?`;
    expect(parseAgentQuestions(content)).toEqual([
      { var: 'v', question: 'Quelle valeur ?' },
    ]);
  });

  // ── 0.8.4 follow-up — anchoring + code-region exclusion ─────────────
  // Bug repro: the QP Improver agent emits markdown like
  //   `--after="{{date}}T{{h1}}:00"` (inline code)
  // and ` ```git log ... {{h2}}:00 ... ``` ` (fenced).
  // Pre-fix the parser matched `{{h1}}:` mid-sentence and rendered a
  // garbage mini-form with "00\" --before=..." as the question text.

  it('rejects {{var}}: appearing mid-sentence (not at start of line)', () => {
    const content = `Voici la commande : --after="{{date}}T{{h1}}:00" suivie de plus de texte.`;
    expect(parseAgentQuestions(content)).toEqual([]);
  });

  it('ignores {{var}}: inside inline code (backticks)', () => {
    const content = 'Use this : `--after="{{date}}T{{h1}}:00"` — strict ISO 8601.';
    expect(parseAgentQuestions(content)).toEqual([]);
  });

  it('ignores {{var}}: inside fenced code blocks', () => {
    const content = `Voilà :\n\n\`\`\`bash\ngit log --after="{{date}}T{{h1}}:00" --before="{{date}}T{{h2}}:00"\n\`\`\`\n`;
    expect(parseAgentQuestions(content)).toEqual([]);
  });

  it('still matches questions prefixed with a bullet marker', () => {
    const content = `- {{priority}}: low/medium/high ?\n* {{scope}}: backend ou full-stack ?\n+ {{deadline}}: deadline ?\n• {{owner}}: qui pilote ?`;
    expect(parseAgentQuestions(content)).toEqual([
      { var: 'priority', question: 'low/medium/high ?' },
      { var: 'scope', question: 'backend ou full-stack ?' },
      { var: 'deadline', question: 'deadline ?' },
      { var: 'owner', question: 'qui pilote ?' },
    ]);
  });

  it('still matches questions prefixed with an ordered-list marker', () => {
    const content = `1. {{priority}}: priorité ?\n2) {{scope}}: scope ?`;
    expect(parseAgentQuestions(content)).toEqual([
      { var: 'priority', question: 'priorité ?' },
      { var: 'scope', question: 'scope ?' },
    ]);
  });

  it('reproduces the QP Improver bug case from the field', () => {
    // Slice of the real agent reply: recommendation prose with the
    // problematic `{{date}}T{{h1}}:00` inline code + a fenced JSON
    // block containing the same pattern. None of these are questions.
    const content = [
      '## Recommandations',
      '',
      '1. **Git command** → remplacer par `--after="{{date}}T{{h1}}:00" --before="{{date}}T{{h2}}:00"` (format ISO 8601 strict).',
      '',
      '```json',
      '{',
      '  "prompt_template": "git log --after=\\"{{date}}T{{h1}}:00\\" --before=\\"{{date}}T{{h2}}:00\\""',
      '}',
      '```',
    ].join('\n');
    expect(parseAgentQuestions(content)).toEqual([]);
  });
});

describe('composeAnswers', () => {
  const questions = [
    { var: 'priority', question: 'Priorité ?' },
    { var: 'scope', question: 'Scope ?' },
    { var: 'deadline', question: 'Deadline ?' },
  ];

  it('joins filled answers in question order', () => {
    expect(composeAnswers(questions, { priority: 'high', scope: 'full-stack', deadline: 'vendredi' }))
      .toBe('priority: high\nscope: full-stack\ndeadline: vendredi');
  });

  it('skips empty and whitespace-only answers', () => {
    expect(composeAnswers(questions, { priority: 'high', scope: '   ', deadline: '' }))
      .toBe('priority: high');
  });

  it('trims individual answers', () => {
    expect(composeAnswers(questions, { priority: '  high  ' }))
      .toBe('priority: high');
  });

  it('returns empty string when nothing is answered', () => {
    expect(composeAnswers(questions, {})).toBe('');
  });
});
