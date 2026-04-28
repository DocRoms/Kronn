/**
 * "Limites d'exécution" — workflow guards card (0.7.0 / Phase 1).
 *
 * UX rationale (Antoine review):
 *   - Visible by default (collapsible only when no overrides set), placed
 *     after the step list and before Save. Not hidden in "Advanced".
 *   - Plain language labels ("Durée max", "Appels IA max", "Détection
 *     de boucle") — never the API names ("guards", "max_llm_calls").
 *   - Default values shown as grey placeholders so the user sees the
 *     fallback without subscribing to it (opt-in).
 *   - Backend has its own soft default (120 min / 100 calls / 10 revisits)
 *     applied when the user leaves a field blank — this UI exposes those
 *     numbers as placeholders for transparency.
 */
import { useState } from 'react';
import { Shield, ChevronDown, ChevronRight, Info } from 'lucide-react';
import type { WorkflowGuards } from '../../types/generated';

interface ExecutionLimitsCardProps {
  /** Current values (null/undefined = "use default"). */
  value: WorkflowGuards | null | undefined;
  onChange: (next: WorkflowGuards | null) => void;
  /** i18n function from the parent (passed through to keep this component
   *  testable in isolation without the I18nContext). */
  t: (key: string, ...args: (string | number)[]) => string;
}

const DEFAULT_TIMEOUT_MIN = 120;
const DEFAULT_MAX_LLM = 100;
const DEFAULT_LOOP_REVISITS = 10;

/** Convert seconds → minutes for the UI input, with safe fallback. */
function secondsToMinutes(s: number | null | undefined): string {
  if (s == null) return '';
  return Math.round(s / 60).toString();
}

function minutesToSeconds(m: string): number | null {
  const n = parseInt(m, 10);
  return Number.isFinite(n) && n > 0 ? n * 60 : null;
}

function intOrNull(s: string): number | null {
  const n = parseInt(s, 10);
  return Number.isFinite(n) && n > 0 ? n : null;
}

export function ExecutionLimitsCard({ value, onChange, t }: ExecutionLimitsCardProps) {
  // Inputs are strings (controlled) so the user can type "" to reset.
  const [timeoutMin, setTimeoutMin] = useState(() => secondsToMinutes(value?.timeout_seconds));
  const [maxCalls, setMaxCalls] = useState(() => value?.max_llm_calls?.toString() ?? '');
  const [maxRevisits, setMaxRevisits] = useState(() => value?.loop_detection_max_revisits?.toString() ?? '');

  const hasOverrides = Boolean(timeoutMin || maxCalls || maxRevisits);
  const [expanded, setExpanded] = useState(hasOverrides);

  const sync = (nextTimeout?: string, nextCalls?: string, nextRevisits?: string) => {
    const tm = nextTimeout ?? timeoutMin;
    const mc = nextCalls ?? maxCalls;
    const mr = nextRevisits ?? maxRevisits;
    const next: WorkflowGuards = {
      timeout_seconds: minutesToSeconds(tm),
      max_llm_calls: intOrNull(mc),
      loop_detection_max_revisits: intOrNull(mr),
    };
    // If everything is null, send null (= "no overrides, use defaults")
    // so the backend stores absence-of-config rather than an empty struct.
    const allEmpty = next.timeout_seconds == null
      && next.max_llm_calls == null
      && next.loop_detection_max_revisits == null;
    onChange(allEmpty ? null : next);
  };

  const summary = hasOverrides
    ? t('wf.guards.summaryActive')
    : t('wf.guards.summaryDefaults', DEFAULT_TIMEOUT_MIN, DEFAULT_MAX_LLM, DEFAULT_LOOP_REVISITS);

  return (
    <div className="wf-section" style={{ marginTop: 16 }}>
      <button
        type="button"
        onClick={() => setExpanded(e => !e)}
        style={{
          all: 'unset',
          cursor: 'pointer',
          display: 'flex',
          alignItems: 'center',
          gap: 8,
          width: '100%',
          padding: '8px 0',
          fontSize: '0.95em',
          fontWeight: 600,
        }}
      >
        {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <Shield size={14} />
        <span>{t('wf.guards.title')}</span>
        <span className="text-muted" style={{ fontSize: '0.85em', fontWeight: 400, marginLeft: 8 }}>
          {summary}
        </span>
      </button>

      {expanded && (
        <div style={{ paddingLeft: 22, paddingTop: 8, display: 'flex', flexDirection: 'column', gap: 12 }}>
          <p className="text-muted" style={{ fontSize: '0.85em', margin: 0 }}>
            <Info size={11} style={{ verticalAlign: 'text-bottom', marginRight: 4 }} />
            {t('wf.guards.description')}
          </p>

          <div className="flex-row gap-3" style={{ alignItems: 'center', flexWrap: 'wrap' }}>
            <label style={{ minWidth: 160, fontSize: '0.9em' }}>
              {t('wf.guards.timeoutLabel')}
              <span className="text-muted" style={{ fontSize: '0.8em', marginLeft: 4 }}>
                {t('wf.guards.timeoutUnit')}
              </span>
            </label>
            <input
              type="number"
              min="1"
              className="wf-input"
              style={{ width: 100 }}
              value={timeoutMin}
              placeholder={DEFAULT_TIMEOUT_MIN.toString()}
              onChange={e => { setTimeoutMin(e.target.value); sync(e.target.value, undefined, undefined); }}
            />
            <span className="text-muted" style={{ fontSize: '0.85em' }}>
              {t('wf.guards.timeoutHint', DEFAULT_TIMEOUT_MIN)}
            </span>
          </div>

          <div className="flex-row gap-3" style={{ alignItems: 'center', flexWrap: 'wrap' }}>
            <label style={{ minWidth: 160, fontSize: '0.9em' }}>
              {t('wf.guards.maxLlmCallsLabel')}
            </label>
            <input
              type="number"
              min="1"
              className="wf-input"
              style={{ width: 100 }}
              value={maxCalls}
              placeholder={DEFAULT_MAX_LLM.toString()}
              onChange={e => { setMaxCalls(e.target.value); sync(undefined, e.target.value, undefined); }}
            />
            <span className="text-muted" style={{ fontSize: '0.85em' }}>
              {t('wf.guards.maxLlmCallsHint', DEFAULT_MAX_LLM)}
            </span>
          </div>

          <div className="flex-row gap-3" style={{ alignItems: 'center', flexWrap: 'wrap' }}>
            <label style={{ minWidth: 160, fontSize: '0.9em' }}>
              {t('wf.guards.loopDetectionLabel')}
            </label>
            <input
              type="number"
              min="1"
              className="wf-input"
              style={{ width: 100 }}
              value={maxRevisits}
              placeholder={DEFAULT_LOOP_REVISITS.toString()}
              onChange={e => { setMaxRevisits(e.target.value); sync(undefined, undefined, e.target.value); }}
            />
            <span className="text-muted" style={{ fontSize: '0.85em' }}>
              {t('wf.guards.loopDetectionHint', DEFAULT_LOOP_REVISITS)}
            </span>
          </div>
        </div>
      )}
    </div>
  );
}
