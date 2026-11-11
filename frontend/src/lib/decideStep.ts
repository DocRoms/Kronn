import type { DecideConfig, DecideKind, DecideQuestion } from '../types/generated';

export const DECIDE_KINDS: DecideKind[] = ['noul', 'choice', 'score'];

/** Measured defaults (docs/research/decision-step-bench-2026-10-21.md). */
export const DEFAULT_MIN_CONFIDENCE: Record<DecideKind, number> = { noul: 0.85, choice: 0.35, score: 0.3 };

export function blankDecideQuestion(index: number): DecideQuestion {
  return { id: index === 0 ? 'ok' : `q${index + 1}`, kind: 'noul', instructions: '' };
}

export function blankDecideConfig(): DecideConfig {
  return { state: '{{previous_step.output}}', questions: [blankDecideQuestion(0)] };
}

const SAFE = /^[a-z0-9_]{1,48}$/;

/** Why the step cannot be saved yet, mirroring the backend check. */
export function decideConfigProblem(config: DecideConfig | null | undefined): string | null {
  if (!config) return 'missing';
  if (!config.state.trim()) return 'state';
  if (config.questions.length === 0 || config.questions.length > 64) return 'questions';
  const ids = new Set<string>();
  for (const question of config.questions) {
    if (!SAFE.test(question.id) || ids.has(question.id)) return 'id';
    ids.add(question.id);
    if (!question.instructions.trim()) return 'instructions';
    const options = Object.entries(question.options ?? {});
    if (question.kind === 'choice' && (options.length < 2 || options.length > 26
      || options.some(([key, text]) => !SAFE.test(key) || !text.trim()))) return 'options';
    if (question.kind === 'score') {
      const levels = question.levels ?? [];
      if (levels.length < 2 || levels.length > 10 || levels.some(level => !level.trim())) return 'levels';
    }
  }
  return null;
}

/** The signals a question emits, for the `on_result` rules. */
export function decideSignals(question: DecideQuestion): string[] {
  if (question.kind === 'choice') return Object.keys(question.options ?? {}).map(option => `${question.id}_${option}`);
  if (question.kind === 'score') return (question.levels ?? []).map((_, index) => `${question.id}_${index + 1}`);
  return [`${question.id}_yes`, `${question.id}_no`];
}
