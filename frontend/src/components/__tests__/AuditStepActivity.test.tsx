// The running audit step's "Details" panel: the agent's latest actions.
import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import type { ReactElement } from 'react';
import { I18nProvider } from '../../lib/I18nContext';
import { AuditStepActivity } from '../AuditStepActivity';
import type { AuditRecentActivity } from '../../types/generated';

const wrap = (ui: ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

const recent: AuditRecentActivity = {
  entries: [
    { tool: 'Edit', target: 'docs/AGENTS.md', at: new Date(Date.now() - 5_000).toISOString() },
    { tool: 'Grep', target: '"useAuth" src/', at: new Date(Date.now() - 65_000).toISOString() },
    { tool: 'TodoWrite', at: new Date(Date.now() - 70_000).toISOString() },
  ],
  thought: 'Checking how the auth hook is used',
};

describe('AuditStepActivity', () => {
  beforeEach(() => {
    try { sessionStorage.clear(); } catch { /* jsdom */ }
  });

  it('is collapsed by default, then lists the actions newest first with their age', () => {
    wrap(<AuditStepActivity projectId="p1" recent={recent} />);
    const toggle = screen.getByRole('button', { expanded: false });
    expect(screen.queryByRole('list')).toBeNull();

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    const entries = screen.getAllByTestId('audit-step-activity-entry');
    expect(entries).toHaveLength(3);
    expect(entries[0]).toHaveTextContent('Edit docs/AGENTS.md');
    expect(entries[1]).toHaveTextContent('Grep "useAuth" src/');
    expect(entries[2]).toHaveTextContent('TodoWrite');
    expect(entries[0].querySelector('time')?.textContent).toMatch(/5/);
    expect(screen.getByRole('list')).toBeInTheDocument();
    expect(screen.getByText(/Checking how the auth hook is used/)).toBeInTheDocument();
  });

  it('shows a waiting state before the first action', () => {
    wrap(<AuditStepActivity projectId="p1" recent={null} />);
    fireEvent.click(screen.getByRole('button'));
    expect(screen.getByTestId('audit-step-activity-empty')).toBeInTheDocument();
    expect(screen.queryAllByTestId('audit-step-activity-entry')).toHaveLength(0);
  });

  it('remembers the open state per project for the session', () => {
    const first = wrap(<AuditStepActivity projectId="p1" recent={recent} />);
    fireEvent.click(screen.getByRole('button'));
    first.unmount();

    const again = wrap(<AuditStepActivity projectId="p1" recent={recent} />);
    expect(screen.getByRole('button')).toHaveAttribute('aria-expanded', 'true');
    again.unmount();

    wrap(<AuditStepActivity projectId="p2" recent={recent} />);
    expect(screen.getByRole('button')).toHaveAttribute('aria-expanded', 'false');
  });
});
