// KT-1880 — the components of components/ui/ that Settings > Design system
// shows, in page order. Each gets its own heading, an entry in the section
// index and a caption in the four locales; the gallery test fails if one of
// components/ui/ is missing here.

export interface GalleryComponent {
  /** Anchor of its heading: `ds-c-<id>`. */
  id: string;
  /** The component's name in the code, shown as its heading. */
  name: string;
  /** i18n key of its one-line caption. */
  caption: string;
}

const component = (id: string, name: string): GalleryComponent =>
  ({ id, name, caption: `designSystem.caption.${id}` });

/** The base controls: exactly the components of components/ui/. */
export const UI_GALLERY_COMPONENTS: readonly GalleryComponent[] = [
  component('button', 'Button'),
  component('iconButton', 'IconButton'),
  component('menu', 'Menu'),
  component('statusBadge', 'StatusBadge'),
  component('tabs', 'Tabs'),
  component('searchField', 'SearchField'),
  component('tooltip', 'Tooltip'),
  component('dialog', 'Dialog'),
  component('drawer', 'Drawer'),
  component('confirmDialog', 'ConfirmDialog'),
  component('field', 'Field'),
  component('select', 'Select'),
  component('switch', 'Switch'),
  component('formFooter', 'FormFooter'),
  component('notice', 'Notice'),
  component('emptyState', 'EmptyState'),
  component('loadingState', 'LoadingState'),
  component('listHeader', 'ListHeader'),
  component('detailHeader', 'DetailHeader'),
  component('tabHeader', 'TabHeader'),
  component('sectionHeader', 'SectionHeader'),
  component('drawerHeader', 'DrawerHeader'),
  component('cardHeader', 'CardHeader'),
  // KT-1886 — the dense-data components, each in a wide and a narrow container.
  component('copyButton', 'CopyButton'),
  component('table', 'Table'),
  component('keyValueList', 'KeyValueList'),
  component('statTile', 'StatTile'),
  component('progressBar', 'ProgressBar'),
  // KT-2004 — the log and diff viewers, each in a wide and a narrow container.
  component('logViewer', 'LogViewer'),
  component('diffViewer', 'DiffViewer'),
];

/** KT-1885 — shared components built on those controls, shown after them:
 *  the one agent picker and its modes. */
export const SHARED_GALLERY_COMPONENTS: readonly GalleryComponent[] = [
  component('agentSwitchPicker', 'AgentSwitchPicker'),
];

export const GALLERY_COMPONENTS: readonly GalleryComponent[] = [...UI_GALLERY_COMPONENTS, ...SHARED_GALLERY_COMPONENTS];

/** Anchor of the tokens part of the page. */
export const TOKENS_ANCHOR = 'ds-tokens';

export const componentAnchor = (id: string) => `ds-c-${id}`;
