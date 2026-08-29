import { useCallback, useEffect, useRef, useState } from 'react';
import { Download, RefreshCw, ShieldCheck, Upload } from 'lucide-react';
import { portableLibrary, type PortableLibraryItem, type PortableLibraryState } from '../../lib/api';
import type { Project } from '../../types/generated';
import type { ToastFn } from '../../hooks/useToast';
import { userError } from '../../lib/userError';
import { useAsyncGuard } from '../../hooks/useAsyncGuard';
import { useT } from '../../lib/I18nContext';

export function PortableLibrarySection({ projects, toast }: { projects: Project[]; toast: ToastFn }) {
  const { t } = useT();
  const [projectId, setProjectId] = useState('');
  const [search, setSearch] = useState('');
  const [data, setData] = useState<PortableLibraryState | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const load = useCallback(async () => {
    try { setError(''); setData(await portableLibrary.state(projectId || undefined, search)); }
    catch (e) { setError(userError(e)); }
  }, [projectId, search]);
  useEffect(() => { void load(); }, [load]);
  // `act` is frozen by useAsyncGuard on first render, so it must not close
  // over `load` directly (that would replay the mount-time projectId/search).
  // A ref kept current on every render sidesteps the staleness.
  const loadRef = useRef(load);
  loadRef.current = load;
  const act = useAsyncGuard(async (name: string, call: () => Promise<unknown>) => {
    setBusy(true); setError('');
    try { await call(); await loadRef.current(); toast(t('config.portableLibrary.actionDone', name), 'success'); }
    catch (e) { setError(userError(e)); }
    finally { setBusy(false); }
  });
  const exportItems = async () => {
    try {
      const items = await portableLibrary.export(projectId || undefined);
      const url = URL.createObjectURL(new Blob([JSON.stringify({ version: 1, items }, null, 2)], { type: 'application/json' }));
      const link = document.createElement('a'); link.href = url; link.download = 'kronn-portable-library.json'; link.click(); URL.revokeObjectURL(url);
    } catch (e) { setError(userError(e)); }
  };
  const importFile = async (file?: File) => {
    if (!file) return;
    try {
      const parsed = JSON.parse(await file.text()) as { items?: PortableLibraryItem[] };
      if (!Array.isArray(parsed.items)) throw new Error(t('config.portableLibrary.importInvalid'));
      await portableLibrary.import(projectId || undefined, parsed.items); await load();
    } catch (e) { setError(userError(e)); }
  };
  return <div id="settings-portable-library" className="set-card">
    <div className="set-section">
      <div className="flex-row gap-4 set-section-header-lg"><RefreshCw size={14} className="text-accent" /><span className="font-semibold text-lg">{t('config.portableLibrary.title')}</span></div>
      <p className="set-hint">{t('config.portableLibrary.hint')}</p>
      <div className="set-form-grid">
        <label className="set-form-label">{t('config.portableLibrary.scope')}<select className="set-input" aria-label={t('config.portableLibrary.scopeAria')} value={projectId} onChange={e => setProjectId(e.target.value)}><option value="">{t('config.portableLibrary.scopeGlobal')}</option>{projects.map(p => <option key={p.id} value={p.id}>{p.name}</option>)}</select></label>
        <label className="set-form-label">{t('config.portableLibrary.search')}<input className="set-input" aria-label={t('config.portableLibrary.searchAria')} value={search} onChange={e => setSearch(e.target.value)} /></label>
      </div>
      {data && <div className="flex-row gap-6"><span className="set-summary-chip">{t('config.portableLibrary.drift')}: {data.drift}</span><span className="set-summary-chip">{t('config.portableLibrary.trust')}: {data.approved ? t('config.portableLibrary.trustApproved') : t('config.portableLibrary.trustRequired')}</span></div>}
      {error && <div className="set-warning-callout" role="alert">{error}</div>}
      <div className="flex-row gap-4 mt-8">
        <button disabled={busy || !projectId} className="btn btn-secondary" onClick={() => act(t('config.portableLibrary.actionSync'), () => portableLibrary.sync(projectId))}>{t('config.portableLibrary.actionSync')}</button>
        <button disabled={busy || !projectId} className="btn btn-secondary" onClick={() => act(t('config.portableLibrary.actionCheck'), () => portableLibrary.check(projectId))}>{t('config.portableLibrary.actionCheck')}</button>
        <button disabled={busy || !projectId} className="btn btn-secondary" onClick={() => act(t('config.portableLibrary.actionApprove'), () => portableLibrary.approve(projectId))}><ShieldCheck size={12}/> {t('config.portableLibrary.actionApprove')}</button>
        <button disabled={busy} className="btn btn-secondary" onClick={() => act(t('config.portableLibrary.actionMigrate'), () => portableLibrary.migrate(projectId || undefined))}>{t('config.portableLibrary.actionMigrate')}</button>
        <button className="btn btn-secondary" onClick={exportItems}><Download size={12}/> {t('config.portableLibrary.actionExport')}</button>
        <label className="btn btn-secondary"><Upload size={12}/> {t('config.portableLibrary.actionImport')}<input hidden type="file" accept="application/json" onChange={e => void importFile(e.target.files?.[0])}/></label>
      </div>
      <div className="set-capability-grid mt-8">{data?.items.map(item => <div className="set-capability-card" key={`${item.kind}-${item.id}`}><div className="font-semibold">{item.id}</div><div className="flex-row gap-3"><span className="set-accordion-count">{item.kind}</span><span className="set-accordion-count">{item.scope}</span></div><div className="set-hint-xs">{item.source}</div></div>)}</div>
    </div>
  </div>;
}
