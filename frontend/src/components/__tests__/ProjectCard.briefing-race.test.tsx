// ProjectCard — regression: rapid double / triple click on "Start Briefing"
// must NOT spawn two briefings on the same project.
//
// Pre-fix the guard read `briefingStarting` from the closure, which is
// stale between two synchronous click events that fire before React has
// re-rendered the disabled prop. The companion ref is the only race-free
// fix: the second click reads the just-written ref and bails out before
// the second `projects.startBriefing` POST is dispatched.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, act, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
  }),
}));
vi.mock('../../hooks/useMediaQuery', () => ({ useIsMobile: () => false }));

import { ProjectCard } from '../ProjectCard';
import { projects as projectsApi } from '../../lib/api';
import type { Project, AgentDetection } from '../../types/generated';

const noop = () => {};

const PROJECT: Project = {
  id: 'p-noteam',
  name: 'noteam',
  path: '/repos/noteam',
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'NoTemplate',
  ai_todo_count: 0,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

const AGENT: AgentDetection = {
  name: 'Claude Code',
  agent_type: 'ClaudeCode',
  installed: true,
  enabled: true,
  path: '/usr/bin/claude',
  version: '1.0.0',
  latest_version: null,
  origin: 'host',
  install_command: null,
  host_managed: false,
  host_label: null,
  runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

function renderCard() {
  return render(
    <ProjectCard
      project={PROJECT}
      isOpen={true}
      onToggleOpen={noop}
      discussions={[]}
      driftStatus={undefined}
      agents={[AGENT]}
      allSkills={[]}
      mcpConfigs={[]}
      workflows={[]}
      configLanguage="fr"
      toast={vi.fn()}
      onNavigate={noop}
      onSetDiscPrefill={noop}
      onAutoRunDiscussion={noop}
      onOpenDiscussion={noop}
      onRefetch={noop}
      onRefetchDiscussions={noop}
      onRefetchSkills={noop}
      onRefetchDrift={noop}
    />
  );
}

describe('ProjectCard — briefing double / triple click race', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // Make startBriefing hold its promise so the test can fire a second
    // click while the first is "in flight".
    let _resolve: (v: { discussion_id: string }) => void = () => {};
    (projectsApi.startBriefing as ReturnType<typeof vi.fn>).mockImplementation(
      () => new Promise<{ discussion_id: string }>(r => { _resolve = r; })
    );
    // Stash the resolver on a global scratch so tests can release it after
    // asserting the call count (avoids unhandled-rejection noise).
    (globalThis as unknown as { _briefingResolve: typeof _resolve })._briefingResolve = _resolve;
  });

  it('a single click fires startBriefing exactly once', async () => {
    renderCard();
    const btn = screen.getByText('audit.startBriefing').closest('button')!;
    expect(btn).not.toBeNull();

    await act(async () => { fireEvent.click(btn); });

    await waitFor(() => {
      expect(projectsApi.startBriefing).toHaveBeenCalledTimes(1);
    });
  });

  it('two synchronous clicks fire startBriefing exactly once (race-free guard)', async () => {
    renderCard();
    const btn = screen.getByText('audit.startBriefing').closest('button')!;

    await act(async () => {
      fireEvent.click(btn);
      fireEvent.click(btn);
    });

    expect(projectsApi.startBriefing).toHaveBeenCalledTimes(1);
  });

  it('three synchronous clicks fire startBriefing exactly once', async () => {
    renderCard();
    const btn = screen.getByText('audit.startBriefing').closest('button')!;

    await act(async () => {
      fireEvent.click(btn);
      fireEvent.click(btn);
      fireEvent.click(btn);
    });

    expect(projectsApi.startBriefing).toHaveBeenCalledTimes(1);
  });
});
