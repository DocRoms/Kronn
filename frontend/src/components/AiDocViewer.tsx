import { useState, useEffect, useCallback, useMemo, useRef, type ReactNode } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { projects as projectsApi } from '../lib/api';
import type { AiFileNode } from '../types/generated';
import { useT } from '../lib/I18nContext';
import { MermaidDiagram } from './MermaidDiagram';
import './AiDocViewer.css';
import {
  ChevronRight, ChevronDown, ChevronUp,
  FileText, Folder, Loader2, Search, MessageSquare, X, AlertTriangle,
} from 'lucide-react';

interface AiDocViewerProps {
  projectId: string;
  onDiscussFile?: (filePath: string) => void;
  /** Optional folder prefix to auto-expand on mount, e.g. `docs/tech-debt`.
   *  Used when the user clicks the TD badge on the ProjectCard — the
   *  viewer opens with the tech-debt folder unfolded so the user lands
   *  one click away from the items. Path may include or omit the
   *  trailing slash. */
  initialExpandFolder?: string;
  /** Optional banner rendered above the toolbar. Used to surface state-
   *  aware guidance ("run the audit", "validate to finalize", etc.).
   *  Free-form so the caller controls icon + tone. */
  banner?: React.ReactNode;
}

export function AiDocViewer({ projectId, onDiscussFile, initialExpandFolder, banner }: AiDocViewerProps) {
  const { t } = useT();
  const [tree, setTree] = useState<AiFileNode[]>([]);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [treeLoading, setTreeLoading] = useState(true);
  // Seed the expanded-dirs set so the legacy `ai/` root opens by default
  // (back-compat with pre-0.7.1 projects). When `initialExpandFolder` is
  // passed, we add every prefix path of that folder (`docs`, `docs/tech-debt`,
  // etc.) so the path is visibly unfolded in the tree even if the user
  // hasn't clicked any folder yet.
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(() => {
    const seed = new Set<string>(['ai']);
    if (initialExpandFolder) {
      const parts = initialExpandFolder.replace(/\/$/, '').split('/');
      for (let i = 1; i <= parts.length; i++) {
        seed.add(parts.slice(0, i).join('/'));
      }
    }
    return seed;
  });

  // Search state
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<Map<string, number>>(new Map());
  const [searchLoading, setSearchLoading] = useState(false);
  const [currentMatchIdx, setCurrentMatchIdx] = useState(0);
  const contentRef = useRef<HTMLDivElement>(null);
  const [renderKey, setRenderKey] = useState(0);
  const searchDebounceRef = useRef<ReturnType<typeof setTimeout>>(undefined);

  // ─── Load tree ──────────────────────────────────────────────────────────
  useEffect(() => {
    setTreeLoading(true);
    projectsApi.listAiFiles(projectId).then(files => {
      setTree(files);
      setTreeLoading(false);
      // When the caller deep-linked to a folder (e.g. `docs/tech-debt`
      // via the TD badge), preselect the first file inside it so the
      // user lands on actual content instead of an empty pane. Falls
      // through to the canonical entry-file logic when no match.
      if (initialExpandFolder) {
        const firstInFolder = findFirstFileUnder(files, initialExpandFolder);
        if (firstInFolder) {
          setSelectedPath(firstInFolder.path);
          return;
        }
      }
      // Pick the canonical entry file. Path-agnostic — backend's
      // `listAiFiles` returns either `docs/AGENTS.md` (post-pivot),
      // `doc/AGENTS.md`, or `ai/index.md` (legacy) depending on the
      // project's layout. First match wins.
      const entry =
        findFile(files, 'docs/AGENTS.md')
        ?? findFile(files, 'doc/AGENTS.md')
        ?? findFile(files, 'ai/index.md');
      if (entry) setSelectedPath(entry.path);
    }).catch(() => setTreeLoading(false));
  }, [projectId, initialExpandFolder]);

  // ─── Load file content ─────────────────────────────────────────────────
  useEffect(() => {
    if (!selectedPath) { setContent(null); return; }
    setLoading(true);
    projectsApi.readAiFile(projectId, selectedPath).then(res => {
      setContent(res.content);
      setLoading(false);
      setRenderKey(k => k + 1);
    }).catch(() => { setContent(null); setLoading(false); });
  }, [projectId, selectedPath]);

  // ─── Backend search (debounced) ─────────────────────────────────────────
  useEffect(() => {
    if (searchDebounceRef.current) clearTimeout(searchDebounceRef.current);
    const q = searchQuery.trim();
    if (!q) {
      setSearchResults(new Map());
      setSearchLoading(false);
      return;
    }
    setSearchLoading(true);
    searchDebounceRef.current = setTimeout(() => {
      projectsApi.searchAiFiles(projectId, q).then(results => {
        const map = new Map<string, number>();
        results.forEach(r => map.set(r.path, r.match_count));
        setSearchResults(map);
        setSearchLoading(false);
      }).catch(() => { setSearchResults(new Map()); setSearchLoading(false); });
    }, 250);
    return () => { if (searchDebounceRef.current) clearTimeout(searchDebounceRef.current); };
  }, [projectId, searchQuery]);

  // ─── DOM highlighting (runs after markdown renders) ─────────────────────
  useEffect(() => {
    const container = contentRef.current;
    if (!container) return;
    removeHighlights(container);
    const q = searchQuery.trim();
    if (!q || !content) return;

    const count = applyHighlights(container, q, currentMatchIdx);

    if (count > 0) {
      const safeIdx = currentMatchIdx % count;
      const active = container.querySelector(`mark[data-hl="${safeIdx}"]`);
      active?.scrollIntoView({ behavior: 'smooth', block: 'center' });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchQuery, currentMatchIdx, renderKey]);

  // ─── Reset match index on query or file change ─────────────────────────
  useEffect(() => { setCurrentMatchIdx(0); }, [searchQuery, selectedPath]);

  // ─── Ordered list of files with matches (for cross-file nav) ────────────
  const filesWithMatches = useMemo(() => {
    if (searchResults.size === 0) return [];
    const flat: string[] = [];
    flattenFilePaths(tree, flat);
    return flat.filter(p => searchResults.has(p));
  }, [tree, searchResults]);

  // ─── Global match position for display ──────────────────────────────────
  const globalPosition = useMemo(() => {
    if (!selectedPath || filesWithMatches.length === 0) return { current: 0, total: 0 };
    let before = 0;
    for (const fp of filesWithMatches) {
      if (fp === selectedPath) break;
      before += searchResults.get(fp) ?? 0;
    }
    const total = Array.from(searchResults.values()).reduce((a, b) => a + b, 0);
    return { current: before + currentMatchIdx + 1, total };
  }, [selectedPath, filesWithMatches, searchResults, currentMatchIdx]);

  // ─── Navigation ─────────────────────────────────────────────────────────
  const goPrev = useCallback(() => {
    if (globalPosition.total === 0) return;
    if (currentMatchIdx > 0) {
      setCurrentMatchIdx(i => i - 1);
    } else {
      // Jump to previous file
      const idx = filesWithMatches.indexOf(selectedPath ?? '');
      const prevIdx = idx <= 0 ? filesWithMatches.length - 1 : idx - 1;
      const prevFile = filesWithMatches[prevIdx];
      const prevCount = searchResults.get(prevFile) ?? 1;
      setSelectedPath(prevFile);
      setCurrentMatchIdx(prevCount - 1);
    }
  }, [currentMatchIdx, filesWithMatches, selectedPath, searchResults, globalPosition.total]);

  const goNext = useCallback(() => {
    if (globalPosition.total === 0) return;
    const countInCurrent = searchResults.get(selectedPath ?? '') ?? 0;
    if (currentMatchIdx < countInCurrent - 1) {
      setCurrentMatchIdx(i => i + 1);
    } else {
      // Jump to next file
      const idx = filesWithMatches.indexOf(selectedPath ?? '');
      const nextIdx = idx >= filesWithMatches.length - 1 ? 0 : idx + 1;
      setSelectedPath(filesWithMatches[nextIdx]);
      setCurrentMatchIdx(0);
    }
  }, [currentMatchIdx, filesWithMatches, selectedPath, searchResults, globalPosition.total]);

  const handleSearchKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (e.key === 'Enter') { e.preventDefault(); if (e.shiftKey) goPrev(); else goNext(); }
    if (e.key === 'Escape') { setSearchQuery(''); }
  }, [goNext, goPrev]);

  const toggleDir = useCallback((path: string) => {
    setExpandedDirs(prev => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path); else next.add(path);
      return next;
    });
  }, []);

  // ─── Auto-expand dirs containing matches ────────────────────────────────
  const effectiveExpandedDirs = useMemo(() => {
    if (searchResults.size === 0) return expandedDirs;
    const dirs = new Set(expandedDirs);
    for (const path of searchResults.keys()) {
      // Expand all parent dirs of matching files
      const parts = path.split('/');
      for (let i = 1; i < parts.length; i++) {
        dirs.add(parts.slice(0, i).join('/'));
      }
    }
    return dirs;
  }, [expandedDirs, searchResults]);

  // ─── Render ─────────────────────────────────────────────────────────────

  if (treeLoading) {
    return (
      <>
        {banner}
        <div className="aidoc-loading">
          <Loader2 size={14} style={{ animation: 'spin 1s linear infinite' }} /> {t('projects.docAi.loading')}
        </div>
      </>
    );
  }

  if (tree.length === 0) {
    // Empty tree: typically a `NoTemplate` project. The state banner
    // above the empty state is what tells the user how to bootstrap;
    // we don't want to suppress it when there's "no doc yet".
    return (
      <>
        {banner}
        <div className="aidoc-empty">
          {t('projects.docAi.empty')}
        </div>
      </>
    );
  }

  const isSearching = searchQuery.trim().length > 0;

  return (
    <>
      {banner && <div className="aidoc-banner-wrap">{banner}</div>}
      <div className="aidoc-root">
        {/* File tree */}
      <div className="aidoc-tree-panel">
        {/* Search bar */}
        <div className="aidoc-search-wrap">
          <div className="aidoc-search-box">
            <Search size={11} className="aidoc-search-icon" />
            <input
              type="text"
              value={searchQuery}
              onChange={e => setSearchQuery(e.target.value)}
              onKeyDown={handleSearchKeyDown}
              placeholder={t('projects.docAi.search')}
              className="aidoc-search-input"
            />
            {searchQuery && (
              <button
                onClick={() => setSearchQuery('')}
                className="aidoc-search-clear"
              >
                <X size={10} />
              </button>
            )}
          </div>
          {/* Search results bar: count + navigation */}
          {isSearching && (
            <div className="aidoc-search-results">
              {searchLoading ? (
                <Loader2 size={10} style={{ animation: 'spin 1s linear infinite' }} className="text-dim" />
              ) : globalPosition.total > 0 ? (
                <>
                  <span className="aidoc-search-position">
                    {globalPosition.current} / {globalPosition.total}
                  </span>
                  <span className="aidoc-search-files-count">
                    ({filesWithMatches.length} {filesWithMatches.length > 1 ? t('projects.docAi.files') : t('projects.docAi.file')})
                  </span>
                </>
              ) : (
                <span className="aidoc-search-no-results">{t('projects.docAi.noResults')}</span>
              )}
              {globalPosition.total > 1 && (
                <div className="aidoc-search-nav">
                  <button onClick={goPrev} className="aidoc-nav-btn" title="Shift+Enter">
                    <ChevronUp size={10} />
                  </button>
                  <button onClick={goNext} className="aidoc-nav-btn" title="Enter">
                    <ChevronDown size={10} />
                  </button>
                </div>
              )}
            </div>
          )}
        </div>
        {/* Tree — always show ALL files, with match badges */}
        <div className="aidoc-tree-list">
          {tree.map(node => (
            <TreeNode
              key={node.path} node={node} selectedPath={selectedPath}
              expandedDirs={effectiveExpandedDirs}
              onSelect={setSelectedPath} onToggleDir={toggleDir} depth={0}
              searchResults={searchResults} isSearching={isSearching}
            />
          ))}
        </div>
      </div>
      {/* Content */}
      <div className="aidoc-content-panel">
        {/* Toolbar */}
        {selectedPath && content !== null && !loading && (
          <div className="aidoc-toolbar">
            <span className="aidoc-toolbar-path">
              {selectedPath}
            </span>
            {onDiscussFile && (() => {
              // Tech-debt files (`docs/tech-debt/TD-*.md`) get a stronger
              // CTA reframed as "fix this issue" — same underlying action
              // (launch a discussion bound to the file) but the verb is
              // resolution-oriented. The detection is intentionally
              // permissive: any path that contains `/tech-debt/` and
              // points to a `TD-*` file qualifies. Stays in sync with
              // backend `count_tech_debt` (same filename pattern).
              const isTechDebt =
                /\/tech-debt\//.test(selectedPath) &&
                /\/TD-[^/]+\.md$/.test(selectedPath);
              return (
                <button
                  onClick={() => onDiscussFile(selectedPath)}
                  className={`aidoc-discuss-btn${isTechDebt ? ' aidoc-discuss-btn-fix' : ''}`}
                >
                  {isTechDebt ? (
                    <>
                      <AlertTriangle size={10} /> {t('projects.docAi.fixThis')}
                    </>
                  ) : (
                    <>
                      <MessageSquare size={10} /> {t('projects.docAi.discuss')}
                    </>
                  )}
                </button>
              );
            })()}
          </div>
        )}
        {/* Content area */}
        <div ref={contentRef} key={renderKey} className="aidoc-content-area">
          {loading ? (
            <div className="aidoc-loading">
              <Loader2 size={14} style={{ animation: 'spin 1s linear infinite' }} /> {t('projects.docAi.loading')}
            </div>
          ) : content !== null ? (
            <DocMarkdown content={content} />
          ) : (
            <div className="aidoc-select-file">
              {t('projects.docAi.selectFile')}
            </div>
          )}
        </div>
      </div>
      </div>
    </>
  );
}

// ─── DOM highlight ───────────────────────────────────────────────────────────

const HL_ATTR = 'data-hl';

function removeHighlights(container: HTMLElement) {
  const marks = container.querySelectorAll(`mark[${HL_ATTR}]`);
  marks.forEach(mark => {
    const parent = mark.parentNode;
    if (!parent) return;
    parent.replaceChild(document.createTextNode(mark.textContent || ''), mark);
    parent.normalize();
  });
}

function applyHighlights(container: HTMLElement, query: string, activeIdx: number): number {
  const lowerQuery = query.toLowerCase();
  const textNodes: Text[] = [];
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT, null);
  while (walker.nextNode()) textNodes.push(walker.currentNode as Text);

  interface Match { node: Text; start: number; }
  const matches: Match[] = [];
  for (const node of textNodes) {
    const text = node.textContent || '';
    const lower = text.toLowerCase();
    let idx = lower.indexOf(lowerQuery);
    while (idx !== -1) {
      matches.push({ node, start: idx });
      idx = lower.indexOf(lowerQuery, idx + 1);
    }
  }

  if (matches.length === 0) return 0;
  const safeActive = activeIdx % matches.length;

  // Group by node, process each node's matches from end to start
  const byNode = new Map<Text, { start: number; globalIdx: number }[]>();
  matches.forEach((m, i) => {
    let arr = byNode.get(m.node);
    if (!arr) {
      arr = [];
      byNode.set(m.node, arr);
    }
    arr.push({ start: m.start, globalIdx: i });
  });

  for (const [node, nodeMatches] of byNode.entries()) {
    const sorted = [...nodeMatches].sort((a, b) => b.start - a.start);
    const currentNode: Text = node;

    for (const { start, globalIdx } of sorted) {
      const isActive = globalIdx === safeActive;
      const text = currentNode.textContent || '';
      if (start + query.length > text.length) continue;

      currentNode.splitText(start + query.length);
      const matchNode = currentNode.splitText(start);

      const mark = document.createElement('mark');
      mark.setAttribute(HL_ATTR, String(globalIdx));
      mark.style.background = isActive ? 'rgba(var(--kr-accent-rgb), 0.5)' : 'rgba(var(--kr-accent-rgb), 0.18)';
      mark.style.color = 'inherit';
      mark.style.borderRadius = '2px';
      mark.style.padding = '0 1px';
      if (isActive) mark.style.outline = '1.5px solid #c8ff00';

      const parent = matchNode.parentNode;
      if (!parent) continue;
      parent.replaceChild(mark, matchNode);
      mark.appendChild(matchNode);
    }
  }

  return matches.length;
}

// ─── Tree ────────────────────────────────────────────────────────────────────

function TreeNode({ node, selectedPath, expandedDirs, onSelect, onToggleDir, depth, searchResults, isSearching }: {
  node: AiFileNode;
  selectedPath: string | null;
  expandedDirs: Set<string>;
  onSelect: (path: string) => void;
  onToggleDir: (path: string) => void;
  depth: number;
  searchResults: Map<string, number>;
  isSearching: boolean;
}) {
  const isExpanded = expandedDirs.has(node.path);
  const isSelected = selectedPath === node.path;
  const pl = 8 + depth * 14;
  const matchCount = searchResults.get(node.path);
  const hasMatches = matchCount !== undefined && matchCount > 0;
  // Dim files without matches when searching
  const dimmed = isSearching && !node.is_dir && !hasMatches;

  if (node.is_dir) {
    return (
      <>
        <div
          className="aidoc-tree-dir"
          style={{ paddingLeft: pl, padding: `3px 8px 3px ${pl}px` }}
          onClick={() => onToggleDir(node.path)}
        >
          {isExpanded ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
          <Folder size={11} className="aidoc-tree-dir-icon" />
          <span>{node.name}</span>
        </div>
        {isExpanded && (node.children ?? []).map(child => (
          <TreeNode key={child.path} node={child} selectedPath={selectedPath} expandedDirs={expandedDirs}
            onSelect={onSelect} onToggleDir={onToggleDir} depth={depth + 1}
            searchResults={searchResults} isSearching={isSearching}
          />
        ))}
      </>
    );
  }

  return (
    <div
      className="aidoc-tree-file"
      data-selected={isSelected}
      data-dimmed={dimmed}
      style={{ paddingLeft: pl, padding: `3px 8px 3px ${pl}px` }}
      onClick={() => onSelect(node.path)}
    >
      <FileText size={11} className="flex-shrink-0" />
      <span className="aidoc-tree-file-name">
        {node.name}
      </span>
      {hasMatches && (
        <span className="aidoc-match-badge">
          {matchCount}
        </span>
      )}
    </div>
  );
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

function findFile(nodes: AiFileNode[], path: string): AiFileNode | null {
  for (const node of nodes) {
    if (node.path === path) return node;
    if (node.is_dir && node.children) {
      const found = findFile(node.children, path);
      if (found) return found;
    }
  }
  return null;
}

/** Find the first leaf file whose path starts with the given folder
 *  prefix. Used by the TD-badge deep-link: clicking "<N> TD" opens the
 *  viewer pre-selecting the first file in `docs/tech-debt/`. */
function findFirstFileUnder(nodes: AiFileNode[], folder: string): AiFileNode | null {
  const prefix = folder.replace(/\/$/, '') + '/';
  for (const node of nodes) {
    if (!node.is_dir && (node.path === folder || node.path.startsWith(prefix))) {
      return node;
    }
    if (node.is_dir && node.children) {
      const found = findFirstFileUnder(node.children, folder);
      if (found) return found;
    }
  }
  return null;
}

/** Flatten file paths in tree order (for ordered cross-file navigation) */
function flattenFilePaths(nodes: AiFileNode[], out: string[]) {
  for (const node of nodes) {
    if (node.is_dir) {
      if (node.children) flattenFilePaths(node.children, out);
    } else {
      out.push(node.path);
    }
  }
}

// ─── Markdown ────────────────────────────────────────────────────────────────

// HOISTED at module level (NOT inline inside the component) so the
// reference is stable across renders. Critical for performance + state
// retention: when a parent re-renders (e.g. Dashboard's 3s
// `auditStatusAll` polling), passing a fresh `components` object would
// make ReactMarkdown unmount + remount every child, which resets the
// MermaidDiagram's `fullscreen` state and makes the overlay disappear
// every 3 seconds. 0.8.3 user bug report.
const DOC_MD_COMPONENTS = {
  p: ({ children }: { children?: ReactNode }) => <p>{children}</p>,
  h1: ({ children }: { children?: ReactNode }) => <h1>{children}</h1>,
  h2: ({ children }: { children?: ReactNode }) => <h2>{children}</h2>,
  h3: ({ children }: { children?: ReactNode }) => <h3>{children}</h3>,
  ul: ({ children }: { children?: ReactNode }) => <ul>{children}</ul>,
  ol: ({ children }: { children?: ReactNode }) => <ol>{children}</ol>,
  li: ({ children }: { children?: ReactNode }) => <li>{children}</li>,
  code: ({ className, children }: { className?: string; children?: ReactNode }) => {
    const isBlock = className?.includes('language-');
    return isBlock
      ? <code className="aidoc-md-pre-code">{children}</code>
      : <code>{children}</code>;
  },
  pre: ({ children }: { children?: ReactNode }) => {
    // Intercept ```mermaid blocks and render them visually instead of
    // dumping the source. Audit Step 6 ships diagrams in
    // docs/architecture/overview.md and docs/architecture/sequences/*.md.
    const childEl = Array.isArray(children) ? children[0] : children;
    const codeEl = childEl as { props?: { className?: string; children?: ReactNode } } | undefined;
    const className = codeEl?.props?.className ?? '';
    if (className.includes('language-mermaid')) {
      const raw = codeEl?.props?.children;
      const source = Array.isArray(raw) ? raw.join('') : String(raw ?? '');
      return <MermaidDiagram source={source.trim()} />;
    }
    return <pre>{children}</pre>;
  },
  table: ({ children }: { children?: ReactNode }) => <table>{children}</table>,
  th: ({ children }: { children?: ReactNode }) => <th>{children}</th>,
  td: ({ children }: { children?: ReactNode }) => <td>{children}</td>,
  blockquote: ({ children }: { children?: ReactNode }) => <blockquote>{children}</blockquote>,
  hr: () => <hr />,
  a: ({ href, children }: { href?: string; children?: ReactNode }) =>
    <a href={href} target="_blank" rel="noopener noreferrer">{children}</a>,
  strong: ({ children }: { children?: ReactNode }) => <strong>{children}</strong>,
};

const DOC_MD_REMARK_PLUGINS = [remarkGfm];

const DocMarkdown = ({ content }: { content: string }) => (
  <div className="aidoc-md">
    <ReactMarkdown remarkPlugins={DOC_MD_REMARK_PLUGINS} components={DOC_MD_COMPONENTS}>
      {content}
    </ReactMarkdown>
  </div>
);
