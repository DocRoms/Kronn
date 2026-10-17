// KT-1122 — the frontend's only parser of the validation signal. Its cases are
// the backend's own: both sides read `backend/tests/fixtures/validation_signal_cases.json`
// (the backend's `ends_with_terminal_signal` runs the same table), so the two
// cannot drift apart (the banner used to use `includes`, the gate a last line).
import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { endsWithValidationSignal, VALIDATION_SIGNAL } from '../validationSignal';

interface SignalCase {
  content: string;
  ends_with_signal: boolean;
}

const table = JSON.parse(
  readFileSync(
    join(__dirname, '..', '..', '..', '..', 'backend', 'tests', 'fixtures', 'validation_signal_cases.json'),
    'utf8',
  ),
) as { signal: string; cases: SignalCase[] };

describe('endsWithValidationSignal', () => {
  it('reads the same signal as the backend table', () => {
    expect(table.signal).toBe(VALIDATION_SIGNAL);
    expect(table.cases.length).toBeGreaterThanOrEqual(10);
  });

  it.each(table.cases.filter(c => c.ends_with_signal).map(c => c.content))(
    'accepts a last-line signal: %j',
    content => {
      expect(endsWithValidationSignal(content)).toBe(true);
    },
  );

  it.each(table.cases.filter(c => !c.ends_with_signal).map(c => c.content))(
    'refuses what is not a last-line signal: %j',
    content => {
      expect(endsWithValidationSignal(content)).toBe(false);
    },
  );
});
