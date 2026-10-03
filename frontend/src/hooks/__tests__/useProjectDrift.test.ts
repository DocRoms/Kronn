// KT-987 — every refresh of the project list asked drift again for every
// audited project, and drift hashes each mapped source.
import { describe, expect, it, vi, beforeEach } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';
import type { Project } from '../../types/generated';

vi.mock('../../lib/api', () => ({
  projects: { checkDrift: vi.fn() },
}));
import { projects as projectsApi } from '../../lib/api';
import { useProjectDrift } from '../useProjectDrift';

const project = (id: string, audit_status: Project['audit_status']): Project => ({
  id,
  name: id,
  path: `/repos/${id}`,
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status,
  ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false, path_exists: true,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
});

const drift = { audit_date: null, stale_sections: [], fresh_sections: [], total_sections: 0 };

beforeEach(() => {
  vi.mocked(projectsApi.checkDrift).mockReset().mockResolvedValue(drift);
});

describe('useProjectDrift', () => {
  it('asks nothing while the Projects page is not open', () => {
    renderHook(() => useProjectDrift(false, [project('a', 'Audited')]));
    expect(projectsApi.checkDrift).not.toHaveBeenCalled();
  });

  it('asks once per audited project across refreshes of the project list', async () => {
    const { result, rerender } = renderHook(
      ({ list }) => useProjectDrift(true, list),
      { initialProps: { list: [project('a', 'Audited'), project('b', 'NoTemplate')] } },
    );
    await waitFor(() => expect(result.current.driftByProject.a).toEqual(drift));

    // A refresh returns new objects for the same projects.
    rerender({ list: [project('a', 'Audited'), project('b', 'NoTemplate')] });
    rerender({ list: [project('a', 'Audited'), project('b', 'NoTemplate')] });

    expect(vi.mocked(projectsApi.checkDrift).mock.calls).toEqual([['a']]);
  });

  it('asks again when an audit changes the project state', () => {
    const { rerender } = renderHook(
      ({ list }) => useProjectDrift(true, list),
      { initialProps: { list: [project('a', 'Audited')] } },
    );
    rerender({ list: [project('a', 'Validated')] });
    expect(vi.mocked(projectsApi.checkDrift).mock.calls).toEqual([['a'], ['a']]);
  });

  it('asks again after a failure', async () => {
    vi.mocked(projectsApi.checkDrift).mockRejectedValueOnce(new Error('down'));
    const { rerender } = renderHook(
      ({ list }) => useProjectDrift(true, list),
      { initialProps: { list: [project('a', 'Audited')] } },
    );
    await waitFor(() => expect(projectsApi.checkDrift).toHaveBeenCalledTimes(1));
    await new Promise(resolve => setTimeout(resolve, 0));
    rerender({ list: [project('a', 'Audited')] });
    expect(projectsApi.checkDrift).toHaveBeenCalledTimes(2);
  });
});
