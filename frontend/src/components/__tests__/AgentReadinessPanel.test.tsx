import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { AgentReadinessNotice, AgentReadinessPanel } from '../AgentReadinessPanel';
import { readinessFixPath, readinessMessage } from '../../lib/agentReadiness';
import type { AgentReadiness } from '../../types/generated';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}(${args.join('|')})` : key;

const result = (overrides: Partial<AgentReadiness>): AgentReadiness => ({
  agent_type: 'OpenCode',
  status: 'not_ready',
  reason: 'session_timeout',
  message_key: 'readiness.reason.session_timeout',
  servers: [],
  secs: null,
  detail: null,
  cached: false,
  checked_at: '2026-10-09T10:00:00Z',
  ...overrides,
});

describe('readinessMessage', () => {
  it('names the agent, the bound and the servers a stalled session was starting', () => {
    expect(readinessMessage(result({ secs: 90, servers: ['Hang', 'Memory'] }), t)).toBe(
      'readiness.reason.session_timeout(OpenCode|90) readiness.servers(Hang, Memory)',
    );
  });

  it('quotes the runtime error of a session that failed', () => {
    expect(readinessMessage(result({
      reason: 'session_failed', message_key: 'readiness.reason.session_failed', detail: 'MCP exited',
    }), t)).toBe('readiness.reason.session_failed(OpenCode|MCP exited)');
  });
});

describe('readinessFixPath', () => {
  it('sends a session that would not start to the plugins, anything else to the agent settings', () => {
    expect(readinessFixPath(result({ reason: 'session_timeout' }))).toBe('/plugins');
    expect(readinessFixPath(result({ reason: 'session_failed' }))).toBe('/plugins');
    expect(readinessFixPath(result({ reason: 'not_logged_in' }))).toBe('/config#settings-agent-config');
    expect(readinessFixPath(result({ reason: 'not_installed' }))).toBe('/config#settings-agent-config');
  });
});

describe('AgentReadinessPanel — fix in a new tab', () => {
  it.each([
    ['a session that would not start', { reason: 'session_timeout' as const }, '/plugins'],
    ['an agent that is not logged in', { reason: 'not_logged_in' as const, message_key: 'readiness.reason.not_logged_in' }, '/config#settings-agent-config'],
  ])('opens where %s is fixed in a new tab on a Ctrl-click, without fixing here', (_name, overrides, path) => {
    const open = vi.spyOn(window, 'open').mockImplementation(() => null);
    const onFix = vi.fn();
    render(<AgentReadinessPanel agents={['OpenCode']} results={[result(overrides)]} onFix={onFix} t={t} />);

    fireEvent.click(screen.getByText('readiness.fix'), { ctrlKey: true });

    expect(open).toHaveBeenCalledWith(`${window.location.origin}${path}`, '_blank', 'noopener,noreferrer');
    expect(onFix).not.toHaveBeenCalled();
    open.mockRestore();
  });

  it('fixes in this tab on a plain click', () => {
    const open = vi.spyOn(window, 'open').mockImplementation(() => null);
    const onFix = vi.fn();
    const failing = result({});
    render(<AgentReadinessPanel agents={['OpenCode']} results={[failing]} onFix={onFix} t={t} />);

    fireEvent.click(screen.getByText('readiness.fix'));

    expect(onFix).toHaveBeenCalledExactlyOnceWith(failing);
    expect(open).not.toHaveBeenCalled();
    open.mockRestore();
  });
});

describe('AgentReadinessPanel', () => {
  it('shows every agent as checking until the results arrive, and offers launch anyway', () => {
    const onLaunchAnyway = vi.fn();
    render(
      <AgentReadinessPanel agents={['ClaudeCode', 'OpenCode']} results={null} onLaunchAnyway={onLaunchAnyway} t={t} />,
    );
    const panel = screen.getByTestId('agent-readiness');
    expect(panel).toHaveAttribute('data-state', 'checking');
    expect(panel).toHaveTextContent('Claude Code');
    expect(panel).toHaveTextContent('OpenCode');
    expect(screen.queryByText('readiness.recheck')).toBeNull();
    fireEvent.click(screen.getByText('readiness.launchAnyway'));
    expect(onLaunchAnyway).toHaveBeenCalledOnce();
  });

  it('offers fix and remove on a failing agent only, and unknown shows ❔', () => {
    const onRemove = vi.fn();
    render(
      <AgentReadinessPanel
        agents={['ClaudeCode', 'OpenCode']}
        results={[
          result({ agent_type: 'ClaudeCode', status: 'unknown', reason: 'login_unverified', message_key: 'readiness.reason.login_unverified' }),
          result({ secs: 90 }),
        ]}
        onFix={vi.fn()}
        onRemove={onRemove}
        t={t}
      />,
    );
    expect(screen.getByTestId('agent-readiness')).toHaveTextContent('❔');
    expect(screen.getAllByText('readiness.remove')).toHaveLength(1);
    fireEvent.click(screen.getByText('readiness.remove'));
    expect(onRemove).toHaveBeenCalledWith('OpenCode');
  });
});

describe('AgentReadinessNotice', () => {
  it('shows the compact state of a room launched anyway, and can be dismissed', () => {
    const onDismiss = vi.fn();
    render(
      <AgentReadinessNotice
        results={[
          result({ agent_type: 'Codex', status: 'ready', reason: 'ready', message_key: 'readiness.reason.ready' }),
          result({ secs: 90 }),
        ]}
        onDismiss={onDismiss}
        t={t}
      />,
    );
    const notice = screen.getByTestId('agent-readiness-notice');
    expect(notice).toHaveAttribute('data-state', 'blocked');
    expect(notice).toHaveTextContent('readiness.noticeLaunchedAnyway');
    expect(notice).toHaveTextContent('✅ Codex');
    expect(notice).toHaveTextContent('❌ OpenCode');
    fireEvent.click(screen.getByRole('button', { name: 'common.close' }));
    expect(onDismiss).toHaveBeenCalledOnce();
  });
});
