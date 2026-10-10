import { useState } from 'react';
import { Eraser, Save, Trash2 } from 'lucide-react';
import { pages as pagesApi } from '../lib/api';
import { ApiRequestError } from '../lib/apiRequestError';
import { useT } from '../lib/I18nContext';
import { useAsyncGuard } from '../hooks/useAsyncGuard';
import { userError } from '../lib/userError';
import type { LivePageDatasetUsage, LivePageDatasetView } from '../types/generated';
import './LivePageDatasetControls.css';

/** One line per fact the deletion check reads: writers, HTML, buttons. */
export function LivePageDatasetUsageLine({ dataset, usage }: {
  dataset: Pick<LivePageDatasetView, 'updated_at'>;
  usage: LivePageDatasetUsage | undefined;
}) {
  const { locale, t } = useT();
  const writers = usage?.writers.map(writer => writer.workflow_name).join(', ') ?? '';
  return (
    <span className="live-page-dataset-usage">
      <span>{t('pages.dataset.lastWrite', new Date(dataset.updated_at).toLocaleString(locale))}</span>
      {usage && (
        <>
          <span>{writers ? t('pages.dataset.writers', writers) : t('pages.dataset.noWriter')}</span>
          <span>{usage.html_referenced ? t('pages.dataset.htmlReferenced') : t('pages.dataset.htmlUnreferenced')}</span>
          {usage.action_refs.length > 0 && <span>{t('pages.dataset.buttons', usage.action_refs.join(', '))}</span>}
        </>
      )}
    </span>
  );
}

interface LivePageDatasetControlsProps {
  pageId: string;
  dataset: LivePageDatasetView;
  /** Reload the Page after a change. */
  onChanged: () => Promise<void>;
  onDeleted: () => void;
}

/** Empty, delete and re-limit one dataset, each behind a confirmation. */
export function LivePageDatasetControls({ pageId, dataset, onChanged, onDeleted }: LivePageDatasetControlsProps) {
  const { t } = useT();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [maxPoints, setMaxPoints] = useState(String(dataset.max_points));
  const [maxAgeDays, setMaxAgeDays] = useState(dataset.max_age_days == null ? '' : String(dataset.max_age_days));

  const run = async (action: () => Promise<boolean>) => {
    setBusy(true);
    setError(null);
    try {
      if (await action()) await onChanged();
    } catch (cause) {
      setError(userError(cause));
    } finally {
      setBusy(false);
    }
  };

  const clear = useAsyncGuard(() => run(async () => {
    if (!window.confirm(t('pages.dataset.confirmClear', dataset.name))) return false;
    await pagesApi.publish(pageId, {
      workflow_id: null,
      workflow_run_id: null,
      writes: [{ dataset: dataset.name, operation: 'clear', value: null, observed_at: null, dedupe_key: null, key_field: null }],
    });
    return true;
  }));

  const remove = useAsyncGuard(() => run(async () => {
    if (!window.confirm(t('pages.dataset.confirmDelete', dataset.name))) return false;
    try {
      await pagesApi.deleteDataset(pageId, dataset.name);
    } catch (cause) {
      // Still in use: the refusal lists the references; going past it is a second, explicit choice.
      if (!(cause instanceof ApiRequestError) || cause.code !== 'conflict') throw cause;
      if (!window.confirm(t('pages.dataset.confirmForce', cause.message))) return false;
      await pagesApi.deleteDataset(pageId, dataset.name, true);
    }
    onDeleted();
    return true;
  }));

  const saveLimits = useAsyncGuard(() => run(async () => {
    const points = Number(maxPoints);
    const days = maxAgeDays.trim() === '' ? null : Number(maxAgeDays);
    const valid = (value: number) => Number.isInteger(value) && value > 0;
    if (!valid(points) || (days !== null && !valid(days))) {
      setError(t('pages.dataset.limitsInvalid'));
      return false;
    }
    if (!window.confirm(t('pages.dataset.confirmLimits', dataset.name))) return false;
    await pagesApi.updateDataset(pageId, dataset.name, { max_points: points, max_age_days: days });
    return true;
  }));

  return (
    <div className="live-page-dataset-controls">
      {dataset.kind === 'time_series' && (
        <form
          className="live-page-dataset-limits"
          noValidate
          onSubmit={event => { event.preventDefault(); void saveLimits(); }}
        >
          <label>
            {t('pages.dataset.maxPoints')}
            <input type="number" min={1} step={1} value={maxPoints} onChange={event => setMaxPoints(event.target.value)} />
          </label>
          <label>
            {t('pages.dataset.maxAgeDays')}
            <input type="number" min={1} step={1} value={maxAgeDays} onChange={event => setMaxAgeDays(event.target.value)} />
          </label>
          <button type="submit" disabled={busy}><Save size={13} />{t('pages.dataset.saveLimits')}</button>
        </form>
      )}
      <button type="button" onClick={() => void clear()} disabled={busy}><Eraser size={13} />{t('pages.dataset.clear')}</button>
      <button type="button" className="live-page-dataset-delete" onClick={() => void remove()} disabled={busy}>
        <Trash2 size={13} />{t('pages.dataset.delete')}
      </button>
      {error && <p className="live-page-dataset-error" role="alert">{error}</p>}
    </div>
  );
}
