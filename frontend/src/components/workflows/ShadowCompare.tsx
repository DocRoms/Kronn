/**
 * KT-1449 — a normal run and its shadow, step by step: a verdict per step
 * (identical, different, skipped, absent), each side's status, duration and
 * tokens, and a line diff where the outputs differ.
 */
import { useEffect, useMemo, useState } from 'react';
import { ExternalLink, FlaskConical } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { workflows as workflowsApi } from '../../lib/api';
import { diffLines } from '../../lib/qp-history-diff';
import { formatDurationCompact } from '../../lib/kronnToolParser';
import { workflowPath } from '../../lib/routes';
import { clip, compareRuns, needsRunDetail, readableOutput, runStatusKey, type ComparedStep } from '../../lib/shadowRuns';
import { AppLink } from '../AppLink';
import type { StepResult, WorkflowRun } from '../../types/generated';
import '../QPHistoryDrawer.css';

const MAX_OUTPUT = 6000;
type Translate = (key: string, ...args: (string | number)[]) => string;

/** The full run when the list only carries its summary. */
function useFullRun(run: WorkflowRun): { run: WorkflowRun | null; error: string | null } {
  const [loaded, setLoaded] = useState<{ source: WorkflowRun; run: WorkflowRun | null; error: string | null } | null>(null);
  const needed = needsRunDetail(run);
  useEffect(() => {
    if (!needed) return;
    let active = true;
    workflowsApi.getRun(run.workflow_id, run.id)
      .then(detail => { if (active) setLoaded({ source: run, run: detail ?? null, error: detail ? null : '' }); })
      .catch(e => { if (active) setLoaded({ source: run, run: null, error: String((e as Error).message ?? e) }); });
    return () => { active = false; };
  }, [run, needed]);
  if (!needed) return { run, error: null };
  const current = loaded?.source === run ? loaded : null;
  return { run: current?.run ?? null, error: current?.error ?? null };
}

function percent(from: number, to: number): string {
  if (from <= 0) return '';
  const delta = Math.round(((to - from) / from) * 100);
  return delta === 0 ? ' (=)' : ` (${delta > 0 ? '+' : '−'}${Math.abs(delta)} %)`;
}

function Side({ step, t }: { step?: StepResult; t: Translate }) {
  if (!step) return <span className="wf-shadow-cmp-side text-muted">—</span>;
  return (
    <span className="wf-shadow-cmp-side" data-status={step.status}>
      <span className="wf-shadow-cmp-status">{t(runStatusKey(step.status))}</span>
      <span className="text-muted">{formatDurationCompact(step.duration_ms)}</span>
      {typeof step.tokens_used === 'number' && step.tokens_used > 0 && (
        <span className="text-muted">{step.tokens_used.toLocaleString()} {t('wf.stepTokensSuffix')}</span>
      )}
    </span>
  );
}

function Output({ step, t }: { step: StepResult; t: Translate }) {
  const { text, clipped } = clip(readableOutput(step.output), MAX_OUTPUT);
  return (
    <>
      <pre className="wf-shadow-cmp-output" tabIndex={0}>{text}</pre>
      {clipped && <p className="text-muted wf-shadow-cmp-note">{t('wf.shadow.compareClipped', MAX_OUTPUT)}</p>}
    </>
  );
}

function StepRow({ step, t }: { step: ComparedStep; t: Translate }) {
  const { normal, shadow, verdict } = step;
  const shown = normal ?? shadow;
  const rows = useMemo(() => (verdict === 'different' && normal && shadow
    ? diffLines(clip(readableOutput(normal.output), MAX_OUTPUT).text, clip(readableOutput(shadow.output), MAX_OUTPUT).text)
    : []), [verdict, normal, shadow]);
  return (
    <li className="wf-shadow-cmp-step" data-verdict={verdict} data-testid={`wf-shadow-compare-${step.name}`}>
      <div className="wf-shadow-cmp-step-head">
        <span className="wf-shadow-cmp-name">
          {step.name}
          {step.occurrences > 1 && <span className="text-muted"> · {t('wf.shadow.compareOccurrence', step.occurrence, step.occurrences)}</span>}
        </span>
        <span className="wf-shadow-cmp-verdict" data-verdict={verdict}>{t(`wf.shadow.verdict.${verdict}`)}</span>
        <span className="wf-shadow-cmp-sides">
          <span className="wf-shadow-cmp-lane">{t('wf.shadow.compareNormal')}</span>
          <Side step={normal} t={t} />
          <span className="wf-shadow-cmp-lane">{t('wf.shadow.compareShadow')}</span>
          {verdict === 'skipped' ? <span className="wf-shadow-cmp-side text-muted">{t('wf.shadow.compareSkippedShort')}</span> : <Side step={shadow} t={t} />}
        </span>
      </div>
      {verdict === 'different' && (
        <div className="qph-diff wf-shadow-cmp-diff" data-testid={`wf-shadow-diff-${step.name}`}>
          <div className="qph-diff-col qph-diff-col-prev">
            <header>{t('wf.shadow.compareNormal')}</header>
            {rows.map((row, i) => <div key={i} className={`qph-diff-line qph-diff-line-${row.kind}`}>{row.prev || ' '}</div>)}
          </div>
          <div className="qph-diff-col qph-diff-col-next">
            <header>{t('wf.shadow.compareShadow')}</header>
            {rows.map((row, i) => <div key={i} className={`qph-diff-line qph-diff-line-${row.kind}`}>{row.next || ' '}</div>)}
          </div>
        </div>
      )}
      {verdict === 'skipped' && <p className="wf-shadow-cmp-note">{t('wf.shadow.compareSkipped')}</p>}
      {verdict === 'missing' && (
        <p className="wf-shadow-cmp-note">{t(normal ? 'wf.shadow.compareMissingShadow' : 'wf.shadow.compareMissingNormal')}</p>
      )}
      {verdict !== 'different' && shown && (
        <details className="wf-shadow-cmp-more">
          <summary>{t(verdict === 'same' ? 'wf.shadow.compareShowOutput' : 'wf.shadow.compareShowNormalOutput')}</summary>
          <Output step={shown} t={t} />
        </details>
      )}
    </li>
  );
}

interface Props {
  normal: WorkflowRun;
  shadow: WorkflowRun;
  onOpenRun?: (workflowId: string, runId: string) => void;
}

export function ShadowCompare({ normal: normalSummary, shadow: shadowSummary, onOpenRun }: Props) {
  const { t } = useT();
  const normal = useFullRun(normalSummary);
  const shadow = useFullRun(shadowSummary);
  const [onlyDifferences, setOnlyDifferences] = useState(false);
  const comparison = useMemo(
    () => (normal.run && shadow.run ? compareRuns(normal.run, shadow.run) : null),
    [normal.run, shadow.run],
  );
  const error = normal.error ?? shadow.error;
  const visible = (comparison?.steps ?? [])
    .filter(step => !onlyDifferences || step.verdict === 'different' || step.verdict === 'missing');
  const running = ['Pending', 'Running'].includes(shadowSummary.status);
  const link = (run: WorkflowRun, label: string) => (
    <AppLink
      to={workflowPath(run.workflow_id, run.id)}
      onNavigate={onOpenRun ? () => onOpenRun(run.workflow_id, run.id) : undefined}
      className="wf-shadow-cmp-link"
    >
      {label} <ExternalLink size={11} aria-hidden />
    </AppLink>
  );

  return (
    <section className="wf-shadow-cmp" data-testid="wf-shadow-compare" aria-label={t('wf.shadow.compareTitle')}>
      <header className="wf-shadow-cmp-head">
        <strong className="wf-shadow-cmp-title"><FlaskConical size={13} aria-hidden /> {t('wf.shadow.compareTitle')}</strong>
        <span className="wf-shadow-cmp-links">
          {link(normalSummary, t('wf.shadow.openNormal'))}
          {link(shadowSummary, t('wf.shadow.openShadow'))}
        </span>
      </header>
      {running && <p className="wf-shadow-cmp-note" role="status">{t('wf.shadow.compareRunning')}</p>}
      {error !== null && <p className="wf-shadow-cmp-note" role="alert">{t('wf.runDetailError')}{error && ` — ${error}`}</p>}
      {!comparison && error === null && <p className="text-muted wf-shadow-cmp-note" role="status">{t('common.loading')}</p>}
      {comparison && (
        <>
          <div className="wf-shadow-cmp-summary" data-testid="wf-shadow-compare-summary">
            {(['different', 'same', 'skipped', 'missing'] as const).filter(v => comparison.counts[v] > 0).map(v => (
              <span key={v} className="wf-shadow-cmp-verdict" data-verdict={v}>{t(`wf.shadow.count.${v}`, comparison.counts[v])}</span>
            ))}
            <span className="text-muted">
              {t('wf.shadow.compareTokens', comparison.tokens.normal.toLocaleString(), comparison.tokens.shadow.toLocaleString())}
              {percent(comparison.tokens.normal, comparison.tokens.shadow)}
              {' · '}
              {t('wf.shadow.compareDuration', formatDurationCompact(comparison.durationMs.normal), formatDurationCompact(comparison.durationMs.shadow))}
            </span>
            {comparison.counts.same + comparison.counts.skipped > 0 && (
              <label className="wf-shadow-cmp-filter">
                <input type="checkbox" checked={onlyDifferences} onChange={e => setOnlyDifferences(e.target.checked)} />
                {t('wf.shadow.onlyDifferences')}
              </label>
            )}
          </div>
          {visible.length === 0
            ? <p className="wf-shadow-cmp-note">{t('wf.shadow.compareNoDifference')}</p>
            : <ol className="wf-shadow-cmp-steps">{visible.map(step => <StepRow key={step.key} step={step} t={t} />)}</ol>}
        </>
      )}
    </section>
  );
}
