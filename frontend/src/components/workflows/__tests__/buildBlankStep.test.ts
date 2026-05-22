// 0.8.6 phase 4 — Pure unit tests for `buildBlankStep` extracted from
// WorkflowWizard. Closes audit gap #3 (2026-05-22) :
//
// Pre-extraction, `blankStep` was a closure inside the wizard component,
// untestable without rendering the whole 3000-line component. A refactor
// could break the strict semantic (default tier only applied to NEW steps,
// never to existing ones loaded from editWorkflow) and the bug would only
// surface in production.
//
// The contract pinned here :
//   - `defaultTier === 'default' | null` → NO `agent_settings` field set
//     (runner falls back to its built-in default, no `--model` flag passed)
//   - `defaultTier === 'economy' | 'reasoning'` → step gains
//     `agent_settings: { tier: <chosen> }`
//   - Step name uses `step-{N+1}` convention so caller can pass
//     `steps.length` consistently for both append + insert flows
//   - Default output_format is `Structured` so chained steps can read
//     `.data` / `.summary` from the very first save

import { describe, it, expect } from 'vitest';
import { buildBlankStep } from '../WorkflowWizard';

describe('buildBlankStep — default tier semantic', () => {
  it('NULL defaultTier : no agent_settings field set (legacy-compatible)', () => {
    // Mount-time fallback before configApi.getServerConfig resolves.
    // The step must NOT have agent_settings so the runner stays on
    // its built-in default model (no `--model` flag passed). Any
    // refactor introducing `agent_settings: {}` here would silently
    // change behaviour for users who haven't visited Settings yet.
    const step = buildBlankStep(0, null);
    expect(step.agent_settings).toBeUndefined();
    expect(step.agent).toBe('ClaudeCode');
    expect(step.name).toBe('step-1');
    expect(step.prompt_template).toBe('');
  });

  it('"default" defaultTier : no agent_settings field set (matches null)', () => {
    // User opened Settings but kept the picker on "default". Same
    // behaviour as never having opened Settings — empty agent_settings,
    // runner uses built-in default. The if-guard `tier !== 'default'`
    // protects this.
    const step = buildBlankStep(0, 'default');
    expect(step.agent_settings).toBeUndefined();
  });

  it('"economy" defaultTier : agent_settings.tier = "economy" on the new step', () => {
    // The user explicitly picked Economy in Settings. Every NEW step
    // inherits it via agent_settings.tier ; the runner will resolve
    // to the agent's economy model (haiku, gpt-5-codex-mini, …).
    const step = buildBlankStep(0, 'economy');
    expect(step.agent_settings).toBeDefined();
    expect(step.agent_settings?.tier).toBe('economy');
  });

  it('"reasoning" defaultTier : agent_settings.tier = "reasoning"', () => {
    const step = buildBlankStep(0, 'reasoning');
    expect(step.agent_settings).toBeDefined();
    expect(step.agent_settings?.tier).toBe('reasoning');
  });

  it('step name increments correctly with existingCount (1-indexed)', () => {
    // Caller passes `steps.length`. step name = `step-{N+1}`.
    expect(buildBlankStep(0, null).name).toBe('step-1');
    expect(buildBlankStep(1, null).name).toBe('step-2');
    expect(buildBlankStep(7, null).name).toBe('step-8');
  });

  it('output_format defaults to Structured (chain-friendly)', () => {
    // Without this, downstream steps reading `{{steps.X.data}}` would
    // get blank — defeats the whole "chain steps" use case.
    const step = buildBlankStep(0, null);
    expect(step.output_format).toEqual({ type: 'Structured' });
  });

  it('mode defaults to Normal (Linear, no loop / no orchestrator)', () => {
    const step = buildBlankStep(0, null);
    expect(step.mode).toEqual({ type: 'Normal' });
  });

  it('agent defaults to ClaudeCode (most common in user base)', () => {
    // Documents the inherent default rather than locking it in stone.
    // If we change the org-default agent later, this test fires and
    // we update it intentionally.
    const step = buildBlankStep(0, null);
    expect(step.agent).toBe('ClaudeCode');
  });

  it('STRICT semantic : agent_settings is the ONLY field changed by defaultTier', () => {
    // Defensive : the tier choice MUST NOT bleed into other step fields.
    // A regression where (e.g.) prompt_template carries the tier-name
    // would be hilarious AND catastrophic.
    const nullStep = buildBlankStep(0, null);
    const economyStep = buildBlankStep(0, 'economy');
    const reasoningStep = buildBlankStep(0, 'reasoning');
    // Every field except agent_settings is identical across tiers.
    for (const step of [economyStep, reasoningStep]) {
      expect(step.name).toBe(nullStep.name);
      expect(step.agent).toBe(nullStep.agent);
      expect(step.prompt_template).toBe(nullStep.prompt_template);
      expect(step.mode).toEqual(nullStep.mode);
      expect(step.output_format).toEqual(nullStep.output_format);
    }
  });
});
