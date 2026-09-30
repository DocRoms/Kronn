import { describe, expect, it } from 'vitest';
import type { RepositoryResourceComparison, RepositoryResourceFileContent } from '../../types/generated';
import { contentModes, filesOn, isMarkdownContent, splitFrontMatter } from '../repositoryResourceContent';

const file = (path: string, repository?: string, kronn?: string): RepositoryResourceFileContent => ({
  path, repository, kronn, truncated: false,
});
const comparison = (
  files: RepositoryResourceFileContent[],
  diffs: string[] = [],
): RepositoryResourceComparison => ({
  files,
  file_diffs: diffs.map(path => ({ path, diff: '@@ -1 +1 @@\n-a\n+b\n' })),
  field_diff: [],
});

describe('which modes a resource sheet offers, and which it opens on', () => {
  it('opens on the repository when the resource only exists there', () => {
    const modes = contentModes('repository_only', comparison([file('a.md', 'text')]));

    expect(modes.preselected).toBe('repository');
    expect(modes.disabled).toEqual({ kronn: 'absent_kronn', diff: 'absent_kronn' });
    expect(modes.newer).toBeNull();
  });

  it('opens on Kronn when the resource only exists there', () => {
    const modes = contentModes('kronn_only', comparison([file('a.md', undefined, 'text')]));

    expect(modes.preselected).toBe('kronn');
    expect(modes.disabled).toEqual({ repository: 'absent_repository', diff: 'absent_repository' });
    expect(modes.newer).toBeNull();
  });

  it('opens on Kronn, with the diff off as "identical", when both sides agree', () => {
    const modes = contentModes('up_to_date', comparison([file('a.md', 'same', 'same')]));

    expect(modes.preselected).toBe('kronn');
    expect(modes.disabled).toEqual({ diff: 'identical' });
    expect(modes.newer).toBeNull();
  });

  it('opens on the diff when the two sides differ and both moved', () => {
    const modes = contentModes('conflict', comparison([file('a.md', 'one', 'two')], ['a.md']));

    expect(modes.preselected).toBe('diff');
    expect(modes.disabled).toEqual({});
    expect(modes.newer).toBeNull();
  });

  it('opens on the diff and says the repository is newer when only it moved', () => {
    const modes = contentModes('repository_newer', comparison([file('a.md', 'one', 'two')], ['a.md']));

    expect(modes.preselected).toBe('diff');
    expect(modes.newer).toBe('repository');
  });

  it('opens on the diff and says Kronn is newer when only it moved', () => {
    const modes = contentModes('kronn_newer', comparison([file('a.md', 'one', 'two')], ['a.md']));

    expect(modes.preselected).toBe('diff');
    expect(modes.newer).toBe('kronn');
  });

  it('never calls a side newer when the texts are the same', () => {
    expect(contentModes('repository_newer', comparison([file('a.md', 'same', 'same')])).newer).toBeNull();
  });

  it('reads a difference from the texts even when no diff came with them', () => {
    const modes = contentModes('approval_required', comparison([file('a.yaml', 'one', 'two')]));

    expect(modes.preselected).toBe('diff');
    expect(modes.newer).toBeNull();
  });

  it('reads a difference from the diff when only one file of several is on both sides', () => {
    const modes = contentModes('conflict', comparison([
      file('page/index.html', 'same', 'same'),
      file('page/artifact.yaml', undefined, 'only in Kronn'),
    ], ['page/artifact.yaml']));

    expect(modes.preselected).toBe('diff');
    expect(modes.disabled).toEqual({});
  });

  it('offers nothing when neither side has a file, and does not fail on a bare answer', () => {
    expect(contentModes('kronn_only', comparison([])).disabled).toEqual({
      repository: 'absent_repository', kronn: 'absent_kronn', diff: 'absent_repository',
    });
    const bare = { file_diffs: [], field_diff: [] } as unknown as RepositoryResourceComparison;
    expect(contentModes('up_to_date', bare).preselected).toBe('kronn');
  });

  it('keeps the files that hold a text on the side asked for, in the order the backend gave', () => {
    const all = comparison([
      file('page/index.html', 'html', 'html'),
      file('page/artifact.yaml', undefined, 'yaml'),
    ]);

    expect(filesOn(all, 'repository').map(item => item.path)).toEqual(['page/index.html']);
    expect(filesOn(all, 'kronn').map(item => item.path)).toEqual(['page/index.html', 'page/artifact.yaml']);
  });
});

describe('the text of a skill', () => {
  it('splits the header from the body that gets rendered', () => {
    const { header, body } = splitFrontMatter('---\nname: review\ndescription: Look.\n---\n\n# Review\n\nBody.\n');

    expect(header).toBe('name: review\ndescription: Look.');
    expect(body).toBe('# Review\n\nBody.\n');
  });

  it('understands Windows line endings and a file without a header', () => {
    expect(splitFrontMatter('---\r\nname: a\r\n---\r\nBody').body).toBe('Body');
    expect(splitFrontMatter('# Just text\n')).toEqual({ header: null, body: '# Just text\n' });
    expect(splitFrontMatter('Not a header\n---\nrule')).toEqual({ header: null, body: 'Not a header\n---\nrule' });
  });

  it('renders as Markdown a skill file only', () => {
    expect(isMarkdownContent('skill', '.agents/skills/review/SKILL.md')).toBe(true);
    expect(isMarkdownContent('quick_prompt', 'kronn/prompts/review.md')).toBe(false);
    expect(isMarkdownContent('workflow', 'kronn/workflows/nightly.yaml')).toBe(false);
    expect(isMarkdownContent('artifact', 'kronn/artifacts/board/index.html')).toBe(false);
  });
});
