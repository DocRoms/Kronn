/**
 * Regression guard: message content must render `:shortcode:` as the
 * corresponding Unicode emoji. Added 2026-04-15 alongside the emoji
 * autocomplete feature — the conversion is handled by `remark-emoji`
 * inside `MarkdownContent`, so this test also protects against the
 * plugin being silently dropped from `remarkPluginsList`.
 *
 * We render the exported `MarkdownContent` in isolation to keep the
 * test focused: MessageBubble itself has lots of unrelated wiring (TTS,
 * copy buttons, retry, timestamps…) that would only add noise.
 */
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MarkdownContent } from '../MessageBubble';

describe('MarkdownContent — emoji shortcode rendering', () => {
  it('replaces :tada: with 🎉', () => {
    render(<MarkdownContent content="Bravo :tada: bien joué" />);
    // Shortcode must NOT survive in the DOM — the whole point of the
    // conversion is that readers see the glyph, not the ASCII form.
    expect(screen.queryByText(/:tada:/)).toBeNull();
    // The emoji must be somewhere in the rendered text.
    expect(document.body.textContent).toContain('🎉');
  });

  it('handles multiple shortcodes in the same message', () => {
    render(<MarkdownContent content=":rocket: ship it :fire:" />);
    const txt = document.body.textContent ?? '';
    expect(txt).toContain('🚀');
    expect(txt).toContain('🔥');
    expect(txt).not.toMatch(/:rocket:|:fire:/);
  });

  it('leaves unknown shortcodes intact (no silent data loss)', () => {
    // `:definitely_not_an_emoji_ever:` is not a GitHub shortcode — the
    // renderer must pass it through verbatim rather than drop it.
    render(<MarkdownContent content="status :definitely_not_an_emoji_ever: unknown" />);
    expect(document.body.textContent).toContain(':definitely_not_an_emoji_ever:');
  });

  it('emoji rendering cohabits with other markdown features', () => {
    // Bold + emoji in the same line — the remark pipeline processes both
    // plugins (gfm then emoji) without stepping on each other.
    render(<MarkdownContent content="**success** :white_check_mark:" />);
    expect(document.body.textContent).toContain('success');
    expect(document.body.textContent).toContain('✅');
  });
});
