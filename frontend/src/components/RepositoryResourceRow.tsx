import { useEffect, useRef, useState } from 'react';
import { Check, Copy, Ellipsis, Loader2 } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { formatResourceDate } from '../lib/formatResourceDate';
import { splitPath, writesRepository, type ResourceRow } from '../lib/repositoryResourceRows';

export type RowMenuAction = 'view' | 'compare' | 'copy_native';

interface Props {
  row: ResourceRow;
  checked: boolean;
  busy: boolean;
  canWrite: boolean;
  onToggle: () => void;
  onOpen: () => void;
  onPrimary: () => void;
  onMenu: (action: RowMenuAction) => void;
}

function CopyPathButton({ path }: { path: string }) {
  const { t } = useT();
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | null>(null);
  useEffect(() => () => {
    if (timer.current !== null) window.clearTimeout(timer.current);
  }, []);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(path);
      setCopied(true);
      if (timer.current !== null) window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setCopied(false), 1200);
    } catch {
      // Clipboard permission can be refused; the full path stays in the tooltip.
    }
  };
  return (
    <button type="button" className="rr-icon-button" onClick={copy} aria-label={t('projects.repositoryResources.copyPath')}>
      {copied ? <Check size={12} aria-hidden="true" /> : <Copy size={12} aria-hidden="true" />}
    </button>
  );
}

function PathLine({ row }: { row: ResourceRow }) {
  const { t } = useT();
  // No file on the repository side: nothing to print, so no line either.
  if (!row.displayPath) return null;
  const { head, tail } = splitPath(row.displayPath);
  return (
    <span className="rr-path" data-pending={!row.pathExists || undefined}>
      <code title={row.displayPath}>
        <span className="rr-path-head">{head}</span>
        <span className="rr-path-tail">{tail}</span>
      </code>
      {!row.pathExists && (
        <span className="rr-path-note">{t('projects.repositoryResources.willBeCreated')}</span>
      )}
      {row.pathExists && <CopyPathButton path={row.displayPath} />}
    </span>
  );
}

function RowMenu({ row, onMenu }: Pick<Props, 'row' | 'onMenu'>) {
  const { t } = useT();
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const closeFromOutside = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const closeFromKeyboard = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false);
    };
    window.addEventListener('pointerdown', closeFromOutside);
    window.addEventListener('keydown', closeFromKeyboard);
    return () => {
      window.removeEventListener('pointerdown', closeFromOutside);
      window.removeEventListener('keydown', closeFromKeyboard);
    };
  }, [open]);

  const hasDiff = row.fileDiffs.length > 0 || row.fieldDiff.length > 0 || Boolean(row.diff);
  const items: Array<{ id: RowMenuAction; label: string }> = [];
  if (row.primary !== 'view') items.push({ id: 'view', label: t('projects.repositoryResources.action.view') });
  if ((row.state === 'repository_newer' || row.state === 'kronn_newer') && hasDiff) {
    items.push({ id: 'compare', label: t('projects.repositoryResources.action.compareOnly') });
  }
  if (row.state === 'native_skill') {
    items.push({ id: 'copy_native', label: t('projects.repositoryResources.action.copy_native') });
  }
  if (items.length === 0) return null;

  return (
    <div className="rr-menu" ref={root}>
      <button
        type="button"
        className="rr-icon-button"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={t('projects.repositoryResources.moreActions', row.name)}
        onClick={() => setOpen(current => !current)}
      >
        <Ellipsis size={16} aria-hidden="true" />
      </button>
      {open && (
        <div className="rr-menu-list" role="menu">
          {items.map(item => (
            <button
              key={item.id}
              type="button"
              role="menuitem"
              onClick={() => { setOpen(false); onMenu(item.id); }}
            >
              {item.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** One catalog line: Dépôt | Synchro | Kronn | Action. */
export function RepositoryResourceRow({ row, checked, busy, canWrite, onToggle, onOpen, onPrimary, onMenu }: Props) {
  const { t, locale } = useT();
  const blocked = writesRepository(row.primary) && !canWrite;
  const openFromRow = (event: React.MouseEvent<HTMLDivElement>) => {
    if ((event.target as HTMLElement).closest('button, input, label, a')) return;
    onOpen();
  };

  return (
    <div className="rr-row" role="row" data-state={row.state} data-kind={row.kind} onClick={openFromRow}>
      <div className="rr-cell rr-cell-select" role="cell">
        <input
          type="checkbox"
          checked={checked}
          disabled={row.state !== 'kronn_only' || row.suggested}
          onChange={onToggle}
          aria-label={t('projects.repositoryResources.include', row.name)}
        />
      </div>
      <div className="rr-cell rr-cell-repository" role="cell">
        <span className="rr-name">
          <button type="button" className="rr-name-button" onClick={onOpen}>{row.name}</button>
          {row.origins.map(origin => <span key={origin} className="rr-origin">{origin}</span>)}
        </span>
        <PathLine row={row} />
        {row.pathsDiverge && (
          <span className="rr-diverge">{t('projects.repositoryResources.originDiverge', row.origins.join(', '))}</span>
        )}
        {row.description && <small className="rr-description" title={row.description}>{row.description}</small>}
      </div>
      <div className="rr-cell rr-cell-sync" role="cell">
        <span className="rr-pill" data-state={row.state}>
          {t(`projects.repositoryResources.status.${row.state}`)}
        </span>
      </div>
      <div className="rr-cell rr-cell-kronn" role="cell">
        <span>
          {t(`projects.repositoryResources.scope.${row.scope}`)}
          {row.builtin && ` · ${t('projects.repositoryResources.builtin')}`}
        </span>
        {row.suggested && row.suggestedReason && (
          <small>{t('projects.repositoryResources.suggestedBecause', row.suggestedReason)}</small>
        )}
        {!row.suggested && row.kronnUpdatedAt && (
          <small>{formatResourceDate(row.kronnUpdatedAt, locale)}</small>
        )}
      </div>
      <div className="rr-cell rr-cell-action" role="cell">
        {row.primary === 'view' ? (
          <button type="button" className="rr-view" onClick={onOpen}>
            {t('projects.repositoryResources.action.view')}
          </button>
        ) : (
          <button
            type="button"
            className="rr-action"
            data-action={row.primary}
            disabled={busy || blocked}
            title={blocked ? t('projects.repositoryResources.banner.writeDisabled.title') : undefined}
            onClick={onPrimary}
          >
            {busy && <Loader2 size={13} className="animate-spin" aria-hidden="true" />}
            {t(`projects.repositoryResources.action.${row.primary}`)}
          </button>
        )}
        <RowMenu row={row} onMenu={onMenu} />
      </div>
    </div>
  );
}
