import { formatDiscussionDay } from '../lib/discussionDates';

type Translator = (key: string, ...args: (string | number)[]) => string;

interface MessageDateSeparatorProps {
  timestamp: string;
  locale: string;
  t: Translator;
  now?: Date;
}

export function MessageDateSeparator({
  timestamp,
  locale,
  t,
  now,
}: MessageDateSeparatorProps) {
  const label = formatDiscussionDay(timestamp, locale, t('disc.dateToday'), now);
  if (!label) return null;

  return (
    <div
      className="disc-message-date-separator"
      role="separator"
      aria-label={t('disc.dateSeparatorLabel', label)}
    >
      <span>{label}</span>
    </div>
  );
}
