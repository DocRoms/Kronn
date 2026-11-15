import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';
import { ApiRequestError } from '../../lib/apiRequestError';

const { policyMock, workspaceMock, setWorkspaceMock } = vi.hoisted(() => ({
  policyMock: vi.fn(),
  workspaceMock: vi.fn(),
  setWorkspaceMock: vi.fn(),
}));

vi.mock('../../lib/api', () => buildApiMock({
  projects: {
    commitPolicy: policyMock as never,
    workspace: workspaceMock as never,
    setWorkspace: setWorkspaceMock as never,
  },
}));

import { ProjectCommitPolicySetting } from '../ProjectCommitPolicySetting';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join('|')}` : key;

const resolved = (source: string, signoffSource: string, signoff = false) => ({
  name: 'Work Person',
  email: 'work@corp.example',
  identity_source: source,
  signoff,
  signoff_source: signoffSource,
});

const view = (over: Record<string, unknown> = {}) => ({
  settings: { signoff: 'auto' },
  detected: resolved('repository', 'history'),
  effective: resolved('repository', 'history'),
  global_identity: false,
  ...over,
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('ProjectCommitPolicySetting (KT-1750)', () => {
  it('defaults to Inherit and shows the value in force with where it comes from', async () => {
    policyMock.mockResolvedValue(view());
    render(<ProjectCommitPolicySetting projectId="p1" t={t} />);

    await waitFor(() => expect(screen.getByTestId('project-commit-policy-effective'))
      .toHaveTextContent('projects.commitPolicy.effective:Work Person <work@corp.example>|projects.commitPolicy.identitySource.repository'));
    expect(screen.getByTestId('project-commit-policy-name')).toHaveAttribute('placeholder', 'Work Person');
    expect(screen.getByTestId('project-commit-policy-email')).toHaveAttribute('placeholder', 'work@corp.example');
    expect(screen.getByTestId('project-commit-policy-signoff-auto')).toBeChecked();
    expect(screen.getByText('projects.commitPolicy.signoff.auto')).toBeInTheDocument();
    expect(screen.getByTestId('project-commit-policy-signoff-effective'))
      .toHaveTextContent('projects.commitPolicy.signoffEffective:projects.commitPolicy.signoffNo|projects.commitPolicy.signoffSource.history');
    expect(screen.queryByTestId('project-commit-policy-global-warning')).toBeNull();
    expect(screen.queryByTestId('project-commit-policy-load-error')).toBeNull();
    expect(screen.getByTestId('project-commit-policy-save')).toBeDisabled();
  });

  it('shows the Kronn Settings > Workers value when the repository says nothing', async () => {
    policyMock.mockResolvedValue(view({
      detected: { ...resolved('kronn_global', 'kronn_global', true), email: 'team@corp.example' },
      effective: { ...resolved('kronn_global', 'kronn_global', true), email: 'team@corp.example' },
    }));
    render(<ProjectCommitPolicySetting projectId="p1" t={t} />);

    await waitFor(() => expect(screen.getByTestId('project-commit-policy-effective'))
      .toHaveTextContent('Work Person <team@corp.example>|projects.commitPolicy.identitySource.kronn_global'));
    expect(screen.getByTestId('project-commit-policy-signoff-effective'))
      .toHaveTextContent('projects.commitPolicy.signoffYes|projects.commitPolicy.signoffSource.kronn_global');
    // A Kronn setting is a decision: no machine-identity warning.
    expect(screen.queryByTestId('project-commit-policy-global-warning')).toBeNull();
  });

  it('warns when the identity is the machine git one, naming the address', async () => {
    const machine = { name: 'Me', email: 'me@personal.example', identity_source: 'global', signoff: false, signoff_source: 'default' };
    policyMock.mockResolvedValue(view({ detected: machine, effective: machine, global_identity: true }));
    render(<ProjectCommitPolicySetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-commit-policy-global-warning'))
      .toHaveTextContent('projects.commitPolicy.globalWarning:me@personal.example'));
  });

  it('labels its fields like the other settings, not as headings', async () => {
    policyMock.mockResolvedValue(view());
    render(<ProjectCommitPolicySetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-commit-policy-name')).not.toBeDisabled());
    for (const key of ['projects.commitPolicy.identity', 'projects.commitPolicy.signoff']) {
      const label = screen.getByText(key);
      expect(label.tagName).toBe('SPAN');
      // The Workers 'Commits Kronn' block's treatment. `.label` alone would
      // render larger (fs-base) and bolder (600) than the section's h4.
      expect(label).toHaveClass('text-sm');
      expect(label).not.toHaveClass('label');
      expect(label.closest('strong')).toBeNull();
      expect(label.closest('h4')).toBeNull();
    }
  });

  it('saves the identity and the sign-off with the worktree hooks it found', async () => {
    policyMock.mockResolvedValueOnce(view());
    policyMock.mockResolvedValueOnce(view({
      settings: { name: 'Bot', email: 'bot@corp.example', signoff: 'always' },
      effective: resolved('setting', 'setting', true),
    }));
    workspaceMock.mockResolvedValue({ hooks: { after_create: 'make setup' }, commit: { signoff: 'auto' } });
    setWorkspaceMock.mockResolvedValue(true);
    render(<ProjectCommitPolicySetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-commit-policy-name')).not.toBeDisabled());

    fireEvent.change(screen.getByTestId('project-commit-policy-name'), { target: { value: ' Bot ' } });
    fireEvent.change(screen.getByTestId('project-commit-policy-email'), { target: { value: 'bot@corp.example' } });
    fireEvent.click(screen.getByTestId('project-commit-policy-signoff-always'));
    fireEvent.click(screen.getByTestId('project-commit-policy-save'));

    await waitFor(() => expect(setWorkspaceMock).toHaveBeenCalledWith('p1', {
      hooks: { after_create: 'make setup' },
      lane_warm: {},
      commit: { name: 'Bot', email: 'bot@corp.example', signoff: 'always' },
    }));
    await waitFor(() => expect(screen.getByTestId('project-commit-policy-name')).toHaveValue('Bot'));
    expect(screen.getByTestId('project-commit-policy-signoff-always')).toBeChecked();
  });

  it('keeps the lane warm-up and the managed roots when it saves the identity', async () => {
    policyMock.mockResolvedValue(view());
    workspaceMock.mockResolvedValue({
      hooks: {},
      lane_warm: { seed_alignment: false },
      managed_worktree_roots: ['/srv/worktrees'],
    });
    setWorkspaceMock.mockResolvedValue(true);
    render(<ProjectCommitPolicySetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-commit-policy-name')).not.toBeDisabled());

    fireEvent.change(screen.getByTestId('project-commit-policy-name'), { target: { value: 'Bot' } });
    fireEvent.change(screen.getByTestId('project-commit-policy-email'), { target: { value: 'bot@corp.example' } });
    fireEvent.click(screen.getByTestId('project-commit-policy-save'));

    await waitFor(() => expect(setWorkspaceMock).toHaveBeenCalledWith('p1', {
      hooks: {},
      lane_warm: { seed_alignment: false },
      managed_worktree_roots: ['/srv/worktrees'],
      commit: { name: 'Bot', email: 'bot@corp.example', signoff: 'auto' },
    }));
  });

  it('refuses half an identity and goes back to the inherited values on request', async () => {
    policyMock.mockResolvedValue(view({ settings: { name: 'Bot', email: 'bot@corp.example', signoff: 'never' } }));
    workspaceMock.mockResolvedValue(null);
    setWorkspaceMock.mockResolvedValue(true);
    render(<ProjectCommitPolicySetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-commit-policy-name')).toHaveValue('Bot'));

    fireEvent.change(screen.getByTestId('project-commit-policy-email'), { target: { value: '' } });
    expect(screen.getByTestId('project-commit-policy-incomplete')).toBeInTheDocument();
    expect(screen.getByTestId('project-commit-policy-save')).toBeDisabled();

    fireEvent.click(screen.getByTestId('project-commit-policy-reset'));
    await waitFor(() => expect(setWorkspaceMock).toHaveBeenCalledWith('p1', {
      hooks: {},
      lane_warm: {},
      commit: { name: null, email: null, signoff: 'auto' },
    }));
  });

  describe('when the detected identity cannot be read', () => {
    const failLoad = () => policyMock.mockRejectedValue(new ApiRequestError('Server error (HTTP 404)'));

    it('shows a translated message, not the raw server error, and keeps the form usable', async () => {
      failLoad();
      workspaceMock.mockResolvedValue({ hooks: {}, commit: { name: 'Bot', email: 'bot@corp.example', signoff: 'never' } });
      render(<ProjectCommitPolicySetting projectId="p1" t={t} />);

      await waitFor(() => expect(screen.getByTestId('project-commit-policy-load-error'))
        .toHaveTextContent('projects.commitPolicy.loadError'));
      expect(screen.queryByText(/Server error/)).toBeNull();
      // The stored settings are still read, from the workspace, and editable.
      await waitFor(() => expect(screen.getByTestId('project-commit-policy-name')).toHaveValue('Bot'));
      expect(screen.getByTestId('project-commit-policy-name')).not.toBeDisabled();
      expect(screen.getByTestId('project-commit-policy-email')).not.toBeDisabled();
      expect(screen.getByTestId('project-commit-policy-signoff-always')).not.toBeDisabled();
      expect(screen.queryByTestId('project-commit-policy-effective')).toBeNull();
      expect(screen.getByTestId('project-commit-policy-signoff-never')).toBeChecked();
    });

    it('disables only the detected-values button, with its reason as a title', async () => {
      failLoad();
      workspaceMock.mockResolvedValue({ hooks: {}, commit: { name: 'Bot', email: 'bot@corp.example', signoff: 'never' } });
      render(<ProjectCommitPolicySetting projectId="p1" t={t} />);
      await waitFor(() => expect(screen.getByTestId('project-commit-policy-load-error')).toBeInTheDocument());
      await waitFor(() => expect(screen.getByTestId('project-commit-policy-name')).toHaveValue('Bot'));

      const reset = screen.getByTestId('project-commit-policy-reset');
      expect(reset).toBeDisabled();
      expect(reset).toHaveAttribute('title', 'projects.commitPolicy.useDetectedUnavailable');

      // Save is not disabled by the failed load: an edit can still be saved.
      expect(screen.getByTestId('project-commit-policy-save')).toBeDisabled(); // nothing changed yet
      fireEvent.click(screen.getByTestId('project-commit-policy-signoff-always'));
      expect(screen.getByTestId('project-commit-policy-save')).not.toBeDisabled();
    });

    it('saves from the stored settings it could read, keeping the worktree hooks', async () => {
      failLoad();
      workspaceMock.mockResolvedValue({ hooks: { after_create: 'make setup' } });
      setWorkspaceMock.mockResolvedValue(true);
      render(<ProjectCommitPolicySetting projectId="p1" t={t} />);
      await waitFor(() => expect(screen.getByTestId('project-commit-policy-load-error')).toBeInTheDocument());

      fireEvent.click(screen.getByTestId('project-commit-policy-signoff-always'));
      fireEvent.click(screen.getByTestId('project-commit-policy-save'));
      await waitFor(() => expect(setWorkspaceMock).toHaveBeenCalledWith('p1', {
        hooks: { after_create: 'make setup' },
        lane_warm: {},
        commit: { name: null, email: null, signoff: 'always' },
      }));
      // Read back fails again: what was saved stays shown, with the same message.
      await waitFor(() => expect(screen.getByTestId('project-commit-policy-signoff-always')).toBeChecked());
      expect(screen.getByTestId('project-commit-policy-load-error')).toBeInTheDocument();
      expect(screen.queryByText(/Server error/)).toBeNull();
    });
  });
});
