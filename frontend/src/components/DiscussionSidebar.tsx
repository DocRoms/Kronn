import { useState, useMemo } from 'react';
import '../pages/DiscussionsPage.css';
import { SwipeableDiscItem } from './SwipeableDiscItem';
import type { Discussion, Project, Contact } from '../types/generated';
import { getProjectGroup, isHiddenPath } from '../lib/constants';
import { gravatarUrl } from '../lib/gravatar';
import type { ToastFn } from '../hooks/useToast';
import {
  Folder, ChevronLeft, ChevronRight, Plus, X, MessageSquare, Archive, Search, Users2,
} from 'lucide-react';

export interface DiscussionSidebarProps {
  discussions: Discussion[];
  projects: Project[];
  activeId: string | null;
  sendingMap: Record<string, boolean>;
  lastSeenMsgCount: Record<string, number>;
  contacts: Contact[];
  contactsOnline: Record<string, boolean>;
  wsConnected: boolean;
  isMobile: boolean;
  onSelect: (discId: string, msgCount: number) => void;
  onArchive: (discId: string) => void;
  onUnarchive: (discId: string) => void;
  onDelete: (discId: string) => void;
  onNewDiscussion: () => void;
  onClose: () => void;
  onContactAdd: (code: string) => Promise<void>;
  onContactDelete: (id: string) => Promise<void>;
  toast: ToastFn;
  t: (key: string, ...args: any[]) => string;
  /** Ref-setter so parent can expand groups when navigating to a discussion */
  collapsedGroups: Set<string>;
  onToggleGroup: (key: string) => void;
  /** Desktop only: collapse sidebar into a thin rail */
  onCollapse?: () => void;
}

export function DiscussionSidebar({
  discussions,
  projects,
  activeId,
  sendingMap,
  lastSeenMsgCount,
  contacts,
  contactsOnline,
  wsConnected,
  isMobile,
  onSelect,
  onArchive,
  onUnarchive,
  onDelete,
  onNewDiscussion,
  onClose,
  onContactAdd,
  onContactDelete,
  toast,
  t,
  collapsedGroups,
  onToggleGroup,
  onCollapse,
}: DiscussionSidebarProps) {
  // ─── Sidebar-only state ───────────────────────────────────────────────
  const [discSearchFilter, setDiscSearchFilter] = useState('');
  const [showArchives, setShowArchives] = useState(false);
  const [showAddContact, setShowAddContact] = useState(false);
  const [addContactCode, setAddContactCode] = useState('');

  // ─── Derived data ─────────────────────────────────────────────────────
  const { activeDiscByProject, archivedDiscussions } = useMemo(() => {
    const activeMap = new Map<string | null, Discussion[]>();
    const archived: Discussion[] = [];
    for (const d of discussions) {
      if (d.archived) {
        archived.push(d);
      } else {
        const key = d.project_id ?? null;
        const list = activeMap.get(key) ?? [];
        list.push(d);
        activeMap.set(key, list);
      }
    }
    return { activeDiscByProject: activeMap, archivedDiscussions: archived };
  }, [discussions]);

  // ─── Contact handlers ─────────────────────────────────────────────────
  const handleContactAdd = async () => {
    if (!addContactCode.trim()) return;
    try {
      await onContactAdd(addContactCode.trim());
      setAddContactCode('');
      setShowAddContact(false);
    } catch {
      toast(t('contacts.addError'), 'error');
    }
  };

  // ─── Render ───────────────────────────────────────────────────────────
  return (
    <div className="disc-sidebar" data-mobile={isMobile}>
      <div className="disc-sidebar-header">
        <span className="disc-sidebar-header-title">Discussions</span>
        <div className="disc-sidebar-header-actions">
          <button className="disc-scan-btn" onClick={onNewDiscussion}>
            <Plus size={12} /> {t('disc.new')}
          </button>
          {isMobile && (
            <button className="disc-icon-btn" onClick={onClose} aria-label="Close sidebar"><X size={16} /></button>
          )}
          {!isMobile && onCollapse && (
            <button className="disc-icon-btn" onClick={onCollapse} aria-label="Collapse sidebar" title="Collapse sidebar">
              <ChevronLeft size={16} />
            </button>
          )}
        </div>
      </div>

      {/* Search filter */}
      <div className="disc-search-wrap">
        <div className="disc-search-box">
          <Search size={11} className="disc-search-icon" />
          <input
            type="text"
            className="disc-search-input"
            value={discSearchFilter}
            onChange={e => setDiscSearchFilter(e.target.value)}
            placeholder={t('disc.searchPlaceholder')}
          />
          {discSearchFilter && (
            <button onClick={() => setDiscSearchFilter('')} className="disc-search-clear">
              <X size={10} />
            </button>
          )}
        </div>
      </div>

      {/* Discussion list grouped by project */}
      <div className="disc-sidebar-list">
        {/* Contacts section — always visible */}
        <div>
          <div className="disc-group-header" data-no-border="true">
            <Users2 size={10} /> {t('contacts.title')}
            <span className="disc-contacts-meta">
              {contacts.length > 0 && (
                <>
                  <span className="disc-ws-dot" data-connected={wsConnected} title={wsConnected ? t('contacts.wsConnected') : t('contacts.wsDisconnected')} />
                  {contacts.filter(c => contactsOnline[c.id]).length}/{contacts.length}
                </>
              )}
              <button
                onClick={() => setShowAddContact(p => !p)}
                className="disc-contact-add-btn"
                title={t('contacts.add')}
              >
                <Plus size={12} />
              </button>
            </span>
          </div>
          {/* Add contact inline form */}
          {showAddContact && (
            <div className="disc-contact-add-form">
              <input
                type="text"
                className="disc-contact-add-input"
                value={addContactCode}
                onChange={e => setAddContactCode(e.target.value)}
                placeholder={t('contacts.addPlaceholder')}
                onKeyDown={e => {
                  if (e.key === 'Enter' && addContactCode.trim()) {
                    handleContactAdd();
                  }
                }}
              />
              <button
                className="disc-contact-add-submit"
                onClick={handleContactAdd}
              >
                {t('contacts.add')}
              </button>
            </div>
          )}
          {/* Contact list */}
          {contacts.map(c => (
            <div key={c.id} className="disc-contact-row">
              <span className="disc-contact-dot" data-online={contactsOnline[c.id] ?? false} />
              {c.avatar_email ? (
                <img src={gravatarUrl(c.avatar_email, 20)} alt="" className="disc-contact-avatar" />
              ) : (
                <span className="disc-contact-initials">
                  {c.pseudo.slice(0, 2).toUpperCase()}
                </span>
              )}
              <span className="disc-contact-name">{c.pseudo}</span>
              {c.status === 'pending' && !contactsOnline[c.id] && (
                <span className="disc-contact-pending" title="Contact injoignable — vérifiez que les deux machines sont sur le même réseau">{t('contacts.pending')}</span>
              )}
              {c.status === 'accepted' && !contactsOnline[c.id] && (
                <span className="disc-contact-offline">offline</span>
              )}
              <button
                onClick={() => onContactDelete(c.id)}
                className="disc-contact-del-btn"
                title={t('contacts.delete')}
              >
                <X size={10} />
              </button>
            </div>
          ))}
        </div>

        {/* Global discussions (no project) */}
        {(() => {
          const globalDiscs = activeDiscByProject.get(null) ?? [];
          if (globalDiscs.length === 0) return null;
          const isCollapsed = collapsedGroups.has('__global__') && !discSearchFilter;
          return (
            <div>
              <button
                className="disc-group-btn"
                data-no-border="true"
                onClick={() => onToggleGroup('__global__')}
                aria-expanded={!isCollapsed}
              >
                <ChevronRight size={10} className="disc-chevron" data-expanded={!isCollapsed} />
                <MessageSquare size={10} /> {t('disc.general')}
                <span className="disc-group-count">{globalDiscs.length}</span>
              </button>
              {!isCollapsed && globalDiscs.filter(d => !discSearchFilter || d.title.toLowerCase().includes(discSearchFilter.toLowerCase())).sort((a, b) => b.updated_at.localeCompare(a.updated_at)).map(disc => (
                <SwipeableDiscItem
                  key={disc.id}
                  disc={disc}
                  isActive={disc.id === activeId}
                  lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                  isSending={!!sendingMap[disc.id]}
                  onSelect={onSelect}
                  onArchive={onArchive}
                  onDelete={onDelete}
                  t={t}
                />
              ))}
            </div>
          );
        })()}

        {/* Project discussions — grouped by org */}
        {(() => {
          const visibleProjects = projects.filter(p => !isHiddenPath(p.path) && (activeDiscByProject.get(p.id) ?? []).length > 0);
          // Build org groups
          const orgMap = new Map<string, typeof visibleProjects>();
          for (const p of visibleProjects) {
            const org = getProjectGroup(p, t('disc.local'), t('disc.local'));
            const list = orgMap.get(org) ?? [];
            list.push(p);
            orgMap.set(org, list);
          }
          // Sort orgs alphabetically, "Local" last
          const localLabel = t('disc.local');
          const sortedOrgs = [...orgMap.entries()].sort(([a], [b]) => {
            if (a === localLabel) return 1;
            if (b === localLabel) return -1;
            return a.localeCompare(b);
          });

          return sortedOrgs.map(([orgName, orgProjects]) => {
            const orgKey = `org::${orgName}`;
            const isOrgCollapsed = collapsedGroups.has(orgKey) && !discSearchFilter;
            const orgDiscCount = orgProjects.reduce((sum, p) => sum + (activeDiscByProject.get(p.id) ?? []).length, 0);
            // Color from org name hash (same as Dashboard)
            const orgColor = orgName === localLabel ? 'rgba(255,255,255,0.3)'
              : `hsl(${[...orgName].reduce((h, c) => (h * 31 + c.charCodeAt(0)) % 360, 0)}, 50%, 60%)`;

            return (
              <div key={orgKey}>
                {sortedOrgs.length > 1 && (
                  <button
                    className="disc-org-header"
                    style={{ color: orgColor }}
                    onClick={() => onToggleGroup(orgKey)}
                    aria-expanded={!isOrgCollapsed}
                  >
                    <ChevronRight size={9} className="disc-chevron" data-expanded={!isOrgCollapsed} />
                    {orgName}
                    <span className="disc-group-count">{orgDiscCount}</span>
                  </button>
                )}
                {!isOrgCollapsed && orgProjects.map(proj => {
                  const projDiscs = activeDiscByProject.get(proj.id) ?? [];
                  const isCollapsed = collapsedGroups.has(proj.id) && !discSearchFilter;
                  return (
                    <div key={proj.id}>
                      <button
                        className="disc-group-btn"
                        onClick={() => onToggleGroup(proj.id)}
                        aria-expanded={!isCollapsed}
                      >
                        <ChevronRight size={10} className="disc-chevron" data-expanded={!isCollapsed} />
                        <Folder size={10} /> {proj.name}
                        <span className="disc-group-count">{projDiscs.length}</span>
                      </button>
                      {!isCollapsed && (() => {
                        // Filter + sort, then split into batch groups vs loose discs.
                        const filtered = projDiscs
                          .filter(d => !discSearchFilter || d.title.toLowerCase().includes(discSearchFilter.toLowerCase()))
                          .sort((a, b) => b.updated_at.localeCompare(a.updated_at));
                        // Group by workflow_run_id — discs without one are "loose".
                        const batchMap = new Map<string, typeof filtered>();
                        const loose: typeof filtered = [];
                        for (const d of filtered) {
                          if (d.workflow_run_id) {
                            const arr = batchMap.get(d.workflow_run_id) ?? [];
                            arr.push(d);
                            batchMap.set(d.workflow_run_id, arr);
                          } else {
                            loose.push(d);
                          }
                        }
                        // Compute batch live status from its child discs:
                        //   - in_progress: at least one disc in sendingMap (true)
                        //   - or all "terminal" discs done — we approximate via sendingMap
                        const batchGroups = Array.from(batchMap.entries())
                          .map(([runId, discs]) => {
                            const anySending = discs.some(d => !!sendingMap[d.id]);
                            const total = discs.length;
                            // "Done" = not in sendingMap AND has at least 2 messages (user + agent reply).
                            // This is a rough live heuristic; the real authority is workflow_runs in DB.
                            const done = discs.filter(d => !sendingMap[d.id] && d.message_count >= 2).length;
                            return { runId, discs, anySending, total, done };
                          })
                          .sort((a, b) => b.discs[0].updated_at.localeCompare(a.discs[0].updated_at));
                        return (
                          <>
                            {/* Batch groups first — dépliables, collapsed by default */}
                            {batchGroups.map(bg => {
                              const batchKey = `batch::${bg.runId}`;
                              const isBatchCollapsed = collapsedGroups.has(batchKey);
                              // First disc's creation time is a close-enough proxy for the batch start
                              const firstTitle = bg.discs[0].title;
                              // Use a generic label derived from the first disc's title —
                              // "Bootstrap: EW-7100" → "Batch (12) — Bootstrap..."
                              const label = firstTitle.split('—')[0].trim();
                              const statusPill = bg.anySending
                                ? `⏳ ${bg.done}/${bg.total}`
                                : bg.done === bg.total
                                  ? `✓ ${bg.total}/${bg.total}`
                                  : `${bg.done}/${bg.total}`;
                              return (
                                <div key={batchKey} className="disc-batch-wrap">
                                  <button
                                    className="disc-group-btn"
                                    data-variant="batch"
                                    onClick={() => onToggleGroup(batchKey)}
                                    aria-expanded={!isBatchCollapsed}
                                    style={{ marginLeft: 12 }}
                                    title={label}
                                  >
                                    <ChevronRight size={10} className="disc-chevron" data-expanded={!isBatchCollapsed} />
                                    📦 {label}
                                    <span className="disc-group-count" data-batch-status={bg.anySending ? 'running' : 'done'}>
                                      {statusPill}
                                    </span>
                                  </button>
                                  {!isBatchCollapsed && (
                                    // Wrapper with a left "tree line" + indent so the
                                    // batch children read as "inside" the 📦 folder,
                                    // not as siblings of the loose discs below.
                                    <div className="disc-batch-children">
                                      {bg.discs.map(disc => (
                                        <SwipeableDiscItem
                                          key={disc.id}
                                          disc={disc}
                                          isActive={disc.id === activeId}
                                          lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                                          isSending={!!sendingMap[disc.id]}
                                          onSelect={onSelect}
                                          onArchive={onArchive}
                                          onDelete={onDelete}
                                          t={t}
                                        />
                                      ))}
                                    </div>
                                  )}
                                </div>
                              );
                            })}
                            {/* Loose discs below the batches */}
                            {loose.map(disc => (
                              <SwipeableDiscItem
                                key={disc.id}
                                disc={disc}
                                isActive={disc.id === activeId}
                                lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                                isSending={!!sendingMap[disc.id]}
                                onSelect={onSelect}
                                onArchive={onArchive}
                                onDelete={onDelete}
                                t={t}
                              />
                            ))}
                          </>
                        );
                      })()}
                    </div>
                  );
                })}
              </div>
            );
          });
        })()}

        {discussions.length === 0 && (
          <div className="disc-empty">{t('disc.empty')}</div>
        )}

        {/* Archives section */}
        {archivedDiscussions.length > 0 && (
          <div>
            <button
              className="disc-group-btn"
              data-variant="archive"
              onClick={() => setShowArchives(!showArchives)}
              aria-expanded={showArchives}
            >
              <ChevronRight size={10} className="disc-chevron" data-expanded={showArchives} />
              <Archive size={10} /> {t('disc.archived')}
              <span className="disc-group-count">{archivedDiscussions.length}</span>
            </button>
            {(showArchives || !!discSearchFilter) && archivedDiscussions.filter(d => !discSearchFilter || d.title.toLowerCase().includes(discSearchFilter.toLowerCase())).sort((a, b) => b.updated_at.localeCompare(a.updated_at)).map(disc => (
              <SwipeableDiscItem
                key={disc.id}
                disc={disc}
                isActive={disc.id === activeId}
                lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                isSending={!!sendingMap[disc.id]}
                onSelect={onSelect}
                onArchive={onUnarchive}
                onDelete={onDelete}
                archiveLabel={t('disc.unarchive')}
                t={t}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
