import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { MessageDateSeparator } from '../MessageDateSeparator';
import { localCalendarDayKey } from '../../lib/discussionDates';

const translations: Record<string, string> = {
  'disc.dateToday': 'Aujourd’hui',
  'disc.dateSeparatorLabel': 'Messages — {0}',
};

const t = (key: string, ...args: (string | number)[]) => {
  const template = translations[key] ?? key;
  return args.reduce<string>(
    (value, argument, index) => value.replace(`{${index}}`, String(argument)),
    template,
  );
};

describe('MessageDateSeparator', () => {
  it('uses the localized Today label for the current local calendar day', () => {
    const now = new Date(2026, 7, 2, 15, 0);
    render(
      <MessageDateSeparator
        timestamp={new Date(2026, 7, 2, 0, 1).toISOString()}
        locale="fr"
        t={t}
        now={now}
      />,
    );

    expect(screen.getByRole('separator', { name: 'Messages — Aujourd’hui' })).toHaveTextContent('Aujourd’hui');
  });

  it('formats an earlier day with the UI locale', () => {
    render(
      <MessageDateSeparator
        timestamp={new Date(2026, 6, 29, 12, 0).toISOString()}
        locale="fr"
        t={t}
        now={new Date(2026, 7, 2, 15, 0)}
      />,
    );

    expect(screen.getByRole('separator')).toHaveTextContent('29/07/2026');
  });

  it('does not render a separator for an invalid timestamp', () => {
    const { container } = render(
      <MessageDateSeparator timestamp="not-a-date" locale="fr" t={t} />,
    );

    expect(container).toBeEmptyDOMElement();
  });

  it('uses local calendar boundaries rather than UTC boundaries', () => {
    const lateEvening = new Date(2026, 7, 2, 23, 59);
    const afterMidnight = new Date(2026, 7, 3, 0, 1);

    expect(localCalendarDayKey(lateEvening.toISOString())).not.toBe(
      localCalendarDayKey(afterMidnight.toISOString()),
    );
  });
});
