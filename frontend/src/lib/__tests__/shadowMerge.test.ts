// KT-1696 — the shadows of a step and their merge, as the overview writes them.
import { describe, it, expect } from 'vitest';
import {
  activeMergedIds, agentTransport, canPauseShadow, freshShadowId, runShadowSummary, shadowTally, withMergeAgent, withShadowAdded,
  withShadowChanged, withShadowEnabled, withShadowMerged, withShadowRemoved,
} from '../shadowMerge';
import type { ShadowMerge, StepShadow, StepShadowComparison, WorkflowStep } from '../../types/generated';

const shadow = (id: string, extra: Partial<StepShadow> = {}): StepShadow => ({ id, label: '', enabled: true, ...extra });
const merge = (extra: Partial<ShadowMerge> = {}): ShadowMerge => ({ enabled: true, ...extra });
const two = [shadow('argon'), shadow('twin')];

/** The invariant of every write: an explicit list, on iff it names a shadow. */
const expectExplicit = (written: ShadowMerge | null | undefined) => {
  if (!written) return;
  expect(Array.isArray(written.shadows)).toBe(true);
  expect(written.enabled).toBe((written.shadows ?? []).length > 0);
};

describe('withShadowMerged — the Merge toggle', () => {
  it('writes an explicit list and switches the merge on when the first shadow merges', () => {
    const edit = withShadowMerged(two, undefined, 'argon', true);
    expect(edit.merge).toEqual({ enabled: true, shadows: ['argon'] });
    expect(edit.shadows).toEqual(two);
  });

  it('adds a second shadow to the list, in the order of the step', () => {
    const edit = withShadowMerged(two, merge({ shadows: ['twin'] }), 'argon', true);
    expect(edit.merge).toEqual({ enabled: true, shadows: ['argon', 'twin'] });
  });

  it('turning it on for a paused shadow also activates it', () => {
    const paused = [shadow('argon', { enabled: false }), shadow('twin')];
    const edit = withShadowMerged(paused, undefined, 'argon', true);
    expect(edit.shadows[0].enabled).toBe(true);
    expect(edit.merge).toEqual({ enabled: true, shadows: ['argon'] });
  });

  it('turning off the last merged shadow switches the merge off and keeps its other settings', () => {
    const agent = { agent: 'Codex' as const, model: 'gpt-x' };
    const edit = withShadowMerged(two, merge({ shadows: ['argon'], timeout_secs: 300, agent }), 'argon', false);
    expect(edit.merge).toEqual({ enabled: false, shadows: [], timeout_secs: 300, agent });
  });

  it('never writes an empty list with the merge on', () => {
    // « every shadow » (an empty list, on) is read as all of them, then made explicit.
    const all = withShadowMerged(two, merge({ shadows: [] }), 'twin', false);
    expect(all.merge).toEqual({ enabled: true, shadows: ['argon'] });
    for (const written of [
      withShadowMerged(two, merge({ shadows: ['argon'] }), 'argon', false).merge,
      withShadowMerged(two, undefined, 'argon', true).merge,
      withShadowMerged(two, merge({ enabled: false, shadows: ['argon'] }), 'twin', true).merge,
      withShadowRemoved(two, merge({ shadows: ['argon'] }), 'argon').merge,
    ]) expectExplicit(written);
  });

  it('leaves a step without merge alone when a shadow is un-merged, and ignores an unknown id', () => {
    expect(withShadowMerged(two, undefined, 'argon', false).merge).toBeUndefined();
    expect(withShadowMerged(two, merge({ shadows: ['argon'] }), 'ghost', true).merge).toBeUndefined();
  });

  it('a disabled merge merges nothing, whatever list it keeps', () => {
    expect(activeMergedIds(merge({ enabled: false, shadows: ['argon'] }), two)).toEqual([]);
    expect(activeMergedIds(merge({ shadows: [] }), two)).toEqual(['argon', 'twin']);
    expect(activeMergedIds(merge({ shadows: ['gone', 'argon'] }), two)).toEqual(['argon']);
  });
});

describe('withShadowEnabled — the Active toggle', () => {
  it('pauses and resumes a shadow that is not merged', () => {
    expect(withShadowEnabled(two, undefined, 'twin', false).shadows[1].enabled).toBe(false);
    const paused = [shadow('argon'), shadow('twin', { enabled: false })];
    expect(withShadowEnabled(paused, undefined, 'twin', true).shadows[1].enabled).toBe(true);
  });

  it('refuses to pause a merged shadow: it always runs', () => {
    const m = merge({ shadows: ['argon'] });
    expect(canPauseShadow(m, two, 'argon')).toBe(false);
    expect(canPauseShadow(m, two, 'twin')).toBe(true);
    const edit = withShadowEnabled(two, m, 'argon', false);
    expect(edit.shadows).toEqual(two);
    expect(edit.merge).toBeUndefined();
  });
});

describe('add, remove, change', () => {
  it('a new shadow is compared only: the merge list does not take it in', () => {
    const edit = withShadowAdded(two, merge({ shadows: [] }), shadow('shadow-3'));
    expect(edit.merge).toEqual({ enabled: true, shadows: ['argon', 'twin'] });
    expect(edit.shadows.map(s => s.id)).toEqual(['argon', 'twin', 'shadow-3']);
    expect(withShadowAdded(two, undefined, shadow('shadow-3')).merge).toBeUndefined();
  });

  it('removing a merged shadow drops it from the list; removing them all removes the merge', () => {
    expect(withShadowRemoved(two, merge({ shadows: ['argon', 'twin'] }), 'argon').merge).toEqual({ enabled: true, shadows: ['twin'] });
    expect(withShadowRemoved([shadow('argon')], merge({ shadows: ['argon'] }), 'argon').merge).toBeNull();
  });

  it('the merge follows a rename', () => {
    const edit = withShadowChanged(two, merge({ shadows: ['argon'] }), 'argon', shadow('renamed'));
    expect(edit.merge).toEqual({ enabled: true, shadows: ['renamed'] });
  });

  it('changing the agent of a shadow keeps the merge as it was', () => {
    const edit = withShadowChanged(two, merge({ shadows: ['argon'] }), 'argon', shadow('argon', { agent: { agent: 'Ollama', model: 'qwen' } }));
    expect(edit.merge).toEqual({ enabled: true, shadows: ['argon'] });
    expect(edit.shadows[0].agent).toEqual({ agent: 'Ollama', model: 'qwen' });
  });

  it('sets the condense agent only when there is a merge', () => {
    const agent = { agent: 'Codex' as const };
    expect(withMergeAgent(two, merge({ shadows: ['argon'] }), agent).merge).toEqual({ enabled: true, shadows: ['argon'], agent });
    expect(withMergeAgent(two, undefined, agent).merge).toBeUndefined();
  });

  it('picks a shadow id nobody uses', () => {
    expect(freshShadowId([])).toBe('shadow-1');
    expect(freshShadowId([shadow('shadow-2')])).toBe('shadow-3');
    expect(freshShadowId([shadow('shadow-1'), shadow('a')])).toBe('shadow-3');
  });
});

describe('transport, tally and run summary', () => {
  it('badges the transport of an agent', () => {
    expect(agentTransport('ClaudeCode')).toBe('cli');
    expect(agentTransport('Ollama')).toBe('local');
    expect(agentTransport('LiteLlm')).toBe('http');
    expect(agentTransport('ClaudeCode', 'conn-1')).toBe('http');
  });

  it('counts live shadows, doubled steps and whether any merges', () => {
    const steps = [
      { name: 'a', shadows: [shadow('x'), shadow('y', { enabled: false })], shadow_merge: merge({ shadows: ['x'] }) },
      { name: 'b', shadows: [shadow('z')] },
      { name: 'c', shadows: [shadow('off', { enabled: false })] },
      { name: 'd' },
    ] as WorkflowStep[];
    expect(shadowTally(steps)).toEqual({ shadows: 2, steps: 2, merged: true });
    expect(shadowTally([steps[1]])).toEqual({ shadows: 1, steps: 1, merged: false });
    expect(shadowTally(null)).toEqual({ shadows: 0, steps: 0, merged: false });
  });

  const comparison = (shadows: number, mergeReport?: { status: string; missing?: number }): StepShadowComparison => ({
    step_name: 's', result_index: 0, main: {} as never, shadows: Array.from({ length: shadows }, () => ({}) as never),
    ...(mergeReport ? { merge: { status: mergeReport.status, missing: Array.from({ length: mergeReport.missing ?? 0 }, () => ({})) } as never } : {}),
  });

  it('says ok for a whole merge, partial for a merge that lacks a shadow or did not finish', () => {
    expect(runShadowSummary([comparison(0)])).toBeNull();
    expect(runShadowSummary([comparison(2)])).toEqual({ shadows: 2, merge: null });
    expect(runShadowSummary([comparison(2, { status: 'merged' })])).toEqual({ shadows: 2, merge: 'ok' });
    expect(runShadowSummary([comparison(2, { status: 'merged', missing: 1 })])).toEqual({ shadows: 2, merge: 'partial' });
    expect(runShadowSummary([comparison(1, { status: 'failed' })])).toEqual({ shadows: 1, merge: 'partial' });
  });
});
