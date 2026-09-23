import { describe, it, expect } from 'vitest';
import type { ExternalApiConnectionView } from '../api';
import { externalAgentColor, externalAgentTargets, unusableExternalAgentTargets } from '../externalAgentIdentity';

describe('externalAgentColor', () => {
  it('gives two connections two different colours', () => {
    // Every external connection is `Custom` on the wire, so the static colour
    // table gave them all the same fallback violet.
    const a = externalAgentColor('@openrouter');
    const b = externalAgentColor('@groq');
    expect(a).not.toBeNull();
    expect(a).not.toBe(b);
  });

  it('is stable for a given alias', () => {
    expect(externalAgentColor('@openrouter')).toBe(externalAgentColor('openrouter'));
  });

  it('returns null with nothing to derive from, so the caller keeps its fallback', () => {
    expect(externalAgentColor(null)).toBeNull();
    expect(externalAgentColor('  ')).toBeNull();
    expect(externalAgentColor('@')).toBeNull();
  });

  it('stays within a legible band on both themes', () => {
    // Only the hue varies; saturation and lightness are fixed on purpose.
    expect(externalAgentColor('@anything')).toMatch(/^hsl\(\d{1,3}, 62%, 52%\)$/);
  });
});

describe('external agent targets', () => {
  const connection = (overrides: Partial<ExternalApiConnectionView>): ExternalApiConnectionView => ({
    id: 'conn-1',
    display_name: 'OpenRouter',
    mention_alias: 'openrouter',
    endpoint: 'https://openrouter.ai/api/v1',
    origin_preset: 'open_router',
    has_credential: true,
    economy_model: null,
    default_model: 'a/model',
    reasoning_model: null,
    image_model: null,
    video_model: null,
    media_endpoint: null,
    created_at: '2026-09-19T10:00:00Z',
    updated_at: '2026-09-19T10:00:00Z',
    ...overrides,
  } as ExternalApiConnectionView);

  it('offers a connection that has an endpoint and at least one model', () => {
    const targets = externalAgentTargets([connection({})]);
    expect(targets.map(target => target.connectionId)).toEqual(['conn-1']);
    expect(unusableExternalAgentTargets([connection({})])).toEqual([]);
  });

  it('says why an unfinished connection cannot be a target, instead of hiding it', () => {
    const noModel = connection({ id: 'conn-2', display_name: 'Groq', default_model: null });
    const noEndpoint = connection({ id: 'conn-3', display_name: 'Local', endpoint: '' });
    expect(externalAgentTargets([noModel, noEndpoint])).toEqual([]);
    expect(unusableExternalAgentTargets([noModel, noEndpoint])).toEqual([
      { connectionId: 'conn-2', label: 'Groq', reason: 'no_model' },
      { connectionId: 'conn-3', label: 'Local', reason: 'no_endpoint' },
    ]);
  });

  it('leaves a native preset out of both lists: it already has its own agent', () => {
    const native = connection({ id: 'conn-4', origin_preset: 'lite_llm', default_model: null });
    expect(externalAgentTargets([native])).toEqual([]);
    expect(unusableExternalAgentTargets([native])).toEqual([]);
  });
});
