import { useEffect, useRef, useState } from 'react';
import { Loader2, Terminal, X } from 'lucide-react';
import { discussions as discussionsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import type { DiscussionWorkspace } from '../types/generated';
import './DiscussionToolPanel.css';
import './TerminalPanel.css';

interface TerminalEntry {
  command: string;
  stdout: string;
  stderr: string;
  exitCode: number;
}

interface TerminalPanelProps {
  discussionId: string;
  onClose: () => void;
}

export function TerminalPanel({ discussionId, onClose }: TerminalPanelProps) {
  const { t } = useT();
  const [workspaces, setWorkspaces] = useState<DiscussionWorkspace[]>([]);
  const [selectedWorkspaceId, setSelectedWorkspaceId] = useState<string | undefined>();
  const [input, setInput] = useState('');
  const [history, setHistory] = useState<TerminalEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [workspaceLoad, setWorkspaceLoad] = useState<{
    discussionId: string;
    status: 'loading' | 'ready' | 'error';
  }>({ discussionId, status: 'loading' });
  const execInFlightRef = useRef(false);
  const endRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const isDefaultDiscussionWorkspace = (workspace: DiscussionWorkspace) =>
    workspace.session_pk === null
    && workspace.disc_id === discussionId
    && workspace.state === 'attached';

  useEffect(() => {
    setInput('');
    setHistory([]);
    setSelectedWorkspaceId(undefined);
  }, [discussionId]);

  useEffect(() => {
    let current = true;
    if (typeof discussionsApi.workspaces !== 'function') return () => { current = false; };
    void discussionsApi.workspaces(discussionId)
      .then(rows => {
        if (!current) return;
        const attached = rows.filter(row => row.state === 'attached');
        setWorkspaces(rows);
        setWorkspaceLoad({ discussionId, status: 'ready' });
        setSelectedWorkspaceId(previous => {
          if (previous && rows.some(row => row.id === previous)) return previous;
          const hasLegacy = attached.some(
            row => row.session_pk === null && row.disc_id === discussionId,
          );
          return hasLegacy ? undefined : (attached[0]?.id ?? rows[0]?.id);
        });
      })
      .catch(() => {
        if (current) {
          setWorkspaces([]);
          setWorkspaceLoad({ discussionId, status: 'error' });
        }
      });
    return () => { current = false; };
  }, [discussionId]);

  useEffect(() => {
    endRef.current?.scrollIntoView?.({ behavior: 'smooth' });
  }, [history]);

  const selectedWorkspace = selectedWorkspaceId
    ? workspaces.find(workspace => workspace.id === selectedWorkspaceId)
    : undefined;
  const historicalWorkspace = !!selectedWorkspace && selectedWorkspace.state !== 'attached';
  const workspaceLoading = workspaceLoad.discussionId !== discussionId
    || workspaceLoad.status === 'loading';
  const workspaceLoadFailed = workspaceLoad.discussionId === discussionId
    && workspaceLoad.status === 'error';

  useEffect(() => {
    if (!loading && !workspaceLoading && !workspaceLoadFailed && !historicalWorkspace) {
      inputRef.current?.focus();
    }
  }, [historicalWorkspace, loading, workspaceLoadFailed, workspaceLoading]);

  const handleExec = async () => {
    const command = input.trim();
    if (
      !command
      || historicalWorkspace
      || workspaceLoading
      || workspaceLoadFailed
      || execInFlightRef.current
    ) return;
    execInFlightRef.current = true;
    setLoading(true);
    setInput('');
    try {
      const result = selectedWorkspaceId
        ? await discussionsApi.exec(discussionId, command, selectedWorkspaceId)
        : await discussionsApi.exec(discussionId, command);
      setHistory(current => [...current, {
        command,
        stdout: result.stdout,
        stderr: result.stderr,
        exitCode: result.exit_code,
      }]);
    } catch (error) {
      setHistory(current => [...current, {
        command,
        stdout: '',
        stderr: String(error),
        exitCode: 1,
      }]);
    } finally {
      execInFlightRef.current = false;
      setLoading(false);
    }
  };

  return (
    <aside className="disc-tool-panel terminal-panel" aria-label={t('git.terminal')}>
      <header className="disc-tool-panel-header">
        <div className="disc-tool-panel-title">
          <Terminal size={15} />
          <span>{t('git.terminal')}</span>
        </div>
        <div className="disc-tool-panel-actions">
          <button
            type="button"
            className="disc-tool-panel-icon"
            onClick={onClose}
            aria-label={t('common.close')}
          >
            <X size={14} />
          </button>
        </div>
      </header>

      {workspaces.length > 0 && (
        <label className="terminal-workspace-picker">
          <span>{t('git.workspaceSelector')}</span>
          <select
            value={selectedWorkspaceId ?? ''}
            onChange={event => setSelectedWorkspaceId(event.target.value || undefined)}
            aria-label={t('git.workspaceSelector')}
          >
            {workspaces.some(isDefaultDiscussionWorkspace) && (
              <option value="">{t('git.workspaceDefault')}</option>
            )}
            {workspaces
              .filter(workspace => !isDefaultDiscussionWorkspace(workspace))
              .map(workspace => (
                <option key={workspace.id} value={workspace.id}>
                  {workspace.task_reference ? `${workspace.task_reference} · ` : ''}
                  {workspace.branch}
                  {` · ${t(workspace.ownership === 'managed' ? 'git.workspaceManaged' : 'git.workspaceExternal')}`}
                  {workspace.state !== 'attached'
                    ? ` · ${t(`planning.workspaceState.${workspace.state}`)}`
                    : ''}
                </option>
              ))}
          </select>
        </label>
      )}

      <div className="disc-tool-panel-body terminal-body">
        <div className="terminal-output" aria-live="polite">
          {history.map((entry, index) => (
            <div key={`${entry.command}-${index}`} className="terminal-entry">
              <div className="terminal-command">$ {entry.command}</div>
              {entry.stdout && <pre className="terminal-pre">{entry.stdout}</pre>}
              {entry.stderr && (
                <pre className={`terminal-pre ${entry.exitCode !== 0 ? 'terminal-pre-error' : 'terminal-pre-warning'}`}>
                  {entry.stderr}
                </pre>
              )}
            </div>
          ))}
          <div ref={endRef} />
        </div>

        {historicalWorkspace && (
          <p className="terminal-historical" role="status">
            {t('git.terminalHistorical')}
          </p>
        )}

        {workspaceLoadFailed && (
          <p className="terminal-historical" role="alert">
            {t('git.terminalWorkspaceUnavailable')}
          </p>
        )}

        <form
          className="terminal-input-row"
          onSubmit={event => {
            event.preventDefault();
            void handleExec();
          }}
        >
          <span className="terminal-prompt">$</span>
          <input
            ref={inputRef}
            className="terminal-input"
            value={input}
            onChange={event => setInput(event.target.value)}
            placeholder={t('git.terminalPlaceholder')}
            disabled={loading || workspaceLoading || workspaceLoadFailed || historicalWorkspace}
            autoFocus
          />
          {loading && <Loader2 size={13} className="spin text-dim" />}
        </form>
      </div>
    </aside>
  );
}
