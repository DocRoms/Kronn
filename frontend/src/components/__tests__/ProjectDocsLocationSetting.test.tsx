import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';
import { ApiRequestError } from '../../lib/apiRequestError';

const { docsLocationMock, setDocsLocationMock } = vi.hoisted(() => ({
  docsLocationMock: vi.fn(),
  setDocsLocationMock: vi.fn(),
}));

vi.mock('../../lib/api', () => buildApiMock({
  projects: { docsLocation: docsLocationMock as never, setDocsLocation: setDocsLocationMock as never },
}));

import { ProjectDocsLocationSetting } from '../ProjectDocsLocationSetting';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

const detected = { override_dir: null, default_dir: null, effective_dir: 'docs', source: 'detected', refused: null };

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('ProjectDocsLocationSetting (KT-263)', () => {
  it('shows the folder in use and saves a normalized override', async () => {
    docsLocationMock.mockResolvedValue(detected);
    setDocsLocationMock.mockResolvedValue({
      override_dir: '.ai', default_dir: null, effective_dir: '.ai', source: 'project_override', refused: null,
    });
    render(<ProjectDocsLocationSetting projectId="p1" t={t} />);

    await waitFor(() => expect(screen.getByTestId('project-docs-location-effective'))
      .toHaveTextContent('projects.docsLocation.source.detected:docs/'));
    expect(screen.getByTestId('project-docs-location-save')).toBeDisabled();

    fireEvent.change(screen.getByTestId('project-docs-location-input'), { target: { value: ' ./.ai/ ' } });
    fireEvent.click(screen.getByTestId('project-docs-location-save'));
    await waitFor(() => expect(screen.getByTestId('project-docs-location-effective'))
      .toHaveTextContent('projects.docsLocation.source.project_override:.ai/'));
    expect(setDocsLocationMock).toHaveBeenCalledWith('p1', './.ai/');
    expect(screen.getByTestId('project-docs-location-input')).toHaveValue('.ai');
    expect(screen.getByTestId('project-docs-location-reset')).toBeInTheDocument();
  });

  it('clears the override back to the detected folder', async () => {
    docsLocationMock.mockResolvedValue({
      override_dir: '.ai', default_dir: 'handbook', effective_dir: '.ai', source: 'project_override', refused: null,
    });
    setDocsLocationMock.mockResolvedValue({
      override_dir: null, default_dir: 'handbook', effective_dir: 'docs', source: 'detected', refused: null,
    });
    render(<ProjectDocsLocationSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-docs-location-reset')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('project-docs-location-reset'));
    await waitFor(() => expect(screen.getByTestId('project-docs-location-effective'))
      .toHaveTextContent('projects.docsLocation.source.detected:docs/'));
    expect(setDocsLocationMock).toHaveBeenCalledWith('p1', null);
    expect(screen.getByTestId('project-docs-location-input')).toHaveAttribute('placeholder', 'docs');
  });

  it('surfaces the server refusal and keeps the saved value', async () => {
    docsLocationMock.mockResolvedValue(detected);
    setDocsLocationMock.mockRejectedValue(new ApiRequestError('`../x` contains `..`', 'validation'));
    render(<ProjectDocsLocationSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-docs-location-input')).not.toBeDisabled());
    fireEvent.change(screen.getByTestId('project-docs-location-input'), { target: { value: '../x' } });
    fireEvent.click(screen.getByTestId('project-docs-location-save'));
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('contains'));
    expect(screen.getByTestId('project-docs-location-effective'))
      .toHaveTextContent('projects.docsLocation.source.detected:docs/');
  });

  it('says when a configured folder is refused at use time', async () => {
    docsLocationMock.mockResolvedValue({
      override_dir: 'ctx', default_dir: null, effective_dir: null, source: 'project_override',
      refused: 'AI doc folder `ctx` refused: ctx is a symlink',
    });
    render(<ProjectDocsLocationSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-docs-location-refused'))
      .toHaveTextContent('symlink'));
    expect(screen.getByTestId('project-docs-location-effective')).toHaveTextContent('projects.docsLocation.none');
  });
});
