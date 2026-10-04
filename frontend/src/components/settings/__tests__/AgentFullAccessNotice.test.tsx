import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { I18nProvider } from '../../../lib/I18nContext';
import { AgentFullAccessNotice, FULL_ACCESS_NOTICE_KEY } from '../AgentFullAccessNotice';
import type { AgentEffectiveAccess } from '../../../types/generated';

const rows: AgentEffectiveAccess[] = [
  { agent: 'ClaudeCode', full_access: true },
  { agent: 'Codex', full_access: true, reason: 'forced_in_container' },
  { agent: 'GeminiCli', full_access: false },
];

const show = (value: AgentEffectiveAccess[]) =>
  render(<I18nProvider><AgentFullAccessNotice rows={value} /></I18nProvider>);

beforeEach(() => localStorage.removeItem(FULL_ACCESS_NOTICE_KEY));
afterEach(cleanup);

describe('AgentFullAccessNotice', () => {
  it('lists only the agents that effectively run with full access, with the reason', () => {
    show(rows);
    const notice = screen.getByTestId('full-access-notice');
    expect(notice).toHaveTextContent('Claude Code');
    expect(notice).toHaveTextContent('accès complet activé dans les réglages');
    expect(notice).toHaveTextContent('Toujours actif sous Docker');
    expect(notice).not.toHaveTextContent('Gemini');
    expect(notice).toHaveTextContent('injection de prompt');
  });

  it('is absent when no agent has full access', () => {
    show([{ agent: 'ClaudeCode', full_access: false }]);
    expect(screen.queryByTestId('full-access-notice')).toBeNull();
  });

  it('stays dismissed once acknowledged, across mounts', () => {
    const first = show(rows);
    fireEvent.click(screen.getByRole('button', { name: /Compris/ }));
    expect(screen.queryByTestId('full-access-notice')).toBeNull();
    expect(localStorage.getItem(FULL_ACCESS_NOTICE_KEY)).toBe('1');
    first.unmount();
    show(rows);
    expect(screen.queryByTestId('full-access-notice')).toBeNull();
  });

  it('still shows and dismisses when storage throws', () => {
    vi.spyOn(localStorage, 'getItem').mockImplementation(() => { throw new Error('denied'); });
    vi.spyOn(localStorage, 'setItem').mockImplementation(() => { throw new Error('denied'); });
    show(rows);
    expect(screen.getByTestId('full-access-notice')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Compris/ }));
    expect(screen.queryByTestId('full-access-notice')).toBeNull();
    vi.restoreAllMocks();
  });

  it('scrolls to the switches from the review button', () => {
    const scroll = vi.fn();
    const target = document.createElement('div');
    target.className = 'set-agent-access-row';
    target.scrollIntoView = scroll;
    document.body.appendChild(target);
    show(rows);
    fireEvent.click(screen.getByRole('button', { name: /Revoir les interrupteurs/ }));
    expect(scroll).toHaveBeenCalled();
    target.remove();
  });
});
