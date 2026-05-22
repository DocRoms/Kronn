import { describe, it, expect, vi } from 'vitest';
import { sanitizeQpImproverPayload, VALID_QP_TIERS } from '../qp-improver-sanitize';

describe('sanitizeQpImproverPayload', () => {
  // ── id stripping ────────────────────────────────────────────────────

  it('strips `id` even if the agent emitted it', () => {
    const p = { id: 'qp-abc', name: 'X', prompt_template: 'Y' };
    sanitizeQpImproverPayload(p);
    expect('id' in p).toBe(false);
  });

  it('leaves the payload intact when `id` was absent', () => {
    const p: Record<string, unknown> = { name: 'X', prompt_template: 'Y' };
    sanitizeQpImproverPayload(p);
    expect(p.name).toBe('X');
    expect(p.prompt_template).toBe('Y');
  });

  // ── null → default coercion (non-Option backend fields) ─────────────

  it('coerces null `description` to empty string', () => {
    const p = { description: null };
    sanitizeQpImproverPayload(p);
    expect(p.description).toBe('');
  });

  it('coerces null `variables` to empty array', () => {
    const p = { variables: null };
    sanitizeQpImproverPayload(p);
    expect(p.variables).toEqual([]);
  });

  it('coerces null `skill_ids` / `profile_ids` / `directive_ids` to []', () => {
    const p = { skill_ids: null, profile_ids: null, directive_ids: null };
    sanitizeQpImproverPayload(p);
    expect(p.skill_ids).toEqual([]);
    expect(p.profile_ids).toEqual([]);
    expect(p.directive_ids).toEqual([]);
  });

  it('coerces undefined `variables` to empty array (in case the agent left it out and the default kicks in)', () => {
    const p: Record<string, unknown> = { name: 'X', prompt_template: 'Y' };
    sanitizeQpImproverPayload(p);
    // After sanitize, the key is added with the default — defends
    // against axum future-proofing if `#[serde(default)]` is dropped.
    expect(p.variables).toEqual([]);
  });

  it('preserves non-null `description` verbatim', () => {
    const p = { description: 'A nice prompt that does X.' };
    sanitizeQpImproverPayload(p);
    expect(p.description).toBe('A nice prompt that does X.');
  });

  it('preserves non-empty `variables` array verbatim', () => {
    const p = { variables: [{ name: 'ticket', description: 'EW-123' }] };
    sanitizeQpImproverPayload(p);
    expect(p.variables).toEqual([{ name: 'ticket', description: 'EW-123' }]);
  });

  // ── Option<> fields stay alone ──────────────────────────────────────

  it('does NOT touch `icon` when null (Option<String> backend)', () => {
    const p = { icon: null };
    sanitizeQpImproverPayload(p);
    expect(p.icon).toBeNull();
  });

  it('does NOT touch `agent` when null (Option<AgentType> backend)', () => {
    const p = { agent: null };
    sanitizeQpImproverPayload(p);
    expect(p.agent).toBeNull();
  });

  it('does NOT touch `project_id` when null (Option<String> backend)', () => {
    const p = { project_id: null };
    sanitizeQpImproverPayload(p);
    expect(p.project_id).toBeNull();
  });

  // ── tier normalisation ──────────────────────────────────────────────

  it('keeps valid `tier` values verbatim', () => {
    for (const valid of VALID_QP_TIERS) {
      const p: Record<string, unknown> = { tier: valid };
      sanitizeQpImproverPayload(p);
      expect(p.tier).toBe(valid);
    }
  });

  it('lowercases case-insensitive valid tiers', () => {
    const p: Record<string, unknown> = { tier: 'REASONING' };
    sanitizeQpImproverPayload(p);
    expect(p.tier).toBe('reasoning');
  });

  it('normalises unknown tier "standard" to "default" (the regression that landed live 2026-05-21)', () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const p: Record<string, unknown> = { tier: 'standard' };
    sanitizeQpImproverPayload(p);
    expect(p.tier).toBe('default');
    // Warning was emitted so power users can trace what happened.
    expect(warnSpy).toHaveBeenCalled();
    warnSpy.mockRestore();
  });

  it('normalises other common hallucinations (medium, balanced, premium)', () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    for (const bogus of ['medium', 'balanced', 'premium', 'high', 'low', 'pro']) {
      const p: Record<string, unknown> = { tier: bogus };
      sanitizeQpImproverPayload(p);
      expect(p.tier).toBe('default');
    }
    warnSpy.mockRestore();
  });

  it('leaves null `tier` alone (Option<ModelTier> backend, server default kicks in)', () => {
    const p: Record<string, unknown> = { tier: null };
    sanitizeQpImproverPayload(p);
    expect(p.tier).toBeNull();
  });

  // ── Idempotence (running twice == once) ─────────────────────────────

  it('is idempotent — running twice produces the same output', () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const p: Record<string, unknown> = {
      id: 'qp-strip-me',
      description: null,
      variables: null,
      skill_ids: null,
      profile_ids: null,
      directive_ids: null,
      tier: 'STANDARD',
      icon: null,
      agent: null,
      name: 'My QP',
      prompt_template: 'Hello {{name}}',
    };
    sanitizeQpImproverPayload(p);
    const snapshot = JSON.stringify(p);
    sanitizeQpImproverPayload(p);
    expect(JSON.stringify(p)).toBe(snapshot);
    warnSpy.mockRestore();
  });

  // ── End-to-end : the real-world payload from the live bug ───────────

  it('end-to-end : the live 2026-05-21 payload (tier=standard, description=null) lands clean', () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const realWorld: Record<string, unknown> = {
      id: 'qp-old-id-from-agent',
      name: 'qp-jira-fetch-improved',
      icon: '🎫',
      prompt_template: 'Fetch the issue {{ticket}} from Jira.',
      variables: [
        { name: 'ticket', description: 'Jira issue key like EW-123' },
      ],
      agent: 'ClaudeCode',
      project_id: null,
      skill_ids: ['s1', 's2'],
      profile_ids: [],
      directive_ids: null,    // ← was the failure case
      tier: 'standard',       // ← was the failure case
      description: null,      // ← was the failure case
    };
    sanitizeQpImproverPayload(realWorld);
    expect('id' in realWorld).toBe(false);
    expect(realWorld.description).toBe('');
    expect(realWorld.directive_ids).toEqual([]);
    expect(realWorld.tier).toBe('default');
    // Untouched fields stayed intact.
    expect(realWorld.agent).toBe('ClaudeCode');
    expect(realWorld.skill_ids).toEqual(['s1', 's2']);
    warnSpy.mockRestore();
  });
});
