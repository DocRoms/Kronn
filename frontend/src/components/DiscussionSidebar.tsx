import { Fragment, useState, useMemo, useRef, useEffect, useCallback } from 'react';
import '../pages/DiscussionsPage.css';
import { SwipeableDiscItem } from './SwipeableDiscItem';
import { unseenBasis } from '../lib/discussionUiUtils';
import { GlobalSearchPanel } from './GlobalSearchPanel';
import { CollectionShell, CollectionSidebarCollapseButton } from './CollectionShell';
import type { Discussion, Project, Contact, BatchRunSummary, ExecutionDiscussionLink, MessageSearchHit } from '../types/generated';
import { projects as projectsApi } from '../lib/api';
import { getProjectGroup, isHiddenPath } from '../lib/constants';
import { gravatarUrl } from '../lib/gravatar';
import { formatRelativeTime } from '../lib/relativeTime';
import type { ToastFn } from '../hooks/useToast';
import {
  Folder, ChevronRight, Plus, X, MessageSquare, Archive, Search,
  SlidersHorizontal, Users2, Trash2, Star, CheckCheck, Columns3, ListChecks, LogIn,
  Loader2, Upload, CircleDot, Clock3, MoreHorizontal, ChevronDown,
} from 'lucide-react';

export interface DiscussionSidebarProps {
  discussions: Discussion[];
  projects: Project[];
  activeId: string | null;
  sendingMap: Record<string, boolean>;
  /** Batch children created but not yet running (throttled). Rendered
   *  as a distinct "en file" state vs the active "en cours" spinner. */
  queuedMap?: Record<string, boolean>;
  /** Durable parent/child execution lineage. Children are rendered exactly
   * once under their principal room; missing parents stay ordinary rows. */
  executionLinks?: ExecutionDiscussionLink[];
  lastSeenMsgCount: Record<string, number>;
  contacts: Contact[];
  contactsOnline: Record<string, boolean>;
  wsConnected: boolean;
  isMobile: boolean;
  onSelect: (discId: string, msgCount: number) => void;
  onArchive: (discId: string) => void;
  onUnarchive: (discId: string) => void;
  onDelete: (discId: string) => void;
  onBulkArchive?: (discIds: string[]) => Promise<void>;
  onBulkDelete?: (discIds: string[]) => Promise<void>;
  /** Opens a durable free comparison for the selected discussions. */
  onCompareSelected?: (discIds: string[]) => Promise<void>;
  onTogglePin: (discId: string, pinned: boolean) => void;
  onNewDiscussion: () => void;
  onImportDiscussion?: (file: File) => Promise<void>;
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
  /** Opens the generic side-by-side result workspace. Unlike triage review,
   * this preserves and renders each answer as rich Markdown. */
  onCompareBatch?: (runId: string, label: string, discIds: string[]) => void;
  /** Ref-setter so parent can expand groups when navigating to a discussion */
  collapsedGroups: Set<string>;
  onToggleGroup: (key: string) => void;
  /** Batch-run expansion is intentionally separate from persisted group
   *  collapse preferences: run ids are ephemeral and would otherwise grow
   *  localStorage forever. */
  openBatchRuns?: ReadonlySet<string>;
  onToggleBatchRun?: (runId: string) => void;
  /** Desktop only: collapse sidebar into a thin rail */
  onCollapse?: () => void;
  /** 0.8.3 (#277) — bulk-seed every discussion's last-seen counter to
   *  its current `message_count`. Wired from Dashboard via
   *  DiscussionsPage. Surfaces as a "Mark all as read" button in the
   *  sidebar header, gated on a non-zero total unread count so it
   *  doesn't bait the user when nothing's unread. */
  onMarkAllRead?: () => void;
  /** KT-70 — expands the sidebar's shared search field into the advanced
   *  server-side message search. Optional: the affordance is hidden when the
   *  parent doesn't wire it. */
  onOpenGlobalSearch?: () => void;
  globalSearchOpen?: boolean;
  globalSearchAuthors?: string[];
  onCloseGlobalSearch?: () => void;
  onOpenGlobalSearchResult?: (hit: MessageSearchHit) => void;
}

/** Default cap on loose discs per project group in the sidebar. The full
 *  list mounts only when the user explicitly clicks "+N more". On a 500-
 *  discussions seed this drops the initial mount from 4500+ DOM nodes to
 *  ~1000 and the cold render from 4500 ms to under 500 ms. Search bypasses
 *  the cap (the user is explicitly hunting). */
const PROJECT_LOOSE_LIMIT = 10;
const SMART_SECTION_LIMIT = 5;
const EMPTY_BATCH_RUNS: ReadonlySet<string> = new Set();

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
  executionLinks = [],
  lastSeenMsgCount,
  contacts,
  contactsOnline,
  wsConnected,
  isMobile,
  onSelect,
  onArchive,
  onUnarchive,
  onDelete,
  onBulkArchive,
  onBulkDelete,
  onCompareSelected,
  onTogglePin,
  onNewDiscussion,
  onImportDiscussion,
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
  onCompareBatch,
  collapsedGroups,
  onToggleGroup,
  openBatchRuns = EMPTY_BATCH_RUNS,
  onToggleBatchRun = () => {},
  onCollapse,
  onMarkAllRead,
  onOpenGlobalSearch,
  globalSearchOpen = false,
  globalSearchAuthors = [],
  onCloseGlobalSearch,
  onOpenGlobalSearchResult,
}: DiscussionSidebarProps) {
  // ─── Sidebar-only state ───────────────────────────────────────────────
  // One global query shared by the compact entry field and the result panel.
  // Typing does NOT filter/remount the potentially huge local tree: Enter (or
  // the filter button) opens the bounded backend search over title, id and
  // message content. This keeps keystrokes instant on 500+ discussions and
  // matches what the placeholder promises.
  const [discSearchFilter, setDiscSearchFilter] = useState('');

  // Map batch run_id → parent workflow meta. Built from props so the parent
  // can refetch (e.g. on WS batch progress events) and the sidebar updates.
  const batchMetaById = useMemo(() => {
    const m = new Map<string, BatchRunSummary>();
    for (const s of batchSummaries) m.set(s.run_id, s);
    return m;
  }, [batchSummaries]);
  const [showArchives, setShowArchives] = useState(false);
  const [archivedVisibleCount, setArchivedVisibleCount] = useState(50);
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
  const [expandedSmartSections, setExpandedSmartSections] = useState<Set<string>>(() => new Set());
  const [selectionMode, setSelectionMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(() => new Set());
  const [bulkActionBusy, setBulkActionBusy] = useState(false);
  const bulkActionInFlightRef = useRef(false);
  const importInputRef = useRef<HTMLInputElement>(null);
  const importInFlightRef = useRef(false);
  const [importing, setImporting] = useState(false);
  const [headerMenuOpen, setHeaderMenuOpen] = useState(false);
  const headerMenuRef = useRef<HTMLDivElement>(null);
  const headerMenuTriggerRef = useRef<HTMLButtonElement>(null);
  const [openBatchMenuRunId, setOpenBatchMenuRunId] = useState<string | null>(null);
  const batchMenuRef = useRef<HTMLDivElement>(null);
  const batchMenuTriggerRefs = useRef<Map<string, HTMLButtonElement>>(new Map());
  const searchInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!headerMenuOpen) return;
    headerMenuRef.current?.querySelector<HTMLButtonElement>('.disc-sidebar-header-menu > button')?.focus();
    const closeFromOutside = (event: PointerEvent) => {
      if (!headerMenuRef.current?.contains(event.target as Node)) setHeaderMenuOpen(false);
    };
    const closeFromKeyboard = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      setHeaderMenuOpen(false);
      requestAnimationFrame(() => headerMenuTriggerRef.current?.focus());
    };
    window.addEventListener('pointerdown', closeFromOutside);
    window.addEventListener('keydown', closeFromKeyboard);
    return () => {
      window.removeEventListener('pointerdown', closeFromOutside);
      window.removeEventListener('keydown', closeFromKeyboard);
    };
  }, [headerMenuOpen]);

  useEffect(() => {
    if (!openBatchMenuRunId) return;
    batchMenuRef.current?.querySelector<HTMLButtonElement>('.disc-batch-menu-panel > button')?.focus();
    const closeFromOutside = (event: PointerEvent) => {
      if (!batchMenuRef.current?.contains(event.target as Node)) setOpenBatchMenuRunId(null);
    };
    const closeFromKeyboard = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      const runId = openBatchMenuRunId;
      setOpenBatchMenuRunId(null);
      requestAnimationFrame(() => batchMenuTriggerRefs.current.get(runId)?.focus());
    };
    window.addEventListener('pointerdown', closeFromOutside);
    window.addEventListener('keydown', closeFromKeyboard);
    return () => {
      window.removeEventListener('pointerdown', closeFromOutside);
      window.removeEventListener('keydown', closeFromKeyboard);
    };
  }, [openBatchMenuRunId]);

  const toggleSelection = useCallback((discId: string) => {
    setSelectedIds(previous => {
      const next = new Set(previous);
      if (next.has(discId)) next.delete(discId);
      else next.add(discId);
      return next;
    });
  }, []);

  const leaveSelectionMode = useCallback(() => {
    setSelectionMode(false);
    setSelectedIds(new Set());
  }, []);

  const runBulkAction = async (kind: 'archive' | 'delete') => {
    const action = kind === 'archive' ? onBulkArchive : onBulkDelete;
    if (!action || selectedIds.size === 0 || bulkActionInFlightRef.current) return;
    const confirmKey = kind === 'archive'
      ? 'disc.bulk.confirmArchive'
      : 'disc.bulk.confirmDelete';
    if (!confirm(t(confirmKey, selectedIds.size))) return;

    bulkActionInFlightRef.current = true;
    setBulkActionBusy(true);
    try {
      await action([...selectedIds]);
      leaveSelectionMode();
    } catch {
      toast(t('disc.bulk.error'), 'error');
    } finally {
      bulkActionInFlightRef.current = false;
      setBulkActionBusy(false);
    }
  };

  const compareSelected = async () => {
    if (!onCompareSelected || selectedIds.size < 2 || bulkActionInFlightRef.current) return;
    bulkActionInFlightRef.current = true;
    setBulkActionBusy(true);
    try {
      await onCompareSelected([...selectedIds]);
      leaveSelectionMode();
    } catch {
      toast(t('disc.compare.selectionError'), 'error');
    } finally {
      bulkActionInFlightRef.current = false;
      setBulkActionBusy(false);
    }
  };

  // 0.8.4 (#294) — cross-agent source bindings. Fetched once at mount
  // + on each disc list change so newly-imported discs get the badge
  // without a manual refresh. The map keys on disc.id.
  // KT-85 — a LIST per disc, not one binding: a cross-agent room carries one
  // per joined CLI session, and keying a single value here silently showed only
  // whichever row the API returned last.
  const [sourceBindings, setSourceBindings] = useState<Map<string, { source_agent: string; diverged: boolean }[]>>(() => new Map());
  // 0.8.4 (#294) — source filter dropdown. Empty string = "all".
  // Otherwise filters the sidebar to discs whose binding.source_agent
  // matches. The selector populates from the unique set of agents in
  // `sourceBindings`.
  const [sourceFilter, setSourceFilter] = useState<string>('');

  // KT-74 — provenance of PORTABLE imports, kept separate from the bindings
  // above on purpose: a disc can be bound to a CLI session, imported from a
  // colleague's bundle, or both, and the row must not merge the two.
  const [importProvenance, setImportProvenance] = useState<
    Map<string, { pseudo: string | null; avatarEmail: string | null }>
  >(() => new Map());

  const refreshImportProvenance = useCallback(() => {
    projectsApi.discImports()
      .then((rows) => {
        const m = new Map<string, { pseudo: string | null; avatarEmail: string | null }>();
        for (const row of rows ?? []) {
          // Only the portable route has a human to attribute; the reserved
          // `agent_transcript` kind is not implemented and must not render.
          if (row.provenance_kind !== 'portable_bundle') continue;
          m.set(row.disc_id, {
            pseudo: row.imported_by_pseudo,
            avatarEmail: row.imported_by_avatar_email,
          });
        }
        setImportProvenance(m);
      })
      .catch((e) => {
        console.warn('discImports fetch failed', e);
      });
  }, []);

  useEffect(() => {
    refreshImportProvenance();
  }, [discussions.length, refreshImportProvenance]);

  const refreshSourceBindings = useCallback(() => {
    projectsApi.discSources()
      .then((rows) => {
        const m = new Map<string, { source_agent: string; diverged: boolean }[]>();
        for (const r of rows ?? []) {
          const entry = { source_agent: r.source_agent, diverged: r.diverged_at != null };
          const list = m.get(r.disc_id);
          // Same agent twice (an older bridge session of the same CLI) must not
          // render two identical chips.
          if (!list) m.set(r.disc_id, [entry]);
          else if (!list.some(b => b.source_agent === entry.source_agent)) list.push(entry);
        }
        setSourceBindings(m);
      })
      .catch((e) => {
        // Non-fatal — the badge just doesn't render. Don't toast,
        // the user has no remediation path.
        console.warn('discSources fetch failed', e);
      });
  }, []);

  useEffect(() => {
    refreshSourceBindings();
    window.addEventListener('kronn:disc-source-changed', refreshSourceBindings);
    return () => {
      window.removeEventListener('kronn:disc-source-changed', refreshSourceBindings);
    };
    // Re-run on discussions length change to catch newly-created discs
    // bound via `disc_create` after mount. discussions.length is a cheap
    // proxy for "list shape changed".
  }, [discussions.length, refreshSourceBindings]);

  const sourceAgentsAvailable = useMemo(() => {
    const set = new Set<string>();
    for (const list of sourceBindings.values()) for (const b of list) set.add(b.source_agent);
    return Array.from(set).sort();
  }, [sourceBindings]);

  // 0.8.4 (#294) — local predicate for the optional source filter. The main
  // search field is intentionally absent here: it is a real global backend
  // search, not a client-side card filter.
  const matchesFilters = (d: Discussion): boolean => {
    if (sourceFilter) {
      const bind = sourceBindings.get(d.id);
      if (!bind || !bind.some(b => b.source_agent === sourceFilter)) return false;
    }
    return true;
  };

  // Waiting for an agent slot. `queuedMap` is the fast path (live WS frame);
  // `awaiting_agent` is the DB truth serialized with the list — it covers
  // frames missed because the page wasn't mounted when the batch launched,
  // reloads, and WS reconnects. Running always wins over queued.
  //
  // `awaiting_agent` stays set for the WHOLE run (it clears only at a terminal
  // state), so it cannot on its own tell "enqueued" from "running" — that is
  // what `agent_running` answers, the dispatch status carried on the list row.
  const isQueuedDisc = (d: Discussion): boolean =>
    !sendingMap[d.id] && !d.agent_running && (!!queuedMap[d.id] || d.awaiting_agent);

  // A run this client never opened a stream for — started from the API, or
  // from another tab, or before a reload — leaves `sendingMap` empty. The DB
  // knows better: `agent_running` is the job's real status.
  const isRunningDisc = (d: Discussion): boolean =>
    !!sendingMap[d.id] || !!d.agent_running;

  // Live discussions first: an active agent (spinner) is what the user is
  // waiting on — don't let it drown mid-list. Running > queued > rest,
  // most-recent inside each band.
  const byLiveThenRecent = (a: Discussion, b: Discussion): number => {
    const rank = (d: Discussion) => (isRunningDisc(d) ? 0 : isQueuedDisc(d) ? 1 : 2);
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
  const discussionById = new Map(discussions.map(discussion => [discussion.id, discussion]));
  const executionChildrenByParent = new Map<string, ExecutionDiscussionLink[]>();
  const nestedExecutionChildIds = new Set<string>();
  for (const link of executionLinks) {
    const parent = discussionById.get(link.parent_discussion_id);
    const child = discussionById.get(link.sub_discussion_id);
    // An absent/archived parent is an orphan in the active tree: keep the child
    // as a normal row so lineage damage never makes work disappear.
    if (!parent || !child || parent.archived || child.archived || !matchesFilters(parent)) continue;
    const siblings = executionChildrenByParent.get(parent.id) ?? [];
    if (!siblings.some(candidate => candidate.sub_discussion_id === child.id)) {
      siblings.push(link);
      executionChildrenByParent.set(parent.id, siblings);
      nestedExecutionChildIds.add(child.id);
    }
  }
  const contactsGroupKey = '__contacts__';
  const contactsCollapsed = collapsedGroups.has(contactsGroupKey) && !showJoin && !showAddContact;
  const onlineContactCount = contacts.filter(contact => contactsOnline[contact.id]).length;
  const smartCandidates = discussions.filter(disc => (
    !disc.archived && !nestedExecutionChildIds.has(disc.id) && matchesFilters(disc)
  ));
  // Rendering roots and counting the canonical tree are different concerns:
  // execution children render under their parent, but their unread work must
  // still reach the collapsed Projects badge and total.
  const canonicalCandidates = discussions.filter(disc => (
    !disc.archived && (nestedExecutionChildIds.has(disc.id) || matchesFilters(disc))
  ));
  const projectNameById = new Map(projects.map(project => [project.id, project.name]));
  const projectsGroupKey = '__projects__';
  const projectsCollapsed = collapsedGroups.has(projectsGroupKey);
  const canonicalUnseen = canonicalCandidates.reduce((sum, disc) => {
    if (disc.id === activeId) return sum;
    return sum + Math.max(0, unseenBasis(disc) - (lastSeenMsgCount[disc.id] ?? 0));
  }, 0);
  // Smart shortcuts earn their duplication only when the canonical tree is
  // genuinely large. On a small workspace, Projects/General already fits on
  // screen; rendering the same rows twice adds noise and duplicate keyboard
  // targets. Selection mode also stays canonical so one discussion maps to one
  // checkbox.
  const smartSectionsEnabled =
    !selectionMode && discussions.filter(disc => !disc.archived).length >= 20;
  const followUpDiscussions = (smartSectionsEnabled ? smartCandidates : [])
    .filter((disc) => {
      if (isRunningDisc(disc) || isQueuedDisc(disc)) return true;
      // A favorite has its own stable shortcut section. Keep it out of the
      // unread catch-all so Favoris does not disappear on a fresh workspace
      // where every old discussion is technically unseen.
      if (disc.pinned) return false;
      if (disc.id === activeId) return false;
      return unseenBasis(disc) > (lastSeenMsgCount[disc.id] ?? 0);
    })
    .sort(byLiveThenRecent);
  const followUpIds = new Set(followUpDiscussions.map(disc => disc.id));
  // Favoris is a shortcut, not the tree. `smartCandidates` drops execution
  // children so the canonical tree does not render them twice — correct there,
  // but it also made a pinned sub-discussion impossible to reach from Favoris.
  // Pinning one is an explicit request for a direct route to it, so the
  // shortcut reads from a base that keeps them.
  const favoriteCandidates = selectionMode
    ? []
    : discussions.filter(disc => !disc.archived && matchesFilters(disc));
  const favoriteDiscussions = favoriteCandidates
    .filter(disc => disc.pinned && !followUpIds.has(disc.id))
    .sort(byLiveThenRecent);
  const favoriteIds = new Set(favoriteDiscussions.map(disc => disc.id));
  const recentDiscussions = (smartSectionsEnabled ? smartCandidates : [])
    .filter(disc => !followUpIds.has(disc.id) && !favoriteIds.has(disc.id))
    .sort((a, b) => b.updated_at.localeCompare(a.updated_at))
    .slice(0, 10);

  const renderSmartRows = (rows: Discussion[], keyPrefix: string) => rows.map(disc => (
    <SwipeableDiscItem
      key={`${keyPrefix}-${disc.id}`}
      disc={disc}
      isActive={disc.id === activeId}
      lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
      isSending={isRunningDisc(disc)}
      isQueued={isQueuedDisc(disc)}
      selectionMode={selectionMode}
      isSelected={selectedIds.has(disc.id)}
      onToggleSelection={toggleSelection}
      onSelect={onSelect}
      onArchive={onArchive}
      onDelete={onDelete}
      onStop={onStopDiscussion}
      onTogglePin={onTogglePin}
      t={t}
      collectionRowClassName="collection-shell-row-button"
      contextLabel={disc.project_id ? projectNameById.get(disc.project_id) : t('disc.noProject')}
      sourceAgents={sourceBindings.get(disc.id)}
      importedBy={importProvenance.get(disc.id) ?? null}
    />
  ));

  const renderCanonicalRow = (disc: Discussion) => {
    const children = (executionChildrenByParent.get(disc.id) ?? [])
      .filter(link => {
        const child = discussionById.get(link.sub_discussion_id);
        return child ? matchesFilters(child) : false;
      });
    // A parent used to expand the sidebar permanently, one row per execution,
    // with no way to fold it back. The toggle appears only past a single child:
    // hiding one execution behind a click would trade one annoyance for
    // another, and a parent with one execution is the common case.
    //
    // Children stay visible by default — collapsing on their behalf would hide
    // work they just launched. Reuses `collapsedGroups`, so a parent they DO
    // fold stays folded exactly like the Projects and Favoris sections.
    const childGroupKey = `__exec_children__:${disc.id}`;
    const collapsible = children.length > 1;
    const childrenCollapsed = collapsible && collapsedGroups.has(childGroupKey);
    return (
      <Fragment key={disc.id}>
        <SwipeableDiscItem
          disc={disc}
          isActive={disc.id === activeId}
          lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
          isSending={isRunningDisc(disc)}
          isQueued={isQueuedDisc(disc)}
          selectionMode={selectionMode}
          isSelected={selectedIds.has(disc.id)}
          onToggleSelection={toggleSelection}
          onSelect={onSelect}
          onArchive={onArchive}
          onDelete={onDelete}
          onStop={onStopDiscussion}
          onTogglePin={onTogglePin}
          t={t}
          collectionRowClassName="collection-shell-row-button"
          sourceAgents={sourceBindings.get(disc.id)}
          importedBy={importProvenance.get(disc.id) ?? null}
        />
        {collapsible && (
          <button
            type="button"
            className="disc-orchestration-toggle"
            aria-expanded={!childrenCollapsed}
            onClick={() => onToggleGroup(childGroupKey)}
          >
            <ChevronDown size={12} className={childrenCollapsed ? 'is-collapsed' : undefined} />
            <span>{t('orch.sidebar.executionCount', children.length)}</span>
          </button>
        )}
        {children.length > 0 && !childrenCollapsed && (
          <div className="disc-orchestration-children" aria-label={t('orch.sidebar.executions')}>
            {children.map(link => {
              const child = discussionById.get(link.sub_discussion_id);
              if (!child) return null;
              return (
                <div className="disc-orchestration-child" key={link.execution_id}>
                  <div className="disc-orchestration-child-meta">
                    <span>{link.task_reference}</span>
                    <span title={link.task_title}>{link.task_title}</span>
                    <span data-status={link.status}>{link.status}</span>
                  </div>
                  <SwipeableDiscItem
                    disc={child}
                    isActive={child.id === activeId}
                    lastSeenCount={lastSeenMsgCount[child.id] ?? 0}
                    isSending={isRunningDisc(child)}
                    isQueued={isQueuedDisc(child)}
                    selectionMode={selectionMode}
                    isSelected={selectedIds.has(child.id)}
                    onToggleSelection={toggleSelection}
                    onSelect={onSelect}
                    onArchive={onArchive}
                    onDelete={onDelete}
                    onStop={onStopDiscussion}
                    onTogglePin={onTogglePin}
                    t={t}
                    collectionRowClassName="collection-shell-row-button"
                    sourceAgents={sourceBindings.get(child.id)}
                    importedBy={importProvenance.get(child.id) ?? null}
                  />
                </div>
              );
            })}
          </div>
        )}
      </Fragment>
    );
  };
  const renderSmartSectionRows = (rows: Discussion[], keyPrefix: string) => {
    const expanded = expandedSmartSections.has(keyPrefix);
    const visible = expanded ? rows : rows.slice(0, SMART_SECTION_LIMIT);
    const hiddenCount = rows.length - visible.length;
    return (
      <>
        {renderSmartRows(visible, keyPrefix)}
        {hiddenCount > 0 && (
          <button
            type="button"
            className="disc-show-more-btn disc-smart-more"
            onClick={() => setExpandedSmartSections(previous => {
              const next = new Set(previous);
              next.add(keyPrefix);
              return next;
            })}
          >
            + {hiddenCount} {t('disc.showMore')}
          </button>
        )}
      </>
    );
  };

  return (
    <CollectionShell<Discussion>
      ariaLabel="Discussions"
      items={discussions}
      getId={disc => disc.id}
      getLabel={disc => disc.title}
      filterQuery={false}
      persistence={{
        query: discSearchFilter,
        onQueryChange: setDiscSearchFilter,
        favoritesOnly: false,
        onFavoritesOnlyChange: () => {},
      }}
      selectedId={activeId}
      onSelect={id => onSelect(id, 0)}
      selectedIds={selectedIds}
      onSelectedIdsChange={setSelectedIds}
      sidebarOnly
      sidebarClassName="disc-sidebar"
      isMobile={isMobile}
      globalSearchShortcut
      searchInputRef={searchInputRef}
      shortcutsEnabled={!globalSearchOpen}
      showControls={false}
      onSearchSubmit={onOpenGlobalSearch}
      labels={{
        search: t('disc.globalSearch.placeholder'),
        favorites: t('disc.favorites'),
        clearFilters: t('disc.searchClear'),
        moreActions: t('disc.sidebar.moreActions'),
        openCollection: t('disc.openSidebar'),
        closeCollection: t('disc.closeSidebar'),
        selectItem: t('disc.select'),
      }}
      slots={{
        renderDetail: () => null,
        beforeSidebarHeader: <>
      <div className="disc-sidebar-header" data-selection-mode={selectionMode}>
        <span className="disc-sidebar-header-title">
          {selectionMode ? (
            t('disc.bulk.selected', selectedIds.size)
          ) : (
            <>
              Discussions
              <span className="disc-sidebar-header-count">
                {' · '}{discussions.length}
              </span>
            </>
          )}
        </span>
        <div className="disc-sidebar-header-actions">
          {selectionMode ? (
            <>
              <button
                type="button"
                className="disc-icon-btn"
                onClick={() => void compareSelected()}
                disabled={selectedIds.size < 2 || bulkActionBusy || !onCompareSelected}
                aria-label={t('disc.bulk.compare')}
                title={selectedIds.size < 2 ? t('disc.compare.selectAtLeastTwo') : t('disc.bulk.compare')}
              >
                <Columns3 size={14} />
              </button>
              <button
                type="button"
                className="disc-icon-btn"
                onClick={() => void runBulkAction('archive')}
                disabled={selectedIds.size === 0 || bulkActionBusy || !onBulkArchive}
                aria-label={t('disc.bulk.archive')}
                title={t('disc.bulk.archive')}
              >
                {bulkActionBusy ? <Loader2 size={14} className="spin" /> : <Archive size={14} />}
              </button>
              <button
                type="button"
                className="disc-icon-btn disc-bulk-delete-btn"
                onClick={() => void runBulkAction('delete')}
                disabled={selectedIds.size === 0 || bulkActionBusy || !onBulkDelete}
                aria-label={t('disc.bulk.delete')}
                title={t('disc.bulk.delete')}
              >
                <Trash2 size={14} />
              </button>
              <button
                type="button"
                className="disc-icon-btn"
                onClick={leaveSelectionMode}
                disabled={bulkActionBusy}
                aria-label={t('disc.bulk.cancel')}
                title={t('disc.bulk.cancel')}
              >
                <X size={14} />
              </button>
            </>
          ) : (
            <>
              {onImportDiscussion && (
                <input
                  ref={importInputRef}
                  type="file"
                  accept=".json,.kronn-discussion.json,application/json"
                  className="disc-sidebar-visually-hidden"
                  tabIndex={-1}
                  aria-hidden="true"
                  onChange={async event => {
                    const file = event.target.files?.[0];
                    event.target.value = '';
                    if (!file || importInFlightRef.current) return;
                    importInFlightRef.current = true;
                    setImporting(true);
                    try {
                      await onImportDiscussion(file);
                    } catch (error) {
                      toast(t('disc.portability.importError', String(error)), 'error');
                    } finally {
                      importInFlightRef.current = false;
                      setImporting(false);
                    }
                  }}
                />
              )}
              <button
                type="button"
                className="disc-icon-btn disc-sidebar-new-btn"
                data-tour-id="new-disc-btn"
                onClick={onNewDiscussion}
                aria-label={t('disc.new')}
                title={t('disc.new')}
              >
                <Plus size={16} />
                <span className="disc-sidebar-visually-hidden">{t('disc.new')}</span>
              </button>
              <div className="disc-sidebar-header-menu-wrap" ref={headerMenuRef}>
                <button
                  type="button"
                  className="disc-icon-btn"
                  ref={headerMenuTriggerRef}
                  onClick={() => setHeaderMenuOpen(open => !open)}
                  aria-label={t('disc.sidebar.moreActions')}
                  aria-expanded={headerMenuOpen}
                  aria-controls="disc-sidebar-header-actions"
                  title={t('disc.sidebar.moreActions')}
                >
                  <MoreHorizontal size={16} />
                </button>
                {headerMenuOpen && (
                  <div
                    id="disc-sidebar-header-actions"
                    className="disc-sidebar-header-menu"
                    role="group"
                    aria-label={t('disc.sidebar.moreActions')}
                  >
                    {onMarkAllRead && totalUnseenAll > 0 && (
                      <button
                        type="button"
                        aria-label={t('disc.markAllRead')}
                        title={t('disc.markAllReadTooltip', totalUnseenAll)}
                        onClick={() => {
                          onMarkAllRead();
                          setHeaderMenuOpen(false);
                        }}
                      >
                        <CheckCheck size={13} />
                        <span>{t('disc.markAllRead')}</span>
                        <strong>{totalUnseenAll}</strong>
                      </button>
                    )}
                    {(onBulkArchive || onBulkDelete) && discussions.length > 0 && (
                      <button
                        type="button"
                        onClick={() => {
                          setSelectionMode(true);
                          setHeaderMenuOpen(false);
                        }}
                      >
                        <ListChecks size={13} />
                        <span>{t('disc.bulk.start')}</span>
                      </button>
                    )}
                    {onImportDiscussion && (
                      <button
                        type="button"
                        disabled={importing}
                        onClick={() => {
                          importInputRef.current?.click();
                          setHeaderMenuOpen(false);
                        }}
                      >
                        {importing ? <Loader2 size={13} className="spin" /> : <Upload size={13} />}
                        <span>{t('disc.portability.import')}</span>
                      </button>
                    )}
                  </div>
                )}
              </div>
              {isMobile && (
                <CollectionSidebarCollapseButton
                  isMobile
                  label={t('disc.closeSidebar')}
                  onCollapse={onClose}
                />
              )}
              {!isMobile && onCollapse && (
                <CollectionSidebarCollapseButton
                  label={t('disc.closeSidebar')}
                  onCollapse={onCollapse}
                />
              )}
            </>
          )}
        </div>
      </div>
        </>,

        renderSearch: ({ value, inputRef, onChange, onSubmit, clear }) => <>
      {globalSearchOpen && onCloseGlobalSearch && onOpenGlobalSearchResult && (
        <GlobalSearchPanel
          projects={projects}
          authors={globalSearchAuthors}
          initialQuery={discSearchFilter}
          onQueryChange={setDiscSearchFilter}
          onOpenResult={onOpenGlobalSearchResult}
          onClose={() => {
            setDiscSearchFilter('');
            onCloseGlobalSearch();
          }}
          t={t}
          lang={lang}
        />
      )}

      {/* KT-70 / KT-90 — one search entry point. Enter runs the backend query
          over titles, ids and every message; Filtres opens the same result
          panel with its advanced controls. The local tree never remounts on
          each keystroke. */}
      <div className="disc-search-wrap" hidden={globalSearchOpen}>
        <div className="disc-search-controls">
          <div className="disc-search-box">
            <Search size={13} className="disc-search-icon" />
            <input
              ref={inputRef}
              type="text"
              className="disc-search-input"
              value={value}
              onChange={e => onChange(e.target.value)}
              placeholder={t('disc.globalSearch.placeholder')}
              aria-label={t('disc.globalSearch.placeholder')}
              aria-keyshortcuts="/"
              onKeyDown={event => {
                if (event.key === 'Enter' && onSubmit) {
                  event.preventDefault();
                  onSubmit();
                }
              }}
            />
            {discSearchFilter && (
              <button
                type="button"
                onClick={clear}
                className="disc-search-clear"
                aria-label={t('disc.searchClear')}
                title={t('disc.searchClear')}
              >
                <X size={10} />
              </button>
            )}
          </div>
          {onOpenGlobalSearch && (
            <button
              type="button"
              className="disc-search-filter-btn"
              onClick={onOpenGlobalSearch}
              aria-label={t('disc.globalSearch.open')}
              title={t('disc.globalSearch.open')}
              data-testid="disc-open-global-search"
              data-tour-id="global-search-open"
            >
              <SlidersHorizontal size={12} />
              <span>{t('disc.sidebar.filters')}</span>
              {sourceFilter && <strong>1</strong>}
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
            // A title alone is not an accessible name (axe `label-title-only`).
            aria-label={t('disc.source.filterTooltip')}
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
        </>,

        renderList: () => <div className="disc-sidebar-list" hidden={globalSearchOpen}>
        {followUpDiscussions.length > 0 && (() => {
          const isCollapsed = collapsedGroups.has('__follow_up__');
          return (
            <div
              className="disc-sidebar-section disc-sidebar-follow-up"
              data-expanded={!isCollapsed}
            >
              <button
                type="button"
                className="disc-group-btn"
                data-no-border="true"
                onClick={() => onToggleGroup('__follow_up__')}
                aria-expanded={!isCollapsed}
              >
                <ChevronRight size={10} className="disc-chevron" data-expanded={!isCollapsed} />
                <CircleDot size={10} />
                <span>{t('disc.followUp')}</span>
                <span className="disc-group-unseen">{followUpDiscussions.length}</span>
              </button>
              {!isCollapsed && renderSmartSectionRows(followUpDiscussions, 'follow')}
            </div>
          );
        })()}

        {recentDiscussions.length > 0 && (() => {
          const isCollapsed = collapsedGroups.has('__recent__');
          return (
            <div
              className="disc-sidebar-section disc-sidebar-recent"
              data-expanded={!isCollapsed}
            >
              <button
                type="button"
                className="disc-group-btn"
                data-no-border="true"
                onClick={() => onToggleGroup('__recent__')}
                aria-expanded={!isCollapsed}
              >
                <ChevronRight size={10} className="disc-chevron" data-expanded={!isCollapsed} />
                <Clock3 size={10} />
                <span>{t('disc.recent')}</span>
                <span className="disc-group-count">{recentDiscussions.length}</span>
              </button>
              {!isCollapsed && renderSmartSectionRows(recentDiscussions, 'recent')}
            </div>
          );
        })()}

        {/* Contacts remain immediately reachable but no longer consume the
            first screen permanently on large workspaces. The same persisted
            group-state mechanism as projects/favorites keeps the interaction
            predictable across reloads. Add/join actions are siblings of the
            toggle (never nested interactive controls). */}
        <div
          className="disc-sidebar-section disc-sidebar-contacts"
          data-expanded={!contactsCollapsed}
        >
          <div className="disc-contacts-header">
            <button
              type="button"
              className="disc-group-btn disc-contacts-toggle"
              data-no-border="true"
              onClick={() => onToggleGroup(contactsGroupKey)}
              aria-expanded={!contactsCollapsed}
            >
              <ChevronRight size={10} className="disc-chevron" data-expanded={!contactsCollapsed} />
              <Users2 size={10} />
              <span>{t('contacts.title')}</span>
              {contacts.length > 0 && (
                <span className="disc-group-count">
                  {onlineContactCount}/{contacts.length}
                </span>
              )}
            </button>
            <span className="disc-contacts-meta">
              {contacts.length > 0 && (
                <span
                  className="disc-ws-dot"
                  role="status"
                  data-connected={wsConnected}
                  title={wsConnected ? t('contacts.wsConnected') : t('contacts.wsDisconnected')}
                  aria-label={wsConnected ? t('contacts.wsConnected') : t('contacts.wsDisconnected')}
                />
              )}
              {onJoinByCode && (
                <button
                  type="button"
                  onClick={() => { setShowJoin(p => !p); setShowAddContact(false); }}
                  className="disc-contact-add-btn"
                  title={t('contacts.joinByCode')}
                  aria-label={t('contacts.joinByCode')}
                >
                  <LogIn size={12} />
                </button>
              )}
              <button
                type="button"
                onClick={() => { setShowAddContact(p => !p); setShowJoin(false); }}
                className="disc-contact-add-btn"
                title={t('contacts.add')}
                aria-label={t('contacts.add')}
              >
                <Plus size={12} />
              </button>
            </span>
          </div>
          {!contactsCollapsed && (
            <>
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
          {/* Contact list — click a row to open a 1:1 chat with that contact.
              The identity is its own <button> rather than a clickable row: the
              delete button must not sit inside an interactive ancestor (axe
              `nested-interactive`), and a real button gives keyboard activation
              that the previous `div role="button"` only pretended to offer. */}
          {contacts.map(c => {
            const identity = (
              <>
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
              </>
            );
            return (
              <div key={c.id} className="disc-contact-row">
                {onStartChat ? (
                  <button
                    type="button"
                    className="disc-contact-open"
                    title={t('contacts.startChat', c.pseudo)}
                    onClick={() => onStartChat(c)}
                  >
                    {identity}
                  </button>
                ) : (
                  <span className="disc-contact-open">{identity}</span>
                )}
                <button
                  onClick={() => onContactDelete(c.id)}
                  className="disc-contact-del-btn"
                  title={t('contacts.delete')}
                >
                  <X size={10} />
                </button>
              </div>
            );
          })}
            </>
          )}
        </div>

        {/* Pinned / Favorites — cross-project and collapsible. The optional
            source filter applies locally; the primary query renders in the
            dedicated global-results panel above. */}
        {(() => {
          const pinned = favoriteDiscussions;
          if (pinned.length === 0) return null;
          const isCollapsed = collapsedGroups.has('__favorites__');
          return (
            <div
              className="disc-sidebar-section disc-sidebar-favorites"
              data-expanded={!isCollapsed}
            >
              <button
                className="disc-group-btn"
                data-no-border="true"
                onClick={() => onToggleGroup('__favorites__')}
                aria-expanded={!isCollapsed}
              >
                <ChevronRight size={10} className="disc-chevron" data-expanded={!isCollapsed} />
                <Star size={10} style={{ color: 'var(--kr-warning)' }} />
                <span style={{ fontWeight: 600, fontSize: 'var(--kr-fs-sm)' }}>{t('disc.favorites')}</span>
                <span className="disc-group-count">{pinned.length}</span>
              </button>
              {!isCollapsed && renderSmartSectionRows(pinned.sort(byLiveThenRecent), 'pin')}
            </div>
          );
        })()}

        {/* Canonical discussion tree. Smart sections above are shortcuts only;
            Projects remains the complete, non-duplicated source of truth. */}
        {canonicalCandidates.length > 0 && (
          <div
            className="disc-sidebar-section disc-sidebar-projects"
            data-expanded={!projectsCollapsed}
          >
            <button
              type="button"
              className="disc-group-btn"
              data-no-border="true"
              onClick={() => onToggleGroup(projectsGroupKey)}
              aria-expanded={!projectsCollapsed}
            >
              <ChevronRight size={10} className="disc-chevron" data-expanded={!projectsCollapsed} />
              <Folder size={10} />
              <span>{t('projects.title')}</span>
              <span className="disc-group-count">{canonicalCandidates.length}</span>
              {canonicalUnseen > 0 && (
                <span className="disc-group-unseen">{canonicalUnseen}</span>
              )}
            </button>
            {!projectsCollapsed && (
              <div className="disc-project-tree">
        {/* Global discussions (no project) */}
        {(() => {
          // Filter up front so header/count visibility follows the optional
          // source filter.
          const globalDiscs = (activeDiscByProject.get(null) ?? [])
            .filter(matchesFilters)
            .filter(disc => !nestedExecutionChildIds.has(disc.id));
          if (globalDiscs.length === 0) return null;
          const isCollapsed = collapsedGroups.has('__global__');
          return (
            <div>
              <button
                className="disc-group-btn"
                data-no-border="true"
                onClick={() => onToggleGroup('__global__')}
                aria-expanded={!isCollapsed}
              >
                <ChevronRight size={10} className="disc-chevron" data-expanded={!isCollapsed} />
                <MessageSquare size={10} /> {t('disc.noProject')}
                <span className="disc-group-count">{globalDiscs.length}</span>
                {(unseenByGroup.get('__global__') ?? 0) > 0 && (
                  <span className="disc-group-unseen">{unseenByGroup.get('__global__')}</span>
                )}
              </button>
              {!isCollapsed && globalDiscs.sort(byLiveThenRecent).map(renderCanonicalRow)}
            </div>
          );
        })()}

        {/* Project discussions — grouped by org */}
        {(() => {
          // `.filter(matchesFilters)` is a no-op when no source filter is
          // active; otherwise it hides folders with no matching discussion.
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
            const isOrgCollapsed = collapsedGroups.has(orgKey);
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
                  const isCollapsed = collapsedGroups.has(proj.id) && !projContainsActive;
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
                          .filter(disc => !nestedExecutionChildIds.has(disc.id))
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
                            const anySending = discs.some(isRunningDisc);
                            const total = discs.length;
                            // "Done" = not running AND not queued AND has at least 2 messages
                            // (user + agent reply). Excluding queuedMap matters on a batch
                            // retry over EXISTING discs (>=2 messages already): a throttled
                            // child would otherwise count as done and the pill jumps ahead.
                            // Rough live heuristic; the real authority is workflow_runs in DB.
                            const done = discs.filter(d => !isRunningDisc(d) && !isQueuedDisc(d) && d.message_count >= 2).length;
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
                        // A Quick Prompt may produce many batch runs. Showing the QP
                        // title once per run made large projects unreadable, so runs
                        // sharing the same durable QP id live under one campaign row.
                        // Missing summaries stay isolated by run id: grouping by a
                        // fallback title would accidentally merge unrelated batches.
                        const campaignMap = new Map<string, {
                          key: string;
                          label: string;
                          icon: string;
                          runs: typeof batchGroups;
                        }>();
                        for (const batch of batchGroups) {
                          const summary = batchMetaById.get(batch.runId);
                          const qpId = summary?.quick_prompt_id;
                          const key = qpId ? `qp-batches::${qpId}` : `batch-only::${batch.runId}`;
                          const fallbackTitle = batch.discs[0].title.split('—')[0].trim();
                          const existing = campaignMap.get(key);
                          if (existing) {
                            existing.runs.push(batch);
                          } else {
                            campaignMap.set(key, {
                              key,
                              label: summary?.quick_prompt_name ?? fallbackTitle,
                              icon: summary?.quick_prompt_icon || '📦',
                              runs: [batch],
                            });
                          }
                        }
                        const batchCampaigns = Array.from(campaignMap.values()).sort((a, b) => {
                          const rank = (campaign: typeof a) => campaign.runs.some(run => run.anySending)
                            ? 0
                            : campaign.runs.some(run => run.queued > 0) ? 1 : 2;
                          return rank(a) - rank(b)
                            || b.runs[0].discs[0].updated_at.localeCompare(a.runs[0].discs[0].updated_at);
                        });
                        return (
                          <>
                            {/* Batch campaigns first. Every QP keeps the same
                                QP → run → discussion hierarchy, even with one run. */}
                            {batchCampaigns.map(campaign => {
                              const campaignCollapsed = collapsedGroups.has(campaign.key)
                                && !campaign.runs.some(run => run.discs.some(disc => disc.id === activeId))
                                && !campaign.runs.some(run => run.anySending || run.queued > 0);
                              const campaignTotal = campaign.runs.reduce((sum, run) => sum + run.total, 0);
                              const renderRun = (bg: typeof batchGroups[number]) => {
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
                              const isLive = bg.anySending || bg.queued > 0;
                              // Child discussions are expensive and noisy. A run opens
                              // only on explicit request, or automatically while active.
                              const isBatchCollapsed = !openBatchRuns.has(bg.runId)
                                && !containsActive
                                && !isLive;
                              const summaryForLabel = batchMetaById.get(bg.runId);
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
                              const shortRunId = bg.runId.replaceAll('-', '').slice(0, 8);
                              // While active, split "en cours" from "en file"
                              // so a big batch reads honestly (e.g. "⏳ 3/23 · 5▶ · 15⏸")
                              // instead of 23 identical spinners.
                              const terminalStatus = summaryForLabel?.status;
                              const summaryCompleted = summaryForLabel?.batch_completed ?? bg.done;
                              const summaryTotal = summaryForLabel?.batch_total ?? bg.total;
                              const explicitFailures = Math.max(
                                0,
                                (summaryForLabel?.batch_failed ?? 0)
                                  - (summaryForLabel?.batch_no_response ?? 0),
                              );
                              const terminalBreakdown = [
                                explicitFailures > 0 ? `${explicitFailures}✕` : null,
                                (summaryForLabel?.batch_no_response ?? 0) > 0
                                  ? `${summaryForLabel?.batch_no_response}∅`
                                  : null,
                              ].filter(Boolean).join(' · ');
                              const statusPill = (bg.anySending || bg.queued > 0)
                                ? `⏳ ${bg.done}/${bg.total}`
                                  + (bg.running > 0 ? ` · ${bg.running}▶` : '')
                                  + (bg.queued > 0 ? ` · ${bg.queued}⏸` : '')
                                : terminalStatus === 'Partial'
                                  ? `⚠ ${summaryCompleted}/${summaryTotal}`
                                    + (terminalBreakdown ? ` · ${terminalBreakdown}` : '')
                                  : terminalStatus === 'Failed'
                                    ? `✕ 0/${summaryTotal}`
                                      + (terminalBreakdown ? ` · ${terminalBreakdown}` : '')
                                : bg.done === bg.total
                                  ? `✓ ${bg.total}/${bg.total}`
                                  : `${bg.done}/${bg.total}`;
                              const statusKind = (bg.anySending || bg.queued > 0)
                                ? 'running'
                                : terminalStatus === 'Partial'
                                  ? 'partial'
                                  : terminalStatus === 'Failed'
                                    ? 'failed'
                                    : 'done';
                              const summary = batchMetaById.get(bg.runId);
                              const parentLabel = formatBatchParent(summary, t);
                              const parentWorkflowId = summary?.parent_workflow_id ?? null;
                              const hasBatchMenu = Boolean(
                                onReviewBatch
                                || onCompareBatch
                                || (onRetryBatch && summaryForLabel?.quick_prompt_id)
                                || (parentLabel && parentWorkflowId && onNavigateWorkflow)
                                || onDeleteBatch,
                              );
                              return (
                                <div key={batchKey} className="disc-batch-wrap" data-batch-key={batchKey}>
                                  <div
                                    className="disc-batch-header"
                                    data-compact="true"
                                  >
                                    <button
                                      className="disc-group-btn"
                                      data-variant="batch"
                                      onClick={() => onToggleBatchRun(bg.runId)}
                                      aria-expanded={!isBatchCollapsed}
                                      style={{ marginLeft: 0, flex: 1 }}
                                      title={batchWhenAbs}
                                    >
                                      <ChevronRight size={10} className="disc-chevron" data-expanded={!isBatchCollapsed} />
                                      <span className="disc-batch-run-label">
                                        <Clock3 size={10} aria-hidden="true" />
                                        {batchWhen || t('disc.batchRun')}
                                      </span>
                                      <span className="disc-batch-run-id" title={bg.runId}>
                                        {t('disc.batchRunId', shortRunId)}
                                      </span>
                                      <span className="disc-group-count" data-batch-status={statusKind}>
                                        {statusPill}
                                      </span>
                                    </button>
                                    {hasBatchMenu && (
                                      <div
                                        className="disc-batch-menu"
                                        ref={openBatchMenuRunId === bg.runId ? batchMenuRef : undefined}
                                      >
                                        <button
                                          type="button"
                                          className="disc-batch-menu-trigger"
                                          ref={(node) => {
                                            if (node) batchMenuTriggerRefs.current.set(bg.runId, node);
                                            else batchMenuTriggerRefs.current.delete(bg.runId);
                                          }}
                                          aria-label={t('disc.batchMoreActions')}
                                          aria-expanded={openBatchMenuRunId === bg.runId}
                                          aria-controls={`batch-actions-${bg.runId}`}
                                          onClick={(event) => {
                                            event.stopPropagation();
                                            setOpenBatchMenuRunId(current => current === bg.runId ? null : bg.runId);
                                          }}
                                        >
                                          <MoreHorizontal size={14} />
                                        </button>
                                        {openBatchMenuRunId === bg.runId && (
                                          <div
                                            id={`batch-actions-${bg.runId}`}
                                            className="disc-batch-menu-panel"
                                            role="group"
                                            aria-label={t('disc.batchMoreActions')}
                                          >
                                            {parentLabel && parentWorkflowId && onNavigateWorkflow && (
                                              <button
                                                type="button"
                                                onClick={() => {
                                                  setOpenBatchMenuRunId(null);
                                                  onNavigateWorkflow(parentWorkflowId);
                                                }}
                                              >
                                                ↗ <span>{parentLabel}</span>
                                              </button>
                                            )}
                                            {onReviewBatch && (
                                              <button
                                                type="button"
                                                onClick={() => {
                                                  setOpenBatchMenuRunId(null);
                                                  onReviewBatch(bg.runId, campaign.label, bg.discs.map(d => d.id));
                                                }}
                                              >
                                                <ListChecks size={12} /> <span>{t('disc.batchReviewAction')}</span>
                                              </button>
                                            )}
                                            {onCompareBatch && (
                                              <button
                                                type="button"
                                                onClick={() => {
                                                  setOpenBatchMenuRunId(null);
                                                  onCompareBatch(bg.runId, campaign.label, bg.discs.map(d => d.id));
                                                }}
                                              >
                                                <Columns3 size={12} /> <span>{t('disc.compare.action')}</span>
                                              </button>
                                            )}
                                            {onRetryBatch && summaryForLabel?.quick_prompt_id && (
                                              <button
                                                type="button"
                                                onClick={() => {
                                                  setOpenBatchMenuRunId(null);
                                                  if (confirm(t('disc.batchRetryConfirm', bg.total, campaign.label))) {
                                                    const qpId = summaryForLabel.quick_prompt_id;
                                                    if (qpId) onRetryBatch(bg.runId, qpId, bg.discs.map(d => d.id));
                                                  }
                                                }}
                                              >
                                                ↻ <span>{t('disc.batchRetryAction')}</span>
                                              </button>
                                            )}
                                            {onDeleteBatch && (
                                              <button
                                                type="button"
                                                data-danger="true"
                                                onClick={() => {
                                                  setOpenBatchMenuRunId(null);
                                                  if (confirm(t('disc.batchDeleteConfirm', bg.total, campaign.label))) {
                                                    onDeleteBatch(bg.runId, bg.total);
                                                  }
                                                }}
                                              >
                                                <Trash2 size={12} /> <span>{t('disc.batchDeleteAction')}</span>
                                              </button>
                                            )}
                                          </div>
                                        )}
                                      </div>
                                    )}
                                  </div>
                                  {!isBatchCollapsed && (
                                    // Wrapper with a left "tree line" + indent so the
                                    // batch children read as "inside" the 📦 folder,
                                    // not as siblings of the loose discs below.
                                    <div className="disc-batch-children">
                                      {/* Sorted copy keeps the source group stable. */}
                                      {[...bg.discs].sort(byLiveThenRecent).map(disc => (
                                        <SwipeableDiscItem
                                          key={disc.id}
                                          disc={disc}
                                          isActive={disc.id === activeId}
                                          lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                                          isSending={!!sendingMap[disc.id]}
                                          isQueued={isQueuedDisc(disc)}
                                          selectionMode={selectionMode}
                                          isSelected={selectedIds.has(disc.id)}
                                          onToggleSelection={toggleSelection}
                                          onSelect={onSelect}
                                          onArchive={onArchive}
                                          onDelete={onDelete}
                                          onStop={onStopDiscussion}
                                          onTogglePin={onTogglePin}
                                          t={t}
                                          collectionRowClassName="collection-shell-row-button"
                                          sourceAgents={sourceBindings.get(disc.id)}
                                          importedBy={importProvenance.get(disc.id) ?? null}
                                        />
                                      ))}
                                    </div>
                                  )}
                                </div>
                              );
                              };

                              return (
                                <div
                                  key={campaign.key}
                                  className="disc-batch-campaign"
                                  data-batch-campaign-key={campaign.key}
                                >
                                  <button
                                    type="button"
                                    className="disc-group-btn"
                                    data-variant="batch-campaign"
                                    onClick={() => onToggleGroup(campaign.key)}
                                    aria-expanded={!campaignCollapsed}
                                    title={campaign.label}
                                  >
                                    <ChevronRight
                                      size={10}
                                      className="disc-chevron"
                                      data-expanded={!campaignCollapsed}
                                    />
                                    <span className="disc-batch-campaign-icon" aria-hidden="true">{campaign.icon}</span>
                                    <span className="disc-batch-campaign-content">
                                      <span className="disc-batch-campaign-name" title={campaign.label}>
                                        {campaign.label}
                                      </span>
                                      <span
                                        className="disc-batch-campaign-meta"
                                        aria-label={t('disc.batchCampaignStats', campaign.runs.length, campaignTotal)}
                                      >
                                        <span
                                          aria-hidden="true"
                                          title={t('disc.batchRunsTotalTitle', campaign.runs.length)}
                                        >
                                          🔀 {campaign.runs.length}
                                        </span>
                                        <span
                                          aria-hidden="true"
                                          title={t('disc.batchDiscussionsTotalTitle', campaignTotal)}
                                        >
                                          💬 {campaignTotal}
                                        </span>
                                      </span>
                                    </span>
                                  </button>
                                  {!campaignCollapsed && (
                                    <div className="disc-batch-campaign-runs">
                                      {campaign.runs.map(renderRun)}
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
                              const showAll = isExpanded;
                              // Live-first BEFORE the cap: a running disc must
                              // never be hidden behind "afficher plus".
                              const treeLiveRank = (disc: Discussion) => {
                                const children = executionChildrenByParent.get(disc.id) ?? [];
                                if (disc.id === activeId || children.some(link => link.sub_discussion_id === activeId)) return -1;
                                if (children.some(link => {
                                  const child = discussionById.get(link.sub_discussion_id);
                                  return child ? isRunningDisc(child) : false;
                                })) return 0;
                                if (children.some(link => {
                                  const child = discussionById.get(link.sub_discussion_id);
                                  return child ? isQueuedDisc(child) : false;
                                })) return 1;
                                return isRunningDisc(disc) ? 0 : isQueuedDisc(disc) ? 1 : 2;
                              };
                              const orderedLoose = [...loose].sort((a, b) => (
                                treeLiveRank(a) - treeLiveRank(b)
                                || b.updated_at.localeCompare(a.updated_at)
                              ));
                              const visibleLoose = showAll ? orderedLoose : orderedLoose.slice(0, PROJECT_LOOSE_LIMIT);
                              const hiddenCount = orderedLoose.length - visibleLoose.length;
                              return (
                                <>
                                  {visibleLoose.map(renderCanonicalRow)}
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
              </div>
            )}
          </div>
        )}

        {discussions.length === 0 && (
          <div className="disc-empty">{t('disc.empty')}</div>
        )}

        {/* Archives section */}
        {archivedDiscussions.length > 0 && (
          <div
            className="disc-sidebar-section disc-sidebar-archives"
            data-expanded={showArchives}
          >
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
            {showArchives && (() => {
              const orderedArchives = archivedDiscussions
                .filter(matchesFilters)
                .sort((a, b) => b.updated_at.localeCompare(a.updated_at));
              const visibleArchives = orderedArchives.slice(0, archivedVisibleCount);
              const hiddenArchiveCount = orderedArchives.length - visibleArchives.length;
              return (
                <>
                  {visibleArchives.map(disc => (
                    <SwipeableDiscItem
                      key={disc.id}
                      disc={disc}
                      isActive={disc.id === activeId}
                      lastSeenCount={lastSeenMsgCount[disc.id] ?? 0}
                      isSending={!!sendingMap[disc.id]}
                      isQueued={isQueuedDisc(disc)}
                      selectionMode={selectionMode}
                      isSelected={selectedIds.has(disc.id)}
                      onToggleSelection={toggleSelection}
                      onSelect={onSelect}
                      onArchive={onUnarchive}
                      onDelete={onDelete}
                      onTogglePin={onTogglePin}
                      archiveLabel={t('disc.unarchive')}
                      t={t}
                      collectionRowClassName="collection-shell-row-button"
                      sourceAgents={sourceBindings.get(disc.id)}
                      importedBy={importProvenance.get(disc.id) ?? null}
                    />
                  ))}
                  {hiddenArchiveCount > 0 && (
                    <button
                      type="button"
                      className="disc-show-more-btn disc-archives-more"
                      onClick={() => setArchivedVisibleCount(count => count + 50)}
                    >
                      + {Math.min(50, hiddenArchiveCount)} {t('disc.showMore')}
                    </button>
                  )}
                </>
              );
            })()}
          </div>
        )}
      </div>,
        sidebarFooter: !globalSearchOpen ? <div className="disc-sidebar-footer">
          <span>{t('disc.sidebar.compact')}</span>
          <span>
            <kbd>↑↓</kbd> {t('disc.sidebar.navigate')}
            <span aria-hidden="true"> · </span>
            <kbd>/</kbd> {t('disc.sidebar.searchShortcut')}
          </span>
        </div> : null,
      }}
    />
  );
}
