// KT-1880 — Tabs: the tablist / tab / tabpanel roles, one tab in the Tab
// order, arrows that move and select, a disabled tab skipped.
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { Tabs, type TabItem } from './Tabs';

const TABS: TabItem[] = [
  { id: 'overview', label: 'Overview', panel: <p>Overview panel</p> },
  { id: 'activity', label: 'Activity', panel: <p>Activity panel</p>, badge: 3 },
  { id: 'archived', label: 'Archived', panel: <p>Archived panel</p>, disabled: true },
  { id: 'files', label: 'Files', panel: <p>Files panel</p> },
];

describe('Tabs', () => {
  it('exposes a named tablist, tabs and the selected panel', () => {
    render(<Tabs label="Project views" tabs={TABS} />);
    expect(screen.getByRole('tablist', { name: 'Project views' })).toBeInTheDocument();
    const tabs = screen.getAllByRole('tab');
    expect(tabs.map(t => t.textContent)).toEqual(['Overview', 'Activity3', 'Archived', 'Files']);
    expect(tabs[0]).toHaveAttribute('aria-selected', 'true');
    expect(tabs[1]).toHaveAttribute('aria-selected', 'false');
    const panel = screen.getByRole('tabpanel', { name: 'Overview' });
    expect(panel).toHaveTextContent('Overview panel');
    expect(tabs[0]).toHaveAttribute('aria-controls', panel.id);
    expect(screen.queryByText('Activity panel')).toBeNull();
  });

  it('keeps only the selected tab in the Tab order, then the panel', async () => {
    const user = userEvent.setup();
    render(<Tabs label="Project views" tabs={TABS} defaultValue="activity" />);
    expect(screen.getAllByRole('tab').map(t => t.tabIndex)).toEqual([-1, 0, -1, -1]);
    await user.tab();
    expect(screen.getByRole('tab', { name: /Activity/ })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole('tabpanel')).toHaveFocus();
  });

  it('moves and selects with the arrows, skipping the disabled tab and wrapping', async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(<Tabs label="Project views" tabs={TABS} onChange={onChange} />);
    await user.tab();
    await user.keyboard('{ArrowRight}');
    expect(screen.getByRole('tab', { name: /Activity/ })).toHaveFocus();
    expect(screen.getByRole('tabpanel')).toHaveTextContent('Activity panel');
    await user.keyboard('{ArrowRight}');
    expect(screen.getByRole('tab', { name: 'Files' })).toHaveFocus();
    await user.keyboard('{ArrowRight}');
    expect(screen.getByRole('tab', { name: 'Overview' })).toHaveFocus();
    await user.keyboard('{ArrowLeft}');
    expect(screen.getByRole('tab', { name: 'Files' })).toHaveFocus();
    await user.keyboard('{Home}');
    expect(screen.getByRole('tab', { name: 'Overview' })).toHaveFocus();
    await user.keyboard('{End}');
    expect(screen.getByRole('tab', { name: 'Files' })).toHaveAttribute('aria-selected', 'true');
    expect(onChange.mock.calls.map(c => c[0])).toEqual(['activity', 'files', 'overview', 'files', 'overview', 'files']);
  });

  it('selects on click, and a disabled tab cannot be chosen', async () => {
    const user = userEvent.setup();
    render(<Tabs label="Project views" tabs={TABS} />);
    await user.click(screen.getByRole('tab', { name: 'Files' }));
    expect(screen.getByRole('tabpanel')).toHaveTextContent('Files panel');
    await user.click(screen.getByRole('tab', { name: 'Archived' }));
    expect(screen.getByRole('tab', { name: 'Files' })).toHaveAttribute('aria-selected', 'true');
  });

  it('follows a value held by the caller', async () => {
    function Controlled() {
      const [value, setValue] = useState('files');
      return <Tabs label="Project views" tabs={TABS} value={value} onChange={setValue} />;
    }
    const user = userEvent.setup();
    render(<Controlled />);
    expect(screen.getByRole('tab', { name: 'Files' })).toHaveAttribute('aria-selected', 'true');
    await user.click(screen.getByRole('tab', { name: 'Overview' }));
    expect(screen.getByRole('tabpanel')).toHaveTextContent('Overview panel');
  });

  it('without panels, only reports the selection and claims no panel', async () => {
    const onChange = vi.fn();
    render(<Tabs label="Filter" tabs={[{ id: 'all', label: 'All' }, { id: 'mine', label: 'Mine' }]} onChange={onChange} />);
    expect(screen.queryByRole('tabpanel')).toBeNull();
    expect(screen.getByRole('tab', { name: 'All' })).not.toHaveAttribute('aria-controls');
    await userEvent.click(screen.getByRole('tab', { name: 'Mine' }));
    expect(onChange).toHaveBeenCalledWith('mine');
  });
});
