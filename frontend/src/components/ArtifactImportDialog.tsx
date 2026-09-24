import { useEffect, useRef, useState } from 'react';
import { Loader2, Upload, X } from 'lucide-react';
import { pages, projects as projectsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import type { ArtifactImportChoice, ArtifactImportPreview, ArtifactImportRequest, LivePage, Project } from '../types/generated';
import './ArtifactImportDialog.css';

const MAX_BYTES = 16 * 1024 * 1024;

export function ArtifactImportDialog({ onClose, onImported, initialProjectId }: {
  onClose: () => void;
  onImported: (page: LivePage) => void;
  initialProjectId?: string;
}) {
  const { t } = useT();
  const [content, setContent] = useState('');
  const [filename, setFilename] = useState('');
  const [projectId, setProjectId] = useState(initialProjectId ?? '');
  const [projects, setProjects] = useState<Project[]>([]);
  const [choices, setChoices] = useState<ArtifactImportChoice[]>([]);
  const [approvedExecIds, setApprovedExecIds] = useState<string[]>([]);
  const [preview, setPreview] = useState<ArtifactImportPreview | null>(null);
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inFlight = useRef(false);
  const dialog = useRef<HTMLElement>(null);

  useEffect(() => {
    let active = true;
    void projectsApi.list().then(items => { if (active) setProjects(items); })
      .catch(cause => { if (active) setError(userError(cause)); });
    return () => { active = false; };
  }, []);

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    dialog.current?.querySelector<HTMLElement>('input')?.focus();
    const keydown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !inFlight.current) { event.preventDefault(); onClose(); }
      if (event.key !== 'Tab') return;
      const items = Array.from(dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled)') ?? []);
      const first = items[0]; const last = items.at(-1);
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    };
    window.addEventListener('keydown', keydown);
    return () => { window.removeEventListener('keydown', keydown); previous?.focus(); };
  }, [onClose]);

  const readFile = async (file?: File) => {
    if (!file || inFlight.current) return;
    inFlight.current = true; setBusy(true); setError(null); setPreview(null); setContent(''); setChoices([]);
    setApprovedExecIds([]);
    try {
      if (file.size > MAX_BYTES) throw new Error(t('pages.import.tooLarge'));
      const text = await file.text();
      const bundle = JSON.parse(text) as { kind?: unknown; version?: unknown };
      if (bundle?.kind !== 'kronn.artifact' || bundle.version !== 1) throw new Error(t('pages.import.invalidFile'));
      setContent(text); setFilename(file.name); setDirty(true);
    } catch (cause) { setError(userError(cause)); }
    finally { inFlight.current = false; setBusy(false); }
  };

  const request = (): ArtifactImportRequest => ({ content, project_id: projectId || null, choices, approved_quick_exec_ids: approvedExecIds, preview_digest: preview?.digest ?? null });
  const inspect = async () => {
    if (!content || inFlight.current) return;
    inFlight.current = true; setBusy(true); setError(null);
    try { setPreview(await pages.previewImport(request())); setDirty(false); }
    catch (cause) { setError(userError(cause)); setDirty(true); }
    finally { inFlight.current = false; setBusy(false); }
  };
  const commit = async () => {
    if (inFlight.current || dirty || !preview?.can_import) return;
    inFlight.current = true; setBusy(true); setError(null);
    try {
      const result = await pages.importArtifact(request());
      window.dispatchEvent(new Event('kronn:pages-activated'));
      onImported(result.artifact);
    } catch (cause) { setError(userError(cause)); setDirty(true); }
    finally { inFlight.current = false; setBusy(false); }
  };

  return <div className="artifact-import-backdrop" onMouseDown={event => {
    if (event.target === event.currentTarget && !inFlight.current) onClose();
  }}>
    <section ref={dialog} className="artifact-import-dialog" role="dialog" aria-modal="true" aria-labelledby="artifact-import-title">
      <header><h2 id="artifact-import-title">{t('pages.import.title')}</h2>
        <button type="button" onClick={onClose} disabled={busy} aria-label={t('common.close')}><X size={18} /></button></header>
      <p>{t('pages.import.hint')}</p>
      <label>{t('pages.import.file')}<input type="file" accept="application/json,.json" disabled={busy}
        onChange={event => { void readFile(event.target.files?.[0]); event.target.value = ''; }} /></label>
      {filename && <small>{filename}</small>}
      <label>{t('pages.import.project')}<select value={projectId} disabled={busy} onChange={event => {
        setProjectId(event.target.value); setDirty(true);
      }}><option value="">{t('pages.import.noProject')}</option>
        {projects.map(project => <option key={project.id} value={project.id}>{project.name}</option>)}
      </select></label>
      {preview && <>
        <h3>{preview.title}</h3>
        <div className="artifact-import-entries">
          {preview.entries.map(entry => {
            const choice = choices.find(choice => choice.kind === entry.kind && choice.source_id === entry.source_id);
            return <div className="artifact-import-entry" key={`${entry.kind}:${entry.source_id}`}>
              <div><strong>{entry.name}</strong><small>{t(`pages.import.kind.${entry.kind}`)} · {t(`pages.import.status.${entry.disposition}`)}</small>
                <small>{t(`pages.import.reason.${entry.reason}`)}</small>
                {entry.quick_exec && <div className="artifact-import-exec">
                  <strong>{t('pages.import.commandAndArgs')}</strong>
                  <pre>{JSON.stringify({ command: entry.quick_exec.command, args: entry.quick_exec.args }, null, 2)}</pre>
                  <label className="artifact-import-approval"><input type="checkbox" disabled={busy}
                    checked={approvedExecIds.includes(entry.source_id)} onChange={event => {
                      const checked = event.target.checked;
                      setApprovedExecIds(previous => checked ? [...previous, entry.source_id] : previous.filter(id => id !== entry.source_id));
                      setDirty(true);
                    }} />{t('pages.import.approveExec', entry.name)}</label>
                </div>}
                {entry.quick_api && <p className="artifact-import-api"><code>
                  {entry.quick_api.method ?? t('pages.import.methodUnspecified')} {entry.quick_api.endpoint}
                </code><small>{entry.quick_api.plugin}</small></p>}
              </div>
              {entry.reason !== 'root' && <select aria-label={t('pages.import.choice', entry.name)} disabled={busy}
                value={choice?.action ?? 'auto'} onChange={event => {
                  const action = event.target.value;
                  setChoices(previous => [...previous.filter(item => item.kind !== entry.kind || item.source_id !== entry.source_id),
                    ...(action === 'auto' ? [] : [{ kind: entry.kind, source_id: entry.source_id,
                      action: action as 'create' | 'reuse', target_id: action === 'reuse' ? entry.existing_id : null }])]);
                  setDirty(true);
                }}>
                <option value="auto">{t('pages.import.automatic')}</option>
                <option value="create">{t('pages.import.createCopy')}</option>
                {entry.existing_id && entry.reason !== 'retargeted' && <option value="reuse">{t('pages.import.reuseLocal')}</option>}
              </select>}
            </div>;
          })}
        </div>
        {preview.entries.some(entry => entry.quick_exec && !entry.quick_exec.approved)
          && <p className="artifact-import-note">{t('pages.import.execApprovalRequired')}</p>}
        {preview.issues.length > 0 && <ul role="alert">{preview.issues.map(issue => <li key={issue}>{issue}</li>)}</ul>}
        {preview.warnings.length > 0 && <div className="artifact-import-warning"><strong>{t('pages.import.configureLater')}</strong>
          <ul>{preview.warnings.map(warning => <li key={`${warning.kind}:${warning.id}`}>{t(`pages.import.warning.${warning.kind}`, warning.id)}</li>)}</ul></div>}
        <p className="artifact-import-note">{t('pages.import.boundaries')}</p>
      </>}
      {error && <p className="artifact-import-error" role="alert">{error}</p>}
      <footer><button type="button" onClick={onClose} disabled={busy}>{t('common.cancel')}</button>
        <button type="button" onClick={() => void inspect()} disabled={busy || !content}>
          {busy && <Loader2 size={14} className="spin" />}{t('pages.import.preview')}</button>
        {preview && <button type="button" onClick={() => void commit()} disabled={busy || dirty || !preview.can_import}>
          <Upload size={14} />{t(preview.warnings.length ? 'pages.import.confirmWithSetup' : 'pages.import.confirm')}</button>}
      </footer>
    </section>
  </div>;
}
