import { useCallback, useMemo, useRef, useState } from 'react';
import {
  Code2, FileCode2, FileEdit, GitCommitHorizontal, GitCompareArrows, Loader2, RefreshCw, X,
} from 'lucide-react';
import { projects as projectsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import type { GitCommitPatch, GitFileStatus, GitStatusResponse } from '../types/generated';
import { splitPatchByFile } from '../lib/commit-patch';
import { GitDiffViewer } from './GitDiffViewer';
import { SourceCodeViewer } from './SourceCodeViewer';
import './GitPanel.css';
import './ProjectCodePanel.css';

interface ProjectCodePanelProps {
  projectId: string;
}

interface SelectedDiff {
  path: string;
  committed: boolean;
}

export function ProjectCodePanel({ projectId }: ProjectCodePanelProps) {
  const { t } = useT();
  const [mode, setMode] = useState<'source' | 'diff' | 'commit'>('source');
  // KT-75 — ONE temporary commit tab, deliberately: opening another commit
  // replaces it. A stack of eight hashes is navigation debt, not a feature.
  const [commitSha, setCommitSha] = useState<string | null>(null);
  const [commitPatch, setCommitPatch] = useState<GitCommitPatch | null>(null);
  const [commitLoading, setCommitLoading] = useState(false);
  const [commitError, setCommitError] = useState<string | null>(null);
  const commitRequestRef = useRef(0);
  // KT-87 — which file of the commit patch is on screen. Reset per commit.
  const [activeCommitPath, setActiveCommitPath] = useState<string | null>(null);
  const [status, setStatus] = useState<GitStatusResponse | null>(null);
  const [statusLoading, setStatusLoading] = useState(false);
  const [statusError, setStatusError] = useState(false);
  const [selected, setSelected] = useState<SelectedDiff | null>(null);
  const [diff, setDiff] = useState('');
  const [diffLoading, setDiffLoading] = useState(false);

  const commitFiles = useMemo(
    () => splitPatchByFile(commitPatch?.patch ?? ''),
    [commitPatch],
  );
  // Falls back to the first file so the pane is never blank after a load, and
  // survives a selection that no longer exists (another commit opened).
  const activeCommitFile = useMemo(
    () => commitFiles.find(f => f.path === activeCommitPath) ?? commitFiles[0],
    [commitFiles, activeCommitPath],
  );

  const allFiles = useMemo(() => {
    if (!status) return [];
    return [
      ...status.files.map(file => ({ file, committed: false })),
      ...status.committed_files.map(file => ({ file, committed: true })),
    ];
  }, [status]);

  const loadDiff = useCallback(async (next: SelectedDiff) => {
    setSelected(next);
    setDiffLoading(true);
    try {
      const response = await projectsApi.gitDiff(projectId, next.path, next.committed);
      setDiff(response.diff);
    } catch {
      setDiff(t('projects.code.loadError'));
    } finally {
      setDiffLoading(false);
    }
  }, [projectId, t]);

  const loadStatus = useCallback(async () => {
    setStatusLoading(true);
    setStatusError(false);
    try {
      const response = await projectsApi.gitStatus(projectId);
      setStatus(response);
      const first = response.files[0]
        ? { path: response.files[0].path, committed: false }
        : response.committed_files[0]
          ? { path: response.committed_files[0].path, committed: true }
          : null;
      if (first) await loadDiff(first);
      else {
        setSelected(null);
        setDiff('');
      }
    } catch {
      setStatusError(true);
    } finally {
      setStatusLoading(false);
    }
  }, [loadDiff, projectId]);

  const openCommit = useCallback(async (sha: string) => {
    // Open A, open B, then A answers last: without this generation the patch of
    // A would render under B's tab. Closing bumps it too, so a request in flight
    // cannot repopulate a tab the user just dismissed.
    const generation = commitRequestRef.current + 1;
    commitRequestRef.current = generation;
    setCommitSha(sha);
    setMode('commit');
    setCommitPatch(null);
    setCommitError(null);
    setActiveCommitPath(null);
    setCommitLoading(true);
    try {
      const patch = await projectsApi.gitCommitPatch(projectId, sha);
      if (commitRequestRef.current !== generation) return;
      setCommitPatch(patch);
    } catch (error) {
      if (commitRequestRef.current !== generation) return;
      setCommitError(String(error));
    } finally {
      if (commitRequestRef.current === generation) setCommitLoading(false);
    }
  }, [projectId]);

  const closeCommit = useCallback(() => {
    commitRequestRef.current += 1;
    setCommitSha(null);
    setCommitPatch(null);
    setCommitError(null);
    setCommitLoading(false);
    setMode('source');
  }, []);

  const renderFiles = (
    label: string,
    files: GitFileStatus[],
    committed: boolean,
  ) => files.length > 0 && (
    <section className="project-code-diff-group">
      <h4>{label} <span>{files.length}</span></h4>
      {files.map(file => (
        <button
          type="button"
          key={`${committed ? 'committed' : 'working'}-${file.path}`}
          data-active={selected?.path === file.path && selected.committed === committed}
          onClick={() => void loadDiff({ path: file.path, committed })}
          title={file.path}
        >
          <FileEdit size={12} />
          <span>{file.path}</span>
          <small>{file.status}</small>
        </button>
      ))}
    </section>
  );

  return (
    <div className="project-code-panel">
      <div className="project-code-toolbar">
        <div className="project-code-modes" role="group" aria-label={t('projects.code.view')}>
          <button
            type="button"
            data-active={mode === 'source'}
            onClick={() => setMode('source')}
          >
            <Code2 size={13} /> {t('projects.code.source')}
          </button>
          <button
            type="button"
            data-active={mode === 'diff'}
            onClick={() => {
              setMode('diff');
              if (status === null && !statusLoading) void loadStatus();
            }}
          >
            <GitCompareArrows size={13} /> {t('projects.code.changes')}
            {allFiles.length > 0 && <span>{allFiles.length}</span>}
          </button>
          {commitSha && (
            <span className="project-code-commit-tab" data-testid="project-code-commit-tab">
              <button
                type="button"
                data-active={mode === 'commit'}
                onClick={() => setMode('commit')}
                title={t('projects.code.commitTabHint', commitSha)}
              >
                <GitCommitHorizontal size={13} />
                {commitPatch?.short_sha ?? commitSha.slice(0, 7)}
              </button>
              <button
                type="button"
                className="project-code-commit-close"
                onClick={closeCommit}
                aria-label={t('projects.code.commitTabClose')}
                data-testid="project-code-commit-close"
              >
                <X size={11} />
              </button>
            </span>
          )}
        </div>
        {mode === 'diff' && (
          <button
            type="button"
            className="dash-icon-btn"
            onClick={() => void loadStatus()}
            disabled={statusLoading}
            aria-label={t('projects.code.refresh')}
          >
            <RefreshCw size={13} className={statusLoading ? 'spin' : undefined} />
          </button>
        )}
      </div>

      {mode === 'commit' ? (
        <div className="project-code-commit-view" data-testid="project-code-commit-view">
          <header>
            <GitCommitHorizontal size={13} />
            <span>{commitPatch?.subject ?? commitSha}</span>
            {commitPatch && (
              <small>
                {t('projects.code.commitFiles', commitPatch.files_changed)}
                {commitPatch.is_root && ` · ${t('projects.code.commitRoot')}`}
                {commitPatch.truncated && ` · ${t('projects.code.commitTruncated')}`}
              </small>
            )}
          </header>
          {commitError ? (
            <div className="project-code-state">{t('projects.code.commitLoadError')}</div>
          ) : !commitLoading && commitPatch && commitPatch.patch.trim() === '' ? (
            <div className="project-code-state">{t('projects.code.commitEmpty')}</div>
          ) : (
            // KT-87 — one file at a time. Rendering the whole patch meant
            // scrolling 196 000 px to reach a single file on a big commit, and
            // forced a single language on every file at once.
            <div className="project-code-commit-layout">
              <div className="project-code-commit-file">
                <GitDiffViewer
                  path={activeCommitFile?.path ?? ''}
                  content={activeCommitFile?.body ?? ''}
                  loading={commitLoading}
                />
              </div>
              {commitFiles.length > 0 && (
                <aside className="project-code-commit-files" data-testid="project-code-commit-files">
                  {commitFiles.map(file => (
                    <button
                      type="button"
                      key={file.path}
                      data-active={file.path === activeCommitPath}
                      onClick={() => setActiveCommitPath(file.path)}
                      title={file.path}
                    >
                      <FileEdit size={12} />
                      {/* U+200E keeps the path LTR inside the RTL box the CSS
                          uses to clip directories instead of the file name —
                          without it a leading dot (`.env`) jumps to the end. */}
                      <span>{'\u200e'}{file.path}</span>
                      <small>
                        {file.binary
                          ? t('projects.code.commitFileBinary')
                          : `+${file.added} −${file.removed}`}
                      </small>
                    </button>
                  ))}
                </aside>
              )}
            </div>
          )}
        </div>
      ) : mode === 'source' ? (
        <SourceCodeViewer projectId={projectId} onOpenCommit={sha => void openCommit(sha)} />
      ) : statusLoading && !status ? (
        <div className="project-code-state"><Loader2 size={18} className="spin" /></div>
      ) : statusError && !status ? (
        <div className="project-code-state">{t('projects.code.loadError')}</div>
      ) : !status || allFiles.length === 0 ? (
        <div className="project-code-state">
          <FileCode2 size={22} />
          <span>{t('projects.code.empty')}</span>
        </div>
      ) : (
        <div className="project-code-diff-layout">
          <div className="project-code-diff-view">
            <header>
              <GitCompareArrows size={13} />
              <span>{selected?.path ?? t('projects.code.changes')}</span>
            </header>
            <GitDiffViewer
              path={selected?.path ?? ''}
              content={diff}
              loading={diffLoading}
            />
          </div>
          <aside className="project-code-diff-files">
            {renderFiles(t('projects.code.uncommitted'), status.files, false)}
            {renderFiles(t('projects.code.committed'), status.committed_files, true)}
          </aside>
        </div>
      )}
    </div>
  );
}
