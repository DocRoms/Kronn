import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DiscussionQuestion, SourceCheck } from '../../types/generated';

const { questionsMock, answerMock } = vi.hoisted(() => ({
  questionsMock: vi.fn(), answerMock: vi.fn(),
}));
vi.mock('../../lib/api', () => ({
  discussions: { questions: questionsMock, answerQuestion: answerMock },
}));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string, ...args: Array<string | number>) => [key, ...args].join(' ') }),
}));

import { MarkdownContent } from '../MessageBubble';
import { refreshDiscussionQuestions, resetDiscussionQuestions } from '../../lib/discussionQuestions';

const question: DiscussionQuestion = {
  id: 'q-refresh', discussion_id: 'd-refresh', source_message_id: 'm-refresh',
  fence_index: 0, key: 'refresh-proof', question: 'Choisir sans perdre le formulaire',
  context: 'Diagnostic local, sans réseau.',
  options: [{ id: 'a', label: 'Continuer', description: null }],
  multiple: false, recommended_option_ids: [], task_ref: null,
  state: 'pending', answer: null,
  created_at: '2026-09-08T15:00:00Z', updated_at: '2026-09-08T15:00:00Z',
};
const content = '```kronn-question\n{"version":1,"key":"refresh-proof","question":"Choisir sans perdre le formulaire"}\n```';
const markdown = (sources?: SourceCheck[], body = content, messageId = 'm-refresh') => (
  <MarkdownContent content={body} discussionId="d-refresh" sourceMessageId={messageId} sources={sources} />
);

beforeEach(() => {
  vi.clearAllMocks();
  resetDiscussionQuestions();
  questionsMock.mockImplementation(async () => ({ questions: [structuredClone(question)], pending_count: 1 }));
});
afterEach(() => { cleanup(); resetDiscussionQuestions(); });

async function fillForm() {
  await screen.findByTestId('disc-question-q-refresh');
  fireEvent.click(screen.getByTestId('disc-question-option-a'));
  const input = screen.getByTestId('disc-question-text');
  fireEvent.change(input, { target: { value: 'Réponse en cours' } });
  input.focus();
  expect(screen.getByTestId('disc-question-send')).toBeEnabled();
  return input;
}

function expectDraftPreserved(input: HTMLElement) {
  expect(screen.getByTestId('disc-question-text')).toBe(input);
  expect(input).toHaveValue('Réponse en cours');
  expect(input).toHaveFocus();
  expect(screen.getByTestId('disc-question-option-a')).toBeChecked();
  expect(screen.getByTestId('disc-question-send')).toBeEnabled();
}

describe('KT-622 — arbitration remains usable during live Markdown refreshes', () => {
  it('preserves draft, selection and focus when only durable questions refresh', async () => {
    render(markdown([]));
    const input = await fillForm();
    await act(async () => { refreshDiscussionQuestions('d-refresh'); });
    expectDraftPreserved(input);
  });

  it('preserves the form when rerendering the same sources reference', async () => {
    const sources: SourceCheck[] = [];
    const view = render(markdown(sources));
    const input = await fillForm();
    view.rerender(markdown(sources));
    expectDraftPreserved(input);
  });

  it('keeps the same form through repeated equivalent deserialized lint reports', async () => {
    const view = render(markdown([]));
    const input = await fillForm();
    for (let refresh = 0; refresh < 3; refresh++) {
      view.rerender(markdown([]));
      await act(async () => {});
      expectDraftPreserved(input);
    }
  });

  it('updates changed citation verdicts without discarding the human draft', async () => {
    const body = `${content}\n\nSource [src: file: a.rs:1].\n\n- Source [src: file: a.rs:1].`;
    const view = render(markdown(undefined, body));
    const input = await fillForm();
    const source: SourceCheck = { raw: 'file: a.rs:1', kind: 'file', status: 'verified', detail: 'ok' };
    view.rerender(markdown([source], body));
    expectDraftPreserved(input);
    for (const chip of screen.getAllByTestId('source-citation-chip')) {
      expect(chip).toHaveAttribute('data-state', 'verified');
    }
    view.rerender(markdown([{ ...source, status: 'not_found', detail: 'missing' }], body));
    expectDraftPreserved(input);
    for (const chip of screen.getAllByTestId('source-citation-chip')) {
      expect(chip).toHaveAttribute('data-state', 'suspect');
      expect(chip.getAttribute('title')).toContain('missing');
    }
  });

  it('keeps the draft while new text streams after an existing question fence', async () => {
    const view = render(markdown());
    const input = await fillForm();
    view.rerender(markdown(undefined, `${content}\n\nUn résultat arrive.`));
    expectDraftPreserved(input);
    expect(screen.getByText('Un résultat arrive.')).toBeInTheDocument();
  });

  it('preserves the in-flight submission guard through a report refresh', async () => {
    let resolveAnswer!: (answer: DiscussionQuestion) => void;
    answerMock.mockImplementation(() => new Promise<DiscussionQuestion>(resolve => { resolveAnswer = resolve; }));
    const view = render(markdown([]));
    await fillForm();
    fireEvent.click(screen.getByTestId('disc-question-send'));
    expect(answerMock).toHaveBeenCalledTimes(1);
    view.rerender(markdown([]));
    expect(screen.getByTestId('disc-question-option-a')).toBeDisabled();
    expect(screen.getByTestId('disc-question-text')).toBeDisabled();
    expect(screen.getByTestId('disc-question-send')).toBeDisabled();
    fireEvent.click(screen.getByTestId('disc-question-send'));
    expect(answerMock).toHaveBeenCalledTimes(1);
    await act(async () => {
      resolveAnswer({ ...question, state: 'answered', answer: {
        selected_option_ids: ['a'], text: 'Réponse en cours', author_pseudo: 'Human',
        answered_at: '2026-09-08T16:00:00Z', message_id: 'answer-refresh',
      } });
    });
    expect(screen.getByTestId('disc-question-answer')).toHaveTextContent('Human');
  });

  it('does not transfer a draft when the rendered message identifies another question', async () => {
    const other = { ...question, id: 'q-other', source_message_id: 'm-other', key: 'other' };
    questionsMock.mockResolvedValue({ questions: [question, other], pending_count: 2 });
    const view = render(markdown([]));
    const input = await fillForm();
    view.rerender(markdown([], content, 'm-other'));
    await screen.findByTestId('disc-question-q-other');
    expect(screen.getByTestId('disc-question-text')).not.toBe(input);
    expect(screen.getByTestId('disc-question-text')).toHaveValue('');
    expect(screen.getByTestId('disc-question-option-a')).not.toBeChecked();
    expect(screen.getByTestId('disc-question-send')).toBeDisabled();
  });
});
