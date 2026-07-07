// 0.8.5 — Kronn seed-payload toggle.
//
// The QP AI Improver (and future AI-creation flows) post a User
// message containing both a short visible status line and the full
// technical seed (QP JSON + catalog + instructions). MessageBubble
// must:
//   - Split the visible prefix from the marker-wrapped seed.
//   - Render only the visible prefix as primary markdown.
//   - Render the seed inside a collapsed toggle, opened on click.
//
// We test the pure helper directly + the toggle's open/close UX via
// React Testing Library. The actual MessageBubble integration is
// covered by SmokeTests + the higher-level DiscussionsPage tests.

import { describe, it, expect } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { useState } from 'react';
import { splitMessageSeed } from '../MessageBubble';

describe('splitMessageSeed (0.8.5)', () => {
  it('returns content as-is when no marker is present', () => {
    const r = splitMessageSeed('Just a plain message');
    expect(r.visible).toBe('Just a plain message');
    expect(r.seed).toBeNull();
  });

  it('splits on the marker pair and trims both halves', () => {
    const content =
      '✨ Audit en cours…\n' +
      '<!--KRONN_SEED_START-->\n' +
      'Heavy technical seed\n```json\n{"foo": "bar"}\n```\n' +
      '<!--KRONN_SEED_END-->';
    const r = splitMessageSeed(content);
    expect(r.visible).toBe('✨ Audit en cours…');
    expect(r.seed).toContain('Heavy technical seed');
    expect(r.seed).toContain('"foo": "bar"');
  });

  it('handles multi-line visible prefix before the marker', () => {
    const content =
      'Line A\nLine B\n<!--KRONN_SEED_START-->seed body<!--KRONN_SEED_END-->';
    const r = splitMessageSeed(content);
    expect(r.visible).toBe('Line A\nLine B');
    expect(r.seed).toBe('seed body');
  });

  it('returns null seed when only the start marker is present (malformed)', () => {
    // Defensive: a half-marker should NOT swallow the rest of the
    // content. We fall back to "whole content is visible".
    const r = splitMessageSeed('intro <!--KRONN_SEED_START--> no end marker');
    expect(r.visible).toBe('intro <!--KRONN_SEED_START--> no end marker');
    expect(r.seed).toBeNull();
  });

  it('keeps content with KRONN:* signals when no seed marker is present', () => {
    // Regression guard: don't accidentally swallow signal lines.
    const r = splitMessageSeed('Some text\nKRONN:QP_IMPROVED');
    expect(r.visible).toBe('Some text\nKRONN:QP_IMPROVED');
    expect(r.seed).toBeNull();
  });

  it('non-greedy match across multiple potential marker pairs', () => {
    // If a message somehow contains two marker pairs, only the first
    // is consumed (visible = before-first, seed = first-pair-body).
    // The remainder of the message stays inside the seed string —
    // acceptable because future markdown rendering would have shown
    // it anyway, and no current Kronn flow emits two seed envelopes.
    const content =
      'pre' +
      '<!--KRONN_SEED_START-->A<!--KRONN_SEED_END-->' +
      'mid' +
      '<!--KRONN_SEED_START-->B<!--KRONN_SEED_END-->' +
      'post';
    const r = splitMessageSeed(content);
    expect(r.visible).toBe('pre');
    expect(r.seed).toContain('A');
  });
});

// Component-level render: confirm the toggle starts closed, opens on
// click, and exposes the seed body. We don't go through the full
// MessageBubble (it requires a heavy mocked context); the toggle is
// what we care about UX-wise.
describe('Kronn seed toggle (via splitMessageSeed + manual render)', () => {
  it('renders only the visible prefix in the markdown surface (no seed text)', async () => {
    const { default: ReactMarkdown } = await import('react-markdown');
    const content =
      '✨ Audit en cours…\n<!--KRONN_SEED_START-->\nthis-should-not-appear\n<!--KRONN_SEED_END-->';
    const { visible } = splitMessageSeed(content);
    render(<ReactMarkdown>{visible}</ReactMarkdown>);
    expect(screen.getByText(/Audit en cours/)).toBeInTheDocument();
    expect(screen.queryByText(/this-should-not-appear/)).toBeNull();
  });

  it('seed is parsed out and available for a separate disclosure widget', () => {
    const content =
      '✨ Audit en cours…\n<!--KRONN_SEED_START-->\nseed-body\n<!--KRONN_SEED_END-->';
    const { seed } = splitMessageSeed(content);
    expect(seed).toBe('seed-body');
    // Smoke render of a minimal disclosure that mirrors the production
    // KronnSeedToggle, so the assertion stays decoupled from styling.
    function Toggle({ payload }: { payload: string }) {
      const [open, setOpen] = useState(false);
      return (
        <div>
          <button onClick={() => setOpen(o => !o)}>{open ? '▾' : '▸'} Contexte</button>
          {open && <pre data-testid="seed">{payload}</pre>}
        </div>
      );
    }
    render(<Toggle payload={seed ?? ''} />);
    // Seed not visible until the user clicks.
    expect(screen.queryByTestId('seed')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /Contexte/i }));
    expect(screen.getByTestId('seed')).toHaveTextContent('seed-body');
  });
});
