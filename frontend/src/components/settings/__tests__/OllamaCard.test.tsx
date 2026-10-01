/**
 * 0.8.7 — P1-7a of the QA roadmap.
 *
 * OllamaCard has 4 explicit states (not_installed / offline+unreachable /
 * online-zero-models / online+models) and an async default-model picker
 * with confirmed-write semantics. Pinned
 * here :
 *  - the 4 states render their respective wizard / picker UI
 *  - the canirun.ai hint always renders (regression for the 2026-05-11
 *    user report where it was hidden too low)
 *  - default-model picker retains its confirmed value on POST failure
 *  - refresh button re-fetches health + models
 *  - health fetch errors degrade to an "offline" rendering without crash
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, act, cleanup, waitFor, within } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import { SUGGESTED_MODELS, MLX_SUGGESTED_MODELS } from '../ollamaModels';
import type { CatalogModelEntry, ModelTiersConfig, OllamaModel } from '../../../types/generated';

const { ollama, config, catalogList } = vi.hoisted(() => ({
  ollama: { health: vi.fn(), models: vi.fn(), pull: vi.fn(), registry: vi.fn(), setContextOverride: vi.fn() },
  config: { getModelTiers: vi.fn(), setModelTiers: vi.fn() },
  catalogList: vi.fn(),
}));

vi.mock('../../../lib/api', () => buildApiMock({ ollama, config, modelCatalogApi: { list: catalogList } }));

import { OllamaCard } from '../OllamaCard';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}(${args.join('|')})` : key;

const baseTiers: ModelTiersConfig = {
  claude_code: { economy: null, reasoning: null, default: null },
  codex: { economy: null, reasoning: null, default: null },
  open_code: { economy: null, reasoning: null, default: null },
  gemini_cli: { economy: null, reasoning: null, default: null },
  kiro: { economy: null, reasoning: null, default: null },
  vibe: { economy: null, reasoning: null, default: null },
  copilot_cli: { economy: null, reasoning: null, default: null },
  ollama: { economy: null, reasoning: null, default: null },
  lite_llm: { economy: null, reasoning: null, default: null },
  nvidia: { economy: null, reasoning: null, default: null },
};

const installedModel = (name: string, overrides: Partial<OllamaModel> = {}): OllamaModel => ({
  name,
  size: '4.0 GB',
  modified: '2026-01-01',
  advertised_context: 131_072,
  context_ceiling: 65_536,
  context_override: null,
  context_origin: 'machine_ceiling',
  ...overrides,
});

beforeEach(() => {
  for (const mock of [...Object.values(ollama), ...Object.values(config), catalogList]) mock.mockReset();
  localStorage.clear();
  ollama.health.mockResolvedValue({
    status: 'not_installed', version: null, endpoint: '', models_count: 0, hint: null, mlx_capable: false,
  });
  ollama.models.mockResolvedValue({ models: [] });
  ollama.pull.mockResolvedValue(undefined);
  ollama.registry.mockResolvedValue({ models: [], suggestions: [] });
  ollama.setContextOverride.mockResolvedValue({ model: '', num_ctx: null, warnings: [] });
  config.getModelTiers.mockResolvedValue(baseTiers);
  config.setModelTiers.mockResolvedValue(undefined);
  // Installed inventory and saved catalogue are distinct API contracts. Keep
  // the pre-existing picker cases backed by an explicit catalogue fixture.
  catalogList.mockResolvedValue({ targets: [{
    runtime_target_id: 'agent:ollama', agent_type: 'Ollama', live_refresh_ok: true, stale: false,
    models: ['llama3.2', 'llama3.2:latest', 'qwen2.5-coder:14b', 'qwen3:8b'].map((id): CatalogModelEntry => ({
      id: `agent:ollama:${id}`, runtime_target_id: 'agent:ollama', agent_type: 'Ollama',
      model_id: id, display_name: id, provenance: 'live', availability: 'available',
      capabilities: ['chat'], reasoning_modes: [], manual_origin: false,
      first_seen_at: '2026-09-10T00:00:00Z', last_checked_at: '2026-09-10T00:00:00Z',
      created_at: '2026-09-10T00:00:00Z', updated_at: '2026-09-10T00:00:00Z',
    })),
  }] });
});

afterEach(() => { cleanup(); vi.clearAllMocks(); });

/** The tags of the download block's suggestion list, in display order. */
function suggestionNames(): string[] {
  const list = document.querySelector('.set-ollama-download .set-ollama-suggestions');
  return [...(list?.querySelectorAll('.set-ollama-cmd') ?? [])].map(node => node.textContent ?? '');
}

function downloadBlock(): HTMLDetailsElement {
  return screen.getByText('ollama.pullTitle').closest('details') as HTMLDetailsElement;
}

async function mountCard(modelCostSuffix?: (model: string) => string) {
  let result: ReturnType<typeof render>;
  await act(async () => { result = render(<OllamaCard t={t} modelCostSuffix={modelCostSuffix} />); });
  await waitFor(() => expect(screen.getByLabelText('ollama.refresh')).not.toBeDisabled());
  return result!;
}

describe('OllamaCard — 4-state rendering', () => {
  it('not_installed → install wizard with macOS + Linux/WSL commands', async () => {
    await mountCard();
    expect(screen.getByText('ollama.installTitle')).toBeTruthy();
    expect(screen.getByText('brew install ollama')).toBeTruthy();
    expect(screen.getByText('curl -fsSL https://ollama.com/install.sh | sh')).toBeTruthy();
  });

  it('offline → launch instructions + hint surface (if any)', async () => {
    ollama.health.mockResolvedValue({
      status: 'offline', version: null, endpoint: 'http://localhost:11434',
      models_count: 0, hint: 'Run `ollama serve` in another terminal',
    });
    await mountCard();
    expect(screen.getByText('ollama.launchTitle')).toBeTruthy();
    expect(screen.getByText('Run `ollama serve` in another terminal')).toBeTruthy();
  });

  it('unreachable → same launch path as offline', async () => {
    ollama.health.mockResolvedValue({
      status: 'unreachable', version: null, endpoint: 'http://localhost:11434',
      models_count: 0, hint: null,
    });
    await mountCard();
    expect(screen.getByText('ollama.launchTitle')).toBeTruthy();
  });

  it('online + 0 models → pull-suggestion list visible', async () => {
    ollama.health.mockResolvedValue({
      status: 'online', version: '0.3.12', endpoint: 'http://localhost:11434',
      models_count: 0, hint: null,
    });
    ollama.models.mockResolvedValue({ models: [] });
    await mountCard();
    // First use: the download block is open and lists the portable suggestions.
    expect(suggestionNames()).toEqual(['qwen3.5:4b', 'qwen3:8b', 'qwen3:30b-a3b']);
  });

  it('online + models → installed model name appears + status reflects count', async () => {
    ollama.health.mockResolvedValue({
      status: 'online', version: '0.3.12', endpoint: 'http://localhost:11434',
      models_count: 2, hint: null,
    });
    ollama.models.mockResolvedValue({
      models: [
        installedModel('llama3.2:latest'),
        installedModel('qwen2.5-coder:14b'),
      ],
    });
    await mountCard();
    // Model names appear as <option>s across the 3 tier selects → match-all.
    expect(screen.getAllByText(/llama3\.2:latest/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/qwen2\.5-coder:14b/).length).toBeGreaterThan(0);
    // Status line carries the count via the i18n template.
    expect(document.body.textContent).toMatch(/2 ollama\.models/);
  });
});

describe('OllamaCard — per-model context policy', () => {
  beforeEach(() => {
    ollama.health.mockResolvedValue({
      status: 'online', version: '0.12.0', endpoint: 'http://localhost:11434',
      models_count: 1, hint: null,
    });
  });

  it('keeps the context-window editor collapsed by default', async () => {
    ollama.models.mockResolvedValue({ models: [installedModel('qwen3:8b')] });
    await mountCard();

    const summary = screen.getByText('ollama.contextTitle');
    const details = summary.closest('details');
    expect(details).not.toBeNull();
    expect(details!.open).toBe(false);

    fireEvent.click(summary);
    expect(details!.open).toBe(true);
  });

  it('shows trained window, ceiling, origin and a loud portable fallback', async () => {
    ollama.models.mockResolvedValue({
      models: [installedModel('qwen3:8b', {
        advertised_context: null,
        context_ceiling: 8_192,
        context_origin: 'portable_fallback',
      })],
    });
    await mountCard();

    expect(screen.getByText('ollama.contextAdvertised')).toBeTruthy();
    expect(screen.getByText('ollama.contextCeiling')).toBeTruthy();
    expect(screen.getByText('ollama.contextOrigin.portable_fallback')).toBeTruthy();
    expect(screen.getByText('ollama.contextFallbackWarning')).toBeTruthy();
    // `toLocaleString()` deliberately follows the runtime locale: Quick Exec
    // may render 8 192 while an English workstation renders 8,192. Assert the
    // numeric value without coupling this regression test to one separator.
    const ceilingMetric = screen.getByText('ollama.contextCeiling').closest('span');
    expect(ceilingMetric?.textContent?.replace(/\D/g, '')).toBe('8192');
  });

  it('persists a bounded override and refreshes the effective projection', async () => {
    const initial = installedModel('qwen3:8b');
    const overridden = installedModel('qwen3:8b', {
      context_override: 98_304,
      context_ceiling: 98_304,
      context_origin: 'model_override',
    });
    ollama.models
      .mockResolvedValueOnce({ models: [initial] })
      .mockResolvedValueOnce({ models: [overridden] });
    ollama.setContextOverride.mockResolvedValue({
      model: 'qwen3:8b', num_ctx: 98_304, warnings: ['Above RAM heuristic'],
    });
    await mountCard();

    const input = screen.getByLabelText('ollama.contextOverrideFor(qwen3:8b)');
    fireEvent.change(input, { target: { value: '98304' } });
    fireEvent.click(screen.getByText('ollama.contextSave'));
    await waitFor(() => expect(ollama.setContextOverride).toHaveBeenCalledWith('qwen3:8b', 98_304));
    await waitFor(() => expect(screen.getByDisplayValue('98304')).toBeTruthy());
    expect(screen.getByText('Above RAM heuristic')).toBeTruthy();
  });

  it('resets the saved override to automatic sizing', async () => {
    ollama.models.mockResolvedValue({
      models: [installedModel('qwen3:8b', {
        context_override: 65_536,
        context_origin: 'model_override',
      })],
    });
    ollama.setContextOverride.mockResolvedValue({
      model: 'qwen3:8b', num_ctx: null, warnings: [],
    });
    await mountCard();

    fireEvent.click(screen.getByText('ollama.contextReset'));
    await waitFor(() => expect(ollama.setContextOverride).toHaveBeenCalledWith('qwen3:8b', null));
  });

  it('refuses an invalid value before calling the backend', async () => {
    ollama.models.mockResolvedValue({ models: [installedModel('qwen3:8b')] });
    await mountCard();
    fireEvent.change(
      screen.getByLabelText('ollama.contextOverrideFor(qwen3:8b)'),
      { target: { value: '512' } },
    );
    fireEvent.click(screen.getByText('ollama.contextSave'));
    expect(await screen.findByText('ollama.contextInvalid')).toBeTruthy();
    expect(ollama.setContextOverride).not.toHaveBeenCalled();
  });

  it('serializes synchronous context saves and resets until the confirmed inventory returns', async () => {
    ollama.models.mockResolvedValue({ models: [installedModel('qwen3:8b', { context_override: 8192 })] });
    let resolveSave!: (result: { model: string; num_ctx: number | null; warnings: string[] }) => void;
    ollama.setContextOverride.mockImplementation(() => new Promise(resolve => { resolveSave = resolve; }));
    await mountCard();
    fireEvent.click(screen.getByText('ollama.contextTitle'));
    const input = screen.getByLabelText('ollama.contextOverrideFor(qwen3:8b)');
    fireEvent.change(input, { target: { value: '16384' } });
    const save = screen.getByRole('button', { name: 'ollama.contextSave' });
    const reset = screen.getByRole('button', { name: 'ollama.contextReset' });
    act(() => { save.click(); save.click(); reset.click(); });
    const mutationCount = ollama.setContextOverride.mock.calls.length;
    const wasDisabled = (input as HTMLInputElement).disabled;
    await act(async () => resolveSave({ model: 'qwen3:8b', num_ctx: 16384, warnings: [] }));
    expect(mutationCount).toBe(1);
    expect(wasDisabled).toBe(true);
    expect(ollama.setContextOverride).toHaveBeenCalledWith('qwen3:8b', 16384);
    expect(save).not.toBeDisabled();
  });
});

describe('OllamaCard — canirun.ai hint always visible', () => {
  it('renders the canirun link even in not_installed state (2026-05-11 regression guard)', async () => {
    await mountCard();
    const link = document.querySelector('a.set-ollama-canirun') as HTMLAnchorElement | null;
    expect(link).not.toBeNull();
    expect(link!.href).toContain('canirun.ai');
  });

  it('renders the canirun link in online state too', async () => {
    ollama.health.mockResolvedValue({
      status: 'online', version: '0.3.12', endpoint: 'http://localhost:11434',
      models_count: 0, hint: null,
    });
    await mountCard();
    const link = document.querySelector('a.set-ollama-canirun') as HTMLAnchorElement | null;
    expect(link).not.toBeNull();
  });
});

describe('OllamaCard — per-tier model picker', () => {
  it('adds an observed cost suffix supplied by the usage report', async () => {
    ollama.health.mockResolvedValue({
      status: 'online', version: '0.3.12', endpoint: 'http://localhost:11434',
      models_count: 1, hint: null,
    });
    ollama.models.mockResolvedValue({
      models: [installedModel('llama3.2')],
    });

    await mountCard(model => model === 'llama3.2' ? ' · ≈ $0.00/M observed' : '');

    for (const tier of ['economy', 'default', 'reasoning']) {
      const input = screen.getByLabelText(`disc.tier.${tier}`);
      fireEvent.focus(input);
      expect(screen.getByRole('option', { name: 'llama3.2' }))
        .toHaveTextContent('≈ $0.00/M observed');
      fireEvent.keyDown(input, { key: 'Escape' });
    }
  });

  it('choosing a model in the default AND economy selects fires setModelTiers per tier', async () => {
    ollama.health.mockResolvedValue({
      status: 'online', version: '0.3.12', endpoint: 'http://localhost:11434',
      models_count: 1, hint: null,
    });
    ollama.models.mockResolvedValue({
      models: [installedModel('llama3.2')],
    });
    await mountCard();

    // Default tier → writes ollama.default (aria-label is the i18n key here).
    const defSelect = screen.getByLabelText('disc.tier.default') as HTMLInputElement;
    await act(async () => {
      fireEvent.focus(defSelect);
      fireEvent.change(defSelect, { target: { value: 'llama3.2' } });
      fireEvent.click(screen.getByRole('option', { name: 'llama3.2' }));
    });
    await waitFor(() => expect(config.setModelTiers).toHaveBeenCalled());
    expect(config.setModelTiers.mock.calls[0][0].ollama.default).toBe('llama3.2');

    // Economy tier → the NEW capability: writes ollama.economy independently.
    const ecoSelect = screen.getByLabelText('disc.tier.economy') as HTMLInputElement;
    await act(async () => {
      fireEvent.focus(ecoSelect);
      fireEvent.change(ecoSelect, { target: { value: 'llama3.2' } });
      fireEvent.click(screen.getByRole('option', { name: 'llama3.2' }));
    });
    await waitFor(() => expect(config.setModelTiers.mock.calls.length).toBeGreaterThan(1));
    const last = config.setModelTiers.mock.calls.at(-1)![0];
    expect(last.ollama.economy).toBe('llama3.2');
  });

  it('keeps the confirmed value when setModelTiers fails', async () => {
    ollama.health.mockResolvedValue({
      status: 'online', version: '0.3.12', endpoint: 'http://localhost:11434',
      models_count: 2, hint: null,
    });
    ollama.models.mockResolvedValue({
      models: [
        installedModel('llama3.2'),
        installedModel('qwen2.5-coder:14b'),
      ],
    });
    config.getModelTiers.mockResolvedValue({ ...baseTiers, ollama: { economy: null, reasoning: null, default: 'llama3.2' } });
    config.setModelTiers.mockRejectedValue(new Error('500'));
    await mountCard();

    const defSelect = screen.getByLabelText('disc.tier.default') as HTMLInputElement;
    expect(defSelect.value).toBe('llama3.2');
    await act(async () => {
      fireEvent.focus(defSelect);
      fireEvent.change(defSelect, { target: { value: 'qwen2.5-coder:14b' } });
      fireEvent.click(screen.getByRole('option', { name: /qwen2\.5-coder:14b/ }));
    });
    await waitFor(() => expect(config.setModelTiers).toHaveBeenCalled());
    // A failed write never replaces the confirmed model.
    await waitFor(() => expect(defSelect.value).toBe('llama3.2'));
    expect(document.querySelector('.set-ollama-card')).not.toBeNull();
  });
});

describe('OllamaCard — refresh button', () => {
  it('clicking the refresh icon re-fetches health and models', async () => {
    ollama.health.mockResolvedValue({
      status: 'online', version: '0.3.12', endpoint: 'http://localhost:11434',
      models_count: 0, hint: null,
    });
    await mountCard();
    const initialHealthCalls = ollama.health.mock.calls.length;
    fireEvent.click(screen.getByLabelText('ollama.refresh'));
    await waitFor(() => expect(ollama.health.mock.calls.length).toBeGreaterThan(initialHealthCalls));
  });
});

describe('OllamaCard — direct model downloads', () => {
  beforeEach(() => {
    ollama.health.mockResolvedValue({
      status: 'online', version: '0.12.0', endpoint: 'http://localhost:11434',
      models_count: 0, hint: null,
    });
  });

  it('starts one pull on synchronous double-clicks and renders byte progress', async () => {
    let resolvePull!: () => void;
    ollama.pull.mockImplementation((_model: string, handlers: { onProgress: (event: unknown) => void }) => {
      handlers.onProgress({ status: 'downloading', digest: 'sha256:abc', completed: 1_000_000, total: 4_000_000 });
      return new Promise<void>(resolve => { resolvePull = resolve; });
    });
    await mountCard();

    const button = screen.getAllByRole('button', { name: 'ollama.pullButton' })[0];
    fireEvent.click(button);
    fireEvent.click(button);
    await waitFor(() => expect(ollama.pull).toHaveBeenCalledTimes(1));
    expect(screen.getByText(/1 MB.*4 MB.*25%/)).toBeTruthy();
    await act(async () => resolvePull());
  });

  it('cancels an in-flight pull and leaves it relaunchable', async () => {
    let receivedSignal!: AbortSignal;
    ollama.pull.mockImplementation((_model: string, _handlers: unknown, signal: AbortSignal) => new Promise<void>(resolve => {
      receivedSignal = signal;
      signal.addEventListener('abort', () => resolve());
    }));
    await mountCard();

    fireEvent.click(screen.getAllByRole('button', { name: 'ollama.pullButton' })[0]);
    await waitFor(() => expect(screen.getByRole('button', { name: 'ollama.pullCancel' })).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: 'ollama.pullCancel' }));
    await waitFor(() => expect(receivedSignal.aborted).toBe(true));
    await waitFor(() => expect(screen.queryByRole('button', { name: 'ollama.pullCancel' })).toBeNull());
  });

  it('refreshes installed models only after a success event', async () => {
    ollama.pull.mockImplementation(async (_model: string, handlers: { onSuccess: (event: unknown) => void }) => {
      handlers.onSuccess({ status: 'success', digest: null, completed: null, total: null });
    });
    ollama.models
      .mockResolvedValueOnce({ models: [] })
      .mockResolvedValueOnce({ models: [installedModel('llama3.2:1b')] });
    await mountCard();
    fireEvent.click(screen.getAllByRole('button', { name: 'ollama.pullButton' })[0]);
    await waitFor(() => expect(ollama.models).toHaveBeenCalledTimes(2));
    expect(screen.getAllByText(/llama3\.2:1b/).length).toBeGreaterThan(0);
  });

  it('surfaces a pull error without reporting success', async () => {
    ollama.pull.mockImplementation(async (_model: string, handlers: { onError: (message: string) => void }) => {
      handlers.onError('Ollama could not find this model. Check its name and tag, then try again.');
    });
    await mountCard();
    fireEvent.click(screen.getAllByRole('button', { name: 'ollama.pullButton' })[0]);
    expect(await screen.findByRole('alert')).toHaveTextContent('could not find this model');
    expect(ollama.models).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('button', { name: 'ollama.pullCancel' })).toBeNull();
    expect(screen.getAllByRole('button', { name: 'ollama.pullButton' })[0]).not.toBeDisabled();
  });

  it('keeps a confirmed download successful when refreshing installed models fails', async () => {
    ollama.pull.mockImplementation(async (_model: string, handlers: { onSuccess: (event: unknown) => void }) => {
      handlers.onSuccess({ status: 'success', digest: null, completed: null, total: null });
    });
    ollama.models
      .mockResolvedValueOnce({ models: [] })
      .mockRejectedValueOnce(new Error('refresh unavailable'));
    await mountCard();

    fireEvent.click(screen.getAllByRole('button', { name: 'ollama.pullButton' })[0]);

    expect(await screen.findByText('success')).toBeTruthy();
    expect(await screen.findByRole('alert')).toHaveTextContent('downloaded successfully');
    expect(screen.getByRole('alert')).toHaveTextContent('refresh unavailable');
    expect(screen.getAllByRole('button', { name: 'ollama.pullButton' })[0]).not.toBeDisabled();
  });
});

describe('OllamaCard — download block fold (KT-930)', () => {
  const online = (models_count: number, extra: Record<string, unknown> = {}) => ({
    status: 'online', version: '0.34.2', endpoint: 'http://localhost:11434',
    models_count, hint: null, mlx_capable: false, ...extra,
  });

  it('is folded by default once a model is installed, with a summary of what is inside', async () => {
    ollama.health.mockResolvedValue(online(1));
    ollama.models.mockResolvedValue({ models: [installedModel('qwen3:8b')] });
    await mountCard();

    expect(downloadBlock().open).toBe(false);
    expect(downloadBlock()).toHaveTextContent(`ollama.pullSummarySuggestions(${SUGGESTED_MODELS.length})`);
    expect(downloadBlock()).not.toHaveTextContent('ollama.pullSummaryActive');
  });

  it('is open for a first use, when nothing is installed', async () => {
    ollama.health.mockResolvedValue(online(0));
    await mountCard();

    expect(downloadBlock().open).toBe(true);
    // Open, the summary line steps aside: the list itself is the information.
    expect(downloadBlock()).not.toHaveTextContent('ollama.pullSummarySuggestions');
  });

  it('opens and folds on click, and remembers the choice in this browser', async () => {
    ollama.health.mockResolvedValue(online(1));
    ollama.models.mockResolvedValue({ models: [installedModel('qwen3:8b')] });
    const first = await mountCard();

    expect(localStorage.getItem('kronn:ollamaDownloadOpen')).toBeNull();
    fireEvent.click(screen.getByText('ollama.pullTitle'));
    expect(downloadBlock().open).toBe(true);
    expect(localStorage.getItem('kronn:ollamaDownloadOpen')).toBe('1');

    first.unmount();
    await mountCard();
    expect(downloadBlock().open).toBe(true);

    fireEvent.click(screen.getByText('ollama.pullTitle'));
    expect(downloadBlock().open).toBe(false);
    expect(localStorage.getItem('kronn:ollamaDownloadOpen')).toBe('0');
  });

  it('stays folded for someone who folded it, even with nothing installed', async () => {
    localStorage.setItem('kronn:ollamaDownloadOpen', '0');
    ollama.health.mockResolvedValue(online(0));
    await mountCard();
    expect(downloadBlock().open).toBe(false);
  });

  it('never writes its own default to storage', async () => {
    ollama.health.mockResolvedValue(online(0));
    await mountCard();
    expect(downloadBlock().open).toBe(true);
    expect(localStorage.getItem('kronn:ollamaDownloadOpen')).toBeNull();
  });

  it('ignores an unreadable stored value and falls back to the default', async () => {
    localStorage.setItem('kronn:ollamaDownloadOpen', 'maybe');
    ollama.health.mockResolvedValue(online(1));
    ollama.models.mockResolvedValue({ models: [installedModel('qwen3:8b')] });
    await mountCard();
    expect(downloadBlock().open).toBe(false);
  });

  it('keeps a download in flight visible, and cancellable, while the block is folded', async () => {
    let resolvePull!: () => void;
    ollama.health.mockResolvedValue(online(1));
    ollama.models.mockResolvedValue({ models: [installedModel('qwen3:8b')] });
    ollama.pull.mockImplementation((_model: string, handlers: { onProgress: (event: unknown) => void }) => {
      handlers.onProgress({ status: 'downloading', digest: 'sha256:abc', completed: 1_000_000, total: 4_000_000 });
      return new Promise<void>(resolve => { resolvePull = resolve; });
    });
    await mountCard();

    // Open the block, start a download of a suggestion, then fold it back.
    fireEvent.click(screen.getByText('ollama.pullTitle'));
    fireEvent.click(screen.getAllByRole('button', { name: 'ollama.pullButton' })[0]);
    await waitFor(() => expect(screen.getByText(/1 MB.*4 MB.*25%/)).toBeTruthy());
    fireEvent.click(screen.getByText('ollama.pullTitle'));

    expect(downloadBlock().open).toBe(false);
    // The progress lives outside the foldable block, not merely un-hidden by it.
    expect(downloadBlock().contains(screen.getByText(/1 MB.*4 MB.*25%/))).toBe(false);
    expect(screen.getByRole('button', { name: 'ollama.pullCancel' })).toBeTruthy();
    expect(downloadBlock()).toHaveTextContent('ollama.pullSummaryActive(1)');
    await act(async () => resolvePull());
  });

  it('shows no installed-models list, and no update action, before anything is installed', async () => {
    ollama.health.mockResolvedValue(online(0));
    await mountCard();
    expect(screen.queryByText('ollama.installedModels')).toBeNull();
    expect(screen.queryByText('ollama.updateButton')).toBeNull();
  });
});

describe('OllamaCard — updating an installed model (KT-930)', () => {
  beforeEach(() => {
    ollama.health.mockResolvedValue({
      status: 'online', version: '0.34.2', endpoint: 'http://localhost:11434',
      models_count: 1, hint: null, mlx_capable: false,
    });
    ollama.models.mockResolvedValue({ models: [installedModel('qwen3:8b')] });
  });

  const updateButton = (name: string) => screen.getByRole('button', { name: `ollama.updateFor(${name})` });

  it('offers one update action per installed model, and says where "newer" is read from', async () => {
    ollama.models.mockResolvedValue({
      models: [installedModel('qwen3:8b'), installedModel('custom:latest')],
    });
    await mountCard();
    fireEvent.click(screen.getByText('ollama.pullTitle'));

    expect(screen.getByText('ollama.installedModels')).toBeTruthy();
    expect(updateButton('qwen3:8b')).not.toBeDisabled();
    expect(updateButton('custom:latest')).not.toBeDisabled();
    expect(screen.getByText('ollama.updateHint')).toBeTruthy();
  });

  it('re-pulls the exact installed tag through the download flow, with progress', async () => {
    let resolvePull!: () => void;
    ollama.pull.mockImplementation((_model: string, handlers: { onProgress: (event: unknown) => void }) => {
      handlers.onProgress({ status: 'downloading', digest: 'sha256:abc', completed: 2_000_000, total: 8_000_000 });
      return new Promise<void>(resolve => { resolvePull = resolve; });
    });
    await mountCard();
    fireEvent.click(screen.getByText('ollama.pullTitle'));

    fireEvent.click(updateButton('qwen3:8b'));
    fireEvent.click(updateButton('qwen3:8b'));
    await waitFor(() => expect(ollama.pull).toHaveBeenCalledTimes(1));
    expect(ollama.pull.mock.calls[0][0]).toBe('qwen3:8b');
    expect(ollama.pull.mock.calls[0][2]).toBeInstanceOf(AbortSignal);
    expect(screen.getByText(/2 MB.*8 MB.*25%/)).toBeTruthy();
    expect(updateButton('qwen3:8b')).toBeDisabled();
    await act(async () => resolvePull());
  });

  it('refreshes the installed list after a successful update and frees the action again', async () => {
    ollama.pull.mockImplementation(async (_model: string, handlers: { onSuccess: (event: unknown) => void }) => {
      handlers.onSuccess({ status: 'success', digest: null, completed: null, total: null });
    });
    ollama.models
      .mockResolvedValueOnce({ models: [installedModel('qwen3:8b', { size: '4.0 GB' })] })
      .mockResolvedValueOnce({ models: [installedModel('qwen3:8b', { size: '5.2 GB' })] });
    await mountCard();
    fireEvent.click(screen.getByText('ollama.pullTitle'));

    fireEvent.click(updateButton('qwen3:8b'));
    await waitFor(() => expect(ollama.models).toHaveBeenCalledTimes(2));
    expect(await screen.findByText('success')).toBeTruthy();
    await waitFor(() => expect(updateButton('qwen3:8b')).not.toBeDisabled());
    expect(screen.getAllByText('5.2 GB').length).toBeGreaterThan(0);
  });

  it('cancels an update in flight and leaves it relaunchable', async () => {
    let receivedSignal!: AbortSignal;
    ollama.pull.mockImplementation((_model: string, _handlers: unknown, signal: AbortSignal) => new Promise<void>(resolve => {
      receivedSignal = signal;
      signal.addEventListener('abort', () => resolve());
    }));
    await mountCard();
    fireEvent.click(screen.getByText('ollama.pullTitle'));

    fireEvent.click(updateButton('qwen3:8b'));
    await waitFor(() => expect(screen.getByRole('button', { name: 'ollama.pullCancel' })).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: 'ollama.pullCancel' }));
    await waitFor(() => expect(receivedSignal.aborted).toBe(true));
    await waitFor(() => expect(screen.queryByRole('button', { name: 'ollama.pullCancel' })).toBeNull());
    expect(updateButton('qwen3:8b')).not.toBeDisabled();
  });

  it('surfaces a failed update without refreshing or claiming success', async () => {
    ollama.pull.mockImplementation(async (_model: string, handlers: { onError: (message: string) => void }) => {
      handlers.onError('Ollama lost network access while downloading. Check your connection, then try again.');
    });
    await mountCard();
    fireEvent.click(screen.getByText('ollama.pullTitle'));

    fireEvent.click(updateButton('qwen3:8b'));
    expect(await screen.findByRole('alert')).toHaveTextContent('lost network access');
    expect(ollama.models).toHaveBeenCalledTimes(1);
    expect(screen.queryByText('success')).toBeNull();
    expect(updateButton('qwen3:8b')).not.toBeDisabled();
  });
});

describe('OllamaCard — what the official library says (KT-930)', () => {
  type Verdict = 'up_to_date' | 'update_available' | 'unknown';
  const online = (extra: Record<string, unknown> = {}) => ({
    status: 'online', version: '0.34.2', endpoint: 'http://localhost:11434',
    models_count: 2, hint: null, mlx_capable: false, ...extra,
  });
  const answer = (models: Array<[string, Verdict]>, suggestions: Array<[string, string]> = []) => ({
    models: models.map(([name, status]) => ({ name, status })),
    suggestions: suggestions.map(([name, size]) => ({ name, size })),
  });
  const rowOf = (selector: string, name: string) => {
    const row = [...document.querySelectorAll(selector)]
      .find(node => node.querySelector('.set-ollama-cmd')?.textContent === name);
    if (!row) throw new Error(`no row for ${name} in ${selector}`);
    return row as HTMLElement;
  };
  const installedRow = (name: string) => rowOf('.set-ollama-installed .set-ollama-suggestion', name);
  const suggestionRow = (name: string) => rowOf('.set-ollama-download > .set-ollama-suggestions .set-ollama-suggestion', name);
  const succeed = async (_model: string, handlers: { onSuccess: (event: unknown) => void }) => {
    handlers.onSuccess({ status: 'success', digest: null, completed: null, total: null });
  };

  beforeEach(() => {
    ollama.health.mockResolvedValue(online());
    ollama.models.mockResolvedValue({ models: [installedModel('qwen3:8b'), installedModel('gemma4:12b-mlx')] });
  });

  it('flags the model whose tag moved on, next to its Update action, and only that one', async () => {
    ollama.registry.mockResolvedValue(answer([['qwen3:8b', 'up_to_date'], ['gemma4:12b-mlx', 'update_available']]));
    await mountCard();
    fireEvent.click(screen.getByText('ollama.pullTitle'));

    const outdated = installedRow('gemma4:12b-mlx');
    expect(await within(outdated).findByText('ollama.fresh.update_available')).toBeTruthy();
    expect(within(outdated).getByRole('button', { name: 'ollama.updateFor(gemma4:12b-mlx)' })).toBeTruthy();
    expect(within(installedRow('qwen3:8b')).getByText('ollama.fresh.up_to_date')).toBeTruthy();
    expect(within(installedRow('qwen3:8b')).queryByText('ollama.fresh.update_available')).toBeNull();
  });

  it('still lets a model that is up to date be updated by hand', async () => {
    ollama.registry.mockResolvedValue(answer([['qwen3:8b', 'up_to_date'], ['gemma4:12b-mlx', 'up_to_date']]));
    await mountCard();
    fireEvent.click(screen.getByText('ollama.pullTitle'));
    await within(installedRow('qwen3:8b')).findByText('ollama.fresh.up_to_date');
    expect(screen.getByRole('button', { name: 'ollama.updateFor(qwen3:8b)' })).not.toBeDisabled();
  });

  it('says "not checked", never "up to date", for a model the library said nothing about', async () => {
    ollama.models.mockResolvedValue({
      models: [installedModel('qwen3:8b'), installedModel('hf.co/someone/model:Q4_K_M')],
    });
    ollama.registry.mockResolvedValue(answer([['qwen3:8b', 'up_to_date'], ['hf.co/someone/model:Q4_K_M', 'unknown']]));
    await mountCard();
    fireEvent.click(screen.getByText('ollama.pullTitle'));

    const outside = installedRow('hf.co/someone/model:Q4_K_M');
    const badge = await within(outside).findByText('ollama.fresh.unknown');
    expect(badge).toHaveAttribute('title', 'ollama.fresh.unknownHint');
    expect(within(outside).queryByText('ollama.fresh.up_to_date')).toBeNull();
    expect(within(outside).queryByText('ollama.fresh.update_available')).toBeNull();
  });

  it('reads "not checked" for every model when the library call fails outright', async () => {
    ollama.registry.mockRejectedValue(new Error('registry unreachable'));
    await mountCard();
    fireEvent.click(screen.getByText('ollama.pullTitle'));

    await waitFor(() => expect(screen.getAllByText('ollama.fresh.unknown')).toHaveLength(2));
    expect(screen.queryByText('ollama.fresh.up_to_date')).toBeNull();
    expect(screen.queryByText('ollama.fresh.update_available')).toBeNull();
  });

  it('draws the whole card, Update actions included, while the library has not answered yet', async () => {
    ollama.registry.mockImplementation(() => new Promise(() => {}));
    await mountCard();
    fireEvent.click(screen.getByText('ollama.pullTitle'));

    expect(screen.getByRole('button', { name: 'ollama.updateFor(qwen3:8b)' })).not.toBeDisabled();
    expect(screen.getAllByRole('button', { name: 'ollama.pullButton' }).length).toBeGreaterThan(0);
    expect(screen.queryByText(/^ollama\.fresh\./)).toBeNull();
    expect(screen.getByLabelText('ollama.refresh')).not.toBeDisabled();
  });

  it('says how many updates are waiting even while the block is folded', async () => {
    ollama.registry.mockResolvedValue(answer([['qwen3:8b', 'update_available'], ['gemma4:12b-mlx', 'update_available']]));
    await mountCard();

    await waitFor(() => expect(downloadBlock()).toHaveTextContent('ollama.pullSummaryUpdates(2)'));
    expect(downloadBlock().open).toBe(false);
  });

  it('adds nothing to the folded summary when nothing is outdated or nothing is known', async () => {
    ollama.registry.mockResolvedValue(answer([['qwen3:8b', 'up_to_date'], ['gemma4:12b-mlx', 'unknown']]));
    await mountCard();
    await waitFor(() => expect(ollama.registry).toHaveBeenCalled());
    await act(async () => {});
    expect(downloadBlock()).not.toHaveTextContent('ollama.pullSummaryUpdates');
  });

  it("shows each suggestion's real size once the manifest gave it, and none it did not", async () => {
    ollama.registry.mockResolvedValue(answer([], [['qwen3:8b', '5.2 GB'], ['qwen3:30b-a3b', '19.0 GB']]));
    await mountCard();
    fireEvent.click(screen.getByText('ollama.pullTitle'));

    await waitFor(() => expect(within(suggestionRow('qwen3:8b')).getByText('5.2 GB')).toBeTruthy());
    expect(within(suggestionRow('qwen3:30b-a3b')).getByText('19.0 GB')).toBeTruthy();
    // The library gave no size for this one: no figure, and no invented one.
    expect(suggestionRow('qwen3.5:4b').textContent).not.toMatch(/\d\s?(GB|MB)/);
  });

  it('asks the backend about exactly the tags it suggests, MLX builds included on a Mac', async () => {
    ollama.health.mockResolvedValue(online({ mlx_capable: true }));
    await mountCard();
    await waitFor(() => expect(ollama.registry).toHaveBeenCalled());
    expect(ollama.registry.mock.calls.at(-1)![0]).toEqual([
      ...MLX_SUGGESTED_MODELS.map(m => m.name),
      ...SUGGESTED_MODELS.map(m => m.name),
    ]);
  });

  it('asks only for the portable tags where MLX does not run', async () => {
    await mountCard();
    await waitFor(() => expect(ollama.registry).toHaveBeenCalled());
    expect(ollama.registry.mock.calls.at(-1)![0]).toEqual(SUGGESTED_MODELS.map(m => m.name));
  });

  it('does not ask the library while Ollama is not online', async () => {
    ollama.health.mockResolvedValue(online({ status: 'offline', models_count: 0 }));
    await mountCard();
    await act(async () => {});
    expect(ollama.registry).not.toHaveBeenCalled();
  });

  it('drops the stale verdict after an update and asks the library again', async () => {
    ollama.registry
      .mockResolvedValueOnce(answer([['qwen3:8b', 'update_available'], ['gemma4:12b-mlx', 'up_to_date']]))
      .mockResolvedValue(answer([['qwen3:8b', 'up_to_date'], ['gemma4:12b-mlx', 'up_to_date']]));
    ollama.pull.mockImplementation(succeed);
    await mountCard();
    fireEvent.click(screen.getByText('ollama.pullTitle'));
    await within(installedRow('qwen3:8b')).findByText('ollama.fresh.update_available');

    fireEvent.click(screen.getByRole('button', { name: 'ollama.updateFor(qwen3:8b)' }));

    await waitFor(() => expect(within(installedRow('qwen3:8b')).getByText('ollama.fresh.up_to_date')).toBeTruthy());
    expect(within(installedRow('qwen3:8b')).queryByText('ollama.fresh.update_available')).toBeNull();
    expect(ollama.registry.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it('shows no verdict for a tag it just updated until the library has been asked again', async () => {
    let answerAfter!: (value: unknown) => void;
    ollama.registry
      .mockResolvedValueOnce(answer([['qwen3:8b', 'update_available'], ['gemma4:12b-mlx', 'up_to_date']]))
      .mockImplementation(() => new Promise(resolve => { answerAfter = resolve; }));
    ollama.pull.mockImplementation(succeed);
    await mountCard();
    fireEvent.click(screen.getByText('ollama.pullTitle'));
    await within(installedRow('qwen3:8b')).findByText('ollama.fresh.update_available');

    fireEvent.click(screen.getByRole('button', { name: 'ollama.updateFor(qwen3:8b)' }));

    // Neither the old "update available" nor a made-up "not checked": nothing.
    await waitFor(() => expect(installedRow('qwen3:8b').textContent).not.toMatch(/ollama\.fresh\./));
    expect(within(installedRow('gemma4:12b-mlx')).getByText('ollama.fresh.up_to_date')).toBeTruthy();
    await act(async () => answerAfter(answer([['qwen3:8b', 'up_to_date']])));
    expect(await within(installedRow('qwen3:8b')).findByText('ollama.fresh.up_to_date')).toBeTruthy();
  });
});

describe('OllamaCard — MLX builds on a Mac that runs them (KT-930)', () => {
  const health = (mlx_capable?: boolean) => ({
    status: 'online', version: '0.34.2', endpoint: 'http://localhost:11434',
    models_count: 0, hint: null, ...(mlx_capable === undefined ? {} : { mlx_capable }),
  });

  it('lists the -mlx builds first, flagged as optimized for Mac, then the portable list', async () => {
    ollama.health.mockResolvedValue(health(true));
    await mountCard();

    expect(suggestionNames()).toEqual([
      ...MLX_SUGGESTED_MODELS.map(m => m.name),
      ...SUGGESTED_MODELS.map(m => m.name),
    ]);
    expect(suggestionNames().slice(0, MLX_SUGGESTED_MODELS.length).every(name => name.endsWith('-mlx'))).toBe(true);
    expect(screen.getAllByText('ollama.mlxBadge')).toHaveLength(MLX_SUGGESTED_MODELS.length);
  });

  it('changes nothing when the backend does not report MLX', async () => {
    for (const reported of [false, undefined]) {
      ollama.health.mockResolvedValue(health(reported));
      const view = await mountCard();
      expect(suggestionNames()).toEqual(SUGGESTED_MODELS.map(m => m.name));
      expect(screen.queryByText('ollama.mlxBadge')).toBeNull();
      view.unmount();
    }
  });

  it('decides from the backend verdict, not from the browser user agent', async () => {
    const original = navigator.userAgent;
    Object.defineProperty(navigator, 'userAgent', {
      value: 'Mozilla/5.0 (Macintosh; Apple M3 Mac OS X 14_0)', configurable: true,
    });
    try {
      ollama.health.mockResolvedValue(health(false));
      await mountCard();
      expect(screen.queryByText('ollama.mlxBadge')).toBeNull();
      expect(suggestionNames().some(name => name.endsWith('-mlx'))).toBe(false);
    } finally {
      Object.defineProperty(navigator, 'userAgent', { value: original, configurable: true });
    }
  });

  it('pulls an MLX build under its exact tag', async () => {
    ollama.health.mockResolvedValue(health(true));
    await mountCard();
    fireEvent.click(screen.getAllByRole('button', { name: 'ollama.pullButton' })[0]);
    await waitFor(() => expect(ollama.pull).toHaveBeenCalledTimes(1));
    expect(ollama.pull.mock.calls[0][0]).toBe(MLX_SUGGESTED_MODELS[0].name);
  });
});

describe('OllamaCard — error resilience', () => {
  it('health rejection degrades to an offline rendering without throwing', async () => {
    ollama.health.mockRejectedValue(new Error('ECONNREFUSED'));
    await mountCard();
    // Card mounts ; the offline branch renders the launch wizard.
    expect(document.querySelector('.set-ollama-card')).not.toBeNull();
    expect(screen.getByText('ollama.launchTitle')).toBeTruthy();
  });
});
