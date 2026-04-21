// Declarative step definitions for the guided tour (v2 — 17 steps).
//
// Flow designed by consensus of 3 experts (Marie PM, Alex UX, Sam Learning):
// - 4 interactive steps (waitForClick) for learn-by-doing
// - 5 acts with group labels for chunking
// - Ends on Discussions page (action-oriented, not passive Settings)
// - Max ~2.5 minutes

export type Page = 'projects' | 'discussions' | 'mcps' | 'workflows' | 'settings';

export interface TourStep {
  id: string;
  page: Page;
  selector: string | null;
  titleKey: string;
  descKey: string;
  position?: 'top' | 'bottom' | 'left' | 'right';
  waitForClick?: boolean;
  group?: string;
  pulse?: boolean;
  beforeStep?: () => void;
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

function closeModal() {
  const close = document.querySelector<HTMLElement>('.dash-modal-close');
  if (close) close.click();
}

/** Expand the Profiles accordion inside the new-discussion form so the
 *  tour's next step can anchor on a real (visible) chip. No-op if the
 *  accordion is already open or the form isn't mounted. Called as a
 *  `beforeStep` on the step that highlights the accordion contents. */
function openProfilesAccordion() {
  // Only click if the chevron is still in "collapsed" state — double-
  // triggering would re-collapse the accordion and hide the chips we
  // want to highlight.
  const toggle = document.querySelector<HTMLElement>(
    '[data-tour-id="disc-form-profiles-toggle"]',
  );
  if (!toggle) return;
  const chevron = toggle.querySelector<HTMLElement>('.disc-chevron');
  const alreadyOpen = chevron?.dataset.expanded === 'true';
  if (!alreadyOpen) toggle.click();
}

export const TOUR_STEPS: TourStep[] = [
  // ── Acte 0 : Bienvenue ────────────────────────────────────────────
  {
    id: 'welcome',
    page: 'projects',
    selector: null,
    titleKey: 'tour.welcome.title',
    descKey: 'tour.welcome.desc',
    group: 'Bienvenue',
  },

  // ── Acte 1 : Projets ──────────────────────────────────────────────
  {
    id: 'concept-project',
    page: 'projects',
    selector: '.dash-main',
    titleKey: 'tour.conceptProject.title',
    descKey: 'tour.conceptProject.desc',
    position: 'bottom',
    group: 'Projets',
  },
  {
    id: 'scan-btn',
    page: 'projects',
    selector: '[data-tour-id="scan-btn"]',
    titleKey: 'tour.scan.title',
    descKey: 'tour.scan.desc',
    position: 'bottom',
    group: 'Projets',
  },
  {
    id: 'click-new-project',
    page: 'projects',
    selector: '[data-tour-id="new-project-btn"]',
    titleKey: 'tour.newProject.title',
    descKey: 'tour.newProject.desc',
    position: 'bottom',
    waitForClick: true,
    pulse: true,
    group: 'Projets',
  },
  {
    id: 'modal-overview',
    page: 'projects',
    selector: '.dash-modal',
    titleKey: 'tour.modalOverview.title',
    descKey: 'tour.modalOverview.desc',
    position: 'left',
    group: 'Projets',
    afterStep: closeModal,
  },

  // ── Acte 2 : Plugins ──────────────────────────────────────────────
  {
    id: 'nav-plugins',
    page: 'mcps',
    selector: '[data-tour-id="nav-mcps"]',
    titleKey: 'tour.navPlugins.title',
    descKey: 'tour.navPlugins.desc',
    position: 'bottom',
    group: 'Plugins',
  },
  {
    id: 'add-plugin-btn',
    page: 'mcps',
    selector: '[data-tour-id="add-plugin-btn"]',
    titleKey: 'tour.addPlugin.title',
    descKey: 'tour.addPlugin.desc',
    position: 'bottom',
    group: 'Plugins',
  },

  // ── Acte 3 : Discussions ──────────────────────────────────────────
  {
    id: 'nav-discussions',
    page: 'discussions',
    selector: '[data-tour-id="nav-discussions"]',
    titleKey: 'tour.navDiscussions.title',
    descKey: 'tour.navDiscussions.desc',
    position: 'bottom',
    group: 'Discussions',
  },
  {
    id: 'disc-sidebar',
    page: 'discussions',
    selector: '.disc-sidebar',
    titleKey: 'tour.sidebar.title',
    descKey: 'tour.sidebar.desc',
    position: 'right',
    group: 'Discussions',
  },
  {
    id: 'click-new-disc',
    page: 'discussions',
    selector: '[data-tour-id="new-disc-btn"]',
    titleKey: 'tour.newDisc.title',
    descKey: 'tour.newDisc.desc',
    position: 'bottom',
    waitForClick: true,
    pulse: true,
    group: 'Discussions',
  },
  {
    id: 'disc-form-overview',
    page: 'discussions',
    selector: '.disc-new-card',
    titleKey: 'tour.discForm.title',
    descKey: 'tour.discForm.desc',
    position: 'left',
    group: 'Discussions',
  },
  // ── Profiles — learn-by-doing, stays in the new-discussion form ──
  // Previous attempt drew a static tooltip that sat ON TOP of the form
  // so the user couldn't see the actual target. New interactive flow:
  //   1. Highlight the accordion toggle + waitForClick → user opens it
  //      themselves (so they feel the affordance + the form is
  //      visible below the tooltip).
  //   2. Highlight the first profile chip + waitForClick → user picks
  //      one to proceed (concrete action instead of "read this blurb").
  // waitForClick keeps the tour paused on the spotlight until the user
  // performs the gesture; a pulse animation makes the target obvious.
  {
    id: 'disc-form-profiles',
    page: 'discussions',
    selector: '[data-tour-id="disc-form-profiles-toggle"]',
    // Spotlight pins on the tiny toggle; tooltip positions against
    // the whole form card so it never overlaps form content.
    tooltipAnchor: '.disc-new-card',
    titleKey: 'tour.discProfiles.title',
    descKey: 'tour.discProfiles.desc',
    position: 'left',
    waitForClick: true,
    pulse: true,
    group: 'Discussions',
  },
  {
    id: 'disc-form-profile-chip',
    page: 'discussions',
    selector: '[data-tour-id="disc-form-profile-chip"]',
    tooltipAnchor: '.disc-new-card',
    titleKey: 'tour.discProfileChip.title',
    descKey: 'tour.discProfileChip.desc',
    position: 'left',
    waitForClick: true,
    pulse: true,
    group: 'Discussions',
    // Safety net: if the user navigated backward and closed the
    // accordion between step 12 and step 13, re-open it so the chip
    // is visible (and the spotlight can anchor on a real, visible
    // element rather than a hidden one with 0×0 bounding box).
    beforeStep: openProfilesAccordion,
  },

  // ── Acte 4 : Automatisation ───────────────────────────────────────
  {
    id: 'nav-workflows',
    page: 'workflows',
    selector: '[data-tour-id="nav-workflows"]',
    titleKey: 'tour.navAutomation.title',
    descKey: 'tour.navAutomation.desc',
    position: 'bottom',
    group: 'Automatisation',
  },

  // ── Acte 5 : Config ───────────────────────────────────────────────
  {
    id: 'nav-settings',
    page: 'settings',
    selector: '[data-tour-id="nav-settings"]',
    titleKey: 'tour.navConfig.title',
    descKey: 'tour.navConfig.desc',
    position: 'bottom',
    group: 'Config',
  },
  {
    id: 'usage-section',
    page: 'settings',
    selector: '[data-tour-id="usage-header"]',
    titleKey: 'tour.usage.title',
    descKey: 'tour.usage.desc',
    position: 'bottom',
    group: 'Config',
  },

  // ── Fin ────────────────────────────────────────────────────────────
  {
    id: 'done',
    page: 'discussions',
    selector: null,
    titleKey: 'tour.done.title',
    descKey: 'tour.done.desc',
    group: 'Fin',
  },
];
