// Tests for scanUndeclaredVars — the live wizard helper that flags
// `{{var}}` references not matching any known source.
//
// Coverage focus :
// 1. Each "always-valid" prefix (state, iter, artifacts, previous_step)
//    silently passes
// 2. steps.<name>.* validates against earlier steps only (typo or
//    forward-reference flagged)
// 3. failed_step.* gated by inRollback
// 4. Bare names match workflow.variables, otherwise flagged
// 5. Unknown bare names with dot also flagged
// 6. Edge cases : empty prompt, malformed `{{}}`, filter/pipe `{{x | y}}`
//    skipped (out of scope for the simple scanner)

import { describe, it, expect } from 'vitest';
import { scanUndeclaredVars } from '../scanUndeclaredVars';
import type { WorkflowStep, PromptVariable } from '../../types/generated';

const mkStep = (name: string): WorkflowStep => ({
  name,
  step_type: { type: 'Agent' },
  description: null,
  agent: 'ClaudeCode',
  prompt_template: '',
  mode: { type: 'Normal' },
  output_format: { type: 'FreeText' },
  on_result: [],
  agent_settings: null,
  stall_timeout_secs: null,
  retry: null,
  delay_after_secs: null,
  mcp_config_ids: [],
  skill_ids: [],
  profile_ids: [],
  directive_ids: [],
  batch_quick_prompt_id: null,
  batch_items_from: null,
  batch_wait_for_completion: null,
  batch_max_items: null,
  batch_workspace_mode: null,
  batch_chain_prompt_ids: [],
  notify_config: null,
  api_plugin_slug: null,
  api_config_id: null,
  api_endpoint_path: null,
  api_method: null,
  api_query: null,
  api_path_params: null,
  api_headers: null,
  api_body: null,
  api_extract: null,
  api_pagination: null,
  api_timeout_ms: null,
  api_max_retries: null,
  api_output_var: null,
  gate_message: null,
  gate_request_changes_target: null,
  gate_notify_url: null,
  exec_command: null,
  exec_args: [],
  exec_timeout_secs: null,
});

const baseOpts = (over: Partial<Parameters<typeof scanUndeclaredVars>[1]> = {}) => ({
  allSteps: [mkStep('first')],
  currentStepIdx: 1,
  inRollback: false,
  triggerType: 'Manual' as const,
  workflowVariables: [] as PromptVariable[],
  artifacts: {},
  ...over,
});

describe('scanUndeclaredVars', () => {
  it('returns empty for a prompt with no variables', () => {
    const result = scanUndeclaredVars('Just plain text, no vars.', baseOpts());
    expect(result).toEqual([]);
  });

  it('accepts always-valid runtime prefixes silently', () => {
    const prompt = 'Tour {{iter.implement}}, dernier verdict : {{state.last_review}}. ' +
      'Précédent : {{previous_step.summary}}. Plan : {{artifacts.plan}}.';
    const result = scanUndeclaredVars(prompt, baseOpts());
    expect(result).toEqual([]);
  });

  it('flags steps.<name>.* when <name> is not an earlier step', () => {
    const prompt = 'Use {{steps.unknown.summary}} please.';
    const result = scanUndeclaredVars(prompt, baseOpts());
    expect(result).toHaveLength(1);
    expect(result[0].name).toBe('steps.unknown.summary');
    expect(result[0].reason).toBe('unknown_step');
  });

  it('accepts steps.<name>.* when <name> is an earlier step', () => {
    const prompt = 'Read {{steps.first.summary}} carefully.';
    const result = scanUndeclaredVars(prompt, baseOpts());
    expect(result).toEqual([]);
  });

  it('flags forward step references (the step is later in the list)', () => {
    // currentStepIdx = 0 means we're at the FIRST step, so referencing
    // a later step is invalid. (`first` itself is at idx 0 — referencing
    // it from idx 0 means self-reference which is also not valid.)
    const prompt = 'Read {{steps.first.summary}}.';
    const result = scanUndeclaredVars(prompt, baseOpts({ currentStepIdx: 0 }));
    expect(result).toHaveLength(1);
    expect(result[0].reason).toBe('unknown_step');
  });

  it('flags failed_step.* outside rollback', () => {
    const prompt = 'Last failure was {{failed_step.name}}.';
    const result = scanUndeclaredVars(prompt, baseOpts());
    expect(result).toHaveLength(1);
    expect(result[0].reason).toBe('failed_step_outside_rollback');
  });

  it('accepts failed_step.* in rollback', () => {
    const prompt = 'Last failure was {{failed_step.name}}.';
    const result = scanUndeclaredVars(prompt, baseOpts({ inRollback: true }));
    expect(result).toEqual([]);
  });

  it('accepts bare names matching workflow.variables', () => {
    const prompt = 'Process ticket {{ticket_id}} with brief {{brief}}.';
    const result = scanUndeclaredVars(prompt, baseOpts({
      workflowVariables: [
        { name: 'ticket_id', label: '', placeholder: '', description: null, required: true },
        { name: 'brief', label: '', placeholder: '', description: null, required: false },
      ],
    }));
    expect(result).toEqual([]);
  });

  it('flags bare names NOT in workflow.variables', () => {
    const prompt = 'Process ticket {{ticket_id}} with brief {{undeclared_var}}.';
    const result = scanUndeclaredVars(prompt, baseOpts({
      workflowVariables: [
        { name: 'ticket_id', label: '', placeholder: '', description: null, required: true },
      ],
    }));
    expect(result).toHaveLength(1);
    expect(result[0].name).toBe('undeclared_var');
    expect(result[0].reason).toBe('unknown_bare');
  });

  it('accepts issue.* (tracker trigger context)', () => {
    const prompt = 'New issue: {{issue.title}} — body: {{issue.body}}.';
    const result = scanUndeclaredVars(prompt, baseOpts({ triggerType: 'Tracker' }));
    expect(result).toEqual([]);
  });

  it('deduplicates the same undeclared var appearing twice', () => {
    const prompt = 'Use {{my_var}} and again {{my_var}}.';
    const result = scanUndeclaredVars(prompt, baseOpts());
    expect(result).toHaveLength(1);
    expect(result[0].name).toBe('my_var');
  });

  it('skips filter/pipe expressions like `{{x | y}}` (out of scope)', () => {
    // These would be valid templates in some engines but Kronn's render
    // doesn't support filters today. The scanner stays conservative and
    // skips them rather than flagging false positives.
    const prompt = 'Greet: {{name | upper}}';
    const result = scanUndeclaredVars(prompt, baseOpts());
    expect(result).toEqual([]);
  });

  it('skips empty `{{}}` and whitespace-only references', () => {
    const prompt = 'Empty {{}} and {{   }} and {{ leading_space}}.';
    const result = scanUndeclaredVars(prompt, baseOpts());
    // `{{ leading_space}}` is trimmed → counts as `leading_space` bare.
    expect(result).toHaveLength(1);
    expect(result[0].name).toBe('leading_space');
  });

  it('flags dotted unknown paths (not steps.* / state.* / etc.)', () => {
    const prompt = 'Some weird {{custom.path.deep}}.';
    const result = scanUndeclaredVars(prompt, baseOpts());
    expect(result).toHaveLength(1);
    expect(result[0].name).toBe('custom.path.deep');
    expect(result[0].reason).toBe('unknown_bare');
  });
});
