/**
 * 0.8.7 — P1-7a of the QA roadmap.
 *
 * OllamaCard has 4 explicit states (not_installed / offline+unreachable /
 * online-zero-models / online+models) and an async default-model picker
 * with optimistic-rollback semantics. Pre-test : zero coverage. Pinned
 * here :
 *  - the 4 states render their respective wizard / picker UI
 *  - the canirun.ai hint always renders (regression for the 2026-05-11
 *    user report where it was hidden too low)
 *  - default-model picker is optimistic ; rollback fires on POST failure
 *  - refresh button re-fetches health + models
 *  - health fetch errors degrade to an "offline" rendering without crash
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, act, cleanup, waitFor } from '@testing-library/react';

const { ollama, config } = vi.hoisted(() => ({
  ollama: { health: vi.fn(), models: vi.fn(), pull: vi.fn(), setContextOverride: vi.fn() },
  config: { getModelTiers: vi.fn(), setModelTiers: vi.fn() },
}));

vi.mock('../../../lib/api', () => ({ ollama, config }));

import { OllamaCard } from '../OllamaCard';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}(${args.join('|')})` : key;

const baseTiers = {
  claude_code: { economy: null, reasoning: null, default: null },
  codex: { economy: null, reasoning: null, default: null },
  gemini_cli: { economy: null, reasoning: null, default: null },
  kiro: { economy: null, reasoning: null, default: null },
  vibe: { economy: null, reasoning: null, default: null },
  copilot_cli: { economy: null, reasoning: null, default: null },
  ollama: { economy: null, reasoning: null, default: null },
};

const installedModel = (name: string, overrides: Record<string, unknown> = {}) => ({
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
  ollama.health.mockResolvedValue({
    status: 'not_installed', version: null, endpoint: '', models_count: 0, hint: null,
  });
  ollama.models.mockResolvedValue({ models: [] });
  ollama.pull.mockResolvedValue(undefined);
  ollama.setContextOverride.mockResolvedValue({ model: '', num_ctx: null, warnings: [] });
  config.getModelTiers.mockResolvedValue(baseTiers);
  config.setModelTiers.mockResolvedValue(undefined);
});

afterEach(() => { cleanup(); vi.clearAllMocks(); });

async function mountCard(modelCostSuffix?: (model: string) => string) {
  let result: ReturnType<typeof render>;
  await act(async () => { result = render(<OllamaCard t={t} modelCostSuffix={modelCostSuffix} />); });
  await act(async () => { await new Promise(r => setTimeout(r, 0)); });
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
    // At least one of the suggested models appears in the UI. (The list now
    // includes both `llama3.2:1b` and `llama3.2`, so match-all + count.)
    expect(screen.getAllByText(/llama3\.2/).length).toBeGreaterThan(0);
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
      fireEvent.click(screen.getByRole('option', { name: /llama3\.2/ }));
    });
    await waitFor(() => expect(config.setModelTiers).toHaveBeenCalled());
    expect(config.setModelTiers.mock.calls[0][0].ollama.default).toBe('llama3.2');

    // Economy tier → the NEW capability: writes ollama.economy independently.
    const ecoSelect = screen.getByLabelText('disc.tier.economy') as HTMLInputElement;
    await act(async () => {
      fireEvent.focus(ecoSelect);
      fireEvent.change(ecoSelect, { target: { value: 'llama3.2' } });
      fireEvent.click(screen.getByRole('option', { name: /llama3\.2/ }));
    });
    await waitFor(() => expect(config.setModelTiers.mock.calls.length).toBeGreaterThan(1));
    const last = config.setModelTiers.mock.calls.at(-1)![0];
    expect(last.ollama.economy).toBe('llama3.2');
  });

  it('rolls back the select to its prior value when setModelTiers fails', async () => {
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
    // Optimistic flip reverted on failure → select shows the original model again.
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
    resolvePull();
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

describe('OllamaCard — error resilience', () => {
  it('health rejection degrades to an offline rendering without throwing', async () => {
    ollama.health.mockRejectedValue(new Error('ECONNREFUSED'));
    await mountCard();
    // Card mounts ; the offline branch renders the launch wizard.
    expect(document.querySelector('.set-ollama-card')).not.toBeNull();
    expect(screen.getByText('ollama.launchTitle')).toBeTruthy();
  });
});
