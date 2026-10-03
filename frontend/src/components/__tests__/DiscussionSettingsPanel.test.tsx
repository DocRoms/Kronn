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
import { discussions as discussionsApi, projects as projectsApi, skills as skillsApi } from '../../lib/api';
import type { Discussion, Skill, AgentProfile, Directive, Project } from '../../types/generated';

const noop = () => {};

beforeEach(() => {
  // The skill picker reads its lists when it opens (KT-923).
  vi.mocked(skillsApi.list).mockReset();
  vi.mocked(skillsApi.list).mockImplementation(() => Promise.resolve(skills));
  vi.mocked(projectsApi.usedSkills).mockReset();
  vi.mocked(projectsApi.usedSkills).mockResolvedValue([]);
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
    message_count: 0, non_system_message_count: 0, tier: "default" as const, summary_strategy: "OnDemand" as const, introspection_call_count: 0,
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

// KT-923 — the skills of a discussion: the project's own first (the native
// skills of its repository included), then the ticked ones, then the catalog
// by category behind a fold, all searchable.
describe('DiscussionSettingsPanel — skill picker', () => {
  const catalog = [
    { id: 'rust', name: 'Rust', description: 'Idiomatic Rust', icon: '🦀', category: 'Language', content: '', is_builtin: true, token_estimate: 0 },
    { id: 'security', name: 'Security', description: 'Threat modelling', icon: '🔒', category: 'Domain', content: '', is_builtin: true, token_estimate: 0 },
    { id: 'seo', name: 'SEO', description: '', icon: '📈', category: 'Business', content: '', is_builtin: true, token_estimate: 0 },
  ] as Skill[];
  const project = {
    id: 'p-1', name: 'front_euronews', path: '/repos/front_euronews', default_skill_ids: [],
  } as unknown as Project;
  const blockMigration = {
    project_id: 'p-1', slug: 'block-migration', name: 'Block migration',
    root: '.agents/skills', relative_path: '.agents/skills/block-migration/SKILL.md',
    referenced: true, published: false,
  };

  const openSkills = async (disc: Discussion, projects: Project[]) => {
    vi.mocked(skillsApi.list).mockResolvedValue(catalog);
    render(
      <DiscussionSettingsPanel
        discussion={disc}
        projects={projects}
        availableSkills={catalog}
        availableProfiles={profiles}
        availableDirectives={directives}
        mcpConfigs={[]}
        mcpIncompatibilities={[]}
        contacts={[]}
        onClose={noop}
        onDiscussionUpdated={noop}
        onShare={noop}
        toast={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByText('skills.selectSkills'));
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
  };

  it('puts the project\'s skills first — block-migration with its origin — and folds the catalog', async () => {
    vi.mocked(projectsApi.usedSkills).mockResolvedValue([blockMigration]);
    await openSkills(makeDiscussion({ skill_ids: ['security'] }), [project]);

    const used = document.querySelector('[data-section="used"]') as HTMLElement;
    expect(used).toHaveTextContent('disc.skillsUsedByProject');
    expect(used).toHaveTextContent('Block migration');
    // (This file's `t` drops the arguments: the origin names its folder in the picker's own test.)
    expect(used).toHaveTextContent('automation.skill.originRepository');
    // Ticked skills come next, the catalog last and folded.
    expect(document.querySelector('[data-section="ticked"]')).toHaveTextContent('Security');
    expect(screen.queryByRole('button', { name: /Rust/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'automation.skill.availableToggle' }));
    expect(screen.getByRole('button', { name: /Rust/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /SEO/ })).toBeInTheDocument();
  });

  it('a search opens what matches without unfolding by hand', async () => {
    vi.mocked(projectsApi.usedSkills).mockResolvedValue([blockMigration]);
    await openSkills(makeDiscussion(), [project]);

    fireEvent.change(screen.getByRole('searchbox', { name: 'disc.skillSearch' }), { target: { value: 'threat' } });
    expect(screen.getByRole('button', { name: /Security/ })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Block migration/ })).not.toBeInTheDocument();
  });

  it('ticking the repository skill saves its stable id with the discussion', async () => {
    vi.mocked(projectsApi.usedSkills).mockResolvedValue([blockMigration]);
    vi.mocked(discussionsApi.update).mockResolvedValue(undefined);
    await openSkills(makeDiscussion({ skill_ids: ['rust'] }), [project]);

    fireEvent.click(screen.getByRole('button', { name: /Block migration/ }));
    expect(discussionsApi.update).toHaveBeenCalledWith('d-1', {
      skill_ids: ['rust', 'repository:p-1:block-migration'],
    });
  });

  it('unticking removes only that id and leaves the others as they were', async () => {
    vi.mocked(projectsApi.usedSkills).mockResolvedValue([blockMigration]);
    vi.mocked(discussionsApi.update).mockResolvedValue(undefined);
    await openSkills(makeDiscussion({ skill_ids: ['rust', 'repository:p-1:block-migration'] }), [project]);

    fireEvent.click(screen.getByRole('button', { name: /Block migration/ }));
    expect(discussionsApi.update).toHaveBeenCalledWith('d-1', { skill_ids: ['rust'] });
  });

  it('shows the catalog by category, unfolded, for a discussion with no project', async () => {
    await openSkills(makeDiscussion({ project_id: null }), [project]);

    expect(screen.queryByText('disc.skillsUsedByProject')).not.toBeInTheDocument();
    const categories = [...document.querySelectorAll('.skill-picker-group')].map(group => group.getAttribute('data-category'));
    expect(categories).toEqual(['Language', 'Domain', 'Business']);
  });
});
