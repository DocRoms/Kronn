// KT-1990 — what the run view offers per execution, and what the three delete
// confirmations (a run, every run, a workflow) say about the library Artifacts
// made from those runs, apart from the run's own working files, in every locale.
import { describe, expect, it } from 'vitest';
import en from '../i18n/locales/en';
import es from '../i18n/locales/es';
import fr from '../i18n/locales/fr';
import zh from '../i18n/locales/zh';
import type { ShadowExecution, StepShadowComparison } from '../../types/generated';
import {
  artifactRowLabel, deleteAllRunsConfirmation, deleteRunConfirmation, htmlStateKey, offersMosaic,
  renderedExecutions, workflowDeleteImpact,
} from '../stepArtifacts';

const LOCALES = { en, es, fr, zh } as const;

/** A translator over one real locale dictionary, as the UI formats it. */
const translator = (dict: Record<string, string>) => (key: string, ...args: (string | number)[]) => {
  const template = dict[key];
  if (template === undefined) throw new Error(`missing ${key}`);
  return args.reduce<string>((text, arg, i) => text.replaceAll(`{${i}}`, String(arg)), template);
};

const exec = (shadowId: string | null, extra: Partial<ShadowExecution> = {}): ShadowExecution => ({
  shadow_id: shadowId ?? undefined, label: shadowId ?? '', status: 'success', identity: { agent: 'Ollama', prompt: 'inline:abc', skills: [] },
  output: 'x', html: 'complete', ...extra,
});

describe('step execution Artifacts', () => {
  it('tells every HTML state apart', () => {
    expect(htmlStateKey('complete')).toBeNull();
    expect(htmlStateKey('none')).toBe('wf.shadow.artifact.none');
    expect(htmlStateKey('cut_unclosed_fence')).toBe('wf.shadow.artifact.cutUnclosedFence');
    expect(htmlStateKey('cut_no_closing_tag')).toBe('wf.shadow.artifact.cutNoClosingTag');
    expect(htmlStateKey('cut_output')).toBe('wf.shadow.artifact.cutOutput');
    for (const dict of Object.values(LOCALES)) {
      for (const state of ['none', 'cut_unclosed_fence', 'cut_no_closing_tag', 'cut_output'] as const) {
        expect(dict[htmlStateKey(state)!]).toBeTruthy();
      }
    }
  });

  it('lists the renders of a step in mosaic order, and needs two', () => {
    const step: StepShadowComparison = {
      step_name: 'jeu', result_index: 0, main: exec(null),
      shadows: [exec('a', { html: 'none' }), exec('b'), exec('c', { html: 'cut_output', artifact_id: 'p-c' })],
    };
    expect(renderedExecutions(step).map(e => e.shadow_id ?? 'main')).toEqual(['main', 'b', 'c']);
    expect(offersMosaic(step)).toBe(true);
    expect(offersMosaic({ ...step, shadows: [exec('a', { html: 'none' })] })).toBe(false);
  });

  it('labels a library row by execution and model', () => {
    const link = { page_id: 'p', run_id: 'r', result_index: 0, step_name: 'jeu', shadow_id: 'qwen', label: 'Qwen local', model: 'qwen3:30b', created_at: '' };
    expect(artifactRowLabel(link, 'main')).toBe('Qwen local · qwen3:30b');
    expect(artifactRowLabel({ ...link, shadow_id: '', label: 'main', model: undefined }, 'principale')).toBe('principale');
    expect(artifactRowLabel({ ...link, label: '' }, 'main')).toBe('qwen · qwen3:30b');
  });
});

describe.each(Object.entries(LOCALES))('delete confirmations (%s)', (_locale, dict) => {
  const t = translator(dict as Record<string, string>);

  it('deleting a run states how many library Artifacts go, apart from its working files', () => {
    const three = deleteRunConfirmation(t, 3);
    expect(three).toContain(t('wf.deleteRunConfirm'));
    expect(three).toContain(t('wf.deleteRunArtifacts', 3));
    expect(three).toContain('3');
    expect(deleteRunConfirmation(t, null)).toContain(t('wf.deleteRunArtifactsUnknown'));
  });

  it('adds no sentence about Artifacts when there are none', () => {
    // The reworded first sentence is enough: most deletes take no Artifact.
    expect(deleteRunConfirmation(t, 0)).toBe(t('wf.deleteRunConfirm'));
    expect(deleteAllRunsConfirmation(t, 12, 0)).toBe(t('wf.deleteAllRunsConfirm', 12));
    expect(workflowDeleteImpact(t, 5, 0)).toBe(t('wf.deleteImpact', 5));
    expect(dict).not.toHaveProperty('wf.deleteRunArtifactsNone');
    expect(dict).not.toHaveProperty('wf.deleteRunsArtifactsNone');
  });

  it('deleting every run of a workflow states the runs and the Artifacts', () => {
    const text = deleteAllRunsConfirmation(t, 12, 4);
    expect(text).toContain(t('wf.deleteAllRunsConfirm', 12));
    expect(text).toContain(t('wf.deleteRunsArtifacts', 4));
  });

  it('deleting a workflow states the runs and the Artifacts made from them in one short sentence', () => {
    expect(workflowDeleteImpact(t, 0, 0)).toBe(t('automation.deleteImpactNone'));
    const text = workflowDeleteImpact(t, 5, 3);
    expect(text).toBe(t('wf.deleteImpactArtifacts', 5, 3));
    expect(text).toContain('5');
    expect(text).toContain('3');
    expect(text).toMatch(/Artifact/);
    expect(workflowDeleteImpact(t, 5, null)).toBe(t('wf.deleteImpactArtifactsUnknown', 5));
  });

  it('keeps the workflow impact short: the line of a workflow card does not wrap', () => {
    // ConfirmDeleteButton shows the impact on one nowrap line beside the card's controls.
    const base = t('wf.deleteImpact', 5).length;
    expect(workflowDeleteImpact(t, 5, 3).length - base).toBeLessThanOrEqual(40);
    expect(workflowDeleteImpact(t, 5, null).length - base).toBeLessThanOrEqual(55);
  });

  it('ends the Artifact sentences of the all-runs dialog with a period, as a run delete does', () => {
    for (const key of ['wf.deleteRunArtifacts', 'wf.deleteRunArtifactsUnknown', 'wf.deleteRunsArtifacts', 'wf.deleteRunsArtifactsUnknown']) {
      expect(dict[key as keyof typeof dict]).toMatch(/[.。]$/);
    }
  });

  it('never calls a run working file an Artifact of the library', () => {
    // The run's own words say working files; the Artifact sentences name the library.
    for (const key of ['wf.deleteRunConfirm', 'wf.deleteAllRunsConfirm']) {
      expect(dict[key as keyof typeof dict]).not.toMatch(/artifact/i);
    }
    for (const key of ['wf.deleteRunArtifacts', 'wf.deleteRunsArtifacts']) {
      expect(dict[key as keyof typeof dict]).toMatch(/Artifact/);
    }
  });
});
