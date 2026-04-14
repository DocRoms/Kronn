import { useState, useEffect } from 'react';
import { Send } from 'lucide-react';
import type { AgentQuestion } from '../lib/agent-question-parse';
import { composeAnswers } from '../lib/agent-question-parse';

interface Props {
  /** Parsed questions from the latest agent message. Rendered in source order. */
  questions: AgentQuestion[];
  /**
   * Discussion id — used as a cache key so answers don't leak between
   * discussions when the user switches. Switching to a disc whose last agent
   * message has questions resets the inputs.
   */
  discussionId: string;
  /**
   * Called with the composed `var: value` reply string. Caller routes it
   * through the normal send-message flow (same pathway as ChatInput).
   */
  onSubmit: (reply: string) => void;
  /** i18n accessor — decoupled to keep this component test-friendly. */
  t: (key: string, ...args: (string | number)[]) => string;
}

/**
 * Mini-form surfaced above ChatInput when the agent's last message contains
 * structured `{{var}}: question` entries. Lets the user answer each one in
 * its own field; a single Send button assembles `var: value` lines and
 * forwards them through the normal send flow.
 *
 * The regular ChatInput stays visible alongside the form — the user can
 * ignore the structured path entirely and type a free-form reply.
 */
export function AgentQuestionForm({ questions, discussionId, onSubmit, t }: Props) {
  const [answers, setAnswers] = useState<Record<string, string>>({});

  // Reset state when the user switches discussion OR the agent produces a
  // new message with a different question set. Keying by (disc + vars)
  // covers both: switching disc changes discussionId, a new question set
  // on the same disc changes the joined var list.
  const signature = `${discussionId}:${questions.map(q => q.var).join(',')}`;
  useEffect(() => {
    setAnswers({});
  }, [signature]);

  const allRequired = questions.every(q => (answers[q.var] ?? '').trim().length > 0);

  const handleSubmit = () => {
    const reply = composeAnswers(questions, answers);
    if (!reply) return;
    onSubmit(reply);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && allRequired) {
      e.preventDefault();
      handleSubmit();
    }
  };

  return (
    <div className="agent-question-form" data-testid="agent-question-form" onKeyDown={handleKeyDown}>
      <p className="agent-question-form-hint">{t('agentQuestions.hint')}</p>
      <div className="agent-question-form-fields">
        {questions.map(q => (
          <label key={q.var} className="agent-question-form-field">
            <span className="agent-question-form-label">
              <code>{q.var}</code> — {q.question}
            </span>
            <input
              type="text"
              className="agent-question-form-input"
              value={answers[q.var] ?? ''}
              onChange={e => setAnswers(prev => ({ ...prev, [q.var]: e.target.value }))}
              placeholder={q.question}
              aria-label={q.question}
            />
          </label>
        ))}
      </div>
      <button
        type="button"
        className="agent-question-form-submit"
        onClick={handleSubmit}
        disabled={!allRequired}
        title={allRequired ? t('agentQuestions.submit') : t('agentQuestions.fillAll')}
      >
        <Send size={12} /> {t('agentQuestions.submit')}
        <span className="agent-question-form-shortcut">Ctrl+Enter</span>
      </button>
    </div>
  );
}
