import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';
import { ApiRequestError } from '../../lib/apiRequestError';

const { getMock, saveMock } = vi.hoisted(() => ({ getMock: vi.fn(), saveMock: vi.fn() }));

vi.mock('../../lib/api', () => buildApiMock({
  config: { getDefaultDocsDir: getMock as never, saveDefaultDocsDir: saveMock as never },
}));

import { DefaultDocsDirSetting } from '../DefaultDocsDirSetting';

const t = (key: string) => key;

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('DefaultDocsDirSetting (KT-263)', () => {
  it('loads, saves the normalized value, and clears with an empty field', async () => {
    getMock.mockResolvedValue(null);
    saveMock.mockResolvedValueOnce('handbook').mockResolvedValueOnce(null);
    render(<DefaultDocsDirSetting t={t} />);
    const input = await screen.findByTestId('default-docs-dir-input');
    await waitFor(() => expect(input).not.toBeDisabled());

    fireEvent.change(input, { target: { value: './handbook/' } });
    fireEvent.click(screen.getByTestId('default-docs-dir-save'));
    await waitFor(() => expect(input).toHaveValue('handbook'));
    expect(saveMock).toHaveBeenCalledWith('./handbook/');

    fireEvent.change(input, { target: { value: '  ' } });
    fireEvent.click(screen.getByTestId('default-docs-dir-save'));
    await waitFor(() => expect(saveMock).toHaveBeenLastCalledWith(null));
  });

  it('shows the 400 message and keeps the field editable', async () => {
    getMock.mockResolvedValue('docs');
    saveMock.mockRejectedValue(new ApiRequestError('`/abs` is absolute', 'validation'));
    render(<DefaultDocsDirSetting t={t} />);
    const input = await screen.findByTestId('default-docs-dir-input');
    await waitFor(() => expect(input).toHaveValue('docs'));
    fireEvent.change(input, { target: { value: '/abs' } });
    fireEvent.click(screen.getByTestId('default-docs-dir-save'));
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('absolute'));
    expect(input).not.toBeDisabled();
  });
});
