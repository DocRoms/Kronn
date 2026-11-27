import { useId, useState, type ReactNode } from 'react';
import { Archive, Copy, Pencil, Plus, RefreshCw, Trash2 } from 'lucide-react';
import { Button, type ButtonSize, type ButtonVariant } from '../ui/Button';
import { CardHeader } from '../ui/CardHeader';
import { ConfirmDialog } from '../ui/ConfirmDialog';
import { DetailHeader } from '../ui/DetailHeader';
import { Dialog } from '../ui/Dialog';
import { Drawer } from '../ui/Drawer';
import { DrawerHeader } from '../ui/DrawerHeader';
import { EmptyState } from '../ui/EmptyState';
import { ListHeader } from '../ui/ListHeader';
import { SectionHeader } from '../ui/SectionHeader';
import { TabHeader } from '../ui/TabHeader';
import { Field } from '../ui/Field';
import { FormFooter } from '../ui/FormFooter';
import { IconButton } from '../ui/IconButton';
import { LoadingState, Skeleton } from '../ui/LoadingState';
import { Menu, type MenuItem } from '../ui/Menu';
import { Notice } from '../ui/Notice';
import { SearchField } from '../ui/SearchField';
import { Select, type SelectOption } from '../ui/Select';
import { StatusBadge } from '../ui/StatusBadge';
import { Switch } from '../ui/Switch';
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

/** The agents the form demos offer: product names, the same in every locale. */
const DEMO_AGENTS: readonly SelectOption[] = [
  { value: 'claude', label: 'Claude Code' },
  { value: 'codex', label: 'Codex' },
  { value: 'gemini', label: 'Gemini CLI' },
];

/** KT-1882 — a Field in each of its states: empty, filled, help, error,
 *  disabled, required, optional. */
function FieldDemo({ t }: { t: T }) {
  const name = t('designSystem.demo.workflowName');
  const placeholder = t('designSystem.demo.workflowNamePlaceholder');
  const value = t('designSystem.demo.workflowNameValue');
  return (
    <>
      <Row label={t('designSystem.demo.state.empty')}>
        <Field label={name}><input type="text" placeholder={placeholder} /></Field>
      </Row>
      <Row label={t('designSystem.demo.state.filled')}>
        <Field label={name}><input type="text" defaultValue={value} /></Field>
      </Row>
      <Row label={t('designSystem.demo.state.help')}>
        <Field label={name} help={t('designSystem.demo.workflowNameHelp')}><input type="text" defaultValue={value} /></Field>
      </Row>
      <Row label={t('designSystem.demo.state.error')}>
        <Field label={name} error={t('designSystem.demo.workflowNameError')} help={t('designSystem.demo.workflowNameHelp')}>
          <input type="text" defaultValue={value} />
        </Field>
      </Row>
      <Row label={t('designSystem.demo.state.disabled')}>
        <Field label={name} disabled><input type="text" defaultValue={value} /></Field>
      </Row>
      <Row label={t('designSystem.demo.state.required')}>
        <Field label={name} required><input type="text" placeholder={placeholder} /></Field>
      </Row>
      <Row label={t('designSystem.demo.state.optional')}>
        <Field label={t('designSystem.demo.description')} optional><textarea placeholder={t('designSystem.demo.descriptionPlaceholder')} /></Field>
      </Row>
    </>
  );
}

/** KT-1882 — a Select, named by its Field, in each state, and the small size. */
function SelectDemo({ t }: { t: T }) {
  const [empty, setEmpty] = useState('');
  const [filled, setFilled] = useState('claude');
  const [small, setSmall] = useState('codex');
  const agent = t('designSystem.demo.agent');
  const placeholder = t('designSystem.demo.agentPlaceholder');
  const none = () => {};
  return (
    <>
      <Row label={t('designSystem.demo.state.empty')}>
        <Field label={agent}><Select value={empty} onChange={setEmpty} placeholder={placeholder} options={DEMO_AGENTS} /></Field>
      </Row>
      <Row label={t('designSystem.demo.state.filled')}>
        <Field label={agent}><Select value={filled} onChange={setFilled} options={DEMO_AGENTS} /></Field>
      </Row>
      <Row label={t('designSystem.demo.state.help')}>
        <Field label={agent} help={t('designSystem.demo.agentHelp')}><Select value={filled} onChange={setFilled} options={DEMO_AGENTS} /></Field>
      </Row>
      <Row label={t('designSystem.demo.state.error')}>
        <Field label={agent} error={t('designSystem.demo.agentError')}>
          <Select value="gemini" onChange={none} options={DEMO_AGENTS} />
        </Field>
      </Row>
      <Row label={t('designSystem.demo.state.disabled')}>
        <Field label={agent} disabled><Select value="claude" onChange={none} options={DEMO_AGENTS} /></Field>
      </Row>
      <Row label={t('designSystem.demo.state.required')}>
        <Field label={agent} required><Select value={empty} onChange={setEmpty} placeholder={placeholder} options={DEMO_AGENTS} /></Field>
      </Row>
      <Row label={t('designSystem.demo.size.sm')}>
        <Field label={agent}><Select size="sm" value={small} onChange={setSmall} options={DEMO_AGENTS} /></Field>
      </Row>
    </>
  );
}

/** KT-1882 — a Switch off, on, with its description, and disabled. */
function SwitchDemo({ t }: { t: T }) {
  const [off, setOff] = useState(false);
  const [on, setOn] = useState(true);
  const [described, setDescribed] = useState(false);
  const none = () => {};
  return (
    <>
      <Row label={t('designSystem.demo.state.off')}>
        <Switch label={t('designSystem.demo.notify')} checked={off} onChange={setOff} />
      </Row>
      <Row label={t('designSystem.demo.state.on')}>
        <Switch label={t('designSystem.demo.autoMerge')} checked={on} onChange={setOn} />
      </Row>
      <Row label={t('designSystem.demo.state.help')}>
        <Switch
          label={t('designSystem.demo.notify')}
          description={t('designSystem.demo.notifyDescription')}
          checked={described}
          onChange={setDescribed}
        />
      </Row>
      <Row label={t('designSystem.demo.state.disabled')}>
        <Switch label={t('designSystem.demo.notify')} checked={false} disabled onChange={none} />
        <Switch label={t('designSystem.demo.autoMerge')} checked disabled onChange={none} />
      </Row>
    </>
  );
}

/** KT-1882 — a form that scrolls, its footer pinned: the action is held, with
 *  its reasons, until the two required fields are filled. Then a busy and a
 *  danger footer, in the flow. */
function FormFooterDemo({ t }: { t: T }) {
  const [name, setName] = useState('');
  const [agent, setAgent] = useState('');
  const [created, setCreated] = useState<string | null>(null);
  const blockers = [
    ...(name.trim() === '' ? [t('designSystem.demo.blockerName')] : []),
    ...(agent === '' ? [t('designSystem.demo.blockerAgent')] : []),
  ];
  const reset = () => { setName(''); setAgent(''); setCreated(null); };
  return (
    <>
      <Row label={t('designSystem.demo.state.pinned')}>
        <form
          className="ds-demo-form"
          data-testid="ds-form-demo"
          onSubmit={e => { e.preventDefault(); setCreated(name.trim()); }}
        >
          <Field label={t('designSystem.demo.workflowName')} required>
            <input type="text" value={name} placeholder={t('designSystem.demo.workflowNamePlaceholder')} onChange={e => setName(e.target.value)} />
          </Field>
          <Field label={t('designSystem.demo.agent')} required>
            <Select value={agent} onChange={setAgent} placeholder={t('designSystem.demo.agentPlaceholder')} options={DEMO_AGENTS} />
          </Field>
          <Field label={t('designSystem.demo.description')} optional>
            <textarea placeholder={t('designSystem.demo.descriptionPlaceholder')} />
          </Field>
          <FormFooter submitLabel={t('designSystem.demo.createWorkflow')} onCancel={reset} blockers={blockers} />
        </form>
      </Row>
      <p className="ds-demo-note" aria-live="polite" data-testid="ds-form-created">
        {created ? t('designSystem.demo.created', created) : t('designSystem.demo.createdNone')}
      </p>
      <Row label={t('designSystem.demo.state.busy')}>
        <FormFooter submitLabel={t('designSystem.demo.saveWorkflow')} onSubmit={() => {}} onCancel={() => {}} busy pinned={false} />
      </Row>
      <Row label={t('designSystem.demo.variant.danger')}>
        <FormFooter submitLabel={t('designSystem.demo.deleteWorkflow')} tone="danger" onSubmit={() => {}} onCancel={() => {}} pinned={false} />
      </Row>
    </>
  );
}

/** KT-1883 — a Notice in each of its four tones; the danger one with its
 *  action, the success one dismissible. */
function NoticeDemo({ t }: { t: T }) {
  const [dismissed, setDismissed] = useState(false);
  const [retried, setRetried] = useState(0);
  return (
    <>
      <Row label={t('designSystem.demo.tone.info')}>
        <Notice tone="info" title={t('designSystem.demo.noticeInfoTitle')}>{t('designSystem.demo.noticeInfo')}</Notice>
      </Row>
      <Row label={t('designSystem.demo.tone.warning')}>
        <Notice tone="warning" title={t('designSystem.demo.noticeWarningTitle')}>{t('designSystem.demo.noticeWarning')}</Notice>
      </Row>
      <Row label={t('designSystem.demo.tone.danger')}>
        <Notice
          tone="danger"
          title={t('designSystem.demo.noticeDangerTitle')}
          action={{ label: t('designSystem.demo.retry'), onClick: () => setRetried(n => n + 1) }}
        >
          <span data-testid="ds-notice-retried" data-count={retried}>{t('designSystem.demo.noticeDanger')}</span>
        </Notice>
      </Row>
      <Row label={t('designSystem.demo.tone.success')}>
        {dismissed
          ? <Button onClick={() => setDismissed(false)}>{t('designSystem.demo.showNoticeAgain')}</Button>
          : (
            <Notice tone="success" title={t('designSystem.demo.noticeSuccessTitle')} onDismiss={() => setDismissed(true)}>
              {t('designSystem.demo.noticeSuccess')}
            </Notice>
          )}
      </Row>
    </>
  );
}

/** KT-1883 — the three cases of an EmptyState, each with its way out. */
function EmptyStateDemo({ t }: { t: T }) {
  const none = () => {};
  return (
    <>
      <Row label={t('designSystem.demo.empty.firstUse')}>
        <div className="ds-demo-frame">
          <EmptyState
            kind="firstUse"
            title={t('designSystem.demo.emptyFirstUseTitle')}
            description={t('designSystem.demo.emptyFirstUse')}
            action={{ label: t('designSystem.demo.emptyFirstUseAction'), icon: <Plus />, onClick: none }}
          />
        </div>
      </Row>
      <Row label={t('designSystem.demo.empty.noResult')}>
        <div className="ds-demo-frame">
          <EmptyState
            kind="noResult"
            title={t('designSystem.demo.emptyNoResultTitle')}
            description={t('designSystem.demo.emptyNoResult')}
            onClear={none}
          />
        </div>
      </Row>
      <Row label={t('designSystem.demo.empty.nothingSelected')}>
        <div className="ds-demo-frame">
          <EmptyState
            kind="nothingSelected"
            title={t('designSystem.demo.emptyNothingSelectedTitle')}
            description={t('designSystem.demo.emptyNothingSelected')}
          />
        </div>
      </Row>
    </>
  );
}

/** KT-1883 — LoadingState in its three sizes, then the list and detail
 *  skeletons. The demos stay mounted: they are content, so they must not hold
 *  the cold-start screen when Settings is the first page opened. */
function LoadingStateDemo({ t }: { t: T }) {
  const message = t('designSystem.demo.loadingTasks');
  return (
    <>
      <Row label={t('designSystem.demo.size.sm')}>
        <LoadingState size="sm" message={message} />
      </Row>
      <Row label={t('designSystem.demo.size.md')}>
        <div className="ds-demo-frame"><LoadingState size="md" message={message} holdsBootScreen={false} /></div>
      </Row>
      <Row label={t('designSystem.demo.size.lg')}>
        <div className="ds-demo-frame"><LoadingState size="lg" message={message} holdsBootScreen={false} /></div>
      </Row>
      <Row label={t('designSystem.demo.skeleton.list')}>
        <div className="ds-demo-frame"><Skeleton kind="list" label={message} /></div>
      </Row>
      <Row label={t('designSystem.demo.skeleton.detail')}>
        <div className="ds-demo-frame"><Skeleton kind="detail" label={message} /></div>
      </Row>
    </>
  );
}

/** KT-1884 — one header kind in its normal and its compact form, each in a
 *  frame as wide as the row. A demo header is plain content: it holds no
 *  screen and covers nothing. The compact row carries a long title, to show
 *  it ellipsizing while the actions stay on screen. */
function HeaderRows({ t, render }: { t: T; render: (compact: boolean, id: string) => ReactNode }) {
  const base = useId();
  return (
    <>
      <Row label={t('designSystem.demo.header.normal')}>
        <div className="ds-demo-frame ds-demo-header-frame">{render(false, `${base}-normal`)}</div>
      </Row>
      <Row label={t('designSystem.demo.header.compact')}>
        <div className="ds-demo-frame ds-demo-header-frame">{render(true, `${base}-compact`)}</div>
      </Row>
    </>
  );
}

function headerMenu(t: T): ReactNode {
  const none = () => {};
  return (
    <Menu
      label={t('designSystem.demo.menuLabel')}
      size="sm"
      items={[{ id: 'rename', label: t('designSystem.demo.rename'), icon: <Pencil />, onSelect: none }]}
      destructiveItem={{ id: 'delete', label: t('designSystem.demo.delete'), icon: <Trash2 />, onSelect: none }}
    />
  );
}

function ListHeaderDemo({ t }: { t: T }) {
  return (
    <HeaderRows t={t} render={compact => (
      <ListHeader
        compact={compact}
        title={compact ? t('designSystem.demo.header.longTitle') : t('designSystem.demo.header.listTitle')}
        description={t('designSystem.demo.header.listCount')}
        actions={<Button variant="primary" size={compact ? 'sm' : 'md'} icon={<Plus />}>{t('designSystem.demo.header.listAction')}</Button>}
      />
    )} />
  );
}

function DetailHeaderDemo({ t }: { t: T }) {
  return (
    <HeaderRows t={t} render={compact => (
      <DetailHeader
        compact={compact}
        title={compact ? t('designSystem.demo.header.longTitle') : t('designSystem.demo.header.detailTitle')}
        meta={<code>wf-42</code>}
        status={<StatusBadge tone="info">{t('designSystem.demo.status.info')}</StatusBadge>}
        description={t('designSystem.demo.header.detailDescription')}
        actions={<><Button size="sm">{t('designSystem.demo.header.run')}</Button>{headerMenu(t)}</>}
      />
    )} />
  );
}

function TabHeaderDemo({ t }: { t: T }) {
  return (
    <HeaderRows t={t} render={compact => (
      <TabHeader
        compact={compact}
        title={compact ? t('designSystem.demo.header.longTitle') : t('designSystem.demo.header.tabTitle')}
        meta={<span>4</span>}
        actions={<Button size="sm" icon={<Plus />}>{t('designSystem.demo.header.tabAction')}</Button>}
      />
    )} />
  );
}

function SectionHeaderDemo({ t }: { t: T }) {
  return (
    <HeaderRows t={t} render={compact => (
      <SectionHeader
        compact={compact}
        title={compact ? t('designSystem.demo.header.longTitle') : t('designSystem.demo.header.sectionTitle')}
        description={t('designSystem.demo.header.sectionHint')}
        actions={<IconButton size="sm" label={t('designSystem.demo.add')} icon={<Plus />} />}
      />
    )} />
  );
}

function DrawerHeaderDemo({ t }: { t: T }) {
  const [closed, setClosed] = useState(0);
  return (
    <>
      <HeaderRows t={t} render={(compact, id) => (
        <div role="group" aria-labelledby={id}>
          <DrawerHeader
            compact={compact}
            titleId={id}
            title={compact ? t('designSystem.demo.header.longTitle') : t('designSystem.demo.header.drawerTitle')}
            status={<StatusBadge tone="success">{t('designSystem.demo.header.drawerShared')}</StatusBadge>}
            description={t('designSystem.demo.header.drawerDescription')}
            actions={<IconButton size="sm" label={t('designSystem.demo.header.refresh')} icon={<RefreshCw />} />}
            onClose={() => setClosed(n => n + 1)}
            closeLabel={t('ui.overlay.close')}
          />
        </div>
      )} />
      <p className="ds-demo-note" aria-live="polite" data-testid="ds-drawer-header-closed" data-count={closed}>
        {t('designSystem.demo.header.closeHint')}
      </p>
    </>
  );
}

function CardHeaderDemo({ t }: { t: T }) {
  return (
    <HeaderRows t={t} render={(compact, id) => (
      <article className="ds-demo-card" aria-labelledby={id}>
        <CardHeader
          compact={compact}
          titleId={id}
          title={compact ? t('designSystem.demo.header.longTitle') : t('designSystem.demo.header.cardTitle')}
          status={(
            <>
              <StatusBadge tone="success">{t('designSystem.demo.header.cardConnected')}</StatusBadge>
              {!compact && <StatusBadge tone="warning">{t('designSystem.demo.status.warning')}</StatusBadge>}
            </>
          )}
          description="api.github.com"
          actions={headerMenu(t)}
        />
      </article>
    )} />
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
  field: FieldDemo,
  select: SelectDemo,
  switch: SwitchDemo,
  formFooter: FormFooterDemo,
  notice: NoticeDemo,
  emptyState: EmptyStateDemo,
  loadingState: LoadingStateDemo,
  listHeader: ListHeaderDemo,
  detailHeader: DetailHeaderDemo,
  tabHeader: TabHeaderDemo,
  sectionHeader: SectionHeaderDemo,
  drawerHeader: DrawerHeaderDemo,
  cardHeader: CardHeaderDemo,
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
            <h3 id={anchor} className="ds-component-title" tabIndex={-1}><code>{component.name}</code></h3>
            <p className="ds-caption">{t(component.caption)}</p>
            <div className="ds-demo">{Demo ? <Demo t={t} /> : null}</div>
          </section>
        );
      })}
    </>
  );
}
