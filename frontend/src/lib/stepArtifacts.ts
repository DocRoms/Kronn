/**
 * KT-1990 — Artifacts made from the executions of a step: what the run view
 * offers per execution, which executions a mosaic shows, and what a delete
 * confirmation says about the library Artifacts that go with the runs.
 */
import type {
  ExecutionHtml,
  LivePage,
  ShadowExecution,
  StepArtifactFolder,
  StepArtifactLink,
  StepShadowComparison,
} from '../types/generated';

type Translate = (key: string, ...args: (string | number)[]) => string;

/** The key of an execution's Artifact in the run view: the step's result index and the shadow (empty for the main one). */
export const artifactKey = (resultIndex: number, execution: ShadowExecution): string =>
  `${resultIndex}#${execution.shadow_id ?? ''}`;

/** A run folder of the Artifact library, as the library shows it. */
export interface ArtifactRunFolderView {
  folder: StepArtifactFolder;
  /** The folder's Artifacts the library shows (search applied), in execution order. */
  items: { link: StepArtifactLink; page: LivePage }[];
}

/** The folders to show: those with an Artifact the library holds and the search keeps. */
export function visibleRunFolders(
  folders: StepArtifactFolder[],
  pages: ReadonlyMap<string, LivePage>,
  isVisible: (page: LivePage) => boolean,
): ArtifactRunFolderView[] {
  return folders
    .map(folder => ({
      folder,
      items: folder.artifacts.flatMap(link => {
        const page = pages.get(link.page_id);
        return page && !page.archived && isVisible(page) ? [{ link, page }] : [];
      }),
    }))
    .filter(view => view.items.length > 0);
}

/** A run folder's name: `<workflow> · run #N`. */
export function folderLabel(folder: StepArtifactFolder, t: Translate): string {
  return t('pages.runFolders.folder', folder.workflow_name, folder.run_number);
}

/** The i18n key that says why an execution offers no Artifact, or null when it does. */
export function htmlStateKey(html: ExecutionHtml | undefined): string | null {
  switch (html) {
    case 'complete': return null;
    case 'cut_unclosed_fence': return 'wf.shadow.artifact.cutUnclosedFence';
    case 'cut_no_closing_tag': return 'wf.shadow.artifact.cutNoClosingTag';
    case 'cut_output': return 'wf.shadow.artifact.cutOutput';
    default: return 'wf.shadow.artifact.none';
  }
}

/** Whether an execution has a render: an Artifact already, or a complete HTML document. */
export function hasRender(execution: ShadowExecution): boolean {
  return Boolean(execution.artifact_id) || execution.html === 'complete';
}

/**
 * Whether an execution of the step (main or shadow) wrote an HTML document,
 * complete or cut. A step without one (a review, an analysis) says nothing
 * about Artifacts: « no HTML » would show on every execution.
 */
export function stepHasHtml(step: StepShadowComparison): boolean {
  return [step.main, ...step.shadows].some(execution =>
    Boolean(execution.artifact_id) || (execution.html !== undefined && execution.html !== 'none'));
}

/** The executions a step's mosaic shows, in its order: main first, then the shadows as they started. */
export function renderedExecutions(step: StepShadowComparison): ShadowExecution[] {
  return [step.main, ...step.shadows].filter(hasRender);
}

/** Whether a step offers « Open the renders in a mosaic »: two renders at least. */
export function offersMosaic(step: StepShadowComparison): boolean {
  return renderedExecutions(step).length >= 2;
}

/** What the library row of an Artifact shows: its execution, and the model that answered. */
export function artifactRowLabel(link: StepArtifactLink, mainLabel: string): string {
  const name = link.shadow_id === '' ? mainLabel : (link.label || link.shadow_id);
  return link.model ? `${name} · ${link.model}` : name;
}

/**
 * The sentence about library Artifacts a run delete adds: how many, or that the
 * count is unknown; none when there are none, as for most deletes.
 */
function artifactsSentence(t: Translate, key: string, artifacts: number | null): string | null {
  if (artifacts === null) return t(`${key}Unknown`);
  return artifacts === 0 ? null : t(key, artifacts);
}

/** The confirmation of deleting one run. */
export function deleteRunConfirmation(t: Translate, artifacts: number | null): string {
  const sentence = artifactsSentence(t, 'wf.deleteRunArtifacts', artifacts);
  return sentence ? `${t('wf.deleteRunConfirm')}\n\n${sentence}` : t('wf.deleteRunConfirm');
}

/** The confirmation of deleting every run of a workflow. */
export function deleteAllRunsConfirmation(t: Translate, runs: number, artifacts: number | null): string {
  const sentence = artifactsSentence(t, 'wf.deleteRunsArtifacts', artifacts);
  return sentence ? `${t('wf.deleteAllRunsConfirm', runs)}\n\n${sentence}` : t('wf.deleteAllRunsConfirm', runs);
}

/**
 * What deleting a workflow takes with it: its runs, and the library Artifacts
 * made from them, in one short sentence. A workflow card shows it on one line
 * that does not wrap, so the Artifacts are counted beside the runs rather than
 * in a sentence of their own.
 */
export function workflowDeleteImpact(t: Translate, runs: number, artifacts: number | null): string {
  if (runs === 0) return t('automation.deleteImpactNone');
  if (artifacts === null) return t('wf.deleteImpactArtifactsUnknown', runs);
  return artifacts === 0 ? t('wf.deleteImpact', runs) : t('wf.deleteImpactArtifacts', runs, artifacts);
}
