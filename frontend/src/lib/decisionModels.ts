import type { DecideConfig, ModelCatalogSnapshot, Workflow, WorkflowStep } from '../types/generated';
import { isDecisionCapable, modelRuntimeTargetId } from './modelCatalogSelection';

/** KT-1697 — how a decision model is reached: Ollama's `/v1/systemone`
 *  (`local`) or a connection's HTTP API (`http`). */
export type DecisionTransport = 'local' | 'http';

/** One model a Decide step can call. */
export interface DecisionModelOption {
  model: string;
  /** The external connection the model comes from; `null` = local Ollama. */
  connectionId: string | null;
  label: string;
  transport: DecisionTransport;
  /** Listed but not served (the catalogue says so): shown, never selectable. */
  unavailable: boolean;
}

/** What the picker hands back; `model: null` = the configured Decision model. */
export interface DecisionModelChoice {
  model: string | null;
  connectionId: string | null;
}

/** The Ollama decision models of the catalogue. A connection's `decision_model`
 *  is not listed: no HTTP provider has a verified decision contract yet, so a
 *  step could not call it (docs/operations/ollama-local-models.md, « Decision
 *  models »). `connectionId` and `transport: 'http'` are ready for the day one
 *  has. */
export function decisionModelOptions(snapshot: ModelCatalogSnapshot | null | undefined): DecisionModelOption[] {
  const target = snapshot?.targets.find(view => view.runtime_target_id === modelRuntimeTargetId('Ollama'));
  return (target?.models ?? [])
    .filter(model => model.runtime_target_id === target?.runtime_target_id && isDecisionCapable(model.capabilities))
    .map(model => ({
      model: model.model_id,
      connectionId: null,
      label: model.display_alias ?? model.display_name ?? model.model_id,
      transport: 'local' as const,
      unavailable: model.availability === 'unavailable',
    }));
}

/** The step's own choice, `null` model when it uses the configured one. */
export function decideModelChoice(config: DecideConfig | null | undefined): DecisionModelChoice {
  return { model: config?.model?.trim() || null, connectionId: config?.connection_id ?? null };
}

/** The config with the choice applied; the default drops both fields. */
export function withDecisionModel(config: DecideConfig, choice: DecisionModelChoice): DecideConfig {
  const next: DecideConfig = { ...config };
  delete next.model;
  delete next.connection_id;
  if (choice.model) next.model = choice.model;
  if (choice.model && choice.connectionId) next.connection_id = choice.connectionId;
  return next;
}

/** Saves a Decide step's model the moment it is picked, as the Agent step's
 *  picker does: `io.show` puts the choice on screen first, and puts the saved
 *  workflow back if `io.save` fails (the error is rethrown for the caller to
 *  word). `null` = nothing to save. */
export async function saveDecisionModel(
  workflow: Workflow,
  stepIndex: number,
  choice: DecisionModelChoice,
  io: { show: (workflow: Workflow) => void; save: (steps: WorkflowStep[]) => Promise<Workflow> },
): Promise<Workflow | null> {
  const step = workflow.steps[stepIndex];
  if (!step?.decide || step.step_type?.type !== 'Decide') return null;
  if (sameDecisionChoice(decideModelChoice(step.decide), choice)) return null;
  const decide = withDecisionModel(step.decide, choice);
  const steps = workflow.steps.map((current, index) => index === stepIndex ? { ...current, decide } : current);
  io.show({ ...workflow, steps });
  try {
    const updated = await io.save(steps);
    io.show(updated);
    return updated;
  } catch (error) {
    io.show(workflow);
    throw error;
  }
}

export function sameDecisionChoice(a: DecisionModelChoice, b: DecisionModelChoice): boolean {
  return (a.model ?? null) === (b.model ?? null) && (a.connectionId ?? null) === (b.connectionId ?? null);
}
