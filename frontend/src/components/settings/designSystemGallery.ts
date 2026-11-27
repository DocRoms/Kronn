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

export const GALLERY_COMPONENTS: readonly GalleryComponent[] = [
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
];

/** Anchor of the tokens part of the page. */
export const TOKENS_ANCHOR = 'ds-tokens';

export const componentAnchor = (id: string) => `ds-c-${id}`;
