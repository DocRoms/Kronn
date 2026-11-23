/**
 * KT-1449, KT-1459 — per step that ran shadows: the main execution beside each
 * shadow. A header says what differs between them (agent, model, prompt
 * version, skill versions) and groups what is identical; tokens, cost and
 * duration of each; the output of each shadow against the main one, or of any
 * two side by side; and for the shadows identical to the step, how many
 * distinct outputs there are out of N+1.
 */
import { useCallback, useMemo, useState, type ReactNode } from 'react';
import { FlaskConical } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { diffLines } from '../../lib/qp-history-diff';
import { formatDurationCompact } from '../../lib/kronnToolParser';
import {
  clip, comparableOutput, compareIdentities, determinism, executionKey, fieldValue, formatCost, hasOutput,
  mergeExecutionLabel, mergeStatusKey, mergeTally, percent, readableFieldValue, readableOutput, shadowLabel, shadowStatusKey,
  type IdentityField, type IdentityNames,
} from '../../lib/shadowRuns';
import type { ShadowExecution, ShadowMergeReport, StepShadowComparison } from '../../types/generated';
import { useStepShadows } from './useStepShadows';
import { ExecutionArtifact, StepArtifactMosaic, type StepArtifactContext } from './StepArtifactControls';
import '../QPHistoryDrawer.css';

const MAX_OUTPUT = 6000;
type Translate = (key: string, ...args: (string | number)[]) => string;

function DiffBlock({ left, right, leftLabel, rightLabel, testId }: {
  left: ShadowExecution; right: ShadowExecution; leftLabel: string; rightLabel: string; testId: string;
}) {
  const rows = useMemo(
    () => diffLines(clip(readableOutput(left.output), MAX_OUTPUT).text, clip(readableOutput(right.output), MAX_OUTPUT).text),
    [left.output, right.output],
  );
  return (
    <div className="qph-diff wf-shadow-cmp-diff" data-testid={testId}>
      <div className="qph-diff-col qph-diff-col-prev">
        <header>{leftLabel}</header>
        {rows.map((row, i) => <div key={i} className={`qph-diff-line qph-diff-line-${row.kind}`}>{row.prev || ' '}</div>)}
      </div>
      <div className="qph-diff-col qph-diff-col-next">
        <header>{rightLabel}</header>
        {rows.map((row, i) => <div key={i} className={`qph-diff-line qph-diff-line-${row.kind}`}>{row.next || ' '}</div>)}
      </div>
    </div>
  );
}

/** What one execution shows: its name, how it ended, what differs, what it spent. */
function ExecutionHeader({ step, execution, main, differing, label, names, t, testId, artifact }: {
  step: string;
  execution: ShadowExecution;
  main: ShadowExecution;
  differing: readonly IdentityField[];
  label: string;
  names: IdentityNames;
  t: Translate;
  testId: string;
  /** KT-1990 — the execution's Artifact, when the view can make one. */
  artifact?: ReactNode;
}) {
  const isMain = execution.shadow_id == null;
  return (
    <li className="wf-shadow-exec" data-testid={`${testId}-${step}-${isMain ? 'main' : execution.shadow_id}`} data-status={execution.status}>
      <div className="wf-shadow-exec-head">
        <strong className="wf-shadow-exec-name">{label}</strong>
        <span className="wf-shadow-exec-status" data-status={execution.status}>{t(shadowStatusKey(execution.status))}</span>
      </div>
      {differing.length > 0 && (
        <dl className="wf-shadow-exec-diff" data-testid={`${testId}-${step}-${isMain ? 'main' : execution.shadow_id}-differs`}>
          {differing.map(field => (
            <div key={field} className="wf-shadow-exec-field" data-field={field}>
              <dt className="text-muted">{t(`wf.shadow.compare.field.${field}`)}</dt>
              <dd title={fieldValue(execution, field)}>{readableFieldValue(execution, field, names, t('wf.shadow.compare.promptWritten')) || '—'}</dd>
            </div>
          ))}
        </dl>
      )}
      <p className="wf-shadow-exec-metrics" data-testid={`${testId}-${step}-${isMain ? 'main' : execution.shadow_id}-metrics`}>
        <span>{t('wf.shadow.compare.tokens')} <b>{execution.tokens_used != null ? execution.tokens_used.toLocaleString() : '—'}</b>
          {!isMain && <span className="text-muted">{percent(main.tokens_used, execution.tokens_used)}</span>}</span>
        <span>{t('wf.shadow.compare.cost')} <b>{formatCost(execution.cost_usd)}</b></span>
        <span>{t('wf.shadow.compare.duration')} <b>{execution.duration_ms != null ? formatDurationCompact(execution.duration_ms) : '—'}</b>
          {!isMain && <span className="text-muted">{percent(main.duration_ms, execution.duration_ms)}</span>}</span>
      </p>
      {execution.stopped && <p className="wf-shadow-cmp-note" role="note">{execution.stopped}</p>}
      {artifact && <p className="wf-shadow-exec-artifact">{artifact}</p>}
    </li>
  );
}

/**
 * KT-1544 — the merge of the step's executions: how it stands, which shadows
 * are missing, what each execution brought (kept, duplicate, dropped as
 * unverified), the provenance of every point, and the merged result set against
 * each execution.
 */
function MergeSection({ comparison, merge, t }: { comparison: StepShadowComparison; merge: ShadowMergeReport; t: Translate }) {
  const { main, shadows, step_name: step } = comparison;
  const mainLabel = t('wf.shadow.compare.main');
  const name = (token: string) => mergeExecutionLabel(token, comparison, mainLabel);
  const merged: ShadowExecution = { ...main, shadow_id: undefined, output: merge.merged_output };
  const executions = [main, ...shadows].filter(hasOutput);
  const published = merge.status === 'merged';
  const tally = mergeTally(merge);
  return (
    <div className="wf-shadow-merge-result" data-testid={`wf-shadow-merge-${step}`} data-status={merge.status}>
      <p className="wf-shadow-subtitle">{t('wf.shadow.merge.run.title')}</p>
      <p className="wf-shadow-merge-status" data-testid={`wf-shadow-merge-status-${step}`}>
        <span className="wf-shadow-exec-status" data-status={published ? 'success' : merge.status}>{t(mergeStatusKey(merge.status))}</span>
        {' '}
        <span className="text-muted">
          {t(published ? 'wf.shadow.merge.run.isOutput' : 'wf.shadow.merge.run.mainStands')}
        </span>
      </p>
      {merge.note && <p className="wf-shadow-cmp-note" role="note" data-testid={`wf-shadow-merge-note-${step}`}>{merge.note}</p>}
      {merge.missing.length > 0 && (
        <div data-testid={`wf-shadow-merge-missing-${step}`}>
          <p className="wf-shadow-cmp-note">{t('wf.shadow.merge.run.missing', merge.missing.length)}</p>
          <ul className="wf-shadow-merge-list">
            {merge.missing.map(missing => (
              <li key={missing.shadow_id} data-testid={`wf-shadow-merge-missing-${step}-${missing.shadow_id}`}>
                <strong>{missing.label || missing.shadow_id}</strong> <span className="text-muted">— {missing.reason}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
      {tally.rows.length > 0 && (
        <>
          <p className="wf-shadow-cmp-note" data-testid={`wf-shadow-merge-tally-${step}`}>
            <strong>{t(published ? 'wf.shadow.merge.tally.result' : 'wf.shadow.merge.tally.proposal', tally.result)}</strong>
            {' '}{t('wf.shadow.merge.tally.alone', tally.alone)}
            {' '}{t('wf.shadow.merge.tally.dropped', tally.dropped)}
          </p>
          <table className="wf-shadow-merge-table" data-testid={`wf-shadow-merge-contrib-${step}`}>
            <thead>
              <tr>
                <th scope="col">{t('wf.shadow.merge.col.execution')}</th>
                <th scope="col">{t('wf.shadow.merge.col.raised')}</th>
                <th scope="col">{t('wf.shadow.merge.col.kept')}</th>
                <th scope="col">{t('wf.shadow.merge.col.unique')}</th>
                <th scope="col">{t('wf.shadow.merge.col.dropped')}</th>
              </tr>
            </thead>
            <tbody>
              {tally.rows.map(row => (
                <tr key={row.execution} data-testid={`wf-shadow-merge-contrib-${step}-${row.execution}`}>
                  <th scope="row">{name(row.execution)}</th>
                  <td data-col="raised">{row.raised}</td>
                  <td data-col="kept">{row.inResult}</td>
                  <td data-col="unique" data-brought={row.alone > 0 && row.execution !== 'main'}>{row.alone}</td>
                  <td data-col="dropped" data-warn={row.dropped > 0}>{row.dropped}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
      {merge.points.length > 0 && (
        <details className="wf-shadow-cmp-more" data-testid={`wf-shadow-merge-points-${step}`}>
          <summary>{t('wf.shadow.merge.run.points', merge.points.length)}</summary>
          <ul className="wf-shadow-merge-list">
            {merge.points.map(point => (
              <li key={point.id} data-testid={`wf-shadow-merge-point-${step}-${point.id}`} data-verdict={point.verdict}>
                <span className="wf-shadow-merge-verdict" data-verdict={point.verdict}>{t(`wf.shadow.merge.verdict.${point.verdict}`)}</span>
                {' '}{point.summary}
                <span className="text-muted"> · {t('wf.shadow.merge.run.foundBy', point.found_by.map(name).join(', ') || '—')}</span>
                {(point.evidence || point.reason) && <span className="text-muted"> · {point.evidence || point.reason}</span>}
              </li>
            ))}
          </ul>
        </details>
      )}
      {merge.tokens_used != null && (
        <p className="wf-shadow-exec-metrics" data-testid={`wf-shadow-merge-spent-${step}`}>
          <span>{t('wf.shadow.merge.run.spent')} <b>{merge.tokens_used.toLocaleString()}</b> {t('wf.shadow.compare.tokens').toLowerCase()}</span>
          <span>{t('wf.shadow.compare.cost')} <b>{formatCost(merge.cost_usd)}</b></span>
          {merge.model && <span className="text-muted">{merge.model}</span>}
        </p>
      )}
      {published && executions.length > 0 && (
        <div data-testid={`wf-shadow-merge-vs-${step}`}>
          <p className="wf-shadow-subtitle">{t('wf.shadow.merge.run.vs')}</p>
          {executions.map(execution => {
            const same = comparableOutput(execution.output) === comparableOutput(merged.output);
            return (
              <details key={executionKey(execution) || 'main'} className="wf-shadow-cmp-more" data-testid={`wf-shadow-merge-vs-${step}-${execution.shadow_id ?? 'main'}`} data-same={same}>
                <summary>
                  {t('wf.shadow.merge.run.mergedVs', shadowLabel(execution, mainLabel))}{' '}
                  <span className="wf-shadow-cmp-verdict" data-verdict={same ? 'same' : 'different'}>{t(`wf.shadow.compare.verdict.${same ? 'same' : 'different'}`)}</span>
                </summary>
                {!same && (
                  <DiffBlock
                    left={execution}
                    right={merged}
                    leftLabel={shadowLabel(execution, mainLabel)}
                    rightLabel={t('wf.shadow.merge.run.merged')}
                    testId={`wf-shadow-merge-diff-${step}-${execution.shadow_id ?? 'main'}`}
                  />
                )}
              </details>
            );
          })}
        </div>
      )}
    </div>
  );
}

function StepComparison({ comparison, names, t, artifacts }: {
  comparison: StepShadowComparison; names: IdentityNames; t: Translate; artifacts?: StepArtifactContext;
}) {
  const { main, shadows, step_name: step } = comparison;
  const executions = useMemo(() => [main, ...shadows], [main, shadows]);
  const mainLabel = t('wf.shadow.compare.main');
  const labelOf = (execution: ShadowExecution) => shadowLabel(execution, mainLabel);
  const { identical, differing } = useMemo(() => compareIdentities(executions), [executions]);
  const twins = useMemo(() => determinism(comparison), [comparison]);
  const withOutput = executions.filter(hasOutput);
  // Any two executions side by side; the main one and the first shadow to start with.
  const [left, setLeft] = useState('');
  const [right, setRight] = useState(shadows[0] ? executionKey(shadows[0]) : '');
  const byKey = (key: string) => executions.find(execution => executionKey(execution) === key);
  const leftExecution = byKey(left) ?? main;
  const rightExecution = byKey(right) ?? shadows[0];

  return (
    <section className="wf-shadow-cmp-step-block" data-testid={`wf-step-shadow-compare-${step}`} aria-label={t('wf.shadow.compare.stepAria', step)}>
      <h4 className="wf-shadow-cmp-step-title">{step}</h4>
      {artifacts && <StepArtifactMosaic step={comparison} context={artifacts} t={t} />}

      <p className="wf-shadow-identical" data-testid={`wf-shadow-identical-${step}`}>
        <span className="text-muted">{t('wf.shadow.compare.identical')}</span>{' '}
        {identical.length === 0
          ? <span className="text-muted">—</span>
          : identical.map(entry => (
            <span key={entry.field} className="wf-shadow-identical-field" data-field={entry.field}>
              {t(`wf.shadow.compare.field.${entry.field}`)} <b title={entry.value}>{readableFieldValue(main, entry.field, names, t('wf.shadow.compare.promptWritten'))}</b>
            </span>
          ))}
      </p>
      {differing.length === 0 && shadows.length > 0 && (
        <p className="text-muted wf-shadow-cmp-note" data-testid={`wf-shadow-nothing-differs-${step}`}>{t('wf.shadow.compare.nothingDiffers')}</p>
      )}

      <ul className="wf-shadow-execs">
        {executions.map(execution => (
          <ExecutionHeader
            key={executionKey(execution) || 'main'}
            step={step}
            execution={execution}
            main={main}
            differing={differing}
            label={labelOf(execution)}
            names={names}
            t={t}
            testId="wf-shadow-exec"
            artifact={artifacts && (
              <ExecutionArtifact
                step={comparison}
                execution={execution}
                context={artifacts}
                t={t}
                testId={`wf-shadow-exec-${step}-${execution.shadow_id ?? 'main'}`}
              />
            )}
          />
        ))}
      </ul>

      {twins && (
        <p className="wf-shadow-determinism" data-testid={`wf-shadow-determinism-${step}`} data-distinct={twins.distinct}>
          <FlaskConical size={12} aria-hidden />{' '}
          {t('wf.shadow.compare.determinism', twins.distinct, twins.total)}
          {twins.missing > 0 && <span className="text-muted"> · {t('wf.shadow.compare.determinismMissing', twins.missing)}</span>}
        </p>
      )}

      {comparison.merge && <MergeSection comparison={comparison} merge={comparison.merge} t={t} />}

      {shadows.length > 0 && hasOutput(main) && (
        <div className="wf-shadow-vs-main" data-testid={`wf-shadow-vs-main-${step}`}>
          <p className="wf-shadow-subtitle">{t('wf.shadow.compare.vsMain')}</p>
          {shadows.map(shadow => {
            const verdict = !hasOutput(shadow) ? 'none'
              : comparableOutput(shadow.output) === comparableOutput(main.output) ? 'same' : 'different';
            return (
              <details
                key={executionKey(shadow)}
                className="wf-shadow-cmp-more"
                open={shadows.length === 1 && verdict === 'different' ? true : undefined}
                data-testid={`wf-shadow-vs-main-${step}-${shadow.shadow_id}`}
                data-verdict={verdict}
              >
                <summary>
                  {labelOf(shadow)}{' '}
                  <span className="wf-shadow-cmp-verdict" data-verdict={verdict}>{t(`wf.shadow.compare.verdict.${verdict}`)}</span>
                </summary>
                {verdict === 'different' && (
                  <DiffBlock left={main} right={shadow} leftLabel={mainLabel} rightLabel={labelOf(shadow)} testId={`wf-shadow-diff-${step}-${shadow.shadow_id}`} />
                )}
                {verdict === 'same' && <p className="wf-shadow-cmp-note">{t('wf.shadow.compare.sameOutput')}</p>}
                {verdict === 'none' && <p className="wf-shadow-cmp-note">{t('wf.shadow.compare.noOutput')}</p>}
              </details>
            );
          })}
        </div>
      )}

      {withOutput.length > 2 && (
        <div className="wf-shadow-cmp-pick" data-testid={`wf-shadow-pick-${step}`}>
          <label>
            <span className="text-muted">{t('wf.shadow.compare.left')}</span>
            <select className="wf-shadow-select" data-testid={`wf-shadow-pick-left-${step}`} value={executionKey(leftExecution)} onChange={e => setLeft(e.target.value)}>
              {withOutput.map(execution => <option key={executionKey(execution)} value={executionKey(execution)}>{labelOf(execution)}</option>)}
            </select>
          </label>
          <label>
            <span className="text-muted">{t('wf.shadow.compare.right')}</span>
            <select className="wf-shadow-select" data-testid={`wf-shadow-pick-right-${step}`} value={rightExecution ? executionKey(rightExecution) : ''} onChange={e => setRight(e.target.value)}>
              {withOutput.filter(execution => execution !== leftExecution).map(execution => (
                <option key={executionKey(execution)} value={executionKey(execution)}>{labelOf(execution)}</option>
              ))}
            </select>
          </label>
          {rightExecution && rightExecution !== leftExecution && hasOutput(leftExecution) && hasOutput(rightExecution) && (
            <DiffBlock left={leftExecution} right={rightExecution} leftLabel={labelOf(leftExecution)} rightLabel={labelOf(rightExecution)} testId={`wf-shadow-pair-diff-${step}`} />
          )}
        </div>
      )}
    </section>
  );
}

/** The run's step shadows; nothing when its steps ran none. */
export function StepShadowCompare({ workflowId, runId, runStatus }: { workflowId: string; runId: string; runStatus: string }) {
  // Keyed by run: a run's view, and a request it has in flight, never serve another.
  return <StepShadowCompareView key={runId} steps={useStepShadows(workflowId, runId, runStatus)} run={{ workflowId, runId }} />;
}

const NO_NAMES: IdentityNames = {};

export function StepShadowCompareView({ steps, names = NO_NAMES, run }: {
  steps: StepShadowComparison[];
  names?: IdentityNames;
  /** KT-1990 — the run, to make an Artifact from each execution; without it the view only compares. */
  run?: { workflowId: string; runId: string };
}) {
  const { t } = useT();
  const workflowId = run?.workflowId;
  const runId = run?.runId;
  // KT-1999 — what was made here shows at once, until the comparison is read
  // again: from then on the server says which Artifacts exist (one deleted
  // elsewhere is offered to be made again). What was made belongs to its run:
  // the view rendered for another run starts empty, and an answer that comes
  // back for the run it left is not written in it.
  const [made, setMade] = useState<{ runId?: string; pages: Record<string, string> }>(() => ({ runId, pages: {} }));
  const [madeSince, setMadeSince] = useState(steps);
  if (madeSince !== steps || made.runId !== runId) {
    setMadeSince(steps);
    setMade({ runId, pages: {} });
  }
  const onMade = useCallback((key: string, pageId: string) => setMade(current => (
    current.runId !== runId || current.pages[key] === pageId
      ? current
      : { runId, pages: { ...current.pages, [key]: pageId } }
  )), [runId]);
  const madePages = made.pages;
  const artifacts = useMemo<StepArtifactContext | undefined>(
    () => workflowId && runId ? { workflowId, runId, made: madePages, onMade } : undefined,
    [workflowId, runId, madePages, onMade],
  );
  if (steps.length === 0) return null;
  return (
    <section className="wf-shadow-cmp" data-testid="wf-step-shadow-compare" aria-label={t('wf.shadow.compare.title')}>
      <header className="wf-shadow-cmp-head">
        <strong className="wf-shadow-cmp-title"><FlaskConical size={13} aria-hidden /> {t('wf.shadow.compare.title')}</strong>
      </header>
      {steps.map(comparison => <StepComparison key={`${comparison.step_name}#${comparison.result_index}`} comparison={comparison} names={names} t={t} artifacts={artifacts} />)}
    </section>
  );
}
