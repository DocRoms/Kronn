import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProjectRepositoryResources } from '../../types/generated';

const repositoryResources = vi.hoisted(() => vi.fn());

vi.mock('../../lib/api', () => ({
  projects: { repositoryResources },
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
  beforeEach(() => repositoryResources.mockReset());

  it('previews the files selected for the first publication', async () => {
    repositoryResources.mockResolvedValue({
      kronn_exists: false,
      resources: [{
        id: 'qp-1',
        name: 'Review ticket',
        slug: 'review-ticket',
        kind: 'quick_prompt',
        level: 'usable_without_kronn',
        status: 'not_published',
        repository_paths: ['kronn/prompts/review-ticket.md'],
      }],
    } satisfies ProjectRepositoryResources);

    render(<ProjectRepositoryResourcesPanel projectId="project-1" />);

    expect(await screen.findByText('projects.repositoryResources.kronnMissing')).toBeInTheDocument();
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
  });

  it('keeps published resources checked and exposes their level and repository status', async () => {
    repositoryResources.mockResolvedValue({
      kronn_exists: true,
      resources: [
        {
          id: 'wf-1', name: 'Daily report', slug: 'daily-report', kind: 'workflow',
          level: 'kronn_required', status: 'up_to_date',
          repository_paths: ['kronn/workflows/daily-report.yaml'],
        },
        {
          id: 'artifact-1', name: 'Health dashboard', slug: 'health-dashboard', kind: 'artifact',
          level: 'kronn_required', status: 'conflict',
          repository_paths: [
            'kronn/artifacts/health-dashboard/artifact.yaml',
            'kronn/artifacts/health-dashboard/index.html',
          ],
        },
      ],
    } satisfies ProjectRepositoryResources);

    render(<ProjectRepositoryResourcesPanel projectId="project-1" />);

    const daily = await screen.findByText('Daily report');
    const dailyRow = daily.closest('.project-repository-resource-row') as HTMLElement;
    expect(within(dailyRow).getByRole('checkbox')).toBeChecked();
    expect(within(dailyRow).getByRole('checkbox')).toBeDisabled();
    expect(dailyRow).toHaveTextContent('projects.repositoryResources.level.kronn_required');
    expect(dailyRow).toHaveTextContent('projects.repositoryResources.status.up_to_date');

    const artifactRow = screen.getByText('Health dashboard').closest('.project-repository-resource-row') as HTMLElement;
    expect(within(artifactRow).getByRole('checkbox')).toBeChecked();
    expect(within(artifactRow).getByRole('checkbox')).toBeDisabled();
    expect(artifactRow).toHaveTextContent('projects.repositoryResources.status.conflict');
    expect(screen.getAllByText('!')).toHaveLength(2);
  });
});
