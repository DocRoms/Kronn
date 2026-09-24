import { fireEvent, render, screen, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { StandaloneDiscussionMosaic } from '../StandaloneDiscussionMosaic';
import { useDiscussionMonitor } from '../../hooks/useDiscussionMonitor';
import type { DiscussionMonitorItem, DiscussionMonitorPreview } from '../../types/generated';
vi.mock('../../hooks/useDiscussionMonitor', () => ({ useDiscussionMonitor: vi.fn() }));
vi.mock('../../components/DiscussionMosaicComposer', () => ({
  DiscussionMosaicComposer: ({ discussionId, title }: { discussionId: string | null; title: string }) =>
    <div data-testid="mosaic-composer">{discussionId ?? 'none'}|{title}</div>,
}));
vi.mock('../../lib/I18nContext', () => ({ useT: () => ({ t: (key: string, ...args: (string | number)[]) => args.length ? `${key}:${args.join(',')}` : key }) }));
const preview = (title: string, overrides: Partial<DiscussionMonitorPreview> = {}): DiscussionMonitorPreview => ({
  title, shared_id: null, agent: 'Codex', connection_name: null, awaiting_agent: false, agent_running: false,
  progress_phase: null, pending_question_count: 0, updated_at: '2026-09-23T10:00:00Z',
  plan: { done: 2, in_progress: 1, ready: 2, blocked: 1, ideas: 0, later: 3 },
  messages: [{ id: 'm', role: 'Agent', channel: 'main', content: '**Hello** ![diagram](https://example.invalid/image.png)',
    truncated: false, agent_type: 'Codex', model: 'served-model', author_pseudo: null, author_cli_ordinal: 2, timestamp: '2026-09-23T10:00:00Z' }],
  partial_response: null, ...overrides,
});
function monitor(items: DiscussionMonitorItem[], error = false) {
  vi.mocked(useDiscussionMonitor).mockReturnValue({ items, error, updatedAt: 1, connectionState: 'connected', refresh: vi.fn() });
}
beforeEach(() => { vi.clearAllMocks(); window.location.hash = ''; });

describe('discussion mosaic', () => {
  it('selects one tile at a time and binds the composer to it', () => {
    monitor([{ id: 'a', preview: preview('Alpha'), error: null }, { id: 'b', preview: preview('Beta'), error: null }]);
    render(<StandaloneDiscussionMosaic discussionIds={['a', 'b']} layout="two-columns" />);
    expect(screen.getByTestId('mosaic-composer')).toHaveTextContent('none|');
    fireEvent.click(screen.getByRole('button', { name: 'disc.mosaic.replyIn:Alpha' }));
    expect(screen.getByRole('article', { name: 'Alpha' })).toHaveAttribute('data-selected', 'true');
    expect(screen.getByTestId('mosaic-composer')).toHaveTextContent('a|Alpha');
    fireEvent.click(within(screen.getByRole('article', { name: 'Beta' })).getByText('disc.mosaic.previewHint'));
    expect(screen.getByRole('article', { name: 'Alpha' })).not.toHaveAttribute('data-selected');
    expect(screen.getByRole('button', { name: 'disc.mosaic.replyIn:Beta' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByTestId('mosaic-composer')).toHaveTextContent('b|Beta');
  });
  it('shows plan progress, author provenance and isolated missing discussion without executable content', () => {
    monitor([{ id: 'a', preview: preview('Alpha', { pending_question_count: 1 }), error: null }, { id: 'b', preview: null, error: 'not_found' }]);
    const previous = document.title;
    const view = render(<StandaloneDiscussionMosaic discussionIds={['a', 'b']} layout="two-columns" />);
    const alpha = within(screen.getByRole('article', { name: 'Alpha' }));
    expect(alpha.getByText('disc.mosaic.question')).toBeInTheDocument();
    expect(alpha.getByText('disc.mosaic.plan:2,6')).toBeInTheDocument();
    expect(alpha.getByRole('progressbar')).toHaveAttribute('value', '2');
    expect(alpha.getByText('disc.mosaic.planDetails:1,2,1,0')).toBeInTheDocument();
    expect(alpha.getByText('disc.mosaic.later:3')).toBeInTheDocument();
    expect(alpha.getByText('Codex · CLI 2')).toBeInTheDocument();
    expect(alpha.getByText('served-model')).toBeInTheDocument();
    expect(alpha.getByText('Hello').tagName).toBe('STRONG');
    expect(view.container.querySelector('img, iframe, textarea')).toBeNull();
    expect(alpha.getByRole('link', { name: 'Alpha' })).toHaveAttribute('target', '_blank');
    expect(within(screen.getByRole('article', { name: 'b' })).getByRole('alert')).toHaveTextContent('disc.mosaic.notFound');
    view.unmount(); expect(document.title).toBe(previous);
  });
  it('preserves independent scroll positions, resumes following and persists the chosen layout in the URL', () => {
    const first = preview('Alpha'); const second = preview('Beta');
    monitor([{ id: 'a', preview: first, error: null }, { id: 'b', preview: second, error: null }]);
    const view = render(<StandaloneDiscussionMosaic discussionIds={['a', 'b']} layout="two-columns" />);
    const a = screen.getByLabelText('disc.mosaic.messages:Alpha'); const b = screen.getByLabelText('disc.mosaic.messages:Beta');
    for (const element of [a, b]) {
      Object.defineProperties(element, { scrollHeight: { configurable: true, value: 1000 }, clientHeight: { configurable: true, value: 200 } });
    }
    fireEvent.scroll(a, { target: { scrollTop: 100 } });
    monitor([{ id: 'a', preview: { ...first, updated_at: 'new' }, error: null }, { id: 'b', preview: { ...second, updated_at: 'new' }, error: null }]);
    view.rerender(<StandaloneDiscussionMosaic discussionIds={['a', 'b']} layout="two-columns" />);
    expect(a.scrollTop).toBe(100); expect(b.scrollTop).toBe(1000);
    expect(screen.getAllByRole('button', { name: 'disc.mosaic.follow' })).toHaveLength(1);
    fireEvent.click(screen.getByRole('button', { name: 'disc.mosaic.follow' }));
    expect(a.scrollTop).toBe(1000);
    fireEvent.change(screen.getByLabelText('pages.mosaic.chooseLayout'), { target: { value: 'two-rows' } });
    expect(window.location.hash).toContain('discussion=a&discussion=b&layout=two-rows');
    expect(view.container.querySelector('.discussion-mosaic-grid')).toHaveAttribute('data-layout', 'two-rows');
  });
  it('shows partial checkpoints and their bounded tail, keeps previous content after refresh failure', () => {
    const base = preview('Alpha');
    monitor([{ id: 'a', preview: { ...base, agent_running: true, progress_phase: 'upstream_wait', partial_response: { ...base.messages[0], id: 'partial', content: 'latest answer', truncated: true } }, error: null }], true);
    render(<StandaloneDiscussionMosaic discussionIds={['a', 'b']} layout="auto" />);
    expect(screen.getByText('disc.mosaic.checkpoint')).toBeInTheDocument();
    expect(screen.getByText('disc.mosaic.tail')).toBeInTheDocument();
    expect(screen.getByText('latest answer')).toBeInTheDocument();
    expect(screen.getByText('disc.mosaic.upstream')).toBeInTheDocument();
    expect(screen.getByText('disc.mosaic.refreshError')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'disc.mosaic.refresh' }));
    expect(vi.mocked(useDiscussionMonitor).mock.results[0].value.refresh).toHaveBeenCalledOnce();
  });
});
