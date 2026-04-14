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
