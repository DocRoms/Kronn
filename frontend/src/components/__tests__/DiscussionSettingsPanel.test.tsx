// Regression guard for the discussion Settings side panel. The editor used
// to render
// every section's chip wall always-open, so workspaces with many
// configured items overflowed the viewport and clipped the trailing
// sections.
//
// Now each list (Profiles, Skills, Directives) is collapsed behind its
// own toggle, mirroring NewDiscussionForm. Only ONE expanded at a time.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

import { DiscussionSettingsPanel } from '../DiscussionSettingsPanel';
import type { Discussion, Skill, AgentProfile, Directive } from '../../types/generated';

const noop = () => {};

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
});
