// KT-954 — a link in a message must lead somewhere. An agent often links a
// file by its path on the machine (`/private/var/…/out.gif`, `docs/AGENTS.md`);
// rendered as a plain <a>, that resolves against Kronn's own origin and opens
// nothing. These helpers tell such links apart and match them to attachments.
import type { ContextFile } from '../types/generated';
import { defaultUrlTransform } from 'react-markdown';

export type LinkKind = 'external' | 'anchor' | 'local' | 'project';

const SCHEME = /^[a-z][a-z0-9+.-]*:/i;
const WINDOWS_DRIVE = /^[a-z]:[\\/]/i;
const FILE_LINE = /^[^:/]+\.[a-z0-9]+:\d+(?::\d+)?$/i;

export function fileLinkUrlTransform(url: string): string {
  const kind = linkKind(url);
  return kind === 'local' || kind === 'project' ? url : defaultUrlTransform(url);
}

export function unavailableFileReason(href: string): string | undefined {
  const match = href.match(/^#kronn-local-file-unavailable-(missing|sensitive|outside|limit|unavailable)$/);
  return match?.[1];
}

export function linkKind(href: string | undefined): LinkKind {
  if (!href) return 'external';
  if (href.startsWith('#')) return 'anchor';
  if (href.startsWith('file:') || href.startsWith('~/') || WINDOWS_DRIVE.test(href)) return 'local';
  if (href.startsWith('/') && !href.startsWith('//')) return 'local';
  if (FILE_LINE.test(href)) return 'project';
  if (SCHEME.test(href) || href.startsWith('//') || href.startsWith('?')) return 'external';
  return 'project';
}

/** The file name a path-like link points at, without a `:line` suffix. */
export function linkFileName(href: string): string {
  let path = href.replace(/^file:\/\/(localhost)?/, '').split(/[?#]/)[0];
  try {
    path = decodeURIComponent(path);
  } catch {
    // A stray `%` is part of the name.
  }
  const name = path.split(/[\\/]/).filter(Boolean).pop() ?? '';
  return name.replace(/:\d+(:\d+)?$/, '');
}

/** Resolve persisted IDs within this discussion's files; legacy paths may use
 * a unique filename. Federation keeps file IDs but remaps discussion IDs. */
export function attachmentForLink(href: string, files: ContextFile[] | undefined): ContextFile | undefined {
  if (!files?.length) return undefined;
  const exact = href.match(/^\/api\/discussions\/([^/]+)\/context-files\/([^/]+)\/content$/);
  if (exact) return files.find(file => file.id === exact[2]);
  const stored = files.filter(file => file.disk_path === href);
  if (stored.length === 1) return stored[0];
  const name = linkFileName(href);
  const matching = name ? files.filter(file => file.filename === name) : [];
  return matching.length === 1 ? matching[0] : undefined;
}

/** The project file and 1-based line a project path names:
 *  `src/lib.rs:440`, `docs/AGENTS.md#L12`, `./README.md`. */
export function projectFileTarget(href: string): { path: string; line: number | null } {
  let path = href.split('?')[0];
  let line: number | null = null;
  const anchor = path.match(/#L(\d+)(?:-L?\d+)?$/i);
  if (anchor) {
    line = Number(anchor[1]);
    path = path.slice(0, anchor.index);
  } else {
    path = path.split('#')[0];
    const suffix = path.match(/:(\d+)(?::\d+)?$/);
    if (suffix) {
      line = Number(suffix[1]);
      path = path.slice(0, suffix.index);
    }
  }
  try {
    path = decodeURIComponent(path);
  } catch {
    // A stray `%` is part of the name.
  }
  return { path: path.replace(/^\.\//, ''), line: line && line > 0 ? line : null };
}
