// KT-1591 — the bootstrap form silently ran the first audit-capable agent.
// It now offers the shared agent + tier picker and sends the user's choice.
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';
import { withDashboardRoutes } from '../../test/routerWrapper';
import type { AgentDetection, AgentType } from '../../types/generated';

const { detect, bootstrap } = vi.hoisted(() => ({
  detect: vi.fn(),
  bootstrap: vi.fn(),
}));

vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: vi.fn(() => ({ connected: false, connectionState: 'connecting' })),
}));
vi.mock('../DiscussionsPage', () => ({ DiscussionsPage: () => <div data-testid="discussion-page" /> }));
vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock({
    agents: { detect },
    projects: { bootstrap },
    modelCatalogApi: { list: vi.fn().mockResolvedValue({ targets: [] }) },
  });
});

import { Dashboard } from '../Dashboard';

const agent = (agent_type: AgentType, overrides: Partial<AgentDetection> = {}): AgentDetection => ({
  name: agent_type,
  agent_type,
  installed: true,
  enabled: true,
  runtime_available: true,
  path: `/usr/bin/${agent_type}`,
  version: '1.0.0',
  latest_version: null,
  ...overrides,
} as AgentDetection);

async function openBootstrap(agents: AgentDetection[]) {
  detect.mockResolvedValue(agents);
  await act(async () => {
    render(<I18nProvider>{withDashboardRoutes(<Dashboard onReset={vi.fn()} />)}</I18nProvider>);
  });
  await act(async () => {
    fireEvent.click(document.querySelector<HTMLElement>('[data-tour-id="new-project-btn"]')!);
  });
}

function fillForm() {
  fireEvent.change(screen.getByPlaceholderText('my-awesome-project'), { target: { value: 'News Site' } });
  fireEvent.change(document.querySelector('.dash-field-textarea')!, { target: { value: 'A news site' } });
}

const submitButton = () => document.querySelector<HTMLButtonElement>('.dash-submit-btn')!;

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  localStorage.clear();
});

describe('Dashboard — bootstrap agent picker', () => {
  it('defaults to the first bootstrap-capable agent and sends the chosen agent and tier', async () => {
    bootstrap.mockResolvedValue({ project_id: 'p1', discussion_id: 'd1' });
    // Vibe cannot bootstrap (API-only) and stays out of the list.
    await openBootstrap([agent('Vibe'), agent('Codex'), agent('ClaudeCode')]);

    const picker = await screen.findByTestId('bootstrap-agent-picker');
    const trigger = picker.querySelector<HTMLButtonElement>('.kr-agent-switch-btn')!;
    expect(trigger.textContent).toContain('Claude Code');

    await act(async () => { fireEvent.click(trigger); });
    expect(screen.queryByRole('group', { name: /Vibe/ })).toBeNull();
    const codexEconomy = document.querySelector<HTMLButtonElement>(
      '[role="group"][aria-label="Codex"] [data-tier="economy"]',
    )!;
    await act(async () => { fireEvent.click(codexEconomy); });
    await waitFor(() => expect(trigger.textContent).toContain('Codex'));

    fillForm();
    await act(async () => { fireEvent.click(submitButton()); });

    expect(bootstrap).toHaveBeenCalledTimes(1);
    expect(bootstrap.mock.calls[0][0]).toMatchObject({
      name: 'News Site',
      agent: 'Codex',
      tier: 'economy',
      skill_ids: ['bootstrap-architect'],
    });
  });

  it('keeps the Reasoning tier by default for the bootstrap discussion', async () => {
    bootstrap.mockResolvedValue({ project_id: 'p1', discussion_id: 'd1' });
    await openBootstrap([agent('ClaudeCode')]);
    await screen.findByTestId('bootstrap-agent-picker');

    fillForm();
    await act(async () => { fireEvent.click(submitButton()); });

    expect(bootstrap.mock.calls[0][0]).toMatchObject({ agent: 'ClaudeCode', tier: 'reasoning' });
  });

  it('disables the submit and explains why when no agent can bootstrap', async () => {
    await openBootstrap([agent('Vibe'), agent('Codex', { enabled: false })]);

    expect(await screen.findByTestId('bootstrap-no-agent')).toBeTruthy();
    expect(screen.queryByTestId('bootstrap-agent-picker')).toBeNull();
    fillForm();
    expect(submitButton().disabled).toBe(true);
    await act(async () => { fireEvent.click(submitButton()); });
    expect(bootstrap).not.toHaveBeenCalled();
  });
});
