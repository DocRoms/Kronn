// KT-1270 — a room's reach stays in view, and every project in write needs the
// typed confirmation and the fingerprint of the scope the human saw.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import type { DiscussionScope, DiscussionProjectGrant, Project } from '../../types/generated';

const api = vi.hoisted(() => ({
  scope: vi.fn(),
  grantScope: vi.fn(),
  revokeScope: vi.fn(),
  approveScopeRequest: vi.fn(),
  declineScopeRequest: vi.fn(),
}));
vi.mock('../../lib/api', () => ({ discussions: api }));

import { DiscussionScopeChip, DiscussionScopeSection } from '../DiscussionScope';

const t = (key: string, ...args: (string | number)[]) => (args.length ? `${key}:${args.join('|')}` : key);
const toast = vi.fn();
const projects = [
  { id: 'p1', name: 'front', path: '/p1' },
  { id: 'p2', name: 'api', path: '/p2' },
] as unknown as Project[];

function grant(overrides: Partial<DiscussionProjectGrant>): DiscussionProjectGrant {
  return {
    id: 'g1', discussion_id: 'd1', project_id: 'p2', project_name: 'api', access: 'read', source: 'link',
    request_id: null, granted_by: 'romu', granted_at: '2026-10-16T10:00:00Z',
    revoked_by: null, revoked_at: null, revoked_reason: null, ...overrides,
  };
}

function scope(overrides: Partial<DiscussionScope> = {}): DiscussionScope {
  return {
    discussion_id: 'd1', primary_project_id: 'p1', grants: [], revoked: [], requests: [], fingerprint: 'fp-1',
    ...overrides,
  };
}

beforeEach(() => {
  Object.values(api).forEach(fn => fn.mockReset());
  toast.mockReset();
});

describe('DiscussionScopeChip', () => {
  it('shows nothing for a room on its own project', async () => {
    api.scope.mockResolvedValue(scope());
    const { container } = render(<DiscussionScopeChip discussionId="d1" hasProject t={t} />);
    await waitFor(() => expect(api.scope).toHaveBeenCalled());
    expect(container.querySelector('[data-testid="disc-scope-chip"]')).toBeNull();
  });

  it('shows every project in red and the pending requests', async () => {
    api.scope.mockResolvedValue(scope({
      grants: [grant({ project_id: null, project_name: null, source: 'all', access: 'write' })],
      requests: [{
        id: 'r1', discussion_id: 'd1', project_id: 'p2', project_name: 'api', requested_name: 'api', access: 'write', reason: 'docs',
        requested_by: '@ClaudeCode', status: 'pending', decided_by: null, decided_at: null, granted_access: null,
        created_at: '2026-10-16T10:00:00Z',
      }],
    }));
    render(<DiscussionScopeChip discussionId="d1" hasProject t={t} />);
    const red = await screen.findByText('disc.scopeAllWrite');
    expect(red.closest('button')?.className).toContain('badge-error');
    expect(screen.getByTestId('disc-scope-pending').textContent).toBe('disc.scopePending:1');
  });

  it('never asks for a room without a project', () => {
    render(<DiscussionScopeChip discussionId="d1" hasProject={false} t={t} />);
    expect(api.scope).not.toHaveBeenCalled();
  });
});

describe('DiscussionScopeSection', () => {
  it('opens every project in write only after the typed confirmation, with the seen fingerprint', async () => {
    api.scope.mockResolvedValue(scope());
    api.grantScope.mockResolvedValue(grant({ project_id: null, access: 'write', source: 'all' }));
    render(<DiscussionScopeSection discussionId="d1" primaryProjectId="p1" projects={projects} toast={toast} t={t} />);
    await waitFor(() => expect(api.scope).toHaveBeenCalled());
    fireEvent.click(await screen.findByText('disc.scopeAllWrite'));
    const confirm = screen.getByText('disc.scopeAllWriteConfirm').closest('button')!;
    expect(confirm.disabled).toBe(true);
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'yes' } });
    expect(confirm.disabled).toBe(true);
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'ALL-PROJECTS-WRITE' } });
    expect(confirm.disabled).toBe(false);
    fireEvent.click(confirm);
    await waitFor(() => expect(api.grantScope).toHaveBeenCalledWith('d1', expect.objectContaining({
      all: true, access: 'write', confirm: 'ALL-PROJECTS-WRITE', expected_fingerprint: 'fp-1',
    })));
  });

  it('lets the human narrow a write request to read', async () => {
    api.scope.mockResolvedValue(scope({
      requests: [{
        id: 'r1', discussion_id: 'd1', project_id: 'p2', project_name: 'api', requested_name: 'api', access: 'write', reason: 'docs',
        requested_by: '@ClaudeCode', status: 'pending', decided_by: null, decided_at: null, granted_access: null,
        created_at: '2026-10-16T10:00:00Z',
      }],
    }));
    api.approveScopeRequest.mockResolvedValue(scope());
    render(<DiscussionScopeSection discussionId="d1" primaryProjectId="p1" projects={projects} toast={toast} t={t} />);
    fireEvent.click(await screen.findByText('disc.scopeApproveRead'));
    await waitFor(() => expect(api.approveScopeRequest).toHaveBeenCalledWith('d1', 'r1', expect.objectContaining({ access: 'read' })));
  });

  it('offers no other project to a room without one', () => {
    render(<DiscussionScopeSection discussionId="d1" primaryProjectId={null} projects={projects} toast={toast} t={t} />);
    expect(screen.getByText('disc.scopeNeedsProject')).toBeTruthy();
    expect(api.scope).not.toHaveBeenCalled();
  });
});
