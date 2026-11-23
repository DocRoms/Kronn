/**
 * KT-1990 — in the run view, per execution of a step (the main one and each
 * shadow): « Make an Artifact » from the HTML document its output holds, then
 * « Open the Artifact »; per step: « Open the renders in a mosaic ». Kronn makes
 * the Artifact from the stored output on this click: a shadow has no Kronn
 * tool and never makes one itself.
 */
import { useState } from 'react';
import { ExternalLink, FilePlus2, LayoutGrid, Loader2 } from 'lucide-react';
import { workflows as workflowsApi } from '../../lib/api';
import { useAsyncGuard } from '../../hooks/useAsyncGuard';
import { standaloneLivePageMosaicUrl, standaloneLivePageUrl } from '../../lib/live-page-navigation';
import { hasOutput } from '../../lib/shadowRuns';
import { artifactKey, htmlStateKey, offersMosaic, renderedExecutions, stepHasHtml } from '../../lib/stepArtifacts';
import { userError } from '../../lib/userError';
import type { ShadowExecution, StepShadowComparison } from '../../types/generated';

type Translate = (key: string, ...args: (string | number)[]) => string;

/** Where the run view's Artifacts come from, and the ones made since it loaded. */
export interface StepArtifactContext {
  workflowId: string;
  runId: string;
  /**
   * Artifacts made in this view since the comparison was last read, by
   * {@link artifactKey}; they win until the next read, which then wins.
   */
  made: Readonly<Record<string, string>>;
  onMade: (key: string, pageId: string) => void;
}

/**
 * Opens a tab in the click itself (a tab opened after an await is a popup a
 * browser may block), then sends it to the address the request returns.
 */
async function openWhenReady(address: () => Promise<string>): Promise<void> {
  const tab = window.open('', '_blank');
  try {
    const url = await address();
    if (tab) {
      tab.opener = null;
      tab.location.href = url;
    } else {
      window.open(url, '_blank', 'noopener,noreferrer');
    }
  } catch (cause) {
    tab?.close();
    throw cause;
  }
}

/** The Artifact of one execution: open it, make it, or why there is none. */
export function ExecutionArtifact({ step, execution, context, t, testId }: {
  step: StepShadowComparison;
  execution: ShadowExecution;
  context: StepArtifactContext;
  t: Translate;
  testId: string;
}) {
  const key = artifactKey(step.result_index, execution);
  const pageId = context.made[key] ?? execution.artifact_id ?? null;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const make = useAsyncGuard(async () => {
    setBusy(true);
    setError(null);
    try {
      await openWhenReady(async () => {
        const made = await workflowsApi.makeStepArtifact(
          context.workflowId, context.runId, step.result_index, execution.shadow_id ?? null,
        );
        context.onMade(key, made.page_id);
        window.dispatchEvent(new Event('kronn:pages-activated'));
        return standaloneLivePageUrl(made.page_id);
      });
    } catch (cause) {
      setError(userError(cause));
    } finally {
      setBusy(false);
    }
  });

  if (pageId) {
    return (
      <a className="wf-shadow-artifact-open" data-testid={`${testId}-artifact-open`} href={standaloneLivePageUrl(pageId)} target="_blank" rel="noopener noreferrer">
        <ExternalLink size={11} aria-hidden /> {t('wf.shadow.artifact.open')}
      </a>
    );
  }
  if (execution.html === 'complete') {
    return (
      <span className="wf-shadow-artifact">
        <button type="button" className="wf-shadow-artifact-btn" data-testid={`${testId}-artifact-make`} disabled={busy} onClick={() => { void make(); }}>
          {busy ? <Loader2 size={11} className="spin" aria-hidden /> : <FilePlus2 size={11} aria-hidden />} {t('wf.shadow.artifact.make')}
        </button>
        {error && <span className="wf-shadow-artifact-error" role="alert" data-testid={`${testId}-artifact-error`}>{error}</span>}
      </span>
    );
  }
  // An execution still running, failed or silent has no output to judge; in a
  // step where nobody wrote HTML, « no HTML » would only be noise.
  const why = htmlStateKey(execution.html);
  if (!why || !hasOutput(execution)) return null;
  if (why === htmlStateKey('none') && !stepHasHtml(step)) return null;
  return (
    <span className="wf-shadow-artifact-none text-muted" data-testid={`${testId}-artifact-none`} data-html={execution.html}>
      {t(why)}
    </span>
  );
}

/** « Open the renders in a mosaic »: the step's renders side by side, made where missing. */
export function StepArtifactMosaic({ step, context, t }: {
  step: StepShadowComparison;
  context: StepArtifactContext;
  t: Translate;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const open = useAsyncGuard(async () => {
    setBusy(true);
    setError(null);
    try {
      await openWhenReady(async () => {
        // Read before the request: the order the server answers in is this one.
        const shown = renderedExecutions(step);
        const mosaic = await workflowsApi.stepArtifactMosaic(context.workflowId, context.runId, step.result_index);
        if (mosaic.page_ids.length === shown.length) {
          shown.forEach((execution, index) => context.onMade(artifactKey(step.result_index, execution), mosaic.page_ids[index]));
        }
        if (mosaic.created > 0) window.dispatchEvent(new Event('kronn:pages-activated'));
        return standaloneLivePageMosaicUrl(mosaic.page_ids);
      });
    } catch (cause) {
      setError(userError(cause));
    } finally {
      setBusy(false);
    }
  });
  if (!offersMosaic(step)) return null;
  return (
    <p className="wf-shadow-artifact-mosaic" data-testid={`wf-shadow-mosaic-${step.step_name}`}>
      <button type="button" className="wf-shadow-artifact-btn" disabled={busy} onClick={() => { void open(); }} title={t('wf.shadow.artifact.mosaicHint')}>
        {busy ? <Loader2 size={11} className="spin" aria-hidden /> : <LayoutGrid size={11} aria-hidden />} {t('wf.shadow.artifact.mosaic')}
      </button>
      {error && <span className="wf-shadow-artifact-error" role="alert">{error}</span>}
    </p>
  );
}
