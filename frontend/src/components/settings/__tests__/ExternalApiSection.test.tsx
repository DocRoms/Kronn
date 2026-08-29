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

const { listMock, createMock, updateMock, removeMock, testMock } = vi.hoisted(() => ({
  listMock: vi.fn(),
  createMock: vi.fn(),
  updateMock: vi.fn(),
  removeMock: vi.fn(),
  testMock: vi.fn(),
}));

vi.mock('../../../lib/api', () =>
  buildApiMock({
    externalApi: {
      list: listMock as never,
      create: createMock as never,
      update: updateMock as never,
      remove: removeMock as never,
      test: testMock as never,
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
  testMock.mockResolvedValue({ ok: true, status: 'success', models: ['model-a', 'model-b'], hint: null });
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
    // The user pastes the base URL documented by Together — which ends in `/v1`.
    // The UI forwards it verbatim; the backend normalizes the trailing `/v1`
    // away so the shared OpenAiCodec appends exactly one `/v1/chat/completions`
    // (proven backend-side in normalized_endpoint_yields_the_correct_final_chat_url).
    fireEvent.change(screen.getByTestId('ext-api-endpoint'), {
      target: { value: 'https://api.together.xyz/v1' },
    });
    fireEvent.change(screen.getByTestId('ext-api-key'), { target: { value: 'sk-together' } });
    fireEvent.click(screen.getByTestId('ext-api-test'));
    await waitFor(() => expect(screen.getByTestId('ext-api-tier-default')).not.toBeDisabled());
    fireEvent.change(screen.getByTestId('ext-api-tier-default'), { target: { value: 'model-a' } });

    fireEvent.click(screen.getByTestId('ext-api-save'));

    await waitFor(() => expect(createMock).toHaveBeenCalledTimes(1));
    expect(createMock).toHaveBeenCalledWith({
      display_name: 'Together',
      mention_alias: 'together',
      endpoint: 'https://api.together.xyz/v1',
      origin_preset: 'other',
      economy_model: null,
      default_model: 'model-a',
      reasoning_model: null,
      api_key: 'sk-together',
    });
    // The list is reloaded after a successful create.
    expect(listMock).toHaveBeenCalledTimes(2);
  });

  it('blocks Save until a non-empty endpoint is provided (Other preset clears it)', async () => {
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));

    // "Other" clears the endpoint: a connection with no endpoint is not
    // executable, so Save must stay disabled even with a name and alias.
    fireEvent.click(screen.getByTestId('ext-api-preset-other'));
    fireEvent.change(screen.getByTestId('ext-api-display-name'), { target: { value: 'Groq' } });
    fireEvent.change(screen.getByTestId('ext-api-mention-alias'), { target: { value: 'groq' } });
    expect((screen.getByTestId('ext-api-endpoint') as HTMLInputElement).value).toBe('');
    expect((screen.getByTestId('ext-api-save') as HTMLButtonElement).disabled).toBe(true);

    // Typing an endpoint unlocks it.
    fireEvent.change(screen.getByTestId('ext-api-endpoint'), {
      target: { value: 'https://api.groq.com/openai/v1' },
    });
    expect((screen.getByTestId('ext-api-save') as HTMLButtonElement).disabled).toBe(false);
  });

  it('keeps the stored key when editing without retyping it', async () => {
    listMock.mockResolvedValue([
      conn({
        id: 'groq-1',
        display_name: 'Groq',
        mention_alias: 'groq',
        endpoint: 'https://api.groq.com/openai/v1',
        has_credential: true,
      }),
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

  it('loads tested models into every tier selector without saving the draft', async () => {
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));
    expect(screen.getByTestId('ext-api-test-required')).toBeTruthy();
    expect((screen.getByTestId('ext-api-tier-default') as HTMLSelectElement).disabled).toBe(true);

    fireEvent.click(screen.getByTestId('ext-api-test'));
    await waitFor(() => expect(testMock).toHaveBeenCalledWith({ endpoint: 'http://localhost:4000', api_key: null }));
    expect(createMock).not.toHaveBeenCalled();
    expect((screen.getByTestId('ext-api-tier-default') as HTMLSelectElement).disabled).toBe(false);
    expect(screen.getAllByRole('option', { name: 'model-a' })).toHaveLength(3);
  });

  it('tests a saved connection with its server-side credential and renders the returned models', async () => {
    listMock.mockResolvedValue([conn({
      id: 'groq-1',
      display_name: 'Groq',
      mention_alias: 'groq',
      endpoint: 'https://api.groq.com/openai',
      origin_preset: 'other',
      has_credential: true,
    })]);
    renderSection();

    fireEvent.click(await screen.findByTestId('ext-api-test-saved-groq-1'));
    await waitFor(() => expect(testMock).toHaveBeenCalledWith({
      endpoint: 'https://api.groq.com/openai',
      api_key: null,
      connection_id: 'groq-1',
      origin_preset: 'other',
    }));
    expect(await screen.findByTestId('ext-api-saved-test-result-groq-1')).toHaveAttribute('data-status', 'success');
    expect(screen.getByTestId('ext-api-saved-models-groq-1')).toHaveTextContent('model-a');
    expect(screen.getByTestId('ext-api-saved-models-groq-1')).toHaveTextContent('model-b');
  });

  it('shows a loading state while a saved connection test is in progress', async () => {
    listMock.mockResolvedValue([conn({ id: 'saved-1', endpoint: 'https://saved.example.test' })]);
    testMock.mockImplementationOnce(() => new Promise(() => undefined));
    renderSection();

    const button = await screen.findByTestId('ext-api-test-saved-saved-1');
    fireEvent.click(button);
    expect(button).toBeDisabled();
    expect(button.querySelector('.spin')).not.toBeNull();
  });

  it('renders a saved connection test error without stale models', async () => {
    listMock.mockResolvedValue([conn({ id: 'nvidia-1', endpoint: 'https://integrate.api.nvidia.com', origin_preset: 'nvidia' })]);
    testMock.mockResolvedValue({ ok: false, status: 'auth_error', models: [], hint: 'Credentials rejected' });
    renderSection();

    fireEvent.click(await screen.findByTestId('ext-api-test-saved-nvidia-1'));
    const result = await screen.findByTestId('ext-api-saved-test-result-nvidia-1');
    expect(result).toHaveAttribute('data-status', 'auth_error');
    expect(result).toHaveTextContent('Credentials rejected');
    expect(screen.queryByTestId('ext-api-saved-models-nvidia-1')).toBeNull();
  });

  it('invalidates tested models when endpoint, key, or preset changes', async () => {
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));
    fireEvent.click(screen.getByTestId('ext-api-test'));
    await waitFor(() => expect(screen.getAllByRole('option', { name: 'model-a' })).toHaveLength(3));

    fireEvent.change(screen.getByTestId('ext-api-endpoint'), { target: { value: 'https://new.example.test' } });
    expect(screen.getByTestId('ext-api-test-required')).toBeTruthy();
    expect((screen.getByTestId('ext-api-tier-default') as HTMLSelectElement).disabled).toBe(true);

    fireEvent.click(screen.getByTestId('ext-api-test'));
    await waitFor(() => expect(screen.getAllByRole('option', { name: 'model-a' })).toHaveLength(3));
    fireEvent.change(screen.getByTestId('ext-api-key'), { target: { value: 'new-key' } });
    expect(screen.getByTestId('ext-api-test-required')).toBeTruthy();

    fireEvent.click(screen.getByTestId('ext-api-test'));
    await waitFor(() => expect(screen.getAllByRole('option', { name: 'model-a' })).toHaveLength(3));
    fireEvent.click(screen.getByTestId('ext-api-preset-nvidia'));
    expect(screen.getByTestId('ext-api-test-required')).toBeTruthy();
  });

  it('clears all selected tiers after connection details change, so save cannot retain stale models', async () => {
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));
    fireEvent.change(screen.getByTestId('ext-api-display-name'), { target: { value: 'Catalogue' } });
    fireEvent.change(screen.getByTestId('ext-api-mention-alias'), { target: { value: 'catalogue' } });
    fireEvent.click(screen.getByTestId('ext-api-test'));
    await waitFor(() => expect(screen.getByTestId('ext-api-tier-default')).not.toBeDisabled());
    fireEvent.change(screen.getByTestId('ext-api-tier-economy'), { target: { value: 'model-a' } });
    fireEvent.change(screen.getByTestId('ext-api-tier-default'), { target: { value: 'model-b' } });
    fireEvent.change(screen.getByTestId('ext-api-tier-reasoning'), { target: { value: 'model-a' } });

    fireEvent.change(screen.getByTestId('ext-api-endpoint'), { target: { value: 'https://changed.example.test' } });
    expect((screen.getByTestId('ext-api-tier-economy') as HTMLSelectElement).value).toBe('');
    expect((screen.getByTestId('ext-api-tier-default') as HTMLSelectElement).value).toBe('');
    expect((screen.getByTestId('ext-api-tier-reasoning') as HTMLSelectElement).value).toBe('');
    fireEvent.click(screen.getByTestId('ext-api-save'));

    await waitFor(() => expect(createMock).toHaveBeenCalledTimes(1));
    expect(createMock).toHaveBeenCalledWith(expect.objectContaining({
      economy_model: null,
      default_model: null,
      reasoning_model: null,
    }));
  });

  it('ignores a draft response that arrives after the connection changes', async () => {
    let resolveProbe!: (result: { ok: boolean; status: 'success'; models: string[]; hint: null }) => void;
    testMock.mockImplementationOnce(() => new Promise(resolve => { resolveProbe = resolve; }));
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));
    fireEvent.click(screen.getByTestId('ext-api-test'));
    fireEvent.change(screen.getByTestId('ext-api-endpoint'), { target: { value: 'https://changed.example.test' } });
    resolveProbe({ ok: true, status: 'success', models: ['stale-model'], hint: null });

    await waitFor(() => expect(screen.getByTestId('ext-api-test-required')).toBeTruthy());
    expect(screen.queryByRole('option', { name: 'stale-model' })).toBeNull();
  });

  it('uses one synchronous guard for draft and saved test clicks', async () => {
    listMock.mockResolvedValue([
      conn({ id: 'saved-1', endpoint: 'https://saved.example.test' }),
      conn({ id: 'saved-2', endpoint: 'https://saved-two.example.test' }),
    ]);
    testMock.mockImplementationOnce(() => new Promise(() => undefined));
    renderSection();
    const first = await screen.findByTestId('ext-api-test-saved-saved-1');
    const second = screen.getByTestId('ext-api-test-saved-saved-2');
    fireEvent.click(first);
    fireEvent.click(first);
    fireEvent.click(second);
    expect(testMock).toHaveBeenCalledTimes(1);
  });

  it('ignores a saved result after editing that connection', async () => {
    let resolveProbe!: (result: { ok: boolean; status: 'success'; models: string[]; hint: null }) => void;
    listMock.mockResolvedValue([conn({ id: 'saved-1', endpoint: 'https://saved.example.test' })]);
    testMock.mockImplementationOnce(() => new Promise(resolve => { resolveProbe = resolve; }));
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-test-saved-saved-1'));
    fireEvent.click(screen.getByTestId('ext-api-edit-saved-1'));
    resolveProbe({ ok: true, status: 'success', models: ['stale-model'], hint: null });

    await waitFor(() => expect(screen.getByTestId('ext-api-form')).toBeTruthy());
    fireEvent.click(screen.getByTestId('ext-api-cancel'));
    expect(screen.queryByTestId('ext-api-saved-test-result-saved-1')).toBeNull();
  });
});
