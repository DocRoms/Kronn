import { useState, useMemo, useRef, useDeferredValue, useEffect } from 'react';
import '../pages/DiscussionsPage.css';
import { SwipeableDiscItem, unseenBasis } from './SwipeableDiscItem';
import type { Discussion, Project, Contact, BatchRunSummary } from '../types/generated';
import { projects as projectsApi } from '../lib/api';
import { getProjectGroup, isHiddenPath } from '../lib/constants';
import { gravatarUrl } from '../lib/gravatar';
import { formatRelativeTime } from '../lib/relativeTime';
import type { ToastFn } from '../hooks/useToast';
import {
  Folder, ChevronLeft, ChevronRight, Plus, X, MessageSquare, Archive, Search, Users2, Trash2, Star, CheckCheck, ListChecks, LogIn, Loader2,
} from 'lucide-react';

export interface DiscussionSidebarProps {
  discussions: Discussion[];
  projects: Project[];
  activeId: string | null;
  sendingMap: Record<string, boolean>;
  /** Batch children created but not yet running (throttled). Rendered
   *  as a distinct "en file" state vs the active "en cours" spinner. */
  queuedMap?: Record<string, boolean>;
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
  /** Unified "join by code": resolves a kr-join token local OR cross-instance
   *  (mirrors the disc back over WS) and opens it. Rejects with a message on
   *  failure (expired / not found). Optional — the button is hidden when absent. */
  onJoinByCode?: (code: string) => Promise<void>;
  /** Click a contact → open (or create) a 1:1 shared discussion with them.
   *  Optional — the row is only clickable when provided. */
  onStartChat?: (contact: Contact) => void;
  onContactDelete: (id: string) => Promise<void>;
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
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
  /** Called when the user clicks "↻" (retry) on a batch group header.
   *  Parent rebuilds the items from the existing children's title +
   *  initial user prompt, then re-fires the QP batch endpoint. The old
   *  batch stays in place; a new batch is spawned alongside it.
   *  Only enabled when `quick_prompt_id` is known on the BatchRunSummary
   *  (top-level manual batches; nested workflow batches need a different
   *  surface). Tya's audit on 2026-05-09 flagged the missing retry. */
  onRetryBatch?: (runId: string, qpId: string, discIds: string[]) => void;
  /** Opens the batch review cockpit. Parent loads the child messages on
   *  demand so the sidebar list stays cheap. */
  onReviewBatch?: (runId: string, label: string, discIds: string[]) => void;
  /** Ref-setter so parent can expand groups when navigating to a discussion */
  collapsedGroups: Set<string>;
  onToggleGroup: (key: string) => void;
  /** Desktop only: collapse sidebar into a thin rail */
  onCollapse?: () => void;
  /** 0.8.3 (#277) — bulk-seed every discussion's last-seen counter to
   *  its current `message_count`. Wired from Dashboard via
   *  DiscussionsPage. Surfaces as a "Mark all as read" button in the
   *  sidebar header, gated on a non-zero total unread count so it
   *  doesn't bait the user when nothing's unread. */
  onMarkAllRead?: () => void;
}

/** Default cap on loose discs per project group in the sidebar. The full
 *  list mounts only when the user explicitly clicks "+N more". On a 500-
 *  discussions seed this drops the initial mount from 4500+ DOM nodes to
 *  ~1000 and the cold render from 4500 ms to under 500 ms. Search bypasses
 *  the cap (the user is explicitly hunting). */
const PROJECT_LOOSE_LIMIT = 10;

function formatBatchParent(summary: BatchRunSummary | undefined, t: (k: string, ...a: (string | number)[]) => string): string | null {
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
  queuedMap = {},
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
  onJoinByCode,
  onStartChat,
  onContactDelete,
  toast,
  t,
  lang = 'fr',
  batchSummaries = [],
  onNavigateWorkflow,
  onDeleteBatch,
  onRetryBatch,
  onReviewBatch,
  collapsedGroups,
  onToggleGroup,
  onCollapse,
  onMarkAllRead,
}: DiscussionSidebarProps) {
  // ─── Sidebar-only state ───────────────────────────────────────────────
  // Search input — kept fresh for the controlled input. The actual filter
  // pipeline reads `deferredSearch` (React 19), which lags behind the input
  // value during heavy renders. Result: the keystroke commits immediately
  // (no input lag), and the expensive 4500-row filter / sort happens in a
  // lower-priority render that React can interrupt if the user keeps typing.
  // Measured before fix on a 250-projects / 500-discussions seed: 2233 ms
  // per keystroke. Goal: <100 ms perceived latency.
  const [discSearchFilter, setDiscSearchFilter] = useState('');
  const deferredSearch = useDeferredValue(discSearchFilter);
  const searchLower = deferredSearch.toLowerCase();

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
  const [showJoin, setShowJoin] = useState(false);
  const [joinCode, setJoinCode] = useState('');
  const [joining, setJoining] = useState(false);
  // Per-project "expanded" set — by default each project group caps at
  // PROJECT_LOOSE_LIMIT loose discs (most users only care about recent
  // activity). Clicking "+N more" adds the project id to this set, which
  // mounts the rest of its discs on demand. Search still shows all
  // matches because the user is explicitly hunting.
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(() => new Set());

  // 0.8.4 (#294) — cross-agent source bindings. Fetched once at mount
  // + on each disc list change so newly-imported discs get the badge
  // without a manual refresh. The map keys on disc.id.
  const [sourceBindings, setSourceBindings] = useState<Map<string, { source_agent: string; diverged: boolean }>>(() => new Map());
  // 0.8.4 (#294) — source filter dropdown. Empty string = "all".
  // Otherwise filters the sidebar to discs whose binding.source_agent
  // matches. The selector populates from the unique set of agents in
  // `sourceBindings`.
  const [sourceFilter, setSourceFilter] = useState<string>('');

  useEffect(() => {
    let cancelled = false;
    projectsApi.discSources()
      .then((rows) => {
        if (cancelled) return;
        const m = new Map<string, { source_agent: string; diverged: boolean }>();
        for (const r of rows ?? []) {
          m.set(r.disc_id, { source_agent: r.source_agent, diverged: r.diverged_at != null });
        }
        setSourceBindings(m);
      })
      .catch((e) => {
        // Non-fatal — the badge just doesn't render. Don't toast,
        // the user has no remediation path.
        console.warn('discSources fetch failed', e);
      });
    return () => { cancelled = true; };
    // Re-run on discussions length change to catch newly-created discs
    // bound via `disc_create` after mount. discussions.length is a cheap
    // proxy for "list shape changed".
  }, [discussions.length]);

  const sourceAgentsAvailable = useMemo(() => {
    const set = new Set<string>();
    for (const b of sourceBindings.values()) set.add(b.source_agent);
    return Array.from(set).sort();
  }, [sourceBindings]);

  // 0.8.4 (#294) — combined predicate for the disc list filters.
  // Search OR source — both AND'd. Used by every render site below
  // so they stay in lockstep with the source filter.
  const matchesFilters = (d: Discussion): boolean => {
    // 0.8.5 — search input also matches an id prefix (hex, lower-case).
    // Lets the user paste / type a short id (`04a9c927` from an agent
    // summary) into the search and land directly on the disc. The
    // ChatHeader pill copies that same prefix-friendly form. We keep
    // the title substring match too — both fire on the same query.
    if (searchLower) {
      const titleHit = d.title.toLowerCase().includes(searchLower);
      const idHit = d.id.toLowerCase().startsWith(searchLower);
      if (!titleHit && !idHit) return false;
    }
    if (sourceFilter) {
      const bind = sourceBindings.get(d.id);
      if (!bind || bind.source_agent !== sourceFilter) return false;
    }
    return true;
  };

  // Waiting for an agent slot. `queuedMap` is the fast path (live WS frame);
  // `awaiting_agent` is the DB truth serialized with the list — it covers
  // frames missed because the page wasn't mounted when the batch launched,
  // reloads, and WS reconnects. Running always wins over queued.
  const isQueuedDisc = (d: Discussion): boolean =>
    !sendingMap[d.id] && (!!queuedMap[d.id] || d.awaiting_agent);

  // Live discussions first: an active agent (spinner) is what the user is
  // waiting on — don't let it drown mid-list. Running > queued > rest,
  // most-recent inside each band.
  const byLiveThenRecent = (a: Discussion, b: Discussion): number => {
    const rank = (d: Discussion) => (sendingMap[d.id] ? 0 : isQueuedDisc(d) ? 1 : 2);
    return rank(a) - rank(b) || b.updated_at.localeCompare(a.updated_at);
  };

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

  // 0.8.3 (#277) — total unseen count across ALL discussions
  // (including archived + batch children) so we know whether to
  // show the "Mark all as read" button. Mirrors `unseenByGroup`'s
  // math except it doesn't exclude archived nor the active disc —
  // the user clicked the button to clear ALL backlog, so we include
  // both. Cheap O(N) reduce, runs alongside the existing one.
  const totalUnseenAll = useMemo(() => {
    let sum = 0;
    for (const disc of discussions) {
      // 0.8.7 — basis excludes System rows (tool calls + summary breadcrumbs).
      // Pre-fix this aggregate read 400+ for ~26 discussions where each
      // workflow run had a handful of agent replies + dozens of System lines.
      const total = unseenBasis(disc);
      const seen = lastSeenMsgCount[disc.id] ?? 0;
      const unseen = total - seen;
      if (unseen > 0) sum += unseen;
    }
    return sum;
  }, [discussions, lastSeenMsgCount]);

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
      const total = unseenBasis(disc);
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
  // Synchronous re-entry guard. Two fast Enter presses (or two clicks on
  // the submit button) would otherwise call `onContactAdd` twice with the
  // same code, creating duplicate contacts and racing the
  // `setAddContactCode('')` state update. The ref short-circuits the
  // second call before the network round-trip starts.
  const addContactInFlightRef = useRef(false);
  const handleContactAdd = async () => {
    if (addContactInFlightRef.current) return;
    if (!addContactCode.trim()) return;
    addContactInFlightRef.current = true;
    try {
      await onContactAdd(addContactCode.trim());
      setAddContactCode('');
      setShowAddContact(false);
    } catch {
      toast(t('contacts.addError'), 'error');
    } finally {
      addContactInFlightRef.current = false;
    }
  };

  // Unified "join by code". The backend resolves the token local OR
  // cross-instance; the latter mirrors the disc back over WS in ~0.5–8 s, so we
  // hold a `joining` ("resolving…") state for the whole await. Surfaces the
  // backend's own error message (expired / not found) rather than a generic one.
  // Ref guard (not the `joining` state, which doesn't flip synchronously)
  // so two fast Enter/clicks can't fire two joins before the first await.
  const joinInFlightRef = useRef(false);
  const handleJoin = async () => {
    if (joinInFlightRef.current || !joinCode.trim() || !onJoinByCode) return;
    joinInFlightRef.current = true;
    setJoining(true);
    try {
      await onJoinByCode(joinCode.trim());
      setJoinCode('');
      setShowJoin(false);
    } catch (e) {
      toast((e as Error)?.message || t('contacts.joinError'), 'error');
    } finally {
      setJoining(false);
      joinInFlightRef.current = false;
    }
  };

  // ─── Render ───────────────────────────────────────────────────────────
  return (
    <div className="disc-sidebar" data-mobile={isMobile}>
      <div className="disc-sidebar-header">
        <span className="disc-sidebar-header-title">Discussions</span>
        <div className="disc-sidebar-header-actions">
          {/* 0.8.3 (#277) — "Mark all as read" button. Only rendered
              when (a) the parent wired the handler AND (b) there's at
              least one unread message anywhere (archived + batch
              children + active included). Without (b) the button is
              just clutter on a clean inbox; without (a) it'd be a
              dead button. Title carries the count so users know what
              they'd clear before clicking. */}
          {onMarkAllRead && totalUnseenAll > 0 && (
            <button
              className="disc-icon-btn"
              onClick={onMarkAllRead}
              aria-label={t('disc.markAllRead')}
              title={t('disc.markAllReadTooltip', totalUnseenAll)}
            >
              <CheckCheck size={14} />
            </button>
          )}
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
            <button
              onClick={() => setDiscSearchFilter('')}
              className="disc-search-clear"
              aria-label={t('disc.searchClear')}
              title={t('disc.searchClear')}
            >
              <X size={10} />
            </button>
          )}
        </div>
        {/* 0.8.4 (#294) — cross-agent source filter. Hidden when no
           imported discs exist (the dropdown would be pointless).
           Filters the disc list to discs whose source_agent matches. */}
        {sourceAgentsAvailable.length > 0 && (
          <select
            data-testid="disc-source-filter"
            className="disc-source-filter-select"
            value={sourceFilter}
            onChange={e => setSourceFilter(e.target.value)}
            title={t('disc.source.filterTooltip')}
            style={{
              marginTop: 4, fontSize: 11, padding: '2px 4px',
              background: 'var(--kr-bg-elevated, transparent)',
              border: '1px solid var(--kr-border-subtle, rgba(255,255,255,0.1))',
              borderRadius: 4, color: 'inherit',
            }}
          >
            <option value="">{t('disc.source.filterAll')}</option>
            {sourceAgentsAvailable.map(agent => (
              <option key={agent} value={agent}>{t('disc.source.filterFrom', agent)}</option>
            ))}
          </select>
        )}
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
              {onJoinByCode && (
                <button
                  onClick={() => { setShowJoin(p => !p); setShowAddContact(false); }}
                  className="disc-contact-add-btn"
                  title={t('contacts.joinByCode')}
                >
                  <LogIn size={12} />
                </button>
              )}
              <button
                onClick={() => { setShowAddContact(p => !p); setShowJoin(false); }}
                className="disc-contact-add-btn"
                title={t('contacts.add')}
              >
                <Plus size={12} />
              </button>
            </span>
          </div>
          {/* Join a discussion by code — unified local/cross-instance join */}
          {showJoin && (
            <div className="disc-contact-add-form">
              <input
                type="text"
                className="disc-contact-add-input"
                value={joinCode}
                onChange={e => setJoinCode(e.target.value)}
                placeholder={t('contacts.joinPlaceholder')}
                disabled={joining}
                onKeyDown={e => {
                  if (e.key === 'Enter' && joinCode.trim()) {
                    handleJoin();
                  }
                }}
              />
              <button
                className="disc-contact-add-submit"
                onClick={handleJoin}
                disabled={joining || !joinCode.trim()}
              >
                {joining
                  ? <span className="disc-join-resolving"><Loader2 size={11} className="disc-join-spin" /> {t('contacts.joinResolving')}</span>
                  : t('contacts.joinByCode')}
              </button>
            </div>
          )}
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
          {/* Contact list — click a row to open a 1:1 chat with that contact */}
          {contacts.map(c => (
            <div
              key={c.id}
              className="disc-contact-row"
              role={onStartChat ? 'button' : undefined}
              style={onStartChat ? { cursor: 'pointer' } : undefined}
              title={onStartChat ? t('contacts.startChat', c.pseudo) : undefined}
              onClick={onStartChat ? () => onStartChat(c) : undefined}
            >
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
                onClick={(e) => { e.stopPropagation(); onContactDelete(c.id); }}
                className="disc-contact-del-btn"
                title={t('contacts.delete')}
              >
                <X size={10} />
              </button>
            </div>
          ))}
        </div>

        {/* Pinned / Favorites — always at the top, cross-project, never collapsed.
            During an active search/source filter, only matching favorites show
            (and the section hides entirely if none match) — otherwise every
            pinned disc stayed visible and buried the actual search results. */}
        {(() => {
          const pinned = discussions.filter(d => d.pinned && !d.archived && matchesFilters(d));
          if (pinned.length === 0) return null;
          return (
            <div>
              <div className="disc-group-header" data-no-border="true">
                <Star size={10} style={{ color: 'var(--kr-warning)' }} />
                <span style={{ fontWeight: 600, fontSize: 'var(--kr-fs-sm)' }}>{t('disc.favorites')}</span>
                <span className="disc-group-count">{pinned.length}</span>
              </div>
              {pinned.sort(byLiveThenRecent).map(disc => (
                <SwipeableDiscItem
                  key={`pin-${disc.id}`}
                  disc={disc}
                  isActive={disc.id === activeId}
                  lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                  isSending={!!sendingMap[disc.id]}
                  isQueued={isQueuedDisc(disc)}
                  onSelect={onSelect}
                  onArchive={onArchive}
                  onDelete={onDelete}
                  onStop={onStopDiscussion}
                  onTogglePin={onTogglePin}
                  t={t}
                  sourceAgent={sourceBindings.get(disc.id)?.source_agent}
                  sourceDiverged={sourceBindings.get(disc.id)?.diverged}
                />
              ))}
            </div>
          );
        })()}

        {/* Global discussions (no project) */}
        {(() => {
          // Filter up front so the header + count + visibility all reflect the
          // search (an empty "Général" group no longer shows during search).
          const globalDiscs = (activeDiscByProject.get(null) ?? []).filter(matchesFilters);
          if (globalDiscs.length === 0) return null;
          const isCollapsed = collapsedGroups.has('__global__') && !deferredSearch;
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
              {!isCollapsed && globalDiscs.sort(byLiveThenRecent).map(disc => (
                <SwipeableDiscItem
                  key={disc.id}
                  disc={disc}
                  isActive={disc.id === activeId}
                  lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                  isSending={!!sendingMap[disc.id]}
                  isQueued={isQueuedDisc(disc)}
                  onSelect={onSelect}
                  onArchive={onArchive}
                  onDelete={onDelete}
                  onStop={onStopDiscussion}
                  onTogglePin={onTogglePin}
                  t={t}
                  sourceAgent={sourceBindings.get(disc.id)?.source_agent}
                  sourceDiverged={sourceBindings.get(disc.id)?.diverged}
                />
              ))}
            </div>
          );
        })()}

        {/* Project discussions — grouped by org */}
        {(() => {
          // `.filter(matchesFilters)` is a no-op when no search/source filter is
          // active (matchesFilters returns true for all), but during a search it
          // hides folders that contain zero matching discs — the user no longer
          // has to scroll past empty project/org headers to find their results.
          const visibleProjects = projects.filter(p => !isHiddenPath(p.path) && (activeDiscByProject.get(p.id) ?? []).filter(matchesFilters).length > 0);
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
            const isOrgCollapsed = collapsedGroups.has(orgKey) && !deferredSearch;
            const orgDiscCount = orgProjects.reduce((sum, p) => sum + (activeDiscByProject.get(p.id) ?? []).filter(matchesFilters).length, 0);
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
                  // Auto-expand a project folder when its active disc is in
                  // it — same reasoning as the batch auto-expand below.
                  const projContainsActive = projDiscs.some(d => d.id === activeId);
                  const isCollapsed = collapsedGroups.has(proj.id) && !deferredSearch && !projContainsActive;
                  return (
                    <div key={proj.id}>
                      <button
                        className="disc-group-btn"
                        onClick={() => onToggleGroup(proj.id)}
                        aria-expanded={!isCollapsed}
                      >
                        <ChevronRight size={10} className="disc-chevron" data-expanded={!isCollapsed} />
                        <Folder size={10} /> {proj.name}
                        <span className="disc-group-count">{projDiscs.filter(matchesFilters).length}</span>
                        {(unseenByGroup.get(proj.id) ?? 0) > 0 && (
                          <span className="disc-group-unseen">{unseenByGroup.get(proj.id)}</span>
                        )}
                      </button>
                      {!isCollapsed && (() => {
                        // Filter + sort, then split into batch groups vs loose discs.
                        const filtered = projDiscs
                          .filter(matchesFilters)
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
                            // "Done" = not running AND not queued AND has at least 2 messages
                            // (user + agent reply). Excluding queuedMap matters on a batch
                            // retry over EXISTING discs (>=2 messages already): a throttled
                            // child would otherwise count as done and the pill jumps ahead.
                            // Rough live heuristic; the real authority is workflow_runs in DB.
                            const done = discs.filter(d => !sendingMap[d.id] && !isQueuedDisc(d) && d.message_count >= 2).length;
                            // Children created but not yet running (throttled). Lets the
                            // group show "n en file" distinctly from "en cours".
                            const running = discs.filter(d => !!sendingMap[d.id]).length;
                            const queued = discs.filter(isQueuedDisc).length;
                            return { runId, discs, anySending, total, done, running, queued };
                          })
                          .sort((a, b) => {
                            // Batches with live children surface first, same
                            // logic as byLiveThenRecent at the disc level.
                            const rank = (g: { anySending: boolean; queued: number }) =>
                              g.anySending ? 0 : g.queued > 0 ? 1 : 2;
                            return rank(a) - rank(b)
                              || b.discs[0].updated_at.localeCompare(a.discs[0].updated_at);
                          });
                        return (
                          <>
                            {/* Batch groups first — dépliables, collapsed by default */}
                            {batchGroups.map(bg => {
                              const batchKey = `batch::${bg.runId}`;
                              // Auto-expand a batch folder when one of its
                              // children is the currently-active disc.
                              // Without this, a user who lands on disc1 of a
                              // freshly-launched 🤝 Compare-agents batch only
                              // sees the *active* disc in the main pane and
                              // a collapsed `📦 …` folder in the sidebar —
                              // they conclude "only one agent ran" even
                              // though N siblings exist inside the folder.
                              const containsActive = bg.discs.some(d => d.id === activeId);
                              const isBatchCollapsed = collapsedGroups.has(batchKey) && !containsActive;
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
                              // While active, split "en cours" from "en file"
                              // so a big batch reads honestly (e.g. "⏳ 3/23 · 5▶ · 15⏸")
                              // instead of 23 identical spinners.
                              const statusPill = (bg.anySending || bg.queued > 0)
                                ? `⏳ ${bg.done}/${bg.total}`
                                  + (bg.running > 0 ? ` · ${bg.running}▶` : '')
                                  + (bg.queued > 0 ? ` · ${bg.queued}⏸` : '')
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
                                      <span className="disc-group-count" data-batch-status={(bg.anySending || bg.queued > 0) ? 'running' : 'done'}>
                                        {statusPill}
                                      </span>
                                    </button>
                                    {onRetryBatch && summaryForLabel?.quick_prompt_id && (
                                      <button
                                        type="button"
                                        className="disc-batch-retry"
                                        title={t('disc.batchRetryHint', bg.total)}
                                        aria-label={t('disc.batchRetryHint', bg.total)}
                                        onClick={(e) => {
                                          e.stopPropagation();
                                          if (confirm(t('disc.batchRetryConfirm', bg.total, label))) {
                                            const qpId = summaryForLabel?.quick_prompt_id;
                                            if (!qpId) return;
                                            onRetryBatch(bg.runId, qpId, bg.discs.map(d => d.id));
                                          }
                                        }}
                                      >
                                        ↻
                                      </button>
                                    )}
                                    {onReviewBatch && (
                                      <button
                                        type="button"
                                        className="disc-batch-review"
                                        title={t('disc.batchReviewHint', bg.total)}
                                        aria-label={t('disc.batchReviewHint', bg.total)}
                                        onClick={(e) => {
                                          e.stopPropagation();
                                          onReviewBatch(bg.runId, label, bg.discs.map(d => d.id));
                                        }}
                                      >
                                        <ListChecks size={11} />
                                      </button>
                                    )}
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
                                      {/* Sorted copy — bg.discs order feeds the
                                          folder label fallback, don't mutate. */}
                                      {[...bg.discs].sort(byLiveThenRecent).map(disc => (
                                        <SwipeableDiscItem
                                          key={disc.id}
                                          disc={disc}
                                          isActive={disc.id === activeId}
                                          lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                                          isSending={!!sendingMap[disc.id]}
                  isQueued={isQueuedDisc(disc)}
                                          onSelect={onSelect}
                                          onArchive={onArchive}
                                          onDelete={onDelete}
                                          onStop={onStopDiscussion}
                                          t={t}
                                          sourceAgent={sourceBindings.get(disc.id)?.source_agent}
                                          sourceDiverged={sourceBindings.get(disc.id)?.diverged}
                                        />
                                      ))}
                                    </div>
                                  )}
                                </div>
                              );
                            })}
                            {/* Loose discs below the batches — capped at
                                PROJECT_LOOSE_LIMIT by default. Search and
                                explicit-expand bypass the cap. */}
                            {(() => {
                              const isExpanded = expandedProjects.has(proj.id);
                              const showAll = isExpanded || !!deferredSearch;
                              // Live-first BEFORE the cap: a running disc must
                              // never be hidden behind "afficher plus".
                              const orderedLoose = [...loose].sort(byLiveThenRecent);
                              const visibleLoose = showAll ? orderedLoose : orderedLoose.slice(0, PROJECT_LOOSE_LIMIT);
                              const hiddenCount = orderedLoose.length - visibleLoose.length;
                              return (
                                <>
                                  {visibleLoose.map(disc => (
                                    <SwipeableDiscItem
                                      key={disc.id}
                                      disc={disc}
                                      isActive={disc.id === activeId}
                                      lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                                      isSending={!!sendingMap[disc.id]}
                  isQueued={isQueuedDisc(disc)}
                                      onSelect={onSelect}
                                      onArchive={onArchive}
                                      onDelete={onDelete}
                                      onStop={onStopDiscussion}
                                      onTogglePin={onTogglePin}
                                      t={t}
                                      sourceAgent={sourceBindings.get(disc.id)?.source_agent}
                                      sourceDiverged={sourceBindings.get(disc.id)?.diverged}
                                    />
                                  ))}
                                  {hiddenCount > 0 && (
                                    <button
                                      className="disc-show-more-btn"
                                      onClick={() => setExpandedProjects(prev => {
                                        const next = new Set(prev);
                                        next.add(proj.id);
                                        return next;
                                      })}
                                      style={{
                                        marginLeft: 32, fontSize: 11,
                                        background: 'transparent', border: 'none',
                                        color: 'var(--kr-text-faint)', cursor: 'pointer',
                                        padding: '4px 0', textAlign: 'left', width: '100%',
                                      }}
                                    >
                                      + {hiddenCount} {t('disc.showMore')}
                                    </button>
                                  )}
                                </>
                              );
                            })()}
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
            {(showArchives || !!deferredSearch) && archivedDiscussions.filter(matchesFilters).sort((a, b) => b.updated_at.localeCompare(a.updated_at)).map(disc => (
              <SwipeableDiscItem
                key={disc.id}
                disc={disc}
                isActive={disc.id === activeId}
                lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                isSending={!!sendingMap[disc.id]}
                  isQueued={isQueuedDisc(disc)}
                onSelect={onSelect}
                onArchive={onUnarchive}
                onDelete={onDelete}
                archiveLabel={t('disc.unarchive')}
                t={t}
                sourceAgent={sourceBindings.get(disc.id)?.source_agent}
                sourceDiverged={sourceBindings.get(disc.id)?.diverged}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
