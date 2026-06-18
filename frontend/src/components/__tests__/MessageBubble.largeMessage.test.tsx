/**
 * 2026-06-23 — guard against multi-MB messages crashing the browser tab.
 *
 * A killed Codex run persisted a 2.4 MB stderr/reasoning dump as its reply;
 * opening that discussion sent it through ReactMarkdown + remark-gfm + syntax
 * highlight (super-linear) and crashed Chrome. `MarkdownContent` now renders
 * anything past ~200 KB as plain text instead.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<object>('../../lib/api');
  return { ...real, config: { getUiLanguage: vi.fn().mockResolvedValue('fr') } };
});

import { MarkdownContent } from '../MessageBubble';

const renderMd = (content: string) =>
  render(<I18nProvider><MarkdownContent content={content} /></I18nProvider>);

describe('MarkdownContent large-message guard', () => {
  it('renders a normal message through markdown (h1 element present)', () => {
    const { container } = renderMd('# Hello\n\nA normal short reply.');
    expect(container.querySelector('h1')).not.toBeNull();
  });

  it('renders an oversized message as PLAIN TEXT, not markdown (no h1, raw # kept)', () => {
    // > MAX_MARKDOWN_CHARS (200k). A leading markdown header that, if parsed,
    // would become an <h1> — proves markdown was bypassed when it stays literal.
    const huge = '# BIGTITLE_LITERAL\n' + 'x'.repeat(250_000);
    const { container } = renderMd(huge);
    // markdown NOT applied → no heading element
    expect(container.querySelector('h1')).toBeNull();
    // the raw markdown source is shown verbatim (plain text)
    expect(screen.getByText(/# BIGTITLE_LITERAL/)).toBeTruthy();
    // a notice banner is present
    expect(screen.getByRole('note')).toBeTruthy();
  });

  it('truncates the inline plain text for a very large message', () => {
    const huge = 'A'.repeat(500_000);
    const { container } = renderMd(huge);
    const pre = container.querySelector('pre');
    expect(pre).not.toBeNull();
    // inline render is capped well under the full 500k (truncation marker added)
    expect((pre!.textContent ?? '').length).toBeLessThan(150_000);
  });
});
