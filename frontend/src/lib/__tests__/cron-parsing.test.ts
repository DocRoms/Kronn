import { describe, it, expect } from 'vitest';

// Inline the parseCronExpr function to test it (it's not exported from the component)
// Must stay in sync with WorkflowWizard.tsx:parseCronExpr
function parseCronExpr(expr: string): {
  every: number; unit: string; at: string; weekdays: number[]; raw?: string;
} {
  const parts = expr.split(' ');
  if (parts.length !== 5) return { every: 5, unit: 'minutes', at: '00:00', weekdays: [] };
  const [min, hour, dom, _mon, dow] = parts;

  const parseIntList = (s: string): number[] | null => {
    if (s === '*') return [];
    if (!/^[0-9,]+$/.test(s)) return null;
    const nums = s.split(',').map(n => parseInt(n, 10)).filter(n => !isNaN(n) && n >= 0 && n <= 6);
    return nums.length > 0 ? Array.from(new Set(nums)).sort((a, b) => a - b) : null;
  };

  if (min.startsWith('*/')) return { every: parseInt(min.slice(2)) || 5, unit: 'minutes', at: '00:00', weekdays: [] };
  if (hour.startsWith('*/')) return { every: parseInt(hour.slice(2)) || 1, unit: 'hours', at: `00:${min.padStart(2, '0')}`, weekdays: [] };

  if (dom === '*' && dow !== '*') {
    const parsed = parseIntList(dow);
    if (parsed !== null && parsed.length > 0 && parsed.length < 7) {
      return { every: 1, unit: 'days', at: `${hour.padStart(2, '0')}:${min.padStart(2, '0')}`, weekdays: parsed };
    }
  }

  if (dom.startsWith('*/')) return { every: parseInt(dom.slice(2)) || 1, unit: 'days', at: `${hour.padStart(2, '0')}:${min.padStart(2, '0')}`, weekdays: [] };

  return { every: 1, unit: 'days', at: `${hour.padStart(2, '0')}:${min.padStart(2, '0')}`, weekdays: [], raw: expr };
}

// Inline buildCronExpr with day-of-week support — mirror of WorkflowWizard.tsx
function buildCronExpr(cronRaw: string, cronEvery: number, cronUnit: string, cronAt: string, cronWeekdays: number[]): string {
  if (cronRaw) return cronRaw;
  const [hh, mm] = cronAt.split(':').map(Number);
  const h = isNaN(hh) ? 0 : hh;
  const m = isNaN(mm) ? 0 : mm;
  switch (cronUnit) {
    case 'minutes': return `*/${cronEvery} * * * *`;
    case 'hours':   return `${m} */${cronEvery} * * *`;
    case 'days':
      if (cronWeekdays.length > 0 && cronWeekdays.length < 7) {
        return `${m} ${h} * * ${[...cronWeekdays].sort((a, b) => a - b).join(',')}`;
      }
      return `${m} ${h} */${cronEvery} * *`;
    case 'weeks':   return `${m} ${h} * * 1`;
    case 'months':  return `${m} ${h} 1 */${cronEvery} *`;
    default:        return '*/5 * * * *';
  }
}

describe('parseCronExpr', () => {
  it('parses simple minute interval', () => {
    const r = parseCronExpr('*/5 * * * *');
    expect(r).toEqual({ every: 5, unit: 'minutes', at: '00:00', weekdays: [] });
    expect(r.raw).toBeUndefined();
  });

  it('parses simple hour interval', () => {
    const r = parseCronExpr('0 */2 * * *');
    expect(r).toEqual({ every: 2, unit: 'hours', at: '00:00', weekdays: [] });
    expect(r.raw).toBeUndefined();
  });

  it('parses simple day interval', () => {
    const r = parseCronExpr('30 9 */1 * *');
    expect(r).toEqual({ every: 1, unit: 'days', at: '09:30', weekdays: [] });
    expect(r.raw).toBeUndefined();
  });

  it('parses Monday 9am as day-of-week', () => {
    const r = parseCronExpr('0 9 * * 1');
    expect(r.unit).toBe('days');
    expect(r.at).toBe('09:00');
    expect(r.weekdays).toEqual([1]);
    expect(r.raw).toBeUndefined();
  });

  it('parses multiple weekdays with sort + dedupe', () => {
    const r = parseCronExpr('30 14 * * 5,1,3');
    expect(r.unit).toBe('days');
    expect(r.at).toBe('14:30');
    expect(r.weekdays).toEqual([1, 3, 5]);
    expect(r.raw).toBeUndefined();
  });

  it('parses Sunday (0) correctly', () => {
    const r = parseCronExpr('0 10 * * 0');
    expect(r.weekdays).toEqual([0]);
    expect(r.unit).toBe('days');
  });

  it('preserves complex cron as raw (range not supported in simple UI)', () => {
    const expr = '0 7,10,13,16,19 * * 1-5';
    const r = parseCronExpr(expr);
    expect(r.raw).toBe(expr);
  });

  it('preserves "every day" 9am as raw (no DoW restriction, no DoM interval)', () => {
    // Pattern "0 9 * * *" has no */N in any field and no specific DoW → raw
    const expr = '0 9 * * *';
    const r = parseCronExpr(expr);
    expect(r.raw).toBe(expr);
  });

  it('handles invalid expression gracefully', () => {
    const r = parseCronExpr('invalid');
    expect(r.every).toBe(5);
    expect(r.unit).toBe('minutes');
    expect(r.weekdays).toEqual([]);
    expect(r.raw).toBeUndefined();
  });
});

describe('buildCronExpr', () => {
  it('returns raw when set', () => {
    expect(buildCronExpr('0 7,10,13 * * 1-5', 5, 'minutes', '00:00', [])).toBe('0 7,10,13 * * 1-5');
  });

  it('builds */N minute pattern', () => {
    expect(buildCronExpr('', 5, 'minutes', '00:00', [])).toBe('*/5 * * * *');
  });

  it('builds */N hour pattern', () => {
    expect(buildCronExpr('', 2, 'hours', '00:15', [])).toBe('15 */2 * * *');
  });

  it('builds */N day pattern when no weekdays picked', () => {
    expect(buildCronExpr('', 1, 'days', '09:30', [])).toBe('30 9 */1 * *');
  });

  it('builds day-of-week pattern for Marie\'s weekly recap (Monday 9am)', () => {
    // Marie's real use case: weekly recap every Monday at 9am
    expect(buildCronExpr('', 1, 'days', '09:00', [1])).toBe('0 9 * * 1');
  });

  it('builds day-of-week pattern for Mon/Wed/Fri', () => {
    expect(buildCronExpr('', 1, 'days', '14:30', [1, 3, 5])).toBe('30 14 * * 1,3,5');
  });

  it('sorts weekdays in output even if given out of order', () => {
    expect(buildCronExpr('', 1, 'days', '09:00', [5, 1, 3])).toBe('0 9 * * 1,3,5');
  });

  it('falls back to */N day when all 7 weekdays selected', () => {
    // 7 days = "every day" — emit interval form instead
    expect(buildCronExpr('', 1, 'days', '09:00', [0, 1, 2, 3, 4, 5, 6])).toBe('0 9 */1 * *');
  });

  it('falls back to */N day when 0 weekdays selected', () => {
    expect(buildCronExpr('', 2, 'days', '09:00', [])).toBe('0 9 */2 * *');
  });

  it('roundtrips Marie\'s "every Monday 9am" through parse + build', () => {
    const original = '0 9 * * 1';
    const parsed = parseCronExpr(original);
    expect(parsed.raw).toBeUndefined(); // should NOT fall into raw mode
    const rebuilt = buildCronExpr('', parsed.every, parsed.unit, parsed.at, parsed.weekdays);
    expect(rebuilt).toBe(original);
  });

  it('roundtrips "every Mon/Wed/Fri 2:30pm"', () => {
    const original = '30 14 * * 1,3,5';
    const parsed = parseCronExpr(original);
    const rebuilt = buildCronExpr('', parsed.every, parsed.unit, parsed.at, parsed.weekdays);
    expect(rebuilt).toBe(original);
  });
});
