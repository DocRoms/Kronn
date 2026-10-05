import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { GithubScope, ProjectGithubConnection } from '../../../types/generated';

const { getMock, setMock, refreshMock } = vi.hoisted(() => ({
  getMock: vi.fn(),
  setMock: vi.fn(),
  refreshMock: vi.fn(),
}));

vi.mock('../../../lib/api', () => buildApiMock({
  projects: {
    githubConnection: getMock as never,
    setGithubConnection: setMock as never,
    refreshGithubScope: refreshMock as never,
  },
}));

import { GithubConnectionChip, ProjectGithubRow } from '../ProjectGithubRow';
import { GITHUB_NOTICE_DISMISSED_KEY, githubScopeSummary } from '../githubConnection';

const scope = (over: Partial<GithubScope> = {}): GithubScope => ({
  verified: true, token_kind: 'oauth', login: 'octo', scopes: ['repo', 'workflow'], repositories: [],
  repositories_truncated: false, broad: true, reason: null, checked_at: '2026-10-05T10:00:00Z', ...over,
});

const connection = (over: Partial<ProjectGithubConnection> = {}): ProjectGithubConnection => ({
  project_id: 'p1', mode: 'not_connected', state: 'available_but_off', machine_token_available: true,
  machine_token_source: 'gh_cli', on_github: true, scope: null, connected_on_upgrade: false,
  updated_at: null, ...over,
});

const connected = (over: Partial<ProjectGithubConnection> = {}) =>
  connection({ mode: 'gh_login', state: 'connected_gh_login', scope: scope(), ...over });

beforeEach(() => {
  getMock.mockReset();
  setMock.mockReset();
  refreshMock.mockReset();
  localStorage.clear();
});

describe('ProjectGithubRow', () => {
  it('shows the state, its meaning and a connect button when the project is off', async () => {
    getMock.mockResolvedValueOnce(connection());
    render(<ProjectGithubRow projectId="p1" enabled />);
    expect(await screen.findByText('github.meaning.available_but_off')).toBeInTheDocument();
    expect(screen.getAllByText('github.state.available_but_off').length).toBeGreaterThan(0);
    expect(screen.getByRole('button', { name: 'github.connect' })).toBeEnabled();
    expect(screen.getByText('github.limit')).toBeInTheDocument();
  });

  it('loads nothing while the overview is not shown', () => {
    render(<ProjectGithubRow projectId="p1" enabled={false} />);
    expect(getMock).not.toHaveBeenCalled();
  });

  it('confirms the risk before connecting with the gh login', async () => {
    getMock.mockResolvedValueOnce(connection());
    refreshMock.mockResolvedValueOnce(connection({ scope: scope() }));
    setMock.mockResolvedValueOnce(connected());
    render(<ProjectGithubRow projectId="p1" enabled />);
    fireEvent.click(await screen.findByRole('button', { name: 'github.connect' }));

    const dialog = await screen.findByRole('dialog');
    expect(dialog).toHaveTextContent('github.dialog.risk');
    await waitFor(() => expect(screen.getByTestId('github-broad-warning')).toBeInTheDocument());
    expect(refreshMock).toHaveBeenCalledWith('p1');

    fireEvent.click(screen.getByRole('button', { name: 'github.dialog.useGh' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(setMock).toHaveBeenCalledWith('p1', { mode: 'gh_login' });
    expect(screen.getByRole('button', { name: 'github.turnOff' })).toBeInTheDocument();
  });

  it('a double click sends one connect request', async () => {
    getMock.mockResolvedValueOnce(connection({ scope: scope({ broad: false }) }));
    refreshMock.mockResolvedValue(connection({ scope: scope({ broad: false }) }));
    let resolve!: (value: ProjectGithubConnection) => void;
    setMock.mockReturnValueOnce(new Promise(r => { resolve = r; }));
    render(<ProjectGithubRow projectId="p1" enabled />);
    fireEvent.click(await screen.findByRole('button', { name: 'github.connect' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'github.dialog.useGh' })).toBeEnabled());
    const use = screen.getByRole('button', { name: 'github.dialog.useGh' });
    fireEvent.click(use);
    fireEvent.click(use);
    resolve(connected());
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(setMock).toHaveBeenCalledTimes(1);
  });

  it('pastes a token instead, and keeps the dialog open with the error on refusal', async () => {
    getMock.mockResolvedValueOnce(connection({ machine_token_available: false, state: 'not_connected' }));
    refreshMock.mockResolvedValueOnce(connection({ machine_token_available: false, state: 'not_connected' }));
    setMock.mockRejectedValueOnce(new Error('The GitHub token contains characters a GitHub token never has'));
    render(<ProjectGithubRow projectId="p1" enabled />);
    fireEvent.click(await screen.findByRole('button', { name: 'github.connect' }));
    await screen.findByText('github.dialog.noMachineToken');
    expect(screen.queryByRole('button', { name: 'github.dialog.useGh' })).toBeNull();

    const use = screen.getByRole('button', { name: 'github.dialog.useToken' });
    expect(use).toBeDisabled();
    const input = screen.getByLabelText('github.dialog.tokenLabel');
    expect(input).toHaveAttribute('type', 'password');
    fireEvent.change(input, { target: { value: 'github_pat_é' } });
    fireEvent.click(use);
    expect(await screen.findByRole('alert')).toHaveTextContent('never has');
    expect(setMock).toHaveBeenCalledWith('p1', { mode: 'stored_token', token: 'github_pat_é' });
    expect(screen.getByRole('dialog')).toBeInTheDocument();
  });

  it('turning off says running agents keep their token, then turns off', async () => {
    getMock.mockResolvedValueOnce(connected());
    setMock.mockResolvedValueOnce(connection());
    render(<ProjectGithubRow projectId="p1" enabled />);
    fireEvent.click(await screen.findByRole('button', { name: 'github.turnOff' }));
    const dialog = screen.getByRole('alertdialog');
    expect(dialog).toHaveTextContent('github.turnOffNote');
    fireEvent.click(screen.getByRole('button', { name: 'github.turnOffDialog.confirm' }));
    await screen.findByRole('button', { name: 'github.connect' });
    expect(setMock).toHaveBeenCalledWith('p1', { mode: 'not_connected' });
  });

  it('Escape closes the connect dialog', async () => {
    getMock.mockResolvedValueOnce(connection());
    refreshMock.mockResolvedValueOnce(connection());
    render(<ProjectGithubRow projectId="p1" enabled />);
    fireEvent.click(await screen.findByRole('button', { name: 'github.connect' }));
    await screen.findByRole('dialog');
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).toBeNull();
  });
});

describe('upgrade notice', () => {
  it('shows once for a project kept connected by the upgrade, and Keep dismisses it for good', async () => {
    getMock.mockResolvedValue(connected({ connected_on_upgrade: true }));
    const first = render(<ProjectGithubRow projectId="p1" enabled />);
    expect(await screen.findByTestId('github-upgrade-notice')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'github.notice.keep' }));
    expect(screen.queryByTestId('github-upgrade-notice')).toBeNull();
    expect(JSON.parse(localStorage.getItem(GITHUB_NOTICE_DISMISSED_KEY) ?? '[]')).toEqual(['p1']);
    first.unmount();

    render(<ProjectGithubRow projectId="p1" enabled />);
    await screen.findByText('github.meaning.connected_gh_login');
    expect(screen.queryByTestId('github-upgrade-notice')).toBeNull();
  });

  it('Turn off from the notice turns the project off and dismisses the notice', async () => {
    getMock.mockResolvedValueOnce(connected({ connected_on_upgrade: true }));
    setMock.mockResolvedValueOnce(connection());
    render(<ProjectGithubRow projectId="p2" enabled />);
    const notice = await screen.findByTestId('github-upgrade-notice');
    fireEvent.click(notice.querySelector('button') as HTMLButtonElement);
    fireEvent.click(screen.getByRole('button', { name: 'github.turnOffDialog.confirm' }));
    await waitFor(() => expect(screen.queryByTestId('github-upgrade-notice')).toBeNull());
    expect(setMock).toHaveBeenCalledWith('p2', { mode: 'not_connected' });
    expect(localStorage.getItem(GITHUB_NOTICE_DISMISSED_KEY)).toContain('p2');
  });

  it('never shows for a project the user connected', async () => {
    getMock.mockResolvedValueOnce(connected({ connected_on_upgrade: false }));
    render(<ProjectGithubRow projectId="p3" enabled />);
    await screen.findByText('github.meaning.connected_gh_login');
    expect(screen.queryByTestId('github-upgrade-notice')).toBeNull();
  });
});

describe('GithubConnectionChip', () => {
  it('shows the project state in the discussion header', async () => {
    getMock.mockResolvedValueOnce(connected());
    render(<GithubConnectionChip projectId="p1" />);
    const chip = await screen.findByTestId('github-connection-chip');
    expect(chip).toHaveAttribute('data-tone', 'success');
    expect(chip).toHaveTextContent('github.chip.label');
  });

  it('renders nothing when the state cannot be read', async () => {
    getMock.mockRejectedValueOnce(new Error('down'));
    const { container } = render(<GithubConnectionChip projectId="p1" />);
    await waitFor(() => expect(getMock).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });
});

describe('githubScopeSummary', () => {
  const t = (key: string, ...args: (string | number)[]) => [key, ...args].join('|');
  it('names scopes, repositories or why the scope is unknown', () => {
    expect(githubScopeSummary(null, t)).toBe('github.scope.notChecked');
    expect(githubScopeSummary(scope(), t)).toBe('github.scope.scopes|repo, workflow');
    expect(githubScopeSummary(scope({ scopes: [] }), t)).toBe('github.scope.noScopes');
    expect(githubScopeSummary(scope({ token_kind: 'fine_grained', scopes: [], repositories: ['o/a', 'o/b', 'o/c', 'o/d'] }), t))
      .toBe('github.scope.repositories|4|o/a, o/b, o/c…');
    expect(githubScopeSummary(scope({ token_kind: 'fine_grained', repositories: ['o/réseau'], repositories_truncated: true }), t))
      .toBe('github.scope.repositoriesMore|1|o/réseau');
    expect(githubScopeSummary(scope({ verified: false, reason: 'GitHub refused the token (401)' }), t))
      .toBe('github.scope.notVerified|GitHub refused the token (401)');
  });
});
