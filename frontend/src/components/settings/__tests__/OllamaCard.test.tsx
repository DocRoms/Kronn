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
  ollama: { health: vi.fn(), models: vi.fn() },
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

beforeEach(() => {
  ollama.health.mockResolvedValue({
    status: 'not_installed', version: null, endpoint: '', models_count: 0, hint: null,
  });
  ollama.models.mockResolvedValue({ models: [] });
  config.getModelTiers.mockResolvedValue(baseTiers);
  config.setModelTiers.mockResolvedValue(undefined);
});

afterEach(() => { cleanup(); vi.clearAllMocks(); });

async function mountCard() {
  let result: ReturnType<typeof render>;
  await act(async () => { result = render(<OllamaCard t={t} />); });
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
        { name: 'llama3.2:latest', size: 2_500_000_000, digest: 'sha:abc', modified_at: '2026-01-01' },
        { name: 'qwen2.5-coder:14b', size: 9_000_000_000, digest: 'sha:def', modified_at: '2026-01-02' },
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
  it('choosing a model in the default AND economy selects fires setModelTiers per tier', async () => {
    ollama.health.mockResolvedValue({
      status: 'online', version: '0.3.12', endpoint: 'http://localhost:11434',
      models_count: 1, hint: null,
    });
    ollama.models.mockResolvedValue({
      models: [{ name: 'llama3.2', size: 2_500_000_000, digest: 'sha:abc', modified_at: '2026-01-01' }],
    });
    await mountCard();

    // Default tier → writes ollama.default (aria-label is the i18n key here).
    const defSelect = screen.getByLabelText('disc.tier.default') as HTMLSelectElement;
    await act(async () => { fireEvent.change(defSelect, { target: { value: 'llama3.2' } }); });
    await waitFor(() => expect(config.setModelTiers).toHaveBeenCalled());
    expect(config.setModelTiers.mock.calls[0][0].ollama.default).toBe('llama3.2');

    // Economy tier → the NEW capability: writes ollama.economy independently.
    const ecoSelect = screen.getByLabelText('disc.tier.economy') as HTMLSelectElement;
    await act(async () => { fireEvent.change(ecoSelect, { target: { value: 'llama3.2' } }); });
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
        { name: 'llama3.2', size: 2_500_000_000, digest: 'sha:abc', modified_at: '2026-01-01' },
        { name: 'qwen2.5-coder:14b', size: 9_000_000_000, digest: 'sha:def', modified_at: '2026-01-02' },
      ],
    });
    config.getModelTiers.mockResolvedValue({ ...baseTiers, ollama: { economy: null, reasoning: null, default: 'llama3.2' } });
    config.setModelTiers.mockRejectedValue(new Error('500'));
    await mountCard();

    const defSelect = screen.getByLabelText('disc.tier.default') as HTMLSelectElement;
    expect(defSelect.value).toBe('llama3.2');
    await act(async () => { fireEvent.change(defSelect, { target: { value: 'qwen2.5-coder:14b' } }); });
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

describe('OllamaCard — error resilience', () => {
  it('health rejection degrades to an offline rendering without throwing', async () => {
    ollama.health.mockRejectedValue(new Error('ECONNREFUSED'));
    await mountCard();
    // Card mounts ; the offline branch renders the launch wizard.
    expect(document.querySelector('.set-ollama-card')).not.toBeNull();
    expect(screen.getByText('ollama.launchTitle')).toBeTruthy();
  });
});
