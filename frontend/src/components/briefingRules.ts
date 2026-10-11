// KT-1193 — pure helpers of the briefing's rules-and-bans editor.
import type { ProjectRule, ProjectRuleKind, ProjectRulePatternKind } from '../types/generated';

export const RULES_MAX = 20;
export const RULE_TEXT_MAX = 200;
export const RULE_PATTERN_MAX = 100;
export const RULE_PATTERN_KINDS: ProjectRulePatternKind[] = ['dependency', 'import', 'file_glob'];

export interface RuleRow {
  id: number;
  kind: ProjectRuleKind;
  text: string;
  patternKind: ProjectRulePatternKind | null;
  pattern: string;
}

export const PATTERN_PLACEHOLDERS: Record<ProjectRulePatternKind, string> = {
  dependency: 'lodash',
  import: 'lodash',
  file_glob: 'src/legacy/**',
};

/** The rows as answers: empty lines dropped, a pattern kept on a ban only. */
export function rowsToRules(rows: RuleRow[]): ProjectRule[] {
  return rows
    .filter(r => r.text.trim())
    .map(r => ({
      kind: r.kind,
      text: r.text.trim(),
      pattern: r.kind === 'ban' && r.patternKind && r.pattern.trim()
        ? { kind: r.patternKind, value: r.pattern.trim() }
        : null,
    }));
}
