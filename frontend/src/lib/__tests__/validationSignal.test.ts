// KT-1122 — the frontend's only parser of the validation signal. Each case
// mirrors a case of the backend's `ends_with_terminal_signal` tests, so the two
// cannot drift apart (the banner used to use `includes`, the gate a last line).
import { describe, it, expect } from 'vitest';
import { endsWithValidationSignal } from '../validationSignal';

describe('endsWithValidationSignal', () => {
  it.each([
    'KRONN:VALIDATION_COMPLETE',
    'Done.\nKRONN:VALIDATION_COMPLETE',
    'Done.\n\nKRONN:VALIDATION_COMPLETE\n',
    'Done.\n**KRONN:VALIDATION_COMPLETE**',
    'Done.\n**KRONN:VALIDATION_COMPLETE',
    'Done.\n`KRONN:VALIDATION_COMPLETE`',
    'done\nkronn:validation_complete',
  ])('accepts a last-line signal: %j', content => {
    expect(endsWithValidationSignal(content)).toBe(true);
  });

  it.each([
    'I will emit KRONN:VALIDATION_COMPLETE later.',
    'KRONN:VALIDATION_COMPLETE\nmore work',
    'KRONN:VALIDATION_COMPLETE puis la suite',
    'Emit "KRONN:VALIDATION_COMPLETE" when done',
    '',
  ])('refuses a mention that is not the last line: %j', content => {
    expect(endsWithValidationSignal(content)).toBe(false);
  });
});
