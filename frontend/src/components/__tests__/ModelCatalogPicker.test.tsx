import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../test/apiMock';
import type { CatalogModelEntry, ModelCatalogView } from '../../types/generated';

const { list } = vi.hoisted(() => ({ list: vi.fn() }));
vi.mock('../../lib/api', () => buildApiMock({ modelCatalogApi: { list: list as never } }));
vi.mock('../../lib/I18nContext', () => ({ useT: () => ({ t: (key: string) => key }) }));
import { ModelCatalogPicker } from '../ModelCatalogPicker';

function view(runtime: string, label: string): ModelCatalogView {
  const model: CatalogModelEntry = {
    id: `${runtime}:model`, model_id: 'same-id', display_name: label, runtime_target_id: runtime,
    agent_type: 'Custom', provenance: 'live', availability: 'available', capabilities: ['chat'],
    reasoning_modes: ['minimal', 'xhigh'], default_reasoning_mode: 'minimal', tier_assignment: 'default',
    manual_origin: false, first_seen_at: '2026-09-09T00:00:00Z', last_checked_at: '2026-09-09T00:00:00Z',
    created_at: '2026-09-09T00:00:00Z', updated_at: '2026-09-09T00:00:00Z',
  };
  return { runtime_target_id: runtime, agent_type: 'Custom', stale: false, live_refresh_ok: true, models: [model] };
}
beforeEach(() => { list.mockReset().mockResolvedValue({ targets: [view('http:one', 'One'), view('http:two', 'Two')] }); });
afterEach(cleanup);

describe('ModelCatalogPicker', () => {
  it('switches namespaces without reusing a same-ID alias from the previous connection', async () => {
    const change = vi.fn();
    const rendered = render(<ModelCatalogPicker agent="Custom" connectionId="one" value="same-id" onChange={change} />);
    await act(async () => {});
    expect(screen.getByRole('combobox', { name: 'wiz.model' })).toHaveValue('One');
    rendered.rerender(<ModelCatalogPicker agent="Custom" connectionId="two" value="same-id" onChange={change} />);
    expect(screen.getByRole('combobox', { name: 'wiz.model' })).toHaveValue('Two');
    expect(change).not.toHaveBeenCalled();
    expect(list).toHaveBeenCalledTimes(2); // One read when switching runtime, never on typing.
    await act(async () => {});
  });

  it('keeps an unadvertised saved reasoning mode visibly unavailable without rewriting it', async () => {
    const change = vi.fn();
    const modeChange = vi.fn();
    render(<ModelCatalogPicker agent="Custom" connectionId="one" value="same-id" onChange={change}
      reasoningEffort="operator-mode" onReasoningChange={modeChange} />);
    await act(async () => {});
    const effort = screen.getByRole('combobox', { name: 'wiz.reasoningEffort' });
    expect(effort).toHaveValue('operator-mode — modelCatalog.notInCatalog');
    fireEvent.focus(effort);
    expect(screen.getByRole('option', { name: /operator-mode/ })).toBeDisabled();
    expect(screen.getByRole('option', { name: 'xhigh' })).toBeEnabled();
    expect(modeChange).not.toHaveBeenCalled();
    expect(change).not.toHaveBeenCalled();
  });

  it('resolves an unset HTTP tier through its own configured default for reasoning metadata', async () => {
    render(<ModelCatalogPicker agent="Custom" connectionId="one" value="" onChange={vi.fn()} tier="reasoning"
      targetModelTiers={{ default: 'same-id' }} reasoningEffort="" onReasoningChange={vi.fn()} />);
    await act(async () => {});
    expect(screen.getByRole('combobox', { name: 'wiz.model' })).toHaveAttribute('placeholder', 'same-id');
    expect(screen.getByRole('combobox', { name: 'wiz.reasoningEffort' })).toHaveAttribute('placeholder', 'minimal');
  });

  it('disables editing both values while the caller saves', async () => {
    render(<ModelCatalogPicker agent="Custom" connectionId="one" value="same-id" onChange={vi.fn()}
      reasoningEffort="xhigh" onReasoningChange={vi.fn()} disabled />);
    await act(async () => {});
    expect(screen.getByRole('combobox', { name: 'wiz.model' })).toBeDisabled();
    expect(screen.getByRole('combobox', { name: 'wiz.reasoningEffort' })).toBeDisabled();
  });
});
