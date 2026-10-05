import { useEffect, useState } from 'react';
import { AlertTriangle } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { workflows as workflowsApi } from '../../lib/api';
import type { ExecScriptFile, ExecScriptFileStatus } from '../../types/generated';
import { parseScriptPaths } from './execScriptPaths';

interface Props {
  files: ExecScriptFile[];
  /** The workflow's home project: its repository carries the scripts. */
  projectId: string | null | undefined;
  onChange: (next: ExecScriptFile[]) => void;
}

const STATE_COLOR: Record<ExecScriptFileStatus['state'], string> = {
  approved: 'var(--kr-success)',
  changed: 'var(--kr-error)',
  pending: 'var(--kr-warning)',
  invalid: 'var(--kr-error)',
};

/** KT-918 — the repository files an Exec step runs and where each stands
 *  against its approved hash. Clearing a hash re-approves the file at save. */
export function ExecScriptFilesEditor({ files, projectId, onChange }: Props) {
  const { t } = useT();
  const [text, setText] = useState(() => files.map(file => file.path).join('\n'));
  const [statuses, setStatuses] = useState<{ key: string; list: ExecScriptFileStatus[] } | null>(null);
  const key = JSON.stringify([projectId ?? null, files]);

  useEffect(() => {
    if (!projectId || files.length === 0) return;
    let alive = true;
    const timer = setTimeout(() => {
      workflowsApi.execScriptStatus({ project_id: projectId, files })
        .then(list => { if (alive) setStatuses({ key, list }); })
        .catch(() => { if (alive) setStatuses({ key, list: [] }); });
    }, 300);
    return () => { alive = false; clearTimeout(timer); };
  }, [projectId, files, key]);

  // Only the answer about the current list counts.
  const current = statuses?.key === key ? statuses.list : [];
  const changed = current.filter(status => status.state === 'changed').map(status => status.path);

  const approveChanged = () => {
    const stale = new Set(changed);
    onChange(files.map(file => (stale.has(file.path) ? { ...file, sha256: '' } : file)));
  };

  return (
    <div className="mt-2 mb-2">
      <label className="text-xs text-muted mb-1">{t('wiz.execScripts')}</label>
      <textarea
        className="wf-textarea text-sm"
        rows={Math.max(2, Math.min(6, files.length + 1))}
        value={text}
        onChange={event => {
          setText(event.target.value);
          onChange(parseScriptPaths(event.target.value, files));
        }}
        placeholder={t('wiz.execScriptsPlaceholder')}
        aria-label={t('wiz.execScripts')}
      />
      <p className="text-2xs text-ghost mt-1">{t('wiz.execScriptsHint')}</p>
      {files.length > 0 && !projectId && (
        <div className="wf-restricted-warning mt-1">
          <AlertTriangle size={12} />
          <span className="flex-1">{t('wiz.execScriptsNoProject')}</span>
        </div>
      )}
      {current.length > 0 && (
        <ul className="text-xs mt-1" style={{ listStyle: 'none', padding: 0 }} data-testid="exec-script-status">
          {current.map(status => (
            <li key={status.path} className="flex-row gap-2" title={status.error ?? status.current_sha256 ?? ''}>
              <code>{status.path}</code>
              <span style={{ color: STATE_COLOR[status.state] }}>
                {t(`wiz.execScriptState.${status.state}`)}
              </span>
              {status.error && <span className="text-ghost">{status.error}</span>}
            </li>
          ))}
        </ul>
      )}
      {changed.length > 0 && (
        <div className="wf-restricted-warning mt-1">
          <AlertTriangle size={12} />
          <span className="flex-1">{t('wiz.execScriptsChangedWarn', changed.join(', '))}</span>
          <button type="button" className="wf-allowlist-cta" onClick={approveChanged}>
            {t('wiz.execScriptsApprove')}
          </button>
        </div>
      )}
    </div>
  );
}
