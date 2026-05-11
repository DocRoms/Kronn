// Tests for the workflow-level placeholder scanner (0.8.1).
// Pins the "autoBot {{issue}} unset" regression: a workflow with
// `{{issue}}` in step prompts but no declared `variables[]` must
// surface `issue` as auto-detected so the launch modal prompts for it.

import { describe, it, expect } from 'vitest';
import {
  collectUnboundVariables,
  placeholdersInStep,
  mergeDeclaredAndDetected,
} from '../workflowVariables';
import type { Workflow, WorkflowStep } from '../../types/generated';

// Minimal step shape — the scanner doesn't care about unused fields, only
// the ones it reads. Cast to WorkflowStep for type-checking but we
// intentionally only set what each test exercises.
const stepWith = (patch: Partial<WorkflowStep>): WorkflowStep =>
  ({
    id: 's1',
    step_type: { type: 'Agent' },
    name: 'test',
    prompt_template: '',
    ...patch,
  }) as WorkflowStep;

// The scanner only reads `steps` and `variables` — we don't need the
// full Workflow shape (project_id, trigger, actions, safety...). Cast
// via unknown to keep the test fixture minimal but type-safe at the
// call site.
const wfWith = (steps: WorkflowStep[], declaredVars: Workflow['variables'] = []): Workflow =>
  ({
    id: 'wf-test',
    name: 'autoBot',
    description: '',
    steps,
    variables: declaredVars,
    enabled: true,
    created_at: '2026-05-12T00:00:00Z',
    updated_at: '2026-05-12T00:00:00Z',
  }) as unknown as Workflow;

describe('placeholdersInStep', () => {
  it('picks up `{{var}}` from an Agent step prompt_template', () => {
    const step = stepWith({ prompt_template: 'Analyse {{issue}} and propose a fix.' });
    expect(placeholdersInStep(step)).toEqual(['issue']);
  });

  it('walks ApiCall path / query / headers / body via collectPlaceholders', () => {
    const step = stepWith({
      step_type: { type: 'ApiCall' },
      api_endpoint_path: '/projects/{{project_key}}/issues/{{issue}}',
      api_query: { since: '{{since}}' },
      api_headers: { 'X-Org': '{{org_id}}' },
    });
    expect(placeholdersInStep(step).sort()).toEqual(['issue', 'org_id', 'project_key', 'since']);
  });

  it('picks up notify_config.url + body_template + headers', () => {
    const step = stepWith({
      step_type: { type: 'Notify' },
      notify_config: {
        url: 'https://hooks.example.com/{{channel}}',
        method: 'POST',
        headers: { 'X-Token': '{{webhook_token}}' },
        body_template: '{"text":"Done for {{issue}}"}',
      },
    });
    expect(placeholdersInStep(step).sort()).toEqual(['channel', 'issue', 'webhook_token']);
  });

  it('picks up exec_args templated strings', () => {
    const step = stepWith({
      step_type: { type: 'Exec' },
      exec_args: ['--ticket', '{{issue}}', '--branch', '{{branch}}'],
    });
    expect(placeholdersInStep(step).sort()).toEqual(['branch', 'issue']);
  });

  it('picks up batch_items_from when templated', () => {
    const step = stepWith({
      step_type: { type: 'BatchQuickPrompt' },
      batch_items_from: '{{steps.fetch.output}}/{{org}}/items.json',
    });
    // `steps.fetch.output` filtered (runtime), `org` kept.
    expect(placeholdersInStep(step)).toEqual(['org']);
  });

  it('EXCLUDES multi-segment runtime namespaces', () => {
    // `steps.fetch.data`, `batch.item.title`, `previous_qp.output`,
    // `iter.X`, `state.Y`, `failed_step.Z`, `env.HOME`, `trigger.issue.key`,
    // `previous_step.summary`, `artifacts.A` are all runtime.
    const step = stepWith({
      prompt_template:
        'fetch={{steps.fetch.data}} batch={{batch.item.title}} prev={{previous_qp.output}} ' +
        'iter={{iter.x}} state={{state.k}} failed={{failed_step.error}} env={{env.HOME}} ' +
        'trig={{trigger.issue.key}} ps={{previous_step.summary}} art={{artifacts.report}} ' +
        'user={{issue}}',
    });
    // Only `issue` (bare, user-provided) should survive.
    expect(placeholdersInStep(step)).toEqual(['issue']);
  });

  it('KEEPS bare runtime-namespace tokens as user-vars (0.8.1 rule change)', () => {
    // Pre-0.8.1, bare `{{batch}}` was filtered as runtime. New rule: only
    // multi-segment `ns.X` is runtime. Catches the user-reported case
    // where `{{issue}}` was being eaten by the filter.
    const step = stepWith({ prompt_template: '{{batch}} {{issue}} {{steps}}' });
    expect(placeholdersInStep(step).sort()).toEqual(['batch', 'issue', 'steps']);
  });
});

describe('collectUnboundVariables', () => {
  it('unions placeholders across all steps, deduped + sorted', () => {
    const wf = wfWith([
      stepWith({ prompt_template: 'Fetch {{issue}}' }),
      stepWith({
        step_type: { type: 'ApiCall' },
        api_endpoint_path: '/r/{{owner}}/{{repo}}',
        api_query: { tag: '{{issue}}' }, // dup
      }),
      stepWith({
        step_type: { type: 'Notify' },
        notify_config: {
          url: 'https://hook/{{channel}}',
          method: 'POST',
          headers: {},
          body_template: 'done',
        },
      }),
    ]);
    expect(collectUnboundVariables(wf)).toEqual(['channel', 'issue', 'owner', 'repo']);
  });

  it('empty workflow → empty array', () => {
    expect(collectUnboundVariables(wfWith([]))).toEqual([]);
  });

  it('workflow with only runtime tokens → empty array', () => {
    const wf = wfWith([
      stepWith({
        prompt_template: 'Use {{steps.fetch.data}} and {{previous_step.summary}}.',
      }),
    ]);
    expect(collectUnboundVariables(wf)).toEqual([]);
  });
});

describe('mergeDeclaredAndDetected', () => {
  it('preserves declared variables and appends auto-detected ones', () => {
    const wf = wfWith(
      [stepWith({ prompt_template: '{{issue}} + {{branch}}' })],
      [
        { name: 'issue', label: 'Jira ticket', placeholder: 'ABC-123', description: 'Issue key', required: true },
      ],
    );
    const merged = mergeDeclaredAndDetected(wf);
    expect(merged).toHaveLength(2);
    // Declared first
    expect(merged[0].name).toBe('issue');
    expect(merged[0].label).toBe('Jira ticket');
    expect(merged[0].description).toBe('Issue key');
    // Auto-detected next
    expect(merged[1].name).toBe('branch');
    expect(merged[1].label).toBe('branch');
    expect(merged[1].description).toContain('Auto-detected');
  });

  it('autoBot {{issue}} regression — no declared variables, issue still prompted', () => {
    // Original user bug: workflow first step uses `{{issue}}` but
    // `variables[]` is empty. Without auto-detect, the launch modal
    // is skipped and the step fires with literal `{{issue}}`.
    const wf = wfWith([
      stepWith({
        step_type: { type: 'ApiCall' },
        api_endpoint_path: '/jira/issue/{{issue}}',
      }),
    ]);
    const merged = mergeDeclaredAndDetected(wf);
    expect(merged).toHaveLength(1);
    expect(merged[0].name).toBe('issue');
    expect(merged[0].required).toBe(true);
  });

  it('runtime-only workflow → no variables to prompt', () => {
    const wf = wfWith([
      stepWith({ prompt_template: 'Run on {{steps.fetch.output}}' }),
    ]);
    expect(mergeDeclaredAndDetected(wf)).toEqual([]);
  });
});
