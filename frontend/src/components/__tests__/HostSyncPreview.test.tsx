// Unit tests for HostSyncPreview — Phase-3 refactor.
// Validates the dynamic preview shows the right destinations per CLI
// based on Kronn scope (is_global / project_ids), and surfaces the
// asymmetry between Claude (scope-aware) and Gemini/Codex/Copilot
// (always top-level).

import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import type { Project } from '../../types/generated';
import { HostSyncPreview } from '../HostSyncPreview';

function mkProject(id: string, name: string, path: string): Project {
  return {
    id, name, path,
    repo_url: null,
    token_override: null,
    ai_config: { detected: false, configs: [] },
    audit_status: 'NoTemplate',
    ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false, path_exists: true,
    created_at: '2026-04-27T00:00:00Z',
    updated_at: '2026-04-27T00:00:00Z',
  };
}

describe('HostSyncPreview', () => {
  const projects = [
    mkProject('p1', 'APP_ANDROID', '/home/me/Repos/APP_ANDROID'),
    mkProject('p2', 'APP_IOS', '/home/me/Repos/APP_IOS'),
    mkProject('p3', 'OTHER', '/home/me/Repos/OTHER'),
  ];

  it('renders Claude top-level when isGlobal=true', () => {
    render(<HostSyncPreview isGlobal projectIds={[]} projects={projects} />);
    expect(screen.getByText(/~\/.claude.json \(top-level, tous projets\)/)).toBeInTheDocument();
  });

  it('renders Claude per-project when isGlobal=false and project_ids set', () => {
    render(<HostSyncPreview isGlobal={false} projectIds={['p1', 'p2']} projects={projects} />);
    expect(screen.getByText(/projects\[\/home\/me\/Repos\/APP_ANDROID\]/)).toBeInTheDocument();
    expect(screen.getByText(/projects\[\/home\/me\/Repos\/APP_IOS\]/)).toBeInTheDocument();
    expect(screen.queryByText(/projects\[\/home\/me\/Repos\/OTHER\]/)).not.toBeInTheDocument();
  });

  it('renders Claude top-level fallback when no project linked', () => {
    render(<HostSyncPreview isGlobal={false} projectIds={[]} projects={projects} />);
    expect(screen.getByText(/aucun projet sélectionné/)).toBeInTheDocument();
  });

  it('shows asymmetry hint on Gemini/Codex/Copilot when scope is per-project', () => {
    render(<HostSyncPreview isGlobal={false} projectIds={['p1']} projects={projects} />);
    // The "scope projet non supporté" hint should appear on Gemini/Codex/Copilot
    const hints = screen.getAllByText(/scope projet non supporté/);
    expect(hints.length).toBeGreaterThanOrEqual(3);
  });

  it('does NOT show asymmetry hint when scope is global', () => {
    render(<HostSyncPreview isGlobal projectIds={[]} projects={projects} />);
    expect(screen.queryByText(/scope projet non supporté/)).not.toBeInTheDocument();
  });

  it('always lists all 4 CLIs', () => {
    render(<HostSyncPreview isGlobal projectIds={[]} projects={projects} />);
    expect(screen.getByText('Claude Code')).toBeInTheDocument();
    expect(screen.getByText('Gemini')).toBeInTheDocument();
    expect(screen.getByText('Codex')).toBeInTheDocument();
    expect(screen.getByText('Copilot')).toBeInTheDocument();
  });

  it('sorts project paths alphabetically by project name', () => {
    // Pass IDs in non-alphabetical order; output should follow project name sort
    const { container } = render(
      <HostSyncPreview isGlobal={false} projectIds={['p3', 'p1']} projects={projects} />
    );
    const html = container.innerHTML;
    const androidIdx = html.indexOf('APP_ANDROID');
    const otherIdx = html.indexOf('OTHER');
    expect(androidIdx).toBeGreaterThan(-1);
    expect(otherIdx).toBeGreaterThan(-1);
    expect(androidIdx).toBeLessThan(otherIdx);
  });
});
