// 0.8.5 — QP AI Improver signal parsing.
//
// Pin the logic the DiscussionsPage banner uses to recover the target
// QP id from the discussion title + extract the refactored QP JSON
// from the agent message. The actual JSX rendering is covered
// by Playwright E2E on a real DOCROMS_WEB project; this file pins the
// pure parsing rules so a regex tweak can't silently break the flow.

import { describe, it, expect } from 'vitest';

const TITLE_REGEX = /^\[Improve QP (qp-[^\]]+|[0-9a-f-]+)\]/i;
const SIGNAL = 'KRONN:QP_IMPROVED';

function extractTargetId(title: string): string | null {
  const m = title.match(TITLE_REGEX);
  return m ? m[1] : null;
}

function extractRefactoredJson(content: string): Record<string, unknown> | null {
  if (!content.toUpperCase().includes(SIGNAL)) return null;
  const m = content.match(/```json\s*\n([\s\S]*?)\n```/);
  if (!m) return null;
  try {
    const v: unknown = JSON.parse(m[1]);
    if (v && typeof v === 'object') return v as Record<string, unknown>;
  } catch { /* fall through */ }
  return null;
}

describe('QP improver signal parsing (0.8.5)', () => {
  it('recovers the QP id from the canonical title prefix', () => {
    expect(extractTargetId('[Improve QP qp-abc] Analyse Jira ticket'))
      .toBe('qp-abc');
  });

  it('recovers a uuid-shaped QP id', () => {
    expect(extractTargetId('[Improve QP 11111111-2222-3333-4444-555555555555] Foo'))
      .toBe('11111111-2222-3333-4444-555555555555');
  });

  it('returns null for a non-improve title', () => {
    expect(extractTargetId('Briefing for project')).toBeNull();
    expect(extractTargetId('[Audit] Foo')).toBeNull();
  });

  it('requires the prefix at the start of the title', () => {
    expect(extractTargetId('Some preamble [Improve QP qp-x] Foo')).toBeNull();
  });

  it('returns null when the signal is absent', () => {
    expect(extractRefactoredJson('Some markdown without the signal.\n```json\n{"name": "X"}\n```'))
      .toBeNull();
  });

  it('extracts the first ```json block when the signal is present', () => {
    const content = 'Audit table here.\n\n```json\n{"name": "X", "prompt_template": "Y"}\n```\n\nKRONN:QP_IMPROVED';
    const parsed = extractRefactoredJson(content);
    expect(parsed).not.toBeNull();
    expect(parsed!.name).toBe('X');
    expect(parsed!.prompt_template).toBe('Y');
  });

  it('returns null when the JSON block is malformed', () => {
    const content = '```json\n{not valid json}\n```\nKRONN:QP_IMPROVED';
    expect(extractRefactoredJson(content)).toBeNull();
  });

  it('returns null when there is no fenced json block', () => {
    expect(extractRefactoredJson('Just text and KRONN:QP_IMPROVED')).toBeNull();
  });

  it('is case-insensitive on the signal', () => {
    const content = '```json\n{"name": "X"}\n```\nkronn:qp_improved';
    expect(extractRefactoredJson(content)).not.toBeNull();
  });

  it('handles multi-line prompt_template inside the JSON', () => {
    const content = `\`\`\`json
{
  "name": "X",
  "prompt_template": "Line 1\\nLine 2\\nLine 3"
}
\`\`\`
KRONN:QP_IMPROVED`;
    const parsed = extractRefactoredJson(content);
    expect(parsed).not.toBeNull();
    expect(parsed!.prompt_template).toBe('Line 1\nLine 2\nLine 3');
  });
});
