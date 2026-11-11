/**
 * KT-1544 — the optional merge of a step: its main execution and the shadows it
 * selects are condensed into one result that becomes the step's output. Off by
 * default, a step with shadows only compares them. On, the merge waits for the
 * selected shadows (up to a timeout), goes on with those that finished, and a
 * condense pass dedups, verifies each point and records which executions found
 * it: a point it cannot verify is never published.
 */
import { Info } from 'lucide-react';
import type { ReactNode } from 'react';
import { DEFAULT_MERGE_MINUTES, MAX_MERGE_MINUTES, mergedShadowIds } from '../../lib/shadowRuns';
import type { ShadowAgent, ShadowMerge, StepShadow } from '../../types/generated';

type Translate = (key: string, ...args: (string | number)[]) => string;

export function StepMergeEditor({ shadows, merge, onChange, agentFields, testId, t }: {
  shadows: readonly StepShadow[];
  merge: ShadowMerge | undefined;
  /** `null` turns the merge off. */
  onChange: (next: ShadowMerge | null) => void;
  /** The agent picker the shadows use, for the condense pass's agent. */
  agentFields: (value: ShadowAgent | undefined, onChange: (next: ShadowAgent | undefined) => void, testId: string) => ReactNode;
  testId: string;
  t: Translate;
}) {
  const enabled = !!merge && merge.enabled !== false;
  const picked = mergedShadowIds(merge, shadows);
  const all = (merge?.shadows ?? []).length === 0;
  const toggle = (id: string) => {
    if (!merge) return;
    const next = picked.includes(id) ? picked.filter(other => other !== id) : [...picked, id];
    // At least one shadow merges; every shadow selected is « all ».
    if (next.length === 0) return;
    onChange({ ...merge, shadows: next.length === shadows.length ? [] : next });
  };

  return (
    <div className="wf-shadow-merge" data-testid={`${testId}-merge`} data-enabled={enabled}>
      <p className="wf-shadow-subtitle">{t('wf.shadow.merge.title')}</p>
      <label className="wf-shadow-item-field wf-shadow-item-enabled">
        <input
          type="checkbox"
          data-testid={`${testId}-merge-enabled`}
          checked={enabled}
          onChange={e => onChange(e.target.checked ? { ...(merge ?? {}), enabled: true } : null)}
        />
        <span>{t('wf.shadow.merge.enable')}</span>
      </label>
      <p className="text-muted wf-retention-help" data-testid={`${testId}-merge-help`}>
        <Info size={11} /> {t(enabled ? 'wf.shadow.merge.description' : 'wf.shadow.merge.off')}
      </p>
      {enabled && merge && (
        <>
          <p className="wf-shadow-subtitle">{t('wf.shadow.merge.which')}</p>
          <div className="wf-shadow-chips" data-testid={`${testId}-merge-which`}>
            <button
              type="button"
              className="wf-chip"
              data-selected={all}
              aria-pressed={all}
              data-testid={`${testId}-merge-all`}
              onClick={() => onChange({ ...merge, shadows: [] })}
            >
              {t('wf.shadow.merge.all')}
            </button>
            {shadows.map(shadow => {
              const on = picked.includes(shadow.id);
              return (
                <button
                  key={shadow.id}
                  type="button"
                  className="wf-chip"
                  data-selected={on}
                  aria-pressed={on}
                  data-testid={`${testId}-merge-shadow-${shadow.id}`}
                  onClick={() => toggle(shadow.id)}
                >
                  {shadow.label || shadow.id}
                </button>
              );
            })}
          </div>
          <div className="wf-shadow-item-row">
            <label className="wf-shadow-item-field">
              <span className="text-muted">{t('wf.shadow.merge.timeout')}</span>
              <input
                className="wf-shadow-input wf-shadow-number"
                type="number"
                min={1}
                max={MAX_MERGE_MINUTES}
                data-testid={`${testId}-merge-timeout`}
                placeholder={String(DEFAULT_MERGE_MINUTES)}
                value={merge.timeout_secs ? Math.ceil(merge.timeout_secs / 60) : ''}
                onChange={e => {
                  const minutes = Number(e.target.value);
                  const valid = Number.isFinite(minutes) && minutes > 0;
                  onChange({ ...merge, timeout_secs: valid ? Math.min(MAX_MERGE_MINUTES, Math.round(minutes)) * 60 : undefined });
                }}
              />
            </label>
          </div>
          <p className="wf-shadow-subtitle">{t('wf.shadow.merge.agent')}</p>
          <div className="wf-shadow-item-row">
            {agentFields(merge.agent, agent => onChange({ ...merge, agent }), `${testId}-merge-condense`)}
          </div>
          <p className="text-muted wf-retention-help"><Info size={11} /> {t('wf.shadow.merge.cost')}</p>
        </>
      )}
    </div>
  );
}
