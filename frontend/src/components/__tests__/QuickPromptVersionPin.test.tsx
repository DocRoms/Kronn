// The Quick Prompt version an Agent step loads: latest, or a pinned one.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import type { QuickPromptVersion } from '../../types/generated';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import QuickPromptVersionPin from '../QuickPromptVersionPin';
import { quickPrompts as quickPromptsApi } from '../../lib/api';

const version = (n: number, prompt: string) => ({ version_index: n, prompt_template: prompt }) as QuickPromptVersion;

beforeEach(() => { vi.clearAllMocks(); });

describe('QuickPromptVersionPin', () => {
  it('pins a version, shows it, warns when a newer one exists, and goes back to the latest', async () => {
    vi.mocked(quickPromptsApi.history).mockResolvedValue([version(2, 'Second wording'), version(1, 'First wording')]);
    const onChange = vi.fn();
    const { rerender } = render(<I18nProvider><QuickPromptVersionPin qpId="qp-1" qpName="Review" value={null} onChange={onChange} /></I18nProvider>);
    const select = await screen.findByRole('combobox');
    await waitFor(() => expect(select).not.toBeDisabled());
    expect(screen.queryByTestId('qp-version-pin-preview')).toBeNull();
    fireEvent.change(select, { target: { value: '1' } });
    expect(onChange).toHaveBeenLastCalledWith(1);

    rerender(<I18nProvider><QuickPromptVersionPin qpId="qp-1" qpName="Review" value={1} onChange={onChange} /></I18nProvider>);
    expect(screen.getByTestId('qp-version-pin-preview')).toHaveTextContent('First wording');
    expect(screen.getByTestId('qp-version-pin-behind')).toBeInTheDocument();
    fireEvent.change(screen.getByRole('combobox'), { target: { value: '' } });
    expect(onChange).toHaveBeenLastCalledWith(null);
  });
});
