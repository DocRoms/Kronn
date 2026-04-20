// Preflight modal — regression tests for the action matrix. If the
// per-kind button set drifts, users end up with wrong remediation options
// (e.g. "Stash and proceed" on WorktreeDirty would destroy agent work).

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { TestModeModal } from '../TestModeModal';
import type { TestModeBlocker } from '../../types/extensions';

const t = (key: string, ...args: (string | number)[]) => {
  if (key === 'testMode.modal.mainDirtyTitle') return `dirty on ${args[0]}`;
  if (key === 'testMode.modal.filesMore') return `+${args[0]} more`;
  return key;
};

function makeBlocker(kind: string, details?: TestModeBlocker['details']): TestModeBlocker {
  return { status: 'blocked', kind, message: `blocker:${kind}`, details: details ?? null };
}

function renderModal(blocker: TestModeBlocker, overrides: Partial<Parameters<typeof TestModeModal>[0]> = {}) {
  const props = {
    blocker,
    busy: false,
    onRetry: vi.fn(),
    onGoCommit: vi.fn(),
    onCancel: vi.fn(),
    t,
    ...overrides,
  };
  render(<TestModeModal {...props} />);
  return props;
}

describe('TestModeModal', () => {
  it('WorktreeDirty: only exposes "open git panel" + cancel (no stash, no proceed)', () => {
    const props = renderModal(makeBlocker('WorktreeDirty', { files: [{ path: 'a.ts', status: ' M' }] }));
    expect(screen.getByRole('button', { name: /openGitPanel/ })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /stashAndProceed/ })).toBeNull();
    expect(screen.queryByRole('button', { name: /proceedAnyway/ })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /openGitPanel/ }));
    expect(props.onGoCommit).toHaveBeenCalled();
  });

  it('MainDirty: shows all three remediation buttons + puts current_branch in the title', () => {
    renderModal(makeBlocker('MainDirty', {
      files: [{ path: 'foo.rs', status: ' M' }],
      current_branch: 'feature/x',
    }));
    expect(screen.getByText('dirty on feature/x')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /stashAndProceed/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /commitFirst/ })).toBeInTheDocument();
    // `cancel` matches both the header X (title) and the footer button —
    // we only need to prove the footer one exists, so scope by class.
    const cancels = screen.getAllByRole('button', { name: /cancel/ });
    expect(cancels.length).toBeGreaterThanOrEqual(1);
    expect(cancels.some((b) => b.className.includes('test-mode-modal-btn'))).toBe(true);
  });

  it('MainDirty: "Stash and proceed" retries with stash_dirty=true', () => {
    const props = renderModal(makeBlocker('MainDirty', { files: [] }));
    fireEvent.click(screen.getByRole('button', { name: /stashAndProceed/ }));
    expect(props.onRetry).toHaveBeenCalledWith({ stash_dirty: true });
  });

  it('Detached: only shows "proceed anyway" (force=true) + cancel', () => {
    const props = renderModal(makeBlocker('Detached'));
    expect(screen.getByRole('button', { name: /proceedAnyway/ })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /stashAndProceed/ })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /proceedAnyway/ }));
    expect(props.onRetry).toHaveBeenCalledWith({ force: true });
  });

  it('Unknown kind falls back to a cancel-only modal with the server message', () => {
    renderModal(makeBlocker('SomeFuture' as any));
    // Unknown kinds still have 2 cancel controls (header X + footer button).
    // We only care that NO action buttons are offered.
    expect(screen.queryByRole('button', { name: /stashAndProceed/ })).toBeNull();
    expect(screen.queryByRole('button', { name: /proceedAnyway/ })).toBeNull();
    expect(screen.queryByRole('button', { name: /openGitPanel/ })).toBeNull();
    expect(screen.queryByRole('button', { name: /commitFirst/ })).toBeNull();
    // Server message is surfaced so the user gets SOMETHING actionable.
    expect(screen.getByText('blocker:SomeFuture')).toBeInTheDocument();
  });

  it('truncates the file list to 8 and surfaces a "+N more" counter', () => {
    const files = Array.from({ length: 12 }, (_, i) => ({ path: `f${i}.rs`, status: ' M' }));
    renderModal(makeBlocker('MainDirty', { files }));
    // First 8 paths rendered, last 4 collapsed into the "+4 more" line.
    expect(screen.getByText('f0.rs')).toBeInTheDocument();
    expect(screen.getByText('f7.rs')).toBeInTheDocument();
    expect(screen.queryByText('f8.rs')).toBeNull();
    expect(screen.getByText('+4 more')).toBeInTheDocument();
  });

  it('clicking the backdrop cancels; clicking the dialog does not', () => {
    const props = renderModal(makeBlocker('MainDirty'));
    // Dialog click should NOT trigger cancel (stopPropagation in the markup).
    fireEvent.click(screen.getByRole('dialog'));
    expect(props.onCancel).not.toHaveBeenCalled();
  });

  it('busy disables the footer action buttons', () => {
    renderModal(makeBlocker('MainDirty'), { busy: true });
    // The three footer buttons must be disabled to avoid double-submits.
    // In busy state the primary button swaps its label to `stashing…` (so
    // we match that text, not `stashAndProceed`). The header close button
    // is intentionally left enabled so the user can always escape.
    expect(screen.getByRole('button', { name: /stashing/ })).toBeDisabled();
    expect(screen.getByRole('button', { name: /commitFirst/ })).toBeDisabled();
    const footerCancel = screen.getAllByRole('button').find(
      (b) => b.className.includes('test-mode-modal-btn') && /testMode\.modal\.cancel/.test(b.textContent ?? '')
    );
    expect(footerCancel).toBeDefined();
    expect(footerCancel!.hasAttribute('disabled')).toBe(true);
  });
});
