import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { questionsMock, answerMock } = vi.hoisted(() => ({
  questionsMock: vi.fn(),
  answerMock: vi.fn(),
}));

vi.mock('../../lib/api', () => ({
  discussions: { questions: questionsMock, answerQuestion: answerMock },
}));

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: Array<string | number>) =>
      (args.length ? `${key} ${args.join(' ')}` : key),
  }),
}));

import { DiscussionQuestionCard } from '../DiscussionQuestionCard';
import { refreshDiscussionQuestions, resetDiscussionQuestions } from '../../lib/discussionQuestions';

function question(over: Partial<Record<string, unknown>> = {}) {
  return {
    id: 'q-1',
    discussion_id: 'd-1',
    source_message_id: 'm-1',
    fence_index: 0,
    key: 'quota-policy',
    question: 'Comment fermer une escalade ?',
    context: 'Huit exécutions bloquent la délégation.',
    options: [
      { id: 'a', label: 'Suivre le statut de la tâche', description: 'done/archived libère' },
      { id: 'b', label: 'Réarmement explicite', description: null },
    ],
    multiple: false,
    recommended_option_ids: ['a'],
    task_ref: 'KT-593',
    state: 'pending',
    answer: null,
    created_at: '2026-09-06T08:00:00Z',
    updated_at: '2026-09-06T08:00:00Z',
    ...over,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  resetDiscussionQuestions();
  questionsMock.mockResolvedValue({ questions: [question()], pending_count: 1 });
});

afterEach(() => cleanup());

const renderCard = (fenceIndex: number | undefined = 0, messageId = 'm-1') => render(
  <DiscussionQuestionCard discussionId="d-1" sourceMessageId={messageId} fenceIndex={fenceIndex} />,
);

describe('DiscussionQuestionCard', () => {
  /// KT-595 — the whole point: a decision that stays put. It renders the
  /// durable row, not the fence, so it survives a reload and a restart.
  it('shows the pending question, what it blocks, and its options', async () => {
    renderCard();
    expect(await screen.findByTestId('disc-question-q-1')).toHaveAttribute('data-state', 'pending');
    expect(screen.getByText('Comment fermer une escalade ?')).toBeInTheDocument();
    expect(screen.getByText('Huit exécutions bloquent la délégation.')).toBeInTheDocument();
    expect(screen.getByTestId('disc-question-task')).toHaveTextContent('KT-593');
  });

  /// A recommendation is a hint, never a decision already taken. A box the
  /// human did not tick must never read back as their answer.
  it('marks the recommended option without selecting it', async () => {
    renderCard();
    await screen.findByTestId('disc-question-q-1');
    expect(screen.getByText('disc.question.recommended')).toBeInTheDocument();
    expect(screen.getByTestId('disc-question-option-a')).not.toBeChecked();
    expect(screen.getByTestId('disc-question-option-b')).not.toBeChecked();
    expect(screen.getByTestId('disc-question-send')).toBeDisabled();
  });

  it('sends the chosen option and shows the answer in its place', async () => {
    const answered = question({
      state: 'answered',
      answer: {
        selected_option_ids: ['a'],
        text: null,
        author_pseudo: 'Romu - mac',
        answered_at: '2026-09-06T09:00:00Z',
        message_id: 'm-2',
      },
    });
    answerMock.mockResolvedValue(answered);

    renderCard();
    await screen.findByTestId('disc-question-q-1');
    fireEvent.click(screen.getByTestId('disc-question-option-a'));
    fireEvent.click(screen.getByTestId('disc-question-send'));

    await waitFor(() => expect(answerMock).toHaveBeenCalledWith('d-1', 'q-1', expect.objectContaining({
      selected_option_ids: ['a'],
      text: null,
    })));
    expect(await screen.findByTestId('disc-question-answer')).toHaveTextContent('Romu - mac');
    // Answered, the form is gone: there is nothing left to decide here.
    expect(screen.queryByTestId('disc-question-send')).toBeNull();
  });

  /// Free text is always offered, options or not — the right answer is
  /// regularly one nobody thought to list.
  it('accepts a written answer with no option chosen', async () => {
    answerMock.mockResolvedValue(question({
      state: 'answered',
      answer: {
        selected_option_ids: [],
        text: 'Ni l’un ni l’autre : on expire au bout de 24 h.',
        author_pseudo: 'Romu - mac',
        answered_at: '2026-09-06T09:00:00Z',
        message_id: 'm-2',
      },
    }));

    renderCard();
    await screen.findByTestId('disc-question-q-1');
    fireEvent.change(screen.getByTestId('disc-question-text'), {
      target: { value: '  Ni l’un ni l’autre  ' },
    });
    fireEvent.click(screen.getByTestId('disc-question-send'));

    await waitFor(() => expect(answerMock).toHaveBeenCalledWith('d-1', 'q-1', expect.objectContaining({
      selected_option_ids: [],
      text: 'Ni l’un ni l’autre',
    })));
  });

  it('replaces the choice when only one is allowed, and accumulates when several are', async () => {
    renderCard();
    await screen.findByTestId('disc-question-q-1');
    fireEvent.click(screen.getByTestId('disc-question-option-a'));
    fireEvent.click(screen.getByTestId('disc-question-option-b'));
    expect(screen.getByTestId('disc-question-option-a')).not.toBeChecked();
    expect(screen.getByTestId('disc-question-option-b')).toBeChecked();

    cleanup();
    resetDiscussionQuestions();
    questionsMock.mockResolvedValue({
      questions: [question({ multiple: true })],
      pending_count: 1,
    });
    renderCard();
    await screen.findByTestId('disc-question-q-1');
    fireEvent.click(screen.getByTestId('disc-question-option-a'));
    fireEvent.click(screen.getByTestId('disc-question-option-b'));
    expect(screen.getByTestId('disc-question-option-a')).toBeChecked();
    expect(screen.getByTestId('disc-question-option-b')).toBeChecked();
  });

  /// A lost response must not record a second decision, and a corrected answer
  /// must not be swallowed as a duplicate of the first. The key settles both.
  it('keeps the idempotency key across a retry and renews it when the answer changes', async () => {
    answerMock.mockRejectedValueOnce(new Error('réseau'));
    answerMock.mockResolvedValue(question({
      state: 'answered',
      answer: {
        selected_option_ids: ['b'],
        text: null,
        author_pseudo: 'Romu - mac',
        answered_at: '2026-09-06T09:00:00Z',
        message_id: 'm-2',
      },
    }));

    renderCard();
    await screen.findByTestId('disc-question-q-1');
    fireEvent.click(screen.getByTestId('disc-question-option-a'));
    fireEvent.click(screen.getByTestId('disc-question-send'));
    await screen.findByTestId('disc-question-error');

    // Same answer, second attempt: the same key, so the server can recognise
    // the retry rather than writing a second decision.
    fireEvent.click(screen.getByTestId('disc-question-send'));
    await waitFor(() => expect(answerMock).toHaveBeenCalledTimes(2));
    const [, , firstBody] = answerMock.mock.calls[0];
    const [, , retryBody] = answerMock.mock.calls[1];
    expect(retryBody.idempotency_key).toBe(firstBody.idempotency_key);
  });

  it('renews the key once the answer itself changes', async () => {
    answerMock.mockRejectedValue(new Error('réseau'));
    renderCard();
    await screen.findByTestId('disc-question-q-1');

    fireEvent.click(screen.getByTestId('disc-question-option-a'));
    fireEvent.click(screen.getByTestId('disc-question-send'));
    await screen.findByTestId('disc-question-error');

    fireEvent.click(screen.getByTestId('disc-question-option-b'));
    fireEvent.click(screen.getByTestId('disc-question-send'));
    await waitFor(() => expect(answerMock).toHaveBeenCalledTimes(2));
    const [, , firstBody] = answerMock.mock.calls[0];
    const [, , secondBody] = answerMock.mock.calls[1];
    expect(secondBody.idempotency_key).not.toBe(firstBody.idempotency_key);
  });

  /// Review @codex-cli-2 — `sending` is state: two clicks in the same tick both
  /// read it as false and both post. A decision must not be recorded twice
  /// because a mouse bounced.
  it('posts once when the button is clicked twice in the same tick', async () => {
    let release!: (value: unknown) => void;
    answerMock.mockReturnValue(new Promise(resolve => { release = resolve; }));

    renderCard();
    await screen.findByTestId('disc-question-q-1');
    fireEvent.click(screen.getByTestId('disc-question-option-a'));

    // Dispatched directly, twice, with no render in between — the shape the
    // `sending` state cannot catch, because neither click has seen it change.
    const send = screen.getByTestId('disc-question-send');
    send.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    send.dispatchEvent(new MouseEvent('click', { bubbles: true }));

    expect(answerMock).toHaveBeenCalledTimes(1);
    release(question({ state: 'answered', answer: {
      selected_option_ids: ['a'], text: null, author_pseudo: 'Romu - mac',
      answered_at: '2026-09-06T09:00:00Z', message_id: 'm-2',
    } }));
    await screen.findByTestId('disc-question-answer');
  });

  /// The answer is frozen while it travels: the idempotency key was minted for
  /// the answer as it stood, and letting the choice change under the request
  /// would decouple the two.
  it('refuses to change the answer while it is being sent', async () => {
    answerMock.mockReturnValue(new Promise(() => {}));
    renderCard();
    await screen.findByTestId('disc-question-q-1');
    fireEvent.click(screen.getByTestId('disc-question-option-a'));
    fireEvent.click(screen.getByTestId('disc-question-send'));

    expect(screen.getByTestId('disc-question-option-b')).toBeDisabled();
    expect(screen.getByTestId('disc-question-text')).toBeDisabled();
  });

  /// Review @codex-cli-2 — a read that started BEFORE the answer and lands
  /// after it is older than what is on screen. Publishing it would put the
  /// answered card back to pending and revive the count.
  it('ignores a read that was already in flight when the answer landed', async () => {
    let releaseRead!: (value: unknown) => void;
    questionsMock.mockReturnValueOnce(new Promise(resolve => { releaseRead = resolve; }));
    answerMock.mockResolvedValue(question({ state: 'answered', answer: {
      selected_option_ids: ['a'], text: null, author_pseudo: 'Romu - mac',
      answered_at: '2026-09-06T09:00:00Z', message_id: 'm-2',
    } }));

    renderCard();
    // The first read is still travelling; let it resolve so the card appears.
    await act(async () => { releaseRead({ questions: [question()], pending_count: 1 }); });
    await screen.findByTestId('disc-question-q-1');

    // A second read leaves, then the answer is recorded before it returns.
    questionsMock.mockReturnValueOnce(new Promise(resolve => { releaseRead = resolve; }));
    refreshDiscussionQuestions('d-1');
    fireEvent.click(screen.getByTestId('disc-question-option-a'));
    fireEvent.click(screen.getByTestId('disc-question-send'));
    await screen.findByTestId('disc-question-answer');

    // The stale read lands, still saying "pending". It must change nothing.
    await act(async () => { releaseRead({ questions: [question()], pending_count: 1 }); });
    expect(screen.getByTestId('disc-question-q-1')).toHaveAttribute('data-state', 'answered');
    expect(screen.queryByTestId('disc-question-send')).toBeNull();
  });

  /// A refusal ("someone already decided this") or a lost response must not
  /// leave the card offering to decide something already decided.
  it('reads the durable decision back when the send is refused', async () => {
    answerMock.mockRejectedValue(new Error('Discussion question already answered'));
    questionsMock
      .mockResolvedValueOnce({ questions: [question()], pending_count: 1 })
      .mockResolvedValue({
        questions: [question({ state: 'answered', answer: {
          selected_option_ids: ['b'], text: null, author_pseudo: 'Quelqu’un d’autre',
          answered_at: '2026-09-06T09:00:00Z', message_id: 'm-9',
        } })],
        pending_count: 0,
      });

    renderCard();
    await screen.findByTestId('disc-question-q-1');
    fireEvent.click(screen.getByTestId('disc-question-option-a'));
    fireEvent.click(screen.getByTestId('disc-question-send'));

    expect(await screen.findByTestId('disc-question-answer')).toHaveTextContent('Quelqu’un d’autre');
    expect(screen.queryByTestId('disc-question-send')).toBeNull();
  });

  it('shows an answered question as settled, with no form', async () => {
    questionsMock.mockResolvedValue({
      questions: [question({
        state: 'answered',
        answer: {
          selected_option_ids: ['b'],
          text: 'Avec un réarmement manuel.',
          author_pseudo: 'Romu - mac',
          answered_at: '2026-09-06T09:00:00Z',
          message_id: 'm-2',
        },
      })],
      pending_count: 0,
    });

    renderCard();
    const card = await screen.findByTestId('disc-question-q-1');
    expect(card).toHaveAttribute('data-state', 'answered');
    expect(screen.getByText('Réarmement explicite')).toBeInTheDocument();
    expect(screen.getByText('Avec un réarmement manuel.')).toBeInTheDocument();
    expect(screen.queryByTestId('disc-question-send')).toBeNull();
  });

  /// A fence the backend never turned into a row: showing the raw JSON would
  /// hand the reader machine notation and no way to answer it.
  it('says the decision is missing instead of printing the fence', async () => {
    questionsMock.mockResolvedValue({ questions: [], pending_count: 0 });
    renderCard();
    await waitFor(() => expect(screen.getByTestId('disc-question-loading'))
      .toHaveTextContent('disc.question.missing'));
  });

  /// Several cards in one room read one answer between them: the endpoint is
  /// polled, and one request per card would repeat that cost for the same list.
  it('reads the room once however many cards are on screen', async () => {
    questionsMock.mockResolvedValue({
      questions: [question(), question({ id: 'q-2', fence_index: 1 })],
      pending_count: 2,
    });
    render(
      <>
        <DiscussionQuestionCard discussionId="d-1" sourceMessageId="m-1" fenceIndex={0} />
        <DiscussionQuestionCard discussionId="d-1" sourceMessageId="m-1" fenceIndex={1} />
      </>,
    );
    expect(await screen.findByTestId('disc-question-q-1')).toBeInTheDocument();
    expect(screen.getByTestId('disc-question-q-2')).toBeInTheDocument();
    expect(questionsMock).toHaveBeenCalledTimes(1);
  });
});
