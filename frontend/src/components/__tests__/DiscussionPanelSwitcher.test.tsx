/**
 * KT-581 — switching panels, next to the panel being switched.
 *
 * The header used to carry six panel buttons. A floating list fixed the width
 * but showed each icon twice, because every panel already draws its own
 * header. So the switcher sits above the open panel and disappears with it.
 *
 * These assertions moved here from the header's own spec: what they guard —
 * the pending-file count, its cap, the tooltip that names it — did not change,
 * only where it is rendered.
 */
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import { DiscussionPanelSwitcher, type SwitchablePanel } from '../DiscussionPanelSwitcher';

afterEach(cleanup);

const panel = (over: Partial<SwitchablePanel> & { id: string }): SwitchablePanel => ({
  label: over.id,
  icon: <span />,
  active: false,
  onSelect: vi.fn(),
  ...over,
});

const renderSwitcher = (panels: SwitchablePanel[], onToggleColumn = vi.fn()) =>
  render(
    <DiscussionPanelSwitcher
      panels={panels}
      groupLabel="panels"
      openLabel="open"
      closeLabel="close"
      onToggleColumn={onToggleColumn}
    />,
  );

describe('DiscussionPanelSwitcher', () => {
  it('stays a narrow strip while every panel is closed', () => {
    renderSwitcher([panel({ id: 'plan' }), panel({ id: 'git' })]);
    // The strip is always there — it holds the control that opens the column,
    // and that control has to sit against the panel it opens. Only the panel
    // icons wait for a panel to move away from.
    expect(screen.getByTestId('panel-open-toggle')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'plan' })).toBeNull();
  });

  it('opens the column from the strip, not from the discussion header', () => {
    const onToggleColumn = vi.fn();
    renderSwitcher([panel({ id: 'plan' })], onToggleColumn);
    fireEvent.click(screen.getByTestId('panel-open-toggle'));
    expect(onToggleColumn).toHaveBeenCalledTimes(1);
  });

  it('shows the panel icons once one is open', () => {
    renderSwitcher([panel({ id: 'plan', active: true }), panel({ id: 'git' })]);
    expect(screen.getByRole('button', { name: 'plan' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'git' })).toBeInTheDocument();
  });

  it('shows a pending count on the panel that has one', () => {
    renderSwitcher([panel({ id: 'git', label: 'Files', badge: 3, active: true })]);
    const badge = document.querySelector('[data-panel="git"] .disc-panel-switcher-badge');
    expect(badge?.textContent).toBe('3');
  });

  it('caps that count so a long number cannot overrun its icon', () => {
    renderSwitcher([panel({ id: 'git', label: 'Files', badge: '9+', active: true })]);
    expect(
      document.querySelector('[data-panel="git"] .disc-panel-switcher-badge')?.textContent,
    ).toBe('9+');
  });

  it('carries no badge when nothing is pending', () => {
    renderSwitcher([panel({ id: 'git', label: 'Files', active: true })]);
    expect(document.querySelector('.disc-panel-switcher-badge')).toBeNull();
  });

  it('names the count in the label, so it is readable without hovering', () => {
    renderSwitcher([panel({ id: 'git', label: '5 files pending', badge: 5, active: true })]);
    const button = screen.getByRole('button', { name: '5 files pending' });
    expect(button.getAttribute('title')).toBe('5 files pending');
  });

  it('announces a panel as expandable, not as a pressed switch', () => {
    renderSwitcher([panel({ id: 'plan', label: 'Plan', active: true })]);
    const item = screen.getByRole('button', { name: 'Plan' });
    // `expanded` is what makes a screen reader say "collapsed" rather than
    // "not pressed": these reveal a region, they are not settings.
    expect(item).toHaveAttribute('aria-expanded', 'true');
    expect(item).not.toHaveAttribute('aria-pressed');
  });

  it('hands the click back untouched, so exclusivity stays where it is decided', () => {
    const onSelect = vi.fn();
    renderSwitcher([panel({ id: 'plan', active: true }), panel({ id: 'git', label: 'Files', onSelect })]);
    fireEvent.click(screen.getByRole('button', { name: 'Files' }));
    // Which panels may be open together is the page's rule — it already owns
    // the Escape-closes-all behaviour, and splitting it in two is how the two
    // halves drift apart.
    expect(onSelect).toHaveBeenCalledTimes(1);
  });
});
