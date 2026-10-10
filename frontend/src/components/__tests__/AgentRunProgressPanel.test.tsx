import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { AgentRunProgressPanel } from '../AgentRunProgressPanel';
import { applyRunFrame, type AgentRunProgressFrame, type LiveRun } from '../../lib/agentRunProgress';
import { secondTickerRunning } from '../../hooks/useSecondTicker';

const liveRun = (progress: Partial<AgentRunProgressFrame['progress']>, now = Date.now()): LiveRun =>
  applyRunFrame({}, {
    type: 'agent_run_progress',
    discussion_id: 'd1',
    dispatch_id: 'job-1',
    trigger_message_id: 'u1',
    agent_type: 'OpenCode',
    run_id: 'run-1',
    started_at: '2026-10-09T09:00:00Z',
    seq: 1,
    progress: {
      phase: 'opening_session',
      phase_ms: 2_000,
      elapsed_ms: 3_000,
      timeline: [
        { phase: 'preparing', at_ms: 0 },
        { phase: 'launching', at_ms: 200 },
        { phase: 'initializing', at_ms: 600 },
        { phase: 'opening_session', at_ms: 1_000 },
      ],
      mcp_servers: 3,
      activity: [],
      tool_calls: 0,
      silent_ms: 0,
      idle_limit_ms: 90_000,
      stopped: null,
      ...progress,
    },
  }, now).d1['run-1'];

const renderPanel = (run: LiveRun) =>
  render(<I18nProvider><AgentRunProgressPanel run={run} agentLabel="OpenCode" /></I18nProvider>);

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe('AgentRunProgressPanel', () => {
  it('shows the startup steps and the time spent before the first token', () => {
    vi.useFakeTimers();
    renderPanel(liveRun({}));
    expect(screen.getByTestId('run-phase')).toHaveTextContent(
      'Ouverture de la session et démarrage des serveurs MCP du projet · 3 serveur(s) MCP',
    );
    expect(screen.getByTestId('run-phase')).toHaveAttribute('aria-live', 'polite');
    const steps = screen.getAllByTestId('run-step');
    expect(steps.map(step => step.textContent)).toEqual([
      'Préparation du lancement0.2 s',
      'Lancement de OpenCode0.4 s',
      'Initialisation de la session ACP0.4 s',
      'Ouverture de la session et démarrage des serveurs MCP du projet',
    ]);
    expect(screen.getByTestId('run-phase-elapsed')).toHaveTextContent('2 s');
    // The shared clock moves the time on, not a new phase.
    act(() => { vi.advanceTimersByTime(3_000); });
    expect(screen.getByTestId('run-phase-elapsed')).toHaveTextContent('5 s');
    expect(screen.getByTestId('run-elapsed')).toHaveTextContent('total 6 s');
  });

  it('counts the inactivity delay down only once the agent is silent', () => {
    vi.useFakeTimers();
    renderPanel(liveRun({ phase: 'waiting_model', silent_ms: 1_000, idle_limit_ms: 60_000 }));
    expect(screen.queryByTestId('run-idle-countdown')).toBeNull();
    act(() => { vi.advanceTimersByTime(5_000); });
    expect(screen.getByTestId('run-idle-countdown')).toHaveTextContent(
      'Silencieux depuis 6 s · arrêt pour inactivité dans 54 s',
    );
  });

  it('announces the stop in the bubble and stops its clock', () => {
    vi.useFakeTimers();
    renderPanel(liveRun({ phase: 'waiting_model', stopped: 'idle', silent_ms: 300_000, idle_limit_ms: null }));
    const stop = screen.getByTestId('run-stopped');
    expect(stop).toHaveAttribute('role', 'alert');
    expect(stop).toHaveTextContent('Kronn a arrêté OpenCode après 5 min 00 s sans activité.');
    expect(screen.queryByTestId('run-idle-countdown')).toBeNull();
    expect(secondTickerRunning()).toBe(false);
    const before = screen.getByTestId('run-elapsed').textContent;
    act(() => { vi.advanceTimersByTime(10_000); });
    expect(screen.getByTestId('run-elapsed').textContent).toBe(before);
  });

  it('lists the tools by category only, behind a toggle', () => {
    renderPanel(liveRun({
      phase: 'tool',
      tool_calls: 2,
      activity: [
        { category: 'Mcp', at: new Date().toISOString() },
        { category: 'Execute', at: new Date().toISOString() },
      ],
    }));
    const toggle = screen.getByTestId('run-activity-toggle');
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getAllByTestId('run-activity-entry').map(entry => entry.querySelector('span')?.textContent))
      .toEqual(['Outil MCP', 'Commande']);
  });
});
