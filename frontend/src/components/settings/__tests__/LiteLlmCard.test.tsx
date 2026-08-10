import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';

const { healthMock, modelsMock, getModelTiersMock, modelFailuresMock, forgetModelFailureMock, retryModelMock } = vi.hoisted(() => ({
  healthMock: vi.fn(),
  modelsMock: vi.fn(),
  getModelTiersMock: vi.fn(),
  modelFailuresMock: vi.fn(),
  forgetModelFailureMock: vi.fn(),
  retryModelMock: vi.fn(),
}));

vi.mock('../../../lib/api', () => buildApiMock({
  liteLlm: {
    health: healthMock as never,
    models: modelsMock as never,
    modelFailures: modelFailuresMock as never,
    forgetModelFailure: forgetModelFailureMock as never,
    retryModel: retryModelMock as never,
  },
  config: {
    getModelTiers: getModelTiersMock as never,
  },
}));

import { LiteLlmCard } from '../LiteLlmCard';

const t = (key: string) => key;
const emptyTier = { economy: null, default: null, reasoning: null };

beforeEach(() => {
  healthMock.mockReset();
  modelsMock.mockReset();
  getModelTiersMock.mockReset();
  modelFailuresMock.mockReset();
  modelFailuresMock.mockResolvedValue({ failures: [] });
  forgetModelFailureMock.mockReset();
  forgetModelFailureMock.mockResolvedValue(true);
  retryModelMock.mockReset();
  retryModelMock.mockResolvedValue({ healthy: true, failure: null });
  getModelTiersMock.mockResolvedValue({
    claude_code: { ...emptyTier },
    codex: { ...emptyTier },
    gemini_cli: { ...emptyTier },
    kiro: { ...emptyTier },
    vibe: { ...emptyTier },
    copilot_cli: { ...emptyTier },
    ollama: { ...emptyTier },
    lite_llm: { ...emptyTier },
  });
});

afterEach(() => {
  cleanup();
});

describe('LiteLlmCard initial snapshot', () => {
  it('loads a healthy proxy and exposes its declared models', async () => {
    healthMock.mockResolvedValue({
      status: 'online',
      endpoint: 'http://localhost:4000',
      models_count: 1,
      hint: null,
      configured: true,
    });
    modelsMock.mockResolvedValue({
      models: [{ id: 'local-fast', backing_model: 'qwen3:8b', provider: 'ollama' }],
    });

    render(<LiteLlmCard t={t} />);

    await waitFor(() => expect(screen.getByText('http://localhost:4000')).toBeTruthy());
    expect(screen.getAllByRole('option', { name: 'local-fast → qwen3:8b' })).toHaveLength(3);
    expect(document.querySelector(
      '[data-model-tier-agent="LiteLlm"][data-model-tier="default"]',
    )).toBeTruthy();
    expect(healthMock).toHaveBeenCalledTimes(1);
    expect(modelsMock).toHaveBeenCalledTimes(1);
  });

  it('shows an unknown status instead of pretending an API failure means unconfigured', async () => {
    healthMock.mockRejectedValue(new Error('proxy unavailable'));

    render(<LiteLlmCard t={t} />);

    await waitFor(() => expect(screen.getByRole('alert'))
      .toHaveTextContent('liteLlm.loadErrorTitle'));
    expect(screen.getByText('liteLlm.stateUnavailable')).toBeTruthy();
    expect(screen.queryByText('liteLlm.connectTitle')).toBeNull();
    expect(modelsMock).not.toHaveBeenCalled();
  });

  it('preserves the last known configuration when a refresh request fails', async () => {
    healthMock
      .mockResolvedValueOnce({
        status: 'online', endpoint: 'https://litellm.corp.example',
        models_count: 1, hint: null, configured: true,
      })
      .mockRejectedValueOnce(new Error('backend restarting'));
    modelsMock.mockResolvedValue({
      models: [{ id: 'corp-default', backing_model: null, provider: 'openai' }],
    });

    render(<LiteLlmCard t={t} />);
    await waitFor(() => expect(screen.getByText('https://litellm.corp.example')).toBeTruthy());

    fireEvent.click(screen.getByRole('button', { name: 'liteLlm.refresh' }));

    await waitFor(() => expect(screen.getByRole('alert'))
      .toHaveTextContent('liteLlm.loadErrorTitle'));
    expect(screen.getByText('https://litellm.corp.example')).toBeTruthy();
    expect(screen.getAllByRole('option', { name: 'corp-default' })).toHaveLength(3);
    expect(screen.queryByText('liteLlm.connectTitle')).toBeNull();
  });

  it('keeps assigned tiers visible when only the model catalogue request fails', async () => {
    healthMock.mockResolvedValue({
      status: 'online', endpoint: 'https://litellm.corp.example',
      models_count: 1, hint: null, configured: true,
    });
    modelsMock.mockRejectedValue(new Error('catalogue request interrupted'));
    getModelTiersMock.mockResolvedValue({
      claude_code: { ...emptyTier }, codex: { ...emptyTier },
      gemini_cli: { ...emptyTier }, kiro: { ...emptyTier }, vibe: { ...emptyTier },
      copilot_cli: { ...emptyTier }, ollama: { ...emptyTier },
      lite_llm: { economy: null, default: 'corp-default', reasoning: null },
    });

    render(<LiteLlmCard t={t} />);

    await waitFor(() => expect(screen.getByRole('alert'))
      .toHaveTextContent('liteLlm.catalogueUnavailableTitle'));
    expect(screen.getByText('https://litellm.corp.example')).toBeTruthy();
    expect(screen.getAllByRole('option', { name: 'corp-default' })).toHaveLength(3);
    expect(screen.getByLabelText('disc.tier.default')).toBeDisabled();
    expect(screen.queryByText('liteLlm.connectTitle')).toBeNull();
    expect(screen.queryByText('liteLlm.noModelsTitle')).toBeNull();
  });

  it('keeps a saved configuration visible when the proxy is temporarily offline', async () => {
    healthMock.mockResolvedValue({
      status: 'offline',
      endpoint: 'https://litellm.corp.example',
      models_count: 0,
      hint: 'VPN required',
      configured: true,
    });
    getModelTiersMock.mockResolvedValue({
      claude_code: { ...emptyTier },
      codex: { ...emptyTier },
      gemini_cli: { ...emptyTier },
      kiro: { ...emptyTier },
      vibe: { ...emptyTier },
      copilot_cli: { ...emptyTier },
      ollama: { ...emptyTier },
      lite_llm: {
        economy: 'local-fast',
        default: 'corp-default',
        reasoning: 'corp-reasoning',
      },
    });

    render(<LiteLlmCard t={t} />);

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('liteLlm.savedUnavailableDesc'));
    expect(screen.getByRole('alert')).toHaveTextContent('liteLlm.savedUnavailablePreserved');
    expect(screen.getByText('liteLlm.offline')).toBeTruthy();
    expect(screen.getByText('https://litellm.corp.example')).toBeTruthy();
    expect(screen.queryByText('liteLlm.connectTitle')).toBeNull();
    expect(screen.queryByLabelText('liteLlm.keyLabel')).toBeNull();
    expect(screen.getAllByRole('option', { name: 'corp-default' })).toHaveLength(3);
    expect(screen.getByLabelText('disc.tier.default')).toBeDisabled();
    expect(modelsMock).not.toHaveBeenCalled();
  });

  it('restores the live catalogue after retrying an offline saved proxy', async () => {
    healthMock
      .mockResolvedValueOnce({
        status: 'offline', endpoint: 'https://litellm.corp.example',
        models_count: 0, hint: 'VPN required', configured: true,
      })
      .mockResolvedValueOnce({
        status: 'online', endpoint: 'https://litellm.corp.example',
        models_count: 1, hint: null, configured: true,
      });
    modelsMock.mockResolvedValue({
      models: [{ id: 'model-back-online', backing_model: null, provider: 'openai' }],
    });

    render(<LiteLlmCard t={t} />);
    await waitFor(() => expect(screen.getByRole('alert')).toBeTruthy());

    fireEvent.click(screen.getByRole('button', { name: 'liteLlm.retryConnection' }));

    await waitFor(() => expect(screen.queryByRole('alert')).toBeNull());
    expect(screen.getAllByRole('option', { name: 'model-back-online' })).toHaveLength(3);
    expect(modelsMock).toHaveBeenCalledTimes(1);
  });

  it('uses the backing model to append an observed cost suffix', async () => {
    healthMock.mockResolvedValue({
      status: 'online',
      endpoint: 'http://localhost:4000',
      models_count: 1,
      hint: null,
      configured: true,
    });
    modelsMock.mockResolvedValue({
      models: [{ id: 'local-fast', backing_model: 'qwen3:8b', provider: 'ollama' }],
    });
    const modelCostSuffix = vi.fn(() => ' · ≈ $0.00/M observed');

    render(<LiteLlmCard t={t} modelCostSuffix={modelCostSuffix} />);

    await waitFor(() => expect(screen.getByText('http://localhost:4000')).toBeTruthy());
    expect(modelCostSuffix).toHaveBeenCalledWith('qwen3:8b');
    expect(screen.getAllByRole('option', {
      name: 'local-fast → qwen3:8b · ≈ $0.00/M observed',
    })).toHaveLength(3);
  });

  it('shows remembered model failures, marks their options and clears one after a healthy retry', async () => {
    healthMock.mockResolvedValue({
      status: 'online', endpoint: 'http://localhost:4000',
      models_count: 2, hint: null, configured: true,
    });
    modelsMock.mockResolvedValue({
      models: [
        { id: 'broken-model', backing_model: null, provider: null },
        { id: 'healthy-model', backing_model: null, provider: null },
      ],
    });
    modelFailuresMock.mockResolvedValue({
      failures: [{
        model: 'broken-model',
        status_code: 404,
        error_message: JSON.stringify({
          error: {
            message: 'litellm.NotFoundError: {"error":{"message":"Publisher model was not found in this region"}}',
          },
        }),
        first_failed_at: '2026-08-10 08:00:00',
        last_failed_at: '2026-08-10 09:00:00',
        failure_count: 2,
      }],
    });

    render(<LiteLlmCard t={t} />);

    await waitFor(() => expect(screen.getByTestId('litellm-model-failures')).toBeTruthy());
    expect(screen.getByTestId('litellm-model-failures')).toHaveTextContent('HTTP 404');
    expect(screen.getByTestId('litellm-model-failures')).toHaveTextContent('Publisher model was not found');
    expect(screen.getByRole('table', { name: 'liteLlm.failuresTitle' })).toBeTruthy();
    expect(screen.getAllByRole('columnheader')).toHaveLength(4);
    expect(screen.getAllByRole('cell')).toHaveLength(4);
    expect(screen.getAllByRole('option', { name: /broken-model.*HTTP 404/ })).toHaveLength(3);
    expect(screen.getAllByRole('option', { name: /broken-model.*HTTP 404/ })[0]).toBeDisabled();

    fireEvent.click(screen.getByRole('button', { name: 'liteLlm.failureRetry' }));
    await waitFor(() => expect(retryModelMock).toHaveBeenCalledWith('broken-model'));
    await waitFor(() => expect(screen.queryByTestId('litellm-model-failures')).toBeNull());
    expect(screen.getByRole('status')).toHaveTextContent('liteLlm.failureRecovered');
  });

  it('refreshes a remembered failure when retry still fails', async () => {
    healthMock.mockResolvedValue({
      status: 'online', endpoint: 'http://localhost:4000',
      models_count: 1, hint: null, configured: true,
    });
    modelsMock.mockResolvedValue({
      models: [{ id: 'broken-model', backing_model: null, provider: null }],
    });
    const initial = {
      model: 'broken-model', status_code: 404, error_message: 'old error',
      first_failed_at: '2026-08-10 08:00:00', last_failed_at: '2026-08-10 09:00:00',
      failure_count: 1,
    };
    modelFailuresMock.mockResolvedValue({ failures: [initial] });
    retryModelMock.mockResolvedValue({
      healthy: false,
      failure: { ...initial, status_code: 422, error_message: 'new error', failure_count: 2 },
    });

    render(<LiteLlmCard t={t} />);
    await waitFor(() => expect(screen.getByTestId('litellm-model-failures')).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: 'liteLlm.failureRetry' }));

    await waitFor(() => expect(screen.getByTestId('litellm-model-failures')).toHaveTextContent('HTTP 422'));
    expect(screen.getByText(/new error/)).toBeTruthy();
    expect(screen.getByRole('status')).toHaveTextContent('liteLlm.failureStillDown');
  });

  it('labels a model removed from the LiteLLM catalogue without showing a new HTTP error', async () => {
    healthMock.mockResolvedValue({
      status: 'online', endpoint: 'http://localhost:4000',
      models_count: 1, hint: null, configured: true,
    });
    modelsMock.mockResolvedValue({
      models: [{ id: 'replacement-model', backing_model: null, provider: null }],
    });
    const removedFailure = {
      model: 'disabled-model', status_code: 410,
      error_message: 'kronn:model-not-in-catalogue',
      first_failed_at: '2026-08-10 08:00:00', last_failed_at: '2026-08-10 09:00:00',
      failure_count: 2,
    };
    modelFailuresMock.mockResolvedValue({ failures: [{
      ...removedFailure, status_code: 404, error_message: 'old upstream error',
    }] });
    retryModelMock.mockResolvedValue({ healthy: false, failure: removedFailure });

    render(<LiteLlmCard t={t} />);
    await waitFor(() => expect(screen.getByTestId('litellm-model-failures')).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: 'liteLlm.failureRetry' }));

    await waitFor(() => expect(screen.getByTestId('litellm-model-failures'))
      .toHaveTextContent('liteLlm.failureRemoved'));
    expect(screen.getByTestId('litellm-model-failures')).not.toHaveTextContent('HTTP 410');
    expect(screen.getByRole('status')).toHaveTextContent('liteLlm.failureRemovedNotice');
  });

  it('lets the user forget a stale model failure without probing the model', async () => {
    healthMock.mockResolvedValue({
      status: 'online', endpoint: 'http://localhost:4000',
      models_count: 1, hint: null, configured: true,
    });
    modelsMock.mockResolvedValue({
      models: [{ id: 'replacement-model', backing_model: null, provider: null }],
    });
    modelFailuresMock.mockResolvedValue({ failures: [{
      model: 'removed-model', status_code: 410,
      error_message: 'kronn:model-not-in-catalogue',
      first_failed_at: '2026-08-10 08:00:00', last_failed_at: '2026-08-10 09:00:00',
      failure_count: 3,
    }] });

    render(<LiteLlmCard t={t} />);
    await waitFor(() => expect(screen.getByTestId('litellm-model-failures')).toBeTruthy());
    fireEvent.click(screen.getByRole('button', {
      name: 'liteLlm.failureForget',
    }));

    await waitFor(() => expect(forgetModelFailureMock).toHaveBeenCalledWith('removed-model'));
    expect(retryModelMock).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByTestId('litellm-model-failures')).toBeNull());
    expect(screen.getByRole('status')).toHaveTextContent('liteLlm.failureForgotten');
  });
});
