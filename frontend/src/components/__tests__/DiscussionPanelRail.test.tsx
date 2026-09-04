/**
 * KT-581 — the panels moved out of the header row into a rail.
 *
 * What matters is not that a list renders: it is that folding the list does
 * not hide what the header used to say. The old row showed pending files and
 * pending proposals directly, and a rail that swallows those counts trades a
 * crowded header for a blind one.
 */
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import { DiscussionPanelRail, type RailPanel } from '../DiscussionPanelRail';

afterEach(() => {
  cleanup();
  localStorage.removeItem('kronn:panelRailExpanded');
});

const panel = (over: Partial<RailPanel> & { id: string }): RailPanel => ({
  label: over.id,
  icon: <span />,
  active: false,
  onToggle: vi.fn(),
  ...over,
});

const renderRail = (panels: RailPanel[]) =>
  render(
    <DiscussionPanelRail
      panels={panels}
      expandLabel="open"
      collapseLabel="close"
      groupLabel="panels"
    />,
  );

describe('DiscussionPanelRail', () => {
  it('starts folded and shows nothing but its own control', () => {
    renderRail([panel({ id: 'plan' }), panel({ id: 'git' })]);
    expect(screen.queryByTestId('panel-rail-list')).toBeNull();
    expect(screen.getByTestId('panel-rail-toggle')).toHaveAttribute('aria-expanded', 'false');
  });

  it('lists every panel with a readable label, not an icon to decipher', () => {
    renderRail([panel({ id: 'plan', label: 'Plan' }), panel({ id: 'git', label: 'Files' })]);
    fireEvent.click(screen.getByTestId('panel-rail-toggle'));
    expect(screen.getByRole('button', { name: 'Plan' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Files' })).toBeInTheDocument();
  });

  it('says which panel is open once folded again', () => {
    renderRail([panel({ id: 'plan', label: 'Plan', active: true }), panel({ id: 'git' })]);
    // Folded, the control still names the open panel: hiding the list must
    // not hide what is currently showing beside it.
    expect(screen.getByTestId('panel-rail-toggle')).toHaveAttribute('data-open-panel', 'plan');
    expect(screen.getByTestId('panel-rail-toggle')).toHaveTextContent('Plan');
  });

  it('keeps a pending count visible while folded', () => {
    renderRail([panel({ id: 'git', label: 'Files', badge: 5 }), panel({ id: 'plan' })]);
    // The header used to show this count directly. Requiring a click to learn
    // that something is waiting would be a loss, not a simplification.
    expect(screen.getByTestId('panel-rail-attention')).toBeInTheDocument();
  });

  it('shows no marker when nothing is pending', () => {
    renderRail([panel({ id: 'git', label: 'Files' }), panel({ id: 'plan' })]);
    expect(screen.queryByTestId('panel-rail-attention')).toBeNull();
  });

  it('announces a panel as expandable, not as a pressed switch', () => {
    renderRail([panel({ id: 'plan', label: 'Plan', active: true })]);
    fireEvent.click(screen.getByTestId('panel-rail-toggle'));
    const item = screen.getByRole('button', { name: 'Plan' });
    // `expanded` is what makes a screen reader say "collapsed" rather than
    // "not pressed" — these open a region, they are not on/off switches.
    expect(item).toHaveAttribute('aria-expanded', 'true');
    expect(item).not.toHaveAttribute('aria-pressed');
  });

  it('hands the click back untouched, so exclusivity stays where it is decided', () => {
    const onToggle = vi.fn();
    renderRail([panel({ id: 'git', label: 'Files', onToggle })]);
    fireEvent.click(screen.getByTestId('panel-rail-toggle'));
    fireEvent.click(screen.getByRole('button', { name: 'Files' }));
    // The rail does not own which panels may be open together: the page does,
    // and it closes the others. Deciding here would split that rule in two.
    expect(onToggle).toHaveBeenCalledTimes(1);
  });

  it('remembers being unfolded, and folds again on Escape', () => {
    const { unmount } = renderRail([panel({ id: 'plan', label: 'Plan' })]);
    fireEvent.click(screen.getByTestId('panel-rail-toggle'));
    expect(screen.getByTestId('panel-rail-list')).toBeInTheDocument();

    unmount();
    renderRail([panel({ id: 'plan', label: 'Plan' })]);
    expect(screen.getByTestId('panel-rail-list')).toBeInTheDocument();

    // Escape puts the list away without closing the panel behind it.
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.queryByTestId('panel-rail-list')).toBeNull();
  });
});
