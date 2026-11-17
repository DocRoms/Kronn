// KT-1706 — every card of Settings > Agents carries ONE « Models and fallbacks »
// block under its header, one row per mode: « Mode · [model ▾] → fallback ». No
// separate tier block repeats the models, and a change is saved at once, left as
// it was when the backend refuses.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup, within, act } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { AgentDetection, AgentType } from '../../../types/generated';

const mocks = vi.hoisted(() => ({
  getModelTiers: vi.fn(),
  setModelTiers: vi.fn(),
  getAgentFallback: vi.fn(),
  setAgentFallback: vi.fn(),
  setConnectionFallback: vi.fn(),
  catalogList: vi.fn(),
  connections: vi.fn(),
  comfyStatus: vi.fn(),
  ollamaHealth: vi.fn(),
  ollamaModels: vi.fn(),
}));

vi.mock('../../../lib/api', () => buildApiMock({
  modelCatalogApi: { list: mocks.catalogList as never },
  config: {
    getModelTiers: mocks.getModelTiers as never,
    setModelTiers: mocks.setModelTiers as never,
    getAgentFallback: mocks.getAgentFallback as never,
    setAgentFallback: mocks.setAgentFallback as never,
    setConnectionFallback: mocks.setConnectionFallback as never,
  },
  externalApi: { list: mocks.connections as never },
  comfyui: { status: mocks.comfyStatus as never },
  ollama: { health: mocks.ollamaHealth as never, models: mocks.ollamaModels as never },
}));

import { AgentsSection } from '../AgentsSection';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

const agent = (agent_type: AgentType, name: string): AgentDetection => ({
  agent_type, name, installed: true, enabled: true, path: null, version: null, latest_version: null,
  origin: 'host', install_command: null, host_managed: false, host_label: null, runtime_available: false,
  rtk_available: false, rtk_hook_configured: false,
});

const catalogEntry = (target: string, type: AgentType, model: string, tier: string | null, capabilities = ['chat']) => ({
  id: `${target}:${model}`, runtime_target_id: target, agent_type: type, model_id: model, display_name: model,
  provenance: 'live', availability: 'available', capabilities, reasoning_modes: [], manual_origin: false,
  tier_assignment: tier, first_seen_at: '2026-09-09T00:00:00Z', last_checked_at: '2026-09-09T00:00:00Z',
  created_at: '2026-09-09T00:00:00Z', updated_at: '2026-09-09T00:00:00Z',
});

const blank = { economy: null, default: null, reasoning: null };
const connection = (over: Record<string, unknown>) => ({
  id: 'conn', display_name: 'Conn', mention_alias: 'conn', endpoint: 'https://x.example', credential_slug: 'slug',
  origin_preset: 'open_router', economy_model: 'eco-m', default_model: 'std-m', reasoning_model: 'adv-m',
  created_at: '2026-08-28T00:00:00Z', updated_at: '2026-08-28T00:00:00Z', has_credential: true, ...over,
});

const comfyStatus = {
  state: 'online', endpoint: 'http://127.0.0.1:8188', version: '0.3', video_blocker: null, install_command: null,
  download_url: 'https://www.comfy.org/download', suggestions: [], install_dir: '/x', configured_endpoint: null,
  configured_install_dir: null, image_settings: {}, video_settings: {}, family_defaults: [], samplers: [], schedulers: [],
  image_model: 'sdxl.safetensors', video_model: 'wan.safetensors',
  models: [
    { folder: 'checkpoints', file: 'sdxl.safetensors', modality: 'image', family: 'sdxl', chosen: false },
    { folder: 'diffusion_models', file: 'wan.safetensors', modality: 'video', family: 'wan21_t2v', chosen: false },
  ],
};

beforeEach(() => {
  mocks.getModelTiers.mockReset().mockResolvedValue({
    claude_code: { economy: 'haiku', default: null, reasoning: 'opus' },
    codex: { ...blank }, open_code: { ...blank }, gemini_cli: { ...blank }, kiro: { ...blank }, vibe: { ...blank },
    copilot_cli: { ...blank }, ollama: { ...blank }, lite_llm: { ...blank }, nvidia: { ...blank },
  });
  mocks.setModelTiers.mockReset().mockResolvedValue(undefined);
  mocks.getAgentFallback.mockReset().mockResolvedValue({ chains: [], cooldowns: [], media: [] });
  mocks.setAgentFallback.mockReset().mockResolvedValue({ chains: [], cooldowns: [], media: [] });
  mocks.setConnectionFallback.mockReset().mockResolvedValue({ chains: [], cooldowns: [], media: [] });
  mocks.connections.mockReset().mockResolvedValue([]);
  mocks.comfyStatus.mockReset().mockResolvedValue({ ...comfyStatus, state: 'not_found', models: [], image_model: null, video_model: null });
  mocks.ollamaHealth.mockReset().mockResolvedValue({
    status: 'online', version: '0.9', endpoint: 'http://localhost:11434', models_count: 1, hint: null, mlx_capable: false,
  });
  mocks.ollamaModels.mockReset().mockResolvedValue({ models: [{ name: 'qwen3:8b', size: '5 GB' }] });
  mocks.catalogList.mockReset().mockResolvedValue({ targets: [
    {
      runtime_target_id: 'agent:claude-code', agent_type: 'ClaudeCode', live_refresh_ok: true, stale: false,
      models: [catalogEntry('agent:claude-code', 'ClaudeCode', 'haiku', 'economy'),
        catalogEntry('agent:claude-code', 'ClaudeCode', 'sonnet', 'default'),
        catalogEntry('agent:claude-code', 'ClaudeCode', 'opus', 'reasoning')],
    },
    {
      runtime_target_id: 'agent:ollama', agent_type: 'Ollama', live_refresh_ok: true, stale: false,
      models: [catalogEntry('agent:ollama', 'Ollama', 'qwen3:8b', 'default'),
        catalogEntry('agent:ollama', 'Ollama', 'gemma3:4b', 'economy'),
        catalogEntry('agent:ollama', 'Ollama', 'router-tiny', null, ['decision'])],
    },
  ] });
});

afterEach(cleanup);

const renderSection = (agents: AgentDetection[]) => {
  const toast = vi.fn();
  render(
    <AgentsSection
      agents={agents}
      agentAccess={null}
      configLanguage="fr"
      refetchAgents={vi.fn()}
      refetchAgentAccess={vi.fn()}
      toast={toast}
      t={t}
    />,
  );
  return { toast };
};

const picker = (agentType: string, tier: string) =>
  document.querySelector<HTMLInputElement>(`[data-model-tier-agent="${agentType}"][data-model-tier="${tier}"]`);

describe('KT-1706 — the models and fallbacks block of each card', () => {
  it('CLI card: one block, one row per mode, the model picker in its row and the resolved model shown', async () => {
    renderSection([agent('ClaudeCode', 'Claude Code'), agent('Codex', 'Codex')]);
    const block = await screen.findByTestId('agent-models-ClaudeCode');
    expect(within(block).getAllByText('agents.modelsAndFallbacks')).toHaveLength(1);
    for (const mode of ['eco', 'standard', 'advanced']) {
      expect(within(block).getByTestId(`agent-fallback-ClaudeCode-${mode}`)).toBeInTheDocument();
    }
    // The standard tier is unset: the row names the model it resolves to, not « default model ».
    await waitFor(() => expect(picker('ClaudeCode', 'default')).toHaveAttribute('placeholder', 'agents.modelAuto:sonnet'));
    expect(picker('ClaudeCode', 'economy')).toHaveValue('haiku');
    // No other tier block on the card.
    expect(screen.queryByTestId('agent-tier-preview-ClaudeCode')).not.toBeInTheDocument();
    expect(document.querySelectorAll('[data-model-tier-agent="ClaudeCode"][data-model-tier="economy"]')).toHaveLength(1);
  });

  it('CLI card: changing a mode’s model saves at once; a refusal leaves the row as it was', async () => {
    renderSection([agent('ClaudeCode', 'Claude Code')]);
    await waitFor(() => expect(picker('ClaudeCode', 'economy')).toHaveValue('haiku'));
    mocks.setModelTiers.mockRejectedValueOnce(new Error('refused'));
    fireEvent.focus(picker('ClaudeCode', 'economy')!);
    fireEvent.click(await screen.findByRole('option', { name: 'sonnet' }));
    await waitFor(() => expect(mocks.setModelTiers).toHaveBeenCalledTimes(1));
    // Rolled back: the confirmed value is still shown.
    await waitFor(() => expect(picker('ClaudeCode', 'economy')).not.toBeDisabled());
    expect(picker('ClaudeCode', 'economy')).toHaveValue('haiku');
  });

  it('Ollama card: the block sits under the header with the four modes, the resolved model in each row, and no tier block below', async () => {
    renderSection([agent('Ollama', 'Ollama'), agent('Codex', 'Codex')]);
    const block = await screen.findByTestId('agent-fallback-Ollama');
    for (const mode of ['eco', 'standard', 'advanced', 'decision']) {
      expect(within(block).getByTestId(`agent-fallback-Ollama-${mode}`)).toBeInTheDocument();
    }
    await waitFor(() => expect(picker('Ollama', 'default')).toHaveAttribute('placeholder', 'agents.modelAuto:qwen3:8b'));
    expect(picker('Ollama', 'economy')).toHaveAttribute('placeholder', 'agents.modelAuto:gemma3:4b');
    // « Model per tier » is gone, the pickers live in their rows.
    expect(screen.queryByText('ollama.tierPickerTitle')).not.toBeInTheDocument();
    expect(within(block).getByTestId('agent-fallback-Ollama-decision-model')).toContainElement(picker('Ollama', 'decision'));
    // The block comes before the card's secondary settings (the model downloads).
    const card = block.closest('.set-ollama-card')!;
    const download = card.querySelector('.set-ollama-download');
    expect(download).not.toBeNull();
    expect(block.compareDocumentPosition(download!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // A fallback is added from the row and saved at once.
    fireEvent.click(within(block).getByTestId('agent-fallback-Ollama-standard-add'));
    fireEvent.click(await screen.findByTestId('agent-fallback-Ollama-standard-menu-Codex-standard'));
    await waitFor(() => expect(mocks.setAgentFallback).toHaveBeenCalledWith({
      agent: 'Ollama', mode: 'standard', fallback: { agent: 'Codex', model: null },
    }));
  });

  it('ComfyUI card: image and video rows, the model of each slot in its row, and an image fallback to another source', async () => {
    mocks.comfyStatus.mockResolvedValue(comfyStatus);
    mocks.connections.mockResolvedValue([connection({
      id: 'or', display_name: 'OpenRouter', image_model: 'flux-pro', video_model: 'seedance',
    })]);
    renderSection([agent('ClaudeCode', 'Claude Code')]);
    const block = await screen.findByTestId('agent-fallback-comfyui');
    expect(within(block).getAllByText('agents.modelsAndFallbacks')).toHaveLength(1);
    const image = within(block).getByTestId('connection-fallback-local:comfyui-image');
    expect(within(image).getByTestId('comfyui-slot-image')).toHaveValue('sdxl.safetensors');
    expect(within(block).getByTestId('connection-fallback-local:comfyui-video')).toBeInTheDocument();
    // The slots are in the block only: no second grid.
    expect(screen.getAllByTestId('comfyui-slot-image')).toHaveLength(1);
    // The menu offers image models of other providers only.
    await waitFor(() => expect(mocks.connections).toHaveBeenCalled());
    fireEvent.click(await within(image).findByTestId('connection-fallback-local:comfyui-image-add'));
    const menu = await screen.findByTestId('connection-fallback-local:comfyui-image-menu');
    expect(within(menu).getAllByRole('group').map(group => group.getAttribute('aria-label'))).toEqual(['OpenRouter']);
    expect(within(menu).queryByText('seedance')).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId('connection-fallback-local:comfyui-image-menu-or-image-flux-pro'));
    await waitFor(() => expect(mocks.setConnectionFallback).toHaveBeenCalledWith({
      owner: 'local:comfyui', mode: 'image', fallback: { connection: 'or', model: 'flux-pro' },
    }));
  });

  it('HTTP card (OpenRouter): one row per mode served, each with its model picker and its own fallback', async () => {
    mocks.connections.mockResolvedValue([connection({
      id: 'or', display_name: 'OpenRouter', image_model: 'flux-pro', video_model: null, decision_model: null,
    })]);
    renderSection([agent('ClaudeCode', 'Claude Code')]);
    const block = await screen.findByTestId('connection-models-or');
    expect(within(block).getAllByText('agents.modelsAndFallbacks')).toHaveLength(1);
    expect(within(block).getByTestId('connection-model-or-eco')).toHaveValue('eco-m');
    expect(within(block).getByTestId('connection-model-or-standard')).toHaveValue('std-m');
    expect(within(block).getByTestId('connection-model-or-advanced')).toHaveValue('adv-m');
    expect(within(block).getByTestId('connection-model-or-image')).toHaveValue('flux-pro');
    // Only the modes the connection serves: an image model, no video, no decision.
    for (const mode of ['eco', 'standard', 'advanced', 'image']) {
      expect(within(block).getByTestId(`connection-fallback-or-${mode}`)).toBeInTheDocument();
    }
    expect(within(block).queryByTestId('connection-fallback-or-video')).not.toBeInTheDocument();
    expect(within(block).queryByTestId('connection-fallback-or-decision')).not.toBeInTheDocument();
    // The old tier list is gone.
    expect(screen.queryByRole('group', { name: 'disc.modelTier' })).not.toBeInTheDocument();
  });

  it('HTTP card (LiteLLM): a text mode falls back on another connection, and is saved at once', async () => {
    mocks.connections.mockResolvedValue([
      connection({ id: 'external-api-litellm', display_name: 'LiteLLM', origin_preset: 'lite_llm', image_model: 'gen-img' }),
      connection({ id: 'or', display_name: 'OpenRouter', economy_model: 'or-eco', default_model: 'or-std', reasoning_model: 'or-adv' }),
    ]);
    renderSection([agent('ClaudeCode', 'Claude Code')]);
    const block = await screen.findByTestId('connection-models-external-api-litellm');
    fireEvent.click(within(block).getByTestId('connection-fallback-external-api-litellm-standard-add'));
    fireEvent.click(await screen.findByTestId('connection-fallback-external-api-litellm-standard-menu-or-standard-or-std'));
    await waitFor(() => expect(mocks.setConnectionFallback).toHaveBeenCalledWith({
      owner: 'external-api-litellm', mode: 'standard', fallback: { connection: 'or', model: 'or-std' },
    }));
    expect(within(block).getByTestId('connection-fallback-external-api-litellm-image')).toBeInTheDocument();
  });

  it('keeps a refused media fallback off the row', async () => {
    mocks.comfyStatus.mockResolvedValue(comfyStatus);
    mocks.connections.mockResolvedValue([connection({ id: 'or', display_name: 'OpenRouter', image_model: 'flux-pro' })]);
    mocks.setConnectionFallback.mockRejectedValue(new Error('This fallback would loop: a → b → a.'));
    renderSection([agent('ClaudeCode', 'Claude Code')]);
    await act(async () => { await Promise.resolve(); });
    const image = await screen.findByTestId('connection-fallback-local:comfyui-image');
    await waitFor(() => expect(mocks.connections).toHaveBeenCalled());
    fireEvent.click(within(image).getByTestId('connection-fallback-local:comfyui-image-add'));
    fireEvent.click(await screen.findByTestId('connection-fallback-local:comfyui-image-menu-or-image-flux-pro'));
    expect(await within(image).findByTestId('connection-fallback-local:comfyui-image-refused')).toHaveTextContent('fallback.refused');
    expect(within(image).queryByTestId('connection-fallback-local:comfyui-image-link-0')).not.toBeInTheDocument();
  });
});

// KT-1807 — one mode, one place: its icon, its model and (Claude Code, Codex) its
// reasoning effort sit in its row, with no Edit fold to open for the effort.
describe('KT-1807 — each mode row carries its icon and its reasoning effort', () => {
  const MODES = [['eco', 'economy', '⚡'], ['standard', 'default', '🎯'], ['advanced', 'reasoning', '🧠']] as const;
  const effortOf = (agentType: string, tier: string) =>
    document.querySelector<HTMLSelectElement>(`[data-model-tier-effort-agent="${agentType}"][data-model-tier-effort="${tier}"]`);
  const withModes = (modes: Record<string, string[]>) => {
    const entry = (model: string, tier: string) => ({
      ...catalogEntry('agent:claude-code', 'ClaudeCode', model, tier), reasoning_modes: modes[model] ?? [],
    });
    mocks.catalogList.mockResolvedValue({ targets: [{
      runtime_target_id: 'agent:claude-code', agent_type: 'ClaudeCode', live_refresh_ok: true, stale: false,
      models: [entry('haiku', 'economy'), entry('sonnet', 'default'), entry('opus', 'reasoning')],
    }] });
  };
  const iconOf = (row: HTMLElement) => row.querySelector('.fbr-mode')?.firstElementChild ?? null;

  it('Claude Code: each mode row shows its icon, then its model with its effort beside it, without opening the card', async () => {
    withModes({ haiku: ['low', 'high'], sonnet: ['medium'], opus: ['high', 'max'] });
    renderSection([agent('ClaudeCode', 'Claude Code')]);
    const block = await screen.findByTestId('agent-models-ClaudeCode');
    await waitFor(() => expect(effortOf('ClaudeCode', 'reasoning')).not.toBeNull());
    expect(screen.getByTestId('agent-configure-ClaudeCode')).toHaveAttribute('aria-expanded', 'false');
    const offered = { economy: ['low', 'high'], default: ['medium'], reasoning: ['high', 'max'] };
    for (const [mode, tier, icon] of MODES) {
      const row = within(block).getByTestId(`agent-fallback-ClaudeCode-${mode}`);
      expect(iconOf(row)).toHaveAttribute('aria-hidden', 'true');
      expect(iconOf(row)?.textContent).toBe(icon);
      expect(row.querySelector('.fbr-mode')?.textContent).toBe(`${icon}fallback.mode.${mode}`);
      // The effort shares the model's line, right after the picker, above the fallbacks.
      const model = within(row).getByTestId(`agent-fallback-ClaudeCode-${mode}-model`);
      const effort = effortOf('ClaudeCode', tier)!;
      const line = row.querySelector('.fbr-track')!.firstElementChild;
      expect(model).toContainElement(picker('ClaudeCode', tier));
      expect(model).not.toContainElement(effort);
      expect(line).toContainElement(model);
      expect(line).toContainElement(effort);
      expect(model.compareDocumentPosition(effort) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
      expect(effort).toHaveAccessibleName(`config.reasoningEffort ${tier}`);
      await waitFor(() => expect(effort).toBeEnabled());
      expect(Array.from(effort.options).map(option => option.value)).toEqual(['', ...offered[tier]]);
    }
    // The models link stays on the block; the old effort panel of the fold is gone, even opened.
    expect(within(block).getByRole('link', { name: /config\.viewModels/ }))
      .toHaveAttribute('href', 'https://docs.anthropic.com/en/docs/about-claude/models');
    fireEvent.click(screen.getByTestId('agent-configure-ClaudeCode'));
    expect(screen.queryByTestId('agent-tier-efforts-ClaudeCode')).not.toBeInTheDocument();
    expect(document.querySelectorAll('[data-model-tier-effort-agent="ClaudeCode"]')).toHaveLength(3);
  });

  it('Claude Code: changing a mode’s effort in its row saves it as the effort of that tier', async () => {
    withModes({ haiku: ['low'], sonnet: ['medium', 'high'], opus: ['max'] });
    renderSection([agent('ClaudeCode', 'Claude Code')]);
    await waitFor(() => expect(effortOf('ClaudeCode', 'default')).not.toBeNull());
    const effort = effortOf('ClaudeCode', 'default')!;
    await waitFor(() => expect(effort).toBeEnabled());
    fireEvent.change(effort, { target: { value: 'high' } });
    await waitFor(() => expect(mocks.setModelTiers).toHaveBeenCalledTimes(1));
    expect(mocks.setModelTiers.mock.calls[0][0].claude_code).toEqual({
      economy: 'haiku', default: null, reasoning: 'opus', default_effort: 'high',
    });
  });

  it('Claude Code: a saved effort the model no longer supports stays visible, disabled, and can be cleared', async () => {
    mocks.getModelTiers.mockResolvedValue({
      claude_code: { economy: 'haiku', default: null, reasoning: 'opus', economy_effort: 'xhigh' },
      codex: { ...blank }, open_code: { ...blank }, gemini_cli: { ...blank }, kiro: { ...blank }, vibe: { ...blank },
      copilot_cli: { ...blank }, ollama: { ...blank }, lite_llm: { ...blank }, nvidia: { ...blank },
    });
    withModes({ haiku: ['low', 'high'] });
    renderSection([agent('ClaudeCode', 'Claude Code')]);
    await waitFor(() => expect(effortOf('ClaudeCode', 'economy')).toHaveValue('xhigh'));
    const effort = effortOf('ClaudeCode', 'economy')!;
    await waitFor(() => expect(effort).toBeEnabled());
    const stale = within(effort).getByRole('option', { name: /xhigh.*config\.reasoningEffortUnavailable/ });
    expect(stale).toBeDisabled();
    fireEvent.change(effort, { target: { value: '' } });
    await waitFor(() => expect(mocks.setModelTiers).toHaveBeenCalledTimes(1));
    expect(mocks.setModelTiers.mock.calls[0][0].claude_code.economy_effort).toBeNull();
  });

  it('Codex: its mode rows carry the effort too', async () => {
    mocks.catalogList.mockResolvedValue({ targets: [{
      runtime_target_id: 'agent:codex', agent_type: 'Codex', live_refresh_ok: true, stale: false,
      models: [{ ...catalogEntry('agent:codex', 'Codex', 'gpt-mini', 'economy'), reasoning_modes: ['low'] },
        { ...catalogEntry('agent:codex', 'Codex', 'gpt-std', 'default'), reasoning_modes: ['medium'] },
        { ...catalogEntry('agent:codex', 'Codex', 'gpt-deep', 'reasoning'), reasoning_modes: ['high'] }],
    }] });
    renderSection([agent('Codex', 'Codex')]);
    const block = await screen.findByTestId('agent-models-Codex');
    for (const [mode, tier] of MODES) {
      await waitFor(() => expect(effortOf('Codex', tier)).not.toBeNull());
      const row = within(block).getByTestId(`agent-fallback-Codex-${mode}`);
      expect(row.querySelector('.fbr-track')!.firstElementChild).toContainElement(effortOf('Codex', tier));
    }
  });

  it.each(['GeminiCli', 'Ollama'] as const)('%s: icons on its text modes, and no effort select', async agentType => {
    renderSection([agent(agentType, agentType)]);
    const block = await screen.findByTestId(`agent-fallback-${agentType}`);
    await waitFor(() => expect(picker(agentType, 'economy')).not.toBeNull());
    for (const [mode, , icon] of MODES) {
      const row = within(block).getByTestId(`agent-fallback-${agentType}-${mode}`);
      expect(iconOf(row)).toHaveAttribute('aria-hidden', 'true');
      expect(iconOf(row)?.textContent).toBe(icon);
      expect(row.querySelector('select')).toBeNull();
    }
    expect(document.querySelector('[data-model-tier-effort-agent]')).toBeNull();
    if (agentType === 'Ollama') {
      // A mode with no icon of its own keeps its plain label.
      const decision = within(block).getByTestId('agent-fallback-Ollama-decision');
      expect(decision.querySelector('.fbr-mode')?.textContent).toBe('fallback.mode.decision');
    } else {
      fireEvent.click(screen.getByTestId(`agent-configure-${agentType}`));
      expect(screen.queryByText('config.reasoningEffortUnsupported')).not.toBeInTheDocument();
    }
  });
});
