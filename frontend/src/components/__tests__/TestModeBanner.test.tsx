// Banner pinned at the top while a discussion is in test mode. Guards
// against regressions in the single exit path — if this breaks, users
// get stuck with the main repo on a kronn branch and no visible way out.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { TestModeBanner } from '../TestModeBanner';
import type { Discussion } from '../../types/generated';

const t = (key: string, ...args: (string | number)[]) => {
  if (key === 'testMode.bannerRestore') return `restore:${args[0]}`;
  return key;
};

function disc(overrides: Partial<Discussion> = {}): Discussion {
  return {
    id: 'd-1', project_id: 'p-1', title: 'Switch theme tokens',
    agent: 'ClaudeCode' as any, language: 'en',
    participants: ['ClaudeCode' as any], messages: [], message_count: 0,
    archived: false, pinned: false,
    workspace_mode: 'Isolated',
    worktree_branch: 'kronn/switch-theme',
    test_mode_restore_branch: 'main',
    created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  };
}

describe('TestModeBanner', () => {
  it('shows the branch + restore target + disc title in the dev subline', () => {
    render(<TestModeBanner discussion={disc()} busy={false} onExit={() => {}} t={t} />);
    expect(screen.getByText('kronn/switch-theme')).toBeInTheDocument();
    expect(screen.getByText('restore:main')).toBeInTheDocument();
    expect(screen.getByText('Switch theme tokens')).toBeInTheDocument();
  });

  it('renders the primary headline using the testMode.bannerHeadline key', () => {
    render(<TestModeBanner discussion={disc()} busy={false} onExit={() => {}} t={t} />);
    expect(screen.getByText('testMode.bannerHeadline')).toBeInTheDocument();
  });

  it('calls onExit when the stop button is clicked', () => {
    const onExit = vi.fn();
    render(<TestModeBanner discussion={disc()} busy={false} onExit={onExit} t={t} />);
    fireEvent.click(screen.getByRole('button', { name: /testMode\.exit/ }));
    expect(onExit).toHaveBeenCalledTimes(1);
  });

  it('disables the exit button and swaps the label when busy', () => {
    render(<TestModeBanner discussion={disc()} busy={true} onExit={() => {}} t={t} />);
    const btn = screen.getByRole('button', { name: /testMode\.exiting/ });
    expect(btn).toBeDisabled();
  });

  it('falls back to em-dash when restore_branch is missing (detached-HEAD entry)', () => {
    render(<TestModeBanner discussion={disc({ test_mode_restore_branch: null })} busy={false} onExit={() => {}} t={t} />);
    expect(screen.getByText('restore:—')).toBeInTheDocument();
  });
});
