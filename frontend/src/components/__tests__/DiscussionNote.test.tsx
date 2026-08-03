import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { DiscussionMessage } from '../../types/generated';
import { DiscussionNote } from '../DiscussionNote';

vi.mock('../MessageBubble', () => ({
  MarkdownContent: ({ content }: { content: string }) => <div>{content}</div>,
}));

const note: DiscussionMessage = {
  id: '12345678-abcd-4abc-8abc-1234567890ab',
  role: 'User',
  channel: 'note',
  content: 'Décision à garder pour plus tard',
  agent_type: null,
  timestamp: '2026-07-31T08:00:00Z',
  tokens_used: 0,
  auth_mode: null,
  model_tier: null,
  author_pseudo: 'Romuald',
  author_avatar_email: null,
};

const t = (key: string) => ({
  'disc.note.label': 'Note',
  'disc.note.human': 'humain',
  'disc.note.agent': 'agent',
}[key] ?? key);

describe('DiscussionNote', () => {
  beforeEach(() => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
    });
  });

  it('renders a compact collapsed note at its timeline position', () => {
    const { container } = render(
      <DiscussionNote message={note} discussionId="disc-1" t={t} />,
    );

    expect(screen.getByText('Note')).toBeInTheDocument();
    expect(screen.getByText('Romuald')).toBeInTheDocument();
    expect(container.querySelector('details')).not.toHaveAttribute('open');
    expect(container.querySelector('[data-message-id="12345678-abcd-4abc-8abc-1234567890ab"]'))
      .toBeInTheDocument();
  });

  it('copies the durable message id from the same animated pill as messages', async () => {
    render(<DiscussionNote message={note} discussionId="disc-1" t={t} />);

    fireEvent.click(screen.getByRole('button', { name: 'disc.idPillTooltip' }));

    expect(navigator.clipboard.writeText).toHaveBeenCalledWith(note.id);
    expect(await screen.findByRole('button', { name: 'disc.idPillTooltip' }))
      .toHaveAttribute('data-copied', 'true');
  });
});
