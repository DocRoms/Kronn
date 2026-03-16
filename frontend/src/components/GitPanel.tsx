import { useState, useEffect, useCallback, useRef } from 'react';
import { projects as projectsApi, discussions as discussionsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import ReactMarkdown from 'react-markdown';
import {
  GitBranch, GitCommit, GitPullRequest, Upload, RefreshCw, ChevronLeft,
  FileEdit, FilePlus, FileMinus, FileX, AlertTriangle, ExternalLink,
  Loader2, Check, X, Terminal,
} from 'lucide-react';

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
  modified: '#fbbf24',
  added: '#34d399',
  deleted: '#ff4d6a',
  renamed: '#60a5fa',
  copied: '#60a5fa',
  untracked: 'rgba(255,255,255,0.3)',
};

export function GitPanel({ projectId, discussionId, onClose }: Props) {
  const { t } = useT();
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');

  // Diff view
  const [diffPath, setDiffPath] = useState<string | null>(null);
  const [diffContent, setDiffContent] = useState('');
  const [diffLoading, setDiffLoading] = useState(false);

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

  const openDiff = async (path: string) => {
    setDiffPath(path);
    setDiffLoading(true);
    try {
      const res = discussionId
        ? await discussionsApi.gitDiff(discussionId, path)
        : await projectsApi.gitDiff(projectId!, path);
      setDiffContent(res.diff);
    } catch (e) {
      setDiffContent(`Error: ${e}`);
    } finally {
      setDiffLoading(false);
    }
  };

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
      } else {
        await projectsApi.gitCommit(projectId!, commitReq);
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
      } else {
        await projectsApi.gitPush(projectId!);
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
    const id = discussionId || projectId!;
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
      const id = discussionId || projectId!;
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
      const res = discussionId
        ? await discussionsApi.exec(discussionId, cmd)
        : await projectsApi.exec(projectId!, cmd);
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
      <div style={styles.panel}>
        <div style={styles.header}>
          <button style={styles.backBtn} onClick={() => setDiffPath(null)}>
            <ChevronLeft size={14} />
          </button>
          <span style={styles.headerTitle}>{diffPath}</span>
          <button style={styles.closeBtn} onClick={onClose}><X size={14} /></button>
        </div>
        <div style={styles.diffContainer}>
          {diffLoading ? (
            <div style={styles.center}><Loader2 size={16} style={{ animation: 'spin 1s linear infinite' }} /></div>
          ) : (
            <pre style={styles.diffPre}>
              {diffContent.split('\n').map((line, i) => {
                let color = 'rgba(255,255,255,0.6)';
                if (line.startsWith('+') && !line.startsWith('+++')) color = '#34d399';
                else if (line.startsWith('-') && !line.startsWith('---')) color = '#ff4d6a';
                else if (line.startsWith('@@')) color = '#60a5fa';
                return <div key={i} style={{ color, minHeight: 18 }}>{line || ' '}</div>;
              })}
            </pre>
          )}
        </div>
      </div>
    );
  }

  // ─── Main view ──────────────────────────────────────────────────────────────
  return (
    <div style={styles.panel}>
      {/* Header */}
      <div style={styles.header}>
        <span style={styles.headerTitle}>
          <GitBranch size={13} style={{ marginRight: 6 }} />
          {t('git.title')}
        </span>
        <div style={{ display: 'flex', gap: 4 }}>
          <button style={styles.iconBtn} onClick={fetchStatus} title={t('git.refresh')}>
            <RefreshCw size={12} />
          </button>
          <button style={styles.closeBtn} onClick={onClose}><X size={14} /></button>
        </div>
      </div>

      {loading && (
        <div style={styles.center}><Loader2 size={16} style={{ animation: 'spin 1s linear infinite' }} /></div>
      )}

      {error && (
        <div style={styles.error}>{error}</div>
      )}

      {status && !loading && (
        <div style={styles.body}>
          {/* Branch info */}
          <div style={styles.branchBar}>
            <GitBranch size={12} />
            <span style={{ fontWeight: 600 }}>{status.branch || 'HEAD'}</span>
            {status.ahead > 0 && <span style={styles.badge}>↑{status.ahead}</span>}
            {status.behind > 0 && <span style={{ ...styles.badge, background: 'rgba(255,77,106,0.15)', color: '#ff4d6a' }}>↓{status.behind}</span>}
          </div>

          {/* Warning: on default branch */}
          {status.is_default_branch && status.files.length > 0 && (
            <div style={styles.warning}>
              <AlertTriangle size={12} />
              <span>{t('git.onDefaultBranch')}</span>
              <button
                style={styles.smallBtn}
                onClick={() => setShowBranch(true)}
              >
                <GitBranch size={10} /> {t('git.createBranch')}
              </button>
            </div>
          )}

          {/* Create branch form */}
          {showBranch && (
            <div style={styles.formRow}>
              <input
                style={styles.input}
                placeholder={t('git.branchName')}
                value={branchName}
                onChange={e => setBranchName(e.target.value)}
                onKeyDown={e => e.key === 'Enter' && handleCreateBranch()}
                autoFocus
              />
              <button style={styles.actionBtn} onClick={handleCreateBranch} disabled={branchLoading}>
                {branchLoading ? <Loader2 size={12} style={{ animation: 'spin 1s linear infinite' }} /> : <Check size={12} />}
              </button>
              <button style={styles.iconBtn} onClick={() => setShowBranch(false)}><X size={12} /></button>
            </div>
          )}

          {/* Actions bar: push, create PR — always visible when applicable */}
          <div style={{ display: 'flex', gap: 4, padding: '4px 12px', flexWrap: 'wrap' }}>
            {status.ahead > 0 && (
              <button style={{ ...styles.smallBtn, borderColor: 'rgba(96,165,250,0.3)', color: '#60a5fa' }} onClick={handlePush} disabled={pushLoading}>
                {pushLoading ? <Loader2 size={10} style={{ animation: 'spin 1s linear infinite' }} /> : <Upload size={10} />}
                {t('git.push')}
              </button>
            )}
            {/* Create PR/MR button: show when on a non-default branch and no PR exists */}
            {!status.is_default_branch && !status.pr_url && !showPrForm && (
              <button style={{ ...styles.smallBtn, borderColor: 'rgba(139,92,246,0.3)', color: '#a78bfa' }} onClick={openPrForm}>
                <GitPullRequest size={10} />
                {status.provider === 'gitlab' ? t('git.createMr') : t('git.createPr')}
              </button>
            )}
          </div>

          {/* PR link */}
          {status.pr_url && (
            <div style={{ ...styles.success, display: 'flex', alignItems: 'center', gap: 6 }}>
              <GitPullRequest size={11} style={{ flexShrink: 0 }} />
              <a href={status.pr_url} target="_blank" rel="noopener noreferrer" style={{ color: '#a78bfa', textDecoration: 'underline', fontSize: 11, flex: 1, overflow: 'hidden', textOverflow: 'ellipsis' }}>
                {status.pr_url.replace('https://github.com/', '').replace('https://gitlab.com/', '')}
              </a>
              <ExternalLink size={9} style={{ flexShrink: 0, color: 'rgba(255,255,255,0.3)' }} />
            </div>
          )}

          {/* PR creation form */}
          {showPrForm && (
            <div style={{ margin: '4px 12px', padding: '10px 12px', borderRadius: 8, background: 'rgba(139,92,246,0.04)', border: '1px solid rgba(139,92,246,0.15)', display: 'flex', flexDirection: 'column' }}>
              <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 6 }}>
                <span style={{ fontSize: 11, fontWeight: 600, color: '#a78bfa', display: 'flex', alignItems: 'center', gap: 4 }}>
                  <GitPullRequest size={11} /> {status?.provider === 'gitlab' ? t('git.createMr') : t('git.createPr')}
                </span>
                <div style={{ display: 'flex', gap: 2 }}>
                  {prTemplateSource && (
                    <span style={{ fontSize: 9, color: 'rgba(255,255,255,0.3)' }}>
                      {prTemplateSource === 'project' ? t('git.prTemplateProject') : t('git.prTemplateKronn')}
                    </span>
                  )}
                  <button style={styles.iconBtn} onClick={() => setShowPrForm(false)}><X size={10} /></button>
                </div>
              </div>
              <input
                style={{ ...styles.input, marginBottom: 6, width: '100%', boxSizing: 'border-box' }}
                value={prTitle}
                onChange={e => setPrTitle(e.target.value)}
                placeholder={t('git.prTitle')}
                autoFocus
              />
              <div style={{ display: 'flex', gap: 0, marginBottom: 4 }}>
                <button
                  style={{ ...styles.smallBtn, borderRadius: '6px 0 0 6px', fontSize: 9, background: !prPreview ? 'rgba(139,92,246,0.15)' : 'transparent', borderColor: 'rgba(139,92,246,0.2)', color: !prPreview ? '#a78bfa' : 'rgba(255,255,255,0.35)' }}
                  onClick={() => setPrPreview(false)}
                >
                  {t('git.prEdit')}
                </button>
                <button
                  style={{ ...styles.smallBtn, borderRadius: '0 6px 6px 0', fontSize: 9, background: prPreview ? 'rgba(139,92,246,0.15)' : 'transparent', borderColor: 'rgba(139,92,246,0.2)', color: prPreview ? '#a78bfa' : 'rgba(255,255,255,0.35)' }}
                  onClick={() => setPrPreview(true)}
                >
                  {t('git.prPreview')}
                </button>
              </div>
              {prPreview ? (
                <div style={{ padding: '8px 10px', borderRadius: 6, background: 'rgba(255,255,255,0.02)', border: '1px solid rgba(255,255,255,0.06)', fontSize: 12, color: 'rgba(255,255,255,0.7)', minHeight: 180, maxHeight: 300, overflowY: 'auto', lineHeight: 1.6, width: '100%', boxSizing: 'border-box' }}>
                  <ReactMarkdown>{prBody || '*No description*'}</ReactMarkdown>
                </div>
              ) : (
                <textarea
                  style={{ ...styles.input, minHeight: 180, maxHeight: 300, resize: 'vertical', fontFamily: 'monospace', fontSize: 12, lineHeight: 1.5, width: '100%', boxSizing: 'border-box' }}
                  value={prBody}
                  onChange={e => setPrBody(e.target.value)}
                  placeholder={t('git.prBodyPlaceholder')}
                />
              )}
              <button
                style={{ ...styles.actionBtn, marginTop: 6, width: '100%', justifyContent: 'center', borderColor: 'rgba(139,92,246,0.3)', background: 'rgba(139,92,246,0.1)', color: '#a78bfa' }}
                onClick={handleCreatePr}
                disabled={prLoading || !prTitle.trim()}
              >
                {prLoading ? <Loader2 size={12} style={{ animation: 'spin 1s linear infinite' }} /> : <GitPullRequest size={12} />}
                {status?.provider === 'gitlab' ? t('git.submitMr') : t('git.submitPr')}
              </button>
            </div>
          )}

          {pushResult && (
            <div style={pushResult === 'success' || pushResult.startsWith('PR:') ? styles.success : styles.error}>
              {pushResult === 'success' ? t('git.pushSuccess') : pushResult.startsWith('PR:') ? pushResult.replace('PR: ', '✓ PR created: ') : pushResult}
            </div>
          )}

          {/* File list */}
          {status.files.length === 0 ? (
            <div style={styles.empty}>{t('git.noChanges')}</div>
          ) : (
            <>
              <div style={styles.fileHeader}>
                <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.4)' }}>
                  {status.files.length} {t('git.filesChanged')}
                </span>
                <div style={{ display: 'flex', gap: 4 }}>
                  {!showCommit && (
                    <button style={styles.smallBtn} onClick={() => { setShowCommit(true); setSelectedFiles(status.files.map(f => f.path)); }}>
                      <GitCommit size={10} /> {t('git.commit')}
                    </button>
                  )}
                </div>
              </div>

              {/* Commit form */}
              {showCommit && (
                <div style={styles.commitForm}>
                  <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 6 }}>
                    <button style={{ ...styles.linkBtn, fontSize: 10 }} onClick={selectAll}>
                      {selectedFiles.length === status.files.length ? t('git.deselectAll') : t('git.selectAll')}
                    </button>
                    <button style={styles.iconBtn} onClick={() => setShowCommit(false)}><X size={10} /></button>
                  </div>
                  <input
                    style={styles.input}
                    placeholder={t('git.commitMessage')}
                    value={commitMsg}
                    onChange={e => setCommitMsg(e.target.value)}
                    onKeyDown={e => e.key === 'Enter' && handleCommit()}
                    autoFocus
                  />
                  <div style={{ display: 'flex', gap: 12, marginTop: 6, fontSize: 11, color: 'rgba(255,255,255,0.5)' }}>
                    <label style={{ display: 'flex', alignItems: 'center', gap: 4, cursor: 'pointer' }}>
                      <input type="checkbox" checked={commitAmend} onChange={e => setCommitAmend(e.target.checked)} style={{ accentColor: '#c8ff00' }} />
                      {t('git.amend')}
                    </label>
                    <label style={{ display: 'flex', alignItems: 'center', gap: 4, cursor: 'pointer' }}>
                      <input type="checkbox" checked={commitSign} onChange={e => setCommitSign(e.target.checked)} style={{ accentColor: '#c8ff00' }} />
                      {t('git.sign')}
                    </label>
                  </div>
                  <button
                    style={{ ...styles.actionBtn, marginTop: 6, width: '100%', justifyContent: 'center' }}
                    onClick={handleCommit}
                    disabled={commitLoading || !commitMsg.trim() || selectedFiles.length === 0}
                  >
                    {commitLoading ? <Loader2 size={12} style={{ animation: 'spin 1s linear infinite' }} /> : <GitCommit size={12} />}
                    {t('git.commitSelected', String(selectedFiles.length))}
                  </button>
                </div>
              )}

              <div style={styles.fileList}>
                {status.files.map(file => {
                  const Icon = STATUS_ICONS[file.status] || FileX;
                  const color = STATUS_COLORS[file.status] || 'rgba(255,255,255,0.4)';
                  return (
                    <div key={file.path} style={styles.fileRow}>
                      {showCommit && (
                        <input
                          type="checkbox"
                          checked={selectedFiles.includes(file.path)}
                          onChange={() => toggleFile(file.path)}
                          style={{ marginRight: 6, accentColor: '#c8ff00' }}
                        />
                      )}
                      <Icon size={12} style={{ color, flexShrink: 0 }} />
                      <button
                        style={styles.fileBtn}
                        onClick={() => openDiff(file.path)}
                        title={file.path}
                      >
                        {file.path}
                      </button>
                      <span style={{ fontSize: 9, color, flexShrink: 0 }}>{file.status}</span>
                    </div>
                  );
                })}
              </div>
            </>
          )}
        </div>
      )}

      {/* Mini Terminal */}
      <div style={styles.termSection}>
        <button
          style={styles.termToggle}
          onClick={() => setShowTerminal(prev => !prev)}
        >
          <Terminal size={11} />
          <span>{t('git.terminal')}</span>
        </button>
        {showTerminal && (
          <div style={styles.termBody}>
            <div style={styles.termOutput}>
              {termHistory.map((entry, i) => (
                <div key={i}>
                  <div style={{ color: '#c8ff00', fontSize: 11 }}>$ {entry.cmd}</div>
                  {entry.stdout && <pre style={styles.termPre}>{entry.stdout}</pre>}
                  {entry.stderr && <pre style={{ ...styles.termPre, color: entry.code !== 0 ? '#ff8a9e' : 'rgba(255,255,255,0.4)' }}>{entry.stderr}</pre>}
                </div>
              ))}
              <div ref={termEndRef} />
            </div>
            <div style={styles.termInputRow}>
              <span style={{ color: '#c8ff00', fontSize: 11, flexShrink: 0 }}>$</span>
              <input
                style={styles.termInput}
                value={termInput}
                onChange={e => setTermInput(e.target.value)}
                onKeyDown={e => e.key === 'Enter' && handleExec()}
                placeholder={t('git.terminalPlaceholder')}
                disabled={termLoading}
                autoFocus
              />
              {termLoading && <Loader2 size={12} style={{ animation: 'spin 1s linear infinite', color: 'rgba(255,255,255,0.3)' }} />}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

// ─── Styles ─────────────────────────────────────────────────────────────────

const styles: Record<string, React.CSSProperties> = {
  panel: {
    width: 380, height: '100%', display: 'flex', flexDirection: 'column',
    background: '#0d1017', borderLeft: '1px solid rgba(255,255,255,0.08)',
  },
  header: {
    display: 'flex', alignItems: 'center', justifyContent: 'space-between',
    padding: '10px 12px', borderBottom: '1px solid rgba(255,255,255,0.06)',
  },
  headerTitle: {
    fontSize: 13, fontWeight: 600, color: '#e8eaed',
    display: 'flex', alignItems: 'center',
    overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' as const,
  },
  body: { flex: 1, overflowY: 'auto' as const, padding: '8px 0' },
  center: { display: 'flex', justifyContent: 'center', padding: 24, color: 'rgba(255,255,255,0.3)' },
  branchBar: {
    display: 'flex', alignItems: 'center', gap: 6, padding: '6px 12px',
    fontSize: 12, color: '#e8eaed',
  },
  badge: {
    fontSize: 9, padding: '1px 5px', borderRadius: 6, fontWeight: 600,
    background: 'rgba(200,255,0,0.15)', color: '#c8ff00',
  },
  warning: {
    margin: '4px 12px', padding: '8px 10px', borderRadius: 6,
    background: 'rgba(255,200,0,0.06)', border: '1px solid rgba(255,200,0,0.15)',
    fontSize: 11, color: 'rgba(255,200,0,0.8)',
    display: 'flex', alignItems: 'center', gap: 6, flexWrap: 'wrap' as const,
  },
  fileHeader: {
    display: 'flex', alignItems: 'center', justifyContent: 'space-between',
    padding: '6px 12px',
  },
  fileList: { padding: '0 4px' },
  fileRow: {
    display: 'flex', alignItems: 'center', gap: 6,
    padding: '4px 8px', borderRadius: 4,
  },
  fileBtn: {
    background: 'none', border: 'none', color: '#e8eaed', fontSize: 12,
    cursor: 'pointer', textAlign: 'left' as const, flex: 1, fontFamily: 'inherit',
    overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' as const,
    padding: '2px 0',
  },
  formRow: {
    display: 'flex', gap: 4, padding: '4px 12px',
  },
  input: {
    flex: 1, padding: '5px 8px', borderRadius: 6, fontSize: 12, fontFamily: 'inherit',
    border: '1px solid rgba(255,255,255,0.1)', background: 'rgba(255,255,255,0.04)',
    color: '#e8eaed', outline: 'none',
  },
  commitForm: {
    margin: '4px 12px', padding: '8px 10px', borderRadius: 6,
    background: 'rgba(255,255,255,0.02)', border: '1px solid rgba(255,255,255,0.06)',
  },
  smallBtn: {
    padding: '3px 8px', borderRadius: 6, fontSize: 10, fontFamily: 'inherit',
    border: '1px solid rgba(200,255,0,0.3)', background: 'transparent',
    color: '#c8ff00', cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 3,
  },
  actionBtn: {
    padding: '5px 10px', borderRadius: 6, fontSize: 11, fontFamily: 'inherit',
    border: '1px solid rgba(200,255,0,0.3)', background: 'rgba(200,255,0,0.1)',
    color: '#c8ff00', cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 4,
  },
  iconBtn: {
    background: 'none', border: 'none', color: 'rgba(255,255,255,0.4)',
    cursor: 'pointer', padding: 4, borderRadius: 4, display: 'flex',
  },
  closeBtn: {
    background: 'none', border: 'none', color: 'rgba(255,255,255,0.3)',
    cursor: 'pointer', padding: 4,
  },
  linkBtn: {
    background: 'none', border: 'none', color: 'rgba(200,255,0,0.6)',
    cursor: 'pointer', padding: 0, fontFamily: 'inherit', textDecoration: 'underline',
  },
  backBtn: {
    background: 'none', border: 'none', color: 'rgba(255,255,255,0.5)',
    cursor: 'pointer', padding: '2px 4px', display: 'flex',
  },
  diffContainer: { flex: 1, overflowY: 'auto' as const, padding: '8px 12px' },
  diffPre: {
    margin: 0, fontSize: 11, fontFamily: 'monospace', lineHeight: 1.5,
    whiteSpace: 'pre-wrap' as const, wordBreak: 'break-all' as const,
  },
  empty: { padding: 24, textAlign: 'center' as const, fontSize: 12, color: 'rgba(255,255,255,0.3)' },
  error: {
    margin: '4px 12px', padding: '6px 10px', borderRadius: 6,
    background: 'rgba(255,77,106,0.08)', border: '1px solid rgba(255,77,106,0.2)',
    fontSize: 11, color: '#ff8a9e',
  },
  success: {
    margin: '4px 12px', padding: '6px 10px', borderRadius: 6,
    background: 'rgba(52,211,153,0.08)', border: '1px solid rgba(52,211,153,0.2)',
    fontSize: 11, color: '#34d399',
  },
  termSection: {
    borderTop: '1px solid rgba(255,255,255,0.06)',
  },
  termToggle: {
    width: '100%', padding: '6px 12px', background: 'none', border: 'none',
    color: 'rgba(255,255,255,0.4)', fontSize: 11, fontFamily: 'inherit',
    cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 5,
  },
  termBody: {
    display: 'flex', flexDirection: 'column' as const,
    maxHeight: 200, borderTop: '1px solid rgba(255,255,255,0.04)',
  },
  termOutput: {
    flex: 1, overflowY: 'auto' as const, padding: '6px 10px',
    fontSize: 11, fontFamily: 'monospace',
  },
  termPre: {
    margin: '2px 0 6px', fontSize: 11, fontFamily: 'monospace',
    whiteSpace: 'pre-wrap' as const, wordBreak: 'break-all' as const,
    color: 'rgba(255,255,255,0.6)',
  },
  termInputRow: {
    display: 'flex', alignItems: 'center', gap: 6,
    padding: '4px 10px 6px', borderTop: '1px solid rgba(255,255,255,0.04)',
  },
  termInput: {
    flex: 1, background: 'none', border: 'none', outline: 'none',
    color: '#e8eaed', fontSize: 11, fontFamily: 'monospace',
  },
};
