// KT-999 — each linked repo says which AI doc it really has.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import type { LinkedRepo } from '../../types/generated';

const { linkedReposDocs, linkedReposCandidates } = vi.hoisted(() => ({
  linkedReposDocs: vi.fn(),
  linkedReposCandidates: vi.fn(),
}));

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock({ projects: { linkedReposDocs, linkedReposCandidates } });
});

import { ProjectLinkedRepos } from '../ProjectLinkedRepos';

const repo = (id: string, location: string): LinkedRepo => ({ id, name: id, kind: 'api', location });
const REPOS = [
  repo('agents', '/r/agents'),
  repo('legacy', '/r/legacy'),
  repo('readme', '/r/readme'),
  repo('bare', '/r/bare'),
  repo('gone', '/r/gone'),
  repo('remote', 'https://github.com/org/remote'),
];

beforeEach(() => {
  vi.clearAllMocks();
  linkedReposCandidates.mockResolvedValue([]);
  linkedReposDocs.mockResolvedValue([
    { id: 'agents', ai_doc: 'agents_md', doc_path: 'docs/AGENTS.md' },
    { id: 'legacy', ai_doc: 'legacy_ai_index', doc_path: 'ai/index.md' },
    { id: 'readme', ai_doc: 'readme_only', doc_path: 'README.md' },
    { id: 'bare', ai_doc: 'no_ai_doc', doc_path: null },
    { id: 'gone', ai_doc: 'missing', doc_path: null },
    { id: 'remote', ai_doc: 'remote', doc_path: null },
  ]);
});

describe('ProjectLinkedRepos AI doc badges (KT-999)', () => {
  it('labels every local repo by what it has and leaves a URL unlabelled', async () => {
    render(
      <I18nProvider>
        <ProjectLinkedRepos projectId="p" currentRepos={REPOS} onUpdate={() => {}} />
      </I18nProvider>,
    );
    await waitFor(() => expect(screen.getByTestId('linked-repo-ai-doc-agents')).toBeTruthy());
    expect(linkedReposDocs).toHaveBeenCalledWith('p');
    const badge = (id: string) => screen.queryByTestId(`linked-repo-ai-doc-${id}`);
    expect(badge('agents')).toHaveTextContent('doc IA');
    expect(badge('agents')).toHaveAttribute('title', 'docs/AGENTS.md');
    expect(badge('legacy')).toHaveTextContent('doc IA');
    expect(badge('readme')).toHaveTextContent('README seulement');
    expect(badge('bare')).toHaveTextContent('pas de doc IA');
    expect(badge('gone')).toHaveTextContent('pas de doc IA');
    expect(badge('gone')).toHaveAttribute('title', 'Chemin introuvable sur cette machine');
    expect(badge('remote')).toBeNull();
    // Without the inline suggestions, candidates wait for the draft.
    expect(linkedReposCandidates).not.toHaveBeenCalled();
  });

  it('asks nothing when there is no linked repo', async () => {
    render(
      <I18nProvider>
        <ProjectLinkedRepos projectId="p" currentRepos={[]} onUpdate={() => {}} />
      </I18nProvider>,
    );
    await screen.findByText(/Ajouter un dépôt lié/);
    expect(linkedReposDocs).not.toHaveBeenCalled();
  });
});
