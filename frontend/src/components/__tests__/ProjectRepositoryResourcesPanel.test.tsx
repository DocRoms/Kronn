import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  ProjectRepositoryResource,
  ProjectRepositoryResources,
  ProjectRepositorySkill,
} from '../../types/generated';

type ResourceDefaults = 'adr_level' | 'field_diff' | 'file_diffs' | 'write_preview' | 'required_secrets';
type SkillDefaults = 'file_diffs' | 'repository_paths_diverge' | 'write_preview' | 'referenced'
  | 'required_secrets' | 'adr_level';
type ListingFixture = Omit<
  ProjectRepositoryResources,
  'resources' | 'skills_present' | 'skills_available' | 'can_write_repository' | 'uncommitted_managed_paths'
> & {
  resources: Array<Omit<ProjectRepositoryResource, ResourceDefaults> & Partial<Pick<ProjectRepositoryResource, ResourceDefaults>>>;
  skills_present: Array<Omit<ProjectRepositorySkill, SkillDefaults> & Partial<Pick<ProjectRepositorySkill, SkillDefaults>>>;
  skills_available: Array<Omit<ProjectRepositorySkill, SkillDefaults> & Partial<Pick<ProjectRepositorySkill, SkillDefaults>>>;
};

const listing = (fixture: ListingFixture): ProjectRepositoryResources => {
  const skill = (item: ListingFixture['skills_present'][number]): ProjectRepositorySkill => ({
    file_diffs: [], repository_paths_diverge: false, write_preview: [], referenced: false,
    required_secrets: [], adr_level: 'N1', ...item,
  });
  return {
    can_write_repository: true,
    uncommitted_managed_paths: [],
    ...fixture,
    resources: fixture.resources.map(item => ({
      adr_level: 'N1', field_diff: [], file_diffs: [], write_preview: [], required_secrets: [], ...item,
    })),
    skills_present: fixture.skills_present.map(skill),
    skills_available: fixture.skills_available.map(skill),
  };
};

const repositoryResources = vi.hoisted(() => vi.fn());
const publishRepositoryResource = vi.hoisted(() => vi.fn());
const importRepositoryResource = vi.hoisted(() => vi.fn());
const approveRepositoryResource = vi.hoisted(() => vi.fn());

vi.mock('../../lib/api', () => ({
  projects: {
    repositoryResources,
    publishRepositoryResource,
    importRepositoryResource,
    approveRepositoryResource,
  },
}));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: Array<string | number>) => args.reduce<string>(
      (label, arg, index) => label.replace(`{${index}}`, String(arg)),
      key,
    ),
  }),
}));

import { ProjectRepositoryResourcesPanel } from '../ProjectRepositoryResourcesPanel';

describe('ProjectRepositoryResourcesPanel', () => {
  beforeEach(() => {
    repositoryResources.mockReset();
    publishRepositoryResource.mockReset();
    importRepositoryResource.mockReset();
    approveRepositoryResource.mockReset();
    localStorage.removeItem('kronn:projectRepositoryResourcesTab');
  });

  it('lists native skill folders before a secondary kronn/ notice', async () => {
    repositoryResources.mockResolvedValue(listing({
      kronn_exists: false,
      skill_roots: [
        { path: '.agents/skills', skill_count: 3 },
        { path: '.github/skills', skill_count: 1 },
      ],
      skills_present: [],
      skills_available: [],
      resources: [],
    }));

    render(<ProjectRepositoryResourcesPanel projectId="project-1" />);

    const roots = await screen.findByTestId('project-skill-roots');
    expect(within(roots).getByText('.agents/skills/')).toBeInTheDocument();
    expect(within(roots).getByText('.github/skills/')).toBeInTheDocument();
    expect(within(roots).getAllByText('projects.repositoryResources.skillRoots.count')).toHaveLength(2);
    const notice = screen.getByText(/projects\.repositoryResources\.kronnMissingSecondary/).closest('[role="status"]');
    expect(notice).toHaveAttribute('data-tone', 'secondary');
    expect(screen.queryByText('projects.repositoryResources.kronnMissing')).not.toBeInTheDocument();
  });

  it('says so when the repository has no native skill folder', async () => {
    repositoryResources.mockResolvedValue(listing({
      kronn_exists: false,
      skill_roots: [],
      skills_present: [],
      skills_available: [],
      resources: [],
    }));

    render(<ProjectRepositoryResourcesPanel projectId="project-1" />);

    expect(await screen.findByText('projects.repositoryResources.skillRoots.none')).toBeInTheDocument();
    expect(screen.getByText(/projects\.repositoryResources\.kronnMissing$/).closest('[role="status"]')).not.toHaveAttribute('data-tone');
  });

  it('previews the files selected for the first publication', async () => {
    repositoryResources.mockResolvedValue(listing({
      kronn_exists: false,
      skill_roots: [],
      skills_present: [],
      skills_available: [],
      resources: [{
        id: 'qp-1',
        name: 'Review ticket',
        slug: 'review-ticket',
        kind: 'quick_prompt',
        level: 'usable_without_kronn',
        status: 'kronn_only',
        approval_required: false,
        approved: false,
        repository_paths: ['kronn/prompts/review-ticket.md'],
      }],
    }));

    render(<ProjectRepositoryResourcesPanel projectId="project-1" />);

    expect(await screen.findByText('projects.repositoryResources.kronnMissing')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('tab', { name: /projects\.repositoryResources\.tab\.automation/ }));
    const checkbox = screen.getByRole('checkbox', {
      name: 'projects.repositoryResources.include',
    });
    expect(checkbox).not.toBeChecked();
    expect(checkbox).toBeEnabled();

    const scaffold = screen.getByText('INDEX.md · kronn.toml · kronn.lock').closest('.project-repository-tree-entry');
    const root = screen.getByText('kronn/').closest('.project-repository-tree-root');
    const prompt = screen.getByText('prompts/review-ticket.md').closest('.project-repository-tree-entry');
    expect(root).toHaveAttribute('data-excluded', 'true');
    expect(scaffold).toHaveAttribute('data-excluded', 'true');
    expect(prompt).toHaveAttribute('data-excluded', 'true');

    fireEvent.click(checkbox);
    await waitFor(() => {
      expect(root).not.toHaveAttribute('data-excluded');
      expect(scaffold).not.toHaveAttribute('data-excluded');
      expect(prompt).not.toHaveAttribute('data-excluded');
      expect(root?.querySelector('.project-repository-tree-marker')).toHaveTextContent('+');
      expect(scaffold?.querySelector('.project-repository-tree-marker')).toHaveTextContent('+');
      expect(prompt?.querySelector('.project-repository-tree-marker')).toHaveTextContent('+');
    });

    publishRepositoryResource.mockResolvedValue({});
    repositoryResources.mockResolvedValue(listing({
      kronn_exists: true,
      skill_roots: [],
      skills_present: [],
      skills_available: [],
      resources: [{
        id: 'qp-1', name: 'Review ticket', slug: 'review-ticket', kind: 'quick_prompt',
        level: 'usable_without_kronn', status: 'up_to_date', approval_required: false,
        approved: true, repository_paths: ['kronn/prompts/review-ticket.md'],
      }],
    }));
    fireEvent.click(screen.getByRole('button', {
      name: 'projects.repositoryResources.publishSelected',
    }));
    await waitFor(() => expect(publishRepositoryResource).toHaveBeenCalledWith('project-1', {
      kind: 'quick_prompt', id: 'qp-1', overwrite_repository_changes: false,
    }));
  });

  it('offers repository import and hash-bound approval as separate actions', async () => {
    const imported = listing({
      kronn_exists: true,
      skill_roots: [],
      skills_present: [],
      skills_available: [],
      resources: [{
        id: 'repository:quick_exec:lint', name: 'Lint', slug: 'lint', kind: 'quick_exec',
        level: 'usable_without_kronn', status: 'repository_only', approval_required: false,
        approved: false, repository_paths: ['kronn/quick-execs/lint.yaml'],
      }],
    });
    const awaitingApproval = listing({
      ...imported,
      resources: [{
        ...imported.resources[0], id: 'qe-1', status: 'approval_required',
        approval_required: true,
      }],
    });
    const approved = listing({
      ...awaitingApproval,
      resources: [{
        ...awaitingApproval.resources[0], status: 'up_to_date', approval_required: false, approved: true,
      }],
    });
    repositoryResources
      .mockResolvedValueOnce(imported)
      .mockResolvedValueOnce(awaitingApproval)
      .mockResolvedValueOnce(approved);
    importRepositoryResource.mockResolvedValue({});
    approveRepositoryResource.mockResolvedValue({});

    render(<ProjectRepositoryResourcesPanel projectId="project-1" />);
    fireEvent.click(await screen.findByRole('tab', { name: /projects\.repositoryResources\.tab\.automation/ }));
    fireEvent.click(screen.getByRole('button', { name: /projects\.repositoryResources\.import/ }));
    await waitFor(() => expect(importRepositoryResource).toHaveBeenCalledWith('project-1', {
      kind: 'quick_exec', slug: 'lint', overwrite_kronn_changes: false,
    }));
    fireEvent.click(await screen.findByRole('button', { name: /projects\.repositoryResources\.approve/ }));
    await waitFor(() => expect(approveRepositoryResource).toHaveBeenCalledWith('project-1', {
      kind: 'quick_exec', id: 'qe-1',
    }));
  });

  it('keeps published resources checked and exposes their level and repository status', async () => {
    repositoryResources.mockResolvedValue(listing({
      kronn_exists: true,
      skill_roots: [],
      skills_present: [],
      skills_available: [],
      resources: [
        {
          id: 'wf-1', name: 'Daily report', slug: 'daily-report', kind: 'workflow',
          level: 'kronn_required', status: 'up_to_date',
          approval_required: false, approved: true,
          repository_paths: ['kronn/workflows/daily-report.yaml'],
        },
        {
          id: 'artifact-1', name: 'Health dashboard', slug: 'health-dashboard', kind: 'artifact',
          level: 'kronn_required', status: 'conflict',
          approval_required: false, approved: false, diff: '--- repository\n+++ Kronn',
          repository_paths: [
            'kronn/artifacts/health-dashboard/artifact.yaml',
            'kronn/artifacts/health-dashboard/index.html',
          ],
        },
      ],
    }));

    render(<ProjectRepositoryResourcesPanel projectId="project-1" />);

    fireEvent.click(await screen.findByRole('tab', { name: /projects\.repositoryResources\.tab\.automation/ }));
    const daily = await screen.findByText('Daily report');
    const dailyRow = daily.closest('.project-repository-resource-row') as HTMLElement;
    expect(within(dailyRow).getByRole('checkbox')).toBeChecked();
    expect(within(dailyRow).getByRole('checkbox')).toBeDisabled();
    expect(dailyRow).toHaveTextContent('projects.repositoryResources.level.kronn_required');
    expect(dailyRow).toHaveTextContent('projects.repositoryResources.status.up_to_date');

    fireEvent.click(screen.getByRole('tab', { name: /projects\.repositoryResources\.tab\.artifacts/ }));
    const artifactRow = screen.getByText('Health dashboard').closest('.project-repository-resource-row') as HTMLElement;
    expect(within(artifactRow).getByRole('checkbox')).toBeChecked();
    expect(within(artifactRow).getByRole('checkbox')).toBeDisabled();
    expect(artifactRow).toHaveTextContent('projects.repositoryResources.status.conflict');
    expect(screen.getAllByText('!')).toHaveLength(2);
  });

  it('separates present and available skills, groups automations, and remembers the sub-tab', async () => {
    repositoryResources.mockResolvedValue(listing({
      kronn_exists: true,
      skill_roots: [],
      skills_present: [{
        id: 'rust',
        name: 'Rust',
        slug: 'rust',
        description: 'Rust engineering',
        provenance: 'both',
        is_builtin: true,
        status: 'up_to_date',
        approval_required: false,
        approved: true,
        repository_paths: ['kronn/skills/rust/SKILL.md'],
        publication_path: 'kronn/skills/rust/SKILL.md',
      }],
      skills_available: [{
        id: 'custom-review',
        name: 'Review',
        slug: 'custom-review',
        description: 'Review changes',
        provenance: 'kronn',
        is_builtin: false,
        approval_required: false,
        approved: false,
        repository_paths: [],
        publication_path: 'kronn/skills/custom-review/SKILL.md',
      }],
      resources: [
        {
          id: 'wf-1', name: 'Nightly', slug: 'nightly', kind: 'workflow',
          level: 'kronn_required', status: 'kronn_only',
          approval_required: false, approved: false,
          repository_paths: ['kronn/workflows/nightly.yaml'],
        },
        {
          id: 'qp-1', name: 'Review ticket', slug: 'review-ticket', kind: 'quick_prompt',
          level: 'usable_without_kronn', status: 'kronn_only',
          approval_required: false, approved: false,
          repository_paths: ['kronn/prompts/review-ticket.md'],
        },
        {
          id: 'artifact-1', name: 'Dashboard', slug: 'dashboard', kind: 'artifact',
          level: 'kronn_required', status: 'kronn_only',
          approval_required: false, approved: false,
          repository_paths: ['kronn/artifacts/dashboard/index.html'],
        },
      ],
    }));

    const first = render(<ProjectRepositoryResourcesPanel projectId="project-1" />);

    expect(await screen.findByText('Rust')).toBeInTheDocument();
    expect(screen.getByText('Review')).toBeInTheDocument();
    expect(screen.getByText('Rust').closest('.project-repository-resource-row'))
      .toHaveTextContent('projects.repositoryResources.provenance.both');
    expect(screen.getByRole('tab', { name: /projects\.repositoryResources\.tab\.skills 1/ }))
      .toHaveAttribute('aria-selected', 'true');

    fireEvent.click(screen.getByRole('tab', { name: /projects\.repositoryResources\.tab\.automation 2/ }));
    expect(screen.getByText('Nightly')).toBeInTheDocument();
    expect(screen.getByText('Review ticket')).toBeInTheDocument();
    expect(document.querySelector('[data-resource-kind="workflow"]')).toBeInTheDocument();
    expect(document.querySelector('[data-resource-kind="quick_prompt"]')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('tab', { name: /projects\.repositoryResources\.tab\.artifacts 1/ }));
    expect(screen.getByText('Dashboard')).toBeInTheDocument();
    first.unmount();

    render(<ProjectRepositoryResourcesPanel projectId="project-1" />);
    expect(await screen.findByRole('tab', { name: /projects\.repositoryResources\.tab\.artifacts 1/ }))
      .toHaveAttribute('aria-selected', 'true');
  });
});
