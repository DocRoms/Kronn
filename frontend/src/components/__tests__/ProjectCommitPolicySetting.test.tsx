import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

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

const view = (over: Record<string, unknown> = {}) => ({
  settings: { signoff: 'auto' },
  detected: { name: 'Work Person', email: 'work@corp.example', identity_source: 'repository', signoff: false, signoff_source: 'history' },
  effective: { name: 'Work Person', email: 'work@corp.example', identity_source: 'repository', signoff: false, signoff_source: 'history' },
  global_identity: false,
  ...over,
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('ProjectCommitPolicySetting (KT-1750)', () => {
  it('shows what is detected and where it comes from, as the default of both fields', async () => {
    policyMock.mockResolvedValue(view());
    render(<ProjectCommitPolicySetting projectId="p1" t={t} />);

    await waitFor(() => expect(screen.getByTestId('project-commit-policy-detected'))
      .toHaveTextContent('projects.commitPolicy.detected:Work Person <work@corp.example>|projects.commitPolicy.identitySource.repository'));
    expect(screen.getByTestId('project-commit-policy-name')).toHaveAttribute('placeholder', 'Work Person');
    expect(screen.getByTestId('project-commit-policy-email')).toHaveAttribute('placeholder', 'work@corp.example');
    expect(screen.getByTestId('project-commit-policy-signoff-auto')).toBeChecked();
    expect(screen.getByTestId('project-commit-policy-signoff-detected'))
      .toHaveTextContent('projects.commitPolicy.signoffDetected:projects.commitPolicy.signoffNo|projects.commitPolicy.signoffSource.history');
    expect(screen.queryByTestId('project-commit-policy-global-warning')).toBeNull();
    expect(screen.getByTestId('project-commit-policy-save')).toBeDisabled();
  });

  it('warns when the identity is the global git one, naming the address', async () => {
    policyMock.mockResolvedValue(view({
      detected: { name: 'Me', email: 'me@personal.example', identity_source: 'global', signoff: false, signoff_source: 'default' },
      effective: { name: 'Me', email: 'me@personal.example', identity_source: 'global', signoff: false, signoff_source: 'default' },
      global_identity: true,
    }));
    render(<ProjectCommitPolicySetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-commit-policy-global-warning'))
      .toHaveTextContent('projects.commitPolicy.globalWarning:me@personal.example'));
  });

  it('saves the identity and the sign-off with the worktree hooks it found', async () => {
    policyMock.mockResolvedValueOnce(view());
    policyMock.mockResolvedValueOnce(view({
      settings: { name: 'Bot', email: 'bot@corp.example', signoff: 'always' },
      effective: { name: 'Bot', email: 'bot@corp.example', identity_source: 'setting', signoff: true, signoff_source: 'setting' },
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
      commit: { name: 'Bot', email: 'bot@corp.example', signoff: 'always' },
    }));
    await waitFor(() => expect(screen.getByTestId('project-commit-policy-name')).toHaveValue('Bot'));
    expect(screen.getByTestId('project-commit-policy-signoff-always')).toBeChecked();
  });

  it('refuses half an identity and goes back to the detected values on request', async () => {
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
      commit: { name: null, email: null, signoff: 'auto' },
    }));
  });
});
