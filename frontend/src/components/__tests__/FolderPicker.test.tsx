// Note: assertions use French strings because the default UI locale is 'fr'.
import { describe, it, expect, vi, afterEach } from 'vitest';
import { act, render, screen, cleanup, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', () => ({
  setup: { browse: vi.fn() },
  config: {
    getUiLanguage: vi.fn().mockResolvedValue('fr'),
    saveUiLanguage: vi.fn().mockResolvedValue(undefined),
  },
}));

const isTauriRuntime = vi.fn().mockReturnValue(false);
const invokeTauri = vi.fn();
vi.mock('../../lib/tauri', () => ({
  isTauriRuntime: () => isTauriRuntime(),
  invokeTauri: (...args: unknown[]) => invokeTauri(...args),
}));

import { setup as setupApi } from '../../lib/api';
import { FolderPicker } from '../setup/FolderPicker';
import type { BrowseListing } from '../../types/generated';

const listing = (overrides: Partial<BrowseListing> = {}): BrowseListing => ({
  path: '/workspace/git',
  parent: '/workspace',
  roots: [{ label: 'workspace', path: '/workspace' }],
  entries: [
    { name: 'agaches', path: '/workspace/git/agaches', is_repository: false },
    { name: 'depot', path: '/workspace/git/depot', is_repository: true },
  ],
  truncated: false,
  ...overrides,
});

const paint = (props: Partial<React.ComponentProps<typeof FolderPicker>> = {}) =>
  render(
    <I18nProvider>
      <FolderPicker selected={[]} onConfirm={vi.fn()} onClose={vi.fn()} {...props} />
    </I18nProvider>
  );

afterEach(() => { cleanup(); vi.clearAllMocks(); isTauriRuntime.mockReturnValue(false); });

describe('FolderPicker', () => {
  it('can select a mounted repository root without selecting one of its children', async () => {
    vi.mocked(setupApi.browse).mockResolvedValue(listing({ path: '/workspace', parent: null, entries: [] }));
    const onConfirm = vi.fn();
    paint({ onConfirm });
    await userEvent.click(await screen.findByRole('checkbox', { name: 'Sélectionner ce dossier' }));
    await userEvent.click(screen.getByText('Ajouter et scanner'));
    expect(onConfirm).toHaveBeenCalledWith(['/workspace']);
  });

  it.each(['success', 'error'])('keeps the last requested folder when an older request ends with %s', async staleResult => {
    let resolveFirst!: (value: BrowseListing) => void;
    let rejectFirst!: (reason: Error) => void;
    let resolveSecond!: (value: BrowseListing) => void;
    const first = new Promise<BrowseListing>((resolve, reject) => { resolveFirst = resolve; rejectFirst = reject; });
    const second = new Promise<BrowseListing>(resolve => { resolveSecond = resolve; });
    const roots = [{ label: 'First', path: '/first' }, { label: 'Second', path: '/second' }];
    vi.mocked(setupApi.browse).mockImplementation(path => path === '/first' ? first : path === '/second' ? second : Promise.resolve(listing({ roots })));
    paint();
    await userEvent.click(await screen.findByRole('button', { name: 'First' }));
    await userEvent.click(screen.getByRole('button', { name: 'Second' }));
    await act(async () => { resolveSecond(listing({ path: '/second', roots, entries: [] })); });
    expect(screen.getByText('/second')).toBeTruthy();
    await act(async () => {
      if (staleResult === 'success') resolveFirst(listing({ path: '/first', roots }));
      else rejectFirst(new Error('stale access error'));
    });
    expect(screen.getByText('/second')).toBeTruthy();
    expect(screen.queryByText('/first')).toBeNull();
    expect(screen.queryByText('stale access error')).toBeNull();
  });

  it('asks the server what it can reach, and shows which entries are repositories', async () => {
    vi.mocked(setupApi.browse).mockResolvedValue(listing());
    paint();

    expect(await screen.findByText('agaches')).toBeTruthy();
    expect(screen.getByText('depot')).toBeTruthy();
    // Only the repository carries the badge.
    expect(screen.getAllByText('dépôt')).toHaveLength(1);
    expect(setupApi.browse).toHaveBeenCalledWith();
  });

  /// Select multiple folders as supported by config.scan.paths.
  it('confirms every folder that was ticked, not just the last one', async () => {
    vi.mocked(setupApi.browse).mockResolvedValue(listing());
    const onConfirm = vi.fn();
    paint({ onConfirm });

    await screen.findByText('agaches');
    await userEvent.click(screen.getByRole('checkbox', { name: 'Sélectionner agaches' }));
    await userEvent.click(screen.getByRole('checkbox', { name: 'Sélectionner depot' }));
    await userEvent.click(screen.getByText('Ajouter et scanner'));

    expect(onConfirm).toHaveBeenCalledWith([
      '/workspace/git/agaches',
      '/workspace/git/depot',
    ]);
  });

  it('cannot confirm nothing', async () => {
    vi.mocked(setupApi.browse).mockResolvedValue(listing());
    paint();
    await screen.findByText('agaches');
    expect(screen.getByText('Ajouter et scanner').closest('button')?.disabled).toBe(true);
  });

  it('opens a folder when its name is clicked', async () => {
    vi.mocked(setupApi.browse).mockResolvedValue(listing());
    paint();
    await screen.findByText('agaches');
    await userEvent.click(screen.getByText('agaches'));
    await waitFor(() =>
      expect(setupApi.browse).toHaveBeenCalledWith('/workspace/git/agaches')
    );
  });

  /// A refusal from the server is shown as is: "outside the folders Kronn can
  /// explore" is exactly what the user needs to read.
  it('shows the server refusal instead of an empty list', async () => {
    vi.mocked(setupApi.browse).mockRejectedValue(new Error('`/etc` est hors des dossiers que Kronn peut explorer'));
    paint();
    expect(await screen.findByText(/hors des dossiers/)).toBeTruthy();
  });

  it('offers the native window only on the desktop build', async () => {
    vi.mocked(setupApi.browse).mockResolvedValue(listing());
    paint();
    await screen.findByText('agaches');
    expect(screen.queryByText('Ouvrir une fenêtre native')).toBeNull();

    cleanup();
    isTauriRuntime.mockReturnValue(true);
    invokeTauri.mockResolvedValue(['/Volumes/travail']);
    const onConfirm = vi.fn();
    paint({ onConfirm });
    await screen.findByText('agaches');
    await userEvent.click(screen.getByText('Ouvrir une fenêtre native'));
    await waitFor(() => expect(onConfirm).toHaveBeenCalledWith(['/Volumes/travail']));
  });

  /// Dismissing the native window chooses nothing: the modal stays where it
  /// was rather than confirming an empty selection.
  it('confirms nothing when the native window is dismissed', async () => {
    vi.mocked(setupApi.browse).mockResolvedValue(listing());
    isTauriRuntime.mockReturnValue(true);
    const onConfirm = vi.fn();
    const onClose = vi.fn();
    paint({ onConfirm, onClose });

    await screen.findByText('agaches');
    for (const rendu of [null, []]) {
      invokeTauri.mockResolvedValue(rendu);
      await userEvent.click(screen.getByText('Ouvrir une fenêtre native'));
    }
    expect(onConfirm).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByText('agaches')).toBeTruthy();
  });
});
