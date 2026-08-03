export function formatPeriod(kind: string, period: string, locale: string): string {
  if (kind === 'weekly') {
    const date = new Date(`${period}T00:00:00Z`);
    if (Number.isNaN(date.getTime())) return period;
    date.setUTCDate(date.getUTCDate() + 6);
    return `${period} → ${date.toISOString().slice(0, 10)}`;
  }
  if (kind === 'monthly') {
    const [year, month] = period.split('-').map(Number);
    if (year && month) {
      return new Date(Date.UTC(year, month - 1, 1)).toLocaleDateString(locale, {
        month: 'short',
        year: 'numeric',
        timeZone: 'UTC',
      });
    }
  }
  return period;
}

const ROWS_PER_PAGE: Record<string, number> = { daily: 30, weekly: 15, monthly: 12 };

export function rowsPerPage(kind: string): number {
  return ROWS_PER_PAGE[kind] ?? 30;
}
