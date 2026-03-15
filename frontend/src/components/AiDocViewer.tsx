import { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { projects as projectsApi } from '../lib/api';
import type { AiFileNode } from '../types/generated';
import { useT } from '../lib/I18nContext';
import {
  ChevronRight, ChevronDown, ChevronUp,
  FileText, Folder, Loader2, Search, MessageSquare, X,
} from 'lucide-react';

interface AiDocViewerProps {
  projectId: string;
  onDiscussFile?: (filePath: string) => void;
}

export function AiDocViewer({ projectId, onDiscussFile }: AiDocViewerProps) {
  const { t } = useT();
  const [tree, setTree] = useState<AiFileNode[]>([]);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [treeLoading, setTreeLoading] = useState(true);
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(new Set(['ai']));

  // Search state
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<Map<string, number>>(new Map());
  const [searchLoading, setSearchLoading] = useState(false);
  const [currentMatchIdx, setCurrentMatchIdx] = useState(0);
  const contentRef = useRef<HTMLDivElement>(null);
  const [renderKey, setRenderKey] = useState(0);
  const searchDebounceRef = useRef<ReturnType<typeof setTimeout>>();

  // ─── Load tree ──────────────────────────────────────────────────────────
  useEffect(() => {
    setTreeLoading(true);
    projectsApi.listAiFiles(projectId).then(files => {
      setTree(files);
      setTreeLoading(false);
      const indexFile = findFile(files, 'ai/index.md');
      if (indexFile) setSelectedPath(indexFile.path);
    }).catch(() => setTreeLoading(false));
  }, [projectId]);

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
    if (e.key === 'Enter') { e.preventDefault(); e.shiftKey ? goPrev() : goNext(); }
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
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: 16, color: 'rgba(255,255,255,0.4)', fontSize: 12 }}>
        <Loader2 size={14} style={{ animation: 'spin 1s linear infinite' }} /> {t('projects.docAi.loading')}
      </div>
    );
  }

  if (tree.length === 0) {
    return (
      <div style={{ padding: 16, color: 'rgba(255,255,255,0.3)', fontSize: 12 }}>
        {t('projects.docAi.empty')}
      </div>
    );
  }

  const isSearching = searchQuery.trim().length > 0;

  return (
    <div style={{ display: 'flex', maxHeight: 400, border: '1px solid rgba(255,255,255,0.06)', borderRadius: 8, overflow: 'hidden', background: 'rgba(0,0,0,0.2)' }}>
      {/* File tree */}
      <div style={{ width: '30%', minWidth: 170, borderRight: '1px solid rgba(255,255,255,0.06)', display: 'flex', flexDirection: 'column' }}>
        {/* Search bar */}
        <div style={{ padding: '6px 6px 4px', borderBottom: '1px solid rgba(255,255,255,0.04)' }}>
          <div style={{ position: 'relative' }}>
            <Search size={11} style={{ position: 'absolute', left: 7, top: 6, color: 'rgba(255,255,255,0.25)', pointerEvents: 'none' }} />
            <input
              type="text"
              value={searchQuery}
              onChange={e => setSearchQuery(e.target.value)}
              onKeyDown={handleSearchKeyDown}
              placeholder={t('projects.docAi.search')}
              style={{
                width: '100%', padding: '4px 22px 4px 22px', background: 'rgba(255,255,255,0.04)',
                border: '1px solid rgba(255,255,255,0.08)', borderRadius: 4, color: '#e8eaed',
                fontSize: 10, fontFamily: 'inherit', outline: 'none', boxSizing: 'border-box',
              }}
            />
            {searchQuery && (
              <button
                onClick={() => setSearchQuery('')}
                style={{ position: 'absolute', right: 4, top: 3, background: 'none', border: 'none', color: 'rgba(255,255,255,0.3)', cursor: 'pointer', padding: 0, lineHeight: 1 }}
              >
                <X size={10} />
              </button>
            )}
          </div>
          {/* Search results bar: count + navigation */}
          {isSearching && (
            <div style={{ display: 'flex', alignItems: 'center', gap: 4, marginTop: 4, fontSize: 10 }}>
              {searchLoading ? (
                <Loader2 size={10} style={{ animation: 'spin 1s linear infinite', color: 'rgba(255,255,255,0.3)' }} />
              ) : globalPosition.total > 0 ? (
                <>
                  <span style={{ color: 'rgba(200,255,0,0.7)' }}>
                    {globalPosition.current} / {globalPosition.total}
                  </span>
                  <span style={{ color: 'rgba(255,255,255,0.25)', marginLeft: 2 }}>
                    ({filesWithMatches.length} {filesWithMatches.length > 1 ? t('projects.docAi.files') : t('projects.docAi.file')})
                  </span>
                </>
              ) : (
                <span style={{ color: 'rgba(255,255,255,0.3)' }}>{t('projects.docAi.noResults')}</span>
              )}
              {globalPosition.total > 1 && (
                <div style={{ marginLeft: 'auto', display: 'flex', gap: 2 }}>
                  <button onClick={goPrev} style={navBtnStyle} title="Shift+Enter">
                    <ChevronUp size={10} />
                  </button>
                  <button onClick={goNext} style={navBtnStyle} title="Enter">
                    <ChevronDown size={10} />
                  </button>
                </div>
              )}
            </div>
          )}
        </div>
        {/* Tree — always show ALL files, with match badges */}
        <div style={{ flex: 1, overflowY: 'auto', padding: '4px 0' }}>
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
      <div style={{ flex: 1, display: 'flex', flexDirection: 'column', overflow: 'hidden' }}>
        {/* Toolbar */}
        {selectedPath && content !== null && !loading && (
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '6px 12px', borderBottom: '1px solid rgba(255,255,255,0.04)', flexShrink: 0, gap: 8 }}>
            <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)', fontFamily: 'JetBrains Mono, monospace', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
              {selectedPath}
            </span>
            {onDiscussFile && (
              <button
                onClick={() => onDiscussFile(selectedPath)}
                style={{
                  display: 'inline-flex', alignItems: 'center', gap: 4, padding: '3px 8px', borderRadius: 4,
                  fontSize: 10, fontFamily: 'inherit', cursor: 'pointer', flexShrink: 0,
                  background: 'rgba(200,255,0,0.06)', border: '1px solid rgba(200,255,0,0.15)', color: '#c8ff00',
                }}
              >
                <MessageSquare size={10} /> {t('projects.docAi.discuss')}
              </button>
            )}
          </div>
        )}
        {/* Content area */}
        <div ref={contentRef} key={renderKey} style={{ flex: 1, overflowY: 'auto', padding: '12px 16px' }}>
          {loading ? (
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, color: 'rgba(255,255,255,0.4)', fontSize: 12 }}>
              <Loader2 size={14} style={{ animation: 'spin 1s linear infinite' }} /> {t('projects.docAi.loading')}
            </div>
          ) : content !== null ? (
            <DocMarkdown content={content} />
          ) : (
            <div style={{ color: 'rgba(255,255,255,0.3)', fontSize: 12 }}>
              {t('projects.docAi.selectFile')}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

// ─── Styles ──────────────────────────────────────────────────────────────────

const navBtnStyle: React.CSSProperties = {
  background: 'none', border: '1px solid rgba(255,255,255,0.1)', borderRadius: 3,
  color: 'rgba(255,255,255,0.5)', cursor: 'pointer', padding: '1px 3px',
  display: 'inline-flex', alignItems: 'center', lineHeight: 1,
};

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
    if (!byNode.has(m.node)) byNode.set(m.node, []);
    byNode.get(m.node)!.push({ start: m.start, globalIdx: i });
  });

  for (const [node, nodeMatches] of byNode.entries()) {
    const sorted = [...nodeMatches].sort((a, b) => b.start - a.start);
    let currentNode: Text = node;

    for (const { start, globalIdx } of sorted) {
      const isActive = globalIdx === safeActive;
      const text = currentNode.textContent || '';
      if (start + query.length > text.length) continue;

      currentNode.splitText(start + query.length);
      const matchNode = currentNode.splitText(start);

      const mark = document.createElement('mark');
      mark.setAttribute(HL_ATTR, String(globalIdx));
      mark.style.background = isActive ? 'rgba(200,255,0,0.5)' : 'rgba(200,255,0,0.18)';
      mark.style.color = 'inherit';
      mark.style.borderRadius = '2px';
      mark.style.padding = '0 1px';
      if (isActive) mark.style.outline = '1.5px solid #c8ff00';

      matchNode.parentNode!.replaceChild(mark, matchNode);
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
          style={{ display: 'flex', alignItems: 'center', gap: 4, padding: `3px 8px 3px ${pl}px`, cursor: 'pointer', fontSize: 11, color: 'rgba(255,255,255,0.5)', userSelect: 'none' }}
          onClick={() => onToggleDir(node.path)}
        >
          {isExpanded ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
          <Folder size={11} style={{ color: 'rgba(200,255,0,0.5)' }} />
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
      style={{
        display: 'flex', alignItems: 'center', gap: 4, padding: `3px 8px 3px ${pl}px`, cursor: 'pointer', fontSize: 11,
        color: isSelected ? '#c8ff00' : dimmed ? 'rgba(255,255,255,0.2)' : 'rgba(255,255,255,0.6)',
        background: isSelected ? 'rgba(200,255,0,0.06)' : 'transparent',
        borderRight: isSelected ? '2px solid #c8ff00' : '2px solid transparent',
        opacity: dimmed ? 0.5 : 1,
      }}
      onClick={() => onSelect(node.path)}
    >
      <FileText size={11} style={{ flexShrink: 0 }} />
      <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', flex: 1 }}>
        {node.name}
      </span>
      {hasMatches && (
        <span style={{
          fontSize: 9, padding: '0 5px', borderRadius: 8, fontWeight: 600, flexShrink: 0,
          background: 'rgba(200,255,0,0.12)', color: 'rgba(200,255,0,0.8)',
        }}>
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

const mdStyles: Record<string, React.CSSProperties> = {
  p: { margin: '4px 0' },
  h1: { fontSize: 18, fontWeight: 700, margin: '12px 0 6px', color: '#e8eaed' },
  h2: { fontSize: 16, fontWeight: 700, margin: '10px 0 4px', color: '#e8eaed' },
  h3: { fontSize: 14, fontWeight: 600, margin: '8px 0 4px', color: '#e8eaed' },
  ul: { margin: '4px 0', paddingLeft: 20 },
  ol: { margin: '4px 0', paddingLeft: 20 },
  li: { margin: '2px 0' },
  code: { background: 'rgba(255,255,255,0.08)', padding: '1px 5px', borderRadius: 4, fontSize: 12, fontFamily: 'monospace' },
  pre: { background: 'rgba(0,0,0,0.3)', padding: '10px 12px', borderRadius: 8, overflowX: 'auto', margin: '6px 0', border: '1px solid rgba(255,255,255,0.06)' },
  preCode: { background: 'none', padding: 0, fontSize: 12, fontFamily: 'monospace', color: '#c8ff00' },
  table: { borderCollapse: 'collapse' as const, width: '100%', margin: '8px 0', fontSize: 12 },
  th: { border: '1px solid rgba(255,255,255,0.12)', padding: '6px 10px', background: 'rgba(255,255,255,0.05)', fontWeight: 600, textAlign: 'left' as const },
  td: { border: '1px solid rgba(255,255,255,0.08)', padding: '5px 10px' },
  blockquote: { borderLeft: '3px solid rgba(200,255,0,0.3)', margin: '6px 0', paddingLeft: 12, color: 'rgba(255,255,255,0.6)' },
  hr: { border: 'none', borderTop: '1px solid rgba(255,255,255,0.1)', margin: '10px 0' },
  a: { color: '#c8ff00', textDecoration: 'underline' },
  strong: { fontWeight: 700, color: '#f0f0f0' },
};

const DocMarkdown = ({ content }: { content: string }) => (
  <div style={{ fontSize: 13, lineHeight: 1.55 }}>
    <ReactMarkdown
      remarkPlugins={[remarkGfm]}
      components={{
        p: ({ children }) => <p style={mdStyles.p}>{children}</p>,
        h1: ({ children }) => <h1 style={mdStyles.h1}>{children}</h1>,
        h2: ({ children }) => <h2 style={mdStyles.h2}>{children}</h2>,
        h3: ({ children }) => <h3 style={mdStyles.h3}>{children}</h3>,
        ul: ({ children }) => <ul style={mdStyles.ul}>{children}</ul>,
        ol: ({ children }) => <ol style={mdStyles.ol}>{children}</ol>,
        li: ({ children }) => <li style={mdStyles.li}>{children}</li>,
        code: ({ className, children }) => {
          const isBlock = className?.includes('language-');
          return isBlock
            ? <code style={mdStyles.preCode}>{children}</code>
            : <code style={mdStyles.code}>{children}</code>;
        },
        pre: ({ children }) => <pre style={mdStyles.pre}>{children}</pre>,
        table: ({ children }) => <table style={mdStyles.table}>{children}</table>,
        th: ({ children }) => <th style={mdStyles.th}>{children}</th>,
        td: ({ children }) => <td style={mdStyles.td}>{children}</td>,
        blockquote: ({ children }) => <blockquote style={mdStyles.blockquote}>{children}</blockquote>,
        hr: () => <hr style={mdStyles.hr} />,
        a: ({ href, children }) => <a href={href} style={mdStyles.a} target="_blank" rel="noopener noreferrer">{children}</a>,
        strong: ({ children }) => <strong style={mdStyles.strong}>{children}</strong>,
      }}
    >
      {content}
    </ReactMarkdown>
  </div>
);
