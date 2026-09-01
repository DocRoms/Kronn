// ExternalApiSection — unified "External API" settings zone (KT-339).
//
// Pins the three behaviours the task's Definition of Done cares about:
//   1. one zone with a preset selector that pre-fills the endpoint;
//   2. several connections coexist, each with its own endpoint + tiers;
//   3. compatible services share the same generic form, including the
//      first-class OpenRouter preset and arbitrary OpenAI-compatible endpoints.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup, within } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { ExternalApiConnectionView } from '../../../lib/api';

const { listMock, createMock, updateMock, revealMock, removeMock, testMock } = vi.hoisted(() => ({
  listMock: vi.fn(),
  createMock: vi.fn(),
  updateMock: vi.fn(),
  revealMock: vi.fn(),
  removeMock: vi.fn(),
  testMock: vi.fn(),
}));

vi.mock('../../../lib/api', () =>
  buildApiMock({
    externalApi: {
      list: listMock as never,
      create: createMock as never,
      update: updateMock as never,
      reveal: revealMock as never,
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

function renderSection(onModelTiersChanged?: () => void) {
  const toast = vi.fn();
  render(
    <ExternalApiSection
      t={t}
      toast={toast}
      onModelTiersChanged={onModelTiersChanged}
    />,
  );
  return { toast };
}

function chooseTier(testId: string, model: string) {
  const input = screen.getByTestId(testId);
  fireEvent.focus(input);
  fireEvent.change(input, { target: { value: model } });
  fireEvent.click(screen.getByRole('option', { name: model }));
}

function expectTierOption(testId: string, model: string) {
  const input = screen.getByTestId(testId);
  fireEvent.focus(input);
  expect(screen.getByRole('option', { name: model })).toBeInTheDocument();
  fireEvent.keyDown(input, { key: 'Escape' });
}

beforeEach(() => {
  listMock.mockResolvedValue([]);
  createMock.mockResolvedValue(conn({}));
  updateMock.mockResolvedValue(conn({}));
  revealMock.mockResolvedValue('sk-stored-secret');
  removeMock.mockResolvedValue(null);
  testMock.mockResolvedValue({
    ok: true,
    status: 'success',
    models: ['model-a', 'model-b'],
    catalog: [
      { id: 'model-a', display_name: 'Model A', capabilities: ['chat'] },
      { id: 'model-b', display_name: 'Model B', capabilities: ['chat'] },
      { id: 'image-a', display_name: 'Image A', capabilities: ['image'] },
      { id: 'video-a', display_name: 'Video A', capabilities: ['video'] },
    ],
    hint: null,
  });
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
        economy_model: 'nvidia/nano',
        default_model: 'nvidia/super',
        reasoning_model: 'nvidia/ultra',
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

    const nvidiaCard = screen.getByRole('group', { name: 'NVIDIA' });
    expect(within(nvidiaCard).getByText('@nvidia')).toBeInTheDocument();
    expect(within(nvidiaCard).getByText('config.extApi.keyConfigured')).toBeInTheDocument();
    const tierGroup = within(nvidiaCard).getByRole('group', { name: 'disc.modelTier' });
    expect(within(tierGroup).getByText('nvidia/nano')).toBeInTheDocument();
    expect(within(tierGroup).getByText('nvidia/super')).toBeInTheDocument();
    expect(within(tierGroup).getByText('nvidia/ultra')).toBeInTheDocument();
    expect(within(nvidiaCard).getByText('https://integrate.api.nvidia.com').closest('.set-ext-api-conn-endpoint-row'))
      .toBeInTheDocument();
    expect(screen.getByTestId('ext-api-add-connection').querySelector('.set-ext-api-add-icon'))
      .toBeInTheDocument();
  });

  it('pre-fills the endpoint from the chosen preset (DoD 1)', async () => {
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));

    const endpoint = () => screen.getByTestId('ext-api-endpoint') as HTMLInputElement;
    // The default preset already seeds an endpoint on open.
    expect(endpoint().value).toBe('http://localhost:4000');

    fireEvent.click(screen.getByTestId('ext-api-preset-nvidia'));
    expect(endpoint().value).toBe('https://integrate.api.nvidia.com');

    fireEvent.click(screen.getByTestId('ext-api-preset-open_router'));
    expect(endpoint().value).toBe('https://openrouter.ai/api/v1');
    expect(screen.getByTestId('ext-api-display-name')).toHaveValue('OpenRouter');
    expect(screen.getByTestId('ext-api-mention-alias')).toHaveValue('openrouter');

    fireEvent.click(screen.getByTestId('ext-api-preset-lite_llm'));
    expect(endpoint().value).toBe('http://localhost:4000');

    // "Other" clears it so a brand-new service starts from a blank endpoint.
    fireEvent.click(screen.getByTestId('ext-api-preset-other'));
    expect(endpoint().value).toBe('');
  });

  it('serializes the LiteLLM preset with the backend wire value', async () => {
    const onModelTiersChanged = vi.fn();
    renderSection(onModelTiersChanged);
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));
    fireEvent.change(screen.getByTestId('ext-api-display-name'), { target: { value: 'LiteLLM local' } });
    fireEvent.change(screen.getByTestId('ext-api-mention-alias'), { target: { value: 'litellm' } });
    fireEvent.click(screen.getByTestId('ext-api-save'));

    await waitFor(() => expect(createMock).toHaveBeenCalledTimes(1));
    expect(createMock).toHaveBeenCalledWith(expect.objectContaining({
      endpoint: 'http://localhost:4000',
      origin_preset: 'lite_llm',
    }));
    expect(onModelTiersChanged).toHaveBeenCalledTimes(1);
  });

  it('loads GLM 5.3 from OpenRouter, assigns it to standard/reasoning, and saves the preset', async () => {
    testMock.mockResolvedValue({
      ok: true,
      status: 'success',
      models: ['openai/gpt-4o', 'z-ai/glm-5.3'],
      hint: null,
    });
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));
    fireEvent.click(screen.getByTestId('ext-api-preset-open_router'));
    expect(screen.getByTestId('ext-api-save')).toBeDisabled();

    fireEvent.change(screen.getByTestId('ext-api-key'), { target: { value: 'sk-or-v1-openrouter-test' } });
    fireEvent.click(screen.getByTestId('ext-api-test'));

    await waitFor(() => expect(testMock).toHaveBeenCalledWith({
      endpoint: 'https://openrouter.ai/api/v1',
      api_key: 'sk-or-v1-openrouter-test',
      origin_preset: 'open_router',
    }));
    await waitFor(() => {
      expect(screen.getByTestId('ext-api-tier-default')).toHaveValue('z-ai/glm-5.3');
      expect(screen.getByTestId('ext-api-tier-reasoning')).toHaveValue('z-ai/glm-5.3');
    });

    fireEvent.click(screen.getByTestId('ext-api-save'));
    await waitFor(() => expect(createMock).toHaveBeenCalledWith(expect.objectContaining({
      display_name: 'OpenRouter',
      mention_alias: 'openrouter',
      endpoint: 'https://openrouter.ai/api/v1',
      origin_preset: 'open_router',
      default_model: 'z-ai/glm-5.3',
      reasoning_model: 'z-ai/glm-5.3',
      api_key: 'sk-or-v1-openrouter-test',
    })));
  });

  it('explains and blocks an OpenRouter key pasted without its prefix', async () => {
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));
    fireEvent.click(screen.getByTestId('ext-api-preset-open_router'));
    fireEvent.change(screen.getByTestId('ext-api-key'), {
      target: { value: 'a'.repeat(64) },
    });

    expect(screen.getByTestId('ext-api-openrouter-key-format'))
      .toHaveTextContent('config.extApi.openRouterKeyFormat');
    expect(screen.getByTestId('ext-api-save')).toBeDisabled();
  });

  it('restores the hosted NVIDIA endpoint for a legacy connection that stored none', async () => {
    listMock.mockResolvedValue([
      conn({
        id: 'external-api-nvidia',
        display_name: 'NVIDIA',
        mention_alias: 'nvidia',
        origin_preset: 'nvidia',
        endpoint: null,
      }),
    ]);
    renderSection();

    const card = await screen.findByRole('group', { name: 'NVIDIA' });
    expect(within(card).getByText('https://integrate.api.nvidia.com')).toBeInTheDocument();
    const testButton = within(card).getByTestId('ext-api-test-saved-external-api-nvidia');
    expect(testButton).not.toBeDisabled();
    expect(testButton.querySelector('.lucide-plug-zap')).not.toBeNull();
    fireEvent.click(testButton);
    await waitFor(() => expect(testMock).toHaveBeenCalledWith({
      endpoint: 'https://integrate.api.nvidia.com',
      api_key: null,
      connection_id: 'external-api-nvidia',
      origin_preset: 'nvidia',
    }));

    fireEvent.click(within(card).getByTestId('ext-api-edit-external-api-nvidia'));
    expect(screen.getByTestId('ext-api-endpoint')).toHaveValue('https://integrate.api.nvidia.com');
  });

  it('adds a third compatible service from the UI via the generic Other preset (DoD 3)', async () => {
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));

    // The same three presets and the same generic form — no per-service card.
    expect(screen.getByTestId('ext-api-preset-lite_llm')).toBeTruthy();
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
    chooseTier('ext-api-tier-default', 'model-a');

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
      // Media slots travel with every save, empty or not. `media_endpoint` is
      // an advanced override the form does not expose, so it is not sent.
      image_model: null,
      video_model: null,
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
    const keyInput = screen.getByTestId('ext-api-key') as HTMLInputElement;
    expect(keyInput).toHaveValue('••••••••');
    expect(keyInput.readOnly).toBe(true);
    expect(screen.getByTestId('ext-api-key-stored')).toHaveTextContent('config.extApi.keyStoredHint');

    const keyControls = keyInput.parentElement;
    expect(keyControls).not.toBeNull();
    fireEvent.click(within(keyControls as HTMLElement).getByLabelText('mcp.show'));
    await waitFor(() => expect(revealMock).toHaveBeenCalledTimes(1));
    expect(screen.getByTestId('ext-api-key')).toHaveValue('sk-stored-secret');

    fireEvent.change(screen.getByTestId('ext-api-display-name'), { target: { value: 'Groq Prod' } });
    fireEvent.click(screen.getByTestId('ext-api-save'));

    await waitFor(() => expect(updateMock).toHaveBeenCalledTimes(1));
    // api_key null = "keep the stored credential" (the field was never touched).
    expect(updateMock).toHaveBeenCalledWith('groq-1', expect.objectContaining({
      display_name: 'Groq Prod',
      api_key: null,
    }));
  });

  it('replaces a stored key explicitly and can reveal the newly typed value', async () => {
    listMock.mockResolvedValue([conn({
      id: 'groq-1',
      display_name: 'Groq',
      mention_alias: 'groq',
      endpoint: 'https://api.groq.com/openai/v1',
      has_credential: true,
    })]);
    renderSection();

    fireEvent.click(await screen.findByTestId('ext-api-edit-groq-1'));
    fireEvent.click(screen.getByText('mcp.custom.replaceValue'));
    const keyInput = screen.getByTestId('ext-api-key') as HTMLInputElement;
    expect(keyInput.readOnly).toBe(false);
    expect(screen.getByTestId('ext-api-save')).toBeDisabled();
    fireEvent.change(keyInput, { target: { value: 'sk-replacement' } });
    fireEvent.click(within(keyInput.parentElement as HTMLElement).getByLabelText('mcp.show'));
    expect((screen.getByTestId('ext-api-key') as HTMLInputElement).type).toBe('text');
    fireEvent.click(screen.getByTestId('ext-api-save'));

    await waitFor(() => expect(updateMock).toHaveBeenCalledTimes(1));
    expect(updateMock).toHaveBeenCalledWith('groq-1', expect.objectContaining({
      api_key: 'sk-replacement',
    }));
  });

  it('loads tested models into every tier selector without saving the draft', async () => {
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));
    expect(screen.getByTestId('ext-api-test-required')).toBeTruthy();
    expect((screen.getByTestId('ext-api-tier-default') as HTMLSelectElement).disabled).toBe(true);

    fireEvent.click(screen.getByTestId('ext-api-test'));
    await waitFor(() => expect(testMock).toHaveBeenCalledWith({
      endpoint: 'http://localhost:4000',
      api_key: null,
      origin_preset: 'lite_llm',
    }));
    expect(createMock).not.toHaveBeenCalled();
    expect((screen.getByTestId('ext-api-tier-default') as HTMLSelectElement).disabled).toBe(false);
    for (const tier of ['economy', 'default', 'reasoning']) {
      expectTierOption(`ext-api-tier-${tier}`, 'model-a');
    }
  });

  it('keeps model selectors locked when a failed test still returns catalogue entries', async () => {
    testMock.mockResolvedValue({
      ok: false,
      status: 'http_error',
      models: ['model-a'],
      hint: 'Connection failed',
    });
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));
    fireEvent.click(screen.getByTestId('ext-api-test'));

    await screen.findByText('Connection failed');
    expect(screen.getByTestId('ext-api-tier-default')).toBeDisabled();
    expect(screen.getByText('config.extApi.modelsLocked')).toBeInTheDocument();
  });

  it('tests every unique NVIDIA tier model instead of the first catalogue entry', async () => {
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));
    fireEvent.click(screen.getByTestId('ext-api-preset-nvidia'));
    fireEvent.change(screen.getByTestId('ext-api-display-name'), { target: { value: 'NVIDIA' } });
    fireEvent.change(screen.getByTestId('ext-api-mention-alias'), { target: { value: 'nvidia' } });
    expect(screen.getByTestId('ext-api-save')).toBeDisabled();
    fireEvent.change(screen.getByTestId('ext-api-key'), { target: { value: 'nvapi-test' } });

    fireEvent.click(screen.getByTestId('ext-api-test'));
    await waitFor(() => expect(testMock).toHaveBeenCalledWith({
      endpoint: 'https://integrate.api.nvidia.com',
      api_key: 'nvapi-test',
      origin_preset: 'nvidia',
    }));
    await waitFor(() => expect(screen.getByTestId('ext-api-test')).not.toBeDisabled());
    chooseTier('ext-api-tier-economy', 'model-a');
    chooseTier('ext-api-tier-default', 'model-b');
    chooseTier('ext-api-tier-reasoning', 'model-a');
    fireEvent.click(screen.getByTestId('ext-api-test'));

    await waitFor(() => expect(testMock).toHaveBeenLastCalledWith({
      endpoint: 'https://integrate.api.nvidia.com',
      api_key: 'nvapi-test',
      origin_preset: 'nvidia',
      models: ['model-a', 'model-b'],
    }));
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
    await waitFor(() => expect(screen.getByTestId('ext-api-tier-default')).not.toBeDisabled());
    expectTierOption('ext-api-tier-default', 'model-a');

    fireEvent.change(screen.getByTestId('ext-api-endpoint'), { target: { value: 'https://new.example.test' } });
    expect(screen.getByTestId('ext-api-test-required')).toBeTruthy();
    expect((screen.getByTestId('ext-api-tier-default') as HTMLSelectElement).disabled).toBe(true);

    fireEvent.click(screen.getByTestId('ext-api-test'));
    await waitFor(() => expect(screen.getByTestId('ext-api-tier-default')).not.toBeDisabled());
    expectTierOption('ext-api-tier-default', 'model-a');
    fireEvent.change(screen.getByTestId('ext-api-key'), { target: { value: 'new-key' } });
    expect(screen.getByTestId('ext-api-test-required')).toBeTruthy();

    fireEvent.click(screen.getByTestId('ext-api-test'));
    await waitFor(() => expect(screen.getByTestId('ext-api-tier-default')).not.toBeDisabled());
    expectTierOption('ext-api-tier-default', 'model-a');
    fireEvent.click(screen.getByTestId('ext-api-preset-nvidia'));
    expect(screen.getByTestId('ext-api-test-required')).toBeTruthy();
  });

  it('preserves previous tiers as locked values until the changed connection passes a new test', async () => {
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));
    fireEvent.change(screen.getByTestId('ext-api-display-name'), { target: { value: 'Catalogue' } });
    fireEvent.change(screen.getByTestId('ext-api-mention-alias'), { target: { value: 'catalogue' } });
    fireEvent.click(screen.getByTestId('ext-api-test'));
    await waitFor(() => expect(screen.getByTestId('ext-api-tier-default')).not.toBeDisabled());
    chooseTier('ext-api-tier-economy', 'model-a');
    chooseTier('ext-api-tier-default', 'model-b');
    chooseTier('ext-api-tier-reasoning', 'model-a');

    fireEvent.change(screen.getByTestId('ext-api-endpoint'), { target: { value: 'https://changed.example.test' } });
    expect(screen.getByTestId('ext-api-tier-economy')).toHaveValue('model-a');
    expect(screen.getByTestId('ext-api-tier-default')).toHaveValue('model-b');
    expect(screen.getByTestId('ext-api-tier-reasoning')).toHaveValue('model-a');
    expect(screen.getByTestId('ext-api-tier-default')).toBeDisabled();

    fireEvent.click(screen.getByTestId('ext-api-test'));
    await waitFor(() => expect(testMock).toHaveBeenLastCalledWith({
      endpoint: 'https://changed.example.test',
      api_key: null,
      origin_preset: 'lite_llm',
    }));
    await waitFor(() => expect(screen.getByTestId('ext-api-tier-default')).not.toBeDisabled());
    fireEvent.click(screen.getByTestId('ext-api-save'));

    await waitFor(() => expect(createMock).toHaveBeenCalledTimes(1));
    expect(createMock).toHaveBeenCalledWith(expect.objectContaining({
      economy_model: 'model-a',
      default_model: 'model-b',
      reasoning_model: 'model-a',
    }));
  });

  it('immediately starts a replacement draft test after invalidation and ignores the old response', async () => {
    let resolveProbe!: (result: { ok: boolean; status: 'success'; models: string[]; hint: null }) => void;
    testMock.mockImplementationOnce(() => new Promise(resolve => { resolveProbe = resolve; }));
    testMock.mockResolvedValueOnce({ ok: true, status: 'success', models: ['new-model'], hint: null });
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));
    fireEvent.click(screen.getByTestId('ext-api-test'));
    fireEvent.change(screen.getByTestId('ext-api-endpoint'), { target: { value: 'https://changed.example.test' } });

    // The original request is still pending, but invalidation releases the
    // visual and imperative guards so testing the changed endpoint is possible.
    await waitFor(() => expect(screen.getByTestId('ext-api-test')).not.toBeDisabled());
    fireEvent.click(screen.getByTestId('ext-api-test'));
    await waitFor(() => expect(testMock).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByTestId('ext-api-tier-default')).not.toBeDisabled());
    for (const tier of ['economy', 'default', 'reasoning']) {
      expectTierOption(`ext-api-tier-${tier}`, 'new-model');
    }

    resolveProbe({ ok: true, status: 'success', models: ['stale-model'], hint: null });
    expect(screen.queryByRole('option', { name: 'stale-model' })).toBeNull();
    expect((screen.getByTestId('ext-api-tier-default') as HTMLSelectElement).disabled).toBe(false);
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

  it('uses the tested searchable catalogue for image and video models', async () => {
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));
    await waitFor(() => expect(screen.getByTestId('ext-api-media-panel')).toBeTruthy());

    const video = screen.getByTestId('ext-api-media-video') as HTMLInputElement;
    const image = screen.getByTestId('ext-api-media-image') as HTMLInputElement;
    // Exactly like the text tiers: no unverified catalogue can be selected.
    expect(video.value).toBe('');
    expect(image.value).toBe('');
    expect(video).toBeDisabled();
    expect(image).toBeDisabled();

    fireEvent.click(screen.getByTestId('ext-api-test'));
    await waitFor(() => expect(video).not.toBeDisabled());
    expectTierOption('ext-api-media-image', 'Image A');
    const videoPicker = screen.getByTestId('ext-api-media-video');
    fireEvent.focus(videoPicker);
    fireEvent.change(videoPicker, { target: { value: 'Video A' } });
    fireEvent.click(screen.getByRole('option', { name: 'Video A' }));

    expect((screen.getByTestId('ext-api-media-video') as HTMLInputElement).value).toBe('Video A');
    fireEvent.focus(screen.getByTestId('ext-api-media-image'));
    expect(screen.queryByRole('option', { name: 'model-a' })).toBeNull();
  });

  it('keeps a saved but undetected media model visible as unavailable', async () => {
    listMock.mockResolvedValue([conn({
      id: 'saved-media',
      endpoint: 'https://openrouter.ai/api',
      origin_preset: 'open_router',
      has_credential: true,
      video_model: 'retired/video-model',
    })]);
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-edit-saved-media'));
    const video = screen.getByTestId('ext-api-media-video');
    expect(video).toHaveValue('retired/video-model');

    fireEvent.click(screen.getByTestId('ext-api-test'));
    await waitFor(() => expect(video).not.toBeDisabled());
    fireEvent.focus(video);
    expect(screen.getByRole('option', { name: 'retired/video-model' })).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getByRole('option', { name: 'Video A' })).toBeInTheDocument();
    expect(screen.queryByRole('option', { name: 'Image A' })).toBeNull();
  });

  it('keeps the media block separate from the three text tiers', async () => {
    // Modalities are not quality levels: mixing them into the tier list would
    // suggest a text step could pick "tier Image".
    renderSection();
    fireEvent.click(await screen.findByTestId('ext-api-add-connection'));
    await waitFor(() => expect(screen.getByTestId('ext-api-media-panel')).toBeTruthy());

    const mediaPanel = screen.getByTestId('ext-api-media-panel');
    expect(mediaPanel.querySelector('[data-testid="ext-api-tier-economy"]')).toBeNull();
    expect(mediaPanel.querySelector('[data-testid="ext-api-tier-reasoning"]')).toBeNull();
    expect(mediaPanel.querySelector('[data-testid="ext-api-media-video"]')).toBeTruthy();
  });

});
