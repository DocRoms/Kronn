// KT-1270 — a workflow's scope is shown on the launch card, suspended once the
// workflow changed, and saving it is the human's validation.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import type { Project, WorkflowScope } from '../../types/generated';

const api = vi.hoisted(() => ({ scope: vi.fn(), setScope: vi.fn(), revokeScope: vi.fn() }));
vi.mock('../../lib/api', () => ({ workflows: api }));

import { WorkflowScopeEditor, WorkflowScopeSummary } from '../WorkflowScope';

const t = (key: string, ...args: (string | number)[]) => (args.length ? `${key}:${args.join('|')}` : key);
const projects = [
  { id: 'p1', name: 'front', path: '/p1' },
  { id: 'p2', name: 'api', path: '/p2' },
] as unknown as Project[];

function scope(overrides: Partial<WorkflowScope> = {}): WorkflowScope {
  return {
    workflow_id: 'wf', supported: true, entries: [{ project_id: 'p2', project_name: 'api', access: 'write' }],
    valid: true, validated_by: 'romu', validated_at: '2026-10-16T10:00:00Z', ...overrides,
  };
}

beforeEach(() => {
  api.scope.mockReset();
  api.setScope.mockReset();
  api.revokeScope.mockReset();
});

describe('WorkflowScopeSummary', () => {
  it('says what the run reaches, or that the scope is suspended', async () => {
    api.scope.mockResolvedValue(scope());
    const { unmount } = render(<WorkflowScopeSummary workflowId="wf" t={t} />);
    expect((await screen.findByTestId('workflow-scope-summary')).textContent).toContain('wf.scopeOpens');
    unmount();
    api.scope.mockResolvedValue(scope({ valid: false }));
    render(<WorkflowScopeSummary workflowId="wf" t={t} />);
    expect((await screen.findByTestId('workflow-scope-summary')).textContent).toContain('wf.scopeSuspended');
  });

  it('shows nothing for a workflow without a scope', async () => {
    api.scope.mockResolvedValue(scope({ entries: [], valid: false }));
    const { container } = render(<WorkflowScopeSummary workflowId="wf" t={t} />);
    await waitFor(() => expect(api.scope).toHaveBeenCalled());
    expect(container.textContent).toBe('');
  });
});

describe('WorkflowScopeEditor', () => {
  it('revalidates a suspended scope as it is', async () => {
    api.scope.mockResolvedValue(scope({ valid: false }));
    api.setScope.mockResolvedValue(scope());
    render(<WorkflowScopeEditor workflowId="wf" ownProjectId="p1" projects={projects} toast={vi.fn()} t={t} />);
    fireEvent.click(await screen.findByText('wf.scopeRevalidate'));
    await waitFor(() => expect(api.setScope).toHaveBeenCalledWith('wf', {
      entries: [{ project_id: 'p2', access: 'write' }],
    }));
  });

  it('keeps an unsupported workflow\'s grants visible and revocable, without offering new ones', async () => {
    api.scope.mockResolvedValue(scope({ supported: false, valid: false }));
    api.revokeScope.mockResolvedValue(scope({ supported: false, valid: false, entries: [] }));
    render(<WorkflowScopeEditor workflowId="wf" ownProjectId="p1" projects={projects} toast={vi.fn()} t={t} />);
    expect(await screen.findByText('wf.scopeUnsupported')).toBeTruthy();
    expect(screen.queryByText('disc.scopeGrant')).toBeNull();
    fireEvent.click(screen.getByText('disc.scopeRevoke'));
    await waitFor(() => expect(api.revokeScope).toHaveBeenCalledWith('wf', { project_id: 'p2' }));
  });
});
