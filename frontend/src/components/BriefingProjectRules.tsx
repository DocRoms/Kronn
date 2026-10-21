/**
 * KT-1193 — "Rules and bans" in the briefing: a few lines, each always (rule)
 * or never (ban); a ban may carry a pattern Kronn checks at each audit. The
 * server re-validates everything on save.
 */
import { useEffect, useRef, useState } from 'react';
import { Plus, X } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import type { ProjectRule, ProjectRuleKind, ProjectRulePatternKind } from '../types/generated';
import {
  PATTERN_PLACEHOLDERS, RULES_MAX, RULE_PATTERN_KINDS, RULE_PATTERN_MAX, RULE_TEXT_MAX, rowsToRules, rulesToRows, type RuleRow,
} from './briefingRules';

const KINDS: ProjectRuleKind[] = ['rule', 'ban'];

interface Props {
  onChange: (rules: ProjectRule[]) => void;
  /** Rows on mount; the parent remounts the editor (`key`) to load others. */
  initial?: ProjectRule[];
}

export function BriefingProjectRules({ onChange, initial = [] }: Props) {
  const { t } = useT();
  const nextId = useRef(initial.length + 1);
  const [rows, setRows] = useState<RuleRow[]>(() => rulesToRows(initial, 1));

  useEffect(() => { onChange(rowsToRules(rows)); }, [rows, onChange]);

  const patch = (id: number, change: Partial<RuleRow>) =>
    setRows(rs => rs.map(r => (r.id === id ? { ...r, ...change } : r)));
  const add = () => setRows(rs => (rs.length >= RULES_MAX
    ? rs
    : [...rs, { id: nextId.current++, kind: 'ban', text: '', patternKind: null, pattern: '' }]));
  const remove = (id: number) => setRows(rs => rs.filter(r => r.id !== id));

  return (
    <section className="briefing-project briefing-rules" aria-labelledby="briefing-rules-title" data-testid="briefing-rules">
      <h4 id="briefing-rules-title" className="briefing-form-q">{t('briefing.rules.title')}</h4>
      <p className="briefing-form-intro">{t('briefing.rules.intro')}</p>

      {rows.map((row, index) => {
        const n = index + 1;
        return (
          <fieldset key={row.id} className="briefing-card briefing-rule" data-rule-kind={row.kind}>
            <div className="briefing-rule-head">
              <div className="briefing-card-options" role="radiogroup" aria-label={t('briefing.rules.kindQ', n)}>
                {KINDS.map(kind => (
                  <button
                    key={kind}
                    type="button"
                    role="radio"
                    aria-checked={row.kind === kind}
                    className={`briefing-card-option${row.kind === kind ? ' is-selected' : ''}`}
                    onClick={() => patch(row.id, { kind })}
                  >
                    {t(`briefing.rules.kind.${kind}`)}
                  </button>
                ))}
              </div>
              <input
                className="briefing-card-input briefing-rule-text"
                aria-label={t('briefing.rules.textLabel', n)}
                placeholder={t(`briefing.rules.placeholder.${row.kind}`)}
                value={row.text}
                maxLength={RULE_TEXT_MAX}
                onChange={e => patch(row.id, { text: e.target.value })}
              />
              <button
                type="button"
                className="briefing-rule-remove"
                onClick={() => remove(row.id)}
                aria-label={t('briefing.rules.remove', n)}
              >
                <X size={12} />
              </button>
            </div>
            {row.kind === 'ban' && (
              <div className="briefing-rule-pattern">
                <select
                  className="briefing-card-input"
                  aria-label={t('briefing.rules.patternKindLabel', n)}
                  value={row.patternKind ?? ''}
                  onChange={e => patch(row.id, {
                    patternKind: (e.target.value || null) as ProjectRulePatternKind | null,
                  })}
                >
                  <option value="">{t('briefing.rules.pattern.none')}</option>
                  {RULE_PATTERN_KINDS.map(kind => (
                    <option key={kind} value={kind}>{t(`briefing.rules.pattern.${kind}`)}</option>
                  ))}
                </select>
                {row.patternKind && (
                  <input
                    className="briefing-card-input"
                    aria-label={t('briefing.rules.patternLabel', n)}
                    placeholder={PATTERN_PLACEHOLDERS[row.patternKind]}
                    value={row.pattern}
                    maxLength={RULE_PATTERN_MAX}
                    onChange={e => patch(row.id, { pattern: e.target.value })}
                  />
                )}
              </div>
            )}
          </fieldset>
        );
      })}

      <button
        type="button"
        className="briefing-card-option"
        onClick={add}
        disabled={rows.length >= RULES_MAX}
        data-testid="briefing-rules-add"
      >
        <Plus size={11} /> {t('briefing.rules.add')}
      </button>
      {rows.length >= RULES_MAX && <p className="briefing-form-intro">{t('briefing.rules.max')}</p>}
      <p className="briefing-form-option-hint">{t('briefing.rules.patternHint')}</p>
    </section>
  );
}
