// Resolve a link found in a project doc into an in-app target (KT-1200).
// Kronn has no router: a relative `<a href>` would reload the whole app, so
// every doc link is classified here and the viewer acts on the result.

import { createContext } from 'react';
import type { AiFileNode } from '../types/generated';

export type DocLinkTarget =
  | { kind: 'external'; href: string }
  | { kind: 'anchor'; anchor: string }
  | { kind: 'doc'; path: string; anchor: string | null }
  | { kind: 'code'; path: string; line: number | null }
  | { kind: 'outside'; href: string }
  | { kind: 'missing'; path: string }
  | { kind: 'inert' };

/** The doc files the viewer can open, and the docs folders they live in. */
export interface DocIndex {
  files: ReadonlySet<string>;
  roots: readonly string[];
}

export function buildDocIndex(tree: AiFileNode[]): DocIndex {
  const files = new Set<string>();
  const walk = (nodes: AiFileNode[]) => {
    for (const node of nodes) {
      if (node.is_dir) walk(node.children ?? []);
      else files.add(node.path);
    }
  };
  walk(tree);
  return { files, roots: tree.filter(node => node.is_dir).map(node => node.path) };
}

const OUTSIDE = Symbol('outside');

/** Resolve `.`/`..` segments; OUTSIDE when the path climbs above the project root. */
function normalize(path: string): string | typeof OUTSIDE {
  const out: string[] = [];
  for (const part of path.split('/')) {
    if (part === '' || part === '.') continue;
    if (part === '..') {
      if (out.length === 0) return OUTSIDE;
      out.pop();
    } else {
      out.push(part);
    }
  }
  return out.join('/');
}

function safeDecode(value: string): string {
  try { return decodeURIComponent(value); } catch { return value; }
}

const isMarkdownPath = (path: string) => /\.(md|markdown|mdx)$/i.test(path);

export function resolveDocLink(href: string | undefined | null, currentPath: string, index: DocIndex): DocLinkTarget {
  const raw = (href ?? '').trim();
  if (!raw) return { kind: 'inert' };
  if (/^(https?:|mailto:)/i.test(raw)) return { kind: 'external', href: raw };
  // Any other scheme or a protocol-relative URL is neither a repo path nor a safe web link.
  if (/^[a-z][a-z0-9+.-]*:/i.test(raw) || raw.startsWith('//')) return { kind: 'inert' };

  const hashAt = raw.indexOf('#');
  const fragment = hashAt >= 0 ? safeDecode(raw.slice(hashAt + 1)) : '';
  let pathPart = safeDecode((hashAt >= 0 ? raw.slice(0, hashAt) : raw).replace(/\?.*$/, ''));
  if (!pathPart) return fragment ? { kind: 'anchor', anchor: fragment } : { kind: 'inert' };

  let line: number | null = null;
  const lineSuffix = /^(.*?):(\d+)(?:-\d+)?$/.exec(pathPart);
  if (lineSuffix) {
    pathPart = lineSuffix[1];
    line = Number(lineSuffix[2]);
  }
  const githubLine = /^L(\d+)/.exec(fragment);
  if (line === null && githubLine) line = Number(githubLine[1]);

  const fromRoot = pathPart.startsWith('/');
  const explicitRelative = /^\.\.?\//.test(pathPart);
  const baseDir = currentPath.includes('/') ? currentPath.slice(0, currentPath.lastIndexOf('/')) : '';
  const relative = fromRoot ? normalize(pathPart) : normalize(baseDir ? `${baseDir}/${pathPart}` : pathPart);
  // A bare `backend/src/x.rs` is ambiguous: markdown reads it relative, citations read it from the root.
  const rootCandidate = !fromRoot && !explicitRelative ? normalize(pathPart) : null;
  const candidates = [relative, rootCandidate].filter((c): c is string => typeof c === 'string' && c !== '');

  if (relative === OUTSIDE) return { kind: 'outside', href: raw };
  if (candidates.length === 0) return { kind: 'inert' };

  if (isMarkdownPath(pathPart) && line === null) {
    const known = candidates.find(c => index.files.has(c));
    if (known) return { kind: 'doc', path: known, anchor: fragment || null };
    const primary = candidates[0];
    if (index.roots.some(root => primary.startsWith(`${root}/`))) return { kind: 'missing', path: primary };
    // A markdown file outside the docs folders cannot be read by the doc viewer; show its source.
    return { kind: 'code', path: primary, line: null };
  }

  const codePath = typeof rootCandidate === 'string' && rootCandidate && pathPart.includes('/') ? rootCandidate : candidates[0];
  return { kind: 'code', path: codePath, line };
}

/** GitHub-style heading slug, shared by heading ids and anchor lookup. */
export function slugifyHeading(text: string): string {
  return text
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s_-]/gu, '')
    .replace(/\s/g, '-');
}

/** Find the heading an anchor names, by its id or by the slug of the raw anchor. */
export function findDocHeading(container: HTMLElement, anchor: string): HTMLElement | null {
  const wanted = new Set([anchor, slugifyHeading(anchor)]);
  for (const el of Array.from(container.querySelectorAll<HTMLElement>('[id]'))) {
    if (wanted.has(el.id)) return el;
  }
  return null;
}

/** What a rendered doc needs to turn its links into in-app navigation. */
export interface DocLinkContextValue {
  currentPath: string;
  index: DocIndex;
  onNavigate: (target: DocLinkTarget) => void;
}

export const DocLinkContext = createContext<DocLinkContextValue | null>(null);
