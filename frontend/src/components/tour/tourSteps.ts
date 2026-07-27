// Declarative step definitions for the guided tour (v3 — 3 acts).
//
// KT-117 — v2 had 17 steps pinned to individual buttons, and it aged badly: an
// anchor renamed elsewhere silently killed a step, and a step targeting the
// first row of a list had no target at all for the newcomers it was written for.
// v3 follows three rules so the tour degrades instead of rotting:
//
// - teach the MENTAL MODEL, not the button. One step per capability, anchored on
//   a durable container; the per-page help panels carry the detail.
// - every anchor must exist for a BRAND-NEW user: no step may depend on an
//   existing project, profile, discussion or run.
// - no step is worth a freeze. A missing target is skipped by the provider, and
//   `tourAnchors.test.ts` fails the build when an anchor disappears.

import { discussions } from '../../lib/api';

export type Page =
  | 'projects'
  | 'discussions'
  | 'planning'
  | 'mcps'
  | 'workflows'
  | 'settings';

export interface TourStep {
  id: string;
  page: Page;
  selector: string | null;
  titleKey: string;
  descKey: string;
  agentNoteKey?: string;
  infoNoteKey?: string;
  /** Extra controls that deserve an explicit outline without enlarging the
   * primary spotlight to the whole area between them. */
  secondarySelectors?: string[];
  position?: 'top' | 'bottom' | 'left' | 'right';
  waitForClick?: boolean;
  groupKey?: string;
  pulse?: boolean;
  /** The target is intentionally absent in some supported layouts. Only these
   *  steps may be auto-acknowledged when missing; every other miss remains due
   *  for a later retry instead of being mistaken for content the user saw. */
  optionalWhenMissing?: boolean;
  beforeStep?: () => void | Promise<void>;
  afterStep?: () => void;
  /** When set, the tooltip card is positioned relative to THIS element's
   *  bounding rect instead of `selector`'s. Use it when the spotlight
   *  target is a small control nested inside a larger container (e.g. a
   *  button inside a form card) — the spotlight still anchors on the
   *  small target so the user sees what to click, but the tooltip sits
   *  OUTSIDE the container so it never covers the content the user is
   *  trying to interact with. */
  tooltipAnchor?: string;
}

/** Open the new-discussion form if it isn't already. The two steps that explain
 *  it — including the mention identities, the least guessable part of Kronn —
 *  live inside it, and a user who presses Next instead of clicking the button
 *  would otherwise lose them to the skip. */
function openNewDiscussionForm() {
  if (document.querySelector('.disc-new-card')) return;
  document.querySelector<HTMLElement>('[data-tour-id="new-disc-btn"]')?.click();
}

/** Close the new-discussion form so the following step isn't hidden behind it.
 *  No-op when the form isn't open. */
function closeNewDiscussionForm() {
  cancelDemoTyping();
  cancelDemoLauncher();
  const card = document.querySelector<HTMLElement>('.disc-new-card');
  card?.querySelector<HTMLElement>('button[aria-label="Close"]')?.click();
}

/** Seeded demo discussion, resolved once per tour run. */
let demoDiscussion: { id: string; prompt: string } | null = null;
let demoTypingTimer: ReturnType<typeof setTimeout> | null = null;
let demoLauncherTimer: ReturnType<typeof setTimeout> | null = null;
let demoLauncherCleanup: (() => void) | null = null;
let demoLifecycleQueue: Promise<void> = Promise.resolve();

function cancelDemoLauncher() {
  if (demoLauncherTimer !== null) {
    clearTimeout(demoLauncherTimer);
    demoLauncherTimer = null;
  }
  demoLauncherCleanup?.();
  demoLauncherCleanup = null;
}

function cancelDemoTyping() {
  if (demoTypingTimer !== null) {
    clearTimeout(demoTypingTimer);
    demoTypingTimer = null;
  }
  document.querySelector<HTMLTextAreaElement>('.disc-new-card textarea')
    ?.removeAttribute('data-tour-demo-typing');
}

function setControlledTextareaValue(field: HTMLTextAreaElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLTextAreaElement.prototype,
    'value',
  )?.set;
  setter?.call(field, value);
  field.dispatchEvent(new Event('input', { bubbles: true }));
}

/** Ask the backend for the deterministic, agentless demo discussion. The bundle is
 *  built in Rust so it can never drift from the data model — 19 of `Discussion`'s
 *  32 fields are required, and that shape changes. Costs no tokens, and is
 *  idempotent: replaying the tour reopens the same discussion. */
export function ensureDemoDiscussion(): Promise<void> {
  demoLifecycleQueue = demoLifecycleQueue.then(async () => {
    try {
      const demo = await discussions.ensureTourDemo();
      demoDiscussion = { id: demo.discussion_id, prompt: demo.prompt };
      // The sidebar list was fetched at mount, so a discussion seeded afterwards is
      // absent from it — and the step that opens it by clicking its row found
      // nothing, silently losing three steps. This is the event the page already
      // listens to for exactly this purpose.
      window.dispatchEvent(new Event('kronn:discussion-updated'));
    } catch {
      // A tour must never dead-end on a backend hiccup: the steps that need the
      // demo then find no target and the provider skips them.
      demoDiscussion = null;
    }
  });
  return demoLifecycleQueue;
}

/** Archive the local demo when the user completes the tour. A later replay calls
 *  `ensureTourDemo()` again, whose backend contract reopens the same discussion;
 *  abandoning an incomplete tour deliberately leaves it visible for resume. */
export function archiveDemoDiscussion(): Promise<void> {
  demoLifecycleQueue = demoLifecycleQueue.then(async () => {
    const id = demoDiscussion?.id;
    if (!id) return;
    try {
      await discussions.update(id, { archived: true });
      window.dispatchEvent(new Event('kronn:discussion-updated'));
    } catch {
      // Completing onboarding must not be held hostage by a transient refresh
      // failure. The same idempotent demo will be reused on the next replay.
    }
  });
  return demoLifecycleQueue;
}

/** Type the simulated request into the launcher. The form may have been opened
 *  in this same React tick, so the bounded mount wait is part of the animation
 *  rather than assuming the textarea already exists. Every character goes
 *  through the native setter so React's controlled state stays authoritative. */
function typeLauncherDemoPrompt(attempt = 0) {
  const prompt = demoDiscussion?.prompt;
  const field = document.querySelector<HTMLTextAreaElement>('.disc-new-card textarea');
  if (!prompt) return;
  if (!field) {
    if (attempt < 40) {
      demoTypingTimer = setTimeout(() => typeLauncherDemoPrompt(attempt + 1), 25);
    }
    return;
  }
  if (field.value === prompt || (field.value.trim() && !prompt.startsWith(field.value))) return;

  cancelDemoTyping();
  if (window.matchMedia?.('(prefers-reduced-motion: reduce)').matches) {
    setControlledTextareaValue(field, prompt);
    return;
  }

  field.setAttribute('data-tour-demo-typing', 'true');
  let index = field.value.length;
  const typeNextCharacter = () => {
    if (!field.isConnected || !prompt.startsWith(field.value)) {
      cancelDemoTyping();
      return;
    }
    index += 1;
    setControlledTextareaValue(field, prompt.slice(0, index));
    if (index >= prompt.length) {
      field.removeAttribute('data-tour-demo-typing');
      demoTypingTimer = null;
      return;
    }
    demoTypingTimer = setTimeout(typeNextCharacter, 28);
  };
  demoTypingTimer = setTimeout(typeNextCharacter, 80);
}

/** During the demo, both the visible launch button and Ctrl/Cmd+Enter must take
 *  the safe tour path: advance to the pre-seeded discussion, never submit the
 *  real form. Capture runs before React's handler so no discussion or agent can
 *  be created when the user chooses the natural launch action over Next. */
function armDemoLauncher(attempt = 0) {
  cancelDemoLauncher();
  const card = document.querySelector<HTMLElement>('.disc-new-card');
  if (!card) {
    if (attempt < 40) {
      demoLauncherTimer = setTimeout(() => armDemoLauncher(attempt + 1), 25);
    }
    return;
  }

  const advanceSafely = (event: Event) => {
    const isLaunchClick = event instanceof MouseEvent
      && (event.target as Element | null)?.closest('.disc-create-btn');
    const isLaunchShortcut = event instanceof KeyboardEvent
      && event.key === 'Enter'
      && (event.ctrlKey || event.metaKey)
      && !event.isComposing;
    if (!isLaunchClick && !isLaunchShortcut) return;
    event.preventDefault();
    event.stopPropagation();
    event.stopImmediatePropagation();
    window.dispatchEvent(new Event('kronn:tour-demo-launch'));
  };
  card.addEventListener('click', advanceSafely, true);
  card.addEventListener('keydown', advanceSafely, true);
  demoLauncherCleanup = () => {
    card.removeEventListener('click', advanceSafely, true);
    card.removeEventListener('keydown', advanceSafely, true);
  };
}

/** Open the seeded discussion by clicking its sidebar row — the same path a user
 *  takes, so the tour reaches into no private state. The launcher is closed first
 *  since it covers the sidebar. */
function openDemoDiscussion() {
  cancelDemoTyping();
  closeNewDiscussionForm();
  const id = demoDiscussion?.id;
  if (!id) return;
  const row = document.querySelector<HTMLElement>(`[data-tour-disc-id="${id}"]`);
  if (!row) return;
  // The row only wires `onClick` in selection mode; in normal mode it is opened
  // by its keyboard handler. Calling `.click()` did nothing, which silently cost
  // the three steps that follow. Use the accessible activation path instead.
  row.focus();
  row.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
}

function openAutomationTab(tourId: string) {
  document.querySelector<HTMLButtonElement>(`[data-tour-id="${tourId}"]`)?.click();
}

function revealCopyableIds() {
  // The discussion ID lives in the sticky header, while message IDs scroll with
  // the transcript. Bring the latest visible message header into view so the
  // tour can point at both concrete IDs at once.
  const messageIds = document.querySelectorAll<HTMLElement>(
    '[data-tour-id="message-id-pill"]',
  );
  document.querySelectorAll<HTMLElement>('[data-tour-current-message-id]')
    .forEach(element => element.removeAttribute('data-tour-current-message-id'));
  const latestMessageId = messageIds.item(messageIds.length - 1);
  latestMessageId?.setAttribute('data-tour-current-message-id', 'true');
  latestMessageId?.scrollIntoView({
    behavior: 'auto',
    block: 'center',
  });
}

function clearCopyableIdTarget() {
  document.querySelectorAll<HTMLElement>('[data-tour-current-message-id]')
    .forEach(element => element.removeAttribute('data-tour-current-message-id'));
}

async function waitForTourElement<T extends HTMLElement>(
  selector: string,
  timeoutMs = 2000,
): Promise<T | null> {
  const existing = document.querySelector<T>(selector);
  if (existing) return existing;

  return new Promise(resolve => {
    const observer = new MutationObserver(() => {
      const element = document.querySelector<T>(selector);
      if (!element) return;
      clearTimeout(timeout);
      observer.disconnect();
      resolve(element);
    });
    const timeout = setTimeout(() => {
      observer.disconnect();
      resolve(null);
    }, timeoutMs);
    observer.observe(document.body, { childList: true, subtree: true });
  });
}

function setControlledInputValue(field: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLInputElement.prototype,
    'value',
  )?.set;
  setter?.call(field, value);
  field.dispatchEvent(new Event('input', { bubbles: true }));
}

/** Demonstrate the real global message search against the deterministic demo.
 *  The exact launcher prompt is unique and comes from the backend, so this
 *  never teaches with a fake result or depends on the user's existing data. */
async function searchForDemoMessage() {
  if (!demoDiscussion?.prompt) return;

  // Selecting the demo closes the mobile sidebar. Reopen it before looking for
  // the shared quick/advanced search control.
  if (!document.querySelector('[data-tour-id="global-search-open"]')) {
    document.querySelector<HTMLButtonElement>('.disc-mobile-sidebar-btn')?.click();
  }
  const openButton = await waitForTourElement<HTMLButtonElement>(
    '[data-tour-id="global-search-open"]',
  );
  openButton?.click();

  const input = await waitForTourElement<HTMLInputElement>(
    '[data-testid="global-search-input"]',
  );
  if (!input) return;
  setControlledInputValue(input, demoDiscussion.prompt);

  // Let React commit the controlled value before submitting the real form.
  await new Promise(resolve => setTimeout(resolve, 0));
  input.closest('form')?.requestSubmit();

  const result = await waitForTourElement<HTMLElement>(
    `[data-tour-id="global-search-result"][data-disc-id="${demoDiscussion.id}"]`,
  );
  result?.setAttribute('data-tour-demo-result', 'true');
  result?.scrollIntoView({ behavior: 'auto', block: 'center' });
}

function closeAdvancedSearch() {
  document.querySelector<HTMLButtonElement>('.disc-global-search-close')?.click();
}

export const TOUR_STEPS: TourStep[] = [
  // ── Bienvenue ──────────────────────────────────────────────────────
  {
    id: 'welcome',
    page: 'projects',
    selector: null,
    titleKey: 'tour.welcome.title',
    descKey: 'tour.welcome.desc',
    groupKey: 'tour.group.welcome',
  },

  // ── Acte 1 : ton espace de travail ─────────────────────────────────
  {
    id: 'navigation',
    page: 'projects',
    // The nav bar, not one tab: the map matters more than any single stop. Anchored
    // on `.dash-nav` and NOT on `.dash-nav-tabs`, which is `display: contents` on
    // desktop — it generates no box, so it measured 0×0 and the highlight showed
    // up as a 4 px square in the corner.
    selector: '.dash-nav',
    titleKey: 'tour.navigation.title',
    descKey: 'tour.navigation.desc',
    position: 'bottom',
    groupKey: 'tour.group.projects',
  },
  {
    id: 'concept-project',
    page: 'projects',
    selector: '.dash-main',
    titleKey: 'tour.conceptProject.title',
    descKey: 'tour.conceptProject.desc',
    agentNoteKey: 'tour.conceptProject.agentNote',
    position: 'top',
    groupKey: 'tour.group.projects',
  },
  {
    id: 'new-project',
    page: 'projects',
    selector: '[data-tour-id="new-project-btn"]',
    titleKey: 'tour.newProject.title',
    descKey: 'tour.newProject.desc',
    position: 'bottom',
    groupKey: 'tour.group.projects',
  },

  // ── Acte 2 : faire travailler les agents ───────────────────────────
  {
    id: 'agents-config',
    page: 'settings',
    // Enabling an agent comes before talking to one, so this opens the act even
    // though it means one hop into the settings page.
    selector: '[data-tour-id="settings-agents"]',
    titleKey: 'tour.agentsConfig.title',
    descKey: 'tour.agentsConfig.desc',
    position: 'top',
    groupKey: 'tour.group.discussions',
  },
  {
    id: 'new-disc',
    page: 'discussions',
    selector: '[data-tour-id="new-disc-btn"]',
    titleKey: 'tour.newDisc.title',
    descKey: 'tour.newDisc.desc',
    position: 'right',
    // The two following steps live INSIDE the form this button opens, so the
    // click has to actually happen: the backdrop would otherwise swallow it, the
    // form would never mount, and both steps would be skipped for lack of target.
    waitForClick: true,
    pulse: true,
    groupKey: 'tour.group.discussions',
  },
  {
    id: 'disc-form',
    page: 'discussions',
    // Opened here too, not only by the user's click on the previous step: Next
    // must not cost them the explanation. The simulated request is typed in so
    // what the user reads matches the conversation the next step opens.
    beforeStep: async () => {
      // Create/reopen the local demo only when the user reaches the discussion
      // act. Waiting here prevents a fast Next/Launch from racing an unfinished
      // seed, and avoids polluting the sidebar for someone who leaves on welcome.
      await ensureDemoDiscussion();
      openNewDiscussionForm();
      typeLauncherDemoPrompt();
      armDemoLauncher();
    },
    selector: '.disc-new-card',
    // Closes on the way out, now that the next step opens the conversation instead
    // of reopening this modal. Without it the popup stayed on screen one step too
    // long and hid the very conversation the tour was pointing at.
    afterStep: closeNewDiscussionForm,
    // Historical note, kept as a warning: Closing on the way out of this step made the
    // next one reopen the form, and its guard reads the DOM before React has
    // flushed the close — so it saw the card still there, skipped the reopen, and
    // the mentions step was lost. Measured: 13 steps walked instead of 14.
    titleKey: 'tour.discForm.title',
    descKey: 'tour.discForm.desc',
    agentNoteKey: 'tour.discForm.agentNote',
    position: 'left',
    groupKey: 'tour.group.discussions',
  },
  {
    id: 'demo-render',
    page: 'discussions',
    // Lands on the seeded conversation instead of really submitting: the demo
    // already exists, so nothing is sent and no agent runs.
    beforeStep: openDemoDiscussion,
    selector: '.doc-preview',
    titleKey: 'tour.demoRender.title',
    descKey: 'tour.demoRender.desc',
    infoNoteKey: 'tour.demoRender.info',
    position: 'left',
    groupKey: 'tour.group.discussions',
  },
  {
    id: 'demo-exports',
    page: 'discussions',
    selector: '.doc-preview-actions',
    titleKey: 'tour.demoExports.title',
    descKey: 'tour.demoExports.desc',
    position: 'top',
    groupKey: 'tour.group.discussions',
  },
  {
    id: 'mentions',
    page: 'discussions',
    // Anchored on the composer, where mentions are actually typed. It used to sit
    // on the launcher card, which forced that modal to stay open one step longer
    // and read as a bug: the user pressed Next and the popup did not go away.
    selector: '.disc-composer-wrap',
    titleKey: 'tour.mentions.title',
    descKey: 'tour.mentions.desc',
    position: 'top',
    groupKey: 'tour.group.discussions',
  },
  {
    id: 'disc-header',
    page: 'discussions',
    selector: '[data-tour-id="disc-header-controls"]',
    titleKey: 'tour.discHeader.title',
    descKey: 'tour.discHeader.desc',
    agentNoteKey: 'tour.discHeader.agentNote',
    position: 'bottom',
    groupKey: 'tour.group.discussions',
  },
  {
    id: 'copyable-ids',
    page: 'discussions',
    beforeStep: revealCopyableIds,
    afterStep: clearCopyableIdTarget,
    selector: '[data-tour-id="discussion-id-pill"]',
    secondarySelectors: ['[data-tour-current-message-id="true"]'],
    titleKey: 'tour.copyableIds.title',
    descKey: 'tour.copyableIds.desc',
    agentNoteKey: 'tour.copyableIds.agentNote',
    position: 'bottom',
    groupKey: 'tour.group.discussions',
  },
  {
    id: 'global-search',
    page: 'discussions',
    beforeStep: searchForDemoMessage,
    afterStep: closeAdvancedSearch,
    selector: '[data-tour-demo-result="true"]',
    titleKey: 'tour.globalSearch.title',
    descKey: 'tour.globalSearch.desc',
    position: 'right',
    groupKey: 'tour.group.discussions',
  },
  {
    id: 'disc-sidebar',
    page: 'discussions',
    // The form covers the sidebar, so put it away before spotlighting it.
    beforeStep: closeNewDiscussionForm,
    selector: '.disc-sidebar',
    titleKey: 'tour.sidebar.title',
    descKey: 'tour.sidebar.desc',
    position: 'right',
    optionalWhenMissing: true,
    groupKey: 'tour.group.discussions',
  },

  // ── Acte 3 : industrialiser et organiser ───────────────────────────
  {
    id: 'add-plugin',
    page: 'mcps',
    selector: '[data-tour-id="add-plugin-btn"]',
    titleKey: 'tour.addPlugin.title',
    descKey: 'tour.addPlugin.desc',
    agentNoteKey: 'tour.addPlugin.agentNote',
    position: 'bottom',
    groupKey: 'tour.group.plugins',
  },
  {
    id: 'nav-planning',
    page: 'planning',
    selector: '[data-tour-id="nav-planning"]',
    titleKey: 'tour.navPlanning.title',
    descKey: 'tour.navPlanning.desc',
    agentNoteKey: 'tour.navPlanning.agentNote',
    position: 'bottom',
    groupKey: 'tour.group.automation',
  },
  {
    id: 'context-help',
    page: 'planning',
    // In-page help takes over once the tour has taught the mental model.
    selector: '.kr-context-help',
    titleKey: 'tour.contextHelp.title',
    descKey: 'tour.contextHelp.desc',
    position: 'bottom',
    groupKey: 'tour.group.automation',
  },
  {
    id: 'nav-automation',
    page: 'workflows',
    selector: '[data-tour-id="nav-workflows"]',
    titleKey: 'tour.navAutomation.title',
    descKey: 'tour.navAutomation.desc',
    position: 'bottom',
    groupKey: 'tour.group.automation',
  },
  {
    id: 'automation-quick-prompt',
    page: 'workflows',
    beforeStep: () => openAutomationTab('automation-kind-quick-prompt'),
    selector: '[data-tour-id="automation-kind-quick-prompt"]',
    titleKey: 'tour.automationQuickPrompt.title',
    descKey: 'tour.automationQuickPrompt.desc',
    position: 'bottom',
    groupKey: 'tour.group.automation',
  },
  {
    id: 'automation-quick-api',
    page: 'workflows',
    beforeStep: () => openAutomationTab('automation-kind-quick-api'),
    selector: '[data-tour-id="automation-kind-quick-api"]',
    titleKey: 'tour.automationQuickApi.title',
    descKey: 'tour.automationQuickApi.desc',
    position: 'bottom',
    groupKey: 'tour.group.automation',
  },
  {
    id: 'automation-workflow',
    page: 'workflows',
    beforeStep: () => openAutomationTab('automation-kind-workflow'),
    selector: '[data-tour-id="automation-kind-workflow"]',
    titleKey: 'tour.automationWorkflow.title',
    descKey: 'tour.automationWorkflow.desc',
    position: 'bottom',
    groupKey: 'tour.group.automation',
  },
  {
    id: 'automation-actions',
    page: 'workflows',
    selector: '[data-tour-id="automation-actions"]',
    titleKey: 'tour.automationActions.title',
    descKey: 'tour.automationActions.desc',
    position: 'bottom',
    groupKey: 'tour.group.automation',
  },
  {
    id: 'automation-ai',
    page: 'workflows',
    selector: '[data-tour-id="automation-ai-btn"]',
    titleKey: 'tour.automationAi.title',
    descKey: 'tour.automationAi.desc',
    agentNoteKey: 'tour.automationAi.agentNote',
    position: 'bottom',
    pulse: true,
    groupKey: 'tour.group.automation',
  },
  // ── Fin ────────────────────────────────────────────────────────────
  {
    id: 'done',
    page: 'discussions',
    selector: null,
    titleKey: 'tour.done.title',
    descKey: 'tour.done.desc',
    agentNoteKey: 'tour.done.agentNote',
    groupKey: 'tour.group.end',
  },
];
