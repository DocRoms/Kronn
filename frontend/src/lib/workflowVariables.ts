// Workflow-level placeholder scanner. Walks every text field of every
// step in a workflow, extracts `{{var}}` tokens that are NOT runtime
// namespaces (`steps.X`, `batch.X.Y`, etc. — see `apiCallPlaceholders`
// for the canonical list), and returns the sorted unique identifiers.
//
// Used by the "Lancer le workflow" flow to **auto-detect** variables
// the user must supply at trigger time, even when the workflow author
// forgot to declare them in `Workflow.variables`. Closes the
// user-reported "autoBot {{issue}} unset" bug (2026-05-12): a workflow
// with `{{issue}}` in its first step's prompt would fire with the
// literal `{{issue}}` because no `variables[]` had been declared and
// the launch modal was skipped.
//
// Pure helper (no React, no fetch) so it's easy to unit-test.

import type { Workflow, WorkflowStep, PromptVariable } from '../types/generated';
import { isRuntimeToken, collectPlaceholders } from '../components/workflows/apiCallPlaceholders';

/** Extract `{{name}}` tokens from a single string. Ignores runtime
 *  namespaces. Returns the bare identifier list (no dedup). */
function tokensIn(s: string | null | undefined): string[] {
  if (!s) return [];
  const out: string[] = [];
  const matches = s.match(/\{\{([\w.]+)\}\}/g) ?? [];
  for (const m of matches) {
    const name = m.slice(2, -2);
    if (!isRuntimeToken(name)) out.push(name);
  }
  return out;
}

/** Scan a single workflow step for unbound `{{var}}` tokens. Reuses
 *  `collectPlaceholders` for the ApiCall request shape (path / query /
 *  headers / body / path params) and adds the agent + notify text
 *  fields that the ApiCall helper doesn't know about. */
export function placeholdersInStep(step: WorkflowStep): string[] {
  const found = new Set<string>(collectPlaceholders(step));
  // Agent step prompt (also covers BatchQuickPrompt's rendered template).
  for (const name of tokensIn(step.prompt_template)) {
    found.add(name);
  }
  // Notify step (url + body template). The model surfaces these via
  // `notify_config` (since 0.3.5).
  if (step.notify_config) {
    for (const name of tokensIn(step.notify_config.url)) found.add(name);
    for (const name of tokensIn(step.notify_config.body_template)) found.add(name);
    for (const v of Object.values(step.notify_config.headers ?? {})) {
      for (const name of tokensIn(v)) found.add(name);
    }
  }
  // Exec step args (templated strings, fired through allowlist-gated
  // binary). Each arg can carry `{{var}}` tokens.
  if (Array.isArray(step.exec_args)) {
    for (const arg of step.exec_args) {
      if (typeof arg === 'string') {
        for (const name of tokensIn(arg)) found.add(name);
      }
    }
  }
  // BatchQuickPrompt batch source template — `batch_items_from` carries
  // the source spec (often a templated path or step output expression).
  if (typeof step.batch_items_from === 'string') {
    for (const name of tokensIn(step.batch_items_from)) found.add(name);
  }
  return [...found];
}

/** Walk every step in a workflow, return the sorted unique list of
 *  unbound `{{var}}` identifiers. Empty for workflows whose templates
 *  only use runtime namespaces (`{{steps.audit.output}}` etc.). */
export function collectUnboundVariables(workflow: Workflow): string[] {
  const all = new Set<string>();
  for (const step of workflow.steps ?? []) {
    for (const name of placeholdersInStep(step)) {
      all.add(name);
    }
  }
  return [...all].sort();
}

/** Merge declared `Workflow.variables` with auto-detected unbound
 *  identifiers. Declared variables WIN (description, default, kind are
 *  preserved). Auto-detected ones are emitted as synthetic
 *  `PromptVariable` records with no description and no default — the
 *  launch modal renders them as free-text inputs.
 *
 *  Returns the merged ordered list (declared first, in their original
 *  order; auto-detected next, alphabetical) so the user sees the
 *  intentional variables first. */
export function mergeDeclaredAndDetected(workflow: Workflow): PromptVariable[] {
  const declared = workflow.variables ?? [];
  const declaredNames = new Set(declared.map(v => v.name));
  const detected = collectUnboundVariables(workflow);
  const autoDetected: PromptVariable[] = detected
    .filter(name => !declaredNames.has(name))
    .map(name => ({
      name,
      // Friendly default label = the bare identifier. The user can
      // recognize their own placeholder name; the modal also shows it
      // next to the input.
      label: name,
      placeholder: '',
      description: 'Auto-detected from a step template ({{' + name + '}}).',
      required: true,
    }));
  return [...declared, ...autoDetected];
}
