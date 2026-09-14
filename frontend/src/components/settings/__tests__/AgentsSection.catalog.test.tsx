import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../../test/apiMock';
import type { AgentDetection, AgentType, CatalogModelEntry, ModelCatalogSnapshot, ModelCatalogView, ModelTiersConfig } from '../../../types/generated';

const { list, getTiers, setTiers, refresh } = vi.hoisted(() => ({
  list: vi.fn(), getTiers: vi.fn(), setTiers: vi.fn(), refresh: vi.fn(),
}));
vi.mock('../../../lib/api', () => buildApiMock({
  config: { getModelTiers: getTiers as never, setModelTiers: setTiers as never },
  modelCatalogApi: { list: list as never, refresh: refresh as never },
}));
import { AgentsSection } from '../AgentsSection';

const t = (key: string, ...args: (string | number)[]) => args.length ? `${key}:${args.join(',')}` : key;
const checkedAt = '2026-09-09T00:00:00Z';

function model(modelId: string, overrides: Partial<CatalogModelEntry> = {}): CatalogModelEntry {
  return {
    id: `agent:claude-code:${modelId}`, runtime_target_id: 'agent:claude-code', agent_type: 'ClaudeCode',
    model_id: modelId, display_name: modelId, provenance: 'live', availability: 'available',
    capabilities: ['chat'], reasoning_modes: ['high'], manual_origin: false,
    first_seen_at: checkedAt, last_seen_at: checkedAt, last_checked_at: checkedAt,
    created_at: checkedAt, updated_at: checkedAt, ...overrides,
  };
}

function target(models: CatalogModelEntry[], overrides: Partial<ModelCatalogView> = {}): ModelCatalogView {
  return { runtime_target_id: 'agent:claude-code', agent_type: 'ClaudeCode', models,
    live_refresh_ok: true, stale: false, ...overrides };
}

function tiers(): ModelTiersConfig {
  const empty = () => ({ economy: null, default: null, reasoning: null });
  return {
    claude_code: { economy: 'sonnet', default: null, reasoning: null }, codex: empty(),
    open_code: { economy: 'vendor/fast', default: 'vendor/standard', reasoning: 'vendor/deep' },
    gemini_cli: empty(), kiro: empty(), vibe: empty(), copilot_cli: empty(),
    ollama: empty(), lite_llm: empty(), nvidia: empty(),
  };
}

function agent(agentType: AgentType): AgentDetection {
  return { agent_type: agentType, name: agentType, installed: true, enabled: true,
    path: null, version: null, latest_version: null, origin: 'host', install_command: '',
    host_managed: false, host_label: null, runtime_available: true, rtk_available: false,
    rtk_hook_configured: false };
}

function show(agentType: AgentType = 'ClaudeCode') {
  const toast = vi.fn();
  render(<AgentsSection agents={[agent(agentType)]} agentAccess={null} configLanguage="en"
    refetchAgents={vi.fn()} refetchAgentAccess={vi.fn()} toast={toast} t={t} />);
  fireEvent.click(screen.getByTestId(`agent-configure-${agentType}`));
  return toast;
}

async function picker(agentType = 'ClaudeCode', tier = 'economy') {
  let input: HTMLInputElement | null = null;
  await waitFor(() => {
    input = document.querySelector<HTMLInputElement>(`[data-model-tier-agent="${agentType}"][data-model-tier="${tier}"]`);
    expect(input).not.toBeNull();
  });
  return input!;
}

beforeEach(() => {
  vi.clearAllMocks();
  getTiers.mockReset().mockResolvedValue(tiers());
  setTiers.mockReset().mockResolvedValue(undefined);
  list.mockReset().mockResolvedValue({ targets: [target([model('haiku'), model('sonnet')])] } satisfies ModelCatalogSnapshot);
  refresh.mockReset().mockResolvedValue(undefined);
});
afterEach(cleanup);

describe('AgentsSection — runtime catalogue and tier preservation (KT-531)', () => {
  it('offers new catalogue models, never models from another runtime of the same family', async () => {
    list.mockResolvedValue({ targets: [
      target([model('new-model', { display_alias: 'Nouveau modèle' })]),
      target([model('http-only', { runtime_target_id: 'http:other' })], { runtime_target_id: 'http:other' }),
    ] });
    show();
    fireEvent.focus(await picker());
    const option = await screen.findByRole('option', { name: 'Nouveau modèle' });
    expect(option).toHaveTextContent('modelCatalog.provenance.live');
    expect(option).toHaveTextContent(checkedAt);
    expect(option).toHaveTextContent('high');
    expect(screen.queryByRole('option', { name: 'http-only' })).not.toBeInTheDocument();
    expect(screen.queryByRole('option', { name: 'haiku' })).not.toBeInTheDocument();
  });

  it('exposes all OpenCode tiers without erasing configured models missing from the catalogue', async () => {
    show('OpenCode');
    expect((await picker('OpenCode')).value).toContain('vendor/fast');
    expect((await picker('OpenCode', 'default')).value).toContain('vendor/standard');
    expect((await picker('OpenCode', 'reasoning')).value).toContain('vendor/deep');
    fireEvent.focus(await picker('OpenCode'));
    expect(await screen.findByRole('option', { name: /vendor\/fast/ })).toHaveAttribute('aria-disabled', 'true');
    expect(setTiers).not.toHaveBeenCalled();
  });

  it('saves only the selected field over fresh settings, preserving OpenCode and independently edited Ollama', async () => {
    show();
    const input = await picker();
    const fresh = tiers();
    fresh.ollama.default = 'local/operator-updated';
    fresh.claude_code.reasoning = 'operator-new-reasoning';
    getTiers.mockResolvedValue(fresh);
    fireEvent.focus(input);
    fireEvent.click(await screen.findByRole('option', { name: 'haiku' }));
    await waitFor(() => expect(setTiers).toHaveBeenCalledWith({
      ...fresh, claude_code: { ...fresh.claude_code, economy: 'haiku' },
    }));
    expect(input).toHaveValue('haiku');
  });

  it('keeps the saved selection visible when saving fails', async () => {
    setTiers.mockRejectedValue(new Error('save refused'));
    const toast = show();
    const input = await picker();
    fireEvent.focus(input);
    fireEvent.click(await screen.findByRole('option', { name: 'haiku' }));
    await waitFor(() => expect(toast).toHaveBeenCalledWith('config.saveError', 'error'));
    expect(input).toHaveValue('sonnet');
  });

  it('keeps an unavailable configured model disabled, showing stale provenance and its reason', async () => {
    list.mockResolvedValue({ targets: [target([
      model('sonnet', { availability: 'unavailable', unavailable_reason: 'disappeared', unavailable_detail: 'Retired by runtime' }),
    ], { stale: true, live_refresh_ok: false })] });
    show();
    fireEvent.focus(await picker());
    const option = await screen.findByRole('option', { name: /sonnet.*modelCatalog.unavailable/ });
    expect(option).toHaveAttribute('aria-disabled', 'true');
    expect(option).toHaveTextContent('modelCatalog.provenance.cached');
    expect(option).toHaveTextContent('Retired by runtime');
    expect(option).not.toHaveTextContent('modelCatalog.provenance.live');
    fireEvent.click(option);
    expect(setTiers).not.toHaveBeenCalled();
  });

  it('does not invent options or replace the configured model on an empty catalogue', async () => {
    list.mockResolvedValue({ targets: [] });
    show();
    const input = await picker();
    expect(input.value).toContain('sonnet');
    fireEvent.focus(input);
    const options = within(screen.getByRole('listbox')).getAllByRole('option');
    expect(options).toHaveLength(2); // Explicit clear + retained unavailable configured value.
    expect(setTiers).not.toHaveBeenCalled();
  });

  it('updates tier options after the operator explicitly rechecks the catalogue', async () => {
    show();
    await picker();
    const next = { targets: [target([model('new-after-refresh')])] };
    list.mockResolvedValue(next);
    await act(async () => fireEvent.click(await screen.findByRole('button', { name: 'modelCatalog.recheck' })));
    fireEvent.focus(await picker());
    expect(await screen.findByRole('option', { name: 'new-after-refresh' })).toBeInTheDocument();
    expect(refresh).toHaveBeenCalledWith({ runtime_target_id: 'agent:claude-code', agent_type: 'ClaudeCode', force: true });
    expect(setTiers).not.toHaveBeenCalled();
  });

  it('reports a catalogue error, preserves the configured id and retries only the saved snapshot', async () => {
    list.mockRejectedValue(new Error('offline'));
    show();
    const input = await picker();
    expect(await screen.findByRole('alert')).toHaveTextContent('modelCatalog.loadError');
    expect(input.value).toContain('sonnet');
    expect(setTiers).not.toHaveBeenCalled();
    list.mockResolvedValue({ targets: [target([model('recovered-model')])] });
    fireEvent.click(screen.getByRole('button', { name: 'modelCatalog.reload' }));
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
    await waitFor(() => expect(input).not.toBeDisabled());
    fireEvent.focus(input);
    expect(await screen.findByRole('option', { name: 'recovered-model' })).toBeInTheDocument();
    expect(refresh).not.toHaveBeenCalled();
    expect(setTiers).not.toHaveBeenCalled();
  });

  it('does not write from a stale snapshot if the fresh settings read fails', async () => {
    const toast = show();
    const input = await picker();
    getTiers.mockRejectedValue(new Error('settings unavailable'));
    fireEvent.focus(input);
    fireEvent.click(await screen.findByRole('option', { name: 'haiku' }));
    await waitFor(() => expect(toast).toHaveBeenCalledWith('config.saveError', 'error'));
    expect(setTiers).not.toHaveBeenCalled();
    expect(input).toHaveValue('sonnet');
  });

  it('guards a synchronous duplicate choice and retains the confirmed value until the write succeeds', async () => {
    let resolveSave!: () => void;
    setTiers.mockImplementation(() => new Promise<void>(resolve => { resolveSave = resolve; }));
    show();
    const input = await picker();
    fireEvent.focus(input);
    const option = await screen.findByRole('option', { name: 'haiku' });
    act(() => { option.click(); option.click(); });
    await waitFor(() => expect(setTiers).toHaveBeenCalledTimes(1));
    expect(input).toBeDisabled();
    expect(input).toHaveValue('sonnet');
    await act(async () => resolveSave());
    expect(input).toHaveValue('haiku');
    expect(input).not.toBeDisabled();
  });

  it('clears only an explicit override and displays the catalogue tier assignment', async () => {
    list.mockResolvedValue({ targets: [target([model('runtime-default', { tier_assignment: 'economy' }), model('sonnet')])] });
    show();
    const input = await picker();
    fireEvent.focus(input);
    fireEvent.click(await screen.findByRole('option', { name: 'config.defaultModel (runtime-default)' }));
    await waitFor(() => expect(setTiers).toHaveBeenCalledWith({
      ...tiers(), claude_code: { ...tiers().claude_code, economy: null },
    }));
    expect(screen.getByTestId('agent-tier-preview-ClaudeCode')).toHaveTextContent('runtime-default');
    expect(input).toHaveValue('');
    expect(input).toHaveAttribute('placeholder', 'config.defaultModel (runtime-default)');
  });

  it('saves a discovered effort and clears it when the selected model cannot accept it', async () => {
    const current = tiers();
    current.claude_code.economy_effort = 'high';
    getTiers.mockResolvedValue(current);
    list.mockResolvedValue({ targets: [target([
      model('sonnet', { reasoning_modes: ['high'] }),
      model('haiku', { reasoning_modes: ['low'] }),
    ])] });
    show();
    const effort = await screen.findByLabelText('config.reasoningEffort economy');
    expect(effort).toHaveValue('high');
    const input = await picker();
    fireEvent.focus(input);
    fireEvent.click(await screen.findByRole('option', { name: 'haiku' }));
    await waitFor(() => expect(setTiers).toHaveBeenCalledWith({
      ...current,
      claude_code: { ...current.claude_code, economy: 'haiku', economy_effort: null },
    }));
  });

  it('uses the catalogue that arrived after mount when clearing an incompatible effort', async () => {
    let resolveCatalog!: (snapshot: ModelCatalogSnapshot) => void;
    list.mockImplementationOnce(() => new Promise<ModelCatalogSnapshot>(resolve => { resolveCatalog = resolve; }));
    const current = tiers();
    current.claude_code.economy_effort = 'high';
    getTiers.mockResolvedValue(current);
    show();
    await act(async () => resolveCatalog({ targets: [target([
      model('sonnet', { reasoning_modes: ['high'] }),
      model('haiku', { reasoning_modes: ['low'] }),
    ])] }));
    const input = await picker();
    fireEvent.focus(input);
    fireEvent.click(await screen.findByRole('option', { name: 'haiku' }));
    await waitFor(() => expect(setTiers).toHaveBeenCalledWith({
      ...current,
      claude_code: { ...current.claude_code, economy: 'haiku', economy_effort: null },
    }));
  });

  it('rejects an effort selection when the current catalogue marks its model unavailable', async () => {
    const current = tiers();
    current.claude_code.economy_effort = 'high';
    getTiers.mockResolvedValue(current);
    list.mockResolvedValue({ targets: [target([
      model('sonnet', { availability: 'unavailable', reasoning_modes: ['high'] }),
    ])] });
    const toast = show();
    const effort = await screen.findByLabelText('config.reasoningEffort economy');
    expect(effort).toBeDisabled();
    expect(within(effort).getByRole('option', { name: /high.*reasoningEffortUnavailable/ })).toBeInTheDocument();
    expect(toast).not.toHaveBeenCalledWith('config.saved', 'success');
  });

  it('persists only a model-advertised effort preset', async () => {
    show();
    const effort = await screen.findByLabelText('config.reasoningEffort economy');
    expect(within(effort).getByRole('option', { name: 'high' })).toBeInTheDocument();
    fireEvent.change(effort, { target: { value: 'high' } });
    await waitFor(() => expect(setTiers).toHaveBeenCalledWith({
      ...tiers(), claude_code: { ...tiers().claude_code, economy_effort: 'high' },
    }));
  });

  it.each(['Kiro', 'Vibe'] as const)('offers configured manual models for %s instead of N/A', async agentType => {
    const runtime = `agent:${agentType.toLowerCase()}`;
    list.mockResolvedValue({ targets: [target([
      model('manual-model', { runtime_target_id: runtime, agent_type: agentType, provenance: 'manual' }),
    ], { runtime_target_id: runtime, agent_type: agentType, live_refresh_ok: false, stale: true })] });
    show(agentType);
    fireEvent.focus(await picker(agentType));
    const option = await screen.findByRole('option', { name: 'manual-model' });
    expect(option).toHaveTextContent('modelCatalog.provenance.manual');
    expect(option).not.toBeDisabled();
    expect(refresh).not.toHaveBeenCalled();
  });
});
