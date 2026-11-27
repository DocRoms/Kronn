import { useState, type ReactNode } from 'react';
import { Archive, Copy, Pencil, Plus, Trash2 } from 'lucide-react';
import { Button, type ButtonSize, type ButtonVariant } from '../ui/Button';
import { ConfirmDialog } from '../ui/ConfirmDialog';
import { Dialog } from '../ui/Dialog';
import { Drawer } from '../ui/Drawer';
import { IconButton } from '../ui/IconButton';
import { Menu, type MenuItem } from '../ui/Menu';
import { SearchField } from '../ui/SearchField';
import { StatusBadge } from '../ui/StatusBadge';
import { Tabs } from '../ui/Tabs';
import { Tooltip } from '../ui/Tooltip';
import { STATUS_TONES } from '../ui/statusTones';
import type { OverlaySize } from '../ui/overlay';
import { OVERLAY_WIDTHS } from '../../styles/designTokens';
import { GALLERY_COMPONENTS, componentAnchor } from './designSystemGallery';

type T = (key: string, ...args: (string | number)[]) => string;

const VARIANTS: readonly ButtonVariant[] = ['primary', 'neutral', 'ghost', 'danger'];
const SIZES: readonly ButtonSize[] = ['sm', 'md', 'lg'];

/** One labelled row of examples. */
function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="ds-demo-row">
      <span className="ds-demo-label">{label}</span>
      <div className="ds-demo-items">{children}</div>
    </div>
  );
}

function ButtonDemo({ t }: { t: T }) {
  return (
    <>
      {VARIANTS.map(variant => (
        <Row key={variant} label={t(`designSystem.demo.variant.${variant}`)}>
          {SIZES.map(size => (
            <Button key={size} variant={variant} size={size}>{t(`designSystem.demo.size.${size}`)}</Button>
          ))}
          <Button variant={variant} icon={<Plus />}>{t('designSystem.demo.withIcon')}</Button>
          <Button variant={variant} disabled>{t('designSystem.demo.disabled')}</Button>
        </Row>
      ))}
    </>
  );
}

function IconButtonDemo({ t }: { t: T }) {
  const icons: Record<ButtonVariant, { icon: ReactNode; label: string }> = {
    ghost: { icon: <Copy />, label: t('designSystem.demo.copy') },
    neutral: { icon: <Pencil />, label: t('designSystem.demo.edit') },
    primary: { icon: <Plus />, label: t('designSystem.demo.add') },
    danger: { icon: <Trash2 />, label: t('designSystem.demo.delete') },
  };
  return (
    <>
      {(['ghost', 'neutral', 'primary', 'danger'] as const).map(variant => (
        <Row key={variant} label={t(`designSystem.demo.variant.${variant}`)}>
          {SIZES.map(size => (
            <IconButton key={size} variant={variant} size={size} label={icons[variant].label} icon={icons[variant].icon} />
          ))}
          <IconButton variant={variant} label={icons[variant].label} icon={icons[variant].icon} disabled />
        </Row>
      ))}
    </>
  );
}

function MenuDemo({ t }: { t: T }) {
  const [chosen, setChosen] = useState<string | null>(null);
  const item = (id: string, icon: ReactNode): MenuItem => {
    const label = t(`designSystem.demo.${id}`);
    return { id, label, icon, onSelect: () => setChosen(label) };
  };
  const items = [item('rename', <Pencil />), item('duplicate', <Copy />), item('archive', <Archive />)];
  const destructive = item('delete', <Trash2 />);
  return (
    <>
      <Row label={t('designSystem.demo.iconTrigger')}>
        <Menu label={t('designSystem.demo.menuLabel')} items={items} destructiveItem={destructive} align="start" />
      </Row>
      <Row label={t('designSystem.demo.textTrigger')}>
        <Menu label={t('designSystem.demo.options')} showLabel variant="neutral" items={items} destructiveItem={destructive} align="start" />
      </Row>
      <p className="ds-demo-note" aria-live="polite" data-testid="ds-menu-chosen">
        {chosen ? t('designSystem.demo.chosen', chosen) : t('designSystem.demo.chosenNone')}
      </p>
    </>
  );
}

function StatusBadgeDemo({ t }: { t: T }) {
  return (
    <Row label={t('designSystem.demo.tones')}>
      {STATUS_TONES.map(tone => (
        <StatusBadge key={tone} tone={tone}>{t(`designSystem.demo.status.${tone}`)}</StatusBadge>
      ))}
    </Row>
  );
}

function TabsDemo({ t }: { t: T }) {
  const tab = (id: string, extra: { badge?: ReactNode; disabled?: boolean } = {}) => {
    const label = t(`designSystem.demo.tab.${id}`);
    return { id, label, ...extra, panel: <p className="ds-demo-note">{t('designSystem.demo.panel', label)}</p> };
  };
  const tabs = [tab('overview'), tab('activity', { badge: 3 }), tab('archived', { disabled: true })];
  return (
    <>
      <Row label={t('designSystem.demo.size.md')}>
        <Tabs label={t('designSystem.demo.tabs')} tabs={tabs} />
      </Row>
      <Row label={t('designSystem.demo.size.sm')}>
        <Tabs label={t('designSystem.demo.tabs')} size="sm" tabs={tabs.map(({ panel: _panel, ...rest }) => rest)} />
      </Row>
    </>
  );
}

function SearchFieldDemo({ t }: { t: T }) {
  const [md, setMd] = useState('');
  const [sm, setSm] = useState('');
  return (
    <>
      <Row label={t('designSystem.demo.size.md')}>
        <SearchField label={t('designSystem.demo.search')} placeholder={t('designSystem.demo.searchPlaceholder')} value={md} onChange={setMd} />
      </Row>
      <Row label={t('designSystem.demo.size.sm')}>
        <SearchField label={t('designSystem.demo.search')} showLabel size="sm" value={sm} onChange={setSm} />
      </Row>
    </>
  );
}

function TooltipDemo({ t }: { t: T }) {
  return (
    <Row label={t('designSystem.demo.variant.neutral')}>
      <Tooltip content={t('designSystem.demo.tooltipText')}>
        <Button>{t('designSystem.demo.schedule')}</Button>
      </Tooltip>
    </Row>
  );
}

/** One button per size (480, 640, 960 px) that opens the overlay in it. */
function SizeButtons({ t, open, onOpen }: { t: T; open: string; onOpen: (size: OverlaySize) => void }) {
  return (
    <Row label={t('designSystem.demo.sizes')}>
      {SIZES.map(size => (
        <Button key={size} data-size-demo={size} onClick={() => onOpen(size)}>
          {t('designSystem.demo.openIn', open, t(`designSystem.demo.size.${size}`), OVERLAY_WIDTHS[size])}
        </Button>
      ))}
    </Row>
  );
}

function DialogDemo({ t }: { t: T }) {
  const [size, setSize] = useState<OverlaySize | null>(null);
  const close = () => setSize(null);
  return (
    <>
      <SizeButtons t={t} open={t('designSystem.demo.openDialog')} onOpen={setSize} />
      <Dialog
        open={size !== null}
        onClose={close}
        size={size ?? 'md'}
        title={t('designSystem.demo.dialogTitle')}
        description={t('designSystem.demo.dialogDescription')}
        footer={(
          <>
            <Button onClick={close}>{t('designSystem.demo.cancel')}</Button>
            <Button variant="primary" onClick={close}>{t('designSystem.demo.saveName')}</Button>
          </>
        )}
      >
        <label className="ds-demo-field">
          <span>{t('designSystem.demo.nameField')}</span>
          <input type="text" defaultValue="Kronn" />
        </label>
      </Dialog>
    </>
  );
}

function DrawerDemo({ t }: { t: T }) {
  const [size, setSize] = useState<OverlaySize | null>(null);
  return (
    <>
      <SizeButtons t={t} open={t('designSystem.demo.openDrawer')} onOpen={setSize} />
      <Drawer
        open={size !== null}
        onClose={() => setSize(null)}
        size={size ?? 'md'}
        title={t('designSystem.demo.drawerTitle')}
        description={t('designSystem.demo.drawerDescription')}
      >
        <ul className="ds-demo-list">
          <li>{t('designSystem.demo.drawerItem1')}</li>
          <li>{t('designSystem.demo.drawerItem2')}</li>
          <li>{t('designSystem.demo.drawerItem3')}</li>
        </ul>
      </Drawer>
    </>
  );
}

function ConfirmDialogDemo({ t }: { t: T }) {
  const [size, setSize] = useState<OverlaySize | null>(null);
  const [answer, setAnswer] = useState<'confirmed' | 'cancelled' | null>(null);
  const answerWith = (value: 'confirmed' | 'cancelled') => { setAnswer(value); setSize(null); };
  return (
    <>
      <SizeButtons t={t} open={t('designSystem.demo.openConfirm')} onOpen={setSize} />
      <ConfirmDialog
        open={size !== null}
        size={size ?? 'sm'}
        title={t('designSystem.demo.confirmTitle')}
        message={t('designSystem.demo.confirmMessage')}
        confirmLabel={t('designSystem.demo.confirmAction')}
        onConfirm={() => answerWith('confirmed')}
        onCancel={() => answerWith('cancelled')}
      />
      <p className="ds-demo-note" aria-live="polite" data-testid="ds-confirm-answer">
        {answer === 'confirmed' ? t('designSystem.demo.confirmed')
          : answer === 'cancelled' ? t('designSystem.demo.cancelled')
            : t('designSystem.demo.answerNone')}
      </p>
    </>
  );
}

const DEMOS: Record<string, (props: { t: T }) => ReactNode> = {
  button: ButtonDemo,
  iconButton: IconButtonDemo,
  menu: MenuDemo,
  statusBadge: StatusBadgeDemo,
  tabs: TabsDemo,
  searchField: SearchFieldDemo,
  tooltip: TooltipDemo,
  dialog: DialogDemo,
  drawer: DrawerDemo,
  confirmDialog: ConfirmDialogDemo,
};

/** KT-1880 — each component of components/ui/ under its own heading, with its
 *  variants and states, in the current theme. */
export function DesignSystemComponents({ t }: { t: T }) {
  return (
    <>
      {GALLERY_COMPONENTS.map(component => {
        const Demo = DEMOS[component.id];
        const anchor = componentAnchor(component.id);
        return (
          <section key={component.id} className="ds-component" data-component={component.id} aria-labelledby={anchor}>
            <h4 id={anchor} className="ds-component-title" tabIndex={-1}><code>{component.name}</code></h4>
            <p className="ds-caption">{t(component.caption)}</p>
            <div className="ds-demo">{Demo ? <Demo t={t} /> : null}</div>
          </section>
        );
      })}
    </>
  );
}
