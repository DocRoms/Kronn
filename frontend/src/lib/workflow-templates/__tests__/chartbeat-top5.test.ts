// Unit tests for the Chartbeat starter template (désagentification aha moment).
//
// These guard the shape contract the wizard's "Load template" button
// relies on. A drift here (removed field, renamed plugin slug) would
// silently produce a broken 3-step workflow at clone time.

import { describe, it, expect } from 'vitest';
import {
  CHARTBEAT_TOP5_TEMPLATE,
  STARTER_TEMPLATES,
  cloneTemplateSteps,
  assertTemplateInvariants,
} from '../chartbeat-top5';

describe('Chartbeat top-5 starter template', () => {
  it('is registered in STARTER_TEMPLATES', () => {
    expect(STARTER_TEMPLATES).toContain(CHARTBEAT_TOP5_TEMPLATE);
  });

  it('passes its own invariants', () => {
    expect(assertTemplateInvariants(CHARTBEAT_TOP5_TEMPLATE)).toEqual([]);
  });

  it('chains 3 steps in the expected order and types', () => {
    const [s1, s2, s3] = CHARTBEAT_TOP5_TEMPLATE.steps;
    expect(s1.step_type?.type).toBe('ApiCall');
    expect(s1.name).toBe('fetch_top_pages');
    expect(s2.step_type?.type).toBe('Agent');
    expect(s2.name).toBe('summarize');
    expect(s3.step_type?.type).toBe('Notify');
    expect(s3.name).toBe('notify_slack');
  });

  it('extracts an array from Chartbeat so downstream (Batch / Agent) sees a list', () => {
    const [s1] = CHARTBEAT_TOP5_TEMPLATE.steps;
    // `$.pages[*].title` returns an array per RFC 9535. Critical for the
    // Batch QP compatibility banner — a scalar path would break fan-out.
    expect(s1.api_extract?.path).toBe('$.pages[*].title');
    expect(s1.api_extract?.fail_on_empty).toBe(true);
  });

  it('agent step references the ApiCall output via {{steps.fetch_top_pages.data}}', () => {
    const [, s2] = CHARTBEAT_TOP5_TEMPLATE.steps;
    expect(s2.prompt_template).toContain('{{steps.fetch_top_pages.data}}');
  });

  it('notify step surfaces the agent summary via {{steps.summarize.output}}', () => {
    const [, , s3] = CHARTBEAT_TOP5_TEMPLATE.steps;
    expect(s3.notify_config?.body_template).toContain('{{steps.summarize.output}}');
  });

  it('cloneTemplateSteps injects the user-provided config_id into matching ApiCall steps', () => {
    const steps = cloneTemplateSteps(CHARTBEAT_TOP5_TEMPLATE, 'user-cfg-42');
    const [apiStep] = steps;
    expect(apiStep.api_config_id).toBe('user-cfg-42');
    // Other steps untouched — api_config_id stays null because they're
    // not ApiCall steps.
    expect(steps[1].api_config_id).toBeNull();
    expect(steps[2].api_config_id).toBeNull();
  });

  it('cloneTemplateSteps returns a deep copy (mutating the clone does not affect the template)', () => {
    const steps = cloneTemplateSteps(CHARTBEAT_TOP5_TEMPLATE, 'cfg-1');
    steps[0].description = 'mutated';
    expect(CHARTBEAT_TOP5_TEMPLATE.steps[0].description).toBe('Top 5 articles en direct (Chartbeat)');
  });

  it('cloneTemplateSteps with null config_id keeps the ApiCall step valid but unconfigured', () => {
    // User loads the template before wiring a plugin config — the wizard
    // should still accept the clone so the user sees the shape and
    // completes it in-place.
    const steps = cloneTemplateSteps(CHARTBEAT_TOP5_TEMPLATE, null);
    expect(steps[0].api_config_id).toBeNull();
    expect(steps[0].api_plugin_slug).toBe('chartbeat');
    expect(steps[0].api_endpoint_path).toBe('/live/toppages/v4');
  });

  it('assertTemplateInvariants catches a missing ApiCall endpoint_path', () => {
    const broken = {
      ...CHARTBEAT_TOP5_TEMPLATE,
      steps: CHARTBEAT_TOP5_TEMPLATE.steps.map((s, i) =>
        i === 0 ? { ...s, api_endpoint_path: null } : s,
      ),
    };
    const errors = assertTemplateInvariants(broken);
    expect(errors.some(e => e.includes('api_endpoint_path'))).toBe(true);
  });

  it('assertTemplateInvariants catches a missing Notify config', () => {
    const broken = {
      ...CHARTBEAT_TOP5_TEMPLATE,
      steps: CHARTBEAT_TOP5_TEMPLATE.steps.map((s, i) =>
        i === 2 ? { ...s, notify_config: null } : s,
      ),
    };
    const errors = assertTemplateInvariants(broken);
    expect(errors.some(e => e.includes('notify_config'))).toBe(true);
  });
});
