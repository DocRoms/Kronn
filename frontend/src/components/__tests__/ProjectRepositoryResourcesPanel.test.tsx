import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import fr from '../../lib/i18n/locales/fr';
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
// The "available in Kronn, not in this project" catalog is folded until opened.
const named = (key: string) => new RegExp(`${R}${key}`.replace(/\./g, '\\.'));
const catalogToggle = () => screen.getByRole('button', { name: named('skills.available') });
const openCatalog = () => fireEvent.click(catalogToggle());

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
    localStorage.removeItem('kronn:projectRepositoryCatalogOpen:project-1');
    localStorage.removeItem('kronn:projectRepositoryCatalogOpen:project-2');
  });

  describe('skill folders and the kronn/ explanation', () => {
    it('lists the native skill folders, then a folded "Share with the repository" block', async () => {
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
      const share = screen.getByTestId('repository-share');
      expect(share.tagName).toBe('DETAILS');
      expect(share).not.toHaveAttribute('open');
      expect(within(share).getByText(`${R}share.title`)).toBeInTheDocument();
      expect(within(share).getByText(`${R}share.missing`)).toBeInTheDocument();
    });

    it('says what kronn/ is for once unfolded: versioning, cloning, approval, sub-folders', async () => {
      await show(listing({ kronn_exists: false }));

      expect(screen.getByText(`${R}skillRoots.none`)).toBeInTheDocument();
      const share = screen.getByTestId('repository-share');
      for (const sentence of ['purpose', 'approval', 'subprojects']) {
        expect(within(share).getByText(`${R}share.${sentence}`)).toBeInTheDocument();
      }
    });

    it('is not offered once kronn/ exists, nor where the repository cannot be written', async () => {
      const view = await show(listing({ kronn_exists: true }));
      expect(screen.queryByTestId('repository-share')).not.toBeInTheDocument();
      view.unmount();

      await show(listing({ kronn_exists: false, can_write_repository: false, can_write_repository_reason: 'kronn_path_is_file' }));
      expect(screen.queryByTestId('repository-share')).not.toBeInTheDocument();
    });

    it('words the explanation in French with the four points the team asked for', () => {
      const body = [fr[`${R}share.purpose`], fr[`${R}share.approval`], fr[`${R}share.subprojects`]].join(' ');
      expect(fr[`${R}share.title`]).toBe('Partager avec le dépôt (kronn/)');
      expect(body).toMatch(/versionner dans le dépôt les skills, quick execs et workflows/);
      expect(body).toMatch(/clonant/);
      expect(body).toMatch(/approbation/);
      expect(body).toMatch(/sous-projets/);
    });

    it('keeps the block discreet: muted, current text size, no box', () => {
      expect(panelCss).toMatch(/\.rr-share \{[^}]*color: var\(--kr-text-muted\)[^}]*font-size: var\(--kr-fs-sm\)/);
      expect(panelCss).not.toMatch(/\.rr-share[^{]*\{[^}]*font-size: (1[2-9]|2\d)px/);
      expect(panelCss).not.toMatch(/\.rr-share \{[^}]*border:/);
      expect(panelCss).toMatch(/\.project-repository-resources \{[^}]*font-size: var\(--kr-fs-sm\)/);
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
        skills_available: [skill({ id: 'custom-review', name: 'Review', provenance: 'kronn', status: 'kronn_only' })],
        resources: [
          resource({ id: 'wf', name: 'Nightly', kind: 'workflow', status: 'up_to_date' }),
          resource({ id: 'qp', name: 'Review ticket', kind: 'quick_prompt', status: 'up_to_date' }),
          resource({ id: 'art', name: 'Dashboard', kind: 'artifact', status: 'up_to_date', repository_paths: ['kronn/artifacts/dashboard/index.html'] }),
        ],
      }));

      expect(screen.getByText('Rust')).toBeInTheDocument();
      expect(screen.queryByText('Review')).not.toBeInTheDocument();
      openCatalog();
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
      openTab('automation');

      const block = screen.getByTestId('repository-attention');
      const items = within(block).getAllByRole('listitem');
      expect(items.map(item => item.dataset.reason)).toEqual(['conflict', 'approval', 'late', 'new']);
      expect(items[0]).toHaveTextContent(`${R}attention.conflict:Clash`);
      expect(within(items[0]).getByRole('button', { name: `${R}action.compare` })).toBeInTheDocument();
      expect(within(items[1]).getByRole('button', { name: `${R}action.approve` })).toBeInTheDocument();
      expect(within(items[2]).getByRole('button', { name: `${R}action.update_repository` })).toBeInTheDocument();
      expect(items[2]).toHaveTextContent(`${R}attention.kronnNewer:Late`);
      expect(within(screen.getByRole('tab', { name: /tab\.skills/ })).getByTitle(`${R}tab.todo:1`)).toBeInTheDocument();
      expect(within(screen.getByRole('tab', { name: /tab\.automation/ })).getByTitle(`${R}tab.todo:4`)).toBeInTheDocument();
      await waitFor(() => expect(onAttentionChange).toHaveBeenLastCalledWith(5));
    });

    it('is proper to each sub-tab: the automations never show the skills to process', async () => {
      await show(listing({
        skills_present: [
          skill({ id: 'repository:lint', name: 'Lint', provenance: 'repository', status: 'native_skill', repository_paths: ['.claude/skills/lint/SKILL.md'] }),
        ],
        resources: [resource({ id: 'l', name: 'Late', kind: 'workflow', status: 'kronn_newer' })],
      }));

      const skillsBlock = within(screen.getByTestId('repository-attention'));
      expect(skillsBlock.getByText(`${R}attention.nativeSkill:Lint`)).toBeInTheDocument();
      expect(skillsBlock.queryByText(/Late/)).not.toBeInTheDocument();

      openTab('automation');
      const automationBlock = within(screen.getByTestId('repository-attention'));
      expect(automationBlock.getByText(`${R}attention.kronnNewer:Late`)).toBeInTheDocument();
      expect(automationBlock.queryByText(/Lint/)).not.toBeInTheDocument();

      openTab('artifacts');
      expect(screen.queryByTestId('repository-attention')).not.toBeInTheDocument();
    });

    it('never lists suggested or unattached skills, and shows a count only above zero', async () => {
      const onAttentionChange = vi.fn();
      await show(listing({
        skills_present: [
          skill({ id: 'devops', name: 'DevOps', provenance: 'kronn', status: 'kronn_only', suggested: true, suggested_reason: 'Dockerfile', is_builtin: true }),
          skill({ id: 'typescript', name: 'TypeScript', provenance: 'repository', status: 'repository_only', repository_paths: ['.claude/skills/typescript/SKILL.md'] }),
        ],
        skills_available: [skill({ id: 'rust', name: 'Rust', provenance: 'kronn', status: 'kronn_only' })],
      }), { onAttentionChange });

      expect(screen.queryByTestId('repository-attention')).not.toBeInTheDocument();
      expect(within(screen.getByRole('tab', { name: /tab\.skills/ })).queryByTitle(/tab\.todo/)).not.toBeInTheDocument();
      await waitFor(() => expect(onAttentionChange).toHaveBeenLastCalledWith(0));
    });

    it('runs the same flow as the row action', async () => {
      await show(listing({ resources: [resource({ id: 'l', name: 'Late', kind: 'workflow', status: 'repository_newer' })] }));
      openTab('automation');

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

    it('recaps the late items in two lists, all ticked, and moves only the ticked lines', async () => {
      await show(mixed());
      openTab('automation');
      fireEvent.click(screen.getByRole('button', { name: `${R}alignAll:2` }));

      const dialog = within(await screen.findByRole('dialog'));
      const toKronn = within(dialog.getByRole('heading', { name: `${R}align.to_kronn` }).closest('section') as HTMLElement);
      const toRepository = within(dialog.getByRole('heading', { name: `${R}align.to_repository` }).closest('section') as HTMLElement);
      expect(toKronn.getAllByRole('checkbox')).toHaveLength(1);
      expect(toRepository.getAllByRole('checkbox')).toHaveLength(1);
      expect(dialog.queryByText(/Alpha private|Bravo repo|Echo clash|Foxtrot waiting/)).not.toBeInTheDocument();
      expect(dialog.getByText(`${R}align.excluded:2`)).toBeInTheDocument();
      dialog.getAllByRole('checkbox').forEach(box => expect(box).toBeChecked());

      fireEvent.click(toRepository.getByRole('checkbox', { name: /Delta ahead/ }));
      fireEvent.click(dialog.getByRole('button', { name: `${R}align.confirm:1` }));

      await waitFor(() => expect(importRepositoryResource).toHaveBeenCalledTimes(1));
      expect(importRepositoryResource).toHaveBeenCalledWith('project-1', { kind: 'workflow', slug: 'charlie', overwrite_kronn_changes: false });
      expect(publishRepositoryResource).not.toHaveBeenCalled();
    });

    it('moves both directions when both are ticked', async () => {
      await show(mixed());
      openTab('automation');
      fireEvent.click(screen.getByRole('button', { name: `${R}alignAll:2` }));

      fireEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: `${R}align.confirm:2` }));

      await waitFor(() => expect(publishRepositoryResource).toHaveBeenCalledTimes(1));
      expect(publishRepositoryResource).toHaveBeenCalledWith('project-1', { kind: 'workflow', id: 'd', overwrite_repository_changes: false });
      expect(importRepositoryResource).toHaveBeenCalledWith('project-1', { kind: 'workflow', slug: 'charlie', overwrite_kronn_changes: false });
    });

    it('is not shown at all when nothing is behind, so it never reads as a global state', async () => {
      await show(listing({
        skills_available: [skill({ id: 'rust', name: 'Rust', provenance: 'kronn', status: 'kronn_only' })],
        resources: [
          ...Array.from({ length: 79 }, (_, index) => resource({ id: `qe-${index}`, name: `Automation ${index}`, kind: 'quick_exec', status: 'kronn_only' })),
          resource({ id: 'same', name: 'Same', kind: 'workflow', status: 'up_to_date' }),
        ],
      }));
      openTab('automation');

      expect(screen.queryByTestId('align-all')).not.toBeInTheDocument();
      expect(screen.queryByRole('button', { name: /alignAll/ })).not.toBeInTheDocument();
      expect(document.querySelector('.rr-bulk')).toBeNull();
    });

    it('appears for what is really late only, never for attach suggestions', async () => {
      await show(listing({
        skills_present: [skill({ id: 'devops', name: 'DevOps', provenance: 'kronn', status: 'kronn_only', suggested: true, suggested_reason: 'Dockerfile' })],
        resources: [resource({ id: 'l', name: 'Late', kind: 'workflow', status: 'repository_newer' })],
      }));

      expect(screen.queryByTestId('align-all')).not.toBeInTheDocument();
      openTab('automation');
      expect(screen.getByTestId('repository-attention')).toBeInTheDocument();
      expect(screen.getByTestId('align-all')).toHaveTextContent(`${R}alignAll:1`);
    });

    it('counts only the items of the sub-tab being looked at', async () => {
      await show(listing({
        skills_present: [skill({ id: 'kept', name: 'Kept', status: 'repository_newer', repository_paths: ['kronn/skills/kept/SKILL.md'] })],
        resources: [resource({ id: 'l', name: 'Late', kind: 'workflow', status: 'kronn_newer' })],
      }));

      expect(screen.getByTestId('align-all')).toHaveTextContent(`${R}alignAll:1`);
      openTab('automation');
      expect(screen.getByTestId('align-all')).toHaveTextContent(`${R}alignAll:1`);
      openTab('artifacts');
      expect(screen.queryByTestId('align-all')).not.toBeInTheDocument();
    });

    it('is turned off, not hidden, when the only late items are repository writes that cannot happen', async () => {
      await show(listing({
        can_write_repository: false,
        can_write_repository_reason: 'repository_read_only',
        resources: [resource({ id: 'd', name: 'Delta ahead', kind: 'workflow', status: 'kronn_newer' })],
      }));
      openTab('automation');

      const button = screen.getByTestId('align-all');
      expect(button).toBeDisabled();
      expect(button).toHaveAttribute('title', `${R}banner.writeDisabled.title`);
    });

    it('turns off the "repository will be updated" part when the repository cannot be written', async () => {
      await show({ ...mixed(), can_write_repository: false });
      openTab('automation');

      expect(screen.getByRole('button', { name: `${R}alignAll:1` })).toBeInTheDocument();
      fireEvent.click(screen.getByRole('button', { name: `${R}alignAll:1` }));
      const dialog = within(await screen.findByRole('dialog'));
      const toRepository = within(dialog.getByRole('heading', { name: `${R}align.to_repository` }).closest('section') as HTMLElement);
      expect(toRepository.getByRole('checkbox', { name: /Delta ahead/ })).toBeDisabled();
      expect(toRepository.getByRole('checkbox', { name: /Delta ahead/ })).not.toBeChecked();
      expect(dialog.getByRole('button', { name: `${R}align.confirm:1` })).toBeEnabled();
    });

    it('opens in the middle of the screen, outside the card, with checkboxes of one size', () => {
      expect(sheetsCss).toMatch(/\.rr-modal-backdrop \{[^}]*align-items: center; justify-content: center/);
      expect(panelCss).toMatch(/\.rr-cell-select input\[type='checkbox'\],\s*\.rr-align-list input\[type='checkbox'\] \{[^}]*width: 16px; height: 16px/);
    });

    it('renders its dialog at the document root so no ancestor can pin it to a corner', async () => {
      const view = await show(mixed());
      openTab('automation');
      fireEvent.click(screen.getByRole('button', { name: `${R}alignAll:2` }));

      const dialog = await screen.findByRole('dialog');
      expect(view.container.contains(dialog)).toBe(false);
      expect(dialog.closest('.rr-modal-backdrop')?.parentElement).toBe(document.body);
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
        can_write_repository_reason: 'kronn_path_is_file',
        resources: [
          resource({ id: 'a', name: 'Private', kind: 'quick_prompt', status: 'kronn_only' }),
          resource({ id: 'repository:quick_prompt:b', name: 'Shared', kind: 'quick_prompt', status: 'repository_only' }),
        ],
      }));
      openTab('automation');

      const banner = document.querySelector('[data-banner="write-disabled"]') as HTMLElement;
      expect(within(banner).getByText(`${R}banner.writeDisabled.title`)).toBeInTheDocument();
      expect(within(banner).getByText(`${R}banner.writeDisabled.reason.kronn_path_is_file`)).toBeInTheDocument();
      expect(banner).not.toHaveTextContent(/exists and is not a directory/);
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      expect(screen.queryByTestId('repository-share')).not.toBeInTheDocument();
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
        skills_available: [skill({ id: 'rust', name: 'Rust', provenance: 'kronn', status: 'kronn_only' })],
      }));

      openCatalog();
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

      expect(screen.getByTestId('repository-share')).toBeInTheDocument();
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

  describe('every state, as the recette showed them', () => {
    const devops = () => skill({
      id: 'devops', name: 'DevOps', provenance: 'kronn', status: 'kronn_only', is_builtin: true,
      suggested: true, suggested_reason: 'Dockerfile',
    });
    const matrix = () => listing({
      skills_present: [
        skill({ id: 'repository:lint', name: 'Native lint', provenance: 'repository', status: 'native_skill', repository_paths: ['.agents/skills/lint/SKILL.md'] }),
        devops(),
        skill({ id: 'custom-mine', name: 'Mine attached', provenance: 'kronn', status: 'kronn_only', publication_path: 'kronn/skills/mine/SKILL.md' }),
        skill({ id: 'typescript', name: 'In a folder', provenance: 'repository', status: 'repository_only', repository_paths: ['.claude/skills/typescript/SKILL.md'] }),
        skill({ id: 'same', name: 'Same skill', status: 'up_to_date', repository_paths: ['kronn/skills/same/SKILL.md'] }),
      ],
      skills_available: [skill({ id: 'rust', name: 'Unrelated', provenance: 'kronn', status: 'kronn_only' })],
      resources: [
        resource({ id: 'k', name: 'Only Kronn', kind: 'quick_prompt', status: 'kronn_only', repository_paths: ['kronn/prompts/only-kronn.md'] }),
        resource({ id: 'repository:quick_exec:r', name: 'Only Repo', kind: 'quick_exec', status: 'repository_only', repository_paths: ['kronn/quick-execs/only-repo.yaml'] }),
        resource({ id: 'rn', name: 'Repo Newer', kind: 'workflow', status: 'repository_newer' }),
        resource({ id: 'kn', name: 'Kronn Newer', kind: 'workflow', status: 'kronn_newer' }),
        resource({ id: 'cf', name: 'Two Versions', kind: 'quick_api', status: 'conflict' }),
        resource({ id: 'ap', name: 'Needs Approval', kind: 'quick_exec', status: 'approval_required' }),
        resource({ id: 'ok', name: 'Identical', kind: 'quick_prompt', status: 'up_to_date', repository_paths: ['kronn/prompts/identical.md'] }),
      ],
    });

    it('calls a stack-suggested skill "Kronn only · suggested", never "in the repository"', async () => {
      await show(matrix());

      const row = rowOf('DevOps');
      expect(within(row).getByText(`${R}status.kronn_only`)).toBeInTheDocument();
      expect(within(row).getByText(new RegExp(`${R}scope.suggested`))).toBeInTheDocument();
      expect(within(row).getByText(`${R}suggestedBecause:Dockerfile`)).toBeInTheDocument();
      expect(within(row).queryByText('—')).not.toBeInTheDocument();
      expect(row.querySelector('.rr-path')).toBeNull();
      expect(actionIn(row, 'attach')).toBeInTheDocument();
      expect(within(row).getByRole('checkbox')).toBeDisabled();
      expect(screen.queryByText(new RegExp(`attention\\.repositoryOnly:DevOps|attention\\.notAttached`))).not.toBeInTheDocument();
    });

    it('lists the suggestions apart and counts only what is really in the project', async () => {
      await show(matrix());

      const suggested = document.querySelector('[role="rowgroup"][aria-label="' + R + 'skills.suggested"]') as HTMLElement;
      expect(within(suggested).getByText('DevOps')).toBeInTheDocument();
      expect(within(suggested).queryByText('Mine attached')).not.toBeInTheDocument();
      expect(screen.getByRole('tab', { name: /tab\.skills 4/ })).toBeInTheDocument();
    });

    it('prints no path and no dash line for an unrelated catalog skill', async () => {
      await show(matrix());

      openCatalog();
      const row = rowOf('Unrelated');
      expect(within(row).queryByText('—')).not.toBeInTheDocument();
      expect(row.querySelector('.rr-path')).toBeNull();
      expect(row.querySelector('.rr-cell-repository')?.children).toHaveLength(1);
      expect(row).not.toHaveTextContent('SKILL.md');
      expect(row).not.toHaveTextContent(`${R}notInRepository`);
      expect(within(row).getByText(`${R}status.catalog`)).toBeInTheDocument();
    });

    it('leaves one primary action per state and none on the identical ones', async () => {
      await show(matrix());
      openCatalog();
      const expectSkill: Record<string, string | null> = {
        'Native lint': 'use_native',
        DevOps: 'attach',
        'Mine attached': 'publish',
        'In a folder': 'attach',
        'Same skill': null,
        Unrelated: 'attach',
      };
      for (const [name, action] of Object.entries(expectSkill)) {
        const row = rowOf(name);
        if (action === null) {
          expect(row.querySelector('.rr-action')).toBeNull();
          expect(actionIn(row, 'view')).toBeInTheDocument();
        } else {
          expect(actionIn(row, action)).toBeInTheDocument();
          expect(row.querySelectorAll('.rr-action')).toHaveLength(1);
        }
      }
      openTab('automation');
      const expectAutomation: Record<string, string> = {
        'Only Kronn': 'publish', 'Only Repo': 'import', 'Repo Newer': 'update_kronn',
        'Kronn Newer': 'update_repository', 'Two Versions': 'compare', 'Needs Approval': 'approve',
      };
      for (const [name, action] of Object.entries(expectAutomation)) {
        expect(actionIn(rowOf(name), action)).toBeInTheDocument();
      }
    });

    it('only puts what needs a decision in "À traiter", skills and automations apart', async () => {
      await show(matrix());

      const skills = within(screen.getByTestId('repository-attention')).getAllByRole('listitem');
      expect(skills).toHaveLength(1);
      expect(skills[0]).toHaveTextContent(`${R}attention.nativeSkill:Native lint`);

      openTab('automation');
      const automations = within(screen.getByTestId('repository-attention')).getAllByRole('listitem');
      expect(automations.map(item => item.dataset.reason)).toEqual(['conflict', 'approval', 'late', 'late', 'new']);
    });

    it('shows the todo count as a warning badge next to a plainer count, only when above zero', async () => {
      await show(matrix());

      const skillsTab = screen.getByRole('tab', { name: /tab\.skills/ });
      const count = skillsTab.querySelector('.rr-count') as HTMLElement;
      const todo = skillsTab.querySelector('.rr-todo') as HTMLElement;
      expect(count).toHaveTextContent('4');
      expect(todo).toHaveTextContent('1');
      expect(todo).not.toBe(count);
      openTab('artifacts');
      expect(screen.getByRole('tab', { name: /tab\.artifacts/ }).querySelector('.rr-todo')).toBeNull();
      expect(panelCss).toMatch(/\.project-repository-resources-tabs \.rr-todo \{[^}]*var\(--kr-warning\)/);
      expect(panelCss).toMatch(/\.project-repository-resources-tabs \.rr-count \{[^}]*var\(--kr-bg-hover\)/);
    });
  });

  describe('the catalog of Kronn skills that are not in this project', () => {
    const withCatalog = () => listing({
      skills_present: [skill({ id: 'kept', name: 'Kept', status: 'up_to_date', repository_paths: ['kronn/skills/kept/SKILL.md'] })],
      skills_available: Array.from({ length: 34 }, (_, index) => skill({
        id: `catalog-${index}`, name: `Catalog skill ${index}`, provenance: 'kronn', status: 'kronn_only',
        description: `Does thing number ${index}`,
      })),
    });

    it('is folded by default: one header line with its count, and none of the 34 rows', async () => {
      await show(withCatalog());

      const toggle = catalogToggle();
      expect(toggle).toHaveAttribute('aria-expanded', 'false');
      expect(toggle).toHaveTextContent('34');
      expect(toggle).toBeEnabled();
      expect(screen.queryByText('Catalog skill 0')).not.toBeInTheDocument();
      expect(screen.getByText('Kept')).toBeInTheDocument();
      expect(document.querySelectorAll('[role="row"][data-state="catalog"]')).toHaveLength(0);
    });

    it('unfolds on a click and folds again on the next', async () => {
      await show(withCatalog());

      openCatalog();
      expect(catalogToggle()).toHaveAttribute('aria-expanded', 'true');
      expect(screen.getByText('Catalog skill 0')).toBeInTheDocument();
      expect(document.querySelectorAll('[role="row"][data-state="catalog"]')).toHaveLength(34);
      openCatalog();
      expect(catalogToggle()).toHaveAttribute('aria-expanded', 'false');
      expect(screen.queryByText('Catalog skill 0')).not.toBeInTheDocument();
    });

    it('remembers the choice for this project and for no other', async () => {
      const first = await show(withCatalog());
      openCatalog();
      expect(localStorage.getItem('kronn:projectRepositoryCatalogOpen:project-1')).toBe('open');
      first.unmount();

      const again = await show(withCatalog());
      expect(catalogToggle()).toHaveAttribute('aria-expanded', 'true');
      again.unmount();

      repositoryResources.mockResolvedValue(withCatalog());
      const other = render(<ProjectRepositoryResourcesPanel projectId="project-2" />);
      await screen.findByRole('tablist');
      expect(catalogToggle()).toHaveAttribute('aria-expanded', 'false');
      other.unmount();

      await show(withCatalog());
      openCatalog();
      expect(localStorage.getItem('kronn:projectRepositoryCatalogOpen:project-1')).toBeNull();
    });

    it('opens by itself as soon as something is searched, and finds the folded skill', async () => {
      await show(withCatalog());

      fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'skill 7' } });
      expect(catalogToggle()).toHaveAttribute('aria-expanded', 'true');
      expect(screen.getByText('Catalog skill 7')).toBeInTheDocument();
      expect(screen.queryByText('Catalog skill 8')).not.toBeInTheDocument();
      fireEvent.change(screen.getByRole('searchbox'), { target: { value: '' } });
      expect(catalogToggle()).toHaveAttribute('aria-expanded', 'false');
      expect(screen.queryByText('Catalog skill 7')).not.toBeInTheDocument();
    });

    it('opens with the "Kronn only" filter, and stays out of the way of the other filters', async () => {
      await show(withCatalog());

      fireEvent.click(screen.getByRole('button', { name: named('filter.kronn') }));
      expect(catalogToggle()).toHaveAttribute('aria-expanded', 'true');
      expect(screen.getByText('Catalog skill 33')).toBeInTheDocument();

      fireEvent.click(screen.getByRole('button', { name: named('filter.repository') }));
      expect(screen.queryByRole('button', { name: named('skills.available') })).not.toBeInTheDocument();
      fireEvent.click(screen.getByRole('button', { name: named('filter.all') }));
      expect(catalogToggle()).toHaveAttribute('aria-expanded', 'false');
    });

    it('offers no header when there is nothing in the catalog', async () => {
      await show(listing({ skills_present: [skill({ id: 'kept', name: 'Kept', status: 'up_to_date', repository_paths: ['kronn/skills/kept/SKILL.md'] })] }));

      expect(screen.queryByRole('button', { name: named('skills.available') })).not.toBeInTheDocument();
    });

    it('keeps a row on one line of description, the whole text in its tooltip', async () => {
      await show(withCatalog());

      openCatalog();
      const description = rowOf('Catalog skill 3').querySelector('.rr-description') as HTMLElement;
      expect(description).toHaveAttribute('title', 'Does thing number 3');
      expect(panelCss).toMatch(/\.rr-description \{[^}]*-webkit-line-clamp: 1/);
    });

    it('words a provided skill as "provided by Kronn", not as already integrated in the project', () => {
      expect(fr[`${R}builtin`]).toBe('fourni par Kronn');
      expect(Object.values(fr).filter(text => /intégré/.test(text) && /catalogue/i.test(text))).toEqual([]);
    });
  });

  describe('suggestions', () => {
    const suggestion = (id: string, name: string, reason: string) => skill({
      id, name, provenance: 'kronn', status: 'kronn_only', is_builtin: true, suggested: true, suggested_reason: reason,
    });

    it('say where they come from, once, under their heading', async () => {
      await show(listing({ skills_present: [suggestion('devops', 'DevOps', 'Dockerfile')] }));

      const group = document.querySelector('[role="rowgroup"][aria-label="' + R + 'skills.suggested"]') as HTMLElement;
      expect(within(group).getByText(`${R}skills.suggestedHint`)).toBeInTheDocument();
      expect(within(group).getByText(`${R}suggestedBecause:Dockerfile`)).toBeInTheDocument();
      expect(within(group).getByText(named('builtin'))).toBeInTheDocument();
    });

    it('run the detection again from the panel: the listing is read once more, ticked files stay ticked', async () => {
      await show(listing({
        skills_present: [suggestion('devops', 'DevOps', 'Dockerfile')],
        resources: [resource({ id: 'qp', name: 'Mine', kind: 'quick_prompt', status: 'kronn_only' })],
      }));
      openTab('automation');
      fireEvent.click(screen.getByRole('checkbox', { name: `${R}include:Mine` }));
      openTab('skills');
      repositoryResources.mockResolvedValue(listing({
        skills_present: [suggestion('devops', 'DevOps', 'Dockerfile'), suggestion('typescript', 'TypeScript', 'tsconfig.json')],
        resources: [resource({ id: 'qp', name: 'Mine', kind: 'quick_prompt', status: 'kronn_only' })],
      }));

      expect(screen.queryByText('TypeScript')).not.toBeInTheDocument();
      fireEvent.click(screen.getByRole('button', { name: `${R}skills.redetect` }));

      expect(await screen.findByText('TypeScript')).toBeInTheDocument();
      expect(repositoryResources).toHaveBeenCalledTimes(2);
      openTab('automation');
      expect(screen.getByRole('checkbox', { name: `${R}include:Mine` })).toBeChecked();
    });

    it('offer the detection only where there are suggestions', async () => {
      await show(listing({ skills_present: [skill({ id: 'kept', name: 'Kept', status: 'up_to_date', repository_paths: ['kronn/skills/kept/SKILL.md'] })] }));

      expect(screen.queryByRole('button', { name: `${R}skills.redetect` })).not.toBeInTheDocument();
    });
  });

  describe('the repository path never overlaps its note', () => {
    const LONG = 'kronn/workflows/pr-1897-v3-3-pack-context-review-of-the-whole-branch.yaml';
    const longRows = () => listing({
      resources: [
        resource({ id: 'w1', name: 'PR #1897 — v3.3 pack-context', kind: 'workflow', status: 'kronn_only', repository_paths: [LONG] }),
        resource({ id: 'w2', name: 'Written already', kind: 'workflow', status: 'up_to_date', repository_paths: [LONG.replace('1897', '1898')] }),
      ],
    });

    it('keeps the start and the end of the name as separate parts, the whole path in the tooltip', async () => {
      await show(longRows());
      openTab('automation');

      const path = within(rowOf('PR #1897 — v3.3 pack-context')).getByTitle(LONG);
      expect(path.querySelector('.rr-path-head')?.textContent?.startsWith('kronn/workflows/')).toBe(true);
      expect(path.querySelector('.rr-path-tail')?.textContent?.endsWith('branch.yaml')).toBe(true);
      expect(path).toHaveTextContent(LONG);
    });

    it('puts "will be created" on the same line as the path, as its own item', async () => {
      await show(longRows());
      openTab('automation');

      const line = within(rowOf('PR #1897 — v3.3 pack-context')).getByText(`${R}willBeCreated`).closest('.rr-path') as HTMLElement;
      expect(line.querySelector('code')).not.toContainElement(line.querySelector('.rr-path-note') as HTMLElement);
      expect(line.children).toHaveLength(2);
      const written = rowOf('Written already').querySelector('.rr-path') as HTMLElement;
      expect(written.children).toHaveLength(2);
    });

    it('lets only the head shrink, ellipsizing its end, and never lets the note or the tail wrap', () => {
      expect(panelCss).toMatch(/\.rr-path-head \{[^}]*flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap/);
      expect(panelCss).toMatch(/\.rr-path-tail \{[^}]*flex: none; white-space: nowrap/);
      expect(panelCss).toMatch(/\.rr-path-note \{[^}]*flex: none[^}]*white-space: nowrap/);
      expect(panelCss).toMatch(/\.rr-path > code \{[^}]*flex: 0 1 auto; min-width: 0; overflow: hidden/);
    });

    it('gives the path the full width of the row at 400 px, the sync pill going below it', () => {
      const rules = panelCss.slice(panelCss.indexOf('@media (max-width: 640px)'));
      expect(rules).toMatch(/grid-template-areas: 'select repository repository' 'select sync kronn' 'action action action'/);
    });
  });

  describe('sync pills', () => {
    it('are neutral and dashed when something exists on one side only', () => {
      expect(panelCss).toMatch(/\.rr-pill\[data-state='repository_only'\],\s*\.rr-pill\[data-state='kronn_only'\],\s*\.rr-pill\[data-state='native_skill'\],\s*\.rr-pill\[data-state='catalog'\] \{[^}]*border-style: dashed[^}]*var\(--kr-text-muted\)/);
    });

    it('keep blue for "newer", yellow for two versions and pink for approval', () => {
      expect(panelCss).toMatch(/\[data-state='repository_newer'\],\s*\.rr-pill\[data-state='kronn_newer'\] \{[^}]*var\(--kr-info\)/);
      expect(panelCss).toMatch(/\.rr-pill\[data-state='conflict'\] \{[^}]*var\(--kr-warning\)/);
      expect(panelCss).toMatch(/\.rr-pill\[data-state='approval_required'\] \{[^}]*var\(--kr-pink\)/);
    });

    it('are readable: 11 px at least', () => {
      expect(panelCss).toMatch(/\.rr-pill \{[^}]*font-size: var\(--kr-fs-sm\)/);
      expect(panelCss).not.toMatch(/\.rr-pill \{[^}]*--kr-fs-(2xs|xs)/);
    });
  });

  describe('write-blocked notice', () => {
    it.each([
      ['kronn_path_is_file'],
      ['repository_read_only'],
      ['repository_unreadable'],
    ] as const)('words %s from its code, never from a raw error', async reason => {
      await show(listing({ can_write_repository: false, can_write_repository_reason: reason }));

      const banner = document.querySelector('[data-banner="write-disabled"]') as HTMLElement;
      expect(within(banner).getByText(`${R}banner.writeDisabled.reason.${reason}`)).toBeInTheDocument();
      expect(banner).not.toHaveTextContent(/exists and is not a directory|cannot inspect/);
    });

    it('titles it at the size of the other section titles', () => {
      expect(panelCss).toMatch(/\.project-repository-resources-banner strong \{[^}]*font-size: 12px/);
      expect(panelCss).toMatch(/\.project-repository-resources h3 \{[^}]*font-size: 12px/);
    });
  });

  describe('fingerprints on the sheets', () => {
    it('shows the eight-character fingerprint of both sides when comparing', async () => {
      await show(listing({
        resources: [resource({
          id: 'wf-1', name: 'Nightly', slug: 'nightly', kind: 'workflow', status: 'conflict',
          repository_fingerprint: '1a2b3c4d', kronn_fingerprint: '9f8e7d6c',
        })],
      }));
      openTab('automation');

      fireEvent.click(actionIn(rowOf('Nightly'), 'compare'));
      const dialog = within(await screen.findByRole('dialog'));
      expect(dialog.getByTestId('fingerprint-repository')).toHaveTextContent('1a2b3c4d');
      expect(dialog.getByTestId('fingerprint-kronn')).toHaveTextContent('9f8e7d6c');
    });

    it('shows them when approving, and a dash for a side without a file', async () => {
      quickExecsList.mockResolvedValue([]);
      await show(listing({
        resources: [resource({
          id: 'qe-1', name: 'Lint', slug: 'lint', kind: 'quick_exec', status: 'approval_required',
          kronn_fingerprint: '0badc0de',
        })],
      }));
      openTab('automation');

      fireEvent.click(actionIn(rowOf('Lint'), 'approve'));
      const dialog = within(await screen.findByRole('dialog'));
      expect(dialog.getByTestId('fingerprint-kronn')).toHaveTextContent('0badc0de');
      expect(dialog.getByTestId('fingerprint-repository')).toHaveTextContent('—');
    });
  });

  describe('selection checkboxes', () => {
    const rule = /\.rr-cell-select input\[type='checkbox'\],\s*\.rr-align-list input\[type='checkbox'\] \{([^}]*)\}/;

    it('are drawn as a plain box, with a border and a transparent fill, in both themes', () => {
      const declarations = rule.exec(panelCss)?.[1] ?? '';
      expect(declarations).toMatch(/appearance: none/);
      expect(declarations).toMatch(/border: 1\.5px solid var\(--kr-text-muted\)/);
      expect(declarations).toMatch(/background: transparent/);
      expect(panelCss).not.toMatch(/\.rr-cell-select input[^{]*\{[^}]*accent-color/);
    });

    it('draw the tick with a border, so a locked ticked box is never a solid grey block', () => {
      expect(panelCss).toMatch(/input\[type='checkbox'\]:checked::after \{[^}]*content: ''[^}]*border: solid var\(--kr-accent-ink\)/);
      expect(panelCss).toMatch(/input\[type='checkbox'\]:disabled:checked::after \{[^}]*border-color: var\(--kr-text-faint\)/);
      expect(panelCss).not.toMatch(/input\[type='checkbox'\][^{]*\{[^}]*background: var\(--kr-(bg-hover|bg-input|text)/);
    });

    it('stay selection boxes: published rows locked and ticked, others free to tick', async () => {
      await show(listing({
        resources: [
          resource({ id: 'a', name: 'Private', kind: 'quick_prompt', status: 'kronn_only' }),
          resource({ id: 'b', name: 'Written', kind: 'quick_prompt', status: 'up_to_date' }),
        ],
      }));
      openTab('automation');

      expect(within(rowOf('Private')).getByRole('checkbox')).toBeEnabled();
      expect(within(rowOf('Written')).getByRole('checkbox')).toBeDisabled();
      expect(within(rowOf('Written')).getByRole('checkbox')).toBeChecked();
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

    it('keeps the search field at its own height instead of stretching it along the stacked toolbar', () => {
      const rules = mobile(panelCss);
      expect(rules).toMatch(/\.rr-toolbar \{ flex-direction: column; align-items: stretch; \}/);
      // A 200px flex-basis is a 200px tall field once the toolbar is a column.
      expect(rules).toMatch(/\.rr-search \{[^}]*flex: 0 0 auto[^}]*align-self: stretch[^}]*min-height: 44px/);
      expect(panelCss).toMatch(/\.rr-search \{[^}]*flex: 1 1 200px/);
      expect(rules.indexOf('.rr-search {')).toBeGreaterThan(rules.indexOf('.rr-toolbar {'));
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
