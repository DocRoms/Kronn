// KT-1181 — template v2 merge UI: plan table, untracked warning, accept,
// resume after an error (KT-1169: never an error without "Reprendre"), abandon,
// and the "Lancer l'audit" gate.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within, act, renderHook } from '@testing-library/react';
import type { ReactElement } from 'react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { projects as projectsApi } from '../../lib/api';
import { TemplateMergePanel, TemplateMergeOfferSlot } from '../TemplateMergePanel';
import { useTemplateMergeOffer } from '../../hooks/useTemplateMergeOffer';
import { offersTemplateMerge } from '../../lib/templateMerge';
import type { TemplateMergeView } from '../../types/generated';

const wrap = (ui: ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);
const api = vi.mocked(projectsApi);

function plan(overrides: Partial<TemplateMergeView> = {}): TemplateMergeView {
  return {
    docs_dir: 'docs',
    base: 'local',
    base_ref: 'HEAD',
    worktree_path: null,
    branch: null,
    resumed: false,
    correspondence: [
      { source: 'docs/guide.md', section: 'Testing', start_line: 3, end_line: 9, target: 'docs/testing-quality.md', action: 'extracted' },
      { source: 'docs/guide.md', section: 'Mascot lore', start_line: 10, end_line: 14, target: null, action: 'unmapped' },
      { source: 'docs/logo.png', section: null, start_line: 0, end_line: 0, target: null, action: 'not_prose' },
      { source: 'docs/AGENTS.md', section: 'Rules', start_line: 1, end_line: 5, target: 'docs/AGENTS.md', action: 'target_exists' },
    ],
    files: [
      { path: 'docs/testing-quality.md', outcome: null, imported_sections: 1, diff: '--- /dev/null\n+++ b/docs/testing-quality.md\n@@ -0,0 +1,1 @@\n+# Testing\n' },
    ],
    existing_targets: ['docs/AGENTS.md'],
    untracked_in_checkout: [],
    table_markdown: '',
    ...overrides,
  };
}

function result(): TemplateMergeView {
  return plan({
    base_ref: 'HEAD',
    worktree_path: '/repo/.kronn/worktrees/template-v2-merge',
    branch: 'kronn/template-v2-merge',
    files: [
      { path: 'docs/glossary.md', outcome: 'created', imported_sections: 0, diff: '--- /dev/null\n+++ b/docs/glossary.md\n@@ -0,0 +1,1 @@\n+# Glossary\n' },
      { path: 'docs/testing-quality.md', outcome: 'created', imported_sections: 1, diff: '--- /dev/null\n+++ b/docs/testing-quality.md\n@@ -0,0 +1,1 @@\n+# Testing\n' },
    ],
  });
}

async function launch() {
  fireEvent.click(await screen.findByTestId('template-merge-launch'));
  await screen.findAllByTestId('template-merge-file');
}

beforeEach(() => {
  vi.clearAllMocks();
  api.templateMergePlan.mockResolvedValue(plan());
  api.templateMergeStart.mockResolvedValue(result());
  api.templateMergeAccept.mockImplementation(async (_id: string, paths: string[]) => ({
    branch: 'kronn/template-v2-merge', commit: 'abcdef1234567890', committed: paths,
  }));
  api.templateMergeDiscard.mockResolvedValue(undefined);
});

describe('TemplateMergePanel — plan', () => {
  it('renders the correspondence table with a badge per action and highlights unmapped rows', async () => {
    wrap(<TemplateMergePanel projectId="p1" />);
    const table = await screen.findByTestId('template-merge-table');
    const rows = within(table).getAllByRole('row').slice(1);
    expect(rows).toHaveLength(4);
    expect(within(rows[0]).getByText('docs/testing-quality.md')).toBeInTheDocument();
    expect(within(rows[0]).getByText('3–9')).toBeInTheDocument();
    expect(within(rows[0]).getByText('Extrait')).toBeInTheDocument();
    const unmapped = rows[1];
    expect(unmapped).toHaveClass('tm-row-unmapped');
    expect(within(unmapped).getByText('Non classé')).toBeInTheDocument();
    expect(within(unmapped).getByText(/Laissé en place/)).toBeInTheDocument();
    expect(within(rows[2]).getByText('(fichier entier)')).toBeInTheDocument();
    expect(within(rows[2]).getByText('Non textuel')).toBeInTheDocument();
    expect(within(rows[3]).getByText('Cible existante')).toBeInTheDocument();
    expect(rows.filter(r => r.classList.contains('tm-row-unmapped'))).toHaveLength(1);
    expect(screen.getByTestId('template-merge-unmapped-note')).toHaveTextContent('1 section(s) non classée(s)');
    expect(screen.queryByTestId('template-merge-untracked')).toBeNull();
  });

  it('warns before launch about docs untracked in the checkout, and rechecks', async () => {
    api.templateMergePlan.mockResolvedValueOnce(plan({ untracked_in_checkout: ['docs/draft.md', 'NOTES.md'] }));
    wrap(<TemplateMergePanel projectId="p1" />);
    const warning = await screen.findByTestId('template-merge-untracked');
    expect(warning).toHaveTextContent('2 doc(s) non suivie(s) par git');
    expect(warning).toHaveTextContent('commitez d\'abord');
    expect(within(warning).getByText('docs/draft.md')).toBeInTheDocument();
    expect(screen.getByTestId('template-merge-launch')).toBeInTheDocument();
    fireEvent.click(within(warning).getByRole('button', { name: /revérifier/ }));
    await waitFor(() => expect(api.templateMergePlan).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByTestId('template-merge-untracked')).toBeNull());
  });

  it('launches on the remote base when chosen', async () => {
    wrap(<TemplateMergePanel projectId="p1" />);
    fireEvent.click(await screen.findByRole('radio', { name: /Distante/ }));
    await launch();
    expect(api.templateMergeStart).toHaveBeenCalledWith('p1', 'remote');
  });
});

describe('TemplateMergePanel — result', () => {
  it('shows one diff per file and accepts only the selected paths', async () => {
    wrap(<TemplateMergePanel projectId="p1" />);
    await launch();
    expect(api.templateMergeStart).toHaveBeenCalledWith('p1', 'local');
    const files = screen.getAllByTestId('template-merge-file');
    expect(files).toHaveLength(2);
    expect(within(files[0]).getByText('# Glossary')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('checkbox', { name: 'Accepter docs/glossary.md' }));
    const acceptSelected = screen.getByTestId('template-merge-accept-selected');
    expect(acceptSelected).toHaveTextContent('(1)');
    fireEvent.click(acceptSelected);
    await screen.findByTestId('template-merge-accepted');
    expect(api.templateMergeAccept).toHaveBeenCalledTimes(1);
    expect(api.templateMergeAccept).toHaveBeenCalledWith('p1', ['docs/testing-quality.md']);
    expect(screen.getByTestId('template-merge-accepted')).toHaveTextContent('1 fichier(s) commité(s) sur kronn/template-v2-merge (abcdef12)');
    const left = screen.getAllByTestId('template-merge-file');
    expect(left).toHaveLength(1);
    expect(within(left[0]).getByText('docs/glossary.md')).toBeInTheDocument();
  });

  it('accepts every file with "Tout accepter"', async () => {
    wrap(<TemplateMergePanel projectId="p1" />);
    await launch();
    fireEvent.click(screen.getByTestId('template-merge-accept-all'));
    await screen.findByTestId('template-merge-accepted');
    expect(api.templateMergeAccept).toHaveBeenCalledWith('p1', ['docs/glossary.md', 'docs/testing-quality.md']);
    expect(screen.getByText('Plus aucun fichier à valider.')).toBeInTheDocument();
  });

  it('resumes after a failed launch with "Reprendre", showing what to fix', async () => {
    api.templateMergeStart.mockRejectedValueOnce(new Error('the merge worktree /w has edited existing docs (docs/guide.md): commit or revert them, then resume'));
    wrap(<TemplateMergePanel projectId="p1" />);
    fireEvent.click(await screen.findByTestId('template-merge-launch'));
    const error = await screen.findByTestId('template-merge-error');
    expect(error).toHaveTextContent('edited existing docs (docs/guide.md)');
    expect(error).toHaveTextContent('la fusion repart du même worktree');
    fireEvent.click(within(error).getByRole('button', { name: 'Reprendre' }));
    await screen.findAllByTestId('template-merge-file');
    expect(api.templateMergeStart).toHaveBeenCalledTimes(2);
    expect(screen.queryByTestId('template-merge-error')).toBeNull();
  });

  it('every failing operation offers "Reprendre" that replays it (plan, accept)', async () => {
    api.templateMergePlan.mockRejectedValueOnce(new Error('Project path not found: /gone'));
    wrap(<TemplateMergePanel projectId="p1" />);
    let error = await screen.findByTestId('template-merge-error');
    fireEvent.click(within(error).getByRole('button', { name: 'Reprendre' }));
    await launch();
    api.templateMergeAccept.mockRejectedValueOnce(new Error('only files proposed by the merge can be accepted: x'));
    fireEvent.click(screen.getByTestId('template-merge-accept-all'));
    error = await screen.findByTestId('template-merge-error');
    expect(error).toHaveTextContent('only files proposed by the merge');
    fireEvent.click(within(error).getByRole('button', { name: 'Reprendre' }));
    await screen.findByTestId('template-merge-accepted');
    expect(api.templateMergeAccept).toHaveBeenCalledTimes(2);
    expect(api.templateMergeAccept.mock.calls[1]).toEqual(api.templateMergeAccept.mock.calls[0]);
  });

  it('resumes the merge from the result view', async () => {
    wrap(<TemplateMergePanel projectId="p1" />);
    await launch();
    fireEvent.click(screen.getByTestId('template-merge-resume'));
    await waitFor(() => expect(api.templateMergeStart).toHaveBeenCalledTimes(2));
  });

  it('abandons only after confirmation, with DELETE', async () => {
    wrap(<TemplateMergePanel projectId="p1" />);
    await launch();
    fireEvent.click(screen.getByTestId('template-merge-discard'));
    expect(api.templateMergeDiscard).not.toHaveBeenCalled();
    fireEvent.click(screen.getByTestId('template-merge-discard-confirm'));
    await screen.findByTestId('template-merge-discarded');
    expect(api.templateMergeDiscard).toHaveBeenCalledWith('p1');
  });

  it('a failed abandon keeps "Reprendre"', async () => {
    api.templateMergeDiscard.mockRejectedValueOnce(new Error('worktree busy'));
    wrap(<TemplateMergePanel projectId="p1" />);
    await launch();
    fireEvent.click(screen.getByTestId('template-merge-discard'));
    fireEvent.click(screen.getByTestId('template-merge-discard-confirm'));
    const error = await screen.findByTestId('template-merge-error');
    fireEvent.click(within(error).getByRole('button', { name: 'Reprendre' }));
    await screen.findByTestId('template-merge-discarded');
    expect(api.templateMergeDiscard).toHaveBeenCalledTimes(2);
  });
});

describe('audit entry point', () => {
  it('offersTemplateMerge: real extraction from docs, or untracked docs; not a lone README', () => {
    expect(offersTemplateMerge(plan())).toBe(true);
    expect(offersTemplateMerge(null)).toBe(false);
    expect(offersTemplateMerge(plan({ files: [] }))).toBe(false);
    expect(offersTemplateMerge(plan({
      correspondence: [{ source: 'README.md', section: 'Tests', start_line: 1, end_line: 4, target: 'docs/testing-quality.md', action: 'extracted' }],
    }))).toBe(false);
    expect(offersTemplateMerge(plan({ files: [], correspondence: [], untracked_in_checkout: ['docs/a.md'] }))).toBe(true);
  });

  it('"Lancer l\'audit" on a documented, never-merged repo opens the merge first', async () => {
    const audit = vi.fn();
    const { result: hook } = renderHook(() => useTemplateMergeOffer('p1', 'TemplateInstalled', true));
    await waitFor(() => expect(hook.current.eligible).toBe(true));
    await act(() => hook.current.gateLaunch(audit));
    expect(audit).not.toHaveBeenCalled();
    expect(hook.current.mode).toBe('gate');
    act(() => hook.current.launchAuditAnyway());
    expect(audit).toHaveBeenCalledTimes(1);
    expect(hook.current.mode).toBe('closed');
    await act(() => hook.current.gateLaunch(audit));
    expect(audit).toHaveBeenCalledTimes(2);
  });

  it('launches straight away when nothing would be merged, or the repo is already audited', async () => {
    const audit = vi.fn();
    api.templateMergePlan.mockResolvedValue(plan({ files: [] }));
    const { result: hook } = renderHook(() => useTemplateMergeOffer('p1', 'TemplateInstalled', true));
    await act(() => hook.current.gateLaunch(audit));
    expect(audit).toHaveBeenCalledTimes(1);
    const audited = renderHook(() => useTemplateMergeOffer('p2', 'Audited', true));
    await act(() => audited.result.current.gateLaunch(audit));
    expect(audit).toHaveBeenCalledTimes(2);
    expect(api.templateMergePlan).toHaveBeenCalledTimes(1);
  });

  it('a plan read failure never blocks the audit', async () => {
    const audit = vi.fn();
    api.templateMergePlan.mockRejectedValue(new Error('boom'));
    const { result: hook } = renderHook(() => useTemplateMergeOffer('p1', 'NoTemplate', true));
    await act(() => hook.current.gateLaunch(audit));
    expect(audit).toHaveBeenCalledTimes(1);
  });

  it('the slot shows the gate panel with "Lancer l\'audit sans fusionner"', async () => {
    const launchAuditAnyway = vi.fn();
    wrap(
      <TemplateMergeOfferSlot
        projectId="p1"
        offer={{ eligible: true, mode: 'gate', open: vi.fn(), close: vi.fn(), gateLaunch: vi.fn(), launchAuditAnyway }}
      />,
    );
    const gate = await screen.findByTestId('template-merge-gate');
    fireEvent.click(within(gate).getByRole('button', { name: "Lancer l'audit sans fusionner" }));
    expect(launchAuditAnyway).toHaveBeenCalledTimes(1);
    await screen.findByTestId('template-merge-table');
  });

  it('the slot offers the plan as a banner when eligible and closed', () => {
    const open = vi.fn();
    wrap(
      <TemplateMergeOfferSlot
        projectId="p1"
        offer={{ eligible: true, mode: 'closed', open, close: vi.fn(), gateLaunch: vi.fn(), launchAuditAnyway: vi.fn() }}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Voir le plan de fusion' }));
    expect(open).toHaveBeenCalledTimes(1);
  });
});
