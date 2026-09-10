import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../../test/apiMock';
import type { CatalogModelEntry, ModelCatalogSnapshot, ModelCatalogView, ModelTiersConfig } from '../../../types/generated';

const { health, models, list, refresh, getTiers, setTiers } = vi.hoisted(() => ({
  health: vi.fn(), models: vi.fn(), list: vi.fn(), refresh: vi.fn(), getTiers: vi.fn(), setTiers: vi.fn(),
}));
vi.mock('../../../lib/api', () => buildApiMock({
  ollama: { health: health as never, models: models as never },
  modelCatalogApi: { list: list as never, refresh: refresh as never },
  config: { getModelTiers: getTiers as never, setModelTiers: setTiers as never },
}));
import { OllamaCard } from '../OllamaCard';

const t = (key: string, ...args: (string | number)[]) => args.length ? `${key}:${args.join(',')}` : key;
const checkedAt = '2026-09-10T12:00:00Z';
function tiers(): ModelTiersConfig {
  const empty = () => ({ economy: null, default: null, reasoning: null });
  return { claude_code: empty(), codex: empty(), open_code: empty(), gemini_cli: empty(),
    kiro: empty(), vibe: empty(), copilot_cli: empty(), lite_llm: empty(), nvidia: empty(),
    ollama: { economy: null, default: 'current', reasoning: null } };
}
function model(id: string, overrides: Partial<CatalogModelEntry> = {}): CatalogModelEntry {
  return { id: `agent:ollama:${id}`, runtime_target_id: 'agent:ollama', agent_type: 'Ollama',
    model_id: id, display_name: id, provenance: 'live', availability: 'available',
    capabilities: ['chat'], reasoning_modes: [], manual_origin: false,
    first_seen_at: checkedAt, last_seen_at: checkedAt, last_checked_at: checkedAt,
    created_at: checkedAt, updated_at: checkedAt, ...overrides };
}
function target(entries: CatalogModelEntry[], overrides: Partial<ModelCatalogView> = {}): ModelCatalogView {
  return { runtime_target_id: 'agent:ollama', agent_type: 'Ollama', models: entries,
    live_refresh_ok: true, stale: false, ...overrides };
}
function snapshot(...targets: ModelCatalogView[]): ModelCatalogSnapshot { return { targets }; }
async function show() {
  await act(async () => { render(<OllamaCard t={t} />); });
  await waitFor(() => expect(screen.getByLabelText('disc.tier.default')).not.toBeDisabled());
  return screen.getByLabelText('disc.tier.default') as HTMLInputElement;
}
async function choose(input: HTMLInputElement, name: string) {
  fireEvent.focus(input);
  fireEvent.change(input, { target: { value: name } });
  const option = await screen.findByRole('option', { name });
  await act(async () => { fireEvent.click(option); });
}
beforeEach(() => {
  vi.clearAllMocks();
  health.mockReset().mockResolvedValue({ status: 'online', version: null, endpoint: 'fixture', models_count: 2, hint: null });
  models.mockReset().mockResolvedValue({ models: ['current', 'next'].map(name => ({
    name, size: '1 GB', modified: checkedAt, advertised_context: 8192,
    context_ceiling: 8192, context_override: null, context_origin: 'model_limit',
  })) });
  list.mockReset().mockResolvedValue(snapshot(target([model('current'), model('next')])));
  refresh.mockReset().mockResolvedValue(target([]));
  getTiers.mockReset().mockResolvedValue(tiers());
  setTiers.mockReset().mockResolvedValue(undefined);
});
afterEach(cleanup);

describe('OllamaCard shared catalogue and safe persistence (KT-531)', () => {
  it('keeps offline configured identities and manual catalogue choices visible', async () => {
    health.mockResolvedValue({ status: 'offline', version: null, endpoint: 'fixture', models_count: 0, hint: null });
    list.mockResolvedValue(snapshot(target([model('manual-local', { display_alias: 'Local manuel', provenance: 'manual' })], { stale: true, live_refresh_ok: false })));
    const input = await show();
    expect(input.value).toContain('current');
    fireEvent.focus(input);
    expect(await screen.findByRole('option', { name: 'Local manuel' })).toHaveTextContent('modelCatalog.provenance.manual');
    expect(screen.getByRole('option', { name: /current.*modelCatalog.notInCatalog/ })).toHaveAttribute('aria-disabled', 'true');
    expect(models).not.toHaveBeenCalled();
    expect(setTiers).not.toHaveBeenCalled();
  });

  it('reads only the exact runtime catalogue and searches aliases without discovery or saving', async () => {
    list.mockResolvedValue(snapshot(
      target([model('local/new', { display_alias: 'Nouveau local' })]),
      target([model('http-only', { runtime_target_id: 'http:remote', display_alias: 'Remote' })], { runtime_target_id: 'http:remote' }),
    ));
    const input = await show();
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: 'Nouveau' } });
    const option = await screen.findByRole('option', { name: 'Nouveau local' });
    expect(option).toHaveTextContent('local/new');
    expect(option).toHaveTextContent('modelCatalog.provenance.live');
    expect(option).toHaveTextContent(checkedAt);
    expect(screen.queryByRole('option', { name: 'Remote' })).not.toBeInTheDocument();
    expect(screen.queryByRole('option', { name: 'next' })).not.toBeInTheDocument();
    expect(refresh).not.toHaveBeenCalled();
    expect(setTiers).not.toHaveBeenCalled();
  });

  it('keeps a disappeared choice disabled with cached provenance and its reason', async () => {
    list.mockResolvedValue(snapshot(target([model('current', {
      availability: 'unavailable', unavailable_reason: 'disappeared', unavailable_detail: 'No longer installed',
    })], { stale: true, live_refresh_ok: false })));
    fireEvent.focus(await show());
    const option = await screen.findByRole('option', { name: /current.*modelCatalog.unavailable/ });
    expect(option).toHaveAttribute('aria-disabled', 'true');
    expect(option).toHaveTextContent('modelCatalog.provenance.cached');
    expect(option).toHaveTextContent('No longer installed');
    fireEvent.click(option);
    expect(setTiers).not.toHaveBeenCalled();
  });

  it('merges one selected tier into freshly read settings', async () => {
    const input = await show();
    const fresh = tiers();
    fresh.codex.default = 'operator-updated';
    fresh.ollama.reasoning = 'operator-local-deep';
    getTiers.mockResolvedValue(fresh);
    await choose(input, 'next');
    await waitFor(() => expect(setTiers).toHaveBeenCalledWith({ ...fresh, ollama: { ...fresh.ollama, default: 'next' } }));
  });

  it('refuses a stale write when the fresh settings read fails, keeping the confirmed choice', async () => {
    const input = await show();
    getTiers.mockRejectedValue(new Error('settings unavailable'));
    await choose(input, 'next');
    expect(await screen.findByRole('alert')).toHaveTextContent('config.saveError');
    expect(setTiers).not.toHaveBeenCalled();
    expect(input).toHaveValue('current');
  });

  it('guards two synchronous choices and keeps confirmed settings until the write succeeds', async () => {
    let resolveSave!: () => void;
    setTiers.mockImplementation(() => new Promise<void>(resolve => { resolveSave = resolve; }));
    const input = await show();
    fireEvent.focus(input);
    const option = await screen.findByRole('option', { name: 'next' });
    act(() => { option.click(); option.click(); });
    await waitFor(() => expect(setTiers).toHaveBeenCalledTimes(1));
    expect(input).toHaveValue('current');
    expect(input).toBeDisabled();
    const refreshButton = screen.getByRole('button', { name: 'ollama.refresh' });
    expect(refreshButton).toBeDisabled();
    const reads = getTiers.mock.calls.length;
    fireEvent.click(refreshButton);
    expect(getTiers).toHaveBeenCalledTimes(reads);
    await act(async () => resolveSave());
    expect(input).toHaveValue('next');
  });

  it('reports snapshot failure and reloads it without probing the provider', async () => {
    list.mockRejectedValue(new Error('snapshot unavailable'));
    const input = await show();
    expect(await screen.findByRole('alert')).toHaveTextContent('modelCatalog.loadError');
    expect(input.value).toContain('current');
    const healthCalls = health.mock.calls.length;
    const inventoryCalls = models.mock.calls.length;
    list.mockResolvedValue(snapshot(target([model('recovered')])));
    fireEvent.click(screen.getByRole('button', { name: 'modelCatalog.reload' }));
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
    fireEvent.focus(input);
    expect(await screen.findByRole('option', { name: 'recovered' })).toBeInTheDocument();
    expect(health).toHaveBeenCalledTimes(healthCalls);
    expect(models).toHaveBeenCalledTimes(inventoryCalls);
    expect(refresh).not.toHaveBeenCalled();
  });

  it('clears only the explicit override and shows the assigned catalogue default', async () => {
    list.mockResolvedValue(snapshot(target([model('current'), model('runtime-default', { tier_assignment: 'default' })])));
    const input = await show();
    fireEvent.focus(input);
    const option = await screen.findByRole('option', { name: 'ollama.tierAuto (runtime-default)' });
    await act(async () => { fireEvent.click(option); });
    await waitFor(() => expect(setTiers).toHaveBeenCalledWith({ ...tiers(), ollama: { ...tiers().ollama, default: null } }));
    expect(input).toHaveValue('');
    expect(input).toHaveAttribute('placeholder', 'ollama.tierAuto (runtime-default)');
  });

  it('reports an initial settings failure and recovers through explicit refresh without writing', async () => {
    getTiers.mockRejectedValue(new Error('settings unavailable'));
    await act(async () => { render(<OllamaCard t={t} />); });
    expect(await screen.findByRole('alert')).toHaveTextContent('common.error');
    expect(screen.getByLabelText('disc.tier.default')).toBeDisabled();
    expect(setTiers).not.toHaveBeenCalled();
    getTiers.mockResolvedValue(tiers());
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'ollama.refresh' })); });
    await waitFor(() => expect(screen.getByLabelText('disc.tier.default')).not.toBeDisabled());
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.getByLabelText('disc.tier.default')).toHaveValue('current');
    expect(setTiers).not.toHaveBeenCalled();
  });
});
