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
}

function closeModal() {
  const close = document.querySelector<HTMLElement>('.dash-modal-close');
  if (close) close.click();
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
