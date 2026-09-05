import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../../test/apiMock';

const { listMock, createMock, deleteMock } = vi.hoisted(() => ({
  listMock: vi.fn(),
  createMock: vi.fn(),
  deleteMock: vi.fn(),
}));

vi.mock('../../../lib/api', () => buildApiMock({
  modelCatalogApi: {
    list: listMock as never,
    createManual: createMock as never,
    deleteManual: deleteMock as never,
  },
}));

import { ModelCatalogSection } from '../ModelCatalogSection';

const snapshot = {
  targets: [
    {
      runtime_target_id: 'http:one',
      target_label: 'Router one',
      agent_type: 'Custom',
      models: [{
        id: 'http:one:shared', runtime_target_id: 'http:one', agent_type: 'Custom',
        model_id: 'shared', display_name: 'Shared one', provenance: 'live', availability: 'available',
        capabilities: ['chat'], reasoning_modes: [], manual_origin: false,
        first_seen_at: '2026-09-01T00:00:00Z', last_seen_at: '2026-09-01T00:00:00Z',
        last_checked_at: '2026-09-01T00:00:00Z', created_at: '2026-09-01T00:00:00Z', updated_at: '2026-09-01T00:00:00Z',
      }],
      live_refresh_ok: true, stale: false,
    },
    {
      runtime_target_id: 'http:two',
      target_label: 'Router two',
      agent_type: 'Custom',
      models: [{
        id: 'http:two:shared', runtime_target_id: 'http:two', agent_type: 'Custom',
        model_id: 'shared', display_name: 'Shared two', provenance: 'manual', availability: 'available',
        capabilities: ['chat', 'image'], reasoning_modes: ['high'], tier_assignment: 'reasoning', manual_origin: true,
        first_seen_at: '2026-09-01T00:00:00Z', last_seen_at: '2026-09-01T00:00:00Z',
        last_checked_at: '2026-09-01T00:00:00Z', created_at: '2026-09-01T00:00:00Z', updated_at: '2026-09-01T00:00:00Z',
      }],
      live_refresh_ok: false, stale: true,
    },
    {
      runtime_target_id: 'agent:opencode',
      target_label: 'OpenCode',
      agent_type: 'OpenCode',
      models: [{
        id: 'agent:opencode:zen', runtime_target_id: 'agent:opencode', agent_type: 'OpenCode',
        model_id: 'opencode/big-pickle', display_name: 'Big Pickle', provenance: 'live', availability: 'available',
        capabilities: ['chat'], reasoning_modes: [], manual_origin: false,
        cost_hint: 'unknown', privacy_note: 'Routed through OpenCode Zen, a third-party gateway.',
        first_seen_at: '2026-09-01T00:00:00Z', last_seen_at: '2026-09-01T00:00:00Z',
        last_checked_at: '2026-09-01T00:00:00Z', created_at: '2026-09-01T00:00:00Z', updated_at: '2026-09-01T00:00:00Z',
      }],
      live_refresh_ok: true, stale: false,
    },
  ],
};

/// One catalogue entry, with only what the test cares about spelled out.
function model(over: { id: string; model_id: string; display_name: string }) {
  return {
    runtime_target_id: 'http:one', agent_type: 'Custom' as const,
    provenance: 'live' as const, availability: 'available' as const,
    capabilities: ['chat'], reasoning_modes: [], manual_origin: false,
    first_seen_at: '2026-09-01T00:00:00Z', last_seen_at: '2026-09-01T00:00:00Z',
    last_checked_at: '2026-09-01T00:00:00Z', created_at: '2026-09-01T00:00:00Z',
    updated_at: '2026-09-01T00:00:00Z',
    ...over,
  };
}

/// The source strip names each target once; the table's "belongs to" column
/// names it again on every row. Tests that only need "the catalogue loaded"
/// wait on the chip.
async function findSourceChip(label: string) {
  return within(
    document.querySelector('.set-model-catalog-sources') as HTMLElement,
  ).findByText(new RegExp(`^${label}`));
}

describe('ModelCatalogSection', () => {
  beforeEach(() => {
    listMock.mockReset().mockResolvedValue(snapshot);
    createMock.mockReset().mockResolvedValue(snapshot.targets[1].models[0]);
    deleteMock.mockReset().mockResolvedValue(undefined);
  });
  afterEach(cleanup);

  it('keeps identical model ids separated by their named HTTP target', async () => {
    render(<ModelCatalogSection />);
    expect(await findSourceChip('Router one')).toBeInTheDocument();
    expect(await findSourceChip('Router two')).toBeInTheDocument();
    expect(screen.getByText('Shared one')).toBeInTheDocument();
    expect(screen.getByText('Shared two')).toBeInTheDocument();
  });

  it('creates a manual model with the selected stable target identity', async () => {
    render(<ModelCatalogSection />);
    await findSourceChip('Router one');
    fireEvent.click(screen.getByText('modelCatalog.add'));
    fireEvent.change(screen.getByLabelText('modelCatalog.target'), { target: { value: 'http:two' } });
    fireEvent.change(screen.getByLabelText('modelCatalog.modelId'), { target: { value: 'new-model' } });
    fireEvent.change(screen.getByLabelText('modelCatalog.displayName'), { target: { value: 'New model' } });
    fireEvent.click(screen.getByText('common.save'));

    await waitFor(() => expect(createMock).toHaveBeenCalledWith(expect.objectContaining({
      runtime_target_id: 'http:two',
      agent_type: 'Custom',
      model_id: 'new-model',
    })));
  });

  it('shows the catalog-driven cost hint and privacy note for an OpenCode Zen model, never a hardcoded name (KT-543)', async () => {
    render(<ModelCatalogSection />);
    const badge = await screen.findByText('modelCatalog.costHint.unknown');
    expect(badge.getAttribute('data-cost-hint')).toBe('unknown');
    expect(badge.getAttribute('title')).toBe('Routed through OpenCode Zen, a third-party gateway.');
    expect(screen.getByText('opencode/big-pickle', { exact: false })).toBeInTheDocument();
  });

  it('does not render a cost badge for a model with no cost_hint', async () => {
    render(<ModelCatalogSection />);
    await findSourceChip('Router one');
    expect(screen.queryByText('modelCatalog.costHint.free')).toBeNull();
    expect(screen.queryByText('modelCatalog.costHint.paid')).toBeNull();
  });

  it('sends the operator-chosen cost hint and privacy note when creating a manual model', async () => {
    render(<ModelCatalogSection />);
    await findSourceChip('Router one');
    fireEvent.click(screen.getByText('modelCatalog.add'));
    fireEvent.change(screen.getByLabelText('modelCatalog.modelId'), { target: { value: 'new-model' } });
    fireEvent.change(screen.getByLabelText('modelCatalog.displayName'), { target: { value: 'New model' } });
    fireEvent.change(screen.getByLabelText('modelCatalog.costHintField'), { target: { value: 'paid' } });
    fireEvent.change(screen.getByLabelText('modelCatalog.privacyNoteField'), { target: { value: 'Billed per token.' } });
    fireEvent.click(screen.getByText('common.save'));

    await waitFor(() => expect(createMock).toHaveBeenCalledWith(expect.objectContaining({
      cost_hint: 'paid',
      privacy_note: 'Billed per token.',
    })));
  });
});

/// KT-588 — 637 models across ten sources were ten stacked lists. Finding one
/// meant scrolling past the other 636.
describe('ModelCatalogSection — the table', () => {
  const names = () => [...document.querySelectorAll(
    '[data-testid="model-catalog-table"] tbody tr td:first-child span',
  )].map(cell => cell.textContent);

  const targets = () => [...document.querySelectorAll(
    '[data-testid="model-catalog-table"] tbody tr td:nth-child(2)',
  )].map(cell => cell.textContent);

  beforeEach(() => {
    listMock.mockResolvedValue({
      targets: [
        {
          runtime_target_id: 'http:one', agent_type: 'Custom', target_label: 'Router one',
          stale: false, models: [
            model({ id: 'z', model_id: 'zeta', display_name: 'Zeta' }),
            model({ id: 'a', model_id: 'alpha', display_name: 'Alpha' }),
          ],
        },
        {
          runtime_target_id: 'agent:codex', agent_type: 'Codex', target_label: 'Codex',
          stale: false, models: [model({ id: 'm', model_id: 'mid', display_name: 'Mid' })],
        },
      ],
    });
  });

  it('lists every source in one alphabetical table', async () => {
    render(<ModelCatalogSection />);
    await screen.findByTestId('model-catalog-table');
    // Across sources, not grouped by them: Mid comes from Codex and still sits
    // between Alpha and Zeta.
    expect(names()).toEqual(['Alpha', 'Mid', 'Zeta']);
  });

  it('inverts the order on a second click of the same column', async () => {
    render(<ModelCatalogSection />);
    await screen.findByTestId('model-catalog-table');

    fireEvent.click(screen.getByTestId('model-catalog-sort-model'));
    expect(names()).toEqual(['Zeta', 'Mid', 'Alpha']);

    fireEvent.click(screen.getByTestId('model-catalog-sort-model'));
    expect(names()).toEqual(['Alpha', 'Mid', 'Zeta']);
  });

  /// Ties fall back to the model name, so sorting by source is alphabetical
  /// inside each source rather than in whatever order the fetch returned.
  it('sorts by source, and alphabetically within one', async () => {
    render(<ModelCatalogSection />);
    await screen.findByTestId('model-catalog-table');

    fireEvent.click(screen.getByTestId('model-catalog-sort-target'));
    expect(targets()).toEqual(['Codex', 'Router one', 'Router one']);
    expect(names()).toEqual(['Mid', 'Alpha', 'Zeta']);
  });

  it('searches the exact id as well as the displayed name', async () => {
    render(<ModelCatalogSection />);
    await screen.findByTestId('model-catalog-table');

    // The id is often the only thing the reader remembers: it is what gets
    // pasted into a tier.
    fireEvent.change(screen.getByTestId('model-catalog-search'), { target: { value: 'alpha' } });
    expect(names()).toEqual(['Alpha']);

    fireEvent.change(screen.getByTestId('model-catalog-search'), { target: { value: 'zeta' } });
    expect(names()).toEqual(['Zeta']);
  });

  it('narrows to one source, and back', async () => {
    render(<ModelCatalogSection />);
    await screen.findByTestId('model-catalog-table');

    fireEvent.click(await findSourceChip('Codex'));
    expect(names()).toEqual(['Mid']);

    fireEvent.click(screen.getByTestId('model-catalog-clear-filter'));
    expect(names()).toEqual(['Alpha', 'Mid', 'Zeta']);
  });

  it('says so when nothing matches, rather than showing an empty table', async () => {
    render(<ModelCatalogSection />);
    await screen.findByTestId('model-catalog-table');

    fireEvent.change(screen.getByTestId('model-catalog-search'), { target: { value: 'nothing' } });
    expect(screen.queryByTestId('model-catalog-table')).toBeNull();
    expect(screen.getByText('modelCatalog.noMatch')).toBeInTheDocument();
  });
});
