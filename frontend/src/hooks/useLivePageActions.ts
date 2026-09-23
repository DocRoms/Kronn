import { useCallback, useEffect, useRef, useState } from 'react';
import { pages as pagesApi } from '../lib/api';
import type { LivePageAction, LivePageDetail } from '../types/generated';
import {
  hostTheme,
  liveActionBindingKey,
  postLivePageActionSlot,
  postLivePageActionStates,
  postLivePageTheme,
  type LivePageActionAnchor,
  type LivePageActionIntent,
} from '../lib/live-page-sandbox';

export interface LivePageActiveActionState {
  activation: number;
  actionRef: string;
  bindings: Record<string, string>;
  anchor: LivePageActionAnchor;
  /** What this click turned into once launched or declined — its own launch,
   * never written back over the offer the other buttons still draw from. */
  card?: LivePageAction;
}

export interface UseLivePageActionsResult {
  actions: LivePageAction[];
  /** The latest launch of every row that has run, for the buttons' state. */
  launches: LivePageAction[];
  activeAction: LivePageActiveActionState | null;
  selectedAction: LivePageAction | null;
  /** The offer behind the open card, even once that card shows a launch. */
  selectedOffer: LivePageAction | null;
  handleIntent: (intent: LivePageActionIntent) => void;
  /** Keeps the open card glued to its row while the Page scrolls. */
  moveAnchor: (anchor: LivePageActiveActionState['anchor']) => void;
  handleChanged: (action: LivePageAction, activation: number) => void;
  close: () => void;
  reload: (pageId: string | null) => Promise<void>;
}

const IN_FLIGHT = new Set<LivePageAction['state']>(['launching', 'running']);
// A decline is not a run: it never marks a button nor stands for its row.
const NOT_A_RUN = new Set<LivePageAction['state']>(['proposed', 'cancelled']);
const LAUNCH_REFRESH_MS = 3_000;

function launchKey(actionRef: string, bindingKey: string): string {
  return `${actionRef}\n${bindingKey}`;
}

function sameLaunches(left: LivePageAction[], right: LivePageAction[]): boolean {
  return left.length === right.length && left.every((launch, index) =>
    launch.id === right[index].id && launch.state === right[index].state
      && launch.updated_at === right[index].updated_at);
}

/**
 * Owns the load -> validate -> activate -> mutate lifecycle for a Page's
 * inline Kronn actions, shared by the embedded viewer, the standalone tab
 * and every mosaic tile so a `kronn:page-action` intent resolves identically
 * everywhere. Validation reads a ref mirror, not the `actions` state, because
 * the sandbox bridge relay is connected once per iframe load and would
 * otherwise close over a stale snapshot.
 *
 * `actions` are the Page's offers and are never mutated by a click: one block
 * draws a button per dataset row, so the result of one row's launch belongs to
 * that click's card alone. `launches` holds the latest run of each row, which
 * is what a button shows and what clicking it reopens.
 */
export function useLivePageActions(onUnavailable: () => void): UseLivePageActionsResult {
  const [actions, setActions] = useState<LivePageAction[]>([]);
  const actionsRef = useRef<LivePageAction[]>([]);
  const [launches, setLaunchesState] = useState<LivePageAction[]>([]);
  const launchesRef = useRef<LivePageAction[]>([]);
  const [activeAction, setActiveActionState] = useState<LivePageActiveActionState | null>(null);
  const activeActionRef = useRef<LivePageActiveActionState | null>(null);
  const activationRef = useRef(0);
  const pageIdRef = useRef<string | null>(null);
  const onUnavailableRef = useRef(onUnavailable);
  useEffect(() => { onUnavailableRef.current = onUnavailable; });

  const setActiveAction = useCallback((next: LivePageActiveActionState | null) => {
    activeActionRef.current = next;
    setActiveActionState(next);
  }, []);

  const setLaunches = useCallback((next: LivePageAction[]) => {
    if (sameLaunches(launchesRef.current, next)) return;
    launchesRef.current = next;
    setLaunchesState(next);
  }, []);

  const reload = useCallback(async (pageId: string | null) => {
    pageIdRef.current = pageId;
    const [nextActions, nextLaunches] = pageId
      ? await Promise.all([
        pagesApi.actions(pageId),
        // The buttons' state is a comfort: a failure to read it must never
        // cost the Page its actions.
        Promise.resolve().then(() => pagesApi.actionLaunches(pageId)).catch(() => [] as LivePageAction[]),
      ])
      : [[], []];
    setActions(nextActions);
    actionsRef.current = nextActions;
    setLaunches(nextLaunches);
    setActiveAction(null);
  }, [setActiveAction, setLaunches]);

  const moveAnchor = useCallback((anchor: LivePageActiveActionState['anchor']) => {
    const current = activeActionRef.current;
    if (!current) return;
    const previous = current.anchor;
    if (previous.left === anchor.left && previous.top === anchor.top
      && previous.width === anchor.width && previous.height === anchor.height
      && previous.slot === anchor.slot) return;
    setActiveAction({ ...current, anchor });
  }, [setActiveAction]);

  const handleIntent = useCallback((intent: LivePageActionIntent) => {
    const exists = actionsRef.current.some(action => action.action_ref === intent.actionRef);
    if (!exists) {
      onUnavailableRef.current();
      return;
    }
    const bindingKey = liveActionBindingKey(intent.bindings);
    const current = activeActionRef.current;
    // A second click on the button whose card is open closes it.
    if (current && current.actionRef === intent.actionRef
      && liveActionBindingKey(current.bindings) === bindingKey) {
      setActiveAction(null);
      return;
    }
    // A row that has run reopens on its latest run — what happened, or where
    // it stands — rather than on a blank offer; the card offers to launch it
    // again from there.
    const latest = launchesRef.current.find(launch =>
      launch.action_ref === intent.actionRef && launch.binding_key === bindingKey);
    activationRef.current += 1;
    setActiveAction({ ...intent, activation: activationRef.current, card: latest });
  }, [setActiveAction]);

  // Keyed on the activation, not the block: a launch that answers after the
  // user has already clicked another row of the same block must not land on
  // that row's card.
  const handleChanged = useCallback((changed: LivePageAction, activation: number) => {
    const current = activeActionRef.current;
    if (current && current.activation === activation) {
      setActiveAction({ ...current, card: changed });
    }
    if (changed.binding_key == null || NOT_A_RUN.has(changed.state)) return;
    const key = launchKey(changed.action_ref, changed.binding_key);
    const others = launchesRef.current.filter(launch =>
      launchKey(launch.action_ref, launch.binding_key ?? '') !== key);
    // Plain code-unit order, like the server's, so a poll that brings nothing
    // new compares equal to what is already shown.
    setLaunches([...others, changed].sort((left, right) => {
      const a = launchKey(left.action_ref, left.binding_key ?? '');
      const b = launchKey(right.action_ref, right.binding_key ?? '');
      return a < b ? -1 : a > b ? 1 : 0;
    }));
  }, [setActiveAction, setLaunches]);

  const close = useCallback(() => setActiveAction(null), [setActiveAction]);

  // While any row runs, keep the buttons honest even with no card open.
  const anyInFlight = launches.some(launch => IN_FLIGHT.has(launch.state));
  useEffect(() => {
    if (!anyInFlight) return undefined;
    const timer = window.setInterval(() => {
      const pageId = pageIdRef.current;
      if (!pageId) return;
      void pagesApi.actionLaunches(pageId)
        .then(next => { if (pageIdRef.current === pageId) setLaunches(next); })
        .catch(() => { /* the next tick retries; the buttons keep their last state */ });
    }, LAUNCH_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [anyInFlight, setLaunches]);

  const selectedOffer = activeAction
    ? actions.find(action => action.action_ref === activeAction.actionRef) ?? null
    : null;
  const selectedAction = activeAction ? activeAction.card ?? selectedOffer : null;

  return {
    actions, launches, activeAction, selectedAction, selectedOffer,
    handleIntent, handleChanged, close, reload, moveAnchor,
  };
}

/**
 * Mirror the host theme into the opaque iframe by message. Rebuilding the
 * document would reload it and discard Page state.
 */
export function useLivePageTheme(
  iframeRef: { readonly current: HTMLIFrameElement | null },
  channelId: string,
): void {
  useEffect(() => {
    const push = () => {
      const target = iframeRef.current?.contentWindow ?? null;
      const theme = hostTheme();
      if (target && theme) postLivePageTheme(target, channelId, theme);
    };
    push();
    const observer = new MutationObserver(push);
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    return () => observer.disconnect();
  }, [channelId, iframeRef]);
}

/**
 * Reserve the measured card height in the Page DOM. The privileged action card
 * remains in the host while the slot keeps subsequent rows below it.
 */
export function useLivePageActionSlot(
  iframeRef: { readonly current: HTMLIFrameElement | null },
  channelId: string,
  active: LivePageActiveActionState | null,
  cardHeight: number | null,
): void {
  const actionRef = active?.actionRef ?? null;
  const bindingKey = active ? liveActionBindingKey(active.bindings) : null;
  useEffect(() => {
    const target = iframeRef.current?.contentWindow ?? null;
    if (!target) return undefined;
    if (actionRef === null || bindingKey === null || !cardHeight) {
      postLivePageActionSlot(target, channelId, null);
      return undefined;
    }
    postLivePageActionSlot(target, channelId, { actionRef, bindingKey, height: cardHeight });
    // Closing the card must close the collapse, including on unmount. The
    // frame captured above is the one that was told to open it; a frame that
    // has since reloaded carries no slot to close anyway.
    return () => postLivePageActionSlot(target, channelId, null);
  }, [actionRef, bindingKey, cardHeight, channelId, iframeRef]);
}

/**
 * Skip automatic publication when runtimeData is unchanged to preserve Page
 * state. publishPageData stays unconditional for onLoad to seed fresh frames.
 * The revision changes on every publication, including dataset replacements.
 */
export function usePublishPageDataWhenChanged(
  detail: LivePageDetail | null,
  publishPageData: () => void,
): void {
  const publishedRef = useRef<string | null>(null);
  const signature = detail
    ? [detail.id, detail.slug, detail.title, detail.data_revision, detail.datasets.length].join('\u0000')
    : null;
  useEffect(() => {
    if (signature === null || publishedRef.current === signature) return;
    publishedRef.current = signature;
    publishPageData();
  }, [publishPageData, signature]);
}

/** Publish row action states on change and reload; returns the iframe onLoad callback. */
export function useActionStatesInFrame(
  iframeRef: { readonly current: HTMLIFrameElement | null },
  channelId: string,
  launches: LivePageAction[],
  publishPageData: () => void,
): () => void {
  const publishStates = useCallback(() => {
    const target = iframeRef.current?.contentWindow ?? null;
    if (target) postLivePageActionStates(target, channelId, launches);
  }, [channelId, iframeRef, launches]);
  useEffect(() => { publishStates(); }, [publishStates]);
  return useCallback(() => {
    publishPageData();
    publishStates();
  }, [publishPageData, publishStates]);
}
