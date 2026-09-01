// Regression guard for the discussion Settings side panel. The editor used
// to render
// every section's chip wall always-open, so workspaces with many
// configured items overflowed the viewport and clipped the trailing
// sections.
//
// Now each list (Profiles, Skills, Directives) is collapsed behind its
// own toggle, mirroring NewDiscussionForm. Only ONE expanded at a time.

import { beforeEach, describe, it, expect, vi } from 'vitest';
import { act, render, screen, fireEvent, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

import { DiscussionSettingsPanel } from '../DiscussionSettingsPanel';
import { discussions as discussionsApi } from '../../lib/api';
import type { Discussion, Skill, AgentProfile, Directive } from '../../types/generated';

const noop = () => {};

beforeEach(() => {
  vi.mocked(discussionsApi.agentHandoffMode).mockReset();
  vi.mocked(discussionsApi.agentHandoffMode).mockImplementation(() => new Promise(() => {}));
  vi.mocked(discussionsApi.update).mockClear();
  vi.mocked(discussionsApi.executionVariableRetention).mockReset();
  vi.mocked(discussionsApi.executionVariableRetention).mockResolvedValue({
    global_days: 30,
    override_days: null,
    effective_days: 30,
  });
});

function makeDiscussion(over: Partial<Discussion> = {}): Discussion {
  return {
    id: 'd-1',
    project_id: 'p-1',
    title: 'Test',
    agent: 'ClaudeCode' as any,
    language: 'en',
    participants: ['ClaudeCode' as any],
    messages: [],
    message_count: 0, non_system_message_count: 0, tier: "default" as const, summary_strategy: "Auto" as const, introspection_call_count: 0,
    archived: false,
    pinned: false, pin_first_message: false,
    workspace_mode: 'Direct',
    workspace_path: null,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    awaiting_agent: false,
    ...over,
  };
}

const skills: Skill[] = [
  { id: 's1', name: 'tdd', description: '', built_in: false, installed: true },
  { id: 's2', name: 'systematic-debugging', description: '', built_in: false, installed: true },
] as any;
const profiles: AgentProfile[] = [
  { id: 'p1', name: 'Architect', persona_name: 'Architect', avatar: '🏗️', color: null, description: '', built_in: false } as any,
];
const directives: Directive[] = [
  { id: 'd1', name: 'Caveman', icon: '🪨', description: '', built_in: false, enabled: true } as any,
];

function renderPanel(
  disc: Discussion = makeDiscussion(),
  options: { contacts?: any[]; onShare?: (ids: string[]) => void } = {},
) {
  return render(
    <DiscussionSettingsPanel
      discussion={disc}
      projects={[]}
      availableSkills={skills}
      availableProfiles={profiles}
      availableDirectives={directives}
      mcpConfigs={[]}
      mcpIncompatibilities={[]}
      contacts={options.contacts ?? []}
      onClose={noop}
      onDiscussionUpdated={noop}
      onShare={options.onShare ?? noop}
      toast={vi.fn()}
    />
  );
}

describe('DiscussionSettingsPanel — collapsed context sections', () => {
  it('uses the shared themed utility-panel shell', () => {
    const { container } = renderPanel();
    expect(container.querySelector('.disc-tool-panel.disc-settings-panel')).toBeInTheDocument();
    expect(screen.getByText('disc.settingsPanel')).toBeInTheDocument();
    expect(screen.getByText('Claude Code')).toBeInTheDocument();
    expect(
      screen.getByText('disc.agentHandoffTitle').closest('.disc-settings-overview'),
    ).not.toBeNull();
  });

  it('renders all three section toggles but no chip walls by default', () => {
    renderPanel();
    // Toggles visible.
    expect(screen.getByText('profiles.select')).toBeInTheDocument();
    expect(screen.getByText('skills.selectSkills')).toBeInTheDocument();
    expect(screen.getByText('directives.title')).toBeInTheDocument();
    // Chip walls collapsed: no chip text rendered yet.
    expect(screen.queryByText('tdd')).not.toBeInTheDocument();
    expect(screen.queryByText(/Architect/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Caveman/)).not.toBeInTheDocument();
  });

  it('expanding the Skills section reveals only its chips, not the others', () => {
    renderPanel();
    fireEvent.click(screen.getByText('skills.selectSkills'));
    expect(screen.getByText('tdd')).toBeInTheDocument();
    expect(screen.getByText('systematic-debugging')).toBeInTheDocument();
    // Profiles + Directives still collapsed.
    expect(screen.queryByText(/Architect/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Caveman/)).not.toBeInTheDocument();
  });

  it('opening another section auto-collapses the previous one', () => {
    renderPanel();
    fireEvent.click(screen.getByText('skills.selectSkills'));
    expect(screen.getByText('tdd')).toBeInTheDocument();

    fireEvent.click(screen.getByText('profiles.select'));
    expect(screen.queryByText('tdd')).not.toBeInTheDocument();
    expect(screen.getByText(/Architect/)).toBeInTheDocument();
  });

  it('clicking the same toggle twice collapses the section back', () => {
    renderPanel();
    fireEvent.click(screen.getByText('directives.title'));
    expect(screen.getByText(/Caveman/)).toBeInTheDocument();
    fireEvent.click(screen.getByText('directives.title'));
    expect(screen.queryByText(/Caveman/)).not.toBeInTheDocument();
  });

  it('shows the active count next to a toggle when items are selected', () => {
    const disc = makeDiscussion({ skill_ids: ['s1', 's2'] } as any);
    renderPanel(disc);
    // The count badge sits inside the toggle: look for "2" near "skills.selectSkills".
    const skillsToggle = screen.getByText('skills.selectSkills').closest('button');
    expect(skillsToggle).not.toBeNull();
    expect(skillsToggle!.textContent).toContain('2');
  });

  it('shares from the settings panel without rendering a header popover', () => {
    const onShare = vi.fn();
    renderPanel(makeDiscussion(), {
      contacts: [{ id: 'contact-1', pseudo: 'Alice' }],
      onShare,
    });

    fireEvent.click(screen.getByRole('button', { name: /Alice/ }));
    expect(onShare).toHaveBeenCalledWith(['contact-1']);
  });

  it('shows the conversation kill switch disabled when the global opt-in is off', async () => {
    vi.mocked(discussionsApi.agentHandoffMode).mockResolvedValueOnce({
      global_enabled: false,
      disabled: false,
      unlimited_override: false,
      effective_enabled: false,
      paid_limit: 1,
    });
    renderPanel();
    const defaultMode = await screen.findByRole('radio', { name: 'disc.agentHandoffMode.default' });
    expect(defaultMode).toBeDisabled();
    expect(screen.getByText('disc.agentHandoffGlobalOff')).toBeInTheDocument();
    expect(screen.getByText('disc.agentHandoffCliUnaffected')).toBeInTheDocument();
  });

  it('can keep agents separate for only this discussion', async () => {
    vi.mocked(discussionsApi.agentHandoffMode)
      .mockResolvedValueOnce({
      global_enabled: true,
      disabled: false,
      unlimited_override: false,
      effective_enabled: true,
      paid_limit: 1,
      })
      .mockResolvedValueOnce({
        global_enabled: true,
        disabled: true,
        unlimited_override: false,
        effective_enabled: false,
        paid_limit: 1,
      });
    const update = vi.mocked(discussionsApi.update);
    update.mockResolvedValueOnce(undefined);
    renderPanel();

    const blocked = await screen.findByRole('radio', { name: 'disc.agentHandoffMode.disabled' });
    expect(blocked).not.toBeDisabled();
    expect(screen.getByText('disc.agentHandoffChainLimited')).toBeInTheDocument();
    fireEvent.click(blocked);

    expect(update).toHaveBeenCalledWith('d-1', {
      agent_handoffs_disabled: true,
      agent_handoffs_unlimited: false,
    });
    expect(await screen.findByText('disc.agentHandoffDiscussionOff')).toBeInTheDocument();
  });

  it('can remove the financial limit for one discussion with a warning', async () => {
    vi.mocked(discussionsApi.agentHandoffMode)
      .mockResolvedValueOnce({
        global_enabled: true,
        disabled: false,
        unlimited_override: false,
        effective_enabled: true,
        paid_limit: 2,
      })
      .mockResolvedValueOnce({
        global_enabled: true,
        disabled: false,
        unlimited_override: true,
        effective_enabled: true,
        paid_limit: null,
      });
    vi.mocked(discussionsApi.update).mockResolvedValueOnce(undefined);
    renderPanel();

    fireEvent.click(await screen.findByRole('radio', { name: 'disc.agentHandoffMode.unlimited' }));

    expect(discussionsApi.update).toHaveBeenCalledWith('d-1', {
      agent_handoffs_disabled: false,
      agent_handoffs_unlimited: true,
    });
    expect(await screen.findByRole('alert')).toHaveTextContent('disc.agentHandoffUnlimitedWarning');
    expect(screen.getByText('disc.agentHandoffChainUnlimited')).toBeInTheDocument();
    expect(screen.getByText('disc.agentHandoffCliUnaffected')).toBeInTheDocument();
  });

  it('can override execution-value retention and restore the global policy', async () => {
    vi.mocked(discussionsApi.update).mockResolvedValue(undefined);
    renderPanel();

    const select = await screen.findByLabelText('disc.executionVariableRetention') as HTMLSelectElement;
    expect(select.value).toBe('inherit');

    await act(async () => { fireEvent.change(select, { target: { value: '7' } }); });
    await waitFor(() => expect(discussionsApi.update).toHaveBeenLastCalledWith('d-1', {
      execution_variable_retention_days: 7,
    }));

    await act(async () => { fireEvent.change(select, { target: { value: 'inherit' } }); });
    await waitFor(() => expect(discussionsApi.update).toHaveBeenLastCalledWith('d-1', {
      execution_variable_retention_days: null,
    }));
  });
});
