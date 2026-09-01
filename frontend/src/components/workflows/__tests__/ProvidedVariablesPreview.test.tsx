import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import { I18nProvider } from '../../../lib/I18nContext';
import type { PromptVariable } from '../../../types/generated';

const mocks = vi.hoisted(() => ({
  preview: vi.fn(),
  reveal: vi.fn(),
}));

vi.mock('../../../lib/api', () => buildApiMock({
  executionVariables: {
    preview: mocks.preview,
    reveal: mocks.reveal,
  },
}));

import { ProvidedVariablesPreview } from '../ProvidedVariablesPreview';

const variable: PromptVariable = {
  name: 'api_key',
  label: 'API key',
  placeholder: '',
  description: null,
  required: true,
  source: 'project_env',
  source_ref: '<env.API_KEY>',
  allow_manual_override: true,
};

const preview = {
  run_kind: 'preview' as const,
  run_id: 'preview-1',
  metadata: {
    id: 'snapshot-1',
    resolved_at: '2026-09-01T08:00:00Z',
    expires_at: '2026-09-01T08:10:00Z',
    purged: false,
    provenance: [{
      name: 'api_key',
      source: 'project_env' as const,
      source_ref: '<env.API_KEY>',
      effective_source_ref: '<env.API_KEY>',
      overridden: false,
    }],
  },
};

const renderPreview = (onValueChange = vi.fn()) => render(
  <I18nProvider>
    <ProvidedVariablesPreview
      variables={[variable]}
      projectId="project-1"
      values={{}}
      onValueChange={onValueChange}
    />
  </I18nProvider>,
);

beforeEach(() => {
  mocks.preview.mockReset().mockResolvedValue(preview);
  mocks.reveal.mockReset().mockResolvedValue('secret-value');
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('ProvidedVariablesPreview', () => {
  it('previews the selected project without exposing the resolved value', async () => {
    const { container } = renderPreview();

    await waitFor(() => expect(mocks.preview).toHaveBeenCalledWith('project-1', [variable]));
    expect(screen.getByDisplayValue('••••••')).toBeInTheDocument();
    expect(screen.queryByText('secret-value')).not.toBeInTheDocument();
    expect(container).toHaveTextContent('<env.API_KEY>');
  });

  it('reveals only on demand and remasks from the same control', async () => {
    const { container } = renderPreview();
    await waitFor(() => expect(mocks.preview).toHaveBeenCalled());
    const revealButton = container.querySelector<HTMLButtonElement>('.wf-icon-btn')!;

    fireEvent.click(revealButton);
    await waitFor(() => expect(mocks.reveal).toHaveBeenCalledWith('preview', 'preview-1', 'api_key'));
    expect(screen.getByDisplayValue('secret-value')).toBeInTheDocument();

    fireEvent.click(revealButton);
    expect(screen.getByDisplayValue('••••••')).toBeInTheDocument();
    expect(screen.queryByDisplayValue('secret-value')).not.toBeInTheDocument();
  });

  it('enables an explicit manual override and can return to the project value', async () => {
    const onValueChange = vi.fn();
    const { container } = renderPreview(onValueChange);
    await waitFor(() => expect(mocks.preview).toHaveBeenCalled());
    const overrideButton = container.querySelector<HTMLButtonElement>('.wf-small-btn')!;

    fireEvent.click(overrideButton);
    expect(onValueChange).toHaveBeenLastCalledWith('api_key', '');
    const input = container.querySelector<HTMLInputElement>('input:not([readonly])')!;
    fireEvent.change(input, { target: { value: 'manual-value' } });
    expect(onValueChange).toHaveBeenLastCalledWith('api_key', 'manual-value');

    fireEvent.click(overrideButton);
    expect(onValueChange).toHaveBeenLastCalledWith('api_key', undefined);
    expect(screen.getByDisplayValue('••••••')).toBeInTheDocument();
  });

  it('shows a generic error without leaking the backend failure text', async () => {
    mocks.preview.mockRejectedValue(new Error('backend leaked secret-value'));
    renderPreview();

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(/wf\.launchPreviewUnavailable|Valeur indisponible|Value unavailable/);
    expect(screen.queryByText(/backend leaked secret-value/)).not.toBeInTheDocument();
  });
});
