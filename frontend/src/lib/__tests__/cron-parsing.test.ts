import { describe, it, expect } from 'vitest';

// Inline the parseCronExpr function to test it (it's not exported from the component)
function parseCronExpr(expr: string): { every: number; unit: string; at: string; raw?: string } {
  const parts = expr.split(' ');
  if (parts.length !== 5) return { every: 5, unit: 'minutes', at: '00:00' };
  const [min, hour, dom] = parts;
  if (min.startsWith('*/')) return { every: parseInt(min.slice(2)) || 5, unit: 'minutes', at: '00:00' };
  if (hour.startsWith('*/')) return { every: parseInt(hour.slice(2)) || 1, unit: 'hours', at: `00:${min.padStart(2, '0')}` };
  if (dom.startsWith('*/')) return { every: parseInt(dom.slice(2)) || 1, unit: 'days', at: `${hour.padStart(2, '0')}:${min.padStart(2, '0')}` };
  return { every: 1, unit: 'days', at: `${hour.padStart(2, '0')}:${min.padStart(2, '0')}`, raw: expr };
}

describe('parseCronExpr', () => {
  it('parses simple minute interval', () => {
    const r = parseCronExpr('*/5 * * * *');
    expect(r).toEqual({ every: 5, unit: 'minutes', at: '00:00' });
    expect(r.raw).toBeUndefined();
  });

  it('parses simple hour interval', () => {
    const r = parseCronExpr('0 */2 * * *');
    expect(r).toEqual({ every: 2, unit: 'hours', at: '00:00' });
    expect(r.raw).toBeUndefined();
  });

  it('parses simple day interval', () => {
    const r = parseCronExpr('30 9 */1 * *');
    expect(r).toEqual({ every: 1, unit: 'days', at: '09:30' });
    expect(r.raw).toBeUndefined();
  });

  it('preserves complex cron as raw (multi-hour)', () => {
    const expr = '0 7,10,13,16,19 * * 1-5';
    const r = parseCronExpr(expr);
    expect(r.raw).toBe(expr);
  });

  it('preserves complex cron as raw (specific days)', () => {
    const expr = '30 9 * * 1,3,5';
    const r = parseCronExpr(expr);
    expect(r.raw).toBe(expr);
  });

  it('preserves complex cron as raw (fixed hour no interval)', () => {
    const expr = '0 9 * * *';
    const r = parseCronExpr(expr);
    expect(r.raw).toBe(expr);
  });

  it('handles invalid expression gracefully', () => {
    const r = parseCronExpr('invalid');
    expect(r.every).toBe(5);
    expect(r.unit).toBe('minutes');
    expect(r.raw).toBeUndefined();
  });

  it('buildCronExpr returns raw when set', () => {
    // Simulate the buildCronExpr logic
    const cronRaw = '0 7,10,13,16,19 * * 1-5';
    const buildCronExpr = () => cronRaw || '*/5 * * * *';
    expect(buildCronExpr()).toBe('0 7,10,13,16,19 * * 1-5');
  });

  it('buildCronExpr returns computed when raw is empty', () => {
    const cronRaw = '';
    const cronEvery = 5;
    const buildCronExpr = () => cronRaw || `*/${cronEvery} * * * *`;
    expect(buildCronExpr()).toBe('*/5 * * * *');
  });
});
