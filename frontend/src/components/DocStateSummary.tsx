import { AlertTriangle, FileCheck2 } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { docStateNeedsAttention, type DocStateCounts, type DocStateKey } from './docState';
import './DocStateSummary.css';

interface DocStateAction {
  label: string;
  run: () => void;
  disabled?: boolean;
}

interface DocStateSummaryProps {
  counts: DocStateCounts | null;
  onOpen?: () => void;
  variant?: 'summary' | 'block';
  actions?: Partial<Record<DocStateKey, DocStateAction>>;
}

const COUNT_KEYS: DocStateKey[] = ['broken', 'reanchor', 'stale', 'reconfirm'];

export function DocStateSummary({ counts, onOpen, variant = 'summary', actions }: DocStateSummaryProps) {
  const { t } = useT();
  const needsAttention = docStateNeedsAttention(counts);
  const total = counts
    ? COUNT_KEYS.reduce((sum, key) => sum + counts[key], 0)
    : 0;

  if (variant === 'summary') {
    return (
      <button
        type="button"
        className="doc-state-summary"
        data-tone={counts === null ? 'unknown' : needsAttention ? 'warning' : 'success'}
        data-testid="project-doc-state-summary"
        onClick={(event) => {
          event.stopPropagation();
          onOpen?.();
        }}
      >
        {counts === null
          ? <AlertTriangle size={11} aria-hidden="true" />
          : needsAttention
            ? <AlertTriangle size={11} aria-hidden="true" />
            : <FileCheck2 size={11} aria-hidden="true" />}
        {counts === null
          ? t('docState.summaryUnknown')
          : needsAttention
            ? t('docState.summaryAttention', total)
            : t('docState.summaryClean')}
      </button>
    );
  }

  return (
    <section className="doc-state-block" data-testid="doc-state-block" data-tone={counts === null ? 'unknown' : needsAttention ? 'warning' : 'success'}>
      <div className="doc-state-block-heading">
        <div>
          <h3>{t('docState.title')}</h3>
          <p>{counts === null ? t('docState.unknown') : t('docState.explanation')}</p>
        </div>
        {counts === null
          ? <AlertTriangle size={14} aria-hidden="true" />
          : needsAttention
            ? <AlertTriangle size={14} aria-hidden="true" />
            : <FileCheck2 size={14} aria-hidden="true" />}
      </div>
      <div className="doc-state-counts">
        {COUNT_KEYS.map(key => {
          const action = counts !== null && counts[key] > 0 ? actions?.[key] : undefined;
          const content = (
            <>
              <strong>{counts === null ? '—' : counts[key]}</strong>
              <span>{t(`docState.count.${key}`)}</span>
              {action && <span className="doc-state-count-action">{action.label}</span>}
            </>
          );
          return action ? (
            <button
              key={key}
              type="button"
              className="doc-state-count"
              data-testid={`doc-state-${key}`}
              disabled={action.disabled}
              onClick={action.run}
            >
              {content}
            </button>
          ) : (
            <div key={key} className="doc-state-count" data-testid={`doc-state-${key}`}>
              {content}
            </div>
          );
        })}
      </div>
    </section>
  );
}
