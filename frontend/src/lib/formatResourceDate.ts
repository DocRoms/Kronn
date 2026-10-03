/** A repository or Kronn timestamp as the reader's locale writes it. */
export function formatResourceDate(iso: string | undefined, locale?: string): string {
  if (!iso) return '—';
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleString(locale, { dateStyle: 'medium', timeStyle: 'short' });
}
