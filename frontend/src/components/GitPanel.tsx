import { useState, useEffect, useCallback, useRef } from 'react';
import { projects as projectsApi, discussions as discussionsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import ReactMarkdown from 'react-markdown';
import './DiscussionToolPanel.css';
import './GitPanel.css';
import {
  GitBranch, GitCommit, GitPullRequest, Upload, RefreshCw, ChevronLeft,
  FileEdit, FilePlus, FileMinus, FileX, AlertTriangle, ExternalLink,
  Loader2, Check, X, Terminal, Maximize2, Minimize2,
} from 'lucide-react';
import { GitDiffViewer } from './GitDiffViewer';

// ─── Types (mirrors backend GitStatusResponse / GitDiffResponse) ─────────────

interface GitFile {
  path: string;
  status: string; // M, A, D, R, ?, etc.
  staged: boolean;
}

interface GitStatus {
  branch: string;
  default_branch: string;
  is_default_branch: boolean;
  files: GitFile[];
  committed_files?: GitFile[];
  ahead: number;
  behind: number;
  has_upstream: boolean;
  provider: string;  // "github", "gitlab", "unknown"
  pr_url?: string | null;
}

interface Props {
  projectId?: string;
  discussionId?: string;
  onClose: () => void;
  onExpandedChange?: (expanded: boolean) => void;
  terminalEnabled?: boolean;
}

const STATUS_ICONS: Record<string, typeof FileEdit> = {
  modified: FileEdit,
  added: FilePlus,
  deleted: FileMinus,
  renamed: FileEdit,
  copied: FileEdit,
  untracked: FilePlus,
};

const STATUS_COLORS: Record<string, string> = {
  modified: 'var(--kr-warning-soft)',
  added: 'var(--kr-success)',
  deleted: 'var(--kr-error)',
  renamed: 'var(--kr-info)',
  copied: 'var(--kr-info)',
  untracked: 'var(--kr-text-dim)',
};

function ExpandedFileButton({
  file,
  active,
  committed = false,
  onClick,
}: {
  file: GitFile;
  active: boolean;
  committed?: boolean;
  onClick: () => void;
}) {
  const Icon = STATUS_ICONS[file.status] || FileX;
  const color = STATUS_COLORS[file.status] || 'var(--kr-text-faint)';
  return (
    <button
      type="button"
      className="git-expanded-file"
      data-active={active}
      data-committed={committed}
      onClick={onClick}
      title={file.path}
    >
      <Icon size={12} style={{ color }} />
      <span>{file.path}</span>
      <small style={{ color }}>{file.status}</small>
    </button>
  );
}

export function GitPanel({
  projectId,
  discussionId,
  onClose,
  onExpandedChange,
  terminalEnabled = false,
}: Props) {
  const { t } = useT();
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');

  // Diff view
  const [diffPath, setDiffPath] = useState<string | null>(null);
  const [diffCommitted, setDiffCommitted] = useState(false);
  const [diffContent, setDiffContent] = useState('');
  const [diffLoading, setDiffLoading] = useState(false);
  const [expanded, setExpanded] = useState(false);

  // Branch creation
  const [showBranch, setShowBranch] = useState(false);
  const [branchName, setBranchName] = useState('');
  const [branchLoading, setBranchLoading] = useState(false);

  // Commit
  const [showCommit, setShowCommit] = useState(false);
  const [commitMsg, setCommitMsg] = useState('');
  const [selectedFiles, setSelectedFiles] = useState<string[]>([]);
  const [commitLoading, setCommitLoading] = useState(false);
  const [commitAmend, setCommitAmend] = useState(false);
  const [commitSign, setCommitSign] = useState(false);

  // Push
  const [pushLoading, setPushLoading] = useState(false);
  const [pushResult, setPushResult] = useState<string | null>(null);

  // PR form
  const [showPrForm, setShowPrForm] = useState(false);
  const [prTitle, setPrTitle] = useState('');
  const [prBody, setPrBody] = useState('');
  const [prPreview, setPrPreview] = useState(false);
  const [prTemplateSource, setPrTemplateSource] = useState('');
  const [prLoading, setPrLoading] = useState(false);

  // Terminal
  const [showTerminal, setShowTerminal] = useState(false);
  const [termInput, setTermInput] = useState('');
  const [termHistory, setTermHistory] = useState<{ cmd: string; stdout: string; stderr: string; code: number }[]>([]);
  const [termLoading, setTermLoading] = useState(false);
  const termEndRef = useRef<HTMLDivElement>(null);

  const fetchStatus = useCallback(async () => {
    setLoading(true);
    setError('');
    try {
      const res = discussionId
        ? await discussionsApi.gitStatus(discussionId)
        : projectId
          ? await projectsApi.gitStatus(projectId)
          : null;
      if (res) setStatus(res);
      else setError('No project or discussion ID');
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [projectId, discussionId]);

  useEffect(() => { fetchStatus(); }, [fetchStatus]);

  const openDiff = useCallback(async (path: string, committed = false) => {
    setDiffPath(path);
    setDiffCommitted(committed);
    setDiffLoading(true);
    try {
      let res;
      if (discussionId) {
        res = await discussionsApi.gitDiff(discussionId, path, committed);
      } else if (projectId) {
        res = await projectsApi.gitDiff(projectId, path, committed);
      } else {
        return; // GitPanel always mounted with one or the other; defensive.
      }
      setDiffContent(res.diff);
    } catch (e) {
      setDiffContent(`Error: ${e}`);
    } finally {
      setDiffLoading(false);
    }
  }, [discussionId, projectId]);

  const toggleExpanded = () => {
    setExpanded(value => {
      const next = !value;
      onExpandedChange?.(next);
      return next;
    });
  };

  useEffect(() => () => {
    onExpandedChange?.(false);
  }, [onExpandedChange]);

  useEffect(() => {
    if (!expanded || diffPath || !status) return;
    const firstWorkingFile = status.files[0];
    const firstCommittedFile = status.committed_files?.[0];
    if (firstWorkingFile) {
      void openDiff(firstWorkingFile.path);
    } else if (firstCommittedFile) {
      void openDiff(firstCommittedFile.path, true);
    }
  }, [diffPath, expanded, openDiff, status]);

  const handleCreateBranch = async () => {
    if (!branchName.trim() || !projectId) return;
    setBranchLoading(true);
    try {
      await projectsApi.gitCreateBranch(projectId, { name: branchName.trim() });
      setShowBranch(false);
      setBranchName('');
      await fetchStatus();
    } catch (e) {
      setError(String(e));
    } finally {
      setBranchLoading(false);
    }
  };

  const handleCommit = async () => {
    if (!commitMsg.trim() || selectedFiles.length === 0) return;
    setCommitLoading(true);
    try {
      const commitReq = { files: selectedFiles, message: commitMsg.trim(), amend: commitAmend, sign: commitSign };
      if (discussionId) {
        await discussionsApi.gitCommit(discussionId, commitReq);
      } else if (projectId) {
        await projectsApi.gitCommit(projectId, commitReq);
      } else {
        return;
      }
      setShowCommit(false);
      setCommitMsg('');
      setSelectedFiles([]);
      setCommitAmend(false);
      await fetchStatus();
    } catch (e) {
      setError(String(e));
    } finally {
      setCommitLoading(false);
    }
  };

  const handlePush = async () => {
    setPushLoading(true);
    setPushResult(null);
    try {
      if (discussionId) {
        await discussionsApi.gitPush(discussionId);
      } else if (projectId) {
        await projectsApi.gitPush(projectId);
      } else {
        return;
      }
      setPushResult('success');
      await fetchStatus();
    } catch (e) {
      setPushResult(String(e));
    } finally {
      setPushLoading(false);
    }
  };

  const openPrForm = async () => {
    if (!status) return;
    const api = discussionId ? discussionsApi : projectsApi;
    const id = discussionId || projectId;
    if (!id) return;
    // Auto-fill title from branch name
    setPrTitle(status.branch.replace('kronn/', '').replace(/-/g, ' '));
    setPrPreview(false);
    setShowPrForm(true);
    // Fetch template
    try {
      const res = await api.prTemplate(id);
      setPrBody(res.template);
      setPrTemplateSource(res.source);
    } catch {
      setPrBody('');
      setPrTemplateSource('');
    }
  };

  const handleCreatePr = async () => {
    if (!prTitle.trim()) return;
    setPrLoading(true);
    try {
      const api = discussionId ? discussionsApi : projectsApi;
      const id = discussionId || projectId;
      if (!id) return;
      // Auto-push if branch has no upstream yet
      if (status && !status.has_upstream) {
        await api.gitPush(id);
      }
      const res = await api.createPr(id, {
        title: prTitle.trim(),
        body: prBody.trim(),
        base: status?.default_branch || 'main',
      });
      setPushResult(`PR: ${res.url}`);
      setShowPrForm(false);
      await fetchStatus();
    } catch (e) {
      setPushResult(String(e));
    } finally {
      setPrLoading(false);
    }
  };

  const toggleFile = (path: string) => {
    setSelectedFiles(prev =>
      prev.includes(path) ? prev.filter(f => f !== path) : [...prev, path]
    );
  };

  const selectAll = () => {
    if (!status) return;
    if (selectedFiles.length === status.files.length) {
      setSelectedFiles([]);
    } else {
      setSelectedFiles(status.files.map(f => f.path));
    }
  };

  const handleExec = async () => {
    const cmd = termInput.trim();
    if (!cmd || termLoading) return;
    setTermLoading(true);
    setTermInput('');
    try {
      let res;
      if (discussionId) {
        res = await discussionsApi.exec(discussionId, cmd);
      } else if (projectId) {
        res = await projectsApi.exec(projectId, cmd);
      } else {
        return;
      }
      setTermHistory(prev => [...prev, { cmd, stdout: res.stdout, stderr: res.stderr, code: res.exit_code }]);
    } catch (e) {
      setTermHistory(prev => [...prev, { cmd, stdout: '', stderr: String(e), code: 1 }]);
    } finally {
      setTermLoading(false);
      setTimeout(() => termEndRef.current?.scrollIntoView({ behavior: 'smooth' }), 50);
    }
  };

  // ─── Diff view ──────────────────────────────────────────────────────────────
  if (diffPath) {
    return (
      <aside
        className="disc-tool-panel git-panel"
        data-expanded={expanded}
        aria-label={t('git.title')}
      >
        <header className="disc-tool-panel-header git-header">
          <button
            className="disc-tool-panel-icon git-back-btn"
            onClick={() => setDiffPath(null)}
            aria-label={t('git.back')}
          >
            <ChevronLeft size={14} />
          </button>
          <span className="disc-tool-panel-title git-header-title">{diffPath}</span>
          <div className="disc-tool-panel-actions">
            <button
              type="button"
              className="disc-tool-panel-icon"
              onClick={toggleExpanded}
              title={expanded ? t('git.collapsePanel') : t('git.expandPanel')}
              aria-label={expanded ? t('git.collapsePanel') : t('git.expandPanel')}
              aria-pressed={expanded}
            >
              {expanded ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
            </button>
            <button
              className="disc-tool-panel-icon git-close-btn"
              onClick={onClose}
              aria-label={t('common.close')}
            >
              <X size={14} />
            </button>
          </div>
        </header>
        {expanded && status ? (
          <div className="git-expanded-layout">
            <GitDiffViewer
              path={diffPath}
              content={diffContent}
              loading={diffLoading}
              className="git-expanded-preview"
            />
            <aside className="git-expanded-files" aria-label={t('git.changedFilesList')}>
              <div className="git-expanded-files-head">
                <strong>{t('git.changedFilesList')}</strong>
                <span>{status.files.length + (status.committed_files?.length ?? 0)}</span>
              </div>
              <div className="git-expanded-file-list">
                {status.files.map(file => (
                  <ExpandedFileButton
                    key={file.path}
                    file={file}
                    active={!diffCommitted && diffPath === file.path}
                    onClick={() => void openDiff(file.path)}
                  />
                ))}
                {(status.committed_files?.length ?? 0) > 0 && (
                  <div className="git-expanded-file-group">{t('git.committedChanges')}</div>
                )}
                {status.committed_files?.map(file => (
                  <ExpandedFileButton
                    key={`committed-${file.path}`}
                    file={file}
                    active={diffCommitted && diffPath === file.path}
                    committed
                    onClick={() => void openDiff(file.path, true)}
                  />
                ))}
              </div>
            </aside>
          </div>
        ) : (
          <GitDiffViewer path={diffPath} content={diffContent} loading={diffLoading} />
        )}
      </aside>
    );
  }

  // ─── Main view ──────────────────────────────────────────────────────────────
  return (
    <aside
      className="disc-tool-panel git-panel"
      data-expanded={expanded}
      aria-label={t('git.title')}
    >
      {/* Header */}
      <header className="disc-tool-panel-header git-header">
        <span className="disc-tool-panel-title git-header-title">
          <GitBranch size={13} style={{ marginRight: 6 }} />
          {t('git.title')}
        </span>
        <div className="disc-tool-panel-actions git-header-actions">
          <button
            type="button"
            className="disc-tool-panel-icon"
            onClick={toggleExpanded}
            title={expanded ? t('git.collapsePanel') : t('git.expandPanel')}
            aria-label={expanded ? t('git.collapsePanel') : t('git.expandPanel')}
            aria-pressed={expanded}
          >
            {expanded ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
          </button>
          <button className="disc-tool-panel-icon git-icon-btn" onClick={fetchStatus} title={t('git.refresh')} aria-label={t('git.refresh')}>
            <RefreshCw size={12} />
          </button>
          <button className="disc-tool-panel-icon git-close-btn" onClick={onClose} aria-label="Close git panel"><X size={14} /></button>
        </div>
      </header>

      {loading && (
        <div className="git-center"><Loader2 size={16} style={{ animation: 'spin 1s linear infinite' }} /></div>
      )}

      {error && (
        <div className="git-error">{error}</div>
      )}

      {status && !loading && (
        <div className="git-body">
          {/* Branch info */}
          <div className="git-branch-bar">
            <GitBranch size={12} />
            <span className="font-semibold">{status.branch || 'HEAD'}</span>
            {status.ahead > 0 && <span className="git-badge">↑{status.ahead}</span>}
            {status.behind > 0 && <span className="git-badge git-badge-behind">↓{status.behind}</span>}
          </div>

          {/* Warning: on default branch */}
          {status.is_default_branch && status.files.length > 0 && (
            <div className="git-warning">
              <AlertTriangle size={12} />
              <span>{t('git.onDefaultBranch')}</span>
              <button
                className="git-small-btn"
                onClick={() => setShowBranch(true)}
              >
                <GitBranch size={10} /> {t('git.createBranch')}
              </button>
            </div>
          )}

          {/* Create branch form */}
          {showBranch && (
            <div className="git-form-row">
              <input
                className="git-input"
                placeholder={t('git.branchName')}
                value={branchName}
                onChange={e => setBranchName(e.target.value)}
                onKeyDown={e => e.key === 'Enter' && handleCreateBranch()}
                autoFocus
              />
              <button className="git-action-btn" onClick={handleCreateBranch} disabled={branchLoading}>
                {branchLoading ? <Loader2 size={12} style={{ animation: 'spin 1s linear infinite' }} /> : <Check size={12} />}
              </button>
              <button className="git-icon-btn" onClick={() => setShowBranch(false)} aria-label="Cancel branch creation"><X size={12} /></button>
            </div>
          )}

          {/* Actions bar: push, create PR — always visible when applicable */}
          <div className="git-actions-bar">
            {status.ahead > 0 && (
              <button className="git-small-btn git-small-btn-push" onClick={handlePush} disabled={pushLoading}>
                {pushLoading ? <Loader2 size={10} style={{ animation: 'spin 1s linear infinite' }} /> : <Upload size={10} />}
                {t('git.push')}
              </button>
            )}
            {/* Create PR/MR button: show when on a non-default branch and no PR exists */}
            {!status.is_default_branch && !status.pr_url && !showPrForm && (
              <button className="git-small-btn git-small-btn-pr" onClick={openPrForm}>
                <GitPullRequest size={10} />
                {status.provider === 'gitlab' ? t('git.createMr') : t('git.createPr')}
              </button>
            )}
          </div>

          {/* PR link */}
          {status.pr_url && (
            <div className="git-pr-link-row">
              <GitPullRequest size={11} className="flex-shrink-0" />
              <a href={status.pr_url} target="_blank" rel="noopener noreferrer" className="git-pr-link">
                {status.pr_url.replace('https://github.com/', '').replace('https://gitlab.com/', '')}
              </a>
              <ExternalLink size={9} className="flex-shrink-0 text-dim" />
            </div>
          )}

          {/* PR creation form */}
          {showPrForm && (
            <div className="git-pr-form">
              <div className="git-pr-form-header">
                <span className="git-pr-form-title">
                  <GitPullRequest size={11} /> {status?.provider === 'gitlab' ? t('git.createMr') : t('git.createPr')}
                </span>
                <div className="flex-row gap-1">
                  {prTemplateSource && (
                    <span className="git-pr-template-source">
                      {prTemplateSource === 'project' ? t('git.prTemplateProject') : t('git.prTemplateKronn')}
                    </span>
                  )}
                  <button className="git-icon-btn" onClick={() => setShowPrForm(false)} aria-label="Close PR form"><X size={10} /></button>
                </div>
              </div>
              <input
                className="git-input mb-3 w-full"
                value={prTitle}
                onChange={e => setPrTitle(e.target.value)}
                placeholder={t('git.prTitle')}
                autoFocus
                style={{ boxSizing: 'border-box' }}
              />
              <div className="git-pr-tab-group">
                <button
                  className="git-pr-tab"
                  data-active={!prPreview}
                  onClick={() => setPrPreview(false)}
                >
                  {t('git.prEdit')}
                </button>
                <button
                  className="git-pr-tab"
                  data-active={prPreview}
                  onClick={() => setPrPreview(true)}
                >
                  {t('git.prPreview')}
                </button>
              </div>
              {prPreview ? (
                <div className="git-pr-preview">
                  <ReactMarkdown>{prBody || '*No description*'}</ReactMarkdown>
                </div>
              ) : (
                <textarea
                  className="git-input git-pr-body-textarea"
                  value={prBody}
                  onChange={e => setPrBody(e.target.value)}
                  placeholder={t('git.prBodyPlaceholder')}
                />
              )}
              <button
                className="git-action-btn git-pr-submit-btn"
                onClick={handleCreatePr}
                disabled={prLoading || !prTitle.trim()}
              >
                {prLoading ? <Loader2 size={12} style={{ animation: 'spin 1s linear infinite' }} /> : <GitPullRequest size={12} />}
                {status?.provider === 'gitlab' ? t('git.submitMr') : t('git.submitPr')}
              </button>
            </div>
          )}

          {pushResult && (
            <div className={pushResult === 'success' || pushResult.startsWith('PR:') ? 'git-success' : 'git-error'}>
              {pushResult === 'success' ? t('git.pushSuccess') : pushResult.startsWith('PR:') ? pushResult.replace('PR: ', '\u2713 PR created: ') : pushResult}
            </div>
          )}

          {/* File list */}
          {status.files.length === 0 && (!status.committed_files || status.committed_files.length === 0) && (
            <div className="git-empty">{t('git.noChanges')}</div>
          )}
          {status.files.length > 0 && (
            <>
              <div className="git-file-header">
                <span className="git-file-count">
                  {status.files.length} {t('git.filesChanged')}
                </span>
                <div className="flex-row gap-2">
                  {!showCommit && (
                    <button className="git-small-btn" onClick={() => { setShowCommit(true); setSelectedFiles(status.files.map(f => f.path)); }}>
                      <GitCommit size={10} /> {t('git.commit')}
                    </button>
                  )}
                </div>
              </div>

              {/* Commit form */}
              {showCommit && (
                <div className="git-commit-form">
                  <div className="flex-between mb-3">
                    <button className="git-link-btn text-xs" onClick={selectAll}>
                      {selectedFiles.length === status.files.length ? t('git.deselectAll') : t('git.selectAll')}
                    </button>
                    <button className="git-icon-btn" onClick={() => setShowCommit(false)} aria-label="Cancel commit"><X size={10} /></button>
                  </div>
                  <input
                    className="git-input"
                    placeholder={t('git.commitMessage')}
                    value={commitMsg}
                    onChange={e => setCommitMsg(e.target.value)}
                    onKeyDown={e => e.key === 'Enter' && handleCommit()}
                    autoFocus
                  />
                  <div className="git-commit-options">
                    <label className="git-commit-option-label">
                      <input type="checkbox" checked={commitAmend} onChange={e => setCommitAmend(e.target.checked)} style={{ accentColor: 'var(--kr-accent-ink)' }} />
                      {t('git.amend')}
                    </label>
                    <label className="git-commit-option-label">
                      <input type="checkbox" checked={commitSign} onChange={e => setCommitSign(e.target.checked)} style={{ accentColor: 'var(--kr-accent-ink)' }} />
                      {t('git.sign')}
                    </label>
                  </div>
                  <button
                    className="git-action-btn mt-4 w-full"
                    style={{ justifyContent: 'center' }}
                    onClick={handleCommit}
                    disabled={commitLoading || !commitMsg.trim() || selectedFiles.length === 0}
                  >
                    {commitLoading ? <Loader2 size={12} style={{ animation: 'spin 1s linear infinite' }} /> : <GitCommit size={12} />}
                    {t('git.commitSelected', String(selectedFiles.length))}
                  </button>
                </div>
              )}

              <div className="git-file-list">
                {status.files.map(file => {
                  const Icon = STATUS_ICONS[file.status] || FileX;
                  const color = STATUS_COLORS[file.status] || 'var(--kr-text-faint)';
                  return (
                    <div key={file.path} className="git-file-row">
                      {showCommit && (
                        <input
                          type="checkbox"
                          checked={selectedFiles.includes(file.path)}
                          onChange={() => toggleFile(file.path)}
                          style={{ marginRight: 6, accentColor: 'var(--kr-accent-ink)' }}
                        />
                      )}
                      <Icon size={12} style={{ color }} className="flex-shrink-0" />
                      <button
                        className="git-file-btn"
                        onClick={() => openDiff(file.path)}
                        title={file.path}
                      >
                        {file.path}
                      </button>
                      <span className="git-file-status" style={{ color }}>{file.status}</span>
                    </div>
                  );
                })}
              </div>
            </>
          )}

          {/* Committed-on-branch section (vs default branch).
              Surfaces work that's saved on this branch but not yet merged
              — critical for worktree-isolated discs where the agent has
              committed but the working tree is clean. */}
          {status.committed_files && status.committed_files.length > 0 && (
            <div className="git-committed-section" data-testid="git-committed-section">
              <div className="git-file-header">
                <span className="git-file-count">
                  <GitCommit size={11} style={{ marginRight: 4 }} />
                  {t('git.committedOnBranch', String(status.committed_files.length), status.default_branch || 'main')}
                </span>
              </div>
              <div className="git-file-list">
                {status.committed_files.map(file => {
                  const Icon = STATUS_ICONS[file.status] || FileX;
                  const color = STATUS_COLORS[file.status] || 'var(--kr-text-faint)';
                  return (
                    <div key={`committed-${file.path}`} className="git-file-row git-file-row-committed">
                      <Icon size={12} style={{ color }} className="flex-shrink-0" />
                      <button
                        type="button"
                        className="git-file-btn"
                        title={t('git.viewCommittedDiff', file.path)}
                        onClick={() => openDiff(file.path, true)}
                      >
                        {file.path}
                      </button>
                      <span className="git-file-status" style={{ color }}>{file.status}</span>
                    </div>
                  );
                })}
              </div>
            </div>
          )}
        </div>
      )}

      {/* Mini Terminal */}
      {terminalEnabled && (
        <div className="git-term-section">
          <button
            className="git-term-toggle"
            onClick={() => setShowTerminal(prev => !prev)}
          >
            <Terminal size={11} />
            <span>{t('git.terminal')}</span>
          </button>
          {showTerminal && (
            <div className="git-term-body">
              <div className="git-term-output">
                {termHistory.map((entry, i) => (
                  <div key={i}>
                    <div className="git-term-cmd">$ {entry.cmd}</div>
                    {entry.stdout && <pre className="git-term-pre">{entry.stdout}</pre>}
                    {entry.stderr && <pre className={`git-term-pre ${entry.code !== 0 ? 'git-term-pre-error' : 'git-term-pre-warning'}`}>{entry.stderr}</pre>}
                  </div>
                ))}
                <div ref={termEndRef} />
              </div>
              <div className="git-term-input-row">
                <span className="git-term-prompt">$</span>
                <input
                  className="git-term-input"
                  value={termInput}
                  onChange={e => setTermInput(e.target.value)}
                  onKeyDown={e => e.key === 'Enter' && handleExec()}
                  placeholder={t('git.terminalPlaceholder')}
                  disabled={termLoading}
                  autoFocus
                />
                {termLoading && <Loader2 size={12} style={{ animation: 'spin 1s linear infinite' }} className="text-dim" />}
              </div>
            </div>
          )}
        </div>
      )}
    </aside>
  );
}
