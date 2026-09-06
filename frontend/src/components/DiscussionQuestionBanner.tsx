// KT-595 — the pending arbitration, visible on arrival.
//
// The card itself lives in the message that asked, which is exactly where a
// question goes to be forgotten: a thread of a thousand turns scrolls it out
// of sight within the hour. This strip stays at the top of the conversation
// while anything is pending, and takes the reader to it.
import { AlertOctagon } from 'lucide-react';
import { useEffect } from 'react';
import { refreshDiscussionQuestions, useDiscussionQuestions } from '../lib/discussionQuestions';
import { useT } from '../lib/I18nContext';
import './DiscussionQuestionBanner.css';

export function DiscussionQuestionBanner({ discussionId, messageRevision }: {
  discussionId: string;
  messageRevision?: string;
}) {
  const { t } = useT();
  const { questions, pendingCount } = useDiscussionQuestions(discussionId);
  useEffect(() => {
    refreshDiscussionQuestions(discussionId);
  }, [discussionId, messageRevision]);
  if (pendingCount === 0) return null;

  const first = questions.find(question => question.state === 'pending');
  const reveal = () => {
    if (!first) return;
    document
      .querySelector(`[data-testid="disc-question-${first.id}"]`)
      ?.scrollIntoView({ behavior: 'smooth', block: 'center' });
  };

  return (
    <div className="disc-question-banner" role="status" data-testid="disc-question-banner">
      <AlertOctagon size={13} aria-hidden="true" />
      <span className="disc-question-banner-count">
        {t('disc.question.bannerPending', pendingCount)}
      </span>
      {/* The question itself, not just a count: often it is short enough to
          answer without going anywhere. */}
      {first && <span className="disc-question-banner-text">{first.question}</span>}
      <button type="button" onClick={reveal} data-testid="disc-question-banner-go">
        {t('disc.question.bannerGo')}
      </button>
    </div>
  );
}
