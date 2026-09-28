import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { listing, resource, skill } from './repositoryResourceFixtures';

// vitest blanks CSS modules (`css: false`), so read the stylesheets as text.
const stylesheet = (name: string) => readFileSync(join(import.meta.dirname, '..', name), 'utf-8');
const panelCss = stylesheet('ProjectRepositoryResourcesPanel.css');
const sheetsCss = stylesheet('RepositoryResourceSheets.css');

const repositoryResources = vi.hoisted(() => vi.fn());
const publishRepositoryResource = vi.hoisted(() => vi.fn());
const importRepositoryResource = vi.hoisted(() => vi.fn());
const approveRepositoryResource = vi.hoisted(() => vi.fn());
const useNativeSkill = vi.hoisted(() => vi.fn());
const copyNativeSkill = vi.hoisted(() => vi.fn());
const setDefaultSkills = vi.hoisted(() => vi.fn());
const quickExecsList = vi.hoisted(() => vi.fn());
const quickExecsDelete = vi.hoisted(() => vi.fn());

vi.mock('../../lib/api', () => ({
  projects: {
    repositoryResources,
    publishRepositoryResource,
    importRepositoryResource,
    approveRepositoryResource,
    useNativeSkill,
    copyNativeSkill,
    setDefaultSkills,
  },
  quickExecs: { list: quickExecsList, delete: quickExecsDelete },
  quickApis: { list: vi.fn(), delete: vi.fn() },
  quickPrompts: { list: vi.fn(), delete: vi.fn() },
  workflows: { get: vi.fn(), delete: vi.fn() },
}));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    // "key" alone, or "key:arg|arg" so a test can see what a sentence was given.
    t: (key: string, ...args: Array<string | number>) => (args.length > 0 ? `${key}:${args.join('|')}` : key),
  }),
}));

import { ProjectRepositoryResourcesPanel } from '../ProjectRepositoryResourcesPanel';

const R = 'projects.repositoryResources.';
const rowOf = (name: string) => screen.getByRole('button', { name }).closest('[role="row"]') as HTMLElement;
const actionIn = (row: HTMLElement, action: string) => within(row).getByRole('button', { name: `${R}action.${action}` });
const openTab = (tab: string) => fireEvent.click(screen.getByRole('tab', { name: new RegExp(`tab\\.${tab}`) }));

async function show(data = listing(), props: Partial<Parameters<typeof ProjectRepositoryResourcesPanel>[0]> = {}) {
  repositoryResources.mockResolvedValue(data);
  const view = render(<ProjectRepositoryResourcesPanel projectId="project-1" {...props} />);
  await screen.findByRole('tablist');
  return view;
}

describe('ProjectRepositoryResourcesPanel', () => {
  beforeEach(() => {
    [repositoryResources, publishRepositoryResource, importRepositoryResource, approveRepositoryResource,
      useNativeSkill, copyNativeSkill, setDefaultSkills, quickExecsList, quickExecsDelete].forEach(mock => mock.mockReset());
    publishRepositoryResource.mockResolvedValue({});
    importRepositoryResource.mockResolvedValue({});
    localStorage.removeItem('kronn:projectRepositoryResourcesTab');
  });

  describe('skill folders and kronn/ notices', () => {
    it('lists native skill folders before a secondary kronn/ notice', async () => {
      await show(listing({
        kronn_exists: false,
        skill_roots: [
          { path: '.agents/skills', skill_count: 3 },
          { path: '.github/skills', skill_count: 1 },
        ],
      }));

      const roots = screen.getByTestId('project-skill-roots');
      expect(within(roots).getByText('.agents/skills/')).toBeInTheDocument();
      expect(within(roots).getByText('.github/skills/')).toBeInTheDocument();
      expect(within(roots).getByText(`${R}skillRoots.count:3`)).toBeInTheDocument();
      const notice = screen.getByText(new RegExp(`${R}kronnMissingSecondary`)).closest('[role="status"]');
      expect(notice).toHaveAttribute('data-tone', 'secondary');
      expect(screen.queryByText(`${R}kronnMissing`)).not.toBeInTheDocument();
    });

    it('says so when the repository has no native skill folder', async () => {
      await show(listing({ kronn_exists: false }));

      expect(screen.getByText(`${R}skillRoots.none`)).toBeInTheDocument();
      expect(screen.getByText(new RegExp(`${R}kronnMissing$`)).closest('[role="status"]')).not.toHaveAttribute('data-tone');
    });
  });

  describe('catalog grid', () => {
    const everyState = listing({
      resources: [
        resource({ id: 'k', name: 'Only Kronn', kind: 'quick_prompt', status: 'kronn_only', repository_paths: ['kronn/prompts/only-kronn.md'] }),
        resource({ id: 'repository:quick_exec:r', name: 'Only Repo', kind: 'quick_exec', status: 'repository_only', repository_paths: ['kronn/quick-execs/only-repo.yaml'] }),
        resource({ id: 'rn', name: 'Repo Newer', kind: 'workflow', status: 'repository_newer' }),
        resource({ id: 'kn', name: 'Kronn Newer', kind: 'workflow', status: 'kronn_newer' }),
        resource({ id: 'cf', name: 'Two Versions', kind: 'quick_api', status: 'conflict' }),
        resource({ id: 'ap', name: 'Needs Approval', kind: 'quick_exec', status: 'approval_required' }),
        resource({ id: 'ok', name: 'Same', kind: 'quick_prompt', status: 'up_to_date', repository_paths: ['kronn/prompts/same.md'] }),
      ],
    });

    it('shows Dépôt | Synchro | Kronn | Action with one primary action per state', async () => {
      await show(everyState);
      openTab('automation');

      for (const column of ['repository', 'sync', 'kronn', 'action']) {
        expect(screen.getByRole('columnheader', { name: `${R}columns.${column}` })).toBeInTheDocument();
      }
      const expected: Record<string, string> = {
        'Only Kronn': 'publish',
        'Only Repo': 'import',
        'Repo Newer': 'update_kronn',
        'Kronn Newer': 'update_repository',
        'Two Versions': 'compare',
        'Needs Approval': 'approve',
      };
      for (const [name, action] of Object.entries(expected)) {
        const row = rowOf(name);
        expect(within(row).getByText(`${R}status.${row.dataset.state}`)).toBeInTheDocument();
        expect(actionIn(row, action)).toBeInTheDocument();
        expect(row.querySelectorAll('.rr-action')).toHaveLength(1);
      }
    });

    it('offers no button when everything is identical, only a discreet "Voir"', async () => {
      await show(everyState);
      openTab('automation');

      const row = rowOf('Same');
      expect(within(row).getByText(`${R}status.up_to_date`)).toBeInTheDocument();
      expect(row.querySelector('.rr-action')).toBeNull();
      expect(actionIn(row, 'view')).toBeInTheDocument();
    });

    it('prints the full path, keeps it in the tooltip and offers to copy it', async () => {
      await show(everyState);
      openTab('automation');

      const path = within(rowOf('Same')).getByTitle('kronn/prompts/same.md');
      expect(path).toHaveTextContent('kronn/prompts/same.md');
      expect(rowOf('Same').querySelector('.rr-origin')).toHaveTextContent('kronn/');
      expect(within(rowOf('Same')).getByRole('button', { name: `${R}copyPath` })).toBeInTheDocument();
    });

    it('shows the target path of a missing file, dotted, with "will be created"', async () => {
      await show(everyState);
      openTab('automation');

      const row = rowOf('Only Kronn');
      expect(within(row).getByTitle('kronn/prompts/only-kronn.md').closest('.rr-path')).toHaveAttribute('data-pending');
      expect(within(row).getByText(`${R}willBeCreated`)).toBeInTheDocument();
      expect(within(row).queryByRole('button', { name: `${R}copyPath` })).not.toBeInTheDocument();
    });

    it('groups automations by kind and remembers the sub-tab', async () => {
      const first = await show(listing({
        skills_present: [skill({ id: 'rust', name: 'Rust', status: 'up_to_date', repository_paths: ['kronn/skills/rust/SKILL.md'] })],
        skills_available: [skill({ id: 'custom-review', name: 'Review', provenance: 'kronn', status: null })],
        resources: [
          resource({ id: 'wf', name: 'Nightly', kind: 'workflow', status: 'up_to_date' }),
          resource({ id: 'qp', name: 'Review ticket', kind: 'quick_prompt', status: 'up_to_date' }),
          resource({ id: 'art', name: 'Dashboard', kind: 'artifact', status: 'up_to_date', repository_paths: ['kronn/artifacts/dashboard/index.html'] }),
        ],
      }));

      expect(screen.getByText('Rust')).toBeInTheDocument();
      expect(screen.getByText('Review')).toBeInTheDocument();
      expect(screen.getByRole('tab', { name: new RegExp(`tab\\.skills 1`) })).toHaveAttribute('aria-selected', 'true');
      openTab('automation');
      expect(screen.getByText('Nightly')).toBeInTheDocument();
      expect(document.querySelector('[data-resource-kind="workflow"]')).toBeInTheDocument();
      expect(document.querySelector('[data-resource-kind="quick_prompt"]')).toBeInTheDocument();
      expect(screen.getByRole('tab', { name: new RegExp('tab\\.automation 2') })).toBeInTheDocument();
      openTab('artifacts');
      expect(screen.getByText('Dashboard')).toBeInTheDocument();
      first.unmount();

      await show(listing({ resources: [resource({ id: 'art', name: 'Dashboard', kind: 'artifact', status: 'up_to_date' })] }));
      expect(screen.getByRole('tab', { name: new RegExp('tab\\.artifacts 1') })).toHaveAttribute('aria-selected', 'true');
    });

    it('filters by where a resource lives and searches the name and the path', async () => {
      await show(everyState);
      openTab('automation');

      fireEvent.click(screen.getByRole('button', { name: new RegExp(`${R}filter.kronn`) }));
      expect(screen.getByText('Only Kronn')).toBeInTheDocument();
      expect(screen.queryByText('Only Repo')).not.toBeInTheDocument();
      expect(screen.queryByText('Same')).not.toBeInTheDocument();

      fireEvent.click(screen.getByRole('button', { name: new RegExp(`${R}filter.both`) }));
      expect(screen.getByText('Same')).toBeInTheDocument();
      expect(screen.queryByText('Only Kronn')).not.toBeInTheDocument();

      fireEvent.click(screen.getByRole('button', { name: new RegExp(`${R}filter.all`) }));
      fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'quick-execs/only-repo' } });
      expect(screen.getByText('Only Repo')).toBeInTheDocument();
      expect(screen.queryByText('Same')).not.toBeInTheDocument();
      fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'no such thing' } });
      expect(screen.getByText(`${R}emptyFiltered`)).toBeInTheDocument();
    });
  });

  describe('"À traiter" block', () => {
    it('is hidden when nothing needs a decision', async () => {
      const onAttentionChange = vi.fn();
      await show(listing({
        resources: [
          resource({ id: 'ok', name: 'Same', kind: 'quick_prompt', status: 'up_to_date' }),
          resource({ id: 'k', name: 'Private', kind: 'quick_prompt', status: 'kronn_only' }),
        ],
      }), { onAttentionChange });

      expect(screen.queryByTestId('repository-attention')).not.toBeInTheDocument();
      await waitFor(() => expect(onAttentionChange).toHaveBeenCalledWith(0));
    });

    it('sorts by urgency, states each case in a sentence and counts per sub-tab', async () => {
      const onAttentionChange = vi.fn();
      await show(listing({
        skills_present: [
          skill({ id: 'repository:lint', name: 'Lint', provenance: 'repository', status: 'native_skill', repository_paths: ['.claude/skills/lint/SKILL.md'] }),
        ],
        resources: [
          resource({ id: 'n', name: 'Fresh', kind: 'quick_exec', status: 'repository_only' }),
          resource({ id: 'l', name: 'Late', kind: 'workflow', status: 'kronn_newer' }),
          resource({ id: 'a', name: 'Waiting', kind: 'quick_exec', status: 'approval_required' }),
          resource({ id: 'c', name: 'Clash', kind: 'workflow', status: 'conflict' }),
        ],
      }), { onAttentionChange });

      const block = screen.getByTestId('repository-attention');
      const items = within(block).getAllByRole('listitem');
      expect(items.map(item => item.dataset.reason)).toEqual(['conflict', 'approval', 'late', 'new', 'new']);
      expect(items[0]).toHaveTextContent(`${R}attention.conflict:Clash`);
      expect(within(items[0]).getByRole('button', { name: `${R}action.compare` })).toBeInTheDocument();
      expect(within(items[1]).getByRole('button', { name: `${R}action.approve` })).toBeInTheDocument();
      expect(within(items[2]).getByRole('button', { name: `${R}action.update_repository` })).toBeInTheDocument();
      expect(items[2]).toHaveTextContent(`${R}attention.kronnNewer:Late`);
      expect(within(screen.getByRole('tab', { name: /tab\.skills/ })).getByTitle(`${R}tab.todo:1`)).toBeInTheDocument();
      expect(within(screen.getByRole('tab', { name: /tab\.automation/ })).getByTitle(`${R}tab.todo:4`)).toBeInTheDocument();
      await waitFor(() => expect(onAttentionChange).toHaveBeenLastCalledWith(5));
    });

    it('runs the same flow as the row action', async () => {
      await show(listing({ resources: [resource({ id: 'l', name: 'Late', kind: 'workflow', status: 'repository_newer' })] }));

      fireEvent.click(within(screen.getByTestId('repository-attention')).getByRole('button', { name: `${R}action.update_kronn` }));

      expect(await screen.findByRole('dialog')).toHaveAttribute('data-testid', 'repository-transfer');
    });
  });

  describe('transfers announce their effect first', () => {
    it('writes to the repository only after the exact files and the commit to make are shown', async () => {
      await show(listing({
        kronn_exists: false,
        resources: [resource({
          id: 'qp-1', name: 'Review ticket', kind: 'quick_prompt', status: 'kronn_only',
          repository_paths: ['kronn/prompts/review-ticket.md'],
          write_preview: ['kronn/INDEX.md', 'kronn/prompts/review-ticket.md'],
        })],
      }));
      openTab('automation');

      fireEvent.click(actionIn(rowOf('Review ticket'), 'publish'));
      const dialog = await screen.findByRole('dialog');
      expect(within(dialog).getByText(`${R}effect.write:kronn/INDEX.md, kronn/prompts/review-ticket.md`)).toBeInTheDocument();
      expect(within(dialog).getByText(`${R}effect.createsFolder`)).toBeInTheDocument();
      expect(within(dialog).getByText(`${R}effect.commitTodo`)).toBeInTheDocument();
      expect(within(dialog).getByText(`${R}effect.lossNone`)).toBeInTheDocument();
      expect(publishRepositoryResource).not.toHaveBeenCalled();

      repositoryResources.mockResolvedValue(listing({
        resources: [resource({ id: 'qp-1', name: 'Review ticket', kind: 'quick_prompt', status: 'up_to_date' })],
      }));
      fireEvent.click(within(dialog).getByRole('button', { name: `${R}action.publish` }));
      await waitFor(() => expect(publishRepositoryResource).toHaveBeenCalledWith('project-1', {
        kind: 'quick_prompt', id: 'qp-1', overwrite_repository_changes: false,
      }));
      await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    });

    it('cancels without calling the API, on Cancel and on Escape', async () => {
      await show(listing({ resources: [resource({ id: 'a', name: 'Fresh', kind: 'quick_exec', status: 'repository_only' })] }));
      openTab('automation');

      fireEvent.click(actionIn(rowOf('Fresh'), 'import'));
      fireEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'common.cancel' }));
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument();

      fireEvent.click(actionIn(rowOf('Fresh'), 'import'));
      await screen.findByRole('dialog');
      fireEvent.keyDown(document.body, { key: 'Escape' });
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
      expect(importRepositoryResource).not.toHaveBeenCalled();
    });

    it('loads a repository definition into Kronn, then asks for its approval as a separate step', async () => {
      const imported = listing({
        resources: [resource({ id: 'repository:quick_exec:lint', name: 'Lint', slug: 'lint', kind: 'quick_exec', status: 'repository_only' })],
      });
      const awaiting = listing({
        resources: [resource({ id: 'qe-1', name: 'Lint', slug: 'lint', kind: 'quick_exec', status: 'approval_required' })],
      });
      const approved = listing({
        resources: [resource({ id: 'qe-1', name: 'Lint', slug: 'lint', kind: 'quick_exec', status: 'up_to_date', approved: true })],
      });
      repositoryResources
        .mockResolvedValueOnce(imported)
        .mockResolvedValueOnce(awaiting)
        .mockResolvedValueOnce(approved);
      approveRepositoryResource.mockResolvedValue({});
      quickExecsList.mockResolvedValue([{ id: 'qe-1', command: 'npm', args: ['run', 'lint'] }]);

      render(<ProjectRepositoryResourcesPanel projectId="project-1" />);
      await screen.findByRole('tablist');
      openTab('automation');
      fireEvent.click(actionIn(rowOf('Lint'), 'import'));
      const dialog = await screen.findByRole('dialog');
      expect(within(dialog).getByText(`${R}effect.needsApproval`)).toBeInTheDocument();
      fireEvent.click(within(dialog).getByRole('button', { name: `${R}action.import` }));
      await waitFor(() => expect(importRepositoryResource).toHaveBeenCalledWith('project-1', {
        kind: 'quick_exec', slug: 'lint', overwrite_kronn_changes: false,
      }));

      await waitFor(() => expect(within(screen.getByTestId('repository-attention'))
        .getByRole('button', { name: `${R}action.approve` })).toBeInTheDocument());
      expect(approveRepositoryResource).not.toHaveBeenCalled();
    });
  });

  describe('Comparer et choisir', () => {
    const conflict = () => listing({
      resources: [resource({
        id: 'wf-1', name: 'Nightly', slug: 'nightly', kind: 'workflow', status: 'conflict',
        repository_paths: ['kronn/workflows/nightly.yaml'],
        repository_updated_at: '2026-09-01T10:00:00Z', repository_updated_by: 'Ada',
        kronn_updated_at: '2026-09-02T08:00:00Z',
        diff: '--- repository\n+++ Kronn\n@@ -1,2 +1,2 @@\n name: nightly\n-cron: 0 3 * * *\n+cron: 0 4 * * *\n',
        file_diffs: [{ path: 'kronn/workflows/nightly.yaml', diff: '--- repository\n+++ Kronn\n@@ -1,2 +1,2 @@\n name: nightly\n-cron: 0 3 * * *\n+cron: 0 4 * * *\n' }],
        field_diff: [{ field: 'trigger.schedule', repository: '0 3 * * *', kronn: '0 4 * * *' }],
      })],
    });

    async function openCompare(data = conflict()) {
      await show(data);
      openTab('automation');
      fireEvent.click(actionIn(rowOf('Nightly'), 'compare'));
      return within(await screen.findByRole('dialog'));
    }

    it('shows both sides, what differs by field, then the text, and closes on Escape', async () => {
      const dialog = await openCompare();

      expect(dialog.getByText('kronn/workflows/nightly.yaml', { selector: 'dd code' })).toBeInTheDocument();
      expect(dialog.getByText('Ada')).toBeInTheDocument();
      expect(dialog.getByText('trigger.schedule')).toBeInTheDocument();
      expect(dialog.getByText('0 3 * * *', { selector: 'td code' })).toBeInTheDocument();
      expect(dialog.getByText('0 4 * * *', { selector: 'td code' })).toBeInTheDocument();
      expect(dialog.getByText('-cron: 0 3 * * *')).toBeInTheDocument();
      fireEvent.click(dialog.getByRole('button', { name: `${R}compare.side` }));
      expect(document.querySelector('.rr-diff[data-mode="side"]')).toBeInTheDocument();

      fireEvent.keyDown(document.body, { key: 'Escape' });
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    });

    it('keeping the repository overwrites Kronn and says so beforehand', async () => {
      const dialog = await openCompare();

      expect(dialog.getByText(new RegExp(`${R}compare.keepRepositoryEffect`))).toBeInTheDocument();
      fireEvent.click(dialog.getByRole('button', { name: `${R}compare.keepRepository` }));

      await waitFor(() => expect(importRepositoryResource).toHaveBeenCalledWith('project-1', {
        kind: 'workflow', slug: 'nightly', overwrite_kronn_changes: true,
      }));
      expect(publishRepositoryResource).not.toHaveBeenCalled();
    });

    it('keeping Kronn overwrites the named repository file', async () => {
      const dialog = await openCompare();

      expect(dialog.getByText(new RegExp(`${R}compare.keepKronnEffect:kronn/workflows/nightly.yaml`))).toBeInTheDocument();
      fireEvent.click(dialog.getByRole('button', { name: `${R}compare.keepKronn` }));

      await waitFor(() => expect(publishRepositoryResource).toHaveBeenCalledWith('project-1', {
        kind: 'workflow', id: 'wf-1', overwrite_repository_changes: true,
      }));
    });

    it('merging by hand changes nothing and refreshes on demand', async () => {
      const dialog = await openCompare();

      fireEvent.click(dialog.getByRole('button', { name: `${R}compare.merge` }));
      expect(dialog.getByText(`${R}compare.mergeSteps:kronn/workflows/nightly.yaml`)).toBeInTheDocument();
      const calls = repositoryResources.mock.calls.length;
      fireEvent.click(dialog.getByRole('button', { name: `${R}compare.refresh` }));
      await waitFor(() => expect(repositoryResources.mock.calls.length).toBe(calls + 1));
      expect(importRepositoryResource).not.toHaveBeenCalled();
      expect(publishRepositoryResource).not.toHaveBeenCalled();
    });

    it('turns "keep Kronn" off when the repository cannot be written', async () => {
      const dialog = await openCompare({ ...conflict(), can_write_repository: false });

      expect(dialog.getByRole('button', { name: `${R}compare.keepKronn` })).toBeDisabled();
      expect(dialog.getByRole('button', { name: `${R}compare.keepRepository` })).toBeEnabled();
    });

    it('opens the same sheet from a click on any row', async () => {
      await show(listing({ resources: [resource({ id: 'ok', name: 'Same', kind: 'quick_prompt', status: 'up_to_date' })] }));
      openTab('automation');

      fireEvent.click(rowOf('Same'));
      expect(await screen.findByRole('dialog')).toHaveAttribute('data-testid', 'repository-compare');
      fireEvent.keyDown(document.body, { key: 'Escape' });
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    });
  });

  describe('Relire et approuver', () => {
    const awaiting = () => listing({
      resources: [resource({
        id: 'qe-1', name: 'Lint', slug: 'lint', kind: 'quick_exec', status: 'approval_required',
        repository_paths: ['kronn/quick-execs/lint.yaml'],
        repository_updated_at: '2026-09-01T10:00:00Z', repository_updated_by: 'Ada',
        required_secrets: [{ name: 'GITHUB_TOKEN', configured: false }, { name: 'NPM_TOKEN', configured: true }],
      })],
    });

    it('shows what will run, the keys and their state, and approves this version only', async () => {
      quickExecsList.mockResolvedValue([{ id: 'qe-1', command: 'npm', args: ['run', 'lint'] }]);
      approveRepositoryResource.mockResolvedValue({});
      const onAddKey = vi.fn();
      await show(awaiting(), { onAddKey });
      openTab('automation');

      fireEvent.click(actionIn(rowOf('Lint'), 'approve'));
      const dialog = within(await screen.findByRole('dialog'));
      expect(await dialog.findByText('npm')).toBeInTheDocument();
      expect(dialog.getByText('run')).toBeInTheDocument();
      expect(dialog.getByText(`${R}execution.folder.project`)).toBeInTheDocument();
      expect(dialog.getByText('Ada')).toBeInTheDocument();
      expect(dialog.getByText(`${R}approve.keyMissing`)).toBeInTheDocument();
      expect(dialog.getByText(`${R}approve.keyConfigured`)).toBeInTheDocument();
      expect(dialog.getAllByRole('button', { name: `${R}approve.addKey` })).toHaveLength(1);

      fireEvent.click(dialog.getByRole('button', { name: `${R}approve.addKey` }));
      expect(onAddKey).toHaveBeenCalledTimes(1);

      fireEvent.click(dialog.getByRole('button', { name: `${R}approve.confirm` }));
      await waitFor(() => expect(approveRepositoryResource).toHaveBeenCalledWith('project-1', { kind: 'quick_exec', id: 'qe-1' }));
    });

    it('refuses only after a confirmation, removing the Kronn copy and nothing else', async () => {
      quickExecsList.mockResolvedValue([]);
      quickExecsDelete.mockResolvedValue(undefined);
      await show(awaiting());
      openTab('automation');

      fireEvent.click(actionIn(rowOf('Lint'), 'approve'));
      const dialog = within(await screen.findByRole('dialog'));
      fireEvent.click(dialog.getByRole('button', { name: `${R}approve.reject` }));
      expect(quickExecsDelete).not.toHaveBeenCalled();
      expect(dialog.getByText(`${R}approve.rejectConfirm:Lint`)).toBeInTheDocument();

      fireEvent.click(dialog.getByRole('button', { name: `${R}approve.rejectAction` }));
      await waitFor(() => expect(quickExecsDelete).toHaveBeenCalledWith('qe-1'));
      expect(publishRepositoryResource).not.toHaveBeenCalled();
    });

    it('never offers a bulk approval', async () => {
      await show(awaiting());

      expect(screen.queryByRole('button', { name: /approveAll|approve\.all/i })).not.toBeInTheDocument();
    });
  });

  describe('Tout aligner', () => {
    const mixed = () => listing({
      resources: [
        resource({ id: 'a', name: 'Alpha private', slug: 'alpha', kind: 'quick_prompt', status: 'kronn_only' }),
        resource({ id: 'repository:quick_prompt:bravo', name: 'Bravo repo', slug: 'bravo', kind: 'quick_prompt', status: 'repository_only' }),
        resource({ id: 'c', name: 'Charlie late', slug: 'charlie', kind: 'workflow', status: 'repository_newer' }),
        resource({ id: 'd', name: 'Delta ahead', slug: 'delta', kind: 'workflow', status: 'kronn_newer' }),
        resource({ id: 'e', name: 'Echo clash', slug: 'echo', kind: 'quick_api', status: 'conflict' }),
        resource({ id: 'f', name: 'Foxtrot waiting', slug: 'foxtrot', kind: 'quick_exec', status: 'approval_required' }),
      ],
    });

    it('recaps two lists, leaves out conflicts and approvals, and moves only the ticked lines', async () => {
      await show(mixed());
      fireEvent.click(screen.getByRole('button', { name: `${R}alignAll:4` }));

      const dialog = within(await screen.findByRole('dialog'));
      const toKronn = within(dialog.getByRole('heading', { name: `${R}align.to_kronn` }).closest('section') as HTMLElement);
      const toRepository = within(dialog.getByRole('heading', { name: `${R}align.to_repository` }).closest('section') as HTMLElement);
      expect(toKronn.getAllByRole('checkbox')).toHaveLength(2);
      expect(toRepository.getAllByRole('checkbox')).toHaveLength(2);
      expect(dialog.queryByText(/Echo clash|Foxtrot waiting/)).not.toBeInTheDocument();
      expect(dialog.getByText(`${R}align.excluded:2`)).toBeInTheDocument();
      // A Kronn-only resource stays unticked until the user chose it.
      expect(toRepository.getByRole('checkbox', { name: /Alpha private/ })).not.toBeChecked();

      fireEvent.click(toKronn.getByRole('checkbox', { name: /Charlie late/ }));
      fireEvent.click(dialog.getByRole('button', { name: `${R}align.confirm:2` }));

      await waitFor(() => expect(importRepositoryResource).toHaveBeenCalledTimes(1));
      expect(importRepositoryResource).toHaveBeenCalledWith('project-1', { kind: 'quick_prompt', slug: 'bravo', overwrite_kronn_changes: false });
      await waitFor(() => expect(publishRepositoryResource).toHaveBeenCalledTimes(1));
      expect(publishRepositoryResource).toHaveBeenCalledWith('project-1', { kind: 'workflow', id: 'd', overwrite_repository_changes: false });
    });

    it('does not count what it could not write when the repository is read-only', async () => {
      await show({ ...mixed(), can_write_repository: false });

      expect(screen.getByRole('button', { name: `${R}alignAll:2` })).toBeInTheDocument();
    });
  });

  describe('banners', () => {
    it('counts the files changed and not committed, with a link to the Git tab', async () => {
      const onOpenGit = vi.fn();
      await show(listing({ uncommitted_managed_paths: ['kronn/a.md', 'kronn/b.md'] }), { onOpenGit });

      const banner = screen.getByText(`${R}banner.uncommitted:2`).closest('[role="status"]') as HTMLElement;
      fireEvent.click(within(banner).getByRole('button', { name: `${R}banner.openGit` }));
      expect(onOpenGit).toHaveBeenCalledTimes(1);
    });

    it('explains an unavailable write without an error and turns off only the writing actions', async () => {
      await show(listing({
        kronn_exists: false,
        can_write_repository: false,
        can_write_repository_reason: 'a file named kronn is in the way',
        resources: [
          resource({ id: 'a', name: 'Private', kind: 'quick_prompt', status: 'kronn_only' }),
          resource({ id: 'repository:quick_prompt:b', name: 'Shared', kind: 'quick_prompt', status: 'repository_only' }),
        ],
      }));
      openTab('automation');

      const banner = document.querySelector('[data-banner="write-disabled"]') as HTMLElement;
      expect(within(banner).getByText(`${R}banner.writeDisabled.title`)).toBeInTheDocument();
      expect(within(banner).getByText('a file named kronn is in the way')).toBeInTheDocument();
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      expect(screen.queryByText(`${R}kronnMissing`)).not.toBeInTheDocument();
      expect(actionIn(rowOf('Private'), 'publish')).toBeDisabled();
      expect(actionIn(rowOf('Shared'), 'import')).toBeEnabled();
    });

    it('keeps the list on screen when an action fails', async () => {
      publishRepositoryResource.mockRejectedValue(new Error('Invalid path'));
      await show(listing({ resources: [resource({ id: 'a', name: 'Private', kind: 'quick_prompt', status: 'kronn_only' })] }));
      openTab('automation');

      fireEvent.click(actionIn(rowOf('Private'), 'publish'));
      fireEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: `${R}action.publish` }));

      expect(await screen.findByRole('alert')).toHaveTextContent('Invalid path');
      expect(screen.getByRole('button', { name: 'Private' })).toBeInTheDocument();
    });
  });

  describe('skills', () => {
    const lint = (extra = {}) => skill({
      id: 'repository:lint', name: 'Lint', provenance: 'repository', status: 'native_skill',
      repository_paths: ['.claude/skills/lint/SKILL.md'], ...extra,
    });

    it('uses a native skill in Kronn by reference after announcing it', async () => {
      useNativeSkill.mockResolvedValue({});
      await show(listing({ skills_present: [lint()] }));

      fireEvent.click(actionIn(rowOf('Lint'), 'use_native'));
      const dialog = within(await screen.findByRole('dialog'));
      expect(dialog.getByText(`${R}effect.usePath:.claude/skills/lint/SKILL.md`)).toBeInTheDocument();
      fireEvent.click(dialog.getByRole('button', { name: `${R}action.use_native` }));

      await waitFor(() => expect(useNativeSkill).toHaveBeenCalledWith('project-1', { relative_path: '.claude/skills/lint/SKILL.md' }));
    });

    it('does not pick silently between copies that differ', async () => {
      useNativeSkill.mockResolvedValue({});
      await show(listing({
        skills_present: [lint({
          repository_paths: ['.claude/skills/lint/SKILL.md', '.agents/skills/lint/SKILL.md'],
          repository_paths_diverge: true,
        })],
      }));

      expect(within(rowOf('Lint')).getByText(`${R}originDiverge:.claude, .agents`)).toBeInTheDocument();
      fireEvent.click(actionIn(rowOf('Lint'), 'use_native'));
      const dialog = within(await screen.findByRole('dialog'));
      const confirm = dialog.getByRole('button', { name: `${R}action.use_native` });
      expect(confirm).toBeDisabled();
      fireEvent.click(dialog.getByRole('radio', { name: '.agents/skills/lint/SKILL.md' }));
      expect(confirm).toBeEnabled();
      fireEvent.click(confirm);

      await waitFor(() => expect(useNativeSkill).toHaveBeenCalledWith('project-1', { relative_path: '.agents/skills/lint/SKILL.md' }));
    });

    it('copies a native skill into Kronn from the row menu', async () => {
      copyNativeSkill.mockResolvedValue({});
      await show(listing({ skills_present: [lint()] }));

      fireEvent.click(within(rowOf('Lint')).getByRole('button', { name: `${R}moreActions:Lint` }));
      fireEvent.click(screen.getByRole('menuitem', { name: `${R}action.copy_native` }));
      const dialog = within(await screen.findByRole('dialog'));
      expect(dialog.getByText(`${R}effect.copyPath:.claude/skills/lint/SKILL.md`)).toBeInTheDocument();
      fireEvent.click(dialog.getByRole('button', { name: `${R}action.copy_native` }));

      await waitFor(() => expect(copyNativeSkill).toHaveBeenCalledWith('project-1', { relative_path: '.claude/skills/lint/SKILL.md' }));
    });

    it('attaches a catalog skill by resending the skills already attached', async () => {
      setDefaultSkills.mockResolvedValue(true);
      await show(listing({
        skills_present: [skill({ id: 'kept', name: 'Kept', status: 'up_to_date', repository_paths: ['kronn/skills/kept/SKILL.md'] })],
        skills_available: [skill({ id: 'rust', name: 'Rust', provenance: 'kronn', status: null })],
      }));

      const row = rowOf('Rust');
      expect(within(row).getByText(`${R}scope.catalog`)).toBeInTheDocument();
      fireEvent.click(actionIn(row, 'attach'));
      const dialog = within(await screen.findByRole('dialog'));
      expect(dialog.getByText(`${R}effect.attach:Rust`)).toBeInTheDocument();
      fireEvent.click(dialog.getByRole('button', { name: `${R}action.attach` }));

      await waitFor(() => expect(setDefaultSkills).toHaveBeenCalledWith('project-1', ['kept', 'rust']));
    });

    it('shows a skill followed by reference as read-only with nothing left to do', async () => {
      await show(listing({ skills_present: [lint({ referenced: true })] }));

      const row = rowOf('Lint');
      expect(within(row).getByText(`${R}scope.referenced`)).toBeInTheDocument();
      expect(row.querySelector('.rr-action')).toBeNull();
    });
  });

  describe('multiple selection and the kronn/ preview', () => {
    it('previews the files selected for the first write and writes them as a group', async () => {
      await show(listing({
        kronn_exists: false,
        resources: [resource({
          id: 'qp-1', name: 'Review ticket', kind: 'quick_prompt', status: 'kronn_only',
          repository_paths: ['kronn/prompts/review-ticket.md'],
          write_preview: ['kronn/prompts/review-ticket.md'],
        })],
      }));
      openTab('automation');

      expect(screen.getByText(`${R}kronnMissing`)).toBeInTheDocument();
      const checkbox = screen.getByRole('checkbox', { name: `${R}include:Review ticket` });
      expect(checkbox).not.toBeChecked();
      expect(checkbox).toBeEnabled();
      const preview = screen.getByTestId('repository-preview');
      expect(preview.tagName).toBe('DETAILS');
      expect(preview).not.toHaveAttribute('open');
      const root = preview.querySelector('.project-repository-tree-root') as HTMLElement;
      const scaffold = screen.getByText('INDEX.md · kronn.toml · kronn.lock').closest('.project-repository-tree-entry') as HTMLElement;
      const prompt = within(preview).getByText('prompts/review-ticket.md').closest('.project-repository-tree-entry') as HTMLElement;
      expect(root).toHaveAttribute('data-excluded', 'true');
      expect(scaffold).toHaveAttribute('data-excluded', 'true');
      expect(prompt).toHaveAttribute('data-excluded', 'true');

      fireEvent.click(checkbox);
      await waitFor(() => {
        expect(root).not.toHaveAttribute('data-excluded');
        expect(root.querySelector('.project-repository-tree-marker')).toHaveTextContent('+');
        expect(scaffold.querySelector('.project-repository-tree-marker')).toHaveTextContent('+');
        expect(prompt.querySelector('.project-repository-tree-marker')).toHaveTextContent('+');
      });

      fireEvent.click(screen.getByRole('button', { name: `${R}publishSelected:1` }));
      const dialog = within(await screen.findByRole('dialog'));
      expect(dialog.getByText(`${R}effect.write:kronn/prompts/review-ticket.md`)).toBeInTheDocument();
      fireEvent.click(dialog.getByRole('button', { name: `${R}publishSelected:1` }));
      await waitFor(() => expect(publishRepositoryResource).toHaveBeenCalledWith('project-1', {
        kind: 'quick_prompt', id: 'qp-1', overwrite_repository_changes: false,
      }));
    });

    it('keeps published resources ticked and locked', async () => {
      await show(listing({
        resources: [resource({ id: 'wf-1', name: 'Daily report', kind: 'workflow', status: 'up_to_date' })],
      }));
      openTab('automation');

      const box = within(rowOf('Daily report')).getByRole('checkbox');
      expect(box).toBeChecked();
      expect(box).toBeDisabled();
    });
  });

  describe('400 px layout', () => {
    const mobile = (css: string) => css.slice(css.indexOf('@media (max-width: 640px)'));

    it('stacks a row on two levels with full-width controls of at least 44 px', () => {
      const rules = mobile(panelCss);
      expect(rules).toContain('grid-template-areas');
      expect(rules).toMatch(/\.rr-cell-action \.rr-action[^}]*min-height: 44px/);
      expect(rules).toMatch(/\.rr-cell-action \{ grid-area: action/);
      expect(rules).toMatch(/\.rr-head \{ display: none/);
    });

    it('opens the sheet full screen with 44 px controls', () => {
      const rules = mobile(sheetsCss);
      expect(rules).toMatch(/\.rr-modal[^{]*\{[^}]*width: 100%; height: 100%/);
      expect(rules).toMatch(/\.rr-modal \.rr-button[^}]*min-height: 44px/);
    });

    it('uses design tokens only', () => {
      for (const css of [panelCss, sheetsCss]) {
        expect(css).not.toMatch(/#[0-9a-fA-F]{3,8}\b|rgba?\(|var\(--(?!kr-)/);
      }
    });
  });
});
