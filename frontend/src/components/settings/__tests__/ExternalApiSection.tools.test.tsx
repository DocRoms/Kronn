// ExternalApiSection — the Kronn tools switch of a connection (KT-1600).
//
// Tools are on by default; the switch is an off switch for a model that cannot
// take tool calls. The form must open on, save what the operator chose, and a
// saved connection that has them off must say so on its card.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { ExternalApiConnectionView } from '../../../lib/api';

const { listMock, createMock, updateMock, testProgressMock } = vi.hoisted(() => ({
  listMock: vi.fn(),
  createMock: vi.fn(),
  updateMock: vi.fn(),
  testProgressMock: vi.fn(),
}));

vi.mock('../../../lib/api', () =>
  buildApiMock({
    externalApi: {
      list: listMock as never,
      create: createMock as never,
      update: updateMock as never,
      testProgress: testProgressMock as never,
    },
  }),
);

import { ExternalApiSection } from '../ExternalApiSection';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

function conn(over: Partial<ExternalApiConnectionView>): ExternalApiConnectionView {
  return {
    id: 'id',
    display_name: 'Name',
    mention_alias: 'alias',
    endpoint: null,
    credential_slug: 'conn-slug',
    origin_preset: 'other',
    economy_model: null,
    default_model: null,
    reasoning_model: null,
    created_at: '2026-08-28T00:00:00Z',
    updated_at: '2026-08-28T00:00:00Z',
    has_credential: false,
    ...over,
  };
}

function renderSection() {
  render(<ExternalApiSection t={t} toast={vi.fn()} />);
}

beforeEach(() => {
  listMock.mockResolvedValue([]);
  createMock.mockResolvedValue(conn({}));
  updateMock.mockResolvedValue(conn({}));
  testProgressMock.mockResolvedValue({ done: 0, total: 0 });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('ExternalApiSection — Kronn tools switch (KT-1600)', () => {
  it('opens a new connection with the switch on, the hint shown, and saves true', async () => {
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));
    expect((screen.getByTestId('ext-api-tools') as HTMLInputElement).checked).toBe(true);
    expect(screen.getByText('agents.toolsHint')).toBeInTheDocument();
    fireEvent.change(screen.getByTestId('ext-api-display-name'), { target: { value: 'Gateway' } });
    fireEvent.change(screen.getByTestId('ext-api-mention-alias'), { target: { value: 'gateway' } });
    fireEvent.click(screen.getByTestId('ext-api-save'));
    await waitFor(() => expect(createMock).toHaveBeenCalledWith(expect.objectContaining({ tools_enabled: true })));
  });

  it('saves false once the switch is turned off', async () => {
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));
    fireEvent.click(screen.getByTestId('ext-api-tools'));
    expect((screen.getByTestId('ext-api-tools') as HTMLInputElement).checked).toBe(false);
    fireEvent.change(screen.getByTestId('ext-api-display-name'), { target: { value: 'Gateway' } });
    fireEvent.change(screen.getByTestId('ext-api-mention-alias'), { target: { value: 'gateway' } });
    fireEvent.click(screen.getByTestId('ext-api-save'));
    await waitFor(() => expect(createMock).toHaveBeenCalledWith(expect.objectContaining({ tools_enabled: false })));
  });

  it('reads a saved connection back: unset is on, false is off and flagged on its card', async () => {
    listMock.mockResolvedValue([
      conn({ id: 'plain', endpoint: 'https://a.example.test' }),
      conn({ id: 'off', endpoint: 'https://b.example.test', tools_enabled: false }),
    ]);
    renderSection();
    await screen.findByTestId('ext-api-edit-off');
    expect(screen.getByTestId('ext-api-tools-off-off')).toHaveTextContent('agents.toolsOff');
    expect(screen.queryByTestId('ext-api-tools-off-plain')).toBeNull();

    fireEvent.click(screen.getByTestId('ext-api-edit-off'));
    expect((screen.getByTestId('ext-api-tools') as HTMLInputElement).checked).toBe(false);
    fireEvent.click(screen.getByTestId('ext-api-tools'));
    fireEvent.click(screen.getByTestId('ext-api-save'));
    await waitFor(() => expect(updateMock).toHaveBeenCalledWith('off', expect.objectContaining({ tools_enabled: true })));
  });

  it('opens an unset saved connection with the switch on', async () => {
    listMock.mockResolvedValue([conn({ id: 'plain', endpoint: 'https://a.example.test' })]);
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-edit-plain'));
    expect((screen.getByTestId('ext-api-tools') as HTMLInputElement).checked).toBe(true);
  });
});
