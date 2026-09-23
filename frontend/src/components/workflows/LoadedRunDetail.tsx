import { useEffect, useState } from 'react';
import { workflows as workflowsApi } from '../../lib/api';
import { useT } from '../../lib/I18nContext';
import type { WorkflowRun } from '../../types/generated';
import { RunDetail, type RunDetailProps } from './RunDetail';

/** Run lists omit outputs. Load the full row only when its panel is mounted. */
export function LoadedRunDetail(props: RunDetailProps) {
  const { run } = props;
  const { t } = useT();
  const [attempt, setAttempt] = useState(0);
  const [result, setResult] = useState<{
    source: WorkflowRun;
    attempt: number;
    detail?: WorkflowRun;
    error?: string;
  } | null>(null);
  const needsDetail = run.step_results.some(step => step.output === '');
  const current = result?.source === run && result.attempt === attempt ? result : null;

  useEffect(() => {
    if (!needsDetail) return;
    let active = true;
    void (async () => {
      try {
        const detail = await workflowsApi.getRun(run.workflow_id, run.id);
        if (active) setResult({ source: run, attempt, ...(detail ? { detail } : { error: '' }) });
      } catch (error) {
        if (active) setResult({ source: run, attempt, error: error instanceof Error ? error.message : String(error) });
      }
    })();
    return () => { active = false; };
  }, [run, needsDetail, attempt]);

  return <>
    {needsDetail && !current && <p role="status" className="text-xs text-ghost">{t('common.loading')}</p>}
    {needsDetail && current?.error !== undefined && <div role="alert" className="text-sm">
      <p>{t('wf.runDetailError')}{current.error && ` — ${current.error}`}</p>
      <button type="button" className="wf-small-btn" onClick={() => setAttempt(value => value + 1)}>
        {t('wf.runDetailRetry')}
      </button>
    </div>}
    <RunDetail {...props} run={current?.detail ?? run} />
  </>;
}
