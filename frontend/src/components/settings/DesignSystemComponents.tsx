import { useId, useState, type ReactNode } from 'react';
import { Activity, Archive, Coins, Copy, Pencil, Plus, RefreshCw, Trash2, TriangleAlert } from 'lucide-react';
import { Button, type ButtonSize, type ButtonVariant } from '../ui/Button';
import { CardHeader } from '../ui/CardHeader';
import { ConfirmDialog } from '../ui/ConfirmDialog';
import { CopyButton } from '../ui/CopyButton';
import { KeyValueList, type KeyValueItem } from '../ui/KeyValueList';
import { ProgressBar, type ProgressSegment } from '../ui/ProgressBar';
import { StatGrid, StatTile } from '../ui/StatTile';
import { Table, type TableColumn } from '../ui/Table';
import { PROGRESS_TONES, type TableSort } from '../ui/dataKinds';
import { DiffViewer } from '../ui/DiffViewer';
import { LogViewer } from '../ui/LogViewer';
import type { LogLine } from '../ui/logKinds';
import type { DiffFileModel, DiffSelection } from '../../lib/diffModel';
import { highlightLine, languageForPath } from '../../lib/diff-syntax';
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
import { useT } from '../../lib/I18nContext';
import { AgentSwitchPicker, type AgentAvailability, type AgentSwitchTarget } from '../AgentSwitchPicker';
import { AgentIdentity } from '../ui/AgentIdentity';
import { AGENT_IDENTITY_UNKNOWN_STATES, agentIdentityText, type AgentIdentityModel, type AgentIdentityRole } from '../ui/agentIdentityKinds';
import { ParticipantsStrip } from '../ParticipantsStrip';
import { agentTextColor } from '../../lib/constants';
import type { AgentType, ModelTier, ShadowAgent } from '../../types/generated';

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

/** Sample agents of the picker demo: product names, the same in every locale. */
const DEMO_PICKER_TARGETS: AgentSwitchTarget[] = [
  { agent: 'ClaudeCode' }, { agent: 'Codex' }, { agent: 'GeminiCli' }, { agent: 'Ollama' },
];

/** KT-1885 — the one agent picker in its four modes: a field (agent + tier),
 *  a compact chip, a multi-choice list and an exact model. Gemini CLI shows
 *  how an unavailable agent reads in every mode. */
function AgentSwitchPickerDemo({ t }: { t: T }) {
  const [field, setField] = useState<{ agent: AgentType; tier: ModelTier }>({ agent: 'ClaudeCode', tier: 'default' });
  const [withDefault, setWithDefault] = useState<{ agent: AgentType; tier: ModelTier | null }>({ agent: 'ClaudeCode', tier: null });
  const tierLabels = { economy: t('disc.tier.economy'), default: t('disc.tier.default'), reasoning: t('disc.tier.reasoning') };
  const [chip, setChip] = useState<AgentType>('Codex');
  const [many, setMany] = useState<AgentSwitchTarget[]>([{ agent: 'ClaudeCode' }, { agent: 'Codex' }]);
  const [exact, setExact] = useState<ShadowAgent | undefined>(undefined);
  // KT-1994 — the full list: modes with their model, other models, « Personnalisé », « Récents ».
  const [full, setFull] = useState<{ target: AgentSwitchTarget; tier: ModelTier; model: string | null }>({
    target: { agent: 'ClaudeCode' }, tier: 'default', model: null,
  });
  const label = t('designSystem.demo.agentLabel');
  const availability = (target: AgentSwitchTarget): AgentAvailability | null => target.agent === 'GeminiCli'
    ? { state: 'unavailable', label: t('designSystem.demo.agentUnavailable') }
    : null;
  const common = { availableAgents: [], availableTargets: DEMO_PICKER_TARGETS, availability, title: label, ariaLabel: label };
  return (
    <>
      <Row label={t('designSystem.demo.agentField')}>
        <div className="ds-demo-agent-field">
          <AgentSwitchPicker
            {...common}
            variant="field"
            currentAgent={field.agent}
            currentTier={field.tier}
            tierLabels={tierLabels}
            defaultModelLabel={t('disc.defaultAgentModel')}
            onTargetSelectionChange={async (target, tier) => setField({ agent: target.agent, tier })}
          />
        </div>
      </Row>
      <Row label={t('designSystem.demo.agentFullList')}>
        <div className="ds-demo-agent-field">
          <AgentSwitchPicker
            {...common}
            variant="field"
            currentAgent={full.target.agent}
            currentConnectionId={full.target.connectionId}
            currentTier={full.tier}
            tierLabels={tierLabels}
            defaultModelLabel={t('disc.defaultAgentModel')}
            onTargetSelectionChange={async (target, tier) => setFull({ target, tier, model: null })}
            customModel={{
              model: full.model,
              onSelect: async (target, model) => setFull(current => ({ target, tier: current.tier, model })),
            }}
          />
        </div>
      </Row>
      {/* The API assistants' list: a fourth track, « Défaut », the agent's own model. */}
      <Row label={t('designSystem.demo.agentWithDefault')}>
        <AgentSwitchPicker
          {...common}
          currentAgent={withDefault.agent}
          currentTier={withDefault.tier}
          tierLabels={tierLabels}
          defaultModelLabel={t('disc.defaultAgentModel')}
          onTargetSelectionChange={async (target, tier) => setWithDefault({ agent: target.agent, tier })}
          onDefaultTierSelection={async target => setWithDefault({ agent: target.agent, tier: null })}
        />
      </Row>
      <Row label={t('designSystem.demo.agentChip')}>
        <AgentSwitchPicker {...common} compact currentAgent={chip} onChange={async agent => setChip(agent)} />
      </Row>
      <Row label={t('designSystem.demo.agentMultiple')}>
        <AgentSwitchPicker
          {...common}
          currentAgent="ClaudeCode"
          multiple={{
            selected: many,
            onToggle: target => setMany(current => current.some(item => item.agent === target.agent)
              ? current.filter(item => item.agent !== target.agent)
              : [...current, target]),
          }}
        />
      </Row>
      <Row label={t('designSystem.demo.agentExactModel')}>
        <AgentSwitchPicker
          {...common}
          currentAgent="ClaudeCode"
          exactModel={{ value: exact, onChange: setExact, main: { agent: 'ClaudeCode', tier: 'default' }, testId: 'ds-agent-exact' }}
        />
      </Row>
    </>
  );
}

/** KT-1936 — sample identities: product names and model ids, the same in every locale. */
const DEMO_IDENTITIES: readonly { agent: AgentType; name: string; model: AgentIdentityModel; role: AgentIdentityRole; tier?: ModelTier }[] = [
  { agent: 'ClaudeCode', name: '@claude', model: { kind: 'id', id: 'claude-opus-4-5-20251101' }, role: 'main', tier: 'reasoning' },
  { agent: 'Codex', name: '@codex', model: { kind: 'id', id: 'gpt-5.5-codex' }, role: 'oneOff' },
  { agent: 'GeminiCli', name: '@gemini-cli', model: { kind: 'notDeclared' }, role: 'cli' },
  { agent: 'Kiro', name: '@kiro', model: { kind: 'id', id: 'auto' }, role: 'workflowStep' },
  { agent: 'Ollama', name: '@ollama', model: { kind: 'providerDefault' }, role: 'oneOff' },
];

function AgentIdentityDemo({ t }: { t: T }) {
  const { tp } = useT();
  const [main, codex] = DEMO_IDENTITIES;
  const identity = (d: typeof main, extra: Partial<Parameters<typeof AgentIdentity>[0]> = {}) => (
    <AgentIdentity name={d.name} color={agentTextColor(d.agent)} model={d.model} role={d.role} tier={d.tier}
      tierLabel={d.tier ? t(`disc.tier.${d.tier}`) : undefined} {...extra} />
  );
  const unknownModels: Record<string, string> = { notDeclared: '@gemini-cli', notRecorded: '@codex', providerDefault: '@ollama' };
  return (
    <>
      <Row label={t('designSystem.demo.identity.full')}>{identity(main, { size: 'full' })}</Row>
      <Row label={t('designSystem.demo.identity.inline')}>
        {identity(main)}
        {identity(codex)}
      </Row>
      <Row label={t('designSystem.demo.identity.compact')}>
        <span title={agentIdentityText({ name: codex.name, model: codex.model }, t)}>{identity(codex, { size: 'compact', role: null })}</span>
      </Row>
      <Row label={t('designSystem.demo.identity.unknown')}>
        {AGENT_IDENTITY_UNKNOWN_STATES.map(state => (
          <AgentIdentity key={state} name={unknownModels[state]}            color={agentTextColor(state === 'notDeclared' ? 'GeminiCli' : state === 'notRecorded' ? 'Codex' : 'Ollama')}
            model={{ kind: state }} role={state === 'notDeclared' ? 'cli' : 'oneOff'} />
        ))}
      </Row>
      <Row label={t('designSystem.demo.identity.strip')}>
        <div className="ds-demo-identity-strip">
          <ParticipantsStrip
            moreLabel={total => tp('disc.participantsMore', total)}
            allLabel={t('disc.participantsAll')}
            entries={DEMO_IDENTITIES.map(d => ({
              key: d.name,
              chip: (
                <Tooltip content={agentIdentityText({ name: d.name, model: d.model, role: d.role }, t)}>
                  <button type="button" className="disc-participant-chip"
                    aria-label={t('disc.participantDetails', agentIdentityText({ name: d.name, model: d.model, role: d.role }, t))}>
                    <span aria-hidden className="disc-participant-presence-dot" data-presence="listening" />
                    {identity(d, { truncateModel: true, tier: undefined })}
                  </button>
                </Tooltip>
              ),
              row: identity(d, { tier: undefined }),
              onSelect: () => {},
            }))}
          />
        </div>
      </Row>
    </>
  );
}

/** One narrow demo: what it shows, and the form it shows when there are
 *  several (the Table's cards and sideways scroll). */
interface NarrowDemo { id: string; form?: string; node: ReactNode }

/** KT-1886 — a data component in a wide container on its own line, then its
 *  narrow demos together on the next line, each in a 20rem container (they
 *  wrap under each other in a narrow window): the narrow form comes from the
 *  container's width, not from the window's, so both show on any screen. */
function WideAndNarrow({ t, wide, narrow }: { t: T; wide: ReactNode; narrow: ReactNode | readonly NarrowDemo[] }) {
  const demos: readonly NarrowDemo[] = Array.isArray(narrow) ? narrow : [{ id: 'narrow', node: narrow }];
  return (
    <div className="ds-demo-pair">
      <div className="ds-demo-container" data-form="wide">
        <span className="ds-demo-label">{t('designSystem.demo.data.wide')}</span>
        {wide}
      </div>
      <div className="ds-demo-narrow-line">
        {demos.map(demo => (
          <div key={demo.id} className="ds-demo-container" data-form="narrow">
            <span className="ds-demo-label">
              {t('designSystem.demo.data.narrow')}
              {demo.form && ` · ${demo.form}`}
            </span>
            {demo.node}
          </div>
        ))}
      </div>
    </div>
  );
}

/** The demos' numbers, written in the interface's locale. */
function useNumber() {
  const { locale } = useT();
  const integer = new Intl.NumberFormat(locale);
  const decimal = new Intl.NumberFormat(locale, { maximumFractionDigits: 1 });
  const percent = new Intl.NumberFormat(locale, { style: 'percent', maximumFractionDigits: 0 });
  return {
    number: (n: number) => integer.format(n),
    decimal: (n: number) => decimal.format(n),
    percent: (n: number) => percent.format(n),
  };
}

/** A clipboard that always refuses, to show the failure. */
const refusedCopy = (): string => { throw new Error('refused'); };

function CopyButtonDemo({ t }: { t: T }) {
  const command = 'git worktree add .kronn/worktrees/task-kt-1886-4e41d02c kronn/task/kt-1886-4e41d02c';
  const copyLine = (
    <p className="ds-demo-copy-line">
      <code>{command}</code>
      <CopyButton size="sm" text={command} label={t('designSystem.demo.data.copyCommand')} />
    </p>
  );
  return (
    <>
      <Row label={t('designSystem.demo.data.iconForm')}>
        {SIZES.map(size => <CopyButton key={size} size={size} text="kt-1886" label={t('designSystem.demo.data.copyId')} />)}
      </Row>
      <Row label={t('designSystem.demo.data.textForm')}>
        <CopyButton form="text" text={command} label={t('designSystem.demo.data.copyCommand')} />
      </Row>
      <Row label={t('designSystem.demo.data.failure')}>
        <CopyButton getText={refusedCopy} label={t('designSystem.demo.data.copyBroken')} />
        <CopyButton form="text" getText={refusedCopy} label={t('designSystem.demo.data.copyBroken')} />
      </Row>
      <WideAndNarrow t={t} wide={copyLine} narrow={copyLine} />
    </>
  );
}

interface DemoRun { id: string; name: string; status: 'failed' | 'done' | 'running' | 'cancelled'; tokens: number; seconds: number; worktree: string }

const DEMO_RUNS: readonly Omit<DemoRun, 'name'>[] = [
  { id: 'run1', status: 'failed', tokens: 18_240, seconds: 312, worktree: '/srv/kronn/.kronn/worktrees/task-kt-1886-4e41d02c' },
  { id: 'run2', status: 'done', tokens: 96_400, seconds: 1_284, worktree: '/srv/kronn/.kronn/worktrees/task-kt-1878-a1b2c3d4' },
  { id: 'run3', status: 'running', tokens: 4_310, seconds: 48, worktree: '/srv/kronn' },
  { id: 'run4', status: 'cancelled', tokens: 0, seconds: 9, worktree: '/srv/kronn/.kronn/worktrees/task-kt-1990-ffee0011' },
];

const RUN_TONES = { failed: 'danger', done: 'success', running: 'info', cancelled: 'neutral' } as const;
const RUN_STATUS_KEYS = {
  failed: 'designSystem.demo.data.statusFailed',
  done: 'designSystem.demo.data.statusDone',
  running: 'designSystem.demo.data.statusRunning',
  cancelled: 'designSystem.demo.data.statusCancelled',
} as const;

const formatDuration = (seconds: number) => `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;

/** The bounded table's rows: enough to scroll under its sticky header. */
const BOUNDED_ROWS = 12;

/** KT-1886 — a sortable, selectable table whose rows open, in its cards form
 *  in a wide container, then in a narrow one next to the sideways scroll
 *  form; then a bounded table with a sticky header, empty and loading. */
function TableDemo({ t }: { t: T }) {
  const { number } = useNumber();
  const runNames: Record<string, string> = {
    run1: t('designSystem.demo.data.run1'),
    run2: t('designSystem.demo.data.run2'),
    run3: t('designSystem.demo.data.run3'),
    run4: t('designSystem.demo.data.run4'),
  };
  const [sort, setSort] = useState<TableSort | null>({ column: 'tokens', direction: 'descending' });
  const [selected, setSelected] = useState<string[]>(['run2']);
  const [opened, setOpened] = useState<string | null>(null);
  const runs: DemoRun[] = DEMO_RUNS.map(run => ({ ...run, name: runNames[run.id] }));
  const bounded: DemoRun[] = Array.from({ length: BOUNDED_ROWS }, (_, i) => {
    const run = runs[i % runs.length];
    return { ...run, id: `bounded${i + 1}`, name: `${run.name} ${number(i + 1)}`, tokens: run.tokens + i * 137, seconds: run.seconds + i * 11 };
  });
  const sorted = sort ? [...runs].sort((a, b) => {
    const order = sort.column === 'name' ? a.name.localeCompare(b.name)
      : sort.column === 'duration' ? a.seconds - b.seconds
        : a.tokens - b.tokens;
    return sort.direction === 'ascending' ? order : -order;
  }) : runs;
  const columns: TableColumn<DemoRun>[] = [
    { id: 'name', header: t('designSystem.demo.data.colWorkflow'), cell: r => r.name, rowHeader: true, sortable: true },
    {
      id: 'status',
      header: t('designSystem.demo.data.colStatus'),
      cell: r => <StatusBadge tone={RUN_TONES[r.status]}>{t(RUN_STATUS_KEYS[r.status])}</StatusBadge>,
    },
    { id: 'tokens', header: t('designSystem.demo.data.colTokens'), cell: r => number(r.tokens), numeric: true, sortable: true },
    { id: 'duration', header: t('designSystem.demo.data.colDuration'), cell: r => formatDuration(r.seconds), numeric: true, sortable: true },
    { id: 'worktree', header: t('designSystem.demo.data.colWorktree'), cell: r => r.worktree, truncate: 'ellipsis' },
  ];
  const common = {
    caption: t('designSystem.demo.data.runsCaption'),
    columns,
    rows: sorted,
    getRowId: (r: DemoRun) => r.id,
    sort,
    onSortChange: setSort,
    rowTone: (r: DemoRun) => (r.status === 'failed' ? 'danger' as const : r.status === 'cancelled' ? 'muted' as const : undefined),
    selection: { selected, onChange: setSelected, rowLabel: (r: DemoRun) => r.name },
    actions: (r: DemoRun) => (
      <IconButton size="sm" variant="ghost" label={t('designSystem.demo.data.deleteRun', r.name)} icon={<Trash2 />} />
    ),
    onRowOpen: (r: DemoRun) => setOpened(r.name),
    footer: (
      <>
        <span>{t('designSystem.demo.data.moreRows')}</span>
        <Button size="sm">{t('designSystem.demo.data.showMore')}</Button>
      </>
    ),
  };
  return (
    <>
      <WideAndNarrow
        t={t}
        wide={<Table {...common} narrow="cards" stickyHeader maxHeight="md" />}
        narrow={[
          { id: 'cards', form: t('designSystem.demo.data.formCards'), node: <Table {...common} narrow="cards" stickyHeader hideCaption /> },
          { id: 'scroll', form: t('designSystem.demo.data.formScroll'), node: <Table {...common} density="compact" hideCaption /> },
        ]}
      />
      <p className="ds-demo-note" aria-live="polite" data-testid="ds-table-opened">
        {opened ? t('designSystem.demo.data.opened', opened) : t('designSystem.demo.data.openedNone')}
      </p>
      <Row label={t('designSystem.demo.data.bounded')}>
        <div className="ds-demo-container" data-form="wide">
          <Table
            caption={t('designSystem.demo.data.boundedCaption')}
            hideCaption
            columns={columns.filter(c => c.id !== 'status')}
            rows={bounded}
            getRowId={r => r.id}
            maxHeight="sm"
            stickyHeader
          />
        </div>
      </Row>
      <Row label={t('designSystem.demo.data.empty')}>
        <div className="ds-demo-container" data-form="wide">
          <Table
            caption={t('designSystem.demo.data.runsCaption')}
            hideCaption
            columns={columns}
            rows={[]}
            getRowId={r => r.id}
            empty={(
              <EmptyState
                kind="nothingSelected"
                title={t('designSystem.demo.data.emptyTitle')}
                description={t('designSystem.demo.data.emptyDescription')}
              />
            )}
          />
        </div>
      </Row>
      <Row label={t('designSystem.demo.data.loading')}>
        <div className="ds-demo-container" data-form="wide">
          <Table
            caption={t('designSystem.demo.data.runsCaption')}
            hideCaption
            columns={columns}
            rows={[]}
            getRowId={r => r.id}
            loading
            loadingLabel={t('designSystem.demo.data.loadingRuns')}
          />
        </div>
      </Row>
    </>
  );
}

function KeyValueListDemo({ t }: { t: T }) {
  const { number } = useNumber();
  const items: KeyValueItem[] = [
    { label: t('designSystem.demo.data.kvBranch'), value: 'kronn/task/kt-1886-4e41d02c', mono: true, copyable: true },
    { label: t('designSystem.demo.agent'), value: 'Claude Code' },
    { label: t('designSystem.demo.data.kvStarted'), value: null },
    { label: t('designSystem.demo.data.colWorktree'), value: '/srv/kronn/.kronn/worktrees/task-kt-1886-4e41d02c', mono: true, truncate: true },
    { label: t('designSystem.demo.data.colTokens'), value: number(48_210) },
  ];
  return (
    <>
      <Row label={t('designSystem.demo.data.grid')}>
        <WideAndNarrow t={t} wide={<KeyValueList items={items} />} narrow={<KeyValueList items={items} />} />
      </Row>
      <Row label={t('designSystem.demo.data.inline')}>
        <WideAndNarrow
          t={t}
          wide={<KeyValueList items={items} layout="inline" />}
          narrow={<KeyValueList items={items} layout="inline" />}
        />
      </Row>
    </>
  );
}

function StatTileDemo({ t }: { t: T }) {
  const { number, decimal } = useNumber();
  // Two filter tiles, one of them on: the pressed state shows in the gallery.
  const [filter, setFilter] = useState<'failed' | 'running' | null>('running');
  const toggle = (next: 'failed' | 'running') => () => setFilter(f => (f === next ? null : next));
  const tiles = (
    <StatGrid label={t('designSystem.demo.data.statWeek')}>
      <StatTile
        label={t('designSystem.demo.data.statRuns')}
        value={number(42)}
        icon={<Activity />}
        delta={{ text: t('designSystem.demo.data.deltaRuns'), direction: 'up', tone: 'success' }}
        hint={t('designSystem.demo.data.last7')}
      />
      <StatTile
        label={t('designSystem.demo.data.statFailed')}
        value={number(3)}
        icon={<TriangleAlert />}
        delta={{ text: t('designSystem.demo.data.deltaFailed'), direction: 'up', tone: 'danger' }}
        hint={t('designSystem.demo.data.filterHint')}
        onClick={toggle('failed')}
        selected={filter === 'failed'}
      />
      <StatTile
        label={t('designSystem.demo.data.statusRunning')}
        value={number(1)}
        icon={<RefreshCw />}
        hint={t('designSystem.demo.data.filterHint')}
        onClick={toggle('running')}
        selected={filter === 'running'}
      />
      <StatTile label={t('designSystem.demo.data.kvCost')} value={null} unit="$" icon={<Coins />} unknownReason={t('designSystem.demo.data.costUnknown')} />
      <StatTile label={t('designSystem.demo.data.colTokens')} value={decimal(96.4)} unit="k" loading />
    </StatGrid>
  );
  return (
    <>
      <WideAndNarrow t={t} wide={tiles} narrow={tiles} />
      <Row label={t('designSystem.demo.size.lg')}>
        <StatTile size="lg" label={t('designSystem.demo.data.colTokens')} value={decimal(96.4)} unit="k" delta={{ text: number(0), direction: 'flat' }} />
      </Row>
    </>
  );
}

function ProgressBarDemo({ t }: { t: T }) {
  const { number, percent } = useNumber();
  const segments: ProgressSegment[] = [
    { id: 'done', label: t('designSystem.demo.data.segDone'), value: 14, tone: 'success' },
    { id: 'failed', label: t('designSystem.demo.data.segFailed'), value: 1, tone: 'danger' },
    { id: 'running', label: t('designSystem.demo.data.segRunning'), value: 4, tone: 'info' },
    { id: 'waiting', label: t('designSystem.demo.data.segWaiting'), value: 6, tone: 'warning' },
  ];
  const bars = (
    <div className="ds-demo-stack">
      <ProgressBar label={t('designSystem.demo.data.stepsDone')} value={3} max={8} valueText={`${number(3)}/${number(8)}`} />
      <ProgressBar label={t('designSystem.demo.data.tasks')} segments={segments} legend />
    </div>
  );
  const toneLabel = (tone: string) => (tone === 'accent' ? t('designSystem.demo.data.accent') : t(`designSystem.demo.tone.${tone}`));
  return (
    <>
      <WideAndNarrow t={t} wide={bars} narrow={bars} />
      <Row label={t('designSystem.demo.data.determinate')}>
        <div className="ds-demo-stack">
          <ProgressBar label={t('designSystem.demo.data.diskUsed')} value={70} valueText={percent(0.7)} tone="warning" />
          <ProgressBar label={t('designSystem.demo.data.diskUsed')} value={70} tone="warning" size="sm" hideLabel />
        </div>
      </Row>
      <Row label={t('designSystem.demo.data.indeterminate')}>
        <div className="ds-demo-stack"><ProgressBar label={t('designSystem.demo.data.cloning')} /></div>
      </Row>
      <Row label={t('designSystem.demo.data.stacked')}>
        <div className="ds-demo-stack"><ProgressBar label={t('designSystem.demo.data.tasks')} segments={segments} /></div>
      </Row>
      <Row label={t('designSystem.demo.tones')}>
        <div className="ds-demo-stack">
          {PROGRESS_TONES.map(tone => <ProgressBar key={tone} label={toneLabel(tone)} value={60} tone={tone} valueText={percent(0.6)} />)}
        </div>
      </Row>
    </>
  );
}

/** KT-2004 — a log that looks like a run still writing: commands, a warning
 *  on stderr with its ANSI colour codes (removed on screen), a failed test
 *  as an error, times of their own. */
const DEMO_LOG: readonly LogLine[] = [
  { text: '$ pnpm install --frozen-lockfile', time: '2026-11-22T09:14:02Z' },
  { text: 'Lockfile is up to date, resolution step is skipped', time: '2026-11-22T09:14:03Z' },
  { text: '\u001b[33mWARN\u001b[39m deprecated inflight@1.0.6: this module is not supported', stream: 'stderr', time: '2026-11-22T09:14:05Z' },
  { text: 'Packages: +1284', time: '2026-11-22T09:14:11Z' },
  { text: '$ pnpm test -- --run src/components/ui', time: '2026-11-22T09:14:12Z' },
  { text: ' \u001b[32m✓\u001b[39m src/components/ui/Table.test.tsx (34 tests) 812ms', time: '2026-11-22T09:14:19Z' },
  { text: ' ✗ src/components/ui/LogViewer.test.tsx > follow > resumes from the jump control', level: 'error', stream: 'stderr', time: '2026-11-22T09:14:20Z' },
  { text: 'AssertionError: expected 800 to be 120', stream: 'stderr', time: '2026-11-22T09:14:20Z' },
  { text: ' ✓ src/components/ui/DiffViewer.test.tsx (23 tests) 420ms', time: '2026-11-22T09:14:22Z' },
  { text: 'Re-running the failed test…', level: 'info', time: '2026-11-22T09:14:23Z' },
];

/** The demo log is the tail of a longer output: its lines are numbered
 *  where the tail starts, and its note counts the lines shown. */
const DEMO_LOG_TOTAL = 1248;
const DEMO_LOG_START = DEMO_LOG_TOTAL - DEMO_LOG.length + 1;

const LONG_LOG_LINE = 'error[E0308]: mismatched types --> backend/src/workflows/runner.rs:412:27 | expected `Result<RunOutcome, RunError>`, found `Option<RunOutcome>`';

function LogViewerDemo({ t }: { t: T }) {
  const { number } = useNumber();
  const [lines, setLines] = useState<readonly LogLine[]>(DEMO_LOG);
  const label = t('designSystem.demo.log.label');
  const common = {
    label,
    lines,
    streaming: true,
    copyable: true,
    onRefresh: () => setLines(DEMO_LOG),
    onClear: () => setLines([]),
    note: t('designSystem.demo.log.note', number(DEMO_LOG.length)),
  };
  return (
    <>
      <WideAndNarrow
        t={t}
        wide={<LogViewer {...common} lineNumbers startNumber={DEMO_LOG_START} />}
        narrow={<LogViewer {...common} maxHeight="sm" />}
      />
      <Row label={t('designSystem.demo.log.sideways')}>
        <div className="ds-demo-container" data-form="wide">
          <LogViewer label={label} lines={[...DEMO_LOG.slice(0, 2), { text: LONG_LOG_LINE, level: 'error' }]} wrap={false} maxHeight="sm" />
        </div>
      </Row>
      <Row label={t('designSystem.demo.data.empty')}>
        <div className="ds-demo-container" data-form="wide">
          <LogViewer label={label} lines={[]} empty={t('designSystem.demo.log.emptyText')} maxHeight="sm" />
        </div>
      </Row>
    </>
  );
}

/** KT-2004 — two files: a rename with a removed, an added and a changed line
 *  and a meta line; a binary file. */
const DEMO_DIFF: readonly DiffFileModel[] = [
  {
    path: 'src/lib/run.ts',
    previousPath: 'src/lib/execution.ts',
    status: 'renamed',
    added: 3,
    removed: 2,
    hunks: [{
      header: '@@ -12,6 +12,7 @@ export function startRun(task: Task) {',
      lines: [
        { kind: 'context', oldNumber: 12, newNumber: 12, text: '  const started = Date.now();' },
        { kind: 'removed', oldNumber: 13, text: "  const label = 'execution';" },
        { kind: 'added', newNumber: 13, text: "  const label = 'run';" },
        { kind: 'changed', oldNumber: 14, newNumber: 14, text: '  log(label, task.id, started);', otherText: '  log(label, task.id);' },
        { kind: 'context', oldNumber: 15, newNumber: 15, text: '  return dispatch(task);' },
        { kind: 'added', newNumber: 16, text: '  // The worktree is kept for the review.' },
        { kind: 'context', oldNumber: 16, newNumber: 17, text: '}' },
        { kind: 'meta', text: '\\ No newline at end of file' },
      ],
    }],
  },
  { path: 'assets/logo.png', status: 'modified', binary: true, hunks: [] },
];

const highlightDemo = (text: string, path: string) => highlightLine(text, languageForPath(path));

function DiffViewerDemo({ t }: { t: T }) {
  const { number } = useNumber();
  const [selection, setSelection] = useState<DiffSelection | null>({ path: 'src/lib/run.ts', start: 1, end: 3 });
  const label = t('designSystem.demo.diff.label');
  const count = selection ? selection.end - selection.start + 1 : 0;
  return (
    <>
      <WideAndNarrow
        t={t}
        wide={(
          <DiffViewer
            files={DEMO_DIFF}
            label={label}
            layout="split"
            highlight={highlightDemo}
            selectable
            selection={selection}
            onSelectionChange={setSelection}
            selectionToolbar={<Button size="sm">{t('designSystem.demo.diff.comment')}</Button>}
          />
        )}
        narrow={<DiffViewer files={DEMO_DIFF} label={label} layout="split" highlight={highlightDemo} maxHeight="sm" />}
      />
      <p className="ds-demo-note" aria-live="polite" data-testid="ds-diff-selection">
        {selection ? t('designSystem.demo.diff.selected', number(count)) : t('designSystem.demo.diff.selectedNone')}
      </p>
      <Row label={t('designSystem.demo.data.loading')}>
        <div className="ds-demo-container" data-form="wide">
          <DiffViewer files={[]} label={label} loading loadingLabel={t('designSystem.demo.diff.loadingDiff')} />
        </div>
      </Row>
      <Row label={t('designSystem.demo.data.empty')}>
        <div className="ds-demo-container" data-form="wide">
          <DiffViewer files={[]} label={label} />
        </div>
      </Row>
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
  copyButton: CopyButtonDemo,
  table: TableDemo,
  keyValueList: KeyValueListDemo,
  statTile: StatTileDemo,
  progressBar: ProgressBarDemo,
  logViewer: LogViewerDemo,
  diffViewer: DiffViewerDemo,
  agentIdentity: AgentIdentityDemo,
  agentSwitchPicker: AgentSwitchPickerDemo,
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
