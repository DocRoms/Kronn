/**
 * KT-1990 — the run folders of the Artifact library: one per run whose step
 * executions a person turned into Artifacts (`<workflow> · run #N`), its
 * Artifacts labelled by execution and model. The header has the batch-folder
 * interaction of the Discussions sidebar: a chevron that opens the folder and
 * a « … » menu with « Open in mosaic » and « Delete the folder ».
 */
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { ChevronRight, FlaskConical, LayoutGrid, MoreHorizontal, Trash2 } from 'lucide-react';
import { standaloneLivePageMosaicUrl } from '../lib/live-page-navigation';
import { folderLabel, type ArtifactRunFolderView } from '../lib/stepArtifacts';
import type { LivePage, StepArtifactFolder, StepArtifactLink } from '../types/generated';

type Translate = (key: string, ...args: (string | number)[]) => string;

export function ArtifactRunFolders({
  views,
  selectedId,
  renderRow,
  onDeleteFolder,
  collapsed,
  onToggleSection,
  searching = false,
  t,
}: {
  views: ArtifactRunFolderView[];
  selectedId: string | null;
  renderRow: (item: { link: StepArtifactLink; page: LivePage }) => ReactNode;
  /** Resolves `true` once the folder is deleted. */
  onDeleteFolder: (folder: StepArtifactFolder) => Promise<boolean>;
  collapsed: boolean;
  onToggleSection: () => void;
  /** KT-1999 — while the library search holds a query every folder is open, as the project groups are. */
  searching?: boolean;
  t: Translate;
}) {
  // A folder opens on request, or by itself while it holds the Artifact shown
  // or the library is searched.
  const [openRuns, setOpenRuns] = useState<ReadonlySet<string>>(() => new Set());
  const [menuRunId, setMenuRunId] = useState<string | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const triggers = useRef(new Map<string, HTMLButtonElement>());

  useEffect(() => {
    if (!menuRunId) return undefined;
    menuRef.current?.querySelector<HTMLElement>('.disc-batch-menu-panel > button:not(:disabled)')?.focus();
    const outside = (event: MouseEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) setMenuRunId(null);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      const runId = menuRunId;
      setMenuRunId(null);
      requestAnimationFrame(() => triggers.current.get(runId)?.focus());
    };
    document.addEventListener('mousedown', outside);
    document.addEventListener('keydown', escape);
    return () => {
      document.removeEventListener('mousedown', outside);
      document.removeEventListener('keydown', escape);
    };
  }, [menuRunId]);

  if (views.length === 0) return null;
  const toggle = (runId: string) => setOpenRuns(current => {
    const next = new Set(current);
    if (next.has(runId)) next.delete(runId); else next.add(runId);
    return next;
  });
  const total = views.reduce((sum, view) => sum + view.items.length, 0);

  return (
    <div className="disc-sidebar-section live-pages-run-folders" data-expanded={!collapsed} data-testid="artifact-run-folders">
      <button type="button" className="disc-group-btn" data-no-border="true" onClick={onToggleSection} aria-expanded={!collapsed}>
        <ChevronRight size={10} className="disc-chevron" data-expanded={!collapsed} />
        <FlaskConical size={10} />
        <span>{t('pages.runFolders.title')}</span><span className="disc-group-count">{total}</span>
      </button>
      {!collapsed && views.map(({ folder, items }) => {
        const label = folderLabel(folder, t);
        const open = searching || openRuns.has(folder.run_id) || items.some(item => item.page.id === selectedId);
        const ids = items.map(item => item.page.id);
        return (
          <div key={folder.run_id} className="disc-batch-wrap" data-run-folder={folder.run_id} data-testid={`artifact-run-folder-${folder.run_id}`}>
            <div className="disc-batch-header" data-compact="true">
              <button
                type="button"
                className="disc-group-btn"
                data-variant="batch"
                onClick={() => toggle(folder.run_id)}
                aria-expanded={open}
                style={{ marginLeft: 0, flex: 1 }}
                title={label}
              >
                <ChevronRight size={10} className="disc-chevron" data-expanded={open} />
                <span className="disc-batch-run-label">{label}</span>
                <span className="disc-group-count">{items.length}</span>
              </button>
              <div className="disc-batch-menu" ref={menuRunId === folder.run_id ? menuRef : undefined}>
                <button
                  type="button"
                  className="disc-batch-menu-trigger"
                  ref={node => {
                    if (node) triggers.current.set(folder.run_id, node);
                    else triggers.current.delete(folder.run_id);
                  }}
                  aria-label={t('pages.runFolders.actions')}
                  aria-expanded={menuRunId === folder.run_id}
                  aria-controls={`run-folder-actions-${folder.run_id}`}
                  data-testid={`artifact-run-folder-menu-${folder.run_id}`}
                  onClick={event => {
                    event.stopPropagation();
                    setMenuRunId(current => current === folder.run_id ? null : folder.run_id);
                  }}
                >
                  <MoreHorizontal size={14} />
                </button>
                {menuRunId === folder.run_id && (
                  <div id={`run-folder-actions-${folder.run_id}`} className="disc-batch-menu-panel" role="group" aria-label={t('pages.runFolders.actions')}>
                    <button
                      type="button"
                      data-testid={`artifact-run-folder-mosaic-${folder.run_id}`}
                      disabled={ids.length < 2}
                      title={ids.length < 2 ? t('pages.mosaic.minimum') : undefined}
                      onClick={() => {
                        setMenuRunId(null);
                        window.open(standaloneLivePageMosaicUrl(ids), '_blank', 'noopener,noreferrer');
                      }}
                    >
                      <LayoutGrid size={12} /> <span>{t('pages.runFolders.openMosaic')}</span>
                    </button>
                    <button
                      type="button"
                      data-danger="true"
                      data-testid={`artifact-run-folder-delete-${folder.run_id}`}
                      onClick={() => {
                        setMenuRunId(null);
                        void onDeleteFolder(folder);
                      }}
                    >
                      <Trash2 size={12} /> <span>{t('pages.runFolders.deleteFolder')}</span>
                    </button>
                  </div>
                )}
              </div>
            </div>
            {open && <div className="disc-batch-children">{items.map(renderRow)}</div>}
          </div>
        );
      })}
    </div>
  );
}
