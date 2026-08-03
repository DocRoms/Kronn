import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ChevronDown, ChevronRight, ChevronUp, Code2, FileCode2, Folder, FolderX, GitBranch,
  GitCompareArrows, History,
  Loader2, Search, X,
} from 'lucide-react';
import { projects as projectsApi } from '../lib/api';
import type { GitBlameLine, GitCommitDetail, SourceFileNode } from '../types/generated';
import { useT } from '../lib/I18nContext';
import { highlightLine as highlightSourceLine, languageForPath } from '../lib/diff-syntax';
import './SourceCodeViewer.css';

interface SourceCodeViewerProps {
  projectId: string;
  /** KT-75 — open the full patch of a commit in the panel's temporary tab.
   *  Absent when the host has no tab to open it in. */
  onOpenCommit?: (sha: string) => void;
}

export function SourceCodeViewer({ projectId, onOpenCommit }: SourceCodeViewerProps) {
  return (
    <SourceCodeViewerProject
      key={projectId}
      projectId={projectId}
      onOpenCommit={onOpenCommit}
    />
  );
}

interface ContentResult {
  projectId: string;
  path: string;
  content: string | null;
}

interface BlameResult {
  projectId: string;
  path: string;
  lines: GitBlameLine[];
  error: boolean;
}

interface CommitResult {
  projectId: string;
  sha: string;
  detail: GitCommitDetail | null;
  error: string | null;
}

interface SearchResult {
  projectId: string;
  query: string;
  exclusionsKey: string;
  matches: Map<string, number>;
}

const EMPTY_SEARCH_RESULTS = new Map<string, number>();

function SourceCodeViewerProject({ projectId, onOpenCommit }: SourceCodeViewerProps) {
  const { t } = useT();
  const [tree, setTree] = useState<SourceFileNode[]>([]);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [contentResult, setContentResult] = useState<ContentResult | null>(null);
  const [treeLoading, setTreeLoading] = useState(true);
  const [treeHydrating, setTreeHydrating] = useState(false);
  const [treeError, setTreeError] = useState(false);
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResult, setSearchResult] = useState<SearchResult | null>(null);
  const [currentMatchIdx, setCurrentMatchIdx] = useState(0);
  const [branch, setBranch] = useState<string | null>(null);
  const [annotate, setAnnotate] = useState(false);
  const [blameResult, setBlameResult] = useState<BlameResult | null>(null);
  // KT-67 — the commit an annotated line points at. `sha` drives the fetch
  // so a click is enough; the detail arrives asynchronously.
  const [commitSha, setCommitSha] = useState<string | null>(null);
  const [commitResult, setCommitResult] = useState<CommitResult | null>(null);
  const [exclusions, setExclusions] = useState<string[]>([]);
  const [exclusionSaving, setExclusionSaving] = useState<string | null>(null);
  const [exclusionError, setExclusionError] = useState(false);
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(
    () => new Set(['src', 'app', 'application', 'frontend', 'backend']),
  );
  const searchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const treeLoadRef = useRef(0);

  const readTreeRoot = useCallback(() => Promise.all([
    projectsApi.listSourceFiles(projectId, true),
    projectsApi.getSourceExclusions(projectId),
  ]), [projectId]);

  const rejectTreeRoot = useCallback((generation: number) => {
    if (treeLoadRef.current !== generation) return;
    setTreeError(true);
    setTreeLoading(false);
  }, []);

  const applyTreeRoot = useCallback((
    rootFiles: SourceFileNode[],
    savedExclusions: string[],
    generation: number,
  ) => {
    if (treeLoadRef.current !== generation) return Promise.resolve();
    setTree(rootFiles);
    setExclusions(savedExclusions);
    setSelectedPath(previous => (
      previous
        ? previous
        : findPreferredSourceFile(rootFiles)?.path ?? null
    ));
    setTreeLoading(false);
    setTreeHydrating(true);

    return projectsApi.listSourceFiles(projectId)
      .then(files => {
        if (treeLoadRef.current !== generation) return;
        setTree(files);
        setSelectedPath(previous => (
          previous && hasSourcePath(files, previous)
            ? previous
            : findPreferredSourceFile(files)?.path ?? null
        ));
      })
      .catch(() => {
        // The root-level tree is already usable; a failed enrichment must not
        // replace it with a full-page error.
      })
      .finally(() => {
        if (treeLoadRef.current === generation) setTreeHydrating(false);
      });
  }, [projectId]);

  const fetchTree = useCallback(() => {
    const generation = treeLoadRef.current + 1;
    treeLoadRef.current = generation;
    return readTreeRoot()
      .then(([rootFiles, savedExclusions]) => (
        applyTreeRoot(rootFiles, savedExclusions, generation)
      ))
      .catch(() => rejectTreeRoot(generation));
  }, [applyTreeRoot, readTreeRoot, rejectTreeRoot]);

  useEffect(() => {
    const generation = treeLoadRef.current + 1;
    treeLoadRef.current = generation;
    void readTreeRoot()
      .then(([rootFiles, savedExclusions]) => (
        applyTreeRoot(rootFiles, savedExclusions, generation)
      ))
      .catch(() => rejectTreeRoot(generation));
    return () => {
      treeLoadRef.current += 1;
    };
  }, [applyTreeRoot, readTreeRoot, rejectTreeRoot]);

  const retryTree = useCallback(() => {
    setTreeLoading(true);
    setTreeHydrating(false);
    setTreeError(false);
    void fetchTree();
  }, [fetchTree]);

  const saveExclusions = useCallback(async (nextPaths: string[], changedPath: string) => {
    setExclusionSaving(changedPath);
    setExclusionError(false);
    setTreeLoading(true);
    setTreeHydrating(false);
    setTreeError(false);
    try {
      const saved = await projectsApi.setSourceExclusions(projectId, nextPaths);
      setExclusions(saved);
      await fetchTree();
    } catch {
      setExclusionError(true);
    } finally {
      setExclusionSaving(null);
    }
  }, [fetchTree, projectId]);

  useEffect(() => {
    let alive = true;
    projectsApi.gitStatus(projectId)
      .then(status => { if (alive) setBranch(status.branch); })
      .catch(() => { if (alive) setBranch(null); });
    return () => { alive = false; };
  }, [projectId]);

  useEffect(() => {
    if (!selectedPath) return;
    let alive = true;
    const path = selectedPath;
    projectsApi.readSourceFile(projectId, selectedPath)
      .then(file => {
        if (alive) setContentResult({ projectId, path, content: file.content });
      })
      .catch(() => {
        if (alive) setContentResult({ projectId, path, content: null });
      });
    return () => { alive = false; };
  }, [projectId, selectedPath]);

  useEffect(() => {
    if (!annotate || !selectedPath) return;
    let alive = true;
    const path = selectedPath;
    projectsApi.gitBlame(projectId, selectedPath)
      .then(result => {
        if (alive) setBlameResult({ projectId, path, lines: result.lines, error: false });
      })
      .catch(() => {
        if (alive) setBlameResult({ projectId, path, lines: [], error: true });
      });
    return () => { alive = false; };
  }, [annotate, projectId, selectedPath]);

  // KT-67 — load the clicked commit. Keyed on the sha so re-clicking the same
  // line is free, and a stale response can't overwrite a newer one.
  useEffect(() => {
    if (!commitSha) return;
    let alive = true;
    const sha = commitSha;
    projectsApi.gitCommitDetail(projectId, commitSha)
      .then(detail => {
        if (alive) setCommitResult({ projectId, sha, detail, error: null });
      })
      .catch(error => {
        if (alive) setCommitResult({ projectId, sha, detail: null, error: String(error) });
      });
    return () => { alive = false; };
  }, [commitSha, projectId]);

  // Escape closes the commit detail before anything else reacts to it.
  useEffect(() => {
    if (!commitSha) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setCommitSha(null);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [commitSha]);

  useEffect(() => {
    if (searchTimer.current) clearTimeout(searchTimer.current);
    const query = searchQuery.trim();
    if (!query) return;
    const exclusionsKey = exclusions.join('\0');
    let active = true;
    searchTimer.current = setTimeout(() => {
      projectsApi.searchSourceFiles(projectId, query)
        .then(results => {
          if (!active) return;
          const matches = new Map(results.map(result => [result.path, result.match_count]));
          setSearchResult({ projectId, query, exclusionsKey, matches });
          setSelectedPath(current =>
            results[0] && !matches.has(current ?? '') ? results[0].path : current,
          );
          setCurrentMatchIdx(0);
        })
        .catch(() => {
          if (active) {
            setSearchResult({ projectId, query, exclusionsKey, matches: new Map() });
          }
        });
    }, 250);
    return () => {
      active = false;
      if (searchTimer.current) clearTimeout(searchTimer.current);
    };
  }, [exclusions, projectId, searchQuery]);

  const contentIsCurrent = contentResult?.projectId === projectId
    && contentResult.path === selectedPath;
  const content = contentIsCurrent ? contentResult.content : null;
  const contentLoading = selectedPath !== null && !contentIsCurrent;
  const blameIsCurrent = annotate
    && blameResult?.projectId === projectId
    && blameResult.path === selectedPath;
  const blameLoading = annotate && selectedPath !== null && !blameIsCurrent;
  const blameError = blameIsCurrent && blameResult.error;
  const commitIsCurrent = commitResult?.projectId === projectId
    && commitResult.sha === commitSha;
  const commitDetail = commitIsCurrent ? commitResult.detail : null;
  const commitError = commitIsCurrent ? commitResult.error : null;
  const commitLoading = commitSha !== null && !commitIsCurrent;
  const trimmedSearchQuery = searchQuery.trim();
  const exclusionsKey = exclusions.join('\0');
  const searchIsCurrent = searchResult?.projectId === projectId
    && searchResult.query === trimmedSearchQuery
    && searchResult.exclusionsKey === exclusionsKey;
  const searchResults = trimmedSearchQuery && searchIsCurrent
    ? searchResult.matches
    : EMPTY_SEARCH_RESULTS;
  const searchLoading = Boolean(trimmedSearchQuery) && !searchIsCurrent;

  const effectiveExpandedDirs = useMemo(() => {
    if (searchResults.size === 0) return expandedDirs;
    const expanded = new Set(expandedDirs);
    for (const path of searchResults.keys()) {
      const parts = path.split('/');
      for (let index = 1; index < parts.length; index += 1) {
        expanded.add(parts.slice(0, index).join('/'));
      }
    }
    return expanded;
  }, [expandedDirs, searchResults]);

  const totalMatches = useMemo(
    () => Array.from(searchResults.values()).reduce((total, count) => total + count, 0),
    [searchResults],
  );
  const filesWithMatches = useMemo(() => {
    const paths: string[] = [];
    flattenSourcePaths(tree, paths);
    return paths.filter(path => searchResults.has(path));
  }, [tree, searchResults]);
  const globalMatchPosition = useMemo(() => {
    if (!selectedPath || totalMatches === 0) return 0;
    let before = 0;
    for (const path of filesWithMatches) {
      if (path === selectedPath) break;
      before += searchResults.get(path) ?? 0;
    }
    return before + currentMatchIdx + 1;
  }, [currentMatchIdx, filesWithMatches, searchResults, selectedPath, totalMatches]);
  const language = selectedPath ? sourceLanguage(selectedPath) : '';
  const syntaxLanguage = selectedPath ? languageForPath(selectedPath) : null;
  const blameByLine = useMemo(() => {
    const lines = blameIsCurrent ? blameResult.lines : [];
    return new Map(lines.map(line => [line.line_number, line]));
  }, [blameIsCurrent, blameResult]);

  const goToPreviousMatch = useCallback(() => {
    if (!selectedPath || totalMatches === 0) return;
    if (currentMatchIdx > 0) {
      setCurrentMatchIdx(index => index - 1);
      return;
    }
    const fileIndex = filesWithMatches.indexOf(selectedPath);
    const previousFile = filesWithMatches[
      fileIndex <= 0 ? filesWithMatches.length - 1 : fileIndex - 1
    ];
    if (!previousFile) return;
    setSelectedPath(previousFile);
    setCurrentMatchIdx((searchResults.get(previousFile) ?? 1) - 1);
  }, [currentMatchIdx, filesWithMatches, searchResults, selectedPath, totalMatches]);

  const goToNextMatch = useCallback(() => {
    if (!selectedPath || totalMatches === 0) return;
    const matchesInFile = searchResults.get(selectedPath) ?? 0;
    if (currentMatchIdx < matchesInFile - 1) {
      setCurrentMatchIdx(index => index + 1);
      return;
    }
    const fileIndex = filesWithMatches.indexOf(selectedPath);
    const nextFile = filesWithMatches[
      fileIndex < 0 || fileIndex >= filesWithMatches.length - 1 ? 0 : fileIndex + 1
    ];
    if (!nextFile) return;
    setSelectedPath(nextFile);
    setCurrentMatchIdx(0);
  }, [currentMatchIdx, filesWithMatches, searchResults, selectedPath, totalMatches]);

  useEffect(() => {
    const container = contentRef.current;
    if (!container) return;
    removeSourceHighlights(container);
    const query = searchQuery.trim();
    if (!query || content === null) return;
    const count = applySourceHighlights(container, query, currentMatchIdx);
    if (count > 0) {
      const active = container.querySelector(`mark[data-source-hl="${currentMatchIdx % count}"]`);
      active?.scrollIntoView({ behavior: 'smooth', block: 'center', inline: 'nearest' });
    }
  });

  if (treeLoading) {
    return (
      <div className="source-state">
        <Loader2 size={15} className="spin" /> {t('projects.source.loading')}
      </div>
    );
  }
  if (treeError) {
    return (
      <div className="source-state source-state-error">
        <span>{t('projects.source.error')}</span>
        <button type="button" onClick={retryTree}>{t('projects.docAi.retry')}</button>
      </div>
    );
  }
  if (tree.length === 0 && exclusions.length === 0) {
    return <div className="source-state">{t('projects.source.empty')}</div>;
  }

  return (
    <div className="source-viewer">
      <aside className="source-tree-panel">
        <div className="source-search">
          <Search size={12} aria-hidden="true" />
          <input
            value={searchQuery}
            onChange={event => setSearchQuery(event.target.value)}
            onKeyDown={event => {
              if (event.key === 'Enter') {
                event.preventDefault();
                if (event.shiftKey) goToPreviousMatch(); else goToNextMatch();
              } else if (event.key === 'Escape') {
                setSearchQuery('');
              }
            }}
            placeholder={t('projects.source.search')}
            aria-label={t('projects.source.search')}
          />
          {searchLoading ? (
            <Loader2 size={11} className="spin" />
          ) : searchQuery ? (
            <button
              type="button"
              onClick={() => setSearchQuery('')}
              aria-label={t('projects.master.clear')}
            >
              <X size={11} />
            </button>
          ) : null}
          {treeHydrating && (
            <Loader2
              size={11}
              className="spin"
              aria-label={t('projects.source.loadingTreeBackground')}
            />
          )}
        </div>
        {searchQuery.trim() && !searchLoading && (
          <div className="source-search-summary">
            {totalMatches > 0 ? (
              <>
                <span>{globalMatchPosition} / {totalMatches}</span>
                <span>{t('projects.source.filesCount', searchResults.size)}</span>
                {totalMatches > 1 && (
                  <span className="source-search-nav">
                    <button type="button" onClick={goToPreviousMatch} title="Shift+Enter">
                      <ChevronUp size={10} />
                    </button>
                    <button type="button" onClick={goToNextMatch} title="Enter">
                      <ChevronDown size={10} />
                    </button>
                  </span>
                )}
              </>
            ) : t('projects.docAi.noResults')}
          </div>
        )}
        {(exclusions.length > 0 || exclusionError) && (
          <div className="source-exclusions">
            <span className="source-exclusions-label">
              <FolderX size={10} />
              {t('projects.source.exclusions', exclusions.length)}
            </span>
            {exclusions.map(path => (
              <button
                type="button"
                key={path}
                disabled={exclusionSaving !== null}
                onClick={() => void saveExclusions(
                  exclusions.filter(excluded => excluded !== path),
                  path,
                )}
                title={t('projects.source.restoreFolder', path)}
              >
                <span>{path}</span>
                {exclusionSaving === path ? <Loader2 size={9} className="spin" /> : <X size={9} />}
              </button>
            ))}
            {exclusionError && (
              <span className="source-exclusions-error">{t('projects.source.exclusionError')}</span>
            )}
          </div>
        )}
        <div className="source-tree">
          {tree.map(node => (
            <SourceTreeNode
              key={node.path}
              node={node}
              depth={0}
              selectedPath={selectedPath}
              expandedDirs={effectiveExpandedDirs}
              searchResults={searchResults}
              isSearching={searchQuery.trim().length > 0}
              onSelect={path => {
                setSelectedPath(path);
                setCurrentMatchIdx(0);
              }}
              onToggle={path => setExpandedDirs(previous => {
                const next = new Set(previous);
                if (next.has(path)) next.delete(path); else next.add(path);
                return next;
              })}
              onExclude={path => void saveExclusions([...exclusions, path], path)}
              exclusionSaving={exclusionSaving}
            />
          ))}
        </div>
      </aside>
      <section className="source-content-panel">
        {selectedPath && (
          <header className="source-toolbar">
            <span className="source-path">{selectedPath}</span>
            {language && <span className="source-language">{language}</span>}
          </header>
        )}
        <div ref={contentRef} className="source-code-scroll">
          {contentLoading ? (
            <div className="source-state">
              <Loader2 size={15} className="spin" /> {t('projects.source.loadingFile')}
            </div>
          ) : content !== null ? (
            <pre className="source-code">
              {content.split('\n').map((line, index) => {
                const blame = blameByLine.get(index + 1);
                const blameLabel = blame ? formatBlame(blame) : blameLoading ? '…' : '—';
                return (
                  <span className="source-line" key={`${index}-${line.slice(0, 12)}`}>
                    {annotate && (
                      // Keep the table cell as the layout owner and the button as
                      // its interactive child. The cell has an explicit width:
                      // a percentage width combined with this 100%-wide button
                      // has no intrinsic minimum and can otherwise collapse to
                      // ~1px, clipping the author and date.
                      <span className="source-blame" title={blame ? undefined : blameLabel}>
                        {blame ? (
                          <button
                            type="button"
                            className="source-blame-btn"
                            title={t('projects.source.blameOpenCommit', blameLabel)}
                            onClick={() => setCommitSha(blame.commit)}
                            data-testid="source-blame-button"
                          >
                            {blameLabel}
                          </button>
                        ) : (
                          blameLabel
                        )}
                      </span>
                    )}
                    <span className="source-line-number" aria-hidden="true">{index + 1}</span>
                    <code
                      className="hljs"
                      dangerouslySetInnerHTML={{
                        __html: highlightSourceLine(line, syntaxLanguage) || '&nbsp;',
                      }}
                    />
                  </span>
                );
              })}
            </pre>
          ) : (
            <div className="source-state">
              <Code2 size={18} /> {t('projects.source.select')}
            </div>
          )}
          {commitSha && (
            <div className="source-commit-detail" data-testid="source-commit-detail" role="dialog" aria-label={t('projects.source.commitTitle')}>
              <header>
                <strong>{commitDetail?.short_sha ?? commitSha.slice(0, 8)}</strong>
                <button
                  type="button"
                  onClick={() => setCommitSha(null)}
                  aria-label={t('projects.source.commitClose')}
                >
                  <X size={13} />
                </button>
              </header>
              {commitLoading && (
                <p className="source-commit-state"><Loader2 size={12} className="spin" /> {t('projects.source.commitLoading')}</p>
              )}
              {commitError && (
                <p className="source-commit-error">{t('projects.source.commitFailed', commitError)}</p>
              )}
              {commitDetail && (
                <>
                  <p className="source-commit-subject">{commitDetail.subject}</p>
                  {commitDetail.body && <pre className="source-commit-body">{commitDetail.body}</pre>}
                  <dl className="source-commit-meta">
                    <dt>{t('projects.source.commitAuthor')}</dt>
                    <dd>{commitDetail.author_name} &lt;{commitDetail.author_email}&gt;</dd>
                    <dt>{t('projects.source.commitDate')}</dt>
                    <dd>{formatCommitTime(commitDetail.author_time)}</dd>
                    <dt>{t('projects.source.commitFiles')}</dt>
                    <dd>{commitDetail.files_changed}</dd>
                    {commitDetail.branches.length > 0 && (
                      <>
                        <dt>{t('projects.source.commitBranches')}</dt>
                        <dd>
                          {commitDetail.branches.join(', ')}
                          {/* Say the list was cut rather than implying it is complete. */}
                          {commitDetail.branches_truncated && ` ${t('projects.source.commitBranchesMore')}`}
                        </dd>
                      </>
                    )}
                  </dl>
                  <code className="source-commit-sha">{commitDetail.sha}</code>
                  {onOpenCommit && (
                    // KT-75 — the popover answers "what was this commit about";
                    // this opens the change itself, parent → commit.
                    <button
                      type="button"
                      className="source-commit-open-patch"
                      onClick={() => onOpenCommit(commitDetail.sha)}
                      data-testid="source-commit-open-patch"
                    >
                      <GitCompareArrows size={12} /> {t('projects.source.commitOpenPatch')}
                    </button>
                  )}
                </>
              )}
            </div>
          )}
        </div>
        <footer className="source-footer">
          <span className="source-branch" title={t('projects.source.branch')}>
            <GitBranch size={11} />
            <span>{branch ?? '—'}</span>
          </span>
          <span className="source-annotate-zone">
            {blameError && <span className="source-blame-error">{t('projects.source.blameError')}</span>}
            <button
              type="button"
              data-active={annotate}
              disabled={!selectedPath}
              onClick={() => setAnnotate(value => !value)}
              title={t('projects.source.annotateHelp')}
            >
              {blameLoading ? <Loader2 size={11} className="spin" /> : <History size={11} />}
              {t('projects.source.annotate')}
            </button>
          </span>
        </footer>
      </section>
    </div>
  );
}

function SourceTreeNode({
  node,
  depth,
  selectedPath,
  expandedDirs,
  searchResults,
  isSearching,
  onSelect,
  onToggle,
  onExclude,
  exclusionSaving,
}: {
  node: SourceFileNode;
  depth: number;
  selectedPath: string | null;
  expandedDirs: Set<string>;
  searchResults: Map<string, number>;
  isSearching: boolean;
  onSelect: (path: string) => void;
  onToggle: (path: string) => void;
  onExclude: (path: string) => void;
  exclusionSaving: string | null;
}) {
  const { t } = useT();
  const expanded = expandedDirs.has(node.path);
  const matches = searchResults.get(node.path) ?? 0;
  if (node.is_dir) {
    return (
      <>
        <div className="source-tree-dir-wrap">
          <button
            type="button"
            className="source-tree-row source-tree-dir"
            style={{ paddingLeft: 8 + depth * 14 }}
            onClick={() => onToggle(node.path)}
          >
            {expanded ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
            <Folder size={12} />
            <span>{node.name}</span>
          </button>
          <button
            type="button"
            className="source-tree-exclude"
            disabled={exclusionSaving !== null}
            onClick={() => onExclude(node.path)}
            title={t('projects.source.excludeFolder', node.path)}
            aria-label={t('projects.source.excludeFolder', node.path)}
          >
            {exclusionSaving === node.path
              ? <Loader2 size={10} className="spin" />
              : <FolderX size={10} />}
          </button>
        </div>
        {expanded && (node.children ?? []).map(child => (
          <SourceTreeNode
            key={child.path}
            node={child}
            depth={depth + 1}
            selectedPath={selectedPath}
            expandedDirs={expandedDirs}
            searchResults={searchResults}
            isSearching={isSearching}
            onSelect={onSelect}
            onToggle={onToggle}
            onExclude={onExclude}
            exclusionSaving={exclusionSaving}
          />
        ))}
      </>
    );
  }
  return (
    <button
      type="button"
      className="source-tree-row source-tree-file"
      style={{ paddingLeft: 21 + depth * 14 }}
      data-selected={selectedPath === node.path}
      data-dimmed={isSearching && matches === 0}
      onClick={() => onSelect(node.path)}
      title={node.path}
    >
      <FileCode2 size={12} />
      <span>{node.name}</span>
      {node.git_ignored && (
        <span className="source-ignored" title="Git ignored">
          ignored
        </span>
      )}
      {matches > 0 && <span className="source-match-count">{matches}</span>}
    </button>
  );
}

function findPreferredSourceFile(nodes: SourceFileNode[]): SourceFileNode | null {
  const preferred = ['src/main.rs', 'src/lib.rs', 'src/index.ts', 'src/index.tsx', 'package.json', 'Cargo.toml'];
  const flat: SourceFileNode[] = [];
  const visit = (items: SourceFileNode[]) => items.forEach(item => {
    if (item.is_dir) visit(item.children ?? []); else flat.push(item);
  });
  visit(nodes);
  return preferred.map(path => flat.find(file => file.path === path)).find(Boolean) ?? flat[0] ?? null;
}

function hasSourcePath(nodes: SourceFileNode[], path: string): boolean {
  return nodes.some(node => (
    node.is_dir ? hasSourcePath(node.children ?? [], path) : node.path === path
  ));
}

function sourceLanguage(path: string): string {
  const fileName = path.split('/').pop() ?? path;
  if (!fileName.includes('.')) return fileName.toLowerCase();
  return fileName.split('.').pop()?.toLowerCase() ?? '';
}

function flattenSourcePaths(nodes: SourceFileNode[], paths: string[]) {
  nodes.forEach(node => {
    if (node.is_dir) flattenSourcePaths(node.children ?? [], paths);
    else paths.push(node.path);
  });
}

/** Full date+time for a commit detail — the gutter shows a short date, but
 *  "which commit exactly" often hinges on the hour. */
function formatCommitTime(epochSeconds: number): string {
  if (epochSeconds <= 0) return '—';
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: 'medium',
    timeStyle: 'short',
  }).format(new Date(epochSeconds * 1000));
}

function formatBlame(line: GitBlameLine): string {
  const date = line.author_time > 0
    ? new Intl.DateTimeFormat(undefined, { day: '2-digit', month: 'short', year: '2-digit' })
      .format(new Date(line.author_time * 1000))
    : '—';
  return `${line.author} · ${date}`;
}

const SOURCE_HIGHLIGHT_ATTR = 'data-source-hl';

function removeSourceHighlights(container: HTMLElement) {
  container.querySelectorAll(`mark[${SOURCE_HIGHLIGHT_ATTR}]`).forEach(mark => {
    const parent = mark.parentNode;
    if (!parent) return;
    parent.replaceChild(document.createTextNode(mark.textContent ?? ''), mark);
    parent.normalize();
  });
}

function applySourceHighlights(container: HTMLElement, query: string, activeIndex: number): number {
  const lowerQuery = query.toLowerCase();
  const matches: Array<{ node: Text; start: number }> = [];
  container.querySelectorAll('.source-line code').forEach(code => {
    const walker = document.createTreeWalker(code, NodeFilter.SHOW_TEXT);
    while (walker.nextNode()) {
      const node = walker.currentNode as Text;
      const lower = (node.textContent ?? '').toLowerCase();
      let offset = lower.indexOf(lowerQuery);
      while (offset >= 0) {
        matches.push({ node, start: offset });
        offset = lower.indexOf(lowerQuery, offset + lowerQuery.length);
      }
    }
  });
  if (matches.length === 0) return 0;
  const safeActive = activeIndex % matches.length;
  const grouped = new Map<Text, Array<{ start: number; index: number }>>();
  matches.forEach((match, index) => {
    const entries = grouped.get(match.node) ?? [];
    entries.push({ start: match.start, index });
    grouped.set(match.node, entries);
  });
  for (const [node, entries] of grouped) {
    entries.sort((a, b) => b.start - a.start).forEach(({ start, index }) => {
      const text = node.textContent ?? '';
      if (start + query.length > text.length) return;
      node.splitText(start + query.length);
      const matchNode = node.splitText(start);
      const mark = document.createElement('mark');
      mark.setAttribute(SOURCE_HIGHLIGHT_ATTR, String(index));
      mark.dataset.active = String(index === safeActive);
      matchNode.parentNode?.replaceChild(mark, matchNode);
      mark.appendChild(matchNode);
    });
  }
  return matches.length;
}
