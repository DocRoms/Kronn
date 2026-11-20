import { describe, expect, it, vi } from 'vitest';
import {
  decideModelChoice,
  decisionModelOptions,
  saveDecisionModel,
  withDecisionModel,
} from '../decisionModels';
import type { DecideConfig, ModelCatalogSnapshot, Workflow } from '../../types/generated';
import type { ExternalApiConnectionView } from '../api';

const entry = (model_id: string, capabilities: string[], extra: Record<string, unknown> = {}) => ({
  model_id, display_name: model_id, display_alias: null, capabilities,
  runtime_target_id: 'agent:ollama', availability: 'available', ...extra,
});

const snapshot = (models: unknown[], runtime = 'agent:ollama') => ({
  targets: [{ runtime_target_id: runtime, agent_type: 'Ollama', models }],
}) as unknown as ModelCatalogSnapshot;

const config: DecideConfig = { state: 's', questions: [{ id: 'ship', kind: 'noul', instructions: 'Ready?' }] };

describe('decisionModelOptions (KT-1697)', () => {
  it('lists the Ollama decision models only, local, unavailable ones flagged', () => {
    const options = decisionModelOptions(snapshot([
      entry('clef-flash:9b', ['decision']),
      entry('qwen3:8b', ['chat']),
      entry('clef-old:3b', ['decision'], { availability: 'unavailable' }),
    ]));
    expect(options.map(option => option.model)).toEqual(['clef-flash:9b', 'clef-old:3b']);
    expect(options.every(option => option.transport === 'local' && option.connectionId === null)).toBe(true);
    expect(options[1].unavailable).toBe(true);
  });

  it('does not read a model from another runtime target of the catalogue', () => {
    const http = snapshot([entry('perplexity/pplx-decider-flash-v1.2', ['decision'], {
      runtime_target_id: 'http:conn-1',
    })], 'http:conn-1');
    expect(decisionModelOptions(http)).toEqual([]);
    expect(decisionModelOptions(null)).toEqual([]);
  });

  const connection = (extra: Record<string, unknown> = {}) => ({
    id: '731fe83a', display_name: 'OpenRouter', origin_preset: 'open_router',
    decision_model: 'perplexity/pplx-decider-flash-v1.2', has_credential: true, ...extra,
  }) as unknown as ExternalApiConnectionView;

  it('lists an OpenRouter connection\'s decision model after the local ones, over HTTP', () => {
    const options = decisionModelOptions(snapshot([entry('clef-flash:9b', ['decision'])]), [connection()]);
    expect(options.map(option => [option.model, option.connectionId, option.transport])).toEqual([
      ['clef-flash:9b', null, 'local'],
      ['perplexity/pplx-decider-flash-v1.2', '731fe83a', 'http'],
    ]);
    expect(options[1].connectionName).toBe('OpenRouter');
    expect(options[1].unavailable).toBe(false);
  });

  it('lists no connection whose provider has no documented decision contract, or no credential, or no model', () => {
    const options = decisionModelOptions(snapshot([]), [
      connection({ id: 'lite', origin_preset: 'lite_llm' }),
      connection({ id: 'nv', origin_preset: 'nvidia' }),
      connection({ id: 'other', origin_preset: 'other' }),
      connection({ id: 'nokey', has_credential: false }),
      connection({ id: 'none', decision_model: null }),
      connection({ id: 'blank', decision_model: '  ' }),
    ]);
    expect(options).toEqual([]);
  });
});

describe('the step\'s choice in its config', () => {
  it('reads and writes the model; the default drops both fields', () => {
    expect(decideModelChoice(config)).toEqual({ model: null, connectionId: null });
    const own = withDecisionModel(config, { model: 'clef-flash:9b', connectionId: null });
    expect(own.model).toBe('clef-flash:9b');
    expect('connection_id' in own).toBe(false);
    expect(decideModelChoice(own)).toEqual({ model: 'clef-flash:9b', connectionId: null });
    const viaConnection = withDecisionModel(config, { model: 'm', connectionId: 'conn-1' });
    expect(viaConnection.connection_id).toBe('conn-1');
    const back = withDecisionModel(viaConnection, { model: null, connectionId: null });
    expect('model' in back || 'connection_id' in back).toBe(false);
  });
});

describe('saveDecisionModel — immediate save, optimistic, rollback', () => {
  const workflow = {
    id: 'wf', name: 'wf',
    steps: [
      { name: 'agent', step_type: { type: 'Agent' } },
      { name: 'triage', step_type: { type: 'Decide' }, decide: config },
    ],
  } as unknown as Workflow;
  const choice = { model: 'clef-flash:9b', connectionId: null };

  it('shows the choice first, then the saved workflow', async () => {
    const shown: Workflow[] = [];
    const saved = { ...workflow, name: 'saved' } as Workflow;
    const save = vi.fn().mockResolvedValue(saved);
    const result = await saveDecisionModel(workflow, 1, choice, { show: w => shown.push(w), save });
    expect(shown[0].steps[1].decide?.model).toBe('clef-flash:9b');
    expect(save).toHaveBeenCalledWith(shown[0].steps);
    expect(shown[1]).toBe(saved);
    expect(result).toBe(saved);
  });

  it('puts the saved workflow back when the save fails', async () => {
    const shown: Workflow[] = [];
    const save = vi.fn().mockRejectedValue(new Error('boom'));
    await expect(saveDecisionModel(workflow, 1, choice, { show: w => shown.push(w), save })).rejects.toThrow('boom');
    expect(shown[0].steps[1].decide?.model).toBe('clef-flash:9b');
    expect(shown[shown.length - 1]).toBe(workflow);
  });

  it('saves nothing for the same choice or a step that is not a Decide step', async () => {
    const save = vi.fn();
    const io = { show: vi.fn(), save };
    expect(await saveDecisionModel(workflow, 1, { model: null, connectionId: null }, io)).toBeNull();
    expect(await saveDecisionModel(workflow, 0, choice, io)).toBeNull();
    expect(save).not.toHaveBeenCalled();
    expect(io.show).not.toHaveBeenCalled();
  });
});
