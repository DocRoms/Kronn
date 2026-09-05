// KT-581 — inviting a peer, split out from the participants row.
//
// The button and the chip strip used to be one component on one line. The
// header then read as three rows: title, participants, counters. Separating
// them lets the invite sit on the title row, where an action on the whole
// discussion belongs, and leaves the chips to the row below.
//
// The one-shot token and its modal live here rather than in the row: they are
// what the button does, and nothing in the chip strip reads them.
import { useState } from 'react';
import { discussions as discussionsApi } from '../lib/api';
import type { ToastFn } from '../hooks/useToast';
import { UserPlus, Copy, X } from 'lucide-react';

export interface DiscInviteButtonProps {
  discId: string;
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
  /** Lets the participants row re-fetch: a peer invited here shows up there. */
  onInvited?: () => void;
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
  // KT-52 — the enriched handoff is the default: an invited agent that reads
  // only the pasted line still learns to read the plan and to stay. The bare
  // call stays one click away for a human who just wants the token.
  const [handoffMinimal, setHandoffMinimal] = useState(false);

  const handleInvite = async () => {
    if (inviting) return;
    setInviting(true);
    try {
      const r = await discussionsApi.invitePeer(discId);
      setInvite({
        token: r.token,
        instruction: r.instruction_text,
        instructionMinimal: r.instruction_text_minimal,
        expiresAt: r.expires_at,
        ttlSecs: r.ttl_seconds,
      });
      setHandoffMinimal(false);
      setShowModal(true);
      // Refresh in case a previous peer just left.
      onInvited?.();
    } catch (e) {
      toast(t('disc.inviteFailed', String(e)), 'error');
    } finally {
      setInviting(false);
    }
  };

  const shownHandoff = invite
    ? (handoffMinimal ? invite.instructionMinimal : invite.instruction)
    : '';

  const handleCopy = async () => {
    if (!invite) return;
    try {
      await navigator.clipboard.writeText(shownHandoff);
      toast(t('disc.inviteCopied'), 'success');
    } catch {
      toast(t('disc.inviteCopyFailed'), 'error');
    }
  };

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
        >
          <div className="disc-invite-modal">
            <div className="disc-invite-modal-header">
              <h3>{t('disc.inviteModalTitle')}</h3>
              <button
                type="button"
                className="disc-invite-modal-close"
                onClick={() => setShowModal(false)}
                aria-label={t('disc.inviteModalClose')}
              >
                <X size={14} />
              </button>
            </div>
            <p className="disc-invite-modal-intro">
              {t('disc.inviteModalIntro', Math.floor(invite.ttlSecs / 60))}
            </p>
            <pre className="disc-invite-instruction" data-testid="disc-invite-instruction">
              {shownHandoff}
            </pre>
            <div className="disc-invite-modal-actions">
              <label className="disc-invite-handoff-toggle">
                <input
                  type="checkbox"
                  checked={!handoffMinimal}
                  onChange={e => setHandoffMinimal(!e.target.checked)}
                  data-testid="disc-invite-handoff-toggle"
                />
                {t('disc.inviteHandoffFull')}
              </label>
              <button
                type="button"
                className="disc-invite-copy-btn"
                onClick={handleCopy}
              >
                <Copy size={11} /> {t('disc.inviteCopyBtn')}
              </button>
            </div>
            <p className="disc-invite-handoff-hint">
              {t(handoffMinimal ? 'disc.inviteHandoffMinimalHint' : 'disc.inviteHandoffFullHint')}
            </p>
            <p className="disc-invite-expires-hint">
              {t('disc.inviteExpiresHint', invite.expiresAt)}
            </p>
          </div>
        </div>
      )}
    </>
  );
}
