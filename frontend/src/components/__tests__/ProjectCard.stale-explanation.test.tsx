// KT-1218 — the stale-section explanation is one shared panel, reachable from
// the compact header, the audit timeline and the viewer of a stale doc.
//
// Each entry point is exercised through the real ProjectCard (not a callback
// spy): the click must land on the same « Vérifier la doc » dialog, which
// names every stale doc with its audit step and changed sources, says why it
// is stale and what « Mettre à jour » / « Vérifié, inchangé » do.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
    locale: 'fr',
  }),
}));
vi.mock('../../hooks/useMediaQuery', () => ({ useIsMobile: () => false }));

import { ProjectCard } from '../ProjectCard';
import { projects as projectsApi } from '../../lib/api';
import fr from '../../lib/i18n/locales/fr';
import en from '../../lib/i18n/locales/en';
import es from '../../lib/i18n/locales/es';
import zh from '../../lib/i18n/locales/zh';
import type { Project, AgentDetection, DriftCheckResponse } from '../../types/generated';
import type { ComponentProps } from 'react';

const noop = () => {};

const PROJECT: Project = {
  id: 'p-stale', name: 'stale-target', path: '/repos/stale-target',
  repo_url: null, token_override: null, ai_config: { detected: false, configs: [] },
  audit_status: 'Audited', briefing_notes: '# Project Briefing',
  ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false, path_exists: true,
  created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
};

const AGENT: AgentDetection = {
  name: 'Claude Code', agent_type: 'ClaudeCode', installed: true, enabled: true,
  path: '/usr/bin/claude', version: '1.0.0', latest_version: null, origin: 'host',
  install_command: null, host_managed: false, host_label: null,
  runtime_available: false, rtk_available: false, rtk_hook_configured: false,
};

const DRIFT: DriftCheckResponse = {
  audit_date: '2026-10-12',
  stale_sections: [
    { ai_file: 'docs/AGENTS.md', audit_step: 1, changed_sources: ['src/lib.rs', 'Cargo.toml'], missing_paths: [] },
    { ai_file: 'docs/repo-map.md', audit_step: 3, changed_sources: ['src/main.rs'], missing_paths: [] },
  ],
  fresh_sections: [], total_sections: 2, sources_state: 'needs_reverification',
  broken_citations: 0, placeholder_docs: [],
};

function card(location: ComponentProps<typeof ProjectCard>['location']) {
  return (
    <ProjectCard
      project={PROJECT}
      detailMode
      isOpen
      onToggleOpen={noop}
      discussions={[]}
      driftStatus={DRIFT}
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
      location={location}
    />
  );
}

/** The one explanation every entry point must land on. */
async function expectStaleExplanation() {
  const panel = await screen.findByRole('dialog');
  expect(panel).toHaveAttribute('data-testid', 'revalidate-panel');
  expect(panel).toHaveAttribute('aria-modal', 'true');
  const details = within(panel).getByTestId('revalidate-stale-details');
  // Why a section is stale.
  expect(details).toHaveTextContent('docState.stale.explanation');
  // Which doc, which audit step, which changed sources.
  const items = within(details).getAllByRole('listitem').filter(item => item.querySelector(':scope > strong'));
  expect(items).toHaveLength(2);
  expect(items[0]).toHaveTextContent('docs/AGENTS.md');
  expect(items[0]).toHaveTextContent('docState.stale.step 1');
  expect(items[0]).toHaveTextContent('src/lib.rs');
  expect(items[0]).toHaveTextContent('Cargo.toml');
  expect(items[1]).toHaveTextContent('docs/repo-map.md');
  expect(items[1]).toHaveTextContent('docState.stale.step 3');
  expect(items[1]).toHaveTextContent('src/main.rs');
  // What the two actions do.
  expect(details).toHaveTextContent('docState.stale.updateEffect');
  expect(details).toHaveTextContent('docState.stale.unchangedEffect');
  expect(within(details).getByTestId('revalidate-stale-update')).toBeInTheDocument();
  expect(within(details).getByTestId('revalidate-stale-ack')).toBeInTheDocument();
  return panel;
}

beforeEach(() => {
  localStorage.clear();
  sessionStorage.clear();
  vi.clearAllMocks();
  vi.mocked(projectsApi.auditStatus).mockResolvedValue(null);
  vi.mocked(projectsApi.auditResumable).mockResolvedValue(null);
  vi.mocked(projectsApi.auditTimeline).mockResolvedValue({
    runs: [{
      id: 'run-1', project_id: 'p-stale', kind: 'Full', agent_type: 'ClaudeCode',
      started_at: '2026-10-12T10:00:00Z', status: 'Completed', td_total: 0,
    }],
    steps: [], recorded_audits: [], recorded_validated_at: null,
    latest_validation: null, lease_orphaned: false,
  } as never);
  vi.mocked(projectsApi.docProvenance).mockResolvedValue({ docs: [], to_reconfirm: 0 });
  vi.mocked(projectsApi.revalidationPlan).mockResolvedValue({
    sources_state: 'needs_reverification', sections: [], agent_steps: [], reanchor_count: 0, human_count: 0, unanchored: [], unanchored_docs: [], relevance: { sections: 0, anchors_intact: 0, outdated: 0, no_longer_relevant: 0, unanchored: 0, untouched_stale: 0, missing_references: 0, prose_missing: 0, anchored_intact_pct: null },
  });
  vi.mocked(projectsApi.listAiFiles).mockResolvedValue([
    {
      path: 'docs', name: 'docs', is_dir: true, children: [
        { path: 'docs/AGENTS.md', name: 'AGENTS.md', is_dir: false, children: [] },
        { path: 'docs/glossary.md', name: 'glossary.md', is_dir: false, children: [] },
      ],
    },
  ]);
  vi.mocked(projectsApi.readAiFile).mockResolvedValue({ path: 'docs/AGENTS.md', content: '# Agents' });
});

describe('KT-1218 — the stale-section explanation is reachable everywhere', () => {
  it('opens from the compact header summary', async () => {
    render(card({ view: 'overview' }));
    const header = document.querySelector('.project-detail-actions') as HTMLElement;
    const summary = await within(header).findByTestId('project-doc-state-summary');
    await waitFor(() => expect(summary).toHaveTextContent('docState.summaryAttention 2'));
    expect(screen.queryByTestId('revalidate-panel')).toBeNull();

    fireEvent.click(summary);
    await expectStaleExplanation();
  });

  it('opens from the compact header primary action', async () => {
    render(card({ view: 'overview' }));
    const header = document.querySelector('.project-detail-actions') as HTMLElement;
    fireEvent.click(await within(header).findByTestId('project-revalidate'));
    await expectStaleExplanation();
  });

  it('opens from the audit timeline, both the doc-state block and the launch entry', async () => {
    render(card({ view: 'audit' }));
    const block = await screen.findByTestId('audit-timeline-doc-state');
    fireEvent.click(within(block).getByRole('button', { name: 'audit.revalidate.button' }));
    const panel = await expectStaleExplanation();

    fireEvent.click(within(panel).getByRole('button', { name: 'common.close' }));
    await waitFor(() => expect(screen.queryByTestId('revalidate-panel')).toBeNull());

    const launch = await screen.findByTestId('audit-timeline-launch');
    await waitFor(() => expect(launch).toBeEnabled());
    fireEvent.click(launch);
    await expectStaleExplanation();
  });

  it('opens from the banner of a stale doc in the doc viewer, and only for that doc', async () => {
    render(card({ view: 'docs' }));
    // glossary.md is not stale: no banner.
    fireEvent.click(await screen.findByText('glossary.md'));
    await waitFor(() => expect(projectsApi.readAiFile).toHaveBeenCalledWith('p-stale', 'docs/glossary.md'));
    expect(screen.queryByTestId('aidoc-stale-banner')).toBeNull();

    fireEvent.click(await screen.findByText('AGENTS.md'));
    const banner = await screen.findByTestId('aidoc-stale-banner');
    expect(banner).toHaveTextContent('docState.stale.explanation');
    expect(banner).toHaveTextContent('src/lib.rs, Cargo.toml');
    expect(banner).not.toHaveTextContent('src/main.rs');

    fireEvent.click(within(banner).getByTestId('aidoc-open-doc-state'));
    await expectStaleExplanation();
  });
});

describe('KT-1218 — the explanation is worded in the four locales', () => {
  const KEYS = [
    'docState.stale.title', 'docState.stale.explanation', 'docState.stale.step',
    'docState.stale.sources', 'docState.stale.updateEffect', 'docState.stale.unchangedEffect',
    'docState.stale.update', 'docState.staleDoc.title', 'docState.staleDoc.sources',
    'audit.revalidate.button', 'audit.revalidate.title',
  ];

  it('defines every key of the explanation in fr, en, es and zh', () => {
    for (const [name, dict] of Object.entries({ fr, en, es, zh }) as [string, Record<string, string>][]) {
      for (const key of KEYS) {
        expect(dict[key], `${name}:${key}`).toBeTruthy();
      }
    }
  });

  it('keeps the plain-words French sentence and the update effects', () => {
    const frDict = fr as Record<string, string>;
    expect(frDict['docState.stale.explanation'])
      .toBe('Le code cité par ces sections a changé depuis le dernier audit : la doc peut ne plus être exacte.');
    expect(frDict['docState.stale.updateEffect']).toContain('briefing');
    expect(frDict['docState.stale.updateEffect']).toContain('à reconfirmer');
    expect(frDict['docState.stale.unchangedEffect']).toContain('Vérifié, inchangé');
  });
});
