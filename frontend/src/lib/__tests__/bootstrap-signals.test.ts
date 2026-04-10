import { describe, it, expect } from 'vitest';

// Test the signal stripping regex (same as MessageBubble.tsx)
const SIGNAL_REGEX = /KRONN:(BRIEFING_COMPLETE|VALIDATION_COMPLETE|BOOTSTRAP_COMPLETE|WORKFLOW_READY|REPO_READY|ARCHITECTURE_READY|PLAN_READY|STRUCTURE_READY|ISSUES_READY|ISSUES_CREATED)/gi;

describe('Bootstrap++ signal detection', () => {
  it('strips ARCHITECTURE_READY from message content', () => {
    const content = 'Architecture summary here.\n\nKRONN:ARCHITECTURE_READY';
    const cleaned = content.replace(SIGNAL_REGEX, '').trim();
    expect(cleaned).toBe('Architecture summary here.');
  });

  it('strips PLAN_READY from message content', () => {
    const content = 'Plan is ready.\nKRONN:PLAN_READY';
    const cleaned = content.replace(SIGNAL_REGEX, '').trim();
    expect(cleaned).toBe('Plan is ready.');
  });

  it('strips ISSUES_CREATED from message content', () => {
    const content = 'Created 12 issues.\nKRONN:ISSUES_CREATED';
    const cleaned = content.replace(SIGNAL_REGEX, '').trim();
    expect(cleaned).toBe('Created 12 issues.');
  });

  it('is case insensitive', () => {
    const content = 'Done.\nkronn:architecture_ready';
    const cleaned = content.replace(SIGNAL_REGEX, '').trim();
    expect(cleaned).toBe('Done.');
  });

  it('does not strip partial matches', () => {
    const content = 'KRONN:UNKNOWN_SIGNAL should stay';
    const cleaned = content.replace(SIGNAL_REGEX, '').trim();
    expect(cleaned).toBe('KRONN:UNKNOWN_SIGNAL should stay');
  });

  it('strips all known signals', () => {
    const signals = [
      'KRONN:BRIEFING_COMPLETE',
      'KRONN:VALIDATION_COMPLETE',
      'KRONN:BOOTSTRAP_COMPLETE',
      'KRONN:WORKFLOW_READY',
      'KRONN:REPO_READY',
      'KRONN:ARCHITECTURE_READY',
      'KRONN:PLAN_READY',
      'KRONN:ISSUES_CREATED',
    ];
    for (const signal of signals) {
      const cleaned = `Text.\n${signal}`.replace(SIGNAL_REGEX, '').trim();
      expect(cleaned).toBe('Text.');
    }
  });

  it('strips REPO_READY from message content', () => {
    const content = 'Repo created at github.com/foo/bar.\nKRONN:REPO_READY';
    const cleaned = content.replace(SIGNAL_REGEX, '').trim();
    expect(cleaned).toBe('Repo created at github.com/foo/bar.');
  });

  it('strips ISSUES_READY (LLM hallucinates this variant)', () => {
    // Real bug: agent emitted KRONN:ISSUES_READY instead of ISSUES_CREATED
    // because the *_READY family (REPO_READY, ARCHITECTURE_READY, PLAN_READY)
    // makes Claude "harmonize" the last signal name. Both must be recognized.
    const content = 'Created 13 epics.\nKRONN:ISSUES_READY';
    const cleaned = content.replace(SIGNAL_REGEX, '').trim();
    expect(cleaned).toBe('Created 13 epics.');
  });

  it('strips STRUCTURE_READY (LLM hallucinates this PLAN_READY variant)', () => {
    // Same hallucination pattern: when Stage 2 produces a "structure" /
    // "modules / chantiers" breakdown, Claude invents STRUCTURE_READY
    // instead of PLAN_READY. We accept it as an alias.
    const content = 'Modules Core/Dilem/Shared, 15 chantiers.\nKRONN:STRUCTURE_READY';
    const cleaned = content.replace(SIGNAL_REGEX, '').trim();
    expect(cleaned).toBe('Modules Core/Dilem/Shared, 15 chantiers.');
  });
});

describe('Bootstrap signal detection in messages', () => {
  it('detects ARCHITECTURE_READY in agent message', () => {
    const content = 'Here is the architecture.\n\nKRONN:ARCHITECTURE_READY';
    expect(content.toUpperCase().includes('KRONN:ARCHITECTURE_READY')).toBe(true);
    expect(content.toUpperCase().includes('KRONN:PLAN_READY')).toBe(false);
  });

  it('detects PLAN_READY after architecture validation', () => {
    const content = 'Here is the plan.\n\nKRONN:PLAN_READY';
    expect(content.toUpperCase().includes('KRONN:PLAN_READY')).toBe(true);
  });

  it('detects ISSUES_CREATED after plan validation', () => {
    const content = 'Created 8 issues on GitHub.\n\nKRONN:ISSUES_CREATED';
    expect(content.toUpperCase().includes('KRONN:ISSUES_CREATED')).toBe(true);
  });

  it('isBootstrapDisc matches Bootstrap: prefix', () => {
    const isBootstrapDisc = (title: string) => title.startsWith('Bootstrap: ');
    expect(isBootstrapDisc('Bootstrap: MyApp')).toBe(true);
    expect(isBootstrapDisc('Regular discussion')).toBe(false);
    expect(isBootstrapDisc('bootstrap: lowercase')).toBe(false);
  });
});
