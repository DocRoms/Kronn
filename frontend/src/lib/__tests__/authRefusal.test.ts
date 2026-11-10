/** KT-1524 — reading a refused credential back from configs and step outputs. */
import { describe, expect, it } from 'vitest';
import type { McpConfigDisplay, StepResult } from '../../types/generated';
import { authRefusalSentence, connectionsNeedingAttention, runAuthRefusals } from '../authRefusal';

const SENTENCE = 'Connection "GitHub X" (GitHub) refused the token (HTTP 401): Bad credentials — update the token in Settings → APIs.';
const REFUSED_OUTPUT = `${SENTENCE}\nHTTP 401 on GET https://api.github.com/user — {}\n[SIGNAL: ERROR]\n[SIGNAL: http_401]\n[SIGNAL: auth_refused]`;

const step = (over: Partial<StepResult>): StepResult => ({
  step_name: 'collect',
  status: 'Failed',
  output: REFUSED_OUTPUT,
  tokens_used: 0,
  duration_ms: 1,
  is_rollback: false,
  ...over,
});

describe('authRefusalSentence', () => {
  it('returns the plain sentence of an output closed by the auth-refused signal', () => {
    expect(authRefusalSentence(REFUSED_OUTPUT)).toBe(SENTENCE);
  });

  it('ignores an output without the signal, even a plain 401', () => {
    expect(authRefusalSentence('HTTP 401 on GET x — nope\n[SIGNAL: http_401]')).toBeNull();
    expect(authRefusalSentence(`${SENTENCE}\n(no signal)`)).toBeNull();
  });
});

describe('runAuthRefusals', () => {
  it('lists each step that hit a refused credential, with its sentence', () => {
    const refusals = runAuthRefusals([
      step({ step_name: 'ok', status: 'Success', output: 'fine' }),
      step({ step_name: 'collect' }),
      step({ step_name: 'partial', status: 'Success', output: REFUSED_OUTPUT }),
    ]);
    expect(refusals).toEqual([
      { stepName: 'collect', sentence: SENTENCE },
      { stepName: 'partial', sentence: SENTENCE },
    ]);
  });

  it('is empty for a run that failed for another reason', () => {
    expect(runAuthRefusals([step({ output: 'HTTP 500 on GET x — boom' })])).toEqual([]);
  });
});

describe('connectionsNeedingAttention', () => {
  it('keeps only the configs the provider refuses', () => {
    const config = (id: string, auth_failure?: McpConfigDisplay['auth_failure']) =>
      ({ id, auth_failure }) as McpConfigDisplay;
    const refused = config('b', { kind: 'refused', http_status: 401, message: 'Bad credentials', failed_at: '2026-11-10T08:00:00Z' });
    expect(connectionsNeedingAttention([config('a'), refused])).toEqual([refused]);
  });
});
