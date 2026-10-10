// KT-581 — inviting a peer, split out from the participants row.
//
// The button and the chip strip used to be one component on one line. The
// header then read as three rows: title, participants, counters. Separating
// them lets the invite sit on the title row, where an action on the whole
// discussion belongs, and leaves the chips to the row below.
//
// The one-shot token and its modal live here rather than in the row: they are
// what the button does, and nothing in the chip strip reads them.
import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { discussions as discussionsApi } from '../lib/api';
import { browserCopyEnv, copyAsset } from '../lib/copyAsset';
import type { ToastFn } from '../hooks/useToast';
import { UserPlus, Copy, X } from 'lucide-react';

export interface DiscInviteButtonProps {
  discId: string;
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
  /** Lets the participants row re-fetch: a peer invited here shows up there. */
  onInvited?: () => void;
}

type InviteTab = 'token' | 'simple' | 'detailed';
const TABS: InviteTab[] = ['token', 'simple', 'detailed'];

const TAB_LABEL: Record<InviteTab, string> = {
  token: 'disc.inviteTabToken',
  simple: 'disc.inviteTabSimple',
  detailed: 'disc.inviteTabDetailed',
};
const TAB_HINT: Record<InviteTab, string> = {
  token: 'disc.inviteTokenHint',
  simple: 'disc.inviteHandoffMinimalHint',
  detailed: 'disc.inviteHandoffFullHint',
};
const TAB_COPY: Record<InviteTab, string> = {
  token: 'disc.inviteCopyTokenBtn',
  simple: 'disc.inviteCopyBtn',
  detailed: 'disc.inviteCopyBtn',
};

interface CopyState { tab: InviteTab; ok: boolean }

async function writeClipboard(text: string): Promise<boolean> {
  try {
    if (!navigator.clipboard?.writeText) return false;
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

export function DiscInviteButton({ discId, toast, t, onInvited }: DiscInviteButtonProps) {
  const [inviting, setInviting] = useState(false);
  const [showModal, setShowModal] = useState(false);
  const [invite, setInvite] = useState<{
    token: string;
    instruction: string;
    instructionMinimal: string;
    expiresAt: string;
    ttlSecs: number;
  } | null>(null);
  // KT-1146 — the bare token is what is usually needed, so it is copied on
  // click and its tab is the default; the handoffs stay one tab away.
  const [tab, setTab] = useState<InviteTab>('token');
  const [copy, setCopy] = useState<CopyState | null>(null);
  const preRef = useRef<HTMLPreElement>(null);
  const tabRefs = useRef<Record<InviteTab, HTMLButtonElement | null>>({ token: null, simple: null, detailed: null });

  const textFor = (which: InviteTab) => {
    if (!invite) return '';
    if (which === 'token') return invite.token;
    return which === 'simple' ? invite.instructionMinimal : invite.instruction;
  };

  // A refused clipboard is never reported as a copy: the text stays selected
  // so the human can copy it by hand.
  useEffect(() => {
    if (!showModal || !copy || copy.ok || !preRef.current) return;
    const sel = window.getSelection();
    sel?.removeAllRanges();
    sel?.selectAllChildren(preRef.current);
  }, [showModal, copy]);

  const handleInvite = async () => {
    if (inviting) return;
    setInviting(true);
    const mint = discussionsApi.invitePeer(discId);
    // WebKit (the desktop webview) only honours a write started inside the
    // click, so the clipboard item is handed over now and resolves with the mint.
    const copied = copyAsset(
      'text', 'text/plain',
      () => mint.then(r => new Blob([r.token], { type: 'text/plain' })),
      browserCopyEnv(),
    ).then(() => true, () => false);
    try {
      const r = await mint;
      const ok = (await copied) || (await writeClipboard(r.token));
      setInvite({
        token: r.token,
        instruction: r.instruction_text,
        instructionMinimal: r.instruction_text_minimal,
        expiresAt: r.expires_at,
        ttlSecs: r.ttl_seconds,
      });
      setTab('token');
      setCopy({ tab: 'token', ok });
      setShowModal(true);
      // Refresh in case a previous peer just left.
      onInvited?.();
    } catch (e) {
      toast(t('disc.inviteFailed', String(e)), 'error');
    } finally {
      setInviting(false);
    }
  };

  const handleCopy = async () => {
    if (!invite) return;
    const which = tab;
    setCopy({ tab: which, ok: await writeClipboard(textFor(which)) });
  };

  const selectTab = (next: InviteTab) => {
    setTab(next);
    // A failure notice is about the text that was shown; a success still holds.
    setCopy(c => (c && !c.ok ? null : c));
  };

  const onTabKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
    const i = TABS.indexOf(tab);
    let next: InviteTab | null = null;
    if (e.key === 'ArrowRight') next = TABS[(i + 1) % TABS.length];
    else if (e.key === 'ArrowLeft') next = TABS[(i - 1 + TABS.length) % TABS.length];
    else if (e.key === 'Home') next = TABS[0];
    else if (e.key === 'End') next = TABS[TABS.length - 1];
    if (!next) return;
    e.preventDefault();
    selectTab(next);
    tabRefs.current[next]?.focus();
  };

  const notice = copy
    ? copy.ok
      ? t(copy.tab === 'token' ? 'disc.inviteTokenCopied' : 'disc.inviteCopied')
      : t('disc.inviteCopyManual')
    : null;

  return (
    <>
      <button
        type="button"
        className="disc-participants-invite-btn"
        onClick={handleInvite}
        disabled={inviting}
        title={t('disc.invitePeerTooltip')}
        aria-label={t('disc.invitePeerTooltip')}
      >
        <UserPlus size={11} />
        {t('disc.invitePeer')}
      </button>

      {showModal && invite && (
        <div
          className="disc-invite-modal-overlay"
          onClick={e => { if (e.target === e.currentTarget) setShowModal(false); }}
          role="dialog"
          aria-modal="true"
          aria-labelledby="disc-invite-modal-title"
        >
          <div className="disc-invite-modal">
            <div className="disc-invite-modal-header">
              <h3 id="disc-invite-modal-title">{t('disc.inviteModalTitle')}</h3>
              <button
                type="button"
                className="disc-invite-modal-close"
                onClick={() => setShowModal(false)}
                aria-label={t('disc.inviteModalClose')}
              >
                <X size={14} />
              </button>
            </div>
            {notice && (
              <p
                className="disc-invite-notice"
                data-ok={copy?.ok ? 'true' : 'false'}
                role={copy?.ok ? 'status' : 'alert'}
                data-testid="disc-invite-notice"
              >
                {notice}
              </p>
            )}
            <p className="disc-invite-modal-intro">
              {t('disc.inviteModalIntro', Math.floor(invite.ttlSecs / 60))}
            </p>
            <div className="disc-invite-tabs" role="tablist" aria-label={t('disc.inviteTabs')}>
              {TABS.map(id => (
                <button
                  key={id}
                  ref={el => { tabRefs.current[id] = el; }}
                  type="button"
                  role="tab"
                  id={`disc-invite-tab-${id}`}
                  aria-selected={tab === id}
                  aria-controls="disc-invite-panel"
                  tabIndex={tab === id ? 0 : -1}
                  data-active={tab === id}
                  data-testid={`disc-invite-tab-${id}`}
                  onClick={() => selectTab(id)}
                  onKeyDown={onTabKeyDown}
                >
                  {t(TAB_LABEL[id])}
                </button>
              ))}
            </div>
            <div
              className="disc-invite-panel"
              role="tabpanel"
              id="disc-invite-panel"
              aria-labelledby={`disc-invite-tab-${tab}`}
            >
              <pre ref={preRef} className="disc-invite-instruction" data-testid="disc-invite-instruction">
                {textFor(tab)}
              </pre>
              <div className="disc-invite-modal-actions">
                <p className="disc-invite-handoff-hint">{t(TAB_HINT[tab])}</p>
                <button
                  type="button"
                  className="disc-invite-copy-btn"
                  data-testid={`disc-invite-copy-${tab}`}
                  onClick={handleCopy}
                >
                  <Copy size={11} /> {t(TAB_COPY[tab])}
                </button>
              </div>
            </div>
            <p className="disc-invite-expires-hint">
              {t('disc.inviteExpiresHint', invite.expiresAt)}
            </p>
          </div>
        </div>
      )}
    </>
  );
}
