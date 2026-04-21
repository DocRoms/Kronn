import { useState, useMemo } from 'react';
import '../pages/DiscussionsPage.css';
import { SwipeableDiscItem } from './SwipeableDiscItem';
import type { Discussion, Project, Contact, BatchRunSummary } from '../types/generated';
import { getProjectGroup, isHiddenPath } from '../lib/constants';
import { gravatarUrl } from '../lib/gravatar';
import { formatRelativeTime } from '../lib/relativeTime';
import type { ToastFn } from '../hooks/useToast';
import {
  Folder, ChevronLeft, ChevronRight, Plus, X, MessageSquare, Archive, Search, Users2, Trash2, Star,
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
  onTogglePin: (discId: string, pinned: boolean) => void;
  onNewDiscussion: () => void;
  onClose: () => void;
  /** Called when the user clicks the ⏹ stop button inline on a disc that
   *  is currently Running (isSending). Parent calls `discussionsApi.stop`
   *  and updates sendingMap on success. */
  onStopDiscussion?: (discId: string) => void;
  onContactAdd: (code: string) => Promise<void>;
  onContactDelete: (id: string) => Promise<void>;
  toast: ToastFn;
  t: (key: string, ...args: any[]) => string;
  /** Active Kronn locale — used for the batch group relative-time formatter. */
  lang?: string;
  /** Batch run summaries (run_id → parent workflow meta). Populated by the
   *  parent with `quickPromptsApi.listBatchRunSummaries()` so each batch group
   *  in the sidebar can show a clickable pastille pointing back to the
   *  workflow run that spawned it. */
  batchSummaries?: BatchRunSummary[];
  /** Called when the user clicks the "↗ run #N · {workflow}" pastille on a
   *  batch group. Parent is expected to switch to the workflows tab + open
   *  the detail panel for that workflow. */
  onNavigateWorkflow?: (workflowId: string) => void;
  /** Called when the user clicks "🗑" on a batch group header and confirms.
   *  Parent calls the DELETE /api/workflow-runs/:run_id endpoint, then
   *  refetches discussions + batchSummaries so the group disappears live. */
  onDeleteBatch?: (runId: string, discCount: number) => void;
  /** Ref-setter so parent can expand groups when navigating to a discussion */
  collapsedGroups: Set<string>;
  onToggleGroup: (key: string) => void;
  /** Desktop only: collapse sidebar into a thin rail */
  onCollapse?: () => void;
}

function formatBatchParent(summary: BatchRunSummary | undefined, t: (k: string, ...a: any[]) => string): string | null {
  if (!summary) return null;
  const seq = summary.parent_run_sequence;
  const name = summary.parent_workflow_name;
  if (!name) return null;
  return seq != null
    ? t('disc.batchFromWorkflowRun', seq, name)
    : t('disc.batchFromWorkflow', name);
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
  onTogglePin,
  onNewDiscussion,
  onClose,
  onStopDiscussion,
  onContactAdd,
  onContactDelete,
  toast,
  t,
  lang = 'fr',
  batchSummaries = [],
  onNavigateWorkflow,
  onDeleteBatch,
  collapsedGroups,
  onToggleGroup,
  onCollapse,
}: DiscussionSidebarProps) {
  // ─── Sidebar-only state ───────────────────────────────────────────────
  const [discSearchFilter, setDiscSearchFilter] = useState('');

  // Map batch run_id → parent workflow meta. Built from props so the parent
  // can refetch (e.g. on WS batch progress events) and the sidebar updates.
  const batchMetaById = useMemo(() => {
    const m = new Map<string, BatchRunSummary>();
    for (const s of batchSummaries) m.set(s.run_id, s);
    return m;
  }, [batchSummaries]);
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

  // Unseen count PER GROUP KEY — used to badge collapsed group headers so
  // the user can tell at a glance which group hides unread conversations.
  // Keys mirror the ones used by `collapsedGroups`: `"__global__"` for
  // global, `"org::OrgName"` for org headers, `projectId` for projects.
  const unseenByGroup = useMemo(() => {
    const map = new Map<string, number>();
    const add = (key: string, count: number) => {
      map.set(key, (map.get(key) ?? 0) + count);
    };
    for (const disc of discussions) {
      if (disc.archived) continue;
      if (disc.id === activeId) continue; // active disc is always "seen"
      const total = disc.message_count ?? disc.messages.length;
      const seen = lastSeenMsgCount[disc.id] ?? 0;
      const unseen = total - seen;
      if (unseen <= 0) continue;

      // Global group
      if (!disc.project_id) {
        add('__global__', unseen);
        continue;
      }

      // Project group + org group
      add(disc.project_id, unseen);
      const proj = projects.find(p => p.id === disc.project_id);
      if (proj) {
        const org = getProjectGroup(proj, t('disc.local'), t('disc.local'));
        add(`org::${org}`, unseen);
      }
    }
    return map;
  }, [discussions, activeId, lastSeenMsgCount, projects, t]);

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
          <button className="disc-scan-btn" data-tour-id="new-disc-btn" onClick={onNewDiscussion}>
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

        {/* Pinned / Favorites — always at the top, cross-project, never collapsed */}
        {(() => {
          const pinned = discussions.filter(d => d.pinned && !d.archived);
          if (pinned.length === 0) return null;
          return (
            <div>
              <div className="disc-group-header" data-no-border="true">
                <Star size={10} style={{ color: 'var(--kr-warning)' }} />
                <span style={{ fontWeight: 600, fontSize: 'var(--kr-fs-sm)' }}>{t('disc.favorites')}</span>
                <span className="disc-group-count">{pinned.length}</span>
              </div>
              {pinned.sort((a, b) => b.updated_at.localeCompare(a.updated_at)).map(disc => (
                <SwipeableDiscItem
                  key={`pin-${disc.id}`}
                  disc={disc}
                  isActive={disc.id === activeId}
                  lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                  isSending={!!sendingMap[disc.id]}
                  onSelect={onSelect}
                  onArchive={onArchive}
                  onDelete={onDelete}
                  onStop={onStopDiscussion}
                  onTogglePin={onTogglePin}
                  t={t}
                />
              ))}
            </div>
          );
        })()}

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
                {(unseenByGroup.get('__global__') ?? 0) > 0 && (
                  <span className="disc-group-unseen">{unseenByGroup.get('__global__')}</span>
                )}
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
                  onStop={onStopDiscussion}
                  onTogglePin={onTogglePin}
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
            const orgColor = orgName === localLabel ? 'var(--kr-text-dim)'
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
                    {(unseenByGroup.get(orgKey) ?? 0) > 0 && (
                      <span className="disc-group-unseen">{unseenByGroup.get(orgKey)}</span>
                    )}
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
                        {(unseenByGroup.get(proj.id) ?? 0) > 0 && (
                          <span className="disc-group-unseen">{unseenByGroup.get(proj.id)}</span>
                        )}
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
                              const summaryForLabel = batchMetaById.get(bg.runId);
                              // Folder label: prefer the Quick Prompt name (the campaign)
                              // over the first child disc title (one ticket among N,
                              // misleading — was showing "EW-7100" for a 50-disc batch).
                              // Falls back to the old disc-title derivation if we don't
                              // have the summary yet (e.g. fetch in flight on first render).
                              const firstTitle = bg.discs[0].title;
                              const qpIcon = summaryForLabel?.quick_prompt_icon;
                              const qpName = summaryForLabel?.quick_prompt_name;
                              // When a QP icon is available we use IT as the folder
                              // glyph instead of the generic 📦 — avoids stacking two
                              // emojis like "📦 🎯 Analyse...".
                              const folderGlyph = qpIcon || '📦';
                              const label = qpName ?? firstTitle.split('—')[0].trim();
                              // Relative timestamp of the batch — disambiguates between
                              // multiple batches of the same QP (e.g. cron firing every 10min).
                              // We use the earliest disc's created_at since that's when the batch
                              // was spawned. Full ISO shown on hover for precision.
                              const batchStartIso = bg.discs
                                .map(d => d.created_at)
                                .sort()[0] ?? bg.discs[0].created_at;
                              const batchWhen = formatRelativeTime(batchStartIso, lang);
                              const batchWhenAbs = (() => {
                                try { return new Date(batchStartIso).toLocaleString(lang); }
                                catch { return batchStartIso; }
                              })();
                              const statusPill = bg.anySending
                                ? `⏳ ${bg.done}/${bg.total}`
                                : bg.done === bg.total
                                  ? `✓ ${bg.total}/${bg.total}`
                                  : `${bg.done}/${bg.total}`;
                              const summary = batchMetaById.get(bg.runId);
                              const parentLabel = formatBatchParent(summary, t);
                              const parentWorkflowId = summary?.parent_workflow_id ?? null;
                              return (
                                <div key={batchKey} className="disc-batch-wrap" data-batch-key={batchKey}>
                                  <div className="disc-batch-header" style={{ marginLeft: 12 }}>
                                    <button
                                      className="disc-group-btn"
                                      data-variant="batch"
                                      onClick={() => onToggleGroup(batchKey)}
                                      aria-expanded={!isBatchCollapsed}
                                      style={{ marginLeft: 0, flex: 1 }}
                                      title={`${label} — ${batchWhenAbs}`}
                                    >
                                      <ChevronRight size={10} className="disc-chevron" data-expanded={!isBatchCollapsed} />
                                      {folderGlyph} {label}
                                      {batchWhen && (
                                        <span className="disc-batch-when" title={batchWhenAbs}>
                                          · {batchWhen}
                                        </span>
                                      )}
                                      <span className="disc-group-count" data-batch-status={bg.anySending ? 'running' : 'done'}>
                                        {statusPill}
                                      </span>
                                    </button>
                                    {onDeleteBatch && (
                                      <button
                                        type="button"
                                        className="disc-batch-delete"
                                        title={t('disc.batchDeleteHint', bg.total)}
                                        aria-label={t('disc.batchDeleteHint', bg.total)}
                                        onClick={(e) => {
                                          e.stopPropagation();
                                          // Native confirm — keeps the dependency
                                          // surface tiny. Future polish: a styled
                                          // modal with the disc preview list.
                                          if (confirm(t('disc.batchDeleteConfirm', bg.total, label))) {
                                            onDeleteBatch(bg.runId, bg.total);
                                          }
                                        }}
                                      >
                                        <Trash2 size={11} />
                                      </button>
                                    )}
                                  </div>
                                  {parentLabel && parentWorkflowId && onNavigateWorkflow && (
                                    <button
                                      type="button"
                                      className="disc-batch-parent-pill"
                                      style={{ marginLeft: 24 }}
                                      onClick={(e) => {
                                        // stopPropagation isn't strictly needed (we're not
                                        // nested in another button), but future refactors
                                        // could move this inside the group button — keeping
                                        // it defensive.
                                        e.stopPropagation();
                                        onNavigateWorkflow(parentWorkflowId);
                                      }}
                                      title={t('disc.batchParentClickHint')}
                                    >
                                      ↗ {parentLabel}
                                    </button>
                                  )}
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
                                          onStop={onStopDiscussion}
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
                                onStop={onStopDiscussion}
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
