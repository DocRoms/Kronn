// 0.8.5 — unified quick-start catalogue for the workflow wizard.
//
// Pre-fix the wizard surfaced THREE independent "start from something"
// systems in two different places:
//   1. STARTER_TEMPLATES — plugin-aware buttons at the top of step 0
//   2. backend WorkflowSuggestion[] — project-aware cards (toggle) on step 0
//   3. WorkflowPreset[] (v0.7 presets) — primitives-mix cards buried in
//      advanced → step 2
// Three shapes, three apply paths, two locations. Discoverability was bad
// enough that users routinely missed the v07 presets entirely.
//
// This module normalises all three into a single `UnifiedQuickStart`
// shape and a single `applyQuickStart` dispatcher. Adapter functions
// map each source's native shape onto the unified one; nothing is
// invented (`audience` stays undefined for sources that don't have it,
// `applicable` is computed from the wizard's current context).

import type {
  WorkflowSuggestion,
  WorkflowStep,
} from '../types/generated';
import type { StarterTemplate } from './workflow-templates/chartbeat-top5';
import type { WorkflowPreset } from './workflow-templates/v07-presets';
import type { ApiPluginOption } from '../components/workflows/ApiCallStepCard';

/** Where this entry came from — drives the badge + the apply path. */
export type QuickStartSource = 'starter' | 'project-suggestion' | 'preset';

/** Visual complexity tier. Maps from each source's native notion:
 *  - starter: forced 'intermediate' (real demos with 3-4 steps + plugin wiring)
 *  - project-suggestion: `complexity: "simple" | "advanced"` from the backend
 *    (we expand the binary into our tri-state by treating non-advanced as
 *    intermediate when stepsCount > 1, simple otherwise)
 *  - preset: derived from stepsCount + `onFailure` presence
 *  Used to sort the picker (simple → advanced) and render a badge.
 */
export type QuickStartComplexity = 'simple' | 'intermediate' | 'advanced';

/** Tagged-union payload — the picker hands this back to the wizard which
 *  knows how to apply each source's native shape. Keeping the original
 *  objects intact means the wizard's existing apply* handlers don't
 *  need to be rewritten, only re-routed. */
export type QuickStartPayload =
  | { kind: 'starter'; template: StarterTemplate }
  | { kind: 'project-suggestion'; suggestion: WorkflowSuggestion }
  | { kind: 'preset'; preset: WorkflowPreset };

export interface UnifiedQuickStart {
  /** Stable id namespaced by source to avoid cross-source collisions. */
  id: string;
  source: QuickStartSource;
  title: string;
  description: string;
  complexity: QuickStartComplexity;
  /** Persona / job (dev / data / devops / …). Optional — only filled
   *  on backend suggestions today. */
  audience?: string;
  stepsCount: number;
  /** Short chips shown under the title — primitives, plugin slug, etc.
   *  Bounded so a preset with 8 primitives doesn't bloat the row. */
  badges: string[];
  /** True when the entry's native preconditions are met by the wizard's
   *  current context. False = entry is still listed (user might want
   *  to discover it) but greyed-out + carries an explanatory hint. */
  applicable: boolean;
  /** When `applicable === false`, a one-line reason ("requires the
   *  Chartbeat plugin", "no project selected"). */
  notApplicableReason?: string;
  /** Backend suggestions carry a project-specific "why we recommend this"
   *  blurb — shown verbatim under the description when present. */
  reason?: string;
  payload: QuickStartPayload;
}

/** Translator function signature used to resolve preset i18n keys at
 *  catalogue-build time. Mirrors `useT()` from `I18nContext` so the
 *  builder stays pure (no React dep) — the caller passes `t` in. */
type Translator = (key: string, ...args: (string | number)[]) => string;

/** Compose a fresh array from all three sources. Caller passes whatever
 *  it has (the suggestions list may be empty if no project is bound).
 *  Output is sorted simple → advanced, then by stepsCount asc, then by
 *  title — stable and predictable so the same project always shows the
 *  same order. */
export function buildQuickStartCatalogue(args: {
  starters: StarterTemplate[];
  suggestions: WorkflowSuggestion[];
  presets: WorkflowPreset[];
  /** What the wizard currently knows about the project. Drives
   *  `applicable` flags. */
  ctx: {
    projectSelected: boolean;
    availableApiPlugins: ApiPluginOption[];
  };
  /** i18n translator — used to resolve preset `titleKey` / `descKey`
   *  into human-readable strings at catalogue-build time. Without it
   *  presets render their raw `id` ("auto-dev") and i18n key
   *  ("wiz.preset.autoDev.desc") in the picker UI — caught by the
   *  Playwright wizard-presets spec on 2026-05-18. */
  t: Translator;
}): UnifiedQuickStart[] {
  const entries: UnifiedQuickStart[] = [
    ...args.starters.map(s => fromStarterTemplate(s, args.ctx)),
    ...args.suggestions.map(s => fromSuggestion(s, args.ctx)),
    ...args.presets.map(p => fromPreset(p, args.t)),
  ];

  return entries.sort(compareEntries);
}

const COMPLEXITY_ORDER: Record<QuickStartComplexity, number> = {
  simple: 0,
  intermediate: 1,
  advanced: 2,
};

function compareEntries(a: UnifiedQuickStart, b: UnifiedQuickStart): number {
  const c = COMPLEXITY_ORDER[a.complexity] - COMPLEXITY_ORDER[b.complexity];
  if (c !== 0) return c;
  if (a.stepsCount !== b.stepsCount) return a.stepsCount - b.stepsCount;
  return a.title.localeCompare(b.title, 'fr');
}

// ─── Adapter: StarterTemplate ──────────────────────────────────────────
// Starters are plugin-anchored demos (Chartbeat → résumé → Slack, …).
// Always advanced-mode wiring but conceptually entry-level UX ("clic →
// ça marche"). We classify as `intermediate` so they sit between simple
// presets and full advanced suggestions.
//
// Note: title_fr is used directly here. The picker layer doesn't know
// the current locale at construction time — we'd need to thread the
// `Lang` value through to pick `title_fr` vs `title_en`. For 0.8.5 we
// keep `title_fr` to match the previous wizard behaviour (it always
// rendered title_fr). Localising the catalogue properly is a follow-up.
function fromStarterTemplate(
  t: StarterTemplate,
  ctx: { availableApiPlugins: ApiPluginOption[] },
): UnifiedQuickStart {
  const pluginAvailable = ctx.availableApiPlugins.some(
    p => p.server.id === t.primary_plugin_slug,
  );
  return {
    id: `starter:${t.id}`,
    source: 'starter',
    title: t.title_fr,
    description: t.description_fr,
    complexity: 'intermediate',
    stepsCount: t.steps.length,
    badges: [t.primary_plugin_slug],
    applicable: pluginAvailable,
    notApplicableReason: pluginAvailable
      ? undefined
      : `requires plugin: ${t.primary_plugin_slug}`,
    payload: { kind: 'starter', template: t },
  };
}

// ─── Adapter: WorkflowSuggestion (backend, project-aware) ──────────────
// The backend computes these against the project's actual configured
// MCPs/APIs and emits a `complexity: "simple" | "advanced"` string plus
// `required_mcps`. We expand to our tri-state and feed `required_mcps`
// into the badge row so the user sees at a glance what each entry
// touches.
function fromSuggestion(
  s: WorkflowSuggestion,
  _ctx: { projectSelected: boolean },
): UnifiedQuickStart {
  // Backend uses a two-value enum; intermediate kicks in for 2–3 step
  // suggestions that the backend tagged "simple" — they're functionally
  // a bit more than a one-shot agent call.
  let complexity: QuickStartComplexity = 'simple';
  if (s.complexity === 'advanced' || s.steps.length >= 4) {
    complexity = 'advanced';
  } else if (s.steps.length >= 2) {
    complexity = 'intermediate';
  }
  return {
    id: `suggestion:${s.id}`,
    source: 'project-suggestion',
    title: s.title,
    description: s.description,
    complexity,
    audience: s.audience,
    stepsCount: s.steps.length,
    badges: s.required_mcps.slice(0, 4), // cap to keep the row compact
    applicable: true, // backend only emits relevant suggestions
    reason: s.reason,
    payload: { kind: 'project-suggestion', suggestion: s },
  };
}

// ─── Adapter: WorkflowPreset (v0.7 primitives mix) ─────────────────────
// Presets are static and project-agnostic — always applicable. We map
// complexity from stepsCount + onFailure presence: a preset with a
// rollback chain is advanced; 1-2 steps is simple; 3+ steps is
// intermediate. The icon prefix is preserved in the rendered title so
// downstream tests / users can disambiguate presets that share a
// keyword (e.g. 🎫 Ticket Autopilot vs 🎯 Big-ticket AutoPilot).
function fromPreset(p: WorkflowPreset, t: Translator): UnifiedQuickStart {
  let complexity: QuickStartComplexity;
  if (p.onFailure && p.onFailure.length > 0) {
    complexity = 'advanced';
  } else if (p.steps.length >= 3) {
    complexity = 'intermediate';
  } else {
    complexity = 'simple';
  }
  return {
    id: `preset:${p.id}`,
    source: 'preset',
    title: `${p.icon} ${t(p.titleKey)}`,
    description: t(p.descKey),
    complexity,
    stepsCount: p.steps.length,
    badges: p.primitives.slice(0, 5),
    applicable: true,
    payload: { kind: 'preset', preset: p },
  };
}

/** Convenience: same as `buildQuickStartCatalogue` but also filters by
 *  a free-text query (case-insensitive, matches title + description
 *  + badges). Useful for the picker's search input.
 */
export function filterQuickStart(
  list: UnifiedQuickStart[],
  query: string,
): UnifiedQuickStart[] {
  const q = query.trim().toLowerCase();
  if (!q) return list;
  return list.filter(e => {
    if (e.title.toLowerCase().includes(q)) return true;
    if (e.description.toLowerCase().includes(q)) return true;
    if (e.badges.some(b => b.toLowerCase().includes(q))) return true;
    if (e.audience && e.audience.toLowerCase().includes(q)) return true;
    return false;
  });
}

/** Re-export for the test file to assert on the comparator's stability
 *  without re-implementing it. Not part of the public API. */
export const __testing = { compareEntries, COMPLEXITY_ORDER };

/** Steps preview helper used by the picker's tooltip / detail panel —
 *  shows the first N step names as a hint of what the entry does
 *  before the user commits. */
export function quickStartStepsPreview(entry: UnifiedQuickStart, max = 4): string[] {
  const steps: WorkflowStep[] =
    entry.payload.kind === 'starter' ? entry.payload.template.steps :
    entry.payload.kind === 'project-suggestion' ? entry.payload.suggestion.steps :
    entry.payload.preset.steps;
  return steps.slice(0, max).map(s => s.name);
}
