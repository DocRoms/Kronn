export function localCalendarDayKey(timestamp: string): string | null {
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return null;

  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

export function formatDiscussionDay(
  timestamp: string,
  locale: string,
  todayLabel: string,
  now = new Date(),
): string | null {
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return null;

  if (localCalendarDayKey(timestamp) === localCalendarDayKey(now.toISOString())) {
    return todayLabel;
  }

  return new Intl.DateTimeFormat(locale, {
    day: '2-digit',
    month: '2-digit',
    year: 'numeric',
  }).format(date);
}
