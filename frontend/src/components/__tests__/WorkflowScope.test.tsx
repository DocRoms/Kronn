// KT-1270 — a workflow's scope is shown on the launch card, suspended once the
// workflow changed, and saving it is the human's validation.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
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
    workflow_id: 'wf', supported: true, all: null, fingerprint: 'fp-1', entries: [{ project_id: 'p2', project_name: 'api', access: 'write' }],
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

describe('WorkflowScopeEditor — folded line (KT-1355)', () => {
  // The levels read as words, as in the real catalogue, so the chips can be compared.
  const words: Record<string, string> = { 'wf.scopeLevelRead': 'Read', 'wf.scopeLevelWrite': 'Write' };
  const tw = (key: string, ...args: (string | number)[]) => words[key] ?? t(key, ...args);
  const renderEditor = () => render(
    <WorkflowScopeEditor workflowId="wf" ownProjectId="p1" projects={projects} toast={vi.fn()} t={tw} />,
  );
  const chips = async () => (await screen.findByTestId('workflow-scope-chips')).querySelectorAll('li');

  it('folds a validated scope to its title and one chip per grant, the editor behind Details', async () => {
    api.scope.mockResolvedValue(scope({
      entries: [
        { project_id: 'p2', project_name: 'front_api', access: 'write' },
        { project_id: 'p3', project_name: 'front_apollo', access: 'read' },
      ],
    }));
    renderEditor();
    const items = await chips();
    expect([...items].map(li => li.textContent)).toEqual(['front_api · write', 'front_apollo · read']);
    expect(screen.getByRole('heading', { name: 'wf.scopeTitle' })).toBeTruthy();
    expect(screen.queryByRole('radiogroup')).toBeNull();
    expect(screen.queryByText('wf.scopeHint')).toBeNull();
    expect(screen.getByTestId('workflow-scope-editor').getAttribute('data-open')).toBe('false');
    const toggle = screen.getByTestId('workflow-scope-toggle');
    expect(toggle.getAttribute('aria-expanded')).toBe('false');

    fireEvent.click(toggle);
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    expect(toggle.getAttribute('aria-controls')).toBe(screen.getByText('wf.scopeHint').parentElement!.id);
    expect(screen.getByRole('radiogroup', { name: 'disc.scopeAllProjects' })).toBeTruthy();
    expect(screen.queryByTestId('workflow-scope-chips')).toBeNull();

    fireEvent.click(toggle);
    expect(await chips()).toHaveLength(2);
    expect(screen.queryByRole('radiogroup')).toBeNull();
  });

  it('shows the global-access badge for every project', async () => {
    api.scope.mockResolvedValue(scope({ entries: [], all: 'write' }));
    renderEditor();
    const [badge] = await chips();
    expect(badge.textContent).toBe('disc.scopeAllProjects · write');
    expect(badge.getAttribute('data-kind')).toBe('all');
    expect(screen.queryByRole('radiogroup')).toBeNull();
  });

  it('says there is no project access, folded, when nothing is granted', async () => {
    api.scope.mockResolvedValue(scope({ entries: [], valid: false, validated_by: null, validated_at: null }));
    renderEditor();
    const items = await chips();
    expect([...items].map(li => li.textContent)).toEqual(['wf.scopeNoAccess']);
    expect(screen.queryByRole('radiogroup')).toBeNull();
  });

  it('opens by itself when the scope was never validated', async () => {
    api.scope.mockResolvedValue(scope({ valid: false, validated_by: null, validated_at: null }));
    renderEditor();
    expect(await screen.findByRole('radiogroup', { name: 'api' })).toBeTruthy();
    expect(screen.getByTestId('workflow-scope-editor').getAttribute('data-status')).toBe('pending');
    expect(screen.getByText('wf.scopeStatus.pending')).toBeTruthy();
    expect(screen.getByTestId('workflow-scope-toggle').getAttribute('aria-expanded')).toBe('true');
  });

  it('opens by itself when an edit of the workflow suspended the scope, and can be folded', async () => {
    api.scope.mockResolvedValue(scope({ valid: false }));
    renderEditor();
    expect(await screen.findByText('wf.scopeNeedsValidation')).toBeTruthy();
    fireEvent.click(screen.getByTestId('workflow-scope-toggle'));
    const items = await chips();
    expect(items[0].textContent).toBe('wf.scopeChipSuspended');
    expect(items[1].textContent).toBe('api · write');
  });

  it('folds back by itself once the human validated a scope that was waiting for it', async () => {
    api.scope.mockResolvedValue(scope({ valid: false }));
    api.setScope.mockResolvedValue(scope());
    renderEditor();
    fireEvent.click(await screen.findByText('wf.scopeRevalidate'));
    expect((await chips())[0].textContent).toBe('api · write');
  });

  it('keeps an unsupported workflow\'s revocable grants open, and an empty one folded', async () => {
    api.scope.mockResolvedValue(scope({ supported: false, valid: false, entries: [] }));
    renderEditor();
    const items = await chips();
    expect([...items].map(li => li.textContent)).toEqual(['wf.scopeStatus.unsupported']);
    expect(screen.queryByText('wf.scopeUnsupported')).toBeNull();
  });

  it('folds grants past three into a +N chip, the full list staying in the editor', async () => {
    const entries = Array.from({ length: 8 }, (_, i) => ({ project_id: `x${i}`, project_name: `proj${i}`, access: 'read' as const }));
    api.scope.mockResolvedValue(scope({ entries }));
    renderEditor();
    const items = await chips();
    expect(items).toHaveLength(4);
    expect(items[3].textContent).toBe('+5');
  });

  it('keeps a long project name whole in the chip title, for the ellipsis to cut it', async () => {
    const name = 'a-very-long-project-name-'.repeat(8);
    api.scope.mockResolvedValue(scope({ entries: [{ project_id: 'x', project_name: name, access: 'write' }] }));
    renderEditor();
    const label = (await chips())[0].querySelector('.wfs-chip-label');
    expect(label?.getAttribute('title')).toBe(`${name} · write`);
  });

  it('gives the focus back to Details when validating folds the editor away', async () => {
    api.scope.mockResolvedValue(scope({ valid: false }));
    api.setScope.mockResolvedValue(scope());
    renderEditor();
    const validate = await screen.findByText('wf.scopeRevalidate');
    validate.focus();
    expect(document.activeElement).toBe(validate);
    fireEvent.click(validate);
    const toggle = await screen.findByTestId('workflow-scope-toggle');
    await waitFor(() => expect(toggle.getAttribute('aria-expanded')).toBe('false'));
    expect(document.activeElement).toBe(toggle);
  });

  it('leaves the focus alone when folding with Details itself', async () => {
    api.scope.mockResolvedValue(scope());
    renderEditor();
    const toggle = await screen.findByTestId('workflow-scope-toggle');
    fireEvent.click(toggle);
    fireEvent.click(toggle);
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
  });
});

// jsdom does no layout: the narrow-screen contract is pinned on the stylesheet
// (the folded line wraps instead of overflowing, chips cut long names, Details
// keeps its own room, the warning ink is mixed toward the text colour).
describe('WorkflowScope.css — folded line on a narrow screen (KT-1355)', () => {
  const css = readFileSync(resolve(__dirname, '../WorkflowScope.css'), 'utf8');
  const rule = (selector: string) => {
    const match = css.match(new RegExp(`(?:^|\\n)${selector.replace(/[.[\]="]/g, '\\$&')}\\s*\\{([^}]*)\\}`));
    return match?.[1] ?? '';
  };

  it('lets the chip list shrink and wrap, and reserves Details its own room', () => {
    expect(rule('.wfs-chips')).toMatch(/flex:\s*1 1 12rem/);
    expect(rule('.wfs-chips')).toMatch(/min-width:\s*0/);
    expect(rule('.wfs-head-end')).toMatch(/flex:\s*none/);
    expect(rule('.wfs[data-open="false"] .wfs-head')).toMatch(/flex-wrap:\s*wrap/);
  });

  it('cuts a long chip label with an ellipsis instead of widening the page', () => {
    expect(rule('.wfs-chip')).toMatch(/max-width:\s*100%/);
    expect(rule('.wfs-chip-label')).toMatch(/text-overflow:\s*ellipsis/);
    expect(rule('.wfs-chip-label')).toMatch(/overflow:\s*hidden/);
  });

  it('never sets small warning text in the bare warning colour', () => {
    expect(css).toMatch(/--wfs-warn-ink:\s*color-mix\(/);
    expect(rule('.wfs-chip[data-access="write"]')).toContain('var(--wfs-warn-ink)');
    expect(rule('.wfs-chip[data-kind="status"]')).toContain('var(--wfs-warn-ink)');
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
      all: null,
      confirm: null,
      expected_fingerprint: null,
    }));
  });

  it('keeps an unsupported workflow\'s grants visible and revocable, without offering new ones', async () => {
    api.scope.mockResolvedValue(scope({ supported: false, valid: false }));
    api.revokeScope.mockResolvedValue(scope({ supported: false, valid: false, entries: [] }));
    render(<WorkflowScopeEditor workflowId="wf" ownProjectId="p1" projects={projects} toast={vi.fn()} t={t} />);
    expect(await screen.findByText('wf.scopeUnsupported')).toBeTruthy();
    expect(screen.queryByText('disc.scopeGrant')).toBeNull();
    fireEvent.click(screen.getByText('disc.scopeRevoke'));
    await waitFor(() => expect(api.revokeScope).toHaveBeenCalledWith('wf', { project_id: 'p2', all: false }));
  });

  it('opens every project in write only after the typed confirmation, with the scope seen', async () => {
    api.scope.mockResolvedValue(scope({ entries: [], valid: false, validated_by: null, validated_at: null }));
    api.setScope.mockResolvedValue(scope({ entries: [], all: 'write' }));
    render(<WorkflowScopeEditor workflowId="wf" ownProjectId="p1" projects={projects} toast={vi.fn()} t={t} />);
    fireEvent.click(await screen.findByTestId('workflow-scope-toggle'));
    const allGroup = await screen.findByRole('radiogroup', { name: 'disc.scopeAllProjects' });
    fireEvent.click(allGroup.querySelector('[data-level="write"]')!);
    const validate = screen.getByText('wf.scopeValidate').closest('button')!;
    expect(validate.disabled).toBe(true);
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'ALL-PROJECTS-WRITE' } });
    expect(validate.disabled).toBe(false);
    fireEvent.click(validate);
    await waitFor(() => expect(api.setScope).toHaveBeenCalledWith('wf', {
      entries: [],
      all: 'write',
      confirm: 'ALL-PROJECTS-WRITE',
      expected_fingerprint: 'fp-1',
    }));
  });

  it('picks a project in one click and validates the whole scope at once', async () => {
    api.scope.mockResolvedValue(scope({ entries: [], valid: false, validated_by: null, validated_at: null }));
    api.setScope.mockResolvedValue(scope());
    render(<WorkflowScopeEditor workflowId="wf" ownProjectId="p1" projects={projects} toast={vi.fn()} t={t} />);
    fireEvent.click(await screen.findByTestId('workflow-scope-toggle'));
    const apiGroup = await screen.findByRole('radiogroup', { name: 'api' });
    fireEvent.click(apiGroup.querySelector('[data-level="read"]')!);
    expect(screen.queryByRole('radiogroup', { name: 'front' })).toBeNull();
    fireEvent.click(screen.getByText('wf.scopeValidate'));
    await waitFor(() => expect(api.setScope).toHaveBeenCalledWith('wf', {
      entries: [{ project_id: 'p2', access: 'read' }],
      all: null,
      confirm: null,
      expected_fingerprint: null,
    }));
  });

  it('asks the typed confirmation again to revalidate every project in write', async () => {
    api.scope.mockResolvedValue(scope({ entries: [], all: 'write', valid: false, fingerprint: 'fp-2' }));
    api.setScope.mockResolvedValue(scope({ entries: [], all: 'write' }));
    render(<WorkflowScopeEditor workflowId="wf" ownProjectId="p1" projects={projects} toast={vi.fn()} t={t} />);
    const revalidate = (await screen.findByText('wf.scopeRevalidate')).closest('button')!;
    expect(revalidate.disabled).toBe(true);
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'ALL-PROJECTS-WRITE' } });
    expect(revalidate.disabled).toBe(false);
    fireEvent.click(revalidate);
    await waitFor(() => expect(api.setScope).toHaveBeenCalledWith('wf', {
      entries: [],
      all: 'write',
      confirm: 'ALL-PROJECTS-WRITE',
      expected_fingerprint: 'fp-2',
    }));
  });
});
