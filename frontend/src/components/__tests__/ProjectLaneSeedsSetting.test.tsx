import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

const { getMock, setMock, candidatesMock } = vi.hoisted(() => ({
  getMock: vi.fn(),
  setMock: vi.fn(),
  candidatesMock: vi.fn(),
}));

vi.mock('../../lib/api', () => buildApiMock({
  projects: {
    laneSeeds: getMock as never,
    setLaneSeeds: setMock as never,
    laneSeedCandidates: candidatesMock as never,
  },
}));

import { ProjectLaneSeedsSetting } from '../ProjectLaneSeedsSetting';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

const view = (over: Record<string, unknown> = {}) => ({
  seeds: [],
  source: 'none',
  copy_enabled: true,
  adopt_suggested: false,
  adopted_seeds: [],
  needs_confirmation: [],
  secret_findings: [],
  profile_file_written: false,
  ...over,
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('ProjectLaneSeedsSetting (KT-1438)', () => {
  it('lists, adds and removes seeds, sending the whole list each time', async () => {
    getMock.mockResolvedValue(view({ seeds: ['application/public/images'], source: 'project' }));
    setMock
      .mockResolvedValueOnce(view({ seeds: ['application/public/images', 'application/public/build'], source: 'project' }))
      .mockResolvedValueOnce(view({ seeds: ['application/public/build'], source: 'project' }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByText('application/public/images')).toBeInTheDocument());

    fireEvent.change(screen.getByTestId('project-lane-seeds-input'), { target: { value: ' application/public/build ' } });
    fireEvent.click(screen.getByTestId('project-lane-seeds-add'));
    await waitFor(() => expect(screen.getByText('application/public/build')).toBeInTheDocument());
    expect(setMock).toHaveBeenLastCalledWith('p1', ['application/public/images', 'application/public/build'], []);

    fireEvent.click(screen.getByTestId('project-lane-seeds-remove-application/public/images'));
    await waitFor(() => expect(screen.queryByText('application/public/images')).toBeNull());
    expect(setMock).toHaveBeenLastCalledWith('p1', ['application/public/build'], []);
  });

  it('suggests candidates with their sizes and adds one on a click', async () => {
    getMock.mockResolvedValue(view());
    candidatesMock.mockResolvedValue([
      { path: 'application/public/images', size_bytes: 16 * 1024 * 1024, reason: 'gitignored; tracked code mentions `images/`' },
      { path: 'application/public/build', size_bytes: null, reason: 'gitignored; tracked code mentions `build/`' },
    ]);
    setMock.mockResolvedValue(view({ seeds: ['application/public/images'], source: 'project' }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-lane-seeds-empty')).toBeInTheDocument());

    fireEvent.click(screen.getByTestId('project-lane-seeds-suggest'));
    await waitFor(() => expect(screen.getByTestId('project-lane-seeds-candidates')).toBeInTheDocument());
    expect(screen.getByText(/16\.0 MB/)).toBeInTheDocument();
    expect(screen.getByText(/projects\.laneSeeds\.sizeUnknown/)).toBeInTheDocument();

    fireEvent.click(screen.getByTestId('project-lane-seeds-candidate-application/public/images'));
    await waitFor(() => expect(setMock).toHaveBeenCalledWith('p1', ['application/public/images'], []));
    await waitFor(() => expect(screen.getByTestId('project-lane-seeds-candidate-application/public/images')).toBeDisabled());
  });

  it('holds a secret-looking path until the human confirms it', async () => {
    getMock.mockResolvedValue(view({ seeds: ['a/b'], source: 'project' }));
    setMock
      .mockResolvedValueOnce(view({ seeds: ['a/b'], source: 'project', needs_confirmation: [{ path: '.env', files: ['.env'], truncated: false, held: true }] }))
      .mockResolvedValueOnce(view({ seeds: ['a/b', '.env'], source: 'project' }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByText('a/b')).toBeInTheDocument());

    fireEvent.change(screen.getByTestId('project-lane-seeds-input'), { target: { value: '.env' } });
    fireEvent.click(screen.getByTestId('project-lane-seeds-add'));
    const warning = await screen.findByTestId('project-lane-seeds-secret-warning');
    expect(warning).toHaveTextContent('.env');
    expect(setMock).toHaveBeenLastCalledWith('p1', ['a/b', '.env'], []);
    expect(screen.queryByTestId('project-lane-seeds-remove-.env')).toBeNull();

    fireEvent.click(screen.getByTestId('project-lane-seeds-secret-confirm'));
    await waitFor(() => expect(screen.queryByTestId('project-lane-seeds-secret-warning')).toBeNull());
    expect(setMock).toHaveBeenLastCalledWith('p1', ['a/b', '.env'], ['.env']);
    expect(screen.getByText('.env')).toBeInTheDocument();
  });

  it('drops a held secret-looking path when the human cancels', async () => {
    getMock.mockResolvedValue(view());
    setMock.mockResolvedValue(view({ needs_confirmation: [{ path: 'config/server.key', files: ['config/server.key'], truncated: false, held: true }] }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-lane-seeds-empty')).toBeInTheDocument());

    fireEvent.change(screen.getByTestId('project-lane-seeds-input'), { target: { value: 'config/server.key' } });
    fireEvent.click(screen.getByTestId('project-lane-seeds-add'));
    await screen.findByTestId('project-lane-seeds-secret-warning');
    fireEvent.click(screen.getByText('projects.laneSeeds.cancel'));
    expect(screen.queryByTestId('project-lane-seeds-secret-warning')).toBeNull();
    expect(setMock).toHaveBeenCalledTimes(1);
  });

  it('says why a seed was skipped in the last seeding', async () => {
    getMock.mockResolvedValue(view({
      seeds: ['application/public/images', 'application/public/build'],
      source: 'project',
      last_run: {
        execution_id: 'e1',
        at: '2026-11-09T10:00:00Z',
        seeds: [
          { path: 'application/public/images', outcome: 'seeded' },
          { path: 'application/public/build', outcome: 'skipped', detail: 'the main checkout does not have it (generate it there first)' },
        ],
      },
    }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);

    await waitFor(() => expect(screen.getByTestId('project-lane-seeds-last-run')).toBeInTheDocument());
    expect(screen.getByTestId('project-lane-seeds-outcome-application/public/build'))
      .toHaveTextContent('the main checkout does not have it (generate it there first)');
    expect(screen.getByTestId('project-lane-seeds-outcome-application/public/build'))
      .toHaveTextContent('projects.laneSeeds.outcome.skipped');
    expect(screen.getByTestId('project-lane-seeds-outcome-application/public/images'))
      .toHaveTextContent('projects.laneSeeds.outcome.seeded');
  });

  it('shows the project.toml list as a proposal, and adopting it is a save', async () => {
    getMock.mockResolvedValue(view({
      seeds: ['application/public/images'],
      source: 'project',
      profile_seeds: ['application/public/images', 'application/public/build'],
    }));
    setMock.mockResolvedValue(view({
      seeds: ['application/public/images', 'application/public/build'],
      source: 'project',
      profile_seeds: ['application/public/images', 'application/public/build'],
      profile_file_written: true,
    }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);

    const proposal = await screen.findByTestId('project-lane-seeds-profile-proposal');
    expect(proposal).toHaveTextContent('application/public/images, application/public/build');
    fireEvent.click(screen.getByTestId('project-lane-seeds-use-profile'));
    await waitFor(() => expect(screen.getByTestId('project-lane-seeds-file-written')).toBeInTheDocument());
    expect(setMock).toHaveBeenCalledWith('p1', ['application/public/images', 'application/public/build'], []);
    expect(screen.queryByTestId('project-lane-seeds-profile-proposal')).toBeNull();
  });

  it('lists nothing as applied when only the file proposes seeds', async () => {
    getMock.mockResolvedValue(view({
      seeds: [],
      source: 'none',
      profile_seeds: ['application/public/images'],
    }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);

    await waitFor(() => expect(screen.getByTestId('project-lane-seeds-profile-proposal')).toBeInTheDocument());
    expect(screen.getByTestId('project-lane-seeds-empty')).toBeInTheDocument();
    expect(screen.queryByTestId('project-lane-seeds-list')).toBeNull();
  });

  it('lists the secret-looking files found inside a path before it is confirmed', async () => {
    getMock.mockResolvedValue(view());
    setMock
      .mockResolvedValueOnce(view({
        needs_confirmation: [{ path: 'generated', files: ['generated/deep/.env.local', 'generated/cert.pem'], truncated: true, held: true }],
      }))
      .mockResolvedValueOnce(view({
        seeds: ['generated'],
        source: 'project',
        secret_findings: [{ path: 'generated', files: ['generated/deep/.env.local'], truncated: false, held: false }],
      }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-lane-seeds-empty')).toBeInTheDocument());

    fireEvent.change(screen.getByTestId('project-lane-seeds-input'), { target: { value: 'generated' } });
    fireEvent.click(screen.getByTestId('project-lane-seeds-add'));
    const held = await screen.findByTestId('project-lane-seeds-secret-generated');
    expect(held).toHaveTextContent('generated/deep/.env.local, generated/cert.pem');
    expect(held).toHaveTextContent('projects.laneSeeds.secretMore');

    fireEvent.click(screen.getByTestId('project-lane-seeds-secret-confirm'));
    await waitFor(() => expect(setMock).toHaveBeenLastCalledWith('p1', ['generated'], ['generated']));
    // A declared path keeps showing what secret-looking files it holds.
    const standing = await screen.findByTestId('project-lane-seeds-secret-findings');
    expect(standing).toHaveTextContent('generated/deep/.env.local');
  });

  it('shows a declared path held back by an unconfirmed file and confirms it on request', async () => {
    const heldFinding = { path: 'generated', files: ['generated/.env.local', 'generated/id_rsa'], truncated: false, held: true };
    getMock.mockResolvedValue(view({ seeds: ['generated'], source: 'project', secret_findings: [heldFinding] }));
    setMock.mockResolvedValue(view({
      seeds: ['generated'],
      source: 'project',
      secret_findings: [{ ...heldFinding, held: false }],
    }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);

    const standing = await screen.findByTestId('project-lane-seeds-standing-generated');
    expect(standing).toHaveTextContent('generated/.env.local, generated/id_rsa');
    expect(screen.getByTestId('project-lane-seeds-held-generated')).toHaveTextContent('projects.laneSeeds.secretHeld');

    fireEvent.click(screen.getByTestId('project-lane-seeds-confirm-held-generated'));
    await waitFor(() => expect(setMock).toHaveBeenLastCalledWith('p1', ['generated'], ['generated']));
    await waitFor(() => expect(screen.queryByTestId('project-lane-seeds-held-generated')).toBeNull());
    expect(screen.queryByTestId('project-lane-seeds-confirm-held-generated')).toBeNull();
    // Still listed: the files are there, they are just confirmed now.
    expect(screen.getByTestId('project-lane-seeds-standing-generated')).toBeInTheDocument();
  });

  it('does not offer to confirm a declared path whose secret-looking files are all confirmed', async () => {
    getMock.mockResolvedValue(view({
      seeds: ['generated'],
      source: 'project',
      secret_findings: [{ path: 'generated', files: ['generated/.env.local'], truncated: false, held: false }],
    }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);

    await screen.findByTestId('project-lane-seeds-standing-generated');
    expect(screen.queryByTestId('project-lane-seeds-confirm-held-generated')).toBeNull();
  });

  it('says when the declared paths could not be checked for secrets', async () => {
    getMock.mockResolvedValue(view({ seeds: ['gen'], source: 'project', secret_scan_error: 'cannot list `gen/inner`' }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-lane-seeds-scan-error')).toHaveTextContent('cannot list `gen/inner`'));
  });

  it('says copying is off globally, and what the adoption seeds when it is on (DoD 4)', async () => {
    getMock.mockResolvedValue(view({ copy_enabled: false, seeds: ['gen'], source: 'project' }));
    const off = render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);
    await screen.findByTestId('project-lane-seeds-copy-off');
    off.unmount();

    getMock.mockResolvedValue(view({ adopt_suggested: true, source: 'adopted', adopted_seeds: ['gen', 'build'] }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);
    expect(await screen.findByTestId('project-lane-seeds-adopted')).toHaveTextContent('gen, build');
    expect(screen.queryByTestId('project-lane-seeds-copy-off')).toBeNull();
  });

  it('confirming an adopted suggestion that holds a secret declares the adopted list with that file confirmed', async () => {
    getMock.mockResolvedValue(view({
      source: 'adopted',
      adopted_seeds: ['gen'],
      secret_findings: [{ path: 'gen', files: ['gen/.env'], truncated: false, held: true }],
    }));
    setMock.mockResolvedValue(view({ seeds: ['gen'], source: 'project' }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);

    fireEvent.click(await screen.findByTestId('project-lane-seeds-confirm-held-gen'));
    await waitFor(() => expect(setMock).toHaveBeenCalledWith('p1', ['gen'], ['gen']));
  });

  it('shows a file that cannot be kept in step', async () => {
    getMock.mockResolvedValue(view({ profile_error: 'kronn/project.toml is invalid' }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('kronn/project.toml is invalid'));
  });
});
