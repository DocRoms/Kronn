// Regression guard for the 0.7.0 UX fix: the per-discussion config
// popover (Settings cog → Edit profiles/skills/directives) used to render
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

import { ChatHeader } from '../ChatHeader';
import type { Discussion, Skill, AgentProfile, Directive } from '../../types/generated';

const noop = () => {};
const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

function makeDiscussion(over: Partial<Discussion> = {}): Discussion {
  return {
    id: 'd-1',
    project_id: 'p-1',
    title: 'Test',
    agent: 'ClaudeCode' as any,
    language: 'en',
    participants: ['ClaudeCode' as any],
    messages: [],
    message_count: 0,
    archived: false,
    pinned: false,
    workspace_mode: 'Direct',
    workspace_path: null,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
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

function renderHeader(disc: Discussion = makeDiscussion()) {
  return render(
    <ChatHeader
      discussion={disc}
      projects={[]}
      agents={[]}
      availableSkills={skills}
      availableProfiles={profiles}
      availableDirectives={directives}
      mcpConfigs={[]}
      mcpIncompatibilities={[]}
      showGitPanel={false}
      isMobile={false}
      sending={false}
      pendingFilesCount={0}
      onRequestTestMode={noop}
      onToggleGitPanel={noop}
      onToggleSidebar={noop}
      onDelete={noop}
      onDiscussionUpdated={noop}
      onAgentSwitch={noop}
      contacts={[]}
      onShare={noop}
      toast={vi.fn()}
      t={t}
    />
  );
}

function openConfigPopover() {
  // The cog button has aria-label="disc.editConfig" via the i18n stub.
  const btn = screen.getByLabelText('disc.editConfig');
  fireEvent.click(btn);
}

describe('ChatHeader — config popover (collapsed accordion sections)', () => {
  it('renders all three section toggles but no chip walls by default', () => {
    renderHeader();
    openConfigPopover();
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
    renderHeader();
    openConfigPopover();
    fireEvent.click(screen.getByText('skills.selectSkills'));
    expect(screen.getByText('tdd')).toBeInTheDocument();
    expect(screen.getByText('systematic-debugging')).toBeInTheDocument();
    // Profiles + Directives still collapsed.
    expect(screen.queryByText(/Architect/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Caveman/)).not.toBeInTheDocument();
  });

  it('opening another section auto-collapses the previous one', () => {
    renderHeader();
    openConfigPopover();
    fireEvent.click(screen.getByText('skills.selectSkills'));
    expect(screen.getByText('tdd')).toBeInTheDocument();

    fireEvent.click(screen.getByText('profiles.select'));
    expect(screen.queryByText('tdd')).not.toBeInTheDocument();
    expect(screen.getByText(/Architect/)).toBeInTheDocument();
  });

  it('clicking the same toggle twice collapses the section back', () => {
    renderHeader();
    openConfigPopover();
    fireEvent.click(screen.getByText('directives.title'));
    expect(screen.getByText(/Caveman/)).toBeInTheDocument();
    fireEvent.click(screen.getByText('directives.title'));
    expect(screen.queryByText(/Caveman/)).not.toBeInTheDocument();
  });

  it('shows the active count next to a toggle when items are selected', () => {
    const disc = makeDiscussion({ skill_ids: ['s1', 's2'] } as any);
    renderHeader(disc);
    openConfigPopover();
    // The count badge sits inside the toggle: look for "2" near "skills.selectSkills".
    const skillsToggle = screen.getByText('skills.selectSkills').closest('button');
    expect(skillsToggle).not.toBeNull();
    expect(skillsToggle!.textContent).toContain('2');
  });
});
