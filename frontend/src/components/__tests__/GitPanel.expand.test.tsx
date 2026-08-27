// GitPanel — expand-view regression (0.11.0).
//
// Expanding the panel from the list view used to auto-open the first
// file's diff (`GitPanel.tsx`, removed effect), which swapped the whole
// component onto the diff-viewer branch and lost the workspace recap +
// itemized commit history — they only exist in the list-view branch.
// Locks two paths:
//   - expanding the list view (no file selected yet) keeps recap + commits
//     + files visible, and does NOT jump into a diff
//   - clicking a file explicitly still opens its diff, expanded or not
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import type { ReactElement } from 'react';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock({
    discussions: {
      gitStatus: vi.fn().mockResolvedValue({
        branch: 'feat/0.11.0-task-orchestration',
        default_branch: 'main',
        is_default_branch: false,
        files: [],
        committed_files: [
          { path: 'backend/src/api/orchestration.rs', status: 'modified', staged: false },
        ],
        commits: [
          {
            sha: '017b91d6full',
            short_sha: '017b91d6',
            subject: 'KT-410: orchestration HTTP launch persists principal validations',
            author_name: 'Romuald Priol',
            author_time: 1755000000,
          },
        ],
        workspace: {
          workspace_id: 'ws-1',
          ownership: 'managed',
          state: 'attached',
          path: '/repo/.kronn/worktrees/task-kt-410',
          branch: 'kronn/task/kt-410',
          base_sha: 'base123',
          head_sha: '017b91d6full',
          integrated_sha: null,
          task_execution_id: 'exec-1',
          task_reference: 'KT-410',
        },
        empty_reason: null,
        ahead: 1,
        behind: 0,
        has_upstream: false,
        provider: 'unknown',
        pr_url: null,
      }),
      gitDiff: vi.fn().mockResolvedValue({
        path: 'backend/src/api/orchestration.rs',
        diff: '--- a/orchestration.rs\n+++ b/orchestration.rs\n@@ -1 +1 @@\n-old\n+new\n',
      }),
      workspaces: vi.fn().mockResolvedValue([]),
    },
  });
});

import { GitPanel } from '../GitPanel';

const wrap = (ui: ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

describe('GitPanel — expand view', () => {
  it('expanding the list view keeps recap + commits + files, and does not auto-open a diff', async () => {
    const { container } = wrap(
      <GitPanel discussionId="d1" onClose={vi.fn()} />,
    );

    await screen.findByTestId('git-commit-history');
    expect(container.querySelector('.git-workspace-provenance')).toBeTruthy();
    expect(container.querySelector('.git-back-btn')).toBeNull();

    fireEvent.click(screen.getByTestId('git-expand-toggle'));

    // Give any (incorrect) auto-navigation effect a chance to fire.
    await waitFor(() => {
      expect(container.querySelector('[data-expanded="true"]')).toBeTruthy();
    });

    expect(container.querySelector('.git-back-btn')).toBeNull();
    expect(screen.getByTestId('git-commit-history')).toBeTruthy();
    expect(container.querySelector('.git-workspace-provenance')).toBeTruthy();
    expect(screen.getByText('backend/src/api/orchestration.rs')).toBeTruthy();
  });

  it('clicking a file explicitly still opens its diff', async () => {
    const { container } = wrap(
      <GitPanel discussionId="d1" onClose={vi.fn()} />,
    );

    await screen.findByTestId('git-commit-history');
    fireEvent.click(screen.getByText('backend/src/api/orchestration.rs'));

    await waitFor(() => {
      expect(container.querySelector('.git-back-btn')).toBeTruthy();
    });
    await waitFor(() => {
      expect(container.querySelector('.git-diff-pre')?.textContent).toContain('old');
    });
    expect(container.querySelector('.git-diff-pre')?.textContent).toContain('new');
  });
});
