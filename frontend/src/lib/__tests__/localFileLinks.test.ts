import { describe, expect, it } from 'vitest';
import type { ContextFile } from '../../types/generated';
import { attachmentForLink, fileLinkUrlTransform, linkFileName, linkKind, projectFileTarget } from '../localFileLinks';

describe('linkKind', () => {
  it('keeps web links and anchors as they are', () => {
    expect(linkKind('https://example.com/a.png')).toBe('external');
    expect(linkKind('mailto:a@b.c')).toBe('external');
    expect(linkKind('//cdn.example.com/x.js')).toBe('external');
    expect(linkKind('#kronn-agent-codex')).toBe('anchor');
  });

  it('recognises a path on the machine, however it is written', () => {
    for (const href of [
      '/private/var/folders/T/sax.gif',
      '~/out.zip',
      'file:///tmp/f.txt',
      'C:\\out\\w.png',
      'D:/out/w.png',
    ]) {
      expect(linkKind(href), href).toBe('local');
    }
  });

  it('treats a bare path as a project file', () => {
    expect(linkKind('docs/AGENTS.md')).toBe('project');
    expect(linkKind('backend/src/lib.rs:440')).toBe('project');
  });
});

describe('linkFileName', () => {
  it('keeps the file name only, decoded, without a line suffix', () => {
    expect(linkFileName('/private/var/T/sax-groove-loop.gif')).toBe('sax-groove-loop.gif');
    expect(linkFileName('file:///tmp/my%20sheet.png')).toBe('my sheet.png');
    expect(linkFileName('C:\\out\\w.png')).toBe('w.png');
    expect(linkFileName('backend/src/lib.rs:440')).toBe('lib.rs');
    expect(linkFileName('/tmp/100%.txt')).toBe('100%.txt');
  });
});

describe('attachmentForLink', () => {
  const file = (filename: string) => ({ id: filename, filename }) as unknown as ContextFile;

  it('finds the attachment a local link names', () => {
    const files = [file('saxophonist-lpc-walk.png'), file('saxophonist-lpc-pack.zip')];
    expect(
      attachmentForLink('/private/var/folders/T/saxophonist-lpc-pack.zip', files)?.id,
    ).toBe('saxophonist-lpc-pack.zip');
  });

  it('returns nothing when no attachment carries that name', () => {
    expect(attachmentForLink('/tmp/other.png', [file('a.png')])).toBeUndefined();
    expect(attachmentForLink('/tmp/a.png', undefined)).toBeUndefined();
  });

  it('resolves exact persisted IDs and never guesses between same-name files', () => {
    const files = [
      { ...file('out.png'), id: 'one', discussion_id: 'disc' },
      { ...file('out.png'), id: 'two', discussion_id: 'disc' },
    ];
    expect(attachmentForLink('/tmp/out.png', files)).toBeUndefined();
    expect(attachmentForLink('/api/discussions/disc/context-files/two/content', files)?.id).toBe('two');
    expect(attachmentForLink('/api/discussions/origin/context-files/two/content', files)?.id).toBe('two');
    expect(attachmentForLink('/api/discussions/other/context-files/foreign/content', files)).toBeUndefined();
  });
});

it('preserves supported file destinations without permitting active URL schemes', () => {
  for (const path of ['file:///tmp/out.png', 'C:/out.png', 'README.md:12', './src/a.ts:7']) {
    expect(fileLinkUrlTransform(path)).toBe(path);
  }
  for (const url of ['javascript:alert(1)', 'vbscript:alert(1)', 'data:text/html,evil']) {
    expect(fileLinkUrlTransform(url)).toBe('');
  }
});

describe('projectFileTarget', () => {
  it('splits the path from its line, whatever the notation', () => {
    expect(projectFileTarget('backend/src/lib.rs:440')).toEqual({ path: 'backend/src/lib.rs', line: 440 });
    expect(projectFileTarget('backend/src/lib.rs:440:12')).toEqual({ path: 'backend/src/lib.rs', line: 440 });
    expect(projectFileTarget('docs/AGENTS.md#L12')).toEqual({ path: 'docs/AGENTS.md', line: 12 });
    expect(projectFileTarget('docs/AGENTS.md#L12-L20')).toEqual({ path: 'docs/AGENTS.md', line: 12 });
    expect(projectFileTarget('./README.md')).toEqual({ path: 'README.md', line: null });
    expect(projectFileTarget('docs/my%20notes.md#section')).toEqual({ path: 'docs/my notes.md', line: null });
  });
});
