// 0.8.5 — unit tests pinning the 3-source adapter + catalogue
// sort/filter contract that the WorkflowQuickStartPicker depends on.

import { describe, it, expect } from 'vitest';
import {
  buildQuickStartCatalogue,
  filterQuickStart,
  quickStartStepsPreview,
  __testing,
  type UnifiedQuickStart,
} from '../workflow-quick-start';
import type { StarterTemplate } from '../workflow-templates/chartbeat-top5';
import type { WorkflowPreset } from '../workflow-templates/v07-presets';
import type { WorkflowSuggestion, WorkflowStep } from '../../types/generated';
import type { ApiPluginOption } from '../../components/workflows/ApiCallStepCard';

// ─── Fixture helpers ───────────────────────────────────────────────────

function makeStep(name: string): WorkflowStep {
  return {
    name,
    agent: 'ClaudeCode',
    prompt_template: '',
    mode: { type: 'Normal' },
  } as WorkflowStep;
}

function makeStarter(overrides: Partial<StarterTemplate> = {}): StarterTemplate {
  return {
    id: 'starter-id',
    title_fr: 'Démo Chartbeat',
    title_en: 'Chartbeat demo',
    description_fr: 'Récupère le top 5 et résume.',
    description_en: 'Pulls top 5 and summarises.',
    primary_plugin_slug: 'mcp-chartbeat',
    steps: [makeStep('fetch'), makeStep('summarize'), makeStep('notify')],
    ...overrides,
  };
}

function makeSuggestion(overrides: Partial<WorkflowSuggestion> = {}): WorkflowSuggestion {
  return {
    id: 'sug-1',
    title: 'Triage des issues',
    description: 'Triage automatique des nouvelles issues.',
    reason: 'Le projet a un tracker GitHub configuré.',
    required_mcps: ['mcp-github'],
    audience: 'dev',
    complexity: 'simple',
    trigger: { type: 'Manual' } as WorkflowSuggestion['trigger'],
    steps: [makeStep('triage')],
    ...overrides,
  };
}

function makePreset(overrides: Partial<WorkflowPreset> = {}): WorkflowPreset {
  return {
    id: 'auto-dev',
    icon: '🤖',
    titleKey: 'wiz.preset.autoDev.title',
    descKey: 'wiz.preset.autoDev.desc',
    primitives: ['Agent', 'Exec'],
    steps: [makeStep('main'), makeStep('test')],
    ...overrides,
  };
}

function makePlugin(slug: string): ApiPluginOption {
  return {
    server: { id: slug } as ApiPluginOption['server'],
    config: { id: `cfg-${slug}` } as ApiPluginOption['config'],
  };
}

// Minimal i18n translator stub. The real `useT()` resolves keys against
// `i18n.ts`; for these unit tests we return a deterministic synthetic
// string per key so we can pin "key X was resolved" without depending
// on the live translation table. `wiz.preset.<id>.title` keys map to
// the segment after the last dot for a readable assertion target.
function tStub(key: string, ..._args: (string | number)[]): string {
  if (key.endsWith('.title')) {
    const segments = key.split('.');
    return segments[segments.length - 2] ?? key;
  }
  return key;
}

const emptyCtx = {
  projectSelected: false,
  availableApiPlugins: [] as ApiPluginOption[],
};

// ─── Tests ─────────────────────────────────────────────────────────────

describe('buildQuickStartCatalogue', () => {
  it('returns an empty array when no sources are given', () => {
    const out = buildQuickStartCatalogue({ starters: [], suggestions: [], presets: [], ctx: emptyCtx, t: tStub });
    expect(out).toEqual([]);
  });

  it('aggregates entries from all three sources', () => {
    const out = buildQuickStartCatalogue({
      starters: [makeStarter()],
      suggestions: [makeSuggestion()],
      presets: [makePreset()],
      ctx: emptyCtx,
      t: tStub,
    });
    expect(out).toHaveLength(3);
    expect(out.map(e => e.source).sort()).toEqual(
      ['preset', 'project-suggestion', 'starter'].sort(),
    );
  });

  it('namespaces ids per source to avoid collisions', () => {
    const out = buildQuickStartCatalogue({
      starters: [makeStarter({ id: 'same' })],
      suggestions: [makeSuggestion({ id: 'same' })],
      presets: [makePreset({ id: 'auto-dev' })],
      ctx: emptyCtx,
      t: tStub,
    });
    const ids = out.map(e => e.id);
    expect(new Set(ids).size).toBe(ids.length); // unique
    expect(ids).toContain('starter:same');
    expect(ids).toContain('suggestion:same');
    expect(ids).toContain('preset:auto-dev');
  });

  it('sorts simple → intermediate → advanced, then by stepsCount asc, then title', () => {
    const entries = buildQuickStartCatalogue({
      starters: [],
      suggestions: [
        makeSuggestion({ id: 'a', title: 'Z-simple-1step', complexity: 'simple', steps: [makeStep('s')] }),
        makeSuggestion({ id: 'b', title: 'A-advanced-2', complexity: 'advanced', steps: [makeStep('s1'), makeStep('s2')] }),
        makeSuggestion({ id: 'c', title: 'M-simple-1step', complexity: 'simple', steps: [makeStep('s')] }),
      ],
      presets: [],
      ctx: emptyCtx,
      t: tStub,
    });
    // Both 'a' and 'c' are simple/1step → tie-break on title (M before Z).
    expect(entries.map(e => e.id)).toEqual(['suggestion:c', 'suggestion:a', 'suggestion:b']);
  });

  it('flags starters as not applicable when the primary plugin is missing', () => {
    const out = buildQuickStartCatalogue({
      starters: [makeStarter({ primary_plugin_slug: 'mcp-chartbeat' })],
      suggestions: [],
      presets: [],
      ctx: { projectSelected: false, availableApiPlugins: [] },
      t: tStub,
    });
    expect(out[0].applicable).toBe(false);
    expect(out[0].notApplicableReason).toMatch(/mcp-chartbeat/);
  });

  it('flags starters as applicable when the primary plugin IS available', () => {
    const out = buildQuickStartCatalogue({
      starters: [makeStarter({ primary_plugin_slug: 'mcp-chartbeat' })],
      suggestions: [],
      presets: [],
      ctx: { projectSelected: false, availableApiPlugins: [makePlugin('mcp-chartbeat')] },
      t: tStub,
    });
    expect(out[0].applicable).toBe(true);
    expect(out[0].notApplicableReason).toBeUndefined();
  });

  it('promotes a 4+ step "simple" suggestion to advanced', () => {
    // Backend says simple but the chain is long — picker treats as advanced
    // so it sorts at the bottom and badges accordingly.
    const out = buildQuickStartCatalogue({
      starters: [],
      suggestions: [makeSuggestion({
        complexity: 'simple',
        steps: [makeStep('a'), makeStep('b'), makeStep('c'), makeStep('d')],
      })],
      presets: [],
      ctx: emptyCtx,
      t: tStub,
    });
    expect(out[0].complexity).toBe('advanced');
  });

  it('promotes a 2-3 step "simple" suggestion to intermediate', () => {
    const out = buildQuickStartCatalogue({
      starters: [],
      suggestions: [makeSuggestion({
        complexity: 'simple',
        steps: [makeStep('a'), makeStep('b')],
      })],
      presets: [],
      ctx: emptyCtx,
      t: tStub,
    });
    expect(out[0].complexity).toBe('intermediate');
  });

  it('flags a preset with on_failure as advanced regardless of step count', () => {
    const out = buildQuickStartCatalogue({
      starters: [],
      suggestions: [],
      presets: [makePreset({
        steps: [makeStep('s1')], // 1 step
        onFailure: [makeStep('rollback')],
      })],
      ctx: emptyCtx,
      t: tStub,
    });
    expect(out[0].complexity).toBe('advanced');
  });

  it('preserves suggestion reason on the unified entry', () => {
    const out = buildQuickStartCatalogue({
      starters: [],
      suggestions: [makeSuggestion({ reason: 'Le projet utilise déjà Slack.' })],
      presets: [],
      ctx: emptyCtx,
      t: tStub,
    });
    expect(out[0].reason).toBe('Le projet utilise déjà Slack.');
  });

  it('caps starter badges + suggestion mcps to keep rows compact', () => {
    const out = buildQuickStartCatalogue({
      starters: [],
      suggestions: [makeSuggestion({
        required_mcps: ['m1', 'm2', 'm3', 'm4', 'm5', 'm6'],
      })],
      presets: [],
      ctx: emptyCtx,
      t: tStub,
    });
    expect(out[0].badges).toHaveLength(4); // capped at 4
  });

  it('payload roundtrips the native objects verbatim', () => {
    const starter = makeStarter();
    const suggestion = makeSuggestion();
    const preset = makePreset();
    const out = buildQuickStartCatalogue({
      starters: [starter],
      suggestions: [suggestion],
      presets: [preset],
      ctx: emptyCtx,
      t: tStub,
    });
    const byKind = Object.fromEntries(out.map(e => [e.payload.kind, e.payload]));
    expect((byKind['starter'] as { template: StarterTemplate }).template).toBe(starter);
    expect((byKind['project-suggestion'] as { suggestion: WorkflowSuggestion }).suggestion).toBe(suggestion);
    expect((byKind['preset'] as { preset: WorkflowPreset }).preset).toBe(preset);
  });
});

describe('filterQuickStart', () => {
  function fixture(): UnifiedQuickStart[] {
    return buildQuickStartCatalogue({
      starters: [makeStarter({ id: 's', title_fr: 'Chartbeat top 5' })],
      suggestions: [makeSuggestion({ id: 'g', title: 'Triage des issues' })],
      presets: [makePreset({ id: 'auto-dev' })],
      ctx: emptyCtx,
      t: tStub,
    });
  }

  it('returns the input untouched on empty query', () => {
    const list = fixture();
    expect(filterQuickStart(list, '')).toEqual(list);
    expect(filterQuickStart(list, '   ')).toEqual(list);
  });

  it('matches the title case-insensitively', () => {
    const out = filterQuickStart(fixture(), 'CHARTBEAT');
    expect(out).toHaveLength(1);
    expect(out[0].source).toBe('starter');
  });

  it('matches the description', () => {
    const out = filterQuickStart(fixture(), 'triage');
    expect(out).toHaveLength(1);
    expect(out[0].source).toBe('project-suggestion');
  });

  it('matches a badge', () => {
    const out = filterQuickStart(fixture(), 'mcp-github');
    expect(out).toHaveLength(1);
    expect(out[0].source).toBe('project-suggestion');
  });

  it('returns empty when no entry matches', () => {
    expect(filterQuickStart(fixture(), 'zzzzzzzzz')).toEqual([]);
  });
});

describe('quickStartStepsPreview', () => {
  it('returns the first N step names', () => {
    const entries = buildQuickStartCatalogue({
      starters: [makeStarter({ steps: [makeStep('a'), makeStep('b'), makeStep('c'), makeStep('d'), makeStep('e')] })],
      suggestions: [],
      presets: [],
      ctx: emptyCtx,
      t: tStub,
    });
    expect(quickStartStepsPreview(entries[0], 3)).toEqual(['a', 'b', 'c']);
  });

  it('returns all step names if the chain is shorter than max', () => {
    const entries = buildQuickStartCatalogue({
      starters: [],
      suggestions: [makeSuggestion({ steps: [makeStep('only')] })],
      presets: [],
      ctx: emptyCtx,
      t: tStub,
    });
    expect(quickStartStepsPreview(entries[0], 4)).toEqual(['only']);
  });
});

describe('comparator stability', () => {
  it('ranks complexities simple < intermediate < advanced', () => {
    expect(__testing.COMPLEXITY_ORDER.simple)
      .toBeLessThan(__testing.COMPLEXITY_ORDER.intermediate);
    expect(__testing.COMPLEXITY_ORDER.intermediate)
      .toBeLessThan(__testing.COMPLEXITY_ORDER.advanced);
  });
});
