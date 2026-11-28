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
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
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
import { SidePanelShell } from '../SidePanelShell';

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

  it('inside the side panel, hands its actions and the open path to the one header (KT-1884)', async () => {
    const { container } = wrap(
      <SidePanelShell
        ariaLabel="Panels"
        title="My discussion"
        railLabel="Panels"
        items={[{ id: 'git', label: 'Git', icon: null }]}
        activeId="git"
        onSelect={vi.fn()}
        onClose={vi.fn()}
        closeLabel="Close"
      >
        <GitPanel discussionId="d1" />
      </SidePanelShell>,
    );
    await screen.findByTestId('git-commit-history');
    const header = container.querySelector('.ui-drawer-header') as HTMLElement;
    expect(container.querySelector('.git-header')).toBeNull();
    expect(within(header).getByTestId('git-expand-toggle')).toBeInTheDocument();
    expect(header.querySelector('.git-icon-btn')).toBeInTheDocument();

    fireEvent.click(screen.getByText('backend/src/api/orchestration.rs'));
    await waitFor(() => expect(header.querySelector('.git-back-btn')).toBeInTheDocument());
    expect(header.querySelector('.side-panel-shell-meta')).toHaveTextContent('backend/src/api/orchestration.rs');
    expect(container.querySelectorAll('.git-back-btn')).toHaveLength(1);
    fireEvent.click(header.querySelector('.git-back-btn')!);
    await waitFor(() => expect(header.querySelector('.git-back-btn')).toBeNull());
    expect(header.querySelector('.git-icon-btn')).toBeInTheDocument();
  });

  it('inside the side panel, puts the way back of the diff on the left: first control of the header (KT-1884)', async () => {
    const { container } = wrap(
      <SidePanelShell
        ariaLabel="Panels"
        title="My discussion"
        railLabel="Panels"
        items={[{ id: 'git', label: 'Git', icon: null }]}
        activeId="git"
        onSelect={vi.fn()}
        onClose={vi.fn()}
        closeLabel="Close"
        menuLabel="More"
        menuActions={[{ id: 'copy', label: 'Copy id', icon: null, onSelect: vi.fn() }]}
      >
        <GitPanel discussionId="d1" />
      </SidePanelShell>,
    );
    await screen.findByTestId('git-commit-history');
    const header = container.querySelector('.ui-drawer-header') as HTMLElement;
    // The list view has no way back: the leading zone holds nothing.
    expect(header.querySelector('.side-panel-shell-view-leading')).toBeEmptyDOMElement();

    fireEvent.click(screen.getByText('backend/src/api/orchestration.rs'));
    const back = await waitFor(() => {
      const found = header.querySelector<HTMLElement>('.git-back-btn');
      expect(found).toBeInTheDocument();
      return found!;
    });
    // In the leading zone, in the identity, before the title; not among the actions.
    expect(back.closest('.side-panel-shell-view-leading')).not.toBeNull();
    expect(back.closest('.ui-header-identity')).not.toBeNull();
    expect(back.closest('.ui-header-actions')).toBeNull();
    const title = within(header).getByRole('heading', { name: 'My discussion' });
    expect(back.compareDocumentPosition(title) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // The first of every control of the header, so the first in tab order.
    const controls = [...header.querySelectorAll<HTMLElement>('button, a[href], input, select, textarea, [tabindex]')];
    expect(controls.length).toBeGreaterThan(3);
    expect(controls[0]).toBe(back);
    expect(within(header).getByTestId('side-panel-menu-btn')).toBeInTheDocument();

    // Back returns to the list, with the list's actions (expand, refresh) restored.
    fireEvent.click(back);
    await screen.findByTestId('git-commit-history');
    expect(header.querySelector('.git-back-btn')).toBeNull();
    expect(header.querySelector('.side-panel-shell-view-leading')).toBeEmptyDOMElement();
    expect(within(header).getByTestId('git-expand-toggle')).toBeInTheDocument();
    expect(header.querySelector('.git-icon-btn')).toBeInTheDocument();
  });

  it('on its own, keeps the way back before the path in its own header', async () => {
    const { container } = wrap(<GitPanel discussionId="d1" onClose={vi.fn()} />);
    await screen.findByTestId('git-commit-history');
    fireEvent.click(screen.getByText('backend/src/api/orchestration.rs'));
    const header = await waitFor(() => {
      const found = container.querySelector<HTMLElement>('.git-header');
      expect(found?.querySelector('.git-back-btn')).toBeTruthy();
      return found!;
    });
    expect(header.firstElementChild).toHaveClass('git-back-btn');
    expect(header.children[1]).toHaveClass('git-header-title');
  });
});
