import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
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
  ],
};

describe('ModelCatalogSection', () => {
  beforeEach(() => {
    listMock.mockReset().mockResolvedValue(snapshot);
    createMock.mockReset().mockResolvedValue(snapshot.targets[1].models[0]);
    deleteMock.mockReset().mockResolvedValue(undefined);
  });
  afterEach(cleanup);

  it('keeps identical model ids separated by their named HTTP target', async () => {
    render(<ModelCatalogSection />);
    expect(await screen.findByText('Router one')).toBeInTheDocument();
    expect(screen.getByText('Router two')).toBeInTheDocument();
    expect(screen.getByText('Shared one')).toBeInTheDocument();
    expect(screen.getByText('Shared two')).toBeInTheDocument();
  });

  it('creates a manual model with the selected stable target identity', async () => {
    render(<ModelCatalogSection />);
    await screen.findByText('Router one');
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
});
