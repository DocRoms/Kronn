import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { RepositoryResourceComparison, RepositoryResourceFileContent } from '../../types/generated';
import { buildRows, type ResourceRow } from '../../lib/repositoryResourceRows';
import { listing, resource, skill } from './repositoryResourceFixtures';

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: Array<string | number>) => (args.length > 0 ? `${key}:${args.join('|')}` : key),
  }),
}));

import { RepositoryResourceContent } from '../RepositoryResourceContent';

const R = 'projects.repositoryResources.';
const SKILL_PATH = '.agents/skills/review/SKILL.md';
const SKILL_TEXT = '---\nname: review\ndescription: Review a diff.\n---\n\n# Review carefully\n\nRead **every** line.\n';

const file = (path: string, repository?: string, kronn?: string, truncated = false): RepositoryResourceFileContent => ({
  path, repository, kronn, truncated,
});
const answer = (files: RepositoryResourceFileContent[], diffs: string[] = []): RepositoryResourceComparison => ({
  files,
  file_diffs: diffs.map(path => ({ path, diff: '--- repository\n+++ Kronn\n@@ -1 +1 @@\n-old line\n+new line\n' })),
  field_diff: [],
});

const skillRow = (status: 'repository_only' | 'kronn_only' | 'up_to_date' | 'repository_newer' | 'kronn_newer' | 'conflict'): ResourceRow => (
  buildRows(listing({ skills_present: [skill({ id: 'review', name: 'Review', status })] })).skills[0]
);
const workflowRow = (status: 'conflict'): ResourceRow => (
  buildRows(listing({ resources: [resource({ id: 'wf-1', name: 'Nightly', kind: 'workflow', status })] })).automation[0]
);

function show(row: ResourceRow, comparison: RepositoryResourceComparison) {
  const diffs = comparison.file_diffs;
  return render(<RepositoryResourceContent row={row} comparison={comparison} diffs={diffs} />);
}
const mode = (name: 'repository' | 'kronn' | 'diff') => screen.getByRole('button', { name: `${R}content.mode.${name}` });
const pressed = () => ['repository', 'kronn', 'diff'].filter(name => mode(name as 'diff').getAttribute('aria-pressed') === 'true');

describe('the content of a resource, in the mode that matters', () => {
  it('opens on the repository when the resource only exists there, and says why Kronn and the diff are off', () => {
    show(skillRow('repository_only'), answer([file(SKILL_PATH, SKILL_TEXT)]));

    expect(pressed()).toEqual(['repository']);
    expect(mode('kronn')).toBeDisabled();
    expect(mode('kronn').closest('span')).toHaveAttribute('title', `${R}content.disabled.absent_kronn`);
    expect(mode('diff')).toBeDisabled();
    expect(mode('repository')).toBeEnabled();
    expect(screen.getByRole('heading', { name: 'Review carefully' })).toBeInTheDocument();
  });

  it('opens on Kronn when the resource only exists there, and says why the repository and the diff are off', () => {
    show(skillRow('kronn_only'), answer([file(SKILL_PATH, undefined, SKILL_TEXT)]));

    expect(pressed()).toEqual(['kronn']);
    expect(mode('repository')).toBeDisabled();
    expect(mode('repository').closest('span')).toHaveAttribute('title', `${R}content.disabled.absent_repository`);
    expect(mode('diff')).toBeDisabled();
    expect(screen.getByRole('heading', { name: 'Review carefully' })).toBeInTheDocument();
  });

  it('opens on Kronn when both sides are identical, and turns the diff off as "identical"', () => {
    show(skillRow('up_to_date'), answer([file(SKILL_PATH, SKILL_TEXT, SKILL_TEXT)]));

    expect(pressed()).toEqual(['kronn']);
    expect(mode('diff')).toBeDisabled();
    expect(mode('diff').closest('span')).toHaveAttribute('title', `${R}content.disabled.identical`);
    expect(mode('repository')).toBeEnabled();
    expect(screen.queryByTestId('content-newer')).not.toBeInTheDocument();
    expect(screen.getByText(`${R}compare.noDiff`)).toBeInTheDocument();
  });

  it('opens on the diff when the two sides differ, with no side called newer', () => {
    show(workflowRow('conflict'), answer([file('kronn/workflows/nightly.yaml', 'one', 'two')], ['kronn/workflows/nightly.yaml']));

    expect(pressed()).toEqual(['diff']);
    expect(mode('repository')).toBeEnabled();
    expect(mode('kronn')).toBeEnabled();
    expect(screen.getByText('-old line')).toBeInTheDocument();
    expect(screen.queryByTestId('content-newer')).not.toBeInTheDocument();
  });

  it.each([
    ['repository_newer', 'repository'],
    ['kronn_newer', 'kronn'],
  ] as const)('opens on the diff and says which side is newer when only one moved (%s)', (status, side) => {
    show(skillRow(status), answer([file(SKILL_PATH, 'one', 'two')], [SKILL_PATH]));

    expect(pressed()).toEqual(['diff']);
    expect(screen.getByTestId('content-newer')).toHaveTextContent(`${R}content.newer.${side}`);
    expect(screen.getByTestId('content-newer')).toHaveAttribute('data-side', side);
  });

  it('shows the diff with the same component as the Compare sheet: unified, then side by side', () => {
    show(workflowRow('conflict'), answer([file('a.yaml', 'one', 'two')], ['a.yaml']));

    expect(document.querySelector('.rr-diff[data-mode="unified"]')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: `${R}compare.side` }));
    expect(document.querySelector('.rr-diff[data-mode="side"]')).toBeInTheDocument();
  });

  it('switches between the two texts, and a disabled mode does nothing when clicked', () => {
    show(workflowRow('conflict'), answer([file('a.yaml', 'repository text', 'kronn text')], ['a.yaml']));

    fireEvent.click(mode('repository'));
    expect(pressed()).toEqual(['repository']);
    expect(screen.getByTestId('content-source')).toHaveTextContent('repository text');
    fireEvent.click(mode('kronn'));
    expect(screen.getByTestId('content-source')).toHaveTextContent('kronn text');
    fireEvent.click(mode('diff'));
    expect(screen.getByText('-old line')).toBeInTheDocument();
  });

  it('lets a skill be read rendered or as its source, and every other kind as text only', () => {
    const view = show(skillRow('up_to_date'), answer([file(SKILL_PATH, SKILL_TEXT, SKILL_TEXT)]));

    const rendered = within(screen.getByTestId('content-rendered'));
    expect(rendered.getByRole('heading', { name: 'Review carefully' })).toBeInTheDocument();
    expect(rendered.getByText('every').tagName).toBe('STRONG');
    fireEvent.click(screen.getByRole('button', { name: `${R}content.source` }));
    expect(screen.getByTestId('content-source')).toHaveTextContent('# Review carefully');
    expect(screen.getByTestId('content-source')).toHaveTextContent('name: review');
    fireEvent.click(screen.getByRole('button', { name: `${R}content.rendered` }));
    expect(screen.getByTestId('content-rendered')).toBeInTheDocument();
    view.unmount();

    show(workflowRow('conflict'), answer([file('kronn/workflows/nightly.yaml', '# not a title', '# not a title')]));
    expect(screen.queryByRole('button', { name: `${R}content.source` })).not.toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'not a title' })).not.toBeInTheDocument();
    expect(screen.getByTestId('content-source')).toHaveTextContent('# not a title');
  });

  it('shows the text as sent: a secret stays the reference Kronn masked it to', () => {
    show(workflowRow('conflict'), answer([
      file('kronn/workflows/nightly.yaml', undefined, '"token": "secret://KRONN_WORKFLOW_TOKEN"'),
    ]));

    expect(screen.getByTestId('content-source')).toHaveTextContent('secret://KRONN_WORKFLOW_TOKEN');
  });

  it('renders no HTML, follows no link into the skill folder and shows an image as its caption', () => {
    const hostile = [
      '<script>window.leak = true</script><img src=x onerror="window.leak = true">',
      '[folder](references/guide.md) and [web](https://example.com/docs) and [js](javascript:alert(1))',
      '![diagram](assets/diagram.png)',
    ].join('\n\n');
    show(skillRow('kronn_only'), answer([file(SKILL_PATH, undefined, hostile)]));

    const rendered = within(screen.getByTestId('content-rendered'));
    expect(document.querySelector('script, img')).not.toBeInTheDocument();
    expect(rendered.getByText('folder').closest('a')).toBeNull();
    expect(rendered.getByText('js').closest('a')).toBeNull();
    const web = rendered.getByRole('link', { name: 'web' });
    expect(web).toHaveAttribute('href', 'https://example.com/docs');
    expect(web).toHaveAttribute('target', '_blank');
    expect(web.getAttribute('rel')).toContain('noopener');
    expect(rendered.getByText('diagram')).toBeInTheDocument();
  });

  it('says when a long file was cut', () => {
    show(workflowRow('conflict'), answer([file('a.yaml', 'one', 'two', true)]));
    fireEvent.click(mode('kronn'));

    expect(screen.getByText(`${R}content.truncated`)).toBeInTheDocument();
  });

  it('says so when the resource has no file on either side', () => {
    show(skillRow('kronn_only'), answer([]));

    expect(screen.getByText(`${R}content.empty`)).toBeInTheDocument();
    ['repository', 'kronn', 'diff'].forEach(name => expect(mode(name as 'diff')).toBeDisabled());
  });

  it('opens on the preselected mode again for another resource', () => {
    const view = show(skillRow('up_to_date'), answer([file(SKILL_PATH, 'one', 'one')]));
    fireEvent.click(mode('repository'));
    expect(pressed()).toEqual(['repository']);

    view.rerender(
      <RepositoryResourceContent
        row={workflowRow('conflict')}
        comparison={answer([file('a.yaml', 'one', 'two')], ['a.yaml'])}
        diffs={[{ path: 'a.yaml', diff: '@@ -1 +1 @@\n-old line\n+new line\n' }]}
      />,
    );
    expect(pressed()).toEqual(['diff']);
  });
});
