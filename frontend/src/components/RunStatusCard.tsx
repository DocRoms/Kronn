import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, CheckCircle2, Clock3, ExternalLink, Loader2, XCircle } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { formatDurationCompact } from '../lib/kronnToolParser';
import { runsApi } from '../lib/api';
import { useWebSocket } from '../hooks/useWebSocket';
import { mediaRunDetails } from '../lib/mediaRunResult';
import { flatEntries, quickApiPreview } from '../lib/runResultPreview';
import {
  sharedRunStatusCardModel,
  type RunStatusCardModel,
  type RunStatusCardStatus,
} from '../lib/runStatusCardModel';
import './RunStatusCard.css';

function isActive(status: RunStatusCardStatus): boolean {
  return status === 'queued' || status === 'running';
}

function statusIcon(status: RunStatusCardStatus) {
  if (status === 'running' || status === 'queued') return <Loader2 className="spin" size={15} aria-hidden />;
  if (status === 'success') return <CheckCircle2 size={15} aria-hidden />;
  if (status === 'partial' || status === 'preflight_failed' || status === 'timeout') return <AlertTriangle size={15} aria-hidden />;
  return <XCircle size={15} aria-hidden />;
}

function measuredDuration(model: RunStatusCardModel, now: number): number | null {
  if (typeof model.durationMs === 'number' && model.durationMs >= 0) return model.durationMs;
  if (!model.startedAt) return null;
  const start = Date.parse(model.startedAt);
  if (Number.isNaN(start)) return null;
  if (model.finishedAt) {
    const end = Date.parse(model.finishedAt);
    return Number.isNaN(end) ? null : Math.max(0, end - start);
  }
  return isActive(model.status) ? Math.max(0, now - start) : null;
}

interface RunStep {
  name: string;
  kind: string | null;
  status: string;
  durationMs: number | null;
}

/** A workflow's result lists the steps it ran. Read as such, never dumped:
 *  a card that printed `{"progress":…,"steps":[…]}` told a human nothing. */
function workflowSteps(result: unknown): RunStep[] | null {
  const steps = (result as { steps?: unknown } | null)?.steps;
  if (!Array.isArray(steps) || steps.length === 0) return null;
  const read = steps.flatMap(step => {
    const entry = step as { step_name?: unknown; step_kind?: unknown; status?: unknown; duration_ms?: unknown };
    if (typeof entry?.step_name !== 'string' || typeof entry.status !== 'string') return [];
    return [{
      name: entry.step_name,
      kind: typeof entry.step_kind === 'string' ? entry.step_kind : null,
      status: entry.status,
      durationMs: typeof entry.duration_ms === 'number' ? entry.duration_ms : null,
    }];
  });
  return read.length > 0 ? read : null;
}

function stepIcon(status: string) {
  const normalized = status.toLowerCase();
  if (normalized === 'success') return <CheckCircle2 size={12} aria-hidden />;
  if (normalized === 'running' || normalized === 'pending') return <Loader2 className="spin" size={12} aria-hidden />;
  if (normalized === 'skipped') return <span className="run-status-card-step-dot" aria-hidden>–</span>;
  return <XCircle size={12} aria-hidden />;
}

/** What a card will put in the DOM before it stops. A card is a summary and
 *  links to the run for the rest, so there is no size at which dumping more
 *  helps — and an unbounded dump is how a workflow result of several megabytes
 *  froze the tab: pretty-printed, twenty cards at a time. */
const MAX_RESULT_CHARS = 20_000;

function resultText(result: unknown): { text: string; truncated: boolean } | null {
  if (result == null) return null;
  let text: string;
  if (typeof result === 'string') {
    text = result;
  } else {
    try {
      text = JSON.stringify(result, null, 2);
    } catch {
      return null;
    }
  }
  return text.length > MAX_RESULT_CHARS
    ? { text: text.slice(0, MAX_RESULT_CHARS), truncated: true }
    : { text, truncated: false };
}

export function RunStatusCard({ model: initialModel, runId, compact = false, hideRunLink = false }: {
  model?: RunStatusCardModel;
  runId?: string;
  compact?: boolean;
  /** Suppress the "open the run" link. A caller that already offers a better
   *  destination (the media bubble opens the asset itself) must be able to hide
   *  it for good: self-hydration below rebuilds the model from the server, href
   *  included, so a `href: null` passed in props would come back a second later. */
  hideRunLink?: boolean;
}) {
  const { t } = useT();
  const rootRef = useRef<HTMLElement>(null);
  // Start suspended: without a real IntersectionObserver measurement yet, a
  // freshly-mounted card must not assume it is on-screen (DoD #6/#7) — a
  // long timeline mounting N cards would otherwise fire N fetches/sockets
  // before the first layout pass. Only environments without IO support
  // (rare/legacy) fall back to always-visible.
  const [visible, setVisible] = useState(() => typeof IntersectionObserver === 'undefined');
  const [hydrated, setHydrated] = useState<RunStatusCardModel | null>(null);
  const model = hydrated ?? initialModel;
  const [now, setNow] = useState(() => Date.now());
  const active = model ? isActive(model.status) : false;

  useEffect(() => {
    const node = rootRef.current;
    if (!node || typeof IntersectionObserver === 'undefined') return;
    const observer = new IntersectionObserver(entries => setVisible(entries[0]?.isIntersecting ?? false), { rootMargin: '200px' });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  const hydrate = useCallback(async (freshness: RunStatusCardModel['freshness']) => {
    if (!runId || !visible) return;
    try { setHydrated(sharedRunStatusCardModel(await runsApi.get(runId), freshness)); }
    catch { setHydrated(current => current ? { ...current, freshness: 'unavailable' } : null); }
  }, [runId, visible]);
  // KT-587 — `hydrate` awaits its fetch before writing, and the call says so
  // here rather than leaving the reader (and the linter) to trace it.
  useEffect(() => {
    void (async () => { await hydrate('rehydrated'); })();
  }, [hydrate]);
  useWebSocket(message => {
    if (visible && runId && message.type === 'shared_run_updated' && message.run_id === runId) void hydrate('live');
  }, () => { if (visible && runId) void hydrate('rehydrated'); }, Boolean(runId && visible && active));

  // Media cards show no elapsed time (see below), so ticking every second
  // would re-render them for a value nobody reads.
  const ticking = active && model?.kind !== 'media';
  useEffect(() => {
    if (!visible || !ticking || !model?.startedAt) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [model?.startedAt, ticking, visible]);

  const steps = useMemo(() => workflowSteps(model?.result), [model?.result]);
  // A Quick API reads as its summary and first rows, a Quick Exec's flat JSON
  // output as key → value; the raw payload stays one fold away.
  const runKind = model?.kind;
  const runResult = model?.result;
  const apiPreview = useMemo(
    () => runKind === 'quick_api' ? quickApiPreview(runResult) : null,
    [runKind, runResult],
  );
  const execEntries = useMemo(
    () => runKind === 'quick_exec' ? flatEntries(runResult) : null,
    [runKind, runResult],
  );
  const result = useMemo(() => steps ? null : resultText(model?.result), [model?.result, steps]);
  const execStderr = useMemo(() => resultText(model?.execDetails?.stderr), [model?.execDetails?.stderr]);

  if (!model) return <section ref={rootRef} className="run-status-card" data-testid="run-status-card"><span>{t('run.freshness.unavailable')}</span></section>;

  const duration = measuredDuration(model, now);
  const progress = model.progress;
  // Media details only when the run actually carries them: a malformed or
  // absent result degrades to none, never to zeros.
  const media = model.kind === 'media' ? mediaRunDetails(model.result) : null;
  const progressPercent = progress && progress.total > 0
    ? Math.min(100, Math.max(0, (progress.completed / progress.total) * 100))
    : null;
  // A media bubble states no elapsed time and no freshness: the run duration
  // is not the duration of what was produced, and on an image — or a video
  // that failed — the line only ever read "Duration unavailable / Rehydrated
  // from the server", two labels that answer nothing. The duration that means
  // something for a media is the one of the produced file, shown below.
  const showRunMeta = model.kind !== 'media';
  // Media runs publish a full JSON projection the reader rarely needs; it is
  // folded away for them only, so workflow and quick-prompt cards keep the
  // rendering they have today.
  const foldResult = model.kind === 'media' || apiPreview != null || execEntries != null;

  return (
    <section ref={rootRef} className="run-status-card" data-status={model.status} data-kind={model.kind} data-testid="run-status-card">
      <div className="run-status-card-header">
        <span className="run-status-card-kind">{t(`run.kind.${model.kind}`)}</span>
        <span className="run-status-card-status" data-status={model.status}>
          {statusIcon(model.status)} {t(`run.status.${model.status}`)}
        </span>
        {!hideRunLink && model.href && (
          <a className="run-status-card-link" href={model.href} aria-label={t('run.open')}>
            <ExternalLink size={14} aria-hidden />
          </a>
        )}
      </div>
      {!compact && (
        <>
          {showRunMeta && (
            <div className="run-status-card-meta">
              <span><Clock3 size={13} aria-hidden /> {duration == null ? t('run.durationUnavailable') : formatDurationCompact(duration)}</span>
              {model.freshness && <span data-freshness={model.freshness}>{t(`run.freshness.${model.freshness}`)}</span>}
            </div>
          )}
          {progress && progressPercent != null && (
            <div className="run-status-card-progress">
              <div className="run-status-card-progress-label">
                <span>{t('run.progress', progress.completed, progress.total)}</span>
                {progress.currentLabel && <span>{progress.currentLabel}</span>}
              </div>
              <div className="run-status-card-progress-track" role="progressbar" aria-valuenow={progress.completed} aria-valuemax={progress.total}>
                <span style={{ width: `${progressPercent}%` }} />
              </div>
            </div>
          )}
          {media && (
            <div className="run-status-card-media" data-testid="run-status-card-media" data-modality={media.modality}>
              <span>{t(`run.media.${media.modality}`)}</span>
              {media.width && media.height && (
                // Real geometry from the produced file: the provider does not
                // honour the requested resolution.
                <span data-testid="run-status-card-media-size">{media.width}×{media.height}</span>
              )}
              {media.modality === 'video' && media.durationMs && <span>{Math.round(media.durationMs / 1000)}s</span>}
            </div>
          )}
          {model.diagnostic && <p className="run-status-card-diagnostic">{model.diagnostic}</p>}
          {steps && (
            <ol className="run-status-card-steps" data-testid="run-status-card-steps">
              {steps.map((step, index) => {
                const kindKey = `wiz.stepType${step.kind === 'BatchQuickPrompt' ? 'BatchQP' : step.kind}`;
                const kindLabel = step.kind ? (t(kindKey) === kindKey ? step.kind : t(kindKey)) : null;
                return (
                  <li key={`${index}-${step.name}`} data-status={step.status.toLowerCase()}>
                    {stepIcon(step.status)}
                    <span className="run-status-card-step-name">{step.name}</span>
                    {kindLabel && <span className="run-status-card-step-kind">{kindLabel}</span>}
                    {step.durationMs != null && (
                      <span className="run-status-card-step-duration">{formatDurationCompact(step.durationMs)}</span>
                    )}
                  </li>
                );
              })}
            </ol>
          )}
          {apiPreview && (
            <div className="run-status-card-preview" data-testid="run-status-card-api-preview">
              {(apiPreview.summary || apiPreview.status) && (
                <p className="run-status-card-summary" title={apiPreview.summary ?? undefined}>
                  {apiPreview.status && <span className="run-status-card-summary-status">{apiPreview.status}</span>}
                  {apiPreview.summary}
                </p>
              )}
              {apiPreview.table && (
                <div className="run-status-card-table-wrap">
                  <table className="run-status-card-table">
                    <caption>
                      {apiPreview.table.label ? `${apiPreview.table.label} · ` : ''}
                      {t('run.preview.items', apiPreview.table.total)}
                    </caption>
                    <thead>
                      <tr>{apiPreview.table.columns.map(column => <th key={column} scope="col">{column}</th>)}</tr>
                    </thead>
                    <tbody>
                      {apiPreview.table.rows.map((row, index) => (
                        <tr key={index}>
                          {row.map((value, column) => <td key={column} title={value}>{value}</td>)}
                        </tr>
                      ))}
                    </tbody>
                  </table>
                  {apiPreview.table.total > apiPreview.table.rows.length && (
                    <p className="run-status-card-more">
                      {t('run.preview.more', apiPreview.table.total - apiPreview.table.rows.length)}
                    </p>
                  )}
                </div>
              )}
              {apiPreview.list && (
                <>
                  <ul className="run-status-card-list">
                    {apiPreview.list.items.map((item, index) => <li key={index}>{item}</li>)}
                  </ul>
                  {apiPreview.list.total > apiPreview.list.items.length && (
                    <p className="run-status-card-more">
                      {t('run.preview.more', apiPreview.list.total - apiPreview.list.items.length)}
                    </p>
                  )}
                </>
              )}
              {apiPreview.entries && (
                <dl className="run-status-card-entries">
                  {apiPreview.entries.map(([key, value]) => (
                    <div key={key}><dt>{key}</dt><dd>{value}</dd></div>
                  ))}
                </dl>
              )}
            </div>
          )}
          {execEntries && (
            <dl className="run-status-card-entries" data-testid="run-status-card-exec-output">
              {execEntries.map(([key, value]) => (
                <div key={key}><dt>{key}</dt><dd>{value}</dd></div>
              ))}
            </dl>
          )}
          {model.kind === 'quick_exec' && model.execDetails && (
            <div data-testid="run-status-card-exec-diagnostics">
              <span>{model.execDetails.exit_code == null
                ? t('run.exitCodeUnknown')
                : t('run.exitCode', model.execDetails.exit_code)}</span>
              {execStderr?.text && (
                <details className="run-status-card-fold">
                  <summary>{t('run.stderr')}</summary>
                  <pre className="run-status-card-result">
                    {execStderr.text}
                    {execStderr.truncated && `\n${t('run.resultTruncated')}`}
                  </pre>
                </details>
              )}
            </div>
          )}
          {result && (foldResult ? (
            <details className="run-status-card-fold" data-testid="run-status-card-result-fold">
              <summary>{t('run.details')}</summary>
              <pre className="run-status-card-result">
                {result.text}
                {result.truncated && `\n${t('run.resultTruncated')}`}
              </pre>
            </details>
          ) : (
            <pre className="run-status-card-result">
              {result.text}
              {result.truncated && `\n${t('run.resultTruncated')}`}
            </pre>
          ))}
          {/* Last child, aligned right: the price of a generation is what the
              eye looks for once the media is there, not a detail in the header. */}
          {media && media.costUsd != null && (
            <p className="run-status-card-cost" data-testid="run-status-card-media-cost">
              {media.isByok ? t('run.media.byok') : `$${media.costUsd.toFixed(4)}`}
            </p>
          )}
        </>
      )}
    </section>
  );
}
