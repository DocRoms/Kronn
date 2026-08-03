import { memo, useEffect, useRef, useState } from 'react';
import { Check, StickyNote } from 'lucide-react';
import type { DiscussionMessage } from '../types/generated';
import { MarkdownContent } from './MessageBubble';

interface DiscussionNoteProps {
  message: DiscussionMessage;
  discussionId: string;
  t: (key: string, ...args: (string | number)[]) => string;
}

function shortMessageId(id: string): string {
  return `#${id.slice(0, 8)}`;
}

export const DiscussionNote = memo(function DiscussionNote({
  message,
  discussionId,
  t,
}: DiscussionNoteProps) {
  const [copied, setCopied] = useState(false);
  const resetTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const author = message.author_pseudo || (
    message.role === 'Agent'
      ? message.agent_type ?? t('disc.note.agent')
      : t('disc.note.human')
  );

  useEffect(() => () => {
    if (resetTimer.current) clearTimeout(resetTimer.current);
  }, []);

  const copyId = async () => {
    try {
      await navigator.clipboard.writeText(message.id);
      setCopied(true);
      if (resetTimer.current) clearTimeout(resetTimer.current);
      resetTimer.current = setTimeout(() => setCopied(false), 1500);
    } catch {
      setCopied(false);
    }
  };

  return (
    <div className="disc-note-row" data-message-id={message.id} data-role="note">
      <details className="disc-note">
        <summary>
          <span className="disc-note-heading">
            <StickyNote size={13} aria-hidden="true" />
            <span>{t('disc.note.label')}</span>
            <span className="disc-note-author">{author}</span>
          </span>
          <span className="disc-note-excerpt">
            {message.content.replace(/\s+/g, ' ').trim()}
          </span>
          <span className="disc-note-time">
            {new Date(message.timestamp).toLocaleString()}
          </span>
        </summary>
        <div className="disc-note-body">
          <MarkdownContent content={message.content} discussionId={discussionId} />
        </div>
      </details>
      <button
        type="button"
        className="disc-id-pill disc-message-id-pill"
        data-copied={copied}
        onClick={copyId}
        title={t('disc.idPillTooltip', message.id)}
        aria-label={t('disc.idPillTooltip', message.id)}
      >
        {copied ? <Check size={8} /> : null}
        {shortMessageId(message.id)}
      </button>
    </div>
  );
});
