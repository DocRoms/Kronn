// KT-1636 — the backend's `is_full_suite_validation` runs the same table
// (`backend/tests/fixtures/full_suite_validation_cases.json`), so the two
// detectors cannot drift apart.
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { isFullSuiteValidation } from '../fullSuiteValidation';

interface FullSuiteCase {
  command: string;
  full_suite: boolean;
}

const table = JSON.parse(
  readFileSync(
    join(__dirname, '..', '..', '..', '..', 'backend', 'tests', 'fixtures', 'full_suite_validation_cases.json'),
    'utf8',
  ),
) as { cases: FullSuiteCase[] };

describe('isFullSuiteValidation', () => {
  it('keeps the coverage of the shared table', () => {
    expect(table.cases.length).toBeGreaterThanOrEqual(10);
  });

  it.each(table.cases.filter(c => c.full_suite).map(c => c.command))(
    'flags `%s` as the full suite',
    command => {
      expect(isFullSuiteValidation(command)).toBe(true);
    },
  );

  it.each(table.cases.filter(c => !c.full_suite).map(c => c.command))(
    'does not flag `%s`',
    command => {
      expect(isFullSuiteValidation(command)).toBe(false);
    },
  );
});
