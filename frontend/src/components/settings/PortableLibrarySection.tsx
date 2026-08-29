import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import {
  ArrowRight,
  CheckCircle2,
  ChevronDown,
  Download,
  FileCheck2,
  FolderGit2,
  Globe2,
  Info,
  LibraryBig,
  MessageSquareText,
  PackageOpen,
  RefreshCw,
  ScrollText,
  Search,
  ShieldCheck,
  Sparkles,
  Upload,
  Workflow,
} from 'lucide-react';
import { portableLibrary, type PortableLibraryItem, type PortableLibraryState } from '../../lib/api';
import type { Project } from '../../types/generated';
import type { ToastFn } from '../../hooks/useToast';
import { userError } from '../../lib/userError';
import { useAsyncGuard } from '../../hooks/useAsyncGuard';
import { useT } from '../../lib/I18nContext';

function itemIcon(kind: PortableLibraryItem['kind']): ReactNode {
  if (kind === 'skill') return <Sparkles size={15} />;
  if (kind === 'directive') return <ScrollText size={15} />;
  if (kind === 'quick_prompt') return <MessageSquareText size={15} />;
  return <Workflow size={15} />;
}

export function PortableLibrarySection({ projects, toast }: { projects: Project[]; toast: ToastFn }) {
  const { t } = useT();
  const [projectId, setProjectId] = useState('');
  const [search, setSearch] = useState('');
  const [data, setData] = useState<PortableLibraryState | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const load = useCallback(async () => {
    try {
      setError('');
      setData(await portableLibrary.state(projectId || undefined, search));
    } catch (e) {
      setError(userError(e));
    }
  }, [projectId, search]);

  useEffect(() => { void load(); }, [load]);

  // `act` is frozen by useAsyncGuard on first render, so it must not close
  // over `load` directly (that would replay the mount-time projectId/search).
  // A ref kept current on every render sidesteps the staleness.
  const loadRef = useRef(load);
  loadRef.current = load;
  const act = useAsyncGuard(async (name: string, call: () => Promise<unknown>) => {
    setBusy(true);
    setError('');
    try {
      await call();
      await loadRef.current();
      toast(t('config.portableLibrary.actionDone', name), 'success');
    } catch (e) {
      setError(userError(e));
    } finally {
      setBusy(false);
    }
  });

  const exportItems = async () => {
    try {
      const items = await portableLibrary.export(projectId || undefined);
      const url = URL.createObjectURL(new Blob(
        [JSON.stringify({ version: 1, items }, null, 2)],
        { type: 'application/json' },
      ));
      const link = document.createElement('a');
      link.href = url;
      link.download = 'kronn-portable-library.json';
      link.click();
      URL.revokeObjectURL(url);
    } catch (e) {
      setError(userError(e));
    }
  };

  const importFile = async (file?: File) => {
    if (!file) return;
    try {
      const parsed = JSON.parse(await file.text()) as { items?: PortableLibraryItem[] };
      if (!Array.isArray(parsed.items)) throw new Error(t('config.portableLibrary.importInvalid'));
      await portableLibrary.import(projectId || undefined, parsed.items);
      await load();
    } catch (e) {
      setError(userError(e));
    }
  };

  const selectedProject = projects.find(project => project.id === projectId);
  const drift = data?.drift ?? 'not_applicable';
  const itemCount = data?.items.length ?? 0;
  const itemCountLabel = t(
    itemCount === 1 ? 'config.portableLibrary.resourceCountOne' : 'config.portableLibrary.resourceCount',
    itemCount,
  );
  const projectDataReady = Boolean(
    projectId && data?.scope === 'project' && data.project_id === projectId,
  );
  const canCheck = projectDataReady && drift !== 'unsynced';
  const canApprove = projectDataReady && drift === 'clean' && !data?.approved;

  return (
    <div id="settings-portable-library" className="set-card set-portable-card">
      <section className="set-section set-portable-section">
        <header className="set-portable-header">
          <span className="set-portable-header-icon" aria-hidden="true"><LibraryBig size={18} /></span>
          <div className="set-portable-header-copy">
            <h2>{t('config.portableLibrary.title')}</h2>
            <p>{t('config.portableLibrary.hint')}</p>
          </div>
          <span className="set-portable-count">{itemCountLabel}</span>
        </header>

        <div className="set-portable-explainer">
          <Info size={17} aria-hidden="true" />
          <div>
            <strong>{t('config.portableLibrary.whatTitle')}</strong>
            <p>{t('config.portableLibrary.whatBody')}</p>
            <div className="set-portable-flow" aria-label={t('config.portableLibrary.flowAria')}>
              <span><Globe2 size={13} />{t('config.portableLibrary.flowGlobal')}</span>
              <ArrowRight size={13} aria-hidden="true" />
              <span><FolderGit2 size={13} />{t('config.portableLibrary.flowProject')}</span>
              <ArrowRight size={13} aria-hidden="true" />
              <span><PackageOpen size={13} />{t('config.portableLibrary.flowFolder')}</span>
            </div>
          </div>
        </div>

        <div className="set-portable-workspace">
          <div className="set-portable-controls">
            <div className="set-portable-block-title">
              <FolderGit2 size={15} aria-hidden="true" />
              <div>
                <strong>{t('config.portableLibrary.chooseTitle')}</strong>
                <small>{t('config.portableLibrary.chooseHint')}</small>
              </div>
            </div>
            <label className="set-portable-field">
              <span>{t('config.portableLibrary.scope')}</span>
              <select
                className="set-input"
                aria-label={t('config.portableLibrary.scopeAria')}
                value={projectId}
                onChange={event => {
                  setData(null);
                  setProjectId(event.target.value);
                }}
              >
                <option value="">{t('config.portableLibrary.scopeGlobal')}</option>
                {projects.map(project => <option key={project.id} value={project.id}>{project.name}</option>)}
              </select>
            </label>
            <label className="set-portable-field">
              <span>{t('config.portableLibrary.search')}</span>
              <span className="set-portable-search">
                <Search size={14} aria-hidden="true" />
                <input
                  className="set-input"
                  aria-label={t('config.portableLibrary.searchAria')}
                  placeholder={t('config.portableLibrary.searchPlaceholder')}
                  value={search}
                  onChange={event => setSearch(event.target.value)}
                />
              </span>
            </label>
          </div>

          <div className="set-portable-status" data-status={drift} data-approved={data?.approved ?? false}>
            <div className="set-portable-block-title">
              {projectId ? <FileCheck2 size={15} aria-hidden="true" /> : <Globe2 size={15} aria-hidden="true" />}
              <div>
                <strong>
                  {projectId
                    ? t('config.portableLibrary.projectStatus', selectedProject?.name ?? '')
                    : t('config.portableLibrary.globalStatus')}
                </strong>
                <small>
                  {projectId
                    ? t(`config.portableLibrary.statusHint.${drift}`)
                    : t('config.portableLibrary.globalStatusHint')}
                </small>
              </div>
            </div>
            <div className="set-portable-status-pills">
              <span data-tone={drift === 'clean' ? 'success' : drift === 'drifted' ? 'warning' : 'neutral'}>
                {drift === 'clean' && <CheckCircle2 size={12} aria-hidden="true" />}
                {t(`config.portableLibrary.status.${drift}`)}
              </span>
              {projectId && (
                <span data-tone={data?.approved ? 'success' : 'warning'}>
                  <ShieldCheck size={12} aria-hidden="true" />
                  {data?.approved
                    ? t('config.portableLibrary.trustApproved')
                    : t('config.portableLibrary.trustRequired')}
                </span>
              )}
            </div>
          </div>
        </div>

        {projectId ? (
          <div className="set-portable-project-actions">
            <div className="set-portable-actions-head">
              <strong>{t('config.portableLibrary.projectActionsTitle')}</strong>
              <span>{t('config.portableLibrary.projectActionsHint')}</span>
            </div>
            <div className="set-portable-action-grid">
              <button
                type="button"
                className="set-portable-action"
                data-primary="true"
                disabled={busy}
                aria-label={t('config.portableLibrary.actionSync')}
                onClick={() => act(t('config.portableLibrary.actionSync'), () => portableLibrary.sync(projectId))}
              >
                <span className="set-portable-action-step">1</span>
                <RefreshCw size={16} aria-hidden="true" />
                <span><strong>{t('config.portableLibrary.actionSync')}</strong><small>{t('config.portableLibrary.actionSyncHint')}</small></span>
              </button>
              <button
                type="button"
                className="set-portable-action"
                disabled={busy || !canCheck}
                aria-label={t('config.portableLibrary.actionCheck')}
                onClick={() => act(t('config.portableLibrary.actionCheck'), () => portableLibrary.check(projectId))}
              >
                <span className="set-portable-action-step">2</span>
                <FileCheck2 size={16} aria-hidden="true" />
                <span><strong>{t('config.portableLibrary.actionCheck')}</strong><small>{t('config.portableLibrary.actionCheckHint')}</small></span>
              </button>
              <button
                type="button"
                className="set-portable-action"
                disabled={busy || !canApprove}
                aria-label={t('config.portableLibrary.actionApprove')}
                onClick={() => act(t('config.portableLibrary.actionApprove'), () => portableLibrary.approve(projectId))}
              >
                <span className="set-portable-action-step">3</span>
                <ShieldCheck size={16} aria-hidden="true" />
                <span><strong>{t('config.portableLibrary.actionApprove')}</strong><small>{t('config.portableLibrary.actionApproveHint')}</small></span>
              </button>
            </div>
          </div>
        ) : (
          <div className="set-portable-project-prompt">
            <FolderGit2 size={16} aria-hidden="true" />
            <span><strong>{t('config.portableLibrary.selectProjectTitle')}</strong><small>{t('config.portableLibrary.selectProjectHint')}</small></span>
          </div>
        )}

        {error && <div className="set-warning-callout set-portable-error" role="alert">{error}</div>}

        <div className="set-portable-library-list">
          <div className="set-portable-list-head">
            <div>
              <strong>{t('config.portableLibrary.listTitle')}</strong>
              <small>{t('config.portableLibrary.listHint')}</small>
            </div>
            <span>{itemCountLabel}</span>
          </div>
          {data && data.items.length > 0 ? (
            <div className="set-portable-items">
              {data.items.map(item => (
                <article className="set-portable-item" key={`${item.kind}-${item.id}`} data-kind={item.kind}>
                  <span className="set-portable-item-icon" aria-hidden="true">{itemIcon(item.kind)}</span>
                  <div className="set-portable-item-copy">
                    <strong>{item.id}</strong>
                    <div className="set-portable-item-meta">
                      <span>{t(`config.portableLibrary.kind.${item.kind}`)}</span>
                      <span>{t(`config.portableLibrary.scope.${item.scope}`)}</span>
                    </div>
                    <code title={item.source}>{item.source}</code>
                  </div>
                </article>
              ))}
            </div>
          ) : (
            <div className="set-portable-empty">
              <PackageOpen size={20} aria-hidden="true" />
              <span><strong>{t('config.portableLibrary.emptyTitle')}</strong><small>{t('config.portableLibrary.emptyHint')}</small></span>
            </div>
          )}
        </div>

        <details className="set-portable-advanced">
          <summary>
            <span><PackageOpen size={15} aria-hidden="true" /></span>
            <span><strong>{t('config.portableLibrary.transferTitle')}</strong><small>{t('config.portableLibrary.transferHint')}</small></span>
            <ChevronDown size={15} className="set-portable-chevron" aria-hidden="true" />
          </summary>
          <div className="set-portable-transfer-actions">
            <button
              type="button"
              disabled={busy}
              className="set-btn-secondary"
              onClick={() => act(t('config.portableLibrary.actionMigrate'), () => portableLibrary.migrate(projectId || undefined))}
            >
              <RefreshCw size={13} /> {t('config.portableLibrary.actionMigrate')}
            </button>
            <button type="button" className="set-btn-secondary" onClick={() => void exportItems()}>
              <Download size={13} /> {t('config.portableLibrary.actionExport')}
            </button>
            <label className="set-btn-secondary set-portable-import">
              <Upload size={13} /> {t('config.portableLibrary.actionImport')}
              <input hidden type="file" accept="application/json" onChange={event => void importFile(event.target.files?.[0])} />
            </label>
          </div>
        </details>
      </section>
    </div>
  );
}
