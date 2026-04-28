// Tests for the placeholder collection + substitution helpers used by
// ApiCallStepCard's "Test the call" flow. The runtime-namespace exclusion
// list is fragile — adding a new namespace means agents start sending
// literal `{{state.X}}` to upstream APIs (404). Pin the rules here.

import { describe, it, expect } from 'vitest';
import { collectPlaceholders, substitutePlaceholders, isRuntimeToken } from '../apiCallPlaceholders';
import type { WorkflowStep } from '../../../types/generated';

const baseStep = (over: Partial<WorkflowStep> = {}): WorkflowStep => ({
  name: 'fetch',
  step_type: { type: 'ApiCall' },
  description: null,
  agent: 'ClaudeCode',
  prompt_template: '',
  mode: { type: 'Normal' },
  output_format: { type: 'Structured' },
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
  batch_concurrent_limit: null,
  notify_config: null,
  api_plugin_slug: 'chartbeat',
  api_config_id: 'cfg-1',
  api_endpoint_path: '/live/quickstats/v4/',
  api_method: 'GET',
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
  ...over,
});

describe('isRuntimeToken — namespace exclusion', () => {
  it('classifies runtime-resolved namespaces as runtime', () => {
    expect(isRuntimeToken('steps.foo.data')).toBe(true);
    expect(isRuntimeToken('previous_step.output')).toBe(true);
    expect(isRuntimeToken('state.last_review')).toBe(true);
    expect(isRuntimeToken('batch.item.title')).toBe(true);
    expect(isRuntimeToken('batch.index')).toBe(true);
    expect(isRuntimeToken('issue.title')).toBe(true);
    expect(isRuntimeToken('artifacts.review')).toBe(true);
  });

  it('classifies bare user variables as NOT runtime', () => {
    expect(isRuntimeToken('host')).toBe(false);
    expect(isRuntimeToken('ticket_key')).toBe(false);
    expect(isRuntimeToken('limit')).toBe(false);
  });

  it('does not treat lookalikes as runtime (the prefix must match exactly)', () => {
    // Common confusion: a user variable named `step_count` shouldn't be
    // classified as runtime just because `steps` is a runtime namespace.
    expect(isRuntimeToken('step_count')).toBe(false);
    expect(isRuntimeToken('issuer')).toBe(false);
    expect(isRuntimeToken('stateful')).toBe(false);
  });
});

describe('collectPlaceholders — finds user variables across the request shape', () => {
  it('extracts from endpoint path', () => {
    const step = baseStep({ api_endpoint_path: '/repos/{{owner}}/{{repo}}/issues' });
    expect(collectPlaceholders(step)).toEqual(['owner', 'repo']);
  });

  it('extracts from query parameters', () => {
    const step = baseStep({
      api_query: { host: '{{host}}', limit: '5' },
    });
    expect(collectPlaceholders(step)).toEqual(['host']);
  });

  it('extracts from headers', () => {
    const step = baseStep({
      api_headers: { 'X-Account-ID': '{{account_id}}', 'X-Static': 'yes' },
    });
    expect(collectPlaceholders(step)).toEqual(['account_id']);
  });

  it('extracts from a JSON body recursively (nested objects + arrays)', () => {
    const step = baseStep({
      api_body: {
        summary: '{{title}}',
        labels: ['{{type}}', 'auto-fix'],
        nested: { author: '{{owner}}' },
      } as unknown as WorkflowStep['api_body'],
    });
    expect(collectPlaceholders(step)).toEqual(['owner', 'title', 'type']);
  });

  it('extracts from a string body too (legacy shape)', () => {
    const step = baseStep({
      api_body: '{"summary":"{{title}}"}' as unknown as WorkflowStep['api_body'],
    });
    expect(collectPlaceholders(step)).toEqual(['title']);
  });

  it('EXCLUDES runtime namespaces — they belong to the workflow runner', () => {
    // Critical: the test-the-call modal must NOT prompt the user for
    // {{steps.X.data}} — those are filled by the runner at run-time, not
    // by the user. Without this exclusion the test modal would force the
    // user to fake values for every upstream-step variable.
    const step = baseStep({
      api_endpoint_path: '/repos/{{owner}}/{{repo}}/issues/{{steps.fetch.data}}',
      api_query: { since: '{{previous_step.summary}}', host: '{{host}}' },
    });
    expect(collectPlaceholders(step)).toEqual(['host', 'owner', 'repo']);
  });

  it('dedupes: a variable mentioned twice appears once', () => {
    const step = baseStep({
      api_endpoint_path: '/{{host}}/path',
      api_query: { host: '{{host}}', alt: '{{host}}' },
    });
    expect(collectPlaceholders(step)).toEqual(['host']);
  });

  it('returns an empty array when nothing is templated', () => {
    const step = baseStep({
      api_endpoint_path: '/v1/static',
      api_query: { limit: '5' },
    });
    expect(collectPlaceholders(step)).toEqual([]);
  });
});

describe('substitutePlaceholders — replaces user vars, leaves runtime alone', () => {
  it('substitutes user variables in path/query/headers/body', () => {
    const step = baseStep({
      api_endpoint_path: '/{{host}}/v1',
      api_query: { account: '{{account_id}}' },
      api_headers: { 'X-User': '{{username}}' },
      api_body: { title: '{{title}}' } as unknown as WorkflowStep['api_body'],
    });
    const out = substitutePlaceholders(step, {
      host: 'fr.example.com',
      account_id: '42',
      username: 'romuald',
      title: 'My ticket',
    });
    expect(out.api_endpoint_path).toBe('/fr.example.com/v1');
    expect(out.api_query).toEqual({ account: '42' });
    expect(out.api_headers).toEqual({ 'X-User': 'romuald' });
    expect(out.api_body).toEqual({ title: 'My ticket' });
  });

  it('leaves runtime tokens untouched (they are runner-resolved)', () => {
    const step = baseStep({
      api_endpoint_path: '/{{host}}/issues/{{previous_step.summary}}',
    });
    const out = substitutePlaceholders(step, { host: 'fr.example.com' });
    expect(out.api_endpoint_path).toBe('/fr.example.com/issues/{{previous_step.summary}}');
  });

  it('leaves a placeholder alone when no value is provided (defensive)', () => {
    // If the caller forgot to fill a user var, the original token stays
    // in place — the backend will surface a clean error rather than us
    // silently dropping the reference and shipping a malformed URL.
    const step = baseStep({
      api_endpoint_path: '/{{host}}/{{missing}}',
    });
    const out = substitutePlaceholders(step, { host: 'fr.example.com' });
    expect(out.api_endpoint_path).toBe('/fr.example.com/{{missing}}');
  });

  it('walks nested objects + arrays in the body', () => {
    const step = baseStep({
      api_body: {
        title: '{{title}}',
        labels: ['{{type}}', 'static'],
        nested: { author: { login: '{{owner}}' } },
      } as unknown as WorkflowStep['api_body'],
    });
    const out = substitutePlaceholders(step, { title: 'A', type: 'bug', owner: 'alice' });
    expect(out.api_body).toEqual({
      title: 'A',
      labels: ['bug', 'static'],
      nested: { author: { login: 'alice' } },
    });
  });
});
