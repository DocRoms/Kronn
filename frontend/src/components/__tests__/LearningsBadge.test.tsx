import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';

const { learningsApi } = vi.hoisted(() => ({
  learningsApi: {
    pending: vi.fn(),
    list: vi.fn(),
    validate: vi.fn(),
    reject: vi.fn(),
  },
}));

vi.mock('../../lib/api', () => ({ learnings: learningsApi }));

import { LearningsBadge } from '../LearningsBadge';

const t = (key: string) => key;

const learning = (id: string) => ({
  id,
  claim: `claim ${id}`,
  evidence: [{ kind: 'user', ref: 'user:2026-06-01' }],
  kind: 'preference' as const,
  status: 'pending' as const,
  created_at: '2026-06-01T00:00:00Z',
});

describe('LearningsBadge', () => {
  beforeEach(() => {
    cleanup();
    vi.clearAllMocks();
    learningsApi.pending.mockResolvedValue({ count: 0 });
    learningsApi.list.mockResolvedValue([]);
    learningsApi.validate.mockResolvedValue(undefined);
    learningsApi.reject.mockResolvedValue(undefined);
  });

  it('renders nothing when there are 0 pending', async () => {
    const { container } = render(<LearningsBadge t={t} toast={vi.fn()} pollMs={0} />);
    await waitFor(() => expect(learningsApi.pending).toHaveBeenCalled());
    expect(container.querySelector('.learnings-badge')).toBeNull();
  });

  it('shows the count and opens the modal with the pending list', async () => {
    learningsApi.pending.mockResolvedValue({ count: 2 });
    learningsApi.list.mockResolvedValue([learning('a'), learning('b')]);
    render(<LearningsBadge t={t} toast={vi.fn()} pollMs={0} />);
    await waitFor(() => expect(screen.getByText('2')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: 'disc.learningsBadgeTitle' }));
    await waitFor(() => expect(screen.getByText('claim a')).toBeInTheDocument());
    expect(screen.getByText('claim b')).toBeInTheDocument();
  });

  it('validate removes the row, decrements the count and toasts', async () => {
    const toast = vi.fn();
    learningsApi.pending.mockResolvedValue({ count: 1 });
    learningsApi.list.mockResolvedValue([learning('x')]);
    render(<LearningsBadge t={t} toast={toast} pollMs={0} />);
    await waitFor(() => expect(screen.getByText('1')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: 'disc.learningsBadgeTitle' }));
    await waitFor(() => expect(screen.getByText('claim x')).toBeInTheDocument());
    fireEvent.click(screen.getByText('disc.learningValidate'));
    expect(learningsApi.validate).toHaveBeenCalledWith('x');
    await waitFor(() => expect(toast).toHaveBeenCalledWith('disc.learningValidated', 'success'));
    expect(screen.queryByText('claim x')).toBeNull();
  });
});
