// KT-1684 — "Detect repositories": a checklist preview, only the kept ones are
// added, and "Hide" keeps one from coming back.

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';
import type { DetectedRepo } from '../../types/generated';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { projects as projectsApi } from '../../lib/api';
import { DetectRepositoriesDialog } from '../DetectRepositoriesDialog';

const repo = (name: string, extra: Partial<DetectedRepo> = {}): DetectedRepo => ({
  path: `/home/me/${name}`, name, remote_url: null, branch: 'main', ai_configs: [], has_project: false, hidden: false, ...extra,
});

async function openDialog() {
  const onClose = vi.fn();
  const onAdded = vi.fn();
  const toast = vi.fn();
  await act(async () => {
    render(<I18nProvider><DetectRepositoriesDialog onClose={onClose} onAdded={onAdded} toast={toast} /></I18nProvider>);
  });
  return { onClose, onAdded, toast };
}

beforeEach(() => {
  vi.mocked(projectsApi.scan).mockResolvedValue([
    repo('alpha'), repo('beta'), repo('gamma'),
    repo('already', { has_project: true }),
    repo('dotted', { hidden: true }),
  ]);
  vi.mocked(projectsApi.create).mockResolvedValue({} as never);
  vi.mocked(projectsApi.hideScanned).mockResolvedValue(undefined as never);
});
afterEach(() => { cleanup(); vi.clearAllMocks(); });

describe('Detect repositories', () => {
  it('lists the repositories found, all checked, without the ones already projects or hidden', async () => {
    await openDialog();
    const rows = screen.getAllByTestId('detect-repos-row');
    expect(rows.map(r => within(r).getByRole('checkbox').getAttribute('aria-label'))).toEqual(['alpha', 'beta', 'gamma']);
    for (const row of rows) expect(within(row).getByRole('checkbox')).toBeChecked();
    expect(screen.getByTestId('detect-repos-add')).toHaveTextContent('Ajouter 3 sélectionné(s)');
    // Nothing was created by looking.
    expect(projectsApi.create).not.toHaveBeenCalled();
  });

  it('adds only the repositories that stay checked', async () => {
    const { onAdded, onClose, toast } = await openDialog();
    fireEvent.click(screen.getByRole('checkbox', { name: 'beta' }));
    expect(screen.getByTestId('detect-repos-add')).toHaveTextContent('Ajouter 2 sélectionné(s)');
    await act(async () => { fireEvent.click(screen.getByTestId('detect-repos-add')); });

    expect(vi.mocked(projectsApi.create).mock.calls.map(([r]) => r.name)).toEqual(['alpha', 'gamma']);
    expect(toast).toHaveBeenCalledWith('2 projet(s) ajouté(s)', 'success');
    expect(onAdded).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('adds nothing when nothing is checked', async () => {
    await openDialog();
    for (const name of ['alpha', 'beta', 'gamma']) fireEvent.click(screen.getByRole('checkbox', { name }));
    expect(screen.getByTestId('detect-repos-add')).toBeDisabled();
  });

  it('hides a repository for good: it leaves the list and the call is made with its path', async () => {
    await openDialog();
    const row = screen.getAllByTestId('detect-repos-row')[0];
    await act(async () => { fireEvent.click(within(row).getByTestId('detect-repos-hide')); });
    expect(projectsApi.hideScanned).toHaveBeenCalledWith('/home/me/alpha');
    expect(screen.queryByRole('checkbox', { name: 'alpha' })).toBeNull();
    expect(screen.getByTestId('detect-repos-add')).toHaveTextContent('Ajouter 2 sélectionné(s)');

    await act(async () => { fireEvent.click(screen.getByTestId('detect-repos-add')); });
    expect(vi.mocked(projectsApi.create).mock.calls.map(([r]) => r.name)).toEqual(['beta', 'gamma']);
  });

  it('says when there is nothing new, and when the scan fails', async () => {
    vi.mocked(projectsApi.scan).mockResolvedValue([repo('already', { has_project: true })]);
    await openDialog();
    expect(screen.getByTestId('detect-repos-empty')).toHaveTextContent('Aucun nouveau projet détecté');
    cleanup();

    vi.mocked(projectsApi.scan).mockRejectedValue(new Error('Scan failed: no access'));
    await openDialog();
    expect(screen.getByRole('alert')).toHaveTextContent(/Scan failed: no access/);
  });

  it('refreshes the list when a creation fails midway, and keeps the dialog open to retry', async () => {
    vi.mocked(projectsApi.create).mockResolvedValueOnce({} as never).mockRejectedValueOnce(new Error('A project with this path already exists'));
    const { onAdded, onClose, toast } = await openDialog();
    await act(async () => { fireEvent.click(screen.getByTestId('detect-repos-add')); });
    expect(toast).toHaveBeenCalledWith('A project with this path already exists', 'error');
    expect(onAdded).toHaveBeenCalledTimes(1);
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByTestId('detect-repos-add')).not.toBeDisabled();
  });
});
