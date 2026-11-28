// KT-1955 — « Refuser » with the field left empty closes a card and does
// nothing else (the server turns it into `cancelled`); with a message it is
// the refusal the agent receives. One button, two outcomes.
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { questionsMock, declineMock, answerMock } = vi.hoisted(() => ({
  questionsMock: vi.fn(),
  declineMock: vi.fn(),
  answerMock: vi.fn(),
}));

vi.mock('../../lib/api', () => ({
  discussions: {
    questions: questionsMock,
    declineQuestion: declineMock,
    answerQuestion: answerMock,
  },
}));

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: Array<string | number>) =>
      (args.length ? `${key} ${args.join(' ')}` : key),
  }),
}));

import { DiscussionQuestionCard } from '../DiscussionQuestionCard';
import { DiscussionQuestionBanner } from '../DiscussionQuestionBanner';
import { resetDiscussionQuestions } from '../../lib/discussionQuestions';

function question(over: Partial<Record<string, unknown>> = {}) {
  return {
    id: 'q-1',
    discussion_id: 'd-1',
    source_message_id: 'm-1',
    fence_index: 0,
    key: 'settled-in-cli',
    question: 'Quelle politique de quota ?',
    context: null,
    td_errors: null,
    options: [{ id: 'a', label: 'Réarmer', description: null }],
    items: [],
    multiple: false,
    recommended_option_ids: [],
    task_ref: null,
    requester_workflow_step: null,
    resume: null,
    state: 'pending',
    answer: null,
    cancellation: null,
    created_at: '2026-11-18T08:00:00Z',
    updated_at: '2026-11-18T08:00:00Z',
    ...over,
  };
}

/** What the server returns for « Refuser » without a message. */
const closedSilently = (over: Partial<Record<string, unknown>> = {}) => question({
  state: 'cancelled',
  cancellation: {
    author_pseudo: 'Romu',
    cancelled_at: '2026-11-23T10:00:00Z',
    relayed_answers: false,
  },
  updated_at: '2026-11-23T10:00:00Z',
  ...over,
});

/** What the server returns for « Refuser » with a message. */
const declinedWithReason = () => question({
  state: 'declined',
  answer: {
    selected_option_ids: [],
    item_answers: [],
    text: 'Hors périmètre.',
    author_pseudo: 'Romu',
    answered_at: '2026-11-23T10:00:00Z',
    message_id: 'question-decline:q-1',
  },
  updated_at: '2026-11-23T10:00:00Z',
});

beforeEach(() => {
  vi.clearAllMocks();
  resetDiscussionQuestions();
  questionsMock.mockResolvedValue({ questions: [question()], pending_count: 1 });
});

afterEach(() => cleanup());

const renderCard = () => render(
  <DiscussionQuestionCard discussionId="d-1" sourceMessageId="m-1" fenceIndex={0} />,
);

describe('DiscussionQuestionCard — « Refuser » with or without a message (KT-1955)', () => {
  it('offers one « Refuser » button, described with both outcomes, and no « Annuler »', async () => {
    renderCard();
    const decline = await screen.findByTestId('disc-question-decline');
    expect(decline).toHaveTextContent('disc.question.decline');
    expect(decline).toHaveAttribute('title', 'disc.question.declineHint');
    expect(decline).toHaveAccessibleDescription('disc.question.declineHint');
    expect(screen.queryByTestId('disc-question-cancel')).toBeNull();
  });

  it('without a message: closes the card at once as refused, with who and when, and clears the banner', async () => {
    declineMock.mockResolvedValue(closedSilently());
    render(
      <>
        <DiscussionQuestionBanner discussionId="d-1" messageRevision="m-1" onNavigate={vi.fn()} />
        <DiscussionQuestionCard discussionId="d-1" sourceMessageId="m-1" fenceIndex={0} />
      </>,
    );
    expect(await screen.findByTestId('disc-question-banner')).toHaveTextContent(
      'disc.question.bannerPending 1',
    );

    fireEvent.click(screen.getByTestId('disc-question-decline'));

    const card = await screen.findByTestId('disc-question-q-1');
    await waitFor(() => expect(card).toHaveAttribute('data-state', 'cancelled'));
    expect(declineMock).toHaveBeenCalledTimes(1);
    expect(declineMock).toHaveBeenCalledWith('d-1', 'q-1', expect.objectContaining({ reason: null }));
    expect(answerMock).not.toHaveBeenCalled();
    expect(card).toHaveTextContent('disc.question.declinedTitle');
    expect(screen.getByTestId('disc-question-cancelled')).toHaveTextContent(
      'disc.question.declinedSilently',
    );
    expect(screen.getByTestId('disc-question-cancelled-by')).toHaveTextContent(
      `disc.question.declinedBy Romu ${new Date('2026-11-23T10:00:00Z').toLocaleString()}`,
    );
    expect(screen.queryByTestId('disc-question-send')).toBeNull();
    expect(screen.queryByTestId('disc-question-decline')).toBeNull();
    expect(screen.queryByTestId('disc-question-banner')).toBeNull();
  });

  it('with a message: sends it as the reason, and the card reads as a refusal the agent received', async () => {
    declineMock.mockResolvedValue(declinedWithReason());
    renderCard();
    fireEvent.change(await screen.findByTestId('disc-question-text'), {
      target: { value: '  Hors périmètre.  ' },
    });
    fireEvent.click(screen.getByTestId('disc-question-decline'));

    const card = screen.getByTestId('disc-question-q-1');
    await waitFor(() => expect(card).toHaveAttribute('data-state', 'declined'));
    expect(declineMock).toHaveBeenCalledWith(
      'd-1',
      'q-1',
      expect.objectContaining({ reason: 'Hors périmètre.' }),
    );
    expect(card).toHaveTextContent('Hors périmètre.');
    expect(screen.queryByTestId('disc-question-cancelled')).toBeNull();
  });

  it('posts once when « Refuser » is clicked twice in the same tick', async () => {
    let settle: (value: unknown) => void = () => {};
    declineMock.mockReturnValue(new Promise(resolve => { settle = resolve; }));
    renderCard();
    const decline = await screen.findByTestId('disc-question-decline');
    act(() => {
      fireEvent.click(decline);
      fireEvent.click(decline);
    });
    expect(declineMock).toHaveBeenCalledTimes(1);
    await act(async () => { settle(closedSilently()); });
  });

  it('keeps the card open and reads it back when the refusal is refused', async () => {
    declineMock.mockRejectedValue(new Error('This question has already been decided'));
    renderCard();
    fireEvent.click(await screen.findByTestId('disc-question-decline'));
    expect(await screen.findByTestId('disc-question-error')).toBeInTheDocument();
    await waitFor(() => expect(questionsMock).toHaveBeenCalledTimes(2));
  });

  // Several stale cards are refused in a row: the keyboard must not be sent
  // back to the top of the page each time, and the result must be announced.
  it.each([
    ['without a message', closedSilently, 'cancelled', ''],
    ['with a message', declinedWithReason, 'declined', 'Hors périmètre.'],
  ])('moves the focus to the closed card after « Refuser » %s', async (_, result, state, typed) => {
    declineMock.mockResolvedValue(result());
    renderCard();
    if (typed) {
      fireEvent.change(await screen.findByTestId('disc-question-text'), { target: { value: typed } });
    }
    const decline = await screen.findByTestId('disc-question-decline');
    decline.focus();
    expect(document.activeElement).toBe(decline);

    fireEvent.click(decline);

    const card = screen.getByTestId('disc-question-q-1');
    await waitFor(() => expect(card).toHaveAttribute('data-state', state));
    await waitFor(() => expect(document.activeElement).toBe(card));
    expect(card).toHaveAttribute('tabindex', '-1');
  });

  it('keeps the focus on « Refuser » when the refusal fails', async () => {
    declineMock.mockRejectedValue(new Error('network down'));
    render(
      <>
        <button type="button" data-testid="elsewhere">elsewhere</button>
        <DiscussionQuestionCard discussionId="d-1" sourceMessageId="m-1" fenceIndex={0} />
      </>,
    );
    const decline = await screen.findByTestId('disc-question-decline');
    decline.focus();

    fireEvent.click(decline);
    // Chromium's focus fixup drops the focus while the button is disabled in
    // flight. jsdom neither does that nor blurs a disabled element, so stand
    // the loss in by moving the focus away.
    const elsewhere = screen.getByTestId('elsewhere');
    elsewhere.focus();
    expect(document.activeElement).toBe(elsewhere);

    expect(await screen.findByTestId('disc-question-error')).toBeInTheDocument();
    await waitFor(() => expect(decline).not.toBeDisabled());
    await waitFor(() => expect(document.activeElement).toBe(decline));
    expect(screen.getByTestId('disc-question-q-1')).toHaveAttribute('data-state', 'pending');
  });

  it('says that what was parked on the card does not resume', async () => {
    questionsMock.mockResolvedValue({
      questions: [closedSilently({
        task_ref: 'KT-1955',
        resume: {
          workflow_id: 'resume-workflow',
          variables: {},
          state: 'pending',
          run_id: null,
          error: null,
        },
      })],
      pending_count: 0,
    });
    renderCard();
    expect(await screen.findByTestId('disc-question-cancelled-parked')).toHaveTextContent(
      'disc.question.declinedParked',
    );
  });

  // KT-1401 — the last card of an ended step's message: refusing it without a
  // message passed the earlier answers on. The card must not claim that
  // nothing was sent.
  it('says the earlier answers were passed on when the blank refusal relayed them', async () => {
    questionsMock.mockResolvedValue({
      questions: [closedSilently({
        cancellation: {
          author_pseudo: 'Romu',
          cancelled_at: '2026-11-23T10:00:00Z',
          relayed_answers: true,
        },
      })],
      pending_count: 0,
    });
    renderCard();
    const summary = await screen.findByTestId('disc-question-cancelled');
    expect(summary.querySelector('.disc-question-answer-text')?.textContent).toBe(
      'disc.question.declinedSilentlyRelayed',
    );
    expect(screen.getByTestId('disc-question-cancelled-by')).toHaveTextContent('Romu');
  });

  it('shows no parked note when nothing waited on the card', async () => {
    questionsMock.mockResolvedValue({ questions: [closedSilently()], pending_count: 0 });
    renderCard();
    expect(await screen.findByTestId('disc-question-cancelled')).toBeInTheDocument();
    expect(screen.queryByTestId('disc-question-cancelled-parked')).toBeNull();
  });
});
