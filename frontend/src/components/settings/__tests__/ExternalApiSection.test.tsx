// ExternalApiSection — unified "External API" settings zone (KT-339).
//
// Pins the three behaviours the task's Definition of Done cares about:
//   1. one zone with a preset selector that pre-fills the endpoint;
//   2. several connections coexist, each with its own endpoint + tiers;
//   3. a THIRD compatible service is added from the UI alone via the generic
//      "Other" preset — no new enum variant, no dedicated card, no new i18n key
//      (the same generic form handles it).

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { ExternalApiConnectionView } from '../../../lib/api';

const { listMock, createMock, updateMock, removeMock } = vi.hoisted(() => ({
  listMock: vi.fn(),
  createMock: vi.fn(),
  updateMock: vi.fn(),
  removeMock: vi.fn(),
}));

vi.mock('../../../lib/api', () =>
  buildApiMock({
    externalApi: {
      list: listMock as never,
      create: createMock as never,
      update: updateMock as never,
      remove: removeMock as never,
    },
  }),
);

import { ExternalApiSection } from '../ExternalApiSection';

// Echo translator: returns `key` (or `key:arg,arg` with args) so assertions
// stay locale-independent.
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
  const toast = vi.fn();
  render(<ExternalApiSection t={t} toast={toast} />);
  return { toast };
}

beforeEach(() => {
  listMock.mockResolvedValue([]);
  createMock.mockResolvedValue(conn({}));
  updateMock.mockResolvedValue(conn({}));
  removeMock.mockResolvedValue(null);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('ExternalApiSection', () => {
  it('renders several connections at once, each with its own endpoint (DoD 2)', async () => {
    listMock.mockResolvedValue([
      conn({
        id: 'nvidia-1',
        display_name: 'NVIDIA',
        mention_alias: 'nvidia',
        origin_preset: 'nvidia',
        endpoint: 'https://integrate.api.nvidia.com',
        has_credential: true,
      }),
      conn({
        id: 'groq-1',
        display_name: 'Groq',
        mention_alias: 'groq',
        origin_preset: 'other',
        endpoint: 'https://api.groq.com/openai/v1',
      }),
    ]);

    renderSection();

    const cards = await screen.findAllByTestId('ext-api-connection');
    expect(cards).toHaveLength(2);
    const endpoints = cards.map(c => c.querySelector('.set-ext-api-conn-endpoint')?.textContent);
    expect(endpoints).toContain('https://integrate.api.nvidia.com');
    expect(endpoints).toContain('https://api.groq.com/openai/v1');
  });

  it('pre-fills the endpoint from the chosen preset (DoD 1)', async () => {
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));

    const endpoint = () => screen.getByTestId('ext-api-endpoint') as HTMLInputElement;
    // The default preset already seeds an endpoint on open.
    expect(endpoint().value).toBe('http://localhost:4000');

    fireEvent.click(screen.getByTestId('ext-api-preset-nvidia'));
    expect(endpoint().value).toBe('https://integrate.api.nvidia.com');

    fireEvent.click(screen.getByTestId('ext-api-preset-litellm'));
    expect(endpoint().value).toBe('http://localhost:4000');

    // "Other" clears it so a brand-new service starts from a blank endpoint.
    fireEvent.click(screen.getByTestId('ext-api-preset-other'));
    expect(endpoint().value).toBe('');
  });

  it('adds a third compatible service from the UI via the generic Other preset (DoD 3)', async () => {
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));

    // The same three presets and the same generic form — no per-service card.
    expect(screen.getByTestId('ext-api-preset-litellm')).toBeTruthy();
    expect(screen.getByTestId('ext-api-preset-nvidia')).toBeTruthy();
    fireEvent.click(screen.getByTestId('ext-api-preset-other'));

    fireEvent.change(screen.getByTestId('ext-api-display-name'), { target: { value: 'Together' } });
    fireEvent.change(screen.getByTestId('ext-api-mention-alias'), { target: { value: 'together' } });
    fireEvent.change(screen.getByTestId('ext-api-endpoint'), {
      target: { value: 'https://api.together.xyz/v1' },
    });
    fireEvent.change(screen.getByTestId('ext-api-key'), { target: { value: 'sk-together' } });
    fireEvent.change(screen.getByTestId('ext-api-tier-default'), {
      target: { value: 'meta-llama/Llama-3-70b' },
    });

    fireEvent.click(screen.getByTestId('ext-api-save'));

    await waitFor(() => expect(createMock).toHaveBeenCalledTimes(1));
    expect(createMock).toHaveBeenCalledWith({
      display_name: 'Together',
      mention_alias: 'together',
      endpoint: 'https://api.together.xyz/v1',
      origin_preset: 'other',
      economy_model: null,
      default_model: 'meta-llama/Llama-3-70b',
      reasoning_model: null,
      api_key: 'sk-together',
    });
    // The list is reloaded after a successful create.
    expect(listMock).toHaveBeenCalledTimes(2);
  });

  it('keeps the stored key when editing without retyping it', async () => {
    listMock.mockResolvedValue([
      conn({ id: 'groq-1', display_name: 'Groq', mention_alias: 'groq', has_credential: true }),
    ]);
    renderSection();

    fireEvent.click(await screen.findByTestId('ext-api-edit-groq-1'));
    fireEvent.change(screen.getByTestId('ext-api-display-name'), { target: { value: 'Groq Prod' } });
    fireEvent.click(screen.getByTestId('ext-api-save'));

    await waitFor(() => expect(updateMock).toHaveBeenCalledTimes(1));
    // api_key null = "keep the stored credential" (the field was never touched).
    expect(updateMock).toHaveBeenCalledWith('groq-1', expect.objectContaining({
      display_name: 'Groq Prod',
      api_key: null,
    }));
  });
});
