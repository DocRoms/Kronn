/** KT-1449 — the views of a run row that pairs a normal run with its shadow. */
import { useRef, type KeyboardEvent } from 'react';
import { useT } from '../../lib/I18nContext';
import { pairPanelId, pairTabId, type PairView } from '../../lib/shadowRuns';

const VIEWS: readonly PairView[] = ['normal', 'compare', 'shadow'];

interface Props {
  runId: string;
  view: PairView;
  onChange: (view: PairView) => void;
}

export function RunPairTabs({ runId, view, onChange }: Props) {
  const { t } = useT();
  const refs = useRef<Partial<Record<PairView, HTMLButtonElement | null>>>({});
  // Roving tabindex: arrows, Home and End move between the views (WAI-ARIA tabs).
  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    const index = VIEWS.indexOf(view);
    const next = event.key === 'ArrowRight' ? VIEWS[(index + 1) % VIEWS.length]
      : event.key === 'ArrowLeft' ? VIEWS[(index + VIEWS.length - 1) % VIEWS.length]
        : event.key === 'Home' ? VIEWS[0]
          : event.key === 'End' ? VIEWS[VIEWS.length - 1] : null;
    if (!next) return;
    event.preventDefault();
    onChange(next);
    refs.current[next]?.focus();
  };
  return (
    <div className="wf-runs-filter wf-run-pair-tabs" role="tablist" aria-label={t('wf.shadow.viewLabel')} data-testid="wf-run-pair-tabs">
      {VIEWS.map(v => (
        <button
          key={v}
          ref={el => { refs.current[v] = el; }}
          type="button"
          role="tab"
          id={pairTabId(runId, v)}
          aria-controls={pairPanelId(runId)}
          aria-selected={view === v}
          tabIndex={view === v ? 0 : -1}
          className="wf-runs-filter-btn"
          data-active={view === v}
          data-testid={`wf-run-pair-tab-${v}`}
          onClick={() => onChange(v)}
          onKeyDown={onKeyDown}
        >{t(`wf.shadow.view.${v}`)}</button>
      ))}
    </div>
  );
}
