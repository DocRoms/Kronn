import type { DecideConfig, ModelCatalogSnapshot, Workflow, WorkflowStep } from '../types/generated';
import type { ExternalApiConnectionView } from './api';
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
  /** The connection's name, for a model reached over HTTP. */
  connectionName?: string;
  transport: DecisionTransport;
  /** Listed but not served (the catalogue says so): shown, never selectable. */
  unavailable: boolean;
}

/** What the picker hands back; `model: null` = the configured Decision model. */
export interface DecisionModelChoice {
  model: string | null;
  connectionId: string | null;
}

/** The connection kinds with a documented decision contract: OpenRouter serves
 *  its decision models on `POST /api/v1/systemone`. LiteLLM, NVIDIA and the
 *  others have none documented, so their `decision_model` is not listed: a
 *  step could not call it (docs/operations/ollama-local-models.md, « Decision
 *  models »). Keep in step with `callable_decision_model` in the backend. */
const DECISION_PRESETS: ReadonlySet<string> = new Set(['open_router']);

/** The models a Decide step can call: the Ollama decision models of the
 *  catalogue (local), then each connection's `decision_model` the runner can
 *  call (HTTP). A connection without its credential cannot be called. */
export function decisionModelOptions(
  snapshot: ModelCatalogSnapshot | null | undefined,
  connections: readonly ExternalApiConnectionView[] = [],
): DecisionModelOption[] {
  const target = snapshot?.targets.find(view => view.runtime_target_id === modelRuntimeTargetId('Ollama'));
  const local = (target?.models ?? [])
    .filter(model => model.runtime_target_id === target?.runtime_target_id && isDecisionCapable(model.capabilities))
    .map(model => ({
      model: model.model_id,
      connectionId: null,
      label: model.display_alias ?? model.display_name ?? model.model_id,
      transport: 'local' as const,
      unavailable: model.availability === 'unavailable',
    }));
  const http = connections
    .filter(connection => DECISION_PRESETS.has(connection.origin_preset) && connection.has_credential)
    .flatMap(connection => {
      const model = connection.decision_model?.trim();
      return model ? [{
        model,
        connectionId: connection.id,
        label: model,
        connectionName: connection.display_name,
        transport: 'http' as const,
        unavailable: false,
      }] : [];
    });
  return [...local, ...http];
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
