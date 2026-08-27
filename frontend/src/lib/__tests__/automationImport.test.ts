import { describe, expect, it } from 'vitest';
import { detectAutomationImport } from '../automationImport';

describe('detectAutomationImport', () => {
  it.each([
    ['workflow', 'kronn.workflow', 'workflow'],
    ['qp', 'kronn.quick_prompt', 'quick_prompt'],
    ['qa', 'kronn.quick_api', 'quick_api'],
    ['qe', 'kronn.quick_exec', 'quick_exec'],
  ] as const)('detects a %s export from its discriminator and payload', (kind, discriminator, payloadKey) => {
    expect(detectAutomationImport({
      kind: discriminator,
      [payloadKey]: { name: 'Portable resource' },
    })).toEqual({ ok: true, kind });
  });

  it('rejects an unknown or incomplete envelope', () => {
    expect(detectAutomationImport({ kind: 'kronn.unknown', item: {} }))
      .toEqual({ ok: false, reason: 'invalid' });
    expect(detectAutomationImport({ kind: 'kronn.workflow' }))
      .toEqual({ ok: false, reason: 'invalid' });
  });

  it('rejects a mixed envelope instead of guessing one Automation type', () => {
    expect(detectAutomationImport({
      kind: 'kronn.workflow',
      workflow: { name: 'Workflow' },
      quick_prompt: { name: 'Prompt' },
    })).toEqual({ ok: false, reason: 'ambiguous' });
  });
});
