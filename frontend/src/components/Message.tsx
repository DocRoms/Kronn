import {
  useContext, useEffect, useId, useLayoutEffect, useRef, useState,
  type CSSProperties, type ReactNode,
} from 'react';
import { ChevronDown, ChevronUp } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { Button } from './ui/Button';
import { IconButton } from './ui/IconButton';
import { Menu, type MenuItem } from './ui/Menu';
import {
  MESSAGE_FOLD_THRESHOLD_PX, MessageCompactContext, MessageFoldContext, orderedActions,
  type MessageActionSet, type MessageVariant,
} from './messageKinds';
import './Message.css';

/** The side and frame a variant takes. Streaming is an agent still writing,
 *  queued a user message not sent yet, an error a system message. */
const ROLE_OF_VARIANT: Record<MessageVariant, string> = {
  user: 'user',
  agent: 'agent',
  system: 'system',
  error: 'system',
  orchestrator: 'orchestrator',
  streaming: 'agent',
  queued: 'user',
  note: 'note',
  deleted: 'deleted',
  media: 'media',
};

/** The footer actions, in their fixed order: icons with a name, or, under
 *  the first breakpoint, one « … » menu holding them (Delete last). */
export function MessageActions({
  actions,
  compact,
  testId,
}: {
  actions: MessageActionSet;
  compact?: boolean;
  testId?: string;
}) {
  const { t } = useT();
  const fromContext = useContext(MessageCompactContext);
  const inMenu = compact ?? fromContext;
  const present = orderedActions(actions);
  if (present.length === 0) return null;
  if (inMenu) {
    const items: MenuItem[] = [];
    let destructive: MenuItem | undefined;
    for (const [kind, action] of present) {
      const item = { id: kind, label: action.label, icon: action.icon, onSelect: action.onSelect, disabled: action.disabled };
      if (kind === 'delete') destructive = item;
      else items.push(item);
    }
    return (
      <div className="kr-message-actions" data-compact="true" data-testid={testId}>
        <Menu
          label={t('disc.message.actions')}
          size="sm"
          items={items}
          destructiveItem={destructive}
          triggerTestId="message-actions-menu"
        />
      </div>
    );
  }
  return (
    <div className="kr-message-actions" role="group" aria-label={t('disc.message.actions')} data-testid={testId}>
      {present.map(([kind, action]) => (
        <IconButton
          key={kind}
          size="sm"
          label={action.label}
          icon={action.icon}
          onClick={action.onSelect}
          disabled={action.disabled}
          className={['kr-message-action', action.className].filter(Boolean).join(' ')}
          data-action={kind}
          data-state={action.state}
          data-testid={action.testId}
        />
      ))}
    </div>
  );
}

// One observer for every message: a long thread holds hundreds of them.
type Measure = () => void;
const measures = new WeakMap<Element, Measure>();
let sharedObserver: ResizeObserver | null = null;
function observeSize(element: Element, measure: Measure): () => void {
  if (typeof ResizeObserver !== 'function') return () => {};
  sharedObserver ??= new ResizeObserver(entries => {
    for (const entry of entries) measures.get(entry.target)?.();
  });
  measures.set(element, measure);
  sharedObserver.observe(element);
  return () => {
    measures.delete(element);
    sharedObserver?.unobserve(element);
  };
}

interface FoldOptions {
  /** Same id across renders: the thread remembers what was unfolded. */
  id: string;
  /** Unfold now and keep it so: a search hit or an anchor jump landed here. */
  reveal?: boolean;
  /** A message still being written or edited never folds. */
  disabled?: boolean;
}

function MessageBody({ fold, children }: { fold?: FoldOptions; children: ReactNode }) {
  const { t } = useT();
  const bodyId = useId();
  const store = useContext(MessageFoldContext);
  const innerRef = useRef<HTMLDivElement>(null);
  const foldId = fold?.id;
  const enabled = Boolean(fold) && !fold?.disabled;
  const reveal = fold?.reveal === true;
  const [open, setOpen] = useState(() => (foldId ? store?.has(foldId) ?? false : false));
  const [tall, setTall] = useState(false);
  // A search hit or a jump landing here opens it, and it stays open after.
  if (reveal && !open) setOpen(true);

  useLayoutEffect(() => {
    const inner = innerRef.current;
    if (!enabled || !inner) return;
    const measure = () => setTall(inner.scrollHeight > MESSAGE_FOLD_THRESHOLD_PX);
    measure();
    return observeSize(inner, measure);
  }, [enabled]);

  useEffect(() => {
    if (reveal && foldId) store?.add(foldId);
  }, [reveal, foldId, store]);

  if (!fold) return <>{children}</>;
  const foldable = enabled && tall;
  const toggle = () => {
    const next = !open;
    setOpen(next);
    if (foldId && store) {
      if (next) store.add(foldId);
      else store.delete(foldId);
    }
  };
  return (
    <>
      <div className="kr-message-body" id={bodyId} data-folded={(foldable && !open) || undefined}>
        <div ref={innerRef}>{children}</div>
      </div>
      {foldable && (
        <Button
          size="sm"
          variant="ghost"
          className="kr-message-fold-toggle"
          aria-expanded={open}
          aria-controls={bodyId}
          icon={open ? <ChevronUp /> : <ChevronDown />}
          onClick={toggle}
        >
          {open ? t('disc.message.showLess') : t('disc.message.showMore')}
        </Button>
      )}
    </>
  );
}

type DataAttributes = { [key: `data-${string}`]: string | boolean | undefined };

export interface MessageProps {
  variant: MessageVariant;
  /** A finer kind for the stylesheet (`data-variant`): summary, kronn-tool… */
  tone?: string;
  /** Attributes of the row: `data-message-id`, `data-testid`, `aria-live`… */
  rowProps?: DataAttributes & { 'aria-live'?: 'polite' | 'assertive'; 'aria-label'?: string; role?: string };
  bubbleClassName?: string;
  /** Dynamic only: an agent's colour. */
  bubbleStyle?: CSSProperties;
  editing?: boolean;
  /** Author, agent and model, written once. */
  header?: ReactNode;
  /** Controls of the header (a Stop, a details toggle, the id pill). */
  headerActions?: ReactNode;
  /** The time, at the end of the header. */
  time?: { label: string; dateTime: string };
  /** Before the body: what it answers, an expanded summary. */
  lead?: ReactNode;
  /** The body: the only part that folds. */
  children?: ReactNode;
  /** After the body, before the footer: attachments, cards, calls to act. */
  trail?: ReactNode;
  /** Model, duration, tokens and the other facts about the message. */
  footerInfo?: ReactNode;
  actions?: MessageActionSet;
  fold?: FoldOptions;
  /** Below the footer: a detail opened from it (the citation report). */
  after?: ReactNode;
}

/** KT-1902 — a discussion message: header (author, agent and model once,
 *  time), body, footer (the facts on the left, the actions on the right in a
 *  fixed order, in the « … » menu under the first breakpoint). Every bubble of
 *  the discussion page is this component, in one of its variants. */
export function Message({
  variant, tone, rowProps, bubbleClassName, bubbleStyle, editing, header, headerActions, time, lead, children, trail,
  footerInfo, actions, fold, after,
}: MessageProps) {
  const role = ROLE_OF_VARIANT[variant];
  const hasActions = actions !== undefined && orderedActions(actions).length > 0;
  const hasHeader = header !== undefined || headerActions !== undefined || time !== undefined;
  return (
    <div className="disc-msg-row" data-role={role} data-message-variant={variant} {...rowProps}>
      <div
        className={['disc-msg-bubble', bubbleClassName].filter(Boolean).join(' ')}
        data-role={role}
        data-variant={tone}
        data-editing={editing || undefined}
        style={bubbleStyle}
      >
        {hasHeader && (
          <div className="disc-msg-header-row">
            <div className="disc-msg-header-main">{header}</div>
            {(headerActions !== undefined || time) && (
              <div className="disc-msg-header-actions">
                {headerActions}
                {time && <time className="disc-msg-time" dateTime={time.dateTime}>{time.label}</time>}
              </div>
            )}
          </div>
        )}
        {lead}
        <MessageBody fold={fold}>{children}</MessageBody>
        {trail}
        {(footerInfo !== undefined || hasActions) && (
          <div className="disc-msg-footer">
            <div className="disc-msg-footer-info">{footerInfo}</div>
            {hasActions && actions && <MessageActions actions={actions} />}
          </div>
        )}
        {after}
      </div>
    </div>
  );
}
