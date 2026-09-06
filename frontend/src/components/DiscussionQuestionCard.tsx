// KT-595 — the arbitration a human owes the room, where it cannot scroll away.
//
// Agents needed decisions and asked for them in prose. Prose descends: KT-593
// sat unanswered for a day not because it was hard but because the question
// had already scrolled past. This card stays until it is answered, says which
// lot it blocks, and records the answer as a durable message so whoever picks
// the work up later reads the decision rather than asking again.
import { useRef, useState } from 'react';
import { AlertOctagon, CheckCircle2, Loader2 } from 'lucide-react';
import { discussions as discussionsApi } from '../lib/api';
import {
  applyAnsweredQuestion,
  refreshDiscussionQuestions,
  useDiscussionQuestions,
} from '../lib/discussionQuestions';
import { useT } from '../lib/I18nContext';
import { findFenceProblem } from '../lib/questionFence';
import { userError } from '../lib/userError';
import type { DiscussionQuestion, DiscussionQuestionAnswer } from '../types/generated';
import './DiscussionQuestionCard.css';

export interface DiscussionQuestionCardProps {
  discussionId: string;
  /** KT-607 — the fence exactly as it was written. Read only when no durable
   *  row matches, to say WHICH field kept it from being recorded. */
  source?: string;
  /** The message the fence sits in, and which fence within it — together they
   *  identify the durable row the backend wrote when the message landed. */
  sourceMessageId?: string;
  fenceIndex?: number;
}

export function DiscussionQuestionCard({
  discussionId,
  source,
  sourceMessageId,
  fenceIndex,
}: DiscussionQuestionCardProps) {
  const { questions, loading } = useDiscussionQuestions(discussionId);
  const question = sourceMessageId === undefined || fenceIndex === undefined
    ? null
    : questions.find(
      candidate => candidate.source_message_id === sourceMessageId
        && candidate.fence_index === fenceIndex,
    ) ?? null;

  if (question) return <QuestionBody discussionId={discussionId} question={question} />;
  // Still reading, or nothing was ever recorded. Either way the fence itself
  // is not worth showing as code: it is machine notation.
  return <QuestionPlaceholder loading={loading} source={source} />;
}

function QuestionPlaceholder({ loading, source }: { loading: boolean; source?: string }) {
  const { t } = useT();
  if (loading) {
    return (
      <div className="disc-question-card" data-state="loading" data-testid="disc-question-loading">
        <Loader2 size={13} className="spin" />
        <span>{t('disc.question.loading')}</span>
      </div>
    );
  }

  // KT-607 — nothing was recorded, and until now that was all anyone was told.
  // The notation is right here; if it is what refused the question, say which
  // field, so the author can fix it instead of guessing.
  const problem = findFenceProblem(source);
  if (!problem) {
    return (
      <div className="disc-question-card" data-state="loading" data-testid="disc-question-loading">
        <AlertOctagon size={13} />
        <span>{t('disc.question.missing')}</span>
      </div>
    );
  }
  return (
    <section
      className="disc-question-card"
      data-state="invalid"
      data-testid="disc-question-invalid"
    >
      <header className="disc-question-head">
        <AlertOctagon size={13} />
        <span className="disc-question-kind">{t('disc.question.invalidTitle')}</span>
      </header>
      <p className="disc-question-text">{t(problem.key)}</p>
      <p className="disc-question-context">{t('disc.question.invalidHint')}</p>
    </section>
  );
}

function QuestionBody({
  discussionId,
  question,
}: {
  discussionId: string;
  question: DiscussionQuestion;
}) {
  const { t } = useT();
  const [selected, setSelected] = useState<string[]>([]);
  const [text, setText] = useState('');
  const [sending, setSending] = useState(false);
  const [error, setError] = useState('');
  // A retry of the SAME answer must not record a second decision; a retry of a
  // DIFFERENT one must not be swallowed as a duplicate. So the key survives a
  // failed send and is renewed the moment the answer itself changes.
  const idempotencyKey = useRef(crypto.randomUUID());
  const renewKey = () => { idempotencyKey.current = crypto.randomUUID(); };
  /// The `sending` state cannot hold this door: two clicks in the same tick
  /// both read it as false and both post. A decision is not something to send
  /// twice because a mouse bounced.
  const inFlight = useRef(false);

  const answered = question.state === 'answered' && question.answer !== null;
  const canSend = selected.length > 0 || text.trim().length > 0;

  const toggle = (optionId: string) => {
    // Frozen while in flight: the key was minted for the answer as it stood,
    // and letting it change under the request would decouple the two.
    if (inFlight.current) return;
    renewKey();
    setError('');
    setSelected(current => {
      if (!question.multiple) return current[0] === optionId ? [] : [optionId];
      return current.includes(optionId)
        ? current.filter(id => id !== optionId)
        : [...current, optionId];
    });
  };

  const send = async () => {
    if (!canSend || inFlight.current) return;
    inFlight.current = true;
    setSending(true);
    setError('');
    try {
      const updated = await discussionsApi.answerQuestion(discussionId, question.id, {
        selected_option_ids: selected,
        text: text.trim() || null,
        idempotency_key: idempotencyKey.current,
      });
      applyAnsweredQuestion(discussionId, updated);
    } catch (cause) {
      setError(userError(cause));
      // The server may have recorded the decision and lost the response, or
      // refused this one because another had already been recorded. Either way
      // the durable row is the truth: read it back rather than leave the card
      // offering to decide something that is already decided.
      refreshDiscussionQuestions(discussionId);
    } finally {
      inFlight.current = false;
      setSending(false);
    }
  };

  return (
    <section
      className="disc-question-card"
      data-state={answered ? 'answered' : 'pending'}
      data-testid={`disc-question-${question.id}`}
    >
      <header className="disc-question-head">
        {answered ? <CheckCircle2 size={13} /> : <AlertOctagon size={13} />}
        <span className="disc-question-kind">
          {t(answered ? 'disc.question.answeredTitle' : 'disc.question.pendingTitle')}
        </span>
        {question.task_ref && (
          <span className="disc-question-task" data-testid="disc-question-task">
            {t('disc.question.blocks', question.task_ref)}
          </span>
        )}
      </header>

      <p className="disc-question-text">{question.question}</p>
      {question.context && <p className="disc-question-context">{question.context}</p>}

      {question.answer ? (
        <AnsweredSummary question={question} answer={question.answer} />
      ) : (
        <>
          {question.options.length > 0 && (
            <ul className="disc-question-options">
              {question.options.map(option => {
                const recommended = question.recommended_option_ids.includes(option.id);
                return (
                  <li key={option.id}>
                    <label data-selected={selected.includes(option.id)}>
                      <input
                        type={question.multiple ? 'checkbox' : 'radio'}
                        name={`question-${question.id}`}
                        checked={selected.includes(option.id)}
                        onChange={() => toggle(option.id)}
                        disabled={sending}
                        data-testid={`disc-question-option-${option.id}`}
                      />
                      <span className="disc-question-option-label">
                        {option.label}
                        {/* Marked, never pre-selected: a checked box the human
                            did not check is not a decision they made. */}
                        {recommended && (
                          <em className="disc-question-recommended">
                            {t('disc.question.recommended')}
                          </em>
                        )}
                      </span>
                      {option.description && (
                        <span className="disc-question-option-desc">{option.description}</span>
                      )}
                    </label>
                  </li>
                );
              })}
            </ul>
          )}

          {/* Always offered, options or not: the right answer is regularly one
              nobody thought to list. */}
          <textarea
            className="disc-question-text-input"
            value={text}
            onChange={event => { renewKey(); setText(event.target.value); }}
            disabled={sending}
            placeholder={t('disc.question.freeTextPlaceholder')}
            aria-label={t('disc.question.freeTextPlaceholder')}
            data-testid="disc-question-text"
            rows={2}
          />

          {error && <p className="disc-question-error" data-testid="disc-question-error">{error}</p>}

          <button
            type="button"
            className="disc-question-send"
            onClick={() => void send()}
            disabled={!canSend || sending}
            data-testid="disc-question-send"
          >
            {sending ? <Loader2 size={12} className="spin" /> : null}
            {t('disc.question.send')}
          </button>
        </>
      )}
    </section>
  );
}

// The answer is passed in rather than read back off the question: it is what
// decides this branch, and narrowing it at the call site keeps the assertion
// out of here entirely.
function AnsweredSummary({
  question,
  answer,
}: {
  question: DiscussionQuestion;
  answer: DiscussionQuestionAnswer;
}) {
  const { t } = useT();
  const chosen = question.options.filter(option => answer.selected_option_ids.includes(option.id));
  return (
    <div className="disc-question-answer" data-testid="disc-question-answer">
      {chosen.length > 0 && (
        <ul className="disc-question-chosen">
          {chosen.map(option => <li key={option.id}>{option.label}</li>)}
        </ul>
      )}
      {answer.text && <p className="disc-question-answer-text">{answer.text}</p>}
      <p className="disc-question-answer-meta">
        {t(
          'disc.question.answeredBy',
          answer.author_pseudo,
          new Date(answer.answered_at).toLocaleString(),
        )}
      </p>
    </div>
  );
}
