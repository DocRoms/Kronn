// KT-619 — what the reader actually sees, and what the bar promises them.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { StrictMode } from 'react';
import { act, render, screen, waitFor, cleanup } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ImportantMessage } from '../../types/generated';

const importantMessages = vi.fn();

vi.mock('../../lib/api', () => ({
  discussions: {
    get importantMessages() {
      return importantMessages;
    },
  },
}));

vi.mock('../../lib/I18nContext', () => ({
  // Echo the key plus its arguments: a test that asserts on English prose
  // starts failing the day somebody rewords a label, which teaches nothing.
  useT: () => ({
    t: (key: string, ...args: string[]) => [key, ...args].join('|'),
  }),
}));

import { ImportantMessageCard, ImportantMessagesBar } from '../ImportantMessageCard';
import { clearImportantMessages, refreshImportantMessages } from '../../lib/importantMessages';

const DISC = 'd-1';

function card(overrides: Partial<ImportantMessage> = {}): ImportantMessage {
  return {
    id: 'i-1',
    discussion_id: DISC,
    message_id: 'm-1',
    category: 'decision',
    schema_version: 1,
    dedup_key: 'kt-619.ship',
    title: 'Cible de la 0.13.0',
    highlight: 'La release part sans KT-610.',
    context: null,
    impact: 'Les captures arrivent en 0.13.1.',
    action_required: { required: false },
    references: {},
    author_kind: 'orchestrator',
    author_label: 'Codex',
    source_kind: null,
    source_id: null,
    created_at: '2026-09-07T19:00:00Z',
    sort_order: 1,
    ...overrides,
  } as ImportantMessage;
}

function serve(items: ImportantMessage[], totalAll = items.length) {
  importantMessages.mockResolvedValue({ items, total: items.length, total_all: totalAll });
}

beforeEach(() => {
  importantMessages.mockReset();
  // Drop the cache WITHOUT fetching: reloading here would race the mock this
  // test has not armed yet, and freeze the store on an empty result.
  clearImportantMessages(DISC);
});

afterEach(() => {
  cleanup();
});

describe('ImportantMessageCard', () => {
  it('renders the durable row: highlight, category, author and impact', async () => {
    serve([card()]);
    render(<ImportantMessageCard discussionId={DISC} sourceMessageId="m-1" />);

    expect(await screen.findByText('La release part sans KT-610.')).toBeInTheDocument();
    expect(screen.getByText('disc.important.category.decision')).toBeInTheDocument();
    expect(screen.getByText(/disc\.important\.author\.orchestrator/)).toBeInTheDocument();
    expect(screen.getByText('Les captures arrivent en 0.13.1.')).toBeInTheDocument();
  });

  it('says an explicit "no action" instead of leaving the field blank', async () => {
    serve([card()]);
    render(<ImportantMessageCard discussionId={DISC} sourceMessageId="m-1" />);
    expect(await screen.findByText('disc.important.actionNone')).toBeInTheDocument();
  });

  it('shows the action with its owner and due date when one is required', async () => {
    serve([
      card({
        action_required: { required: true, action: 'Trancher', owner: 'Romu', due: 'lundi' },
      }),
    ]);
    render(<ImportantMessageCard discussionId={DISC} sourceMessageId="m-1" />);
    expect(await screen.findByText('Trancher · Romu · lundi')).toBeInTheDocument();
  });

  it('tells the reader when a fence produced no row, rather than showing raw JSON', async () => {
    serve([]);
    render(<ImportantMessageCard discussionId={DISC} sourceMessageId="m-missing" />);
    expect(await screen.findByText('disc.important.missingTitle')).toBeInTheDocument();
    expect(screen.getByText('disc.important.missingHint')).toBeInTheDocument();
  });

  it('carries the category on the element so the colour is the category', async () => {
    serve([card({ category: 'blocking_alert' })]);
    const { container } = render(
      <ImportantMessageCard discussionId={DISC} sourceMessageId="m-1" />,
    );
    await screen.findByText('La release part sans KT-610.');
    expect(container.querySelector('[data-category="blocking_alert"]')).not.toBeNull();
  });

  it('lists only the references that are set', async () => {
    serve([card({ references: { task_ref: 'KT-619', dod_id: null, agent: 'Codex' } })]);
    render(<ImportantMessageCard discussionId={DISC} sourceMessageId="m-1" />);
    expect(await screen.findByText(/disc\.important\.ref\.task_ref: KT-619/)).toBeInTheDocument();
    expect(screen.getByText(/disc\.important\.ref\.agent: Codex/)).toBeInTheDocument();
    expect(screen.queryByText(/disc\.important\.ref\.dod_id/)).toBeNull();
  });
});

describe('ImportantMessagesBar', () => {
  const three = [
    card({ id: 'i-1', message_id: 'm-1', sort_order: 1 }),
    card({ id: 'i-2', message_id: 'm-2', sort_order: 2, category: 'blocking_alert' }),
    card({ id: 'i-3', message_id: 'm-3', sort_order: 3 }),
  ];

  it('stays out of the way when the discussion has no important message', async () => {
    serve([]);
    const { container } = render(<ImportantMessagesBar discussionId={DISC} />);
    await waitFor(() => expect(importantMessages).toHaveBeenCalled());
    expect(container.querySelector('.disc-important-bar')).toBeNull();
  });

  it('counts the discussion, not the rows it happens to hold', async () => {
    // `total` and `total_all` differ on purpose: reading the wrong one is the
    // mistake this test exists to catch, and identical values would hide it.
    serve(three, 7);
    render(<ImportantMessagesBar discussionId={DISC} />);
    expect(await screen.findByText('disc.important.count|7')).toBeInTheDocument();
  });

  it('keeps counting every card while a filter is active', async () => {
    serve(three, 7);
    render(<ImportantMessagesBar discussionId={DISC} />);
    await screen.findByText('disc.important.count|7');

    await userEvent.selectOptions(
      screen.getByLabelText('disc.important.filterLabel'),
      'blocking_alert',
    );

    // Filtering to one card must not make the chip claim the room only ever
    // had one; only the position changes.
    expect(screen.getByText('disc.important.count|7')).toBeInTheDocument();
    expect(screen.getByText('disc.important.position|1|1')).toBeInTheDocument();
  });

  it('navigates to the exact card, through the transcript\'s own jump', async () => {
    serve(three);
    const onNavigate = vi.fn();
    render(<ImportantMessagesBar discussionId={DISC} onNavigate={onNavigate} />);
    expect(await screen.findByText('disc.important.position|1|3')).toBeInTheDocument();

    // The target matters, not just the counter: a pager that moves its label
    // while jumping to the wrong message is worse than no pager.
    await userEvent.click(screen.getByLabelText('disc.important.next'));
    expect(onNavigate).toHaveBeenLastCalledWith('m-2');
    expect(screen.getByText('disc.important.position|2|3')).toBeInTheDocument();

    await userEvent.click(screen.getByLabelText('disc.important.next'));
    expect(onNavigate).toHaveBeenLastCalledWith('m-3');

    await userEvent.click(screen.getByLabelText('disc.important.previous'));
    expect(onNavigate).toHaveBeenLastCalledWith('m-2');
    expect(screen.getByText('disc.important.position|2|3')).toBeInTheDocument();
  });

  it('reaches the only card there is, when both arrows are disabled', async () => {
    serve([card({ id: 'i-9', message_id: 'm-9' })]);
    const onNavigate = vi.fn();
    render(<ImportantMessagesBar discussionId={DISC} onNavigate={onNavigate} />);
    await screen.findByText('disc.important.count|1');

    // With one card there is no previous and no next; without the counter
    // being actionable, nothing would reach it at all.
    expect(screen.getByLabelText('disc.important.previous')).toBeDisabled();
    expect(screen.getByLabelText('disc.important.next')).toBeDisabled();
    await userEvent.click(screen.getByLabelText('disc.important.goToCurrent'));
    expect(onNavigate).toHaveBeenCalledWith('m-9');
  });

  it('reaches the single result a filter leaves behind', async () => {
    serve(three);
    const onNavigate = vi.fn();
    render(<ImportantMessagesBar discussionId={DISC} onNavigate={onNavigate} />);
    await screen.findByText('disc.important.count|3');

    await userEvent.selectOptions(
      screen.getByLabelText('disc.important.filterLabel'),
      'blocking_alert',
    );
    await userEvent.click(screen.getByLabelText('disc.important.goToCurrent'));
    expect(onNavigate).toHaveBeenCalledWith('m-2');
  });

  it('refetches when the transcript grows, so a new card is not missed', async () => {
    serve([card({ id: 'i-1', message_id: 'm-1' })]);
    const { rerender } = render(
      <ImportantMessagesBar discussionId={DISC} messageRevision="m-1" />,
    );
    expect(await screen.findByText('disc.important.count|1')).toBeInTheDocument();
    expect(importantMessages).toHaveBeenCalledTimes(1);

    // A card published after the first GET must not stay invisible until a
    // reload: the newest message id changing is the signal to look again.
    serve(three);
    rerender(<ImportantMessagesBar discussionId={DISC} messageRevision="m-3" />);
    expect(await screen.findByText('disc.important.count|3')).toBeInTheDocument();
    expect(importantMessages).toHaveBeenCalledTimes(2);
  });

  it('does not refetch while the transcript is unchanged', async () => {
    serve(three);
    const { rerender } = render(
      <ImportantMessagesBar discussionId={DISC} messageRevision="m-3" />,
    );
    await screen.findByText('disc.important.count|3');
    rerender(<ImportantMessagesBar discussionId={DISC} messageRevision="m-3" />);
    rerender(<ImportantMessagesBar discussionId={DISC} messageRevision="m-3" />);
    // One fetch per arrival, never one per render or per bubble.
    expect(importantMessages).toHaveBeenCalledTimes(1);
  });

  it('keeps each room on its own current card when switching away and back', async () => {
    const other = 'd-navigation-other';
    clearImportantMessages(other);
    importantMessages.mockImplementation(async (id: string) => {
      const items = id === DISC ? three : [card({ discussion_id: other, message_id: 'other-message' })];
      return { items, total: items.length, total_all: items.length };
    });
    const onNavigate = vi.fn();
    const { rerender } = render(<ImportantMessagesBar discussionId={DISC} onNavigate={onNavigate} />);
    await screen.findByText('disc.important.position|1|3');
    await userEvent.click(screen.getByLabelText('disc.important.next'));
    await userEvent.click(screen.getByLabelText('disc.important.next'));
    expect(screen.getByText('disc.important.position|3|3')).toBeVisible();
    rerender(<ImportantMessagesBar discussionId={other} onNavigate={onNavigate} />);
    await screen.findByText('disc.important.position|1|1');
    expect(screen.getByLabelText('disc.important.previous')).toBeDisabled();
    rerender(<ImportantMessagesBar discussionId={DISC} onNavigate={onNavigate} />);
    await screen.findByText('disc.important.position|3|3');
    await userEvent.click(screen.getByLabelText('disc.important.goToCurrent'));
    expect(onNavigate).toHaveBeenLastCalledWith('m-3');
  });

  it('restores a room filter without applying it to another room', async () => {
    const other = 'd-filter-other';
    clearImportantMessages(other);
    importantMessages.mockImplementation(async (id: string) => {
      const items = id === DISC ? three : [card({ discussion_id: other, message_id: 'other-message' })];
      return { items, total: items.length, total_all: items.length };
    });
    const { rerender } = render(<ImportantMessagesBar discussionId={DISC} />);
    await screen.findByText('disc.important.position|1|3');
    await userEvent.selectOptions(screen.getByLabelText('disc.important.filterLabel'), 'blocking_alert');
    rerender(<ImportantMessagesBar discussionId={other} />);
    await screen.findByText('disc.important.position|1|1');
    expect(screen.getByLabelText('disc.important.filterLabel')).toHaveValue('');
    rerender(<ImportantMessagesBar discussionId={DISC} />);
    expect(screen.getByLabelText('disc.important.filterLabel')).toHaveValue('blocking_alert');
    expect(screen.getByText('disc.important.position|1|1')).toBeVisible();
  });

  it('bounds the cursor after a refresh removes its selected card without moving the transcript', async () => {
    serve(three);
    const onNavigate = vi.fn();
    render(<ImportantMessagesBar discussionId={DISC} onNavigate={onNavigate} />);
    await screen.findByText('disc.important.position|1|3');
    await userEvent.click(screen.getByLabelText('disc.important.next'));
    await userEvent.click(screen.getByLabelText('disc.important.next'));
    serve([three[0]]);
    act(() => refreshImportantMessages(DISC));
    await screen.findByText('disc.important.position|1|1');
    expect(screen.getByLabelText('disc.important.previous')).toBeDisabled();
    expect(screen.getByLabelText('disc.important.next')).toBeDisabled();
    expect(onNavigate).toHaveBeenCalledTimes(2);
  });

  it('tracks the selected message rather than its old array index after a refresh', async () => {
    serve(three);
    const onNavigate = vi.fn();
    render(<ImportantMessagesBar discussionId={DISC} onNavigate={onNavigate} />);
    await screen.findByText('disc.important.position|1|3');
    await userEvent.click(screen.getByLabelText('disc.important.next'));
    serve([card({ id: 'i-older', message_id: 'm-older' }), ...three]);
    act(() => refreshImportantMessages(DISC));
    await screen.findByText('disc.important.position|3|4');
    await userEvent.click(screen.getByLabelText('disc.important.goToCurrent'));
    expect(onNavigate).toHaveBeenLastCalledWith('m-2');
  });

  it('does not lose a publication invalidation while the first snapshot is in flight', async () => {
    let resolveFirst!: (value: unknown) => void;
    importantMessages.mockImplementationOnce(() => new Promise(resolve => { resolveFirst = resolve; }));
    serve(three);
    const { rerender } = render(<ImportantMessagesBar discussionId={DISC} messageRevision="before" />);
    await waitFor(() => expect(importantMessages).toHaveBeenCalledTimes(1));
    rerender(<ImportantMessagesBar discussionId={DISC} messageRevision="after" />);
    act(() => {
      refreshImportantMessages(DISC);
      refreshImportantMessages(DISC);
    });
    await act(async () => resolveFirst({ items: [], total: 0, total_all: 0 }));
    await screen.findByText('disc.important.count|3');
    expect(importantMessages).toHaveBeenCalledTimes(2);
  });

  it('cannot walk past either end', async () => {
    serve(three);
    render(<ImportantMessagesBar discussionId={DISC} />);
    await screen.findByText('disc.important.position|1|3');

    // At the first card there is no previous; at the last there is no next.
    expect(screen.getByLabelText('disc.important.previous')).toBeDisabled();
    await userEvent.click(screen.getByLabelText('disc.important.next'));
    await userEvent.click(screen.getByLabelText('disc.important.next'));
    expect(screen.getByLabelText('disc.important.next')).toBeDisabled();
  });

  it('resets the cursor when the filter changes, rather than landing anywhere', async () => {
    serve(three);
    render(<ImportantMessagesBar discussionId={DISC} />);
    await screen.findByText('disc.important.position|1|3');

    await userEvent.click(screen.getByLabelText('disc.important.next'));
    expect(screen.getByText('disc.important.position|2|3')).toBeInTheDocument();

    await userEvent.selectOptions(
      screen.getByLabelText('disc.important.filterLabel'),
      'blocking_alert',
    );
    expect(screen.getByText('disc.important.position|1|1')).toBeInTheDocument();
  });

  it('says so when a category holds nothing, and disables both directions', async () => {
    serve([card({ category: 'decision' })]);
    render(<ImportantMessagesBar discussionId={DISC} />);
    await screen.findByText('disc.important.count|1');

    await userEvent.selectOptions(
      screen.getByLabelText('disc.important.filterLabel'),
      'dod_waiver',
    );
    expect(screen.getByText('disc.important.noneInFilter')).toBeInTheDocument();
    expect(screen.getByLabelText('disc.important.next')).toBeDisabled();
    expect(screen.getByLabelText('disc.important.previous')).toBeDisabled();
  });

  it('is operable from the keyboard alone', async () => {
    serve(three);
    const onNavigate = vi.fn();
    render(<ImportantMessagesBar discussionId={DISC} onNavigate={onNavigate} />);
    await screen.findByText('disc.important.count|3');

    // Every enabled control must be reachable by tabbing, in reading order.
    // A pager only usable with a mouse is not a pager for the people most
    // likely to need it.
    await userEvent.tab();
    expect(screen.getByLabelText('disc.important.goToCurrent')).toHaveFocus();
    await userEvent.tab();
    expect(screen.getByLabelText('disc.important.filterLabel')).toHaveFocus();

    // `previous` is disabled on the first card, so the browser skips it —
    // that is correct, and the point is that it comes BACK once usable.
    await userEvent.tab();
    expect(screen.getByLabelText('disc.important.next')).toHaveFocus();

    // Activating with the keyboard must do exactly what a click does.
    await userEvent.keyboard('{Enter}');
    expect(onNavigate).toHaveBeenLastCalledWith('m-2');

    await userEvent.tab({ shift: true });
    expect(screen.getByLabelText('disc.important.previous')).toHaveFocus();
    await userEvent.keyboard('{Enter}');
    expect(onNavigate).toHaveBeenLastCalledWith('m-1');
  });

  it('announces the position to a screen reader as it moves', async () => {
    serve(three);
    render(<ImportantMessagesBar discussionId={DISC} onNavigate={vi.fn()} />);
    const position = await screen.findByText('disc.important.position|1|3');

    // Without a live region the position changes silently, which is the same
    // as not being there for anyone not watching that corner of the screen.
    expect(position).toHaveAttribute('aria-live', 'polite');
    await userEvent.click(screen.getByLabelText('disc.important.next'));
    expect(screen.getByText('disc.important.position|2|3')).toHaveAttribute(
      'aria-live',
      'polite',
    );
  });

  it('names the group and every control, so none is an unlabelled icon', async () => {
    serve(three);
    render(<ImportantMessagesBar discussionId={DISC} />);
    await screen.findByText('disc.important.count|3');

    expect(screen.getByRole('group', { name: 'disc.important.barLabel' })).toBeInTheDocument();
    // The arrows are icon-only: their accessible name is the only name they have.
    for (const label of [
      'disc.important.goToCurrent',
      'disc.important.filterLabel',
      'disc.important.previous',
      'disc.important.next',
    ]) {
      expect(screen.getByLabelText(label)).toBeInTheDocument();
    }
  });

  it('leaves the thread readable when the cards cannot be loaded', async () => {
    importantMessages.mockRejectedValue(new Error('offline'));
    const { container } = render(<ImportantMessagesBar discussionId={DISC} />);
    await waitFor(() => expect(importantMessages).toHaveBeenCalled());
    expect(container.querySelector('.disc-important-bar')).toBeNull();
  });

  it('fetches once for several consumers of the same discussion', async () => {
    serve(three);
    render(
      <>
        <ImportantMessagesBar discussionId={DISC} />
        <ImportantMessageCard discussionId={DISC} sourceMessageId="m-1" />
        <ImportantMessageCard discussionId={DISC} sourceMessageId="m-2" />
      </>,
    );
    await screen.findByText('disc.important.count|3');
    expect(importantMessages).toHaveBeenCalledTimes(1);
  });

  it('coalesces the initial read when StrictMode replays mount effects', async () => {
    serve(three);
    render(<StrictMode><ImportantMessagesBar discussionId={DISC} messageRevision="m-3" /></StrictMode>);
    await screen.findByText('disc.important.count|3');
    expect(importantMessages).toHaveBeenCalledTimes(1);
  });
});
