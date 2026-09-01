import { describe, expect, it } from 'vitest';
import type { RunStatus, StepResult, WorkflowRun } from '../../types/generated';
import { mapStepStatus, runToPipeline, type PhaseMapEntry } from '../live-page-pipeline';

function step(overrides: Partial<StepResult> & { step_name: string; status: RunStatus }): StepResult {
  return {
    output: '',
    tokens_used: 0,
    duration_ms: 0,
    ...overrides,
  } as StepResult;
}

function run(overrides: Partial<WorkflowRun> = {}): WorkflowRun {
  return {
    id: 'run-abcdef1234',
    workflow_id: 'wf-1',
    status: 'Running',
    trigger_context: null,
    step_results: [],
    tokens_used: 0,
    workspace_path: null,
    started_at: '2026-08-28T12:41:00Z',
    finished_at: null,
    ...overrides,
  } as WorkflowRun;
}

const PHASE_MAP: PhaseMapEntry[] = [
  { name: 'Préparation', emoji: '🧰', steps: [
    { step: 'resolve_pr', tag: 'lecture' },
    { step: 'gate_confirm_pr', tag: 'gate' },
  ] },
  { name: 'Déploiement', emoji: '🚀', steps: [
    { step: 'gate_prod', tag: 'gate', link: 'https://staging.example.com' },
    { step: 'deploy_prod', tag: 'dispatch', label: 'CD prod' },
  ] },
];

describe('mapStepStatus', () => {
  it('maps every run status to a step status', () => {
    expect(mapStepStatus('Success')).toBe('done');
    expect(mapStepStatus('Partial')).toBe('done');
    expect(mapStepStatus('WaitingApproval')).toBe('wait');
    expect(mapStepStatus('Running')).toBe('current');
    expect(mapStepStatus('Failed')).toBe('failed');
    expect(mapStepStatus('Cancelled')).toBe('failed');
    expect(mapStepStatus('StoppedByGuard')).toBe('failed');
    expect(mapStepStatus('Interrupted')).toBe('failed');
    expect(mapStepStatus('Pending')).toBe('pending');
  });
});

describe('runToPipeline', () => {
  it('folds step_results into the phase-mapped shape with derived statuses', () => {
    const pipeline = runToPipeline(
      run({
        status: 'WaitingApproval',
        trigger_context: { jira_ticket_key: 'EW-7754' },
        step_results: [
          step({ step_name: 'resolve_pr', status: 'Success', output: 'PR #1842 détectée\nautre ligne', started_at: '2026-08-28T12:41:00Z', duration_ms: 3000 }),
          step({ step_name: 'gate_confirm_pr', status: 'Success', output: 'PR confirmée' }),
          step({ step_name: 'gate_prod', status: 'WaitingApproval', output: 'Valide la prod' }),
        ],
      }),
      PHASE_MAP,
      { ticket: 'trigger.jira_ticket_key' },
      { origin: 'https://kronn.example', dataset: 'pipeline' },
    );

    expect(pipeline.phases).toHaveLength(2);
    const prep = pipeline.phases[0];
    expect(prep.name).toBe('Préparation');
    expect(prep.emoji).toBe('🧰');
    expect(prep.steps[0]).toMatchObject({ n: 'resolve_pr', s: 'done', tag: 'lecture', d: 'PR #1842 détectée', at: expect.any(String), dur: '3s' });
    expect(prep.steps[1]).toMatchObject({ n: 'gate_confirm_pr', s: 'done' });

    const deploy = pipeline.phases[1];
    // gate waiting → 'wait', and its link is carried for the page's gate box.
    expect(deploy.steps[0]).toMatchObject({ n: 'gate_prod', s: 'wait', link: 'https://staging.example.com' });
    // deploy_prod not yet reached → pending, uses its label as description.
    expect(deploy.steps[1]).toMatchObject({ n: 'deploy_prod', s: 'pending', d: 'CD prod' });

    // meta: trigger-resolved ticket + defaults.
    expect(pipeline.meta.ticket).toBe('EW-7754');
    expect(pipeline.meta.runUrl).toBe('https://kronn.example');
    expect(pipeline.meta.run).toBe('#run-abcd');
    expect(pipeline.meta.run_id).toBe('run-abcdef1234'); // full id for gate decisions
    expect(pipeline.meta.dataset).toBe('pipeline');
    expect(pipeline.meta.started).toMatch(/^\d{2}:\d{2}$/);
  });

  it('surfaces exactly one current step for a running run', () => {
    const pipeline = runToPipeline(
      run({
        status: 'Running',
        step_results: [step({ step_name: 'resolve_pr', status: 'Success', output: 'ok' })],
      }),
      PHASE_MAP,
    );
    const flat = pipeline.phases.flatMap(p => p.steps);
    expect(flat.filter(s => s.s === 'current')).toHaveLength(1);
    // The first not-yet-recorded step becomes current.
    expect(flat.find(s => s.s === 'current')?.n).toBe('gate_confirm_pr');
    expect(flat.filter(s => s.s === 'pending')).toHaveLength(2);
  });

  it('keeps a single current when a step is already recorded as Running', () => {
    // A recorded Running step claims "current"; the first unrecorded step must
    // NOT also become current (would show two live steps on the page).
    const pipeline = runToPipeline(
      run({
        status: 'Running',
        step_results: [
          step({ step_name: 'resolve_pr', status: 'Success', output: 'ok' }),
          step({ step_name: 'gate_confirm_pr', status: 'Running', output: 'working' }),
        ],
      }),
      PHASE_MAP,
    );
    const flat = pipeline.phases.flatMap(p => p.steps);
    expect(flat.filter(s => s.s === 'current')).toHaveLength(1);
    expect(flat.find(s => s.s === 'current')?.n).toBe('gate_confirm_pr');
    // The two unrecorded downstream steps stay pending, none stolen as current.
    expect(flat.filter(s => s.s === 'pending')).toHaveLength(2);
  });

  it('marks no step current when the run is terminal', () => {
    const pipeline = runToPipeline(
      run({
        status: 'Success',
        step_results: [
          step({ step_name: 'resolve_pr', status: 'Success', output: 'ok' }),
          step({ step_name: 'gate_confirm_pr', status: 'Success', output: 'ok' }),
          step({ step_name: 'gate_prod', status: 'Success', output: 'ok' }),
          step({ step_name: 'deploy_prod', status: 'Success', output: 'ok' }),
        ],
      }),
      PHASE_MAP,
    );
    expect(pipeline.phases.flatMap(p => p.steps).every(s => s.s === 'done')).toBe(true);
  });

  it('falls back to literals and blanks unmapped meta (never "undefined")', () => {
    const pipeline = runToPipeline(
      run({ id: 'deadbeef-0000', trigger_context: {} }),
      [],
      { type: 'Bug', ticket: 'trigger.missing' },
    );
    expect(pipeline.meta.type).toBe('Bug'); // literal passthrough
    expect(pipeline.meta.ticket).toBeUndefined(); // unresolved trigger key dropped
    expect(pipeline.meta.run).toBe('#deadbeef');
    // Display fields the page reads are blanked, not left undefined.
    for (const key of ['title', 'pr', 'prUrl', 'branch', 'tag', 'prev']) {
      expect(pipeline.meta[key]).toBe('');
    }
  });

  it('resolves meta from step outputs via json-path and regex sources', () => {
    const pipeline = runToPipeline(
      run({
        status: 'Success',
        step_results: [
          step({ step_name: 'fetch_ticket_type', status: 'Success',
            output: '---STEP_OUTPUT---\n{"data":{"issuetype":{"name":"Story"},"summary":"[Sitemap] Switch robots.txt"},"status":"OK"}\n---END_STEP_OUTPUT---' }),
          step({ step_name: 'compute_tag', status: 'Success',
            output: 'exit 0 — 485 ms\n---STEP_OUTPUT---\n{"data":{"stdout":"Type Jira : Story\\nDernière release : 17.70.1\\nNouveau tag (calculé) : 17.71.0\\n"}}\n---END_STEP_OUTPUT---' }),
        ],
      }),
      [{ name: 'P', steps: [{ step: 'fetch_ticket_type' }] }],
      {
        type: 'step:fetch_ticket_type:json:data.issuetype.name',
        title: 'step:fetch_ticket_type:json:data.summary',
        tag: 'step:compute_tag:re:calculé\\) : ([\\d.]+)',
        prev: 'step:compute_tag:re:Dernière release : ([\\d.]+)',
        branch: 'step:absent:json:data.x',
      },
    );
    expect(pipeline.meta.type).toBe('Story');
    expect(pipeline.meta.title).toBe('[Sitemap] Switch robots.txt');
    expect(pipeline.meta.tag).toBe('17.71.0');
    expect(pipeline.meta.prev).toBe('17.70.1');
    expect(pipeline.meta.branch).toBe(''); // unresolved known field → blanked, never "undefined"
  });

  it('skips step-output envelope markers when deriving a step description', () => {
    const pipeline = runToPipeline(
      run({
        status: 'Success',
        step_results: [step({
          step_name: 'a',
          status: 'Success',
          output: '---STEP_OUTPUT---\ntype du ticket : Bug\n---STEP_OUTPUT_END---',
        })],
      }),
      [{ name: 'P', steps: [{ step: 'a' }] }],
    );
    expect(pipeline.phases[0].steps[0].d).toBe('type du ticket : Bug');
  });

  it('prefers the step-output envelope summary as the description', () => {
    const pipeline = runToPipeline(
      run({
        status: 'Success',
        step_results: [step({
          step_name: 'a',
          status: 'Success',
          output: '---STEP_OUTPUT---\n{"data":{"issuetype":{"name":"Story"}},"status":"OK","summary":"GET https://…/issue/EW-7168 → object"}\n---END_STEP_OUTPUT---',
        })],
      }),
      [{ name: 'P', steps: [{ step: 'a' }] }],
    );
    expect(pipeline.phases[0].steps[0].d).toBe('GET https://…/issue/EW-7168 → object');
  });
});
