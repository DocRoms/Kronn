/**
 * KT-541 — weight indicator contract.
 *
 * Two properties matter more than the visuals: a pending load must never be
 * mistaken for a measured zero, and the badge must stay operable by keyboard
 * inside a card that is itself a button with a swipe handler.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';
import { DiscussionWeightBadge } from '../DiscussionWeightBadge';
import { formatBytes } from '../../lib/weightFormat';
import type { DiscussionWeightView } from '../../types/generated';

afterEach(cleanup);

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}|${args.join('|')}` : key;

function view(p: Partial<DiscussionWeightView> = {}): DiscussionWeightView {
  return {
    discussion_id: 'd1',
    disk_bytes: 0,
    extracted_text_bytes: 0,
    message_bytes: 0,
    total_bytes: 0,
    reclaimable_bytes: 0,
    level: 'green',
    ...p,
  } as DiscussionWeightView;
}

describe('formatBytes', () => {
  it('scales units and keeps precision only where it informs', () => {
    expect(formatBytes(0)).toBe('0 o');
    expect(formatBytes(512)).toBe('512 o');
    expect(formatBytes(1536)).toBe('1.5 Ko');
    expect(formatBytes(25 * 1024 * 1024)).toBe('25 Mo');
  });
});

describe('DiscussionWeightBadge', () => {
  it('shows no number at all while loading', () => {
    render(<DiscussionWeightBadge state="loading" t={t} />);
    const pending = screen.getByTestId('disc-weight-pending');
    expect(pending).toBeTruthy();
    // The crux: a placeholder "0 o" would read as "this discussion is empty".
    expect(pending.textContent).not.toContain('0');
    expect(screen.queryByTestId('disc-weight-badge')).toBeNull();
  });

  it('shows no number when the weight could not be loaded', () => {
    render(<DiscussionWeightBadge state="unavailable" t={t} />);
    const pending = screen.getByTestId('disc-weight-pending');
    expect(pending.getAttribute('data-state')).toBe('unavailable');
    expect(pending.textContent).not.toContain('0');
  });


  it('shows no number for a row that was never measured', () => {
    // `unmeasured` is a row outside the bounded batch. Rendering "0 o" here
    // would be an invented measurement, not a load state.
    render(<DiscussionWeightBadge state="unmeasured" t={t} />);
    const pending = screen.getByTestId('disc-weight-pending');
    expect(pending.getAttribute('data-state')).toBe('unmeasured');
    expect(pending.textContent).not.toContain('0');
    expect(pending.getAttribute('aria-label')).toContain('disc.weight.unmeasured');
    expect(screen.queryByTestId('disc-weight-badge')).toBeNull();
  });

  it('grades the dot from the level, not from the raw size', () => {
    render(<DiscussionWeightBadge state="ready" t={t} weight={view({ total_bytes: 5, level: 'red' })} />);
    expect(screen.getByTestId('disc-weight-badge').getAttribute('data-level')).toBe('red');
  });

  it('treats an absent weight on a ready batch as a real empty discussion', () => {
    // The response is sparse: `ready` + missing entry means "weighs nothing".
    render(<DiscussionWeightBadge state="ready" t={t} />);
    const badge = screen.getByTestId('disc-weight-badge');
    expect(badge.getAttribute('data-level')).toBe('green');
    expect(badge.textContent).toContain('0 o');
  });

  it('opens the detail with the three masses and the reclaimable line', () => {
    render(
      <DiscussionWeightBadge
        state="ready"
        t={t}
        weight={view({
          disk_bytes: 2048, extracted_text_bytes: 1024, message_bytes: 512,
          total_bytes: 3584, reclaimable_bytes: 2048, level: 'amber',
        })}
      />,
    );
    fireEvent.click(screen.getByTestId('disc-weight-badge'));
    const panel = screen.getByTestId('disc-weight-panel');
    expect(panel.getAttribute('role')).toBe('dialog');
    expect(panel.textContent).toContain('2 Ko');
    expect(panel.textContent).toContain('1 Ko');
    expect(panel.textContent).toContain('512 o');
    expect(screen.getByTestId('disc-weight-reclaimable').textContent).toContain('2 Ko');
  });

  it('does not let its click reach an enclosing clickable row', () => {
    // The badge lives in the card's actions row, a sibling of the card
    // button — never inside it. A div stand-in mirrors that arrangement;
    // wrapping it in a <button> here would reproduce the invalid nesting
    // instead of testing against it.
    const onRowClick = vi.fn();
    render(
      <div onClick={onRowClick}>
        <DiscussionWeightBadge state="ready" t={t} weight={view({ total_bytes: 10 })} />
      </div>,
    );
    fireEvent.click(screen.getByTestId('disc-weight-badge'));
    expect(onRowClick).not.toHaveBeenCalled();
    expect(screen.getByTestId('disc-weight-panel')).toBeTruthy();
  });

  it('renders exactly one interactive element, never nested ones', () => {
    // Regression: the badge was first rendered inside the card's own
    // <button>. Nested interactive content is invalid HTML and triggered a
    // React hydration error.
    const { container } = render(
      <DiscussionWeightBadge state="ready" t={t} weight={view({ total_bytes: 10 })} />,
    );
    const buttons = container.querySelectorAll('button');
    expect(buttons).toHaveLength(1);
    expect(buttons[0].querySelector('button, a, input, select, textarea')).toBeNull();
  });

  it('keeps the pending badge non-interactive so it adds no focus stop', () => {
    const { container } = render(<DiscussionWeightBadge state="loading" t={t} />);
    expect(container.querySelectorAll('button')).toHaveLength(0);
    expect(container.querySelector('[tabindex]')).toBeNull();
  });

  it('exposes its expanded state and closes on Escape', () => {
    render(<DiscussionWeightBadge state="ready" t={t} weight={view({ total_bytes: 10 })} />);
    const badge = screen.getByTestId('disc-weight-badge');
    expect(badge.getAttribute('aria-expanded')).toBe('false');

    fireEvent.click(badge);
    expect(badge.getAttribute('aria-expanded')).toBe('true');
    expect(badge.getAttribute('aria-controls')).toBeTruthy();

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByTestId('disc-weight-panel')).toBeNull();
    expect(badge.getAttribute('aria-expanded')).toBe('false');
  });

  it('is a real button, so it is reachable and operable by keyboard', () => {
    render(<DiscussionWeightBadge state="ready" t={t} weight={view({ total_bytes: 10 })} />);
    const badge = screen.getByTestId('disc-weight-badge') as HTMLButtonElement;
    expect(badge.tagName).toBe('BUTTON');
    expect(badge.getAttribute('aria-label')).toContain('disc.weight.badgeLabel');
    badge.focus();
    expect(document.activeElement).toBe(badge);
  });
});
