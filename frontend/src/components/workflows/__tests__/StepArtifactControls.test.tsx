// KT-1990 — in the run view, each execution of a step (main and shadows) makes
// an Artifact from the HTML its output holds and then opens it; a step with two
// renders or more opens them in a mosaic, main first, then the shadows in start
// order. An output with no HTML or a cut document says so and offers nothing.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ShadowExecution, StepShadowComparison } from '../../../types/generated';

vi.mock('../../../lib/api', async () => {
  const { buildApiMock } = await import('../../../test/apiMock');
  return buildApiMock();
});
vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string, ...args: (string | number)[]) => (args.length ? `${key}:${args.join(',')}` : key) }),
}));

import { workflows as workflowsApi } from '../../../lib/api';
import { StepShadowCompare, StepShadowCompareView } from '../StepShadowCompare';

const exec = (shadowId: string | null, extra: Partial<ShadowExecution> = {}): ShadowExecution => ({
  shadow_id: shadowId ?? undefined,
  label: shadowId ?? '',
  status: 'success',
  identity: { agent: 'Ollama', model: 'qwen', prompt: 'inline:abc', skills: [] },
  output: '```html\n<html><body>game</body></html>\n```',
  tokens_used: 10,
  duration_ms: 100,
  model: shadowId ? `model-${shadowId}` : 'opus',
  html: 'complete',
  ...extra,
});

const comparison = (shadows: ShadowExecution[], main: Partial<ShadowExecution> = {}): StepShadowComparison => ({
  step_name: 'jeu',
  result_index: 1,
  main: exec(null, main),
  shadows,
});

const RUN = { workflowId: 'wf-1', runId: 'run-1' };

/** A tab the click opens at once, then sends where the request says. */
function fakeTab() {
  const tab = { opener: {} as unknown, location: { href: '' }, close: vi.fn() };
  const open = vi.spyOn(window, 'open').mockImplementation(() => tab as unknown as Window);
  return { tab, open };
}

beforeEach(() => { vi.clearAllMocks(); });
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe('Make an Artifact, per execution', () => {
  it('makes the Artifact of one execution, opens it in the standalone tab, then offers to open it', async () => {
    const { tab, open } = fakeTab();
    vi.mocked(workflowsApi.makeStepArtifact).mockResolvedValue({ page_id: 'page-qwen', created: true });
    render(<StepShadowCompareView steps={[comparison([exec('qwen', { label: 'Qwen local' })])]} run={RUN} />);

    fireEvent.click(screen.getByTestId('wf-shadow-exec-jeu-qwen-artifact-make'));

    // The tab opens inside the click, before the request answers.
    expect(open).toHaveBeenCalledWith('', '_blank');
    await waitFor(() => expect(workflowsApi.makeStepArtifact).toHaveBeenCalledWith('wf-1', 'run-1', 1, 'qwen'));
    await waitFor(() => expect(tab.location.href).toMatch(/\/standalone\/pages\/page-qwen$/));
    expect(tab.opener).toBeNull();
    const link = await screen.findByTestId('wf-shadow-exec-jeu-qwen-artifact-open');
    expect(link).toHaveTextContent('wf.shadow.artifact.open');
    expect(link.getAttribute('href')).toMatch(/\/standalone\/pages\/page-qwen$/);
    expect(link).toHaveAttribute('target', '_blank');
    // The main execution keeps its own button, and asks for the main one.
    fireEvent.click(screen.getByTestId('wf-shadow-exec-jeu-main-artifact-make'));
    await waitFor(() => expect(workflowsApi.makeStepArtifact).toHaveBeenLastCalledWith('wf-1', 'run-1', 1, null));
  });

  it('opens the Artifact already made after a reload, never making a second one', () => {
    render(<StepShadowCompareView steps={[comparison([exec('qwen', { artifact_id: 'page-old' })])]} run={RUN} />);
    const link = screen.getByTestId('wf-shadow-exec-jeu-qwen-artifact-open');
    expect(link.getAttribute('href')).toMatch(/\/standalone\/pages\/page-old$/);
    expect(screen.queryByTestId('wf-shadow-exec-jeu-qwen-artifact-make')).toBeNull();
  });

  // KT-1999 — what the server says wins once the comparison is read again.
  it('offers to make the Artifact again when a re-read says it no longer exists', async () => {
    fakeTab();
    vi.mocked(workflowsApi.makeStepArtifact).mockResolvedValue({ page_id: 'page-qwen', created: true });
    const { rerender } = render(<StepShadowCompareView steps={[comparison([exec('qwen')])]} run={RUN} />);
    fireEvent.click(screen.getByTestId('wf-shadow-exec-jeu-qwen-artifact-make'));
    expect(await screen.findByTestId('wf-shadow-exec-jeu-qwen-artifact-open')).toBeInTheDocument();

    // The Artifact was deleted elsewhere: the next read carries none, as the
    // server writes it (`artifact_id: null`).
    const none = { artifact_id: null } as unknown as Partial<ShadowExecution>;
    rerender(<StepShadowCompareView steps={[comparison([exec('qwen', none)])]} run={RUN} />);
    expect(screen.getByTestId('wf-shadow-exec-jeu-qwen-artifact-make')).toBeInTheDocument();
    expect(screen.queryByTestId('wf-shadow-exec-jeu-qwen-artifact-open')).toBeNull();

    // A click made since that read shows at once, without waiting for another read.
    vi.mocked(workflowsApi.makeStepArtifact).mockResolvedValue({ page_id: 'page-qwen-2', created: true });
    fireEvent.click(screen.getByTestId('wf-shadow-exec-jeu-qwen-artifact-make'));
    const link = await screen.findByTestId('wf-shadow-exec-jeu-qwen-artifact-open');
    expect(link.getAttribute('href')).toMatch(/\/standalone\/pages\/page-qwen-2$/);
  });

  it('keeps an Artifact made here while the comparison is not read again', async () => {
    fakeTab();
    vi.mocked(workflowsApi.makeStepArtifact).mockResolvedValue({ page_id: 'page-qwen', created: true });
    const steps = [comparison([exec('qwen')])];
    const { rerender } = render(<StepShadowCompareView steps={steps} run={RUN} />);
    fireEvent.click(screen.getByTestId('wf-shadow-exec-jeu-qwen-artifact-make'));
    await screen.findByTestId('wf-shadow-exec-jeu-qwen-artifact-open');
    // A render with the same read keeps it.
    rerender(<StepShadowCompareView steps={steps} run={RUN} />);
    expect(screen.getByTestId('wf-shadow-exec-jeu-qwen-artifact-open').getAttribute('href')).toMatch(/page-qwen$/);
  });

  it('says when an output holds no HTML, or a cut one, and offers nothing', () => {
    render(<StepShadowCompareView steps={[comparison([
      exec('plain', { html: 'none', output: 'Three findings.' }),
      exec('cut', { html: 'cut_unclosed_fence' }),
      exec('tag', { html: 'cut_no_closing_tag' }),
      exec('stored', { html: 'cut_output' }),
      exec('running', { html: 'none', status: 'running', output: '' }),
    ])]} run={RUN} />);
    const reason = (id: string) => screen.getByTestId(`wf-shadow-exec-jeu-${id}-artifact-none`);
    expect(reason('plain')).toHaveTextContent('wf.shadow.artifact.none');
    expect(reason('cut')).toHaveTextContent('wf.shadow.artifact.cutUnclosedFence');
    expect(reason('tag')).toHaveTextContent('wf.shadow.artifact.cutNoClosingTag');
    expect(reason('stored')).toHaveTextContent('wf.shadow.artifact.cutOutput');
    for (const id of ['plain', 'cut', 'tag', 'stored', 'running']) {
      expect(screen.queryByTestId(`wf-shadow-exec-jeu-${id}-artifact-make`)).toBeNull();
    }
    // A shadow still running has nothing to judge yet.
    expect(screen.queryByTestId('wf-shadow-exec-jeu-running-artifact-none')).toBeNull();
  });

  it('shows nothing about Artifacts in a step where no execution wrote HTML', () => {
    render(<StepShadowCompareView steps={[comparison(
      [exec('plain', { html: 'none', output: 'Three findings.' }), exec('other', { html: 'none', output: 'Two findings.' })],
      { html: 'none', output: 'A review of the diff.' },
    )]} run={RUN} />);
    for (const id of ['main', 'plain', 'other']) {
      expect(screen.queryByTestId(`wf-shadow-exec-jeu-${id}-artifact-none`)).toBeNull();
      expect(screen.queryByTestId(`wf-shadow-exec-jeu-${id}-artifact-make`)).toBeNull();
    }
    expect(screen.queryByText('wf.shadow.artifact.none')).toBeNull();
  });

  it('says « no HTML » when another execution of the step wrote HTML, even a cut one', () => {
    render(<StepShadowCompareView steps={[comparison(
      [exec('plain', { html: 'none', output: 'Three findings.' }), exec('cut', { html: 'cut_unclosed_fence' })],
      { html: 'none', output: 'A review of the diff.' },
    )]} run={RUN} />);
    expect(screen.getByTestId('wf-shadow-exec-jeu-main-artifact-none')).toHaveTextContent('wf.shadow.artifact.none');
    expect(screen.getByTestId('wf-shadow-exec-jeu-plain-artifact-none')).toHaveTextContent('wf.shadow.artifact.none');
    expect(screen.getByTestId('wf-shadow-exec-jeu-cut-artifact-none')).toHaveTextContent('wf.shadow.artifact.cutUnclosedFence');
  });

  it('shows the refusal and closes the tab when the server refuses', async () => {
    const { tab } = fakeTab();
    vi.mocked(workflowsApi.makeStepArtifact).mockRejectedValue(new Error('The HTML document of this execution is cut'));
    render(<StepShadowCompareView steps={[comparison([exec('qwen')])]} run={RUN} />);
    fireEvent.click(screen.getByTestId('wf-shadow-exec-jeu-qwen-artifact-make'));
    expect(await screen.findByTestId('wf-shadow-exec-jeu-qwen-artifact-error')).toHaveTextContent('is cut');
    expect(tab.close).toHaveBeenCalled();
    expect(screen.queryByTestId('wf-shadow-exec-jeu-qwen-artifact-open')).toBeNull();
  });

  // KT-1999 — an answer for one run is never written in another.
  it('shows no Artifact of the first run on the run the view switched to', async () => {
    fakeTab();
    let answer: (made: { page_id: string; created: boolean }) => void = () => {};
    vi.mocked(workflowsApi.makeStepArtifact).mockImplementation(() => new Promise(resolve => { answer = resolve; }));
    // Each read is a new comparison, as the server sends it.
    vi.mocked(workflowsApi.stepShadows).mockImplementation(async () => [comparison([exec('qwen')])]);
    const { rerender } = render(<StepShadowCompare workflowId="wf-1" runId="run-a" runStatus="Success" />);
    fireEvent.click(await screen.findByTestId('wf-shadow-exec-jeu-qwen-artifact-make'));
    await waitFor(() => expect(workflowsApi.makeStepArtifact).toHaveBeenCalledWith('wf-1', 'run-a', 1, 'qwen'));

    rerender(<StepShadowCompare workflowId="wf-1" runId="run-b" runStatus="Success" />);
    await waitFor(() => expect(workflowsApi.stepShadows).toHaveBeenCalledWith('wf-1', 'run-b'));
    // Run-b's button is there and free before run-a's answer comes: the view
    // was mounted again for run-b, not left waiting on run-a's make.
    await waitFor(() => expect(screen.getByTestId('wf-shadow-exec-jeu-qwen-artifact-make')).toBeEnabled());
    await act(async () => { answer({ page_id: 'page-of-run-a', created: true }); });
    expect(screen.queryByTestId('wf-shadow-exec-jeu-qwen-artifact-open')).toBeNull();
    expect(screen.getByTestId('wf-shadow-exec-jeu-qwen-artifact-make')).toBeEnabled();
  });

  it('keeps a late answer out of the view rendered for another run, even with the same comparison', async () => {
    fakeTab();
    let answer: (made: { page_id: string; created: boolean }) => void = () => {};
    vi.mocked(workflowsApi.makeStepArtifact).mockImplementation(() => new Promise(resolve => { answer = resolve; }));
    const steps = [comparison([exec('qwen')])];
    const { rerender } = render(<StepShadowCompareView steps={steps} run={{ workflowId: 'wf-1', runId: 'run-a' }} />);
    fireEvent.click(screen.getByTestId('wf-shadow-exec-jeu-qwen-artifact-make'));
    await waitFor(() => expect(workflowsApi.makeStepArtifact).toHaveBeenCalled());

    rerender(<StepShadowCompareView steps={steps} run={{ workflowId: 'wf-1', runId: 'run-b' }} />);
    // Not mounted again: the button still waits on run-a's make, so it frees
    // only once that answer has been handled.
    expect(screen.getByTestId('wf-shadow-exec-jeu-qwen-artifact-make')).toBeDisabled();
    await act(async () => { answer({ page_id: 'page-of-run-a', created: true }); });
    await waitFor(() => expect(screen.getByTestId('wf-shadow-exec-jeu-qwen-artifact-make')).toBeEnabled());
    expect(screen.queryByTestId('wf-shadow-exec-jeu-qwen-artifact-open')).toBeNull();
  });

  it('offers nothing when the view has no run to make Artifacts in', () => {
    render(<StepShadowCompareView steps={[comparison([exec('qwen')])]} />);
    expect(screen.queryByTestId('wf-shadow-exec-jeu-qwen-artifact-make')).toBeNull();
    expect(screen.queryByTestId('wf-shadow-mosaic-jeu')).toBeNull();
  });
});

describe('Open the renders in a mosaic, per step', () => {
  it('needs two renders', () => {
    render(<StepShadowCompareView steps={[comparison([exec('plain', { html: 'none' })])]} run={RUN} />);
    expect(screen.queryByTestId('wf-shadow-mosaic-jeu')).toBeNull();
  });

  it('makes the missing Artifacts and opens the mosaic, main first then the shadows in start order', async () => {
    const { tab } = fakeTab();
    vi.mocked(workflowsApi.stepArtifactMosaic).mockResolvedValue({ page_ids: ['p-main', 'p-early', 'p-late'], created: 2 });
    render(<StepShadowCompareView steps={[comparison([
      exec('early'),
      exec('broken', { html: 'cut_output' }),
      exec('late', { artifact_id: 'p-late' }),
    ])]} run={RUN} />);

    fireEvent.click(screen.getByText('wf.shadow.artifact.mosaic'));

    await waitFor(() => expect(workflowsApi.stepArtifactMosaic).toHaveBeenCalledWith('wf-1', 'run-1', 1));
    await waitFor(() => expect(tab.location.href).toContain('/standalone/pages/mosaic?'));
    const query = new URLSearchParams(tab.location.href.split('?')[1]);
    expect(query.getAll('page')).toEqual(['p-main', 'p-early', 'p-late']);
    // Each execution now opens its own Artifact.
    expect(screen.getByTestId('wf-shadow-exec-jeu-main-artifact-open').getAttribute('href')).toMatch(/p-main$/);
    expect(screen.getByTestId('wf-shadow-exec-jeu-early-artifact-open').getAttribute('href')).toMatch(/p-early$/);
    expect(screen.queryByTestId('wf-shadow-exec-jeu-broken-artifact-open')).toBeNull();
  });
});
