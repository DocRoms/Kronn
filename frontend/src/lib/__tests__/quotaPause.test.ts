// KT-1657 — a run paused on a provider quota reads as paused, in every language.

import { describe, it, expect } from 'vitest';
import type { ActiveRunSummary } from '../../types/generated';
import { dictionaries } from '../i18n/testing';
import { quotaPauseLabel, quotaPausedRuns, quotaResumeTarget } from '../quotaPause';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

const run = (over: Partial<ActiveRunSummary>): ActiveRunSummary => ({
  run_id: 'run-1',
  workflow_id: 'wf-1',
  workflow_name: 'PR Review',
  project_id: null,
  status: 'WaitingQuota',
  started_at: '2026-11-09T16:19:00Z',
  ...over,
});

describe('quotaPauseLabel', () => {
  it('names the wake time when the engine will resume the run on its own', () => {
    const wake = '2026-11-11T02:02:00Z';
    expect(quotaPauseLabel(t, { wake_at: wake })).toBe(
      `wf.quota.pausedUntil:${new Date(wake).toLocaleString()}`,
    );
  });

  it('says a person resumes a parked run, and stays "paused" when nothing is known', () => {
    expect(quotaPauseLabel(t, { wake_at: null })).toBe('wf.quota.pausedParked');
    expect(quotaPauseLabel(t, undefined)).toBe('wf.quota.paused');
  });
});

describe('quotaResumeTarget and quotaPausedRuns', () => {
  it('resumes a sub-workflow child through its parent', () => {
    expect(quotaResumeTarget(run({ parent_run_id: 'parent-1' }))).toBe('parent-1');
    expect(quotaResumeTarget(run({}))).toBe('run-1');
  });

  it('keeps the paused top-level runs only', () => {
    const runs = [
      run({ run_id: 'a' }),
      run({ run_id: 'child', parent_run_id: 'a' }),
      run({ run_id: 'running', status: 'Running' }),
      run({ run_id: 'gate', status: 'WaitingApproval' }),
    ];
    expect(quotaPausedRuns(runs).map(r => r.run_id)).toEqual(['a']);
    expect(quotaPausedRuns(null)).toEqual([]);
  });
});

describe('the paused wording, in every language', () => {
  const keys = [
    'wf.quota.paused',
    'wf.quota.pausedShort',
    'wf.quota.pausedUntil',
    'wf.quota.pausedParked',
    'wf.quota.resumeNow',
    'wf.quota.switchAgent',
    'wf.quota.switched',
  ];

  it.each(['en', 'fr', 'es', 'zh'] as const)('%s defines every key, and the wake time has its slot', (lang) => {
    const dict = dictionaries[lang] as Record<string, string>;
    for (const key of keys) expect(dict[key], `${lang}:${key}`).toBeTruthy();
    expect(dict['wf.quota.pausedUntil']).toContain('{0}');
    expect(dict['wf.quota.switched']).toContain('{0}');
  });

  it('never calls a paused run "running" in the English label', () => {
    const en = dictionaries.en as Record<string, string>;
    expect(en['wf.quota.pausedUntil']).toBe('Paused — provider quota, resumes automatically at {0}');
    expect(en['wf.quota.pausedUntil'].toLowerCase()).not.toContain('running');
  });
});
