// KT-1657 — "Resume now" reachable without opening the run, and the agent
// of the refused step switched from the paused run.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../../lib/I18nContext';
import { QuotaAgentSwitch, QuotaPauseNotice, QuotaResumeButton } from '../QuotaPauseControls';

const { resumeRunMock, switchMock, detectMock } = vi.hoisted(() => ({
  resumeRunMock: vi.fn(),
  switchMock: vi.fn(),
  detectMock: vi.fn(),
}));
vi.mock('../../../lib/api', async () => {
  const { buildApiMock } = await import('../../../test/apiMock');
  return buildApiMock({
    workflows: { resumeRun: resumeRunMock as never, switchQuotaAgent: switchMock as never },
    agents: { detect: detectMock as never },
  });
});

const detection = (agent_type: string, over: Record<string, unknown> = {}) => ({
  name: agent_type, agent_type, installed: true, enabled: true, runtime_available: true, path: null, ...over,
});

const wrap = async (ui: React.ReactElement) => {
  await act(async () => { render(<I18nProvider>{ui}</I18nProvider>); });
  await act(async () => { await new Promise(r => setTimeout(r, 0)); });
};

beforeEach(() => {
  resumeRunMock.mockReset().mockResolvedValue({ run_id: 'run-1', new_status: 'Running' });
  switchMock.mockReset().mockResolvedValue({ run_id: 'run-1', step_name: 'Agent', agent: 'Codex' });
  detectMock.mockReset().mockResolvedValue([
    detection('ClaudeCode'),
    detection('Codex'),
    detection('Vibe', { installed: false, runtime_available: false }),
    detection('Kiro', { enabled: false }),
    detection('Custom'),
  ]);
});

afterEach(() => {
  cleanup();
});

describe('QuotaPauseNotice', () => {
  it('labels the run paused with its wake time and resumes it from the card, without opening the run', async () => {
    const onResumed = vi.fn();
    const wake = '2026-11-11T02:02:00Z';
    await wrap(<QuotaPauseNotice quota={{ wake_at: wake }} resumeRunId="parent-9" onResumed={onResumed} />);

    const notice = screen.getByTestId('quota-pause-notice');
    expect(notice).toHaveTextContent(`En pause — quota du fournisseur, reprise automatique à ${new Date(wake).toLocaleString()}`);
    expect(notice.dataset.parked).toBeUndefined();

    await act(async () => { fireEvent.click(screen.getByTestId('quota-resume-now')); });
    expect(resumeRunMock).toHaveBeenCalledWith('parent-9');
    expect(onResumed).toHaveBeenCalledTimes(1);
  });

  it('says a parked run waits for a person', async () => {
    await wrap(<QuotaPauseNotice quota={{ wake_at: null }} resumeRunId="run-1" />);
    const notice = screen.getByTestId('quota-pause-notice');
    expect(notice).toHaveTextContent('En pause — quota du fournisseur, en attente de votre reprise');
    expect(notice.dataset.parked).toBe('true');
  });
});

describe('QuotaResumeButton', () => {
  it('shows why a resume was refused, and can be tried again', async () => {
    resumeRunMock.mockRejectedValueOnce(new Error('Concurrency limit reached (1/1).'));
    await wrap(<QuotaResumeButton runId="run-1" />);
    await act(async () => { fireEvent.click(screen.getByTestId('quota-resume-now')); });
    expect(screen.getByRole('alert')).toHaveTextContent('Concurrency limit reached (1/1).');
    expect(screen.getByTestId('quota-resume-now')).not.toBeDisabled();

    await act(async () => { fireEvent.click(screen.getByTestId('quota-resume-now')); });
    expect(resumeRunMock).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('ignores a second click while a resume is in flight', async () => {
    let finish: (value: unknown) => void = () => {};
    resumeRunMock.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    await wrap(<QuotaResumeButton runId="run-1" />);
    await act(async () => { fireEvent.click(screen.getByTestId('quota-resume-now')); });
    expect(screen.getByTestId('quota-resume-now')).toBeDisabled();
    await act(async () => { fireEvent.click(screen.getByTestId('quota-resume-now')); });
    expect(resumeRunMock).toHaveBeenCalledTimes(1);
    await act(async () => { finish({}); });
  });
});

describe('QuotaAgentSwitch', () => {
  it('offers the installed, enabled agents other than the current one, and switches the refused step', async () => {
    const onSwitched = vi.fn();
    await wrap(<QuotaAgentSwitch runId="run-1" current="ClaudeCode" onSwitched={onSwitched} />);

    const select = screen.getByRole('combobox', { name: 'Changer d\'agent' });
    const offered = Array.from(select.querySelectorAll('option')).map(o => o.value).filter(Boolean);
    // Not the current agent, not an uninstalled or disabled one, not a named HTTP connection.
    expect(offered).toEqual(['Codex']);

    const apply = screen.getByTestId('quota-switch-apply');
    expect(apply).toBeDisabled();
    fireEvent.change(select, { target: { value: 'Codex' } });
    await act(async () => { fireEvent.click(apply); });

    expect(switchMock).toHaveBeenCalledWith('run-1', { agent: 'Codex' });
    expect(onSwitched).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('status')).toHaveTextContent('L\'étape s\'exécutera sur Codex');
  });

  it('shows the refusal when the switch is refused', async () => {
    switchMock.mockRejectedValueOnce(new Error('agent(s) not installed or not enabled'));
    await wrap(<QuotaAgentSwitch runId="run-1" current="ClaudeCode" />);
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'Codex' } });
    await act(async () => { fireEvent.click(screen.getByTestId('quota-switch-apply')); });
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('not installed or not enabled'));
  });

  it('renders nothing when no other agent is available', async () => {
    detectMock.mockResolvedValue([detection('ClaudeCode')]);
    await wrap(<QuotaAgentSwitch runId="run-1" current="ClaudeCode" />);
    expect(screen.queryByTestId('quota-agent-switch')).toBeNull();
  });
});
