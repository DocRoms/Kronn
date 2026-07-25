import { useCallback, useMemo, useState } from 'react';
import {
  Code2, FileCode2, FileEdit, GitCompareArrows, Loader2, RefreshCw,
} from 'lucide-react';
import { projects as projectsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import type { GitFileStatus, GitStatusResponse } from '../types/generated';
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
  const [mode, setMode] = useState<'source' | 'diff'>('source');
  const [status, setStatus] = useState<GitStatusResponse | null>(null);
  const [statusLoading, setStatusLoading] = useState(false);
  const [statusError, setStatusError] = useState(false);
  const [selected, setSelected] = useState<SelectedDiff | null>(null);
  const [diff, setDiff] = useState('');
  const [diffLoading, setDiffLoading] = useState(false);

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

      {mode === 'source' ? (
        <SourceCodeViewer projectId={projectId} />
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
