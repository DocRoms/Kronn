// 0.6.0 UX pass — frontend scanner for `{{var}}` references in step
// prompts that don't match any known source. Surfaces a warning in the
// wizard so the user can either declare the var as a launch variable
// or correct a typo before saving.
//
// Why frontend-only : prompts are user-edited live, so we want immediate
// feedback. Backend keeps `validate_step_references` for `{{steps.X.Y}}`
// hard errors at save; this helper handles the broader heuristic that
// would be too noisy as a hard error.
//
// Sources considered VALID :
//   - previous_step.*               (always — runtime injected by runner)
//   - steps.<name>.*                (when <name> is an earlier step)
//   - state.*                       (runtime injected, agent emits)
//   - iter.*                        (runtime injected per iteration)
//   - artifacts.*                   (declared in workflow.artifacts)
//   - failed_step.*                 (only in `on_failure` chain)
//   - issue.* / type / *_url …      (when trigger is Tracker; we accept
//                                    them broadly because trigger contexts
//                                    inject arbitrary string fields)
//   - <bare_name>                   (when listed in workflow.variables)
//
// Anything else → undeclared.
//
// Note on `{{steps.X.Y}}` with X = unknown step : we surface that here
// too as undeclared (the backend's hard validator catches the same case
// at save, but the live warning helps fix typos before the user clicks
// Save).

import type { WorkflowStep, PromptVariable, ArtifactSpec } from '../types/generated';

const VAR_RE = /\{\{\s*([^}]+?)\s*\}\}/g;

const ALWAYS_VALID_PREFIXES = [
  'previous_step.',
  'state.',
  'iter.',
  'artifacts.',
];

/** Trigger-context fields known to be injected by the runner.
 *  - `issue.*` from tracker triggers.
 *  - Bare names like `type`, `triggered_at`, etc. are injected as-is by
 *    `inject_trigger_context`. We don't list them exhaustively because
 *    a tracker-triggered workflow can carry arbitrary string fields. */
const TRIGGER_CONTEXT_PREFIXES = ['issue.'];

export interface UndeclaredVar {
  /** Raw variable name as captured (e.g. `ticket_id`, `steps.X.summary`). */
  name: string;
  /** Why we flagged it — useful for the warning copy. */
  reason: 'unknown_step' | 'unknown_bare' | 'failed_step_outside_rollback';
}

/** Scan a single prompt string and return every `{{var}}` reference
 *  that doesn't resolve against the workflow's known sources.
 *  - `currentStepIdx` lets us check that `steps.X.Y` references only
 *    point to STRICTLY earlier steps.
 *  - `inRollback` enables `failed_step.*` (only valid in on_failure).
 *  - `triggerType` enables tracker-only fields (`issue.*`).  */
export function scanUndeclaredVars(
  prompt: string,
  opts: {
    allSteps: WorkflowStep[];
    currentStepIdx: number;
    inRollback: boolean;
    triggerType: 'Manual' | 'Cron' | 'Tracker';
    workflowVariables: PromptVariable[];
    artifacts: Record<string, ArtifactSpec>;
  },
): UndeclaredVar[] {
  const out: UndeclaredVar[] = [];
  const seen = new Set<string>();
  const stepNames = new Set(opts.allSteps.slice(0, opts.currentStepIdx).map(s => s.name));
  const declaredVarNames = new Set(opts.workflowVariables.map(v => v.name));
  const declaredArtifactKeys = new Set(Object.keys(opts.artifacts));

  let match: RegExpExecArray | null;
  // Reset regex state.
  VAR_RE.lastIndex = 0;
  while ((match = VAR_RE.exec(prompt)) !== null) {
    const raw = match[1].trim();
    // Skip pipes / filters / weird forms — we only check simple `name` or `a.b.c`.
    if (raw === '' || raw.includes('|') || raw.includes(' ')) continue;
    if (seen.has(raw)) continue;
    seen.add(raw);

    // 1) Always-valid runtime prefixes.
    if (ALWAYS_VALID_PREFIXES.some(p => raw.startsWith(p))) {
      // Sub-validation for artifacts: must reference a declared key.
      // (Undeclared artifacts render empty string at runtime — not an
      // error, but worth surfacing as a heads-up later. For now, we
      // accept any artifacts.* to keep the warning narrowly focused on
      // the high-value cases.)
      if (raw.startsWith('artifacts.')) {
        const key = raw.slice('artifacts.'.length);
        // If the artifact isn't declared, it'll render empty — let it pass
        // silently (declaring artifacts is an opt-in feature).
        if (!declaredArtifactKeys.has(key)) {
          // Intentionally not flagged.
        }
      }
      continue;
    }

    // 2) failed_step.* only valid in rollback.
    if (raw.startsWith('failed_step.')) {
      if (!opts.inRollback) {
        out.push({ name: raw, reason: 'failed_step_outside_rollback' });
      }
      continue;
    }

    // 3) steps.<name>.<field> — must reference an earlier step.
    if (raw.startsWith('steps.')) {
      const rest = raw.slice('steps.'.length);
      const stepName = rest.split('.')[0];
      if (!stepNames.has(stepName)) {
        out.push({ name: raw, reason: 'unknown_step' });
      }
      continue;
    }

    // 4) Trigger-context prefixes (issue.*) — accept broadly since
    // tracker triggers inject many fields.
    if (TRIGGER_CONTEXT_PREFIXES.some(p => raw.startsWith(p))) {
      // Only meaningful if trigger is Tracker, but we accept broadly to
      // avoid false positives when the user pre-builds the prompt.
      continue;
    }

    // 5) Bare name (no dot).
    if (!raw.includes('.')) {
      if (declaredVarNames.has(raw)) continue;
      // Some legacy/runtime-injected bare keys we don't want to flag :
      // `type`, `triggered_at` etc. come from the trigger context. We
      // accept them only when trigger is Cron/Tracker (the runner
      // injects them then). For Manual triggers, only declared
      // workflow variables are valid.
      if (opts.triggerType !== 'Manual' && (raw === 'type' || raw === 'triggered_at')) continue;
      out.push({ name: raw, reason: 'unknown_bare' });
      continue;
    }

    // 6) Anything else with a dot but no matching prefix → unknown.
    out.push({ name: raw, reason: 'unknown_bare' });
  }
  return out;
}
