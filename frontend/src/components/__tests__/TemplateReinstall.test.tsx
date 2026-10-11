// KT-1166 — "Recréer les fichiers du template": the button only asks, the
// confirmation lists what is replaced, nothing runs before the confirm, and
// every failure keeps a "Reprendre" (KT-1169).
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import type { ReactElement } from 'react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { projects as projectsApi } from '../../lib/api';
import { TemplateReinstall } from '../TemplateReinstall';
import type { TemplateReinstallPlan, TemplateReinstallReport } from '../../types/generated';

const wrap = (ui: ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);
const api = vi.mocked(projectsApi);

function plan(overrides: Partial<TemplateReinstallPlan> = {}): TemplateReinstallPlan {
  return {
    docs_dir: 'docs',
    audit_running: false,
    files: [
      { path: 'docs/AGENTS.md', action: 'replace', kind: 'damaged_or_template', human_sections: ['team'], human_validated: false },
      { path: 'docs/glossary.md', action: 'replace', kind: 'damaged_or_template', human_sections: [], human_validated: false },
      { path: 'docs/testing-quality.md', action: 'replace', kind: 'filled', human_sections: [], human_validated: true },
      { path: 'docs/repo-map.md', action: 'create', kind: null, human_sections: [], human_validated: false },
      { path: 'docs/decisions.md', action: 'unchanged', kind: null, human_sections: [], human_validated: false },
    ],
    ...overrides,
  };
}

const REPORT: TemplateReinstallReport = {
  backup_dir: '.kronn/audit/template-backups/20261011T120000Z',
  replaced: [
    { path: 'docs/AGENTS.md', backup: '.kronn/audit/template-backups/20261011T120000Z/docs/AGENTS.md' },
    { path: 'docs/glossary.md', backup: '.kronn/audit/template-backups/20261011T120000Z/docs/glossary.md' },
  ],
  created: ['docs/repo-map.md'],
  unchanged: ['docs/decisions.md'],
  to_reconfirm: ['docs/AGENTS.md'],
  skipped: ['docs/testing-quality.md'],
};

beforeEach(() => {
  vi.clearAllMocks();
  api.templateReinstallPlan.mockResolvedValue(plan());
  api.templateReinstall.mockResolvedValue(REPORT);
});

async function openConfirmation() {
  wrap(<TemplateReinstall projectId="p1" />);
  fireEvent.click(screen.getByTestId('template-reinstall-open'));
  return screen.findByTestId('template-reinstall-confirm');
}

describe('TemplateReinstall', () => {
  it('shows only the button at first and reads nothing from the server', () => {
    wrap(<TemplateReinstall projectId="p1" />);
    expect(screen.getByRole('button', { name: 'Recréer les fichiers du template' })).toBeTruthy();
    expect(api.templateReinstallPlan).not.toHaveBeenCalled();
    expect(api.templateReinstall).not.toHaveBeenCalled();
  });

  it('the confirmation lists the files to be replaced, and nothing runs until it is confirmed', async () => {
    const dialog = await openConfirmation();
    expect(api.templateReinstallPlan).toHaveBeenCalledWith('p1');
    const list = within(dialog).getByTestId('template-reinstall-replace-list');
    expect(within(list).getByText('docs/AGENTS.md')).toBeTruthy();
    expect(within(list).getByText('docs/glossary.md')).toBeTruthy();
    // Created and unchanged files are not "replaced".
    expect(within(list).queryByText('docs/repo-map.md')).toBeNull();
    expect(within(list).queryByText('docs/decisions.md')).toBeNull();
    expect(within(list).getByText(/team/)).toBeTruthy();
    expect(within(dialog).getByTestId('template-reinstall-create-count')).toBeTruthy();
    expect(api.templateReinstall).not.toHaveBeenCalled();
  });

  it('lists audited content apart, unchecked by default, with the warning', async () => {
    const dialog = await openConfirmation();
    const filled = within(dialog).getByTestId('template-reinstall-filled-list');
    const box = within(filled).getByRole('checkbox', { name: 'docs/testing-quality.md' }) as HTMLInputElement;
    expect(box.checked).toBe(false);
    expect(within(dialog).getByTestId('template-reinstall-filled-warning').textContent).toMatch(/Contenu d'audit/);
    const damaged = within(dialog).getByTestId('template-reinstall-replace-list');
    for (const box of within(damaged).getAllByRole('checkbox') as HTMLInputElement[]) expect(box.checked).toBe(true);
    // The filled file is not among the damaged ones.
    expect(within(damaged).queryByText('docs/testing-quality.md')).toBeNull();
  });

  it('sends a filled file only once it is ticked, and can drop a damaged one', async () => {
    const dialog = await openConfirmation();
    fireEvent.click(within(dialog).getByRole('checkbox', { name: 'docs/testing-quality.md' }));
    fireEvent.click(within(dialog).getByRole('checkbox', { name: 'docs/glossary.md' }));
    fireEvent.click(screen.getByTestId('template-reinstall-confirm-button'));
    await screen.findByTestId('template-reinstall-done');
    expect(api.templateReinstall).toHaveBeenCalledWith('p1', ['docs/AGENTS.md', 'docs/testing-quality.md']);
  });

  it('announces, without blocking, the human-validated content a reinstall rewrites (KT-1079)', async () => {
    const dialog = await openConfirmation();
    const notices = within(dialog).getAllByTestId('template-reinstall-notice').map(n => n.textContent);
    expect(notices.some(n => n?.includes('team') && n.includes('reconfirmer'))).toBe(true);
    expect(notices.some(n => n?.includes('validée par un humain'))).toBe(true);
    const confirm = within(dialog).getByTestId('template-reinstall-confirm-button') as HTMLButtonElement;
    expect(confirm.disabled).toBe(false);
    fireEvent.click(confirm);
    const done = await screen.findByTestId('template-reinstall-done');
    expect(within(done).getByTestId('template-reinstall-reconfirm').textContent).toContain('docs/AGENTS.md');
  });

  it('cancelling runs nothing and gives the button back', async () => {
    await openConfirmation();
    fireEvent.click(screen.getByRole('button', { name: 'Annuler' }));
    expect(api.templateReinstall).not.toHaveBeenCalled();
    expect(screen.getByTestId('template-reinstall-open')).toBeTruthy();
  });

  it('confirming reinstalls and reports what was replaced and where the backups are', async () => {
    await openConfirmation();
    fireEvent.click(screen.getByTestId('template-reinstall-confirm-button'));
    const done = await screen.findByTestId('template-reinstall-done');
    expect(api.templateReinstall).toHaveBeenCalledTimes(1);
    expect(api.templateReinstall).toHaveBeenCalledWith('p1', ['docs/AGENTS.md', 'docs/glossary.md']);
    expect(within(done).getByText('.kronn/audit/template-backups/20261011T120000Z')).toBeTruthy();
    const replaced = within(done).getByTestId('template-reinstall-replaced');
    expect(within(replaced).getAllByRole('listitem')).toHaveLength(2);
    expect(within(replaced).getByText('.kronn/audit/template-backups/20261011T120000Z/docs/AGENTS.md')).toBeTruthy();
  });

  it('cannot be confirmed while an audit runs', async () => {
    api.templateReinstallPlan.mockResolvedValue(plan({ audit_running: true }));
    const dialog = await openConfirmation();
    expect(within(dialog).getByTestId('template-reinstall-audit-running')).toBeTruthy();
    const confirm = within(dialog).getByTestId('template-reinstall-confirm-button') as HTMLButtonElement;
    expect(confirm.disabled).toBe(true);
    fireEvent.click(confirm);
    expect(api.templateReinstall).not.toHaveBeenCalled();
  });

  it('offers no confirmation when every file already matches the template', async () => {
    api.templateReinstallPlan.mockResolvedValue(plan({
      files: [{ path: 'docs/AGENTS.md', action: 'unchanged', kind: null, human_sections: [], human_validated: false }],
    }));
    const dialog = await openConfirmation();
    expect(within(dialog).getByTestId('template-reinstall-uptodate')).toBeTruthy();
    expect(within(dialog).queryByTestId('template-reinstall-confirm-button')).toBeNull();
  });

  it('a failed plan offers "Reprendre", which reads the plan again', async () => {
    api.templateReinstallPlan.mockRejectedValueOnce(new Error('Project path not found'));
    wrap(<TemplateReinstall projectId="p1" />);
    fireEvent.click(screen.getByTestId('template-reinstall-open'));
    const error = await screen.findByTestId('template-reinstall-error');
    expect(within(error).getByText('Project path not found')).toBeTruthy();
    fireEvent.click(within(error).getByRole('button', { name: 'Reprendre' }));
    await screen.findByTestId('template-reinstall-confirm');
    expect(api.templateReinstallPlan).toHaveBeenCalledTimes(2);
  });

  it('a failed reinstall offers "Reprendre", which replays the reinstall', async () => {
    api.templateReinstall.mockRejectedValueOnce(new Error('An audit is running on this project'));
    await openConfirmation();
    fireEvent.click(screen.getByTestId('template-reinstall-confirm-button'));
    const error = await screen.findByTestId('template-reinstall-error');
    expect(within(error).getByText('An audit is running on this project')).toBeTruthy();
    fireEvent.click(within(error).getByRole('button', { name: 'Reprendre' }));
    await screen.findByTestId('template-reinstall-done');
    await waitFor(() => expect(api.templateReinstall).toHaveBeenCalledTimes(2));
  });
});
