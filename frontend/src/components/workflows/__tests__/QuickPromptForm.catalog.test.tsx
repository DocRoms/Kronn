import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../../test/apiMock';
import type { CatalogModelEntry, QuickPrompt } from '../../../types/generated';

const { list, ollamaModels } = vi.hoisted(() => ({ list: vi.fn(), ollamaModels: vi.fn() }));
vi.mock('../../../lib/api', () => buildApiMock({
  modelCatalogApi: { list: list as never }, ollama: { models: ollamaModels as never },
}));
vi.mock('../../../lib/I18nContext', () => ({ useT: () => ({ t: (key: string) => key }) }));
import { QuickPromptForm } from '../QuickPromptForm';

function entry(id: string, patch: Partial<CatalogModelEntry> = {}): CatalogModelEntry {
  return { id, model_id: id, runtime_target_id: 'agent:codex', agent_type: 'Codex', display_name: id,
    provenance: 'live', availability: 'available', capabilities: ['chat'], reasoning_modes: ['minimal', 'xhigh'],
    tier_assignment: 'default', manual_origin: false, first_seen_at: '2026-09-09T00:00:00Z',
    last_checked_at: '2026-09-09T00:00:00Z', created_at: '2026-09-09T00:00:00Z', updated_at: '2026-09-09T00:00:00Z', ...patch };
}
function makePrompt(patch: Partial<QuickPrompt> = {}): QuickPrompt {
  return { id: 'catalog-qp', name: 'Review', icon: 'Q', prompt_template: 'Review the changes', variables: [],
    pinned: false, agent: 'Codex', tier: 'default', description: '', project_id: null,
    skill_ids: [], profile_ids: [], directive_ids: [],
    agent_settings: { model: 'configured-model', reasoning_effort: 'xhigh', max_tokens: 12345 },
    created_at: '2026-09-09T00:00:00Z', updated_at: '2026-09-09T00:00:00Z', ...patch };
}
async function show(value = makePrompt()) {
  const save = vi.fn().mockResolvedValue(undefined);
  await act(async () => { render(<QuickPromptForm editPrompt={value} projects={[]} onSave={save} onCancel={vi.fn()} />); });
  return save;
}
beforeEach(() => {
  list.mockReset().mockResolvedValue({ targets: [{ runtime_target_id: 'agent:codex', agent_type: 'Codex',
    stale: false, live_refresh_ok: true, models: [entry('configured-model'), entry('live-new-model', { tier_assignment: null })] }] });
  ollamaModels.mockReset().mockResolvedValue({ models: [] });
});
afterEach(cleanup);

describe('Quick Prompt catalogue editing', () => {
  it('preserves unrelated expert settings when saving an existing explicit model', async () => {
    const save = await show();
    fireEvent.click(screen.getByRole('button', { name: 'qp.save' }));
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(save.mock.calls[0][0].agent_settings).toMatchObject({ model: 'configured-model', reasoning_effort: 'xhigh', max_tokens: 12345 });
  });

  it('offers searchable dynamic IDs for a CLI without probing Ollama', async () => {
    const save = await show();
    const picker = screen.getByRole('combobox', { name: 'wiz.model' });
    fireEvent.focus(picker);
    fireEvent.change(picker, { target: { value: 'live-new' } });
    fireEvent.click(screen.getByRole('option', { name: 'live-new-model' }));
    fireEvent.click(screen.getByRole('button', { name: 'qp.save' }));
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(save.mock.calls[0][0].agent_settings.model).toBe('live-new-model');
    expect(ollamaModels).not.toHaveBeenCalled();
  });

  it('shows unavailable and stale entries without enabling a silent replacement', async () => {
    list.mockResolvedValue({ targets: [{ runtime_target_id: 'agent:codex', agent_type: 'Codex',
      stale: true, live_refresh_ok: false, models: [entry('configured-model', { availability: 'unavailable' })] }] });
    await show();
    fireEvent.focus(screen.getByRole('combobox', { name: 'wiz.model' }));
    expect(screen.getByRole('option', { name: /configured-model/ })).toBeDisabled();
    expect(screen.getByText(/modelCatalog.provenance.cached/)).toBeInTheDocument();
  });

  it('allows an explicit unknown ID without claiming it is discovered', async () => {
    const save = await show();
    const picker = screen.getByRole('combobox', { name: 'wiz.model' });
    fireEvent.focus(picker);
    fireEvent.change(picker, { target: { value: 'operator/model-β' } });
    const custom = screen.getByRole('option', { name: 'operator/model-β — modelCatalog.notInCatalog' });
    expect(custom).toBeEnabled();
    fireEvent.click(custom);
    fireEvent.click(screen.getByRole('button', { name: 'qp.save' }));
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(save.mock.calls[0][0].agent_settings.model).toBe('operator/model-β');
  });

  it('reports a snapshot failure and reloads without changing the saved identity', async () => {
    list.mockRejectedValueOnce(new Error('offline'));
    const save = await show();
    expect(screen.getByRole('alert')).toHaveTextContent('modelCatalog.loadError');
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'modelCatalog.reload' })); });
    expect(screen.getByRole('combobox', { name: 'wiz.model' })).toHaveValue('configured-model');
    expect(save).not.toHaveBeenCalled();
  });
});
