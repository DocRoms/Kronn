import { describe, expect, it } from 'vitest';
import type { CatalogModelEntry, ModelCatalogView } from '../../types/generated';
import { catalogModelOptions, catalogModelProvenance, catalogTierEntry, modelRuntimeTargetId } from '../modelCatalogSelection';

function entry(id: string, patch: Partial<CatalogModelEntry> = {}): CatalogModelEntry {
  return {
    id, model_id: id, runtime_target_id: 'http:one', agent_type: 'Custom', display_name: id,
    provenance: 'live', availability: 'available', capabilities: ['chat'], reasoning_modes: [],
    tier_assignment: null, manual_origin: false, first_seen_at: '2026-09-09T00:00:00Z',
    last_checked_at: '2026-09-09T00:00:00Z', created_at: '2026-09-09T00:00:00Z', updated_at: '2026-09-09T00:00:00Z',
    ...patch,
  };
}
function view(models: CatalogModelEntry[]): ModelCatalogView {
  return { runtime_target_id: 'http:one', agent_type: 'Custom', stale: false, live_refresh_ok: true, models };
}

describe('catalogue selection contract', () => {
  it('namespaces a connection independently of its agent family', () => {
    expect(modelRuntimeTargetId('Codex')).toBe('agent:codex');
    expect(modelRuntimeTargetId('OpenCode')).toBe('agent:opencode');
    expect(modelRuntimeTargetId('LiteLlm', 'one')).toBe('http:one');
    expect(modelRuntimeTargetId('Custom', 'one')).toBe('http:one');
  });

  it('keeps an explicit unavailable model instead of substituting an available assignment', () => {
    const configured = entry('configured', { availability: 'unavailable' });
    const target = view([entry('assigned', { tier_assignment: 'reasoning' }), configured]);
    expect(catalogTierEntry(target, 'reasoning', 'configured')).toBe(configured);
    expect(catalogTierEntry(target, 'reasoning', 'unknown')).toBeUndefined();
  });

  it('uses the default assignment only when the caller permits an HTTP fallback', () => {
    const standard = entry('standard', { tier_assignment: 'default' });
    const target = view([standard]);
    expect(catalogTierEntry(target, 'reasoning', '')).toBeUndefined();
    expect(catalogTierEntry(target, 'reasoning', '', true)).toBe(standard);
    expect(catalogTierEntry(target, 'reasoning', 'missing', true)).toBeUndefined();
  });

  it('does not resolve entries from another runtime even when model IDs match', () => {
    const target = view([entry('same', { runtime_target_id: 'http:two', tier_assignment: 'reasoning' })]);
    expect(catalogTierEntry(target, 'reasoning', 'same')).toBeUndefined();
    expect(catalogTierEntry(target, 'reasoning', '')).toBeUndefined();
    expect(catalogTierEntry(undefined, 'reasoning', '')).toBeUndefined();
  });

  it('downgrades live provenance for stale or failed snapshots without relabelling manual entries', () => {
    const live = entry('live');
    const target = view([live]);
    expect(catalogModelProvenance(live, target)).toBe('live');
    expect(catalogModelProvenance(live, { ...target, stale: true })).toBe('cached');
    expect(catalogModelProvenance(live, { ...target, live_refresh_ok: false })).toBe('cached');
    expect(catalogModelProvenance(live, undefined)).toBe('cached');
    expect(catalogModelProvenance(entry('manual', { provenance: 'manual' }), { ...target, stale: true })).toBe('manual');
    expect(catalogModelProvenance(entry('migrated', { provenance: 'migrated' }), target)).toBe('migrated');
  });

  it('keeps an absent configured identity as a disabled option without adopting a foreign row', () => {
    const target = view([entry('same', { runtime_target_id: 'http:two' })]);
    const options = catalogModelOptions(target, 'same', key => key, () => '');
    expect(options).toEqual([{
      value: 'same', label: 'same — modelCatalog.notInCatalog', keywords: 'same',
      description: 'modelCatalog.keepConfigured', disabled: true,
    }]);
  });

  it('includes exact ID, alias, reasoning, freshness, unavailable reason, cost and privacy metadata', () => {
    const unavailable = entry('exact/id', {
      display_name: 'Technical name', display_alias: 'Alias', reasoning_modes: ['high'],
      availability: 'unavailable', unavailable_reason: 'disappeared', unavailable_detail: 'No longer listed',
      privacy_note: 'Operator note', cost_hint: 'paid',
    });
    const [option] = catalogModelOptions({ ...view([unavailable]), stale: true }, 'exact/id', (key, ...args) => [key, ...args].join(' '), () => 'Observed $0.01');
    expect(option.label).toBe('Alias — modelCatalog.unavailable');
    expect(option.disabled).toBe(true);
    for (const text of ['exact/id', 'modelCatalog.provenance.cached', unavailable.last_checked_at, 'high', 'No longer listed', 'Observed $0.01', 'Operator note']) {
      expect(option.description).toContain(text);
    }
    expect(option.keywords).toContain('Technical name');
    expect(catalogModelOptions(undefined, '', key => key, () => '')).toEqual([]);
  });
});
