import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { BatchComparePanel } from '../BatchComparePanel';
import type { Discussion } from '../../types/generated';

function discussion(id: string, agent: Discussion['agent'], tier: Discussion['tier'], answer: string): Discussion {
  return {
    id,
    title: `Result ${id}`,
    agent,
    tier,
    awaiting_agent: false,
    messages: [{
      id: `m-${id}`,
      role: 'Agent',
      channel: 'main',
      content: answer,
      agent_type: agent,
      timestamp: '2026-08-16T10:00:00Z',
      tokens_used: 120,
      duration_ms: 2300,
    }],
  } as Discussion;
}

describe('BatchComparePanel', () => {
  it('renders two rich answers side-by-side and keeps each discussion openable', () => {
    const onOpenDiscussion = vi.fn();
    const t = (key: string, ...args: (string | number)[]) => `${key}${args.length ? ` ${args.join(' ')}` : ''}`;
    render(
      <BatchComparePanel
        label="Compare Jira"
        discussions={[
          discussion('disc-codex', 'Codex', 'default', '## Codex answer\n\n**Actionable**'),
          discussion('disc-claude', 'ClaudeCode', 'reasoning', '## Claude answer\n\n- Alternative'),
        ]}
        loading={false}
        error={null}
        runningIds={new Set()}
        onRefresh={vi.fn()}
        onOpenDiscussion={onOpenDiscussion}
        onClose={vi.fn()}
        t={t}
      />,
    );

    const columns = document.querySelector('.disc-compare-columns');
    expect(columns).toHaveAttribute('data-layout', 'split');
    expect(screen.getByRole('heading', { name: 'Codex answer' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Claude answer' })).toBeInTheDocument();
    expect(screen.getByText('Actionable')).toBeInTheDocument();

    const openButtons = screen.getAllByRole('button', { name: /disc.compare.openDiscussion/ });
    fireEvent.click(openButtons[1]);
    expect(onOpenDiscussion).toHaveBeenCalledWith('disc-claude');
  });

  it('shows a live generating state while a child is still running', () => {
    const pending = discussion('disc-running', 'Codex', 'default', '');
    pending.messages = pending.messages.filter(message => message.role !== 'Agent');
    pending.awaiting_agent = true;
    render(
      <BatchComparePanel
        label="Live compare"
        discussions={[pending]}
        loading={false}
        error={null}
        runningIds={new Set(['disc-running'])}
        onRefresh={vi.fn()}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={(key) => key}
      />,
    );

    expect(screen.getByText('disc.compare.generating')).toBeInTheDocument();
    expect(document.querySelector('.disc-compare-column')).toHaveAttribute('data-running', 'true');
  });
});
