/**
 * 2026-06-24 — injected-context card (approach A).
 *
 * A QP/batch message wraps its injected payload (ticket, file…) server-side in
 * a `<!-- kronn:context title="…" -->…<!-- /kronn:context -->` marker so the
 * agent's instructions stay visually distinct from the big injected data,
 * which folds into a collapsible card.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<object>('../../lib/api');
  return { ...real, config: { getUiLanguage: vi.fn().mockResolvedValue('fr') } };
});

import { splitInjectedContext, MessageBody } from '../MessageBubble';

const MARK = (title: string, body: string) =>
  `<!-- kronn:context title="${title}" -->\n${body}\n<!-- /kronn:context -->`;

describe('splitInjectedContext', () => {
  it('returns a single text segment when there is no marker', () => {
    const segs = splitInjectedContext('just instructions');
    expect(segs).toEqual([{ kind: 'text', body: 'just instructions' }]);
  });

  it('splits instructions / context / instructions in order', () => {
    const content = `## Le ticket\n${MARK('ticket EW-7473', '# Ticket body\n- a\n- b')}\n## Méthode`;
    const segs = splitInjectedContext(content);
    expect(segs.map(s => s.kind)).toEqual(['text', 'context', 'text']);
    const ctx = segs[1] as { kind: 'context'; title: string; body: string };
    expect(ctx.title).toBe('ticket EW-7473');
    expect(ctx.body).toContain('# Ticket body');
    expect((segs[0] as { body: string }).body).toContain('## Le ticket');
    expect((segs[2] as { body: string }).body).toContain('## Méthode');
  });

  it('handles multiple context blocks', () => {
    const content = `${MARK('a', 'AAA')} mid ${MARK('b', 'BBB')}`;
    expect(splitInjectedContext(content).filter(s => s.kind === 'context')).toHaveLength(2);
  });
});

describe('MessageBody injected-context card', () => {
  const renderBody = (content: string) =>
    render(<I18nProvider><MessageBody content={content} /></I18nProvider>);

  it('renders a collapsed card (payload hidden) + the surrounding instructions', () => {
    const content = `Triage le ticket.\n${MARK('ticket EW-7473', 'SECRET_PAYLOAD_LINE')}\nRends le rapport.`;
    renderBody(content);
    // instructions visible
    expect(screen.getByText(/Triage le ticket/)).toBeTruthy();
    expect(screen.getByText(/Rends le rapport/)).toBeTruthy();
    // card label visible, with the title
    expect(screen.getByText(/ticket EW-7473/)).toBeTruthy();
    // payload hidden while collapsed
    expect(screen.queryByText(/SECRET_PAYLOAD_LINE/)).toBeNull();
  });

  it('expands the payload on click', () => {
    const content = `Intro.\n${MARK('ticket', 'SECRET_PAYLOAD_LINE')}`;
    renderBody(content);
    expect(screen.queryByText(/SECRET_PAYLOAD_LINE/)).toBeNull();
    fireEvent.click(screen.getByRole('button'));
    expect(screen.getByText(/SECRET_PAYLOAD_LINE/)).toBeTruthy();
  });

  it('renders plain markdown (no card) when there is no marker', () => {
    const { container } = renderBody('# Heading\n\nplain.');
    expect(container.querySelector('.disc-injected-context')).toBeNull();
    expect(container.querySelector('h1')).not.toBeNull();
  });
});
