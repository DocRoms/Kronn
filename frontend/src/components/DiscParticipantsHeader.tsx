import { useState, useEffect, useCallback } from 'react';
import { discussions as discussionsApi } from '../lib/api';
import { freshnessOf, DEFAULT_AWAY_AFTER_MS, AWAY_MARGIN_MS } from '../lib/discPresence';
import type { Freshness } from '../lib/discPresence';
import type { ToastFn } from '../hooks/useToast';
import { UserPlus, Copy, X } from 'lucide-react';

/// 0.8.6 phase 2 — discussion participants header.
///
/// Renders the live list of CLI sessions bound to this disc (one row
/// per active+paused `discussion_sessions`) + the `[+ Inviter]` button
/// that opens a modal with a one-shot token. Companion to the
/// `disc_join` MCP tool — the user copy-pastes the token into another
/// CLI terminal and that CLI joins the same disc.
///
/// Lifecycle :
///   * fetch on mount + when `discId` changes
///   * re-fetch every 5s to catch peer join/leave without SSE
///   * re-fetch after every successful invite
///
/// Styling : all rules live in `styles/components.css` so the modal
/// inherits the active Kronn theme (dark/light/neon). The earlier
/// inline-style version was rendering black-on-black because no
/// `color` was set and one of the invented `--kr-*` tokens didn't
/// exist (cf. memory `feedback_css_tokens.md`).

export interface DiscParticipantsHeaderProps {
  discId: string;
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

// Light shape of the wire response (mirrors the Rust struct in
// `backend/src/db/discussion_sessions.rs::DiscussionSession`).
interface ParticipantRow {
  id: number;
  agent_type: string;
  session_id: string | null;
  role: string;
  status: string;
  last_seen?: string | null;
}

// Presence thresholds live in `lib/discPresence.ts` (pure, unit-tested);
// the away cap follows the server's poll_policy fetched from the disc meta.
const FRESH_COLOR: Record<Freshness, string> = {
  fresh: 'var(--kr-success)',
  idle: 'var(--kr-warning)',
  away: 'var(--kr-text-tertiary, #888)',
};
// User-visible labels go through t(...) — keys per freshness state.
const FRESH_LABEL_KEY: Record<Freshness, string> = {
  fresh: 'disc.presenceFresh',
  idle: 'disc.presenceIdle',
  away: 'disc.presenceAway',
};

// Agent-type → emoji icon. Distinct glyphs per CLI but none of them
// should LOOK like a status indicator (green circle = "online" etc.) —
// that's what `data-status` is for. All icons are neutral / brand-y.
const AGENT_ICON: Record<string, string> = {
  ClaudeCode: '🤖',
  Codex: '💠',
  GeminiCli: '✨',
  Kiro: '🐙',
  CopilotCli: '💻',
  Vibe: '🐱',
  Ollama: '🦙',
  Custom: '⚙️',
  Unknown: '👤',
};

const iconFor = (agentType: string) => AGENT_ICON[agentType] ?? '👤';

export function DiscParticipantsHeader({ discId, toast, t }: DiscParticipantsHeaderProps) {
  const [participants, setParticipants] = useState<ParticipantRow[]>([]);
  const [awayAfterMs, setAwayAfterMs] = useState(DEFAULT_AWAY_AFTER_MS);
  const [inviting, setInviting] = useState(false);
  const [showModal, setShowModal] = useState(false);
  const [invite, setInvite] = useState<{ token: string; instruction: string; expiresAt: string; ttlSecs: number } | null>(null);

  const fetchParticipants = useCallback(async () => {
    try {
      const list = await discussionsApi.participants(discId);
      setParticipants(list);
    } catch (e) {
      // Don't toast for fetch failures — the header just stays empty,
      // less noisy than a popup every time the user opens a disc.
      console.warn('[DiscParticipantsHeader] participants fetch failed:', e);
    }
  }, [discId]);

  useEffect(() => {
    // The away threshold follows the server's poll policy — a single meta
    // fetch per disc; on ANY failure the fallback constant stays in place.
    let cancelled = false;
    (async () => {
      try {
        const m = await discussionsApi.meta(discId);
        const maxDelaySeconds = m.poll_policy?.max_delay_seconds;
        if (!cancelled && typeof maxDelaySeconds === 'number') {
          setAwayAfterMs(maxDelaySeconds * 1000 + AWAY_MARGIN_MS);
        }
      } catch (e) {
        console.warn('[DiscParticipantsHeader] meta fetch failed:', e);
      }
    })();
    return () => { cancelled = true; };
  }, [discId]);

  useEffect(() => {
    fetchParticipants();
    // 0.8.6 phase 3 — light polling refresh (5s) so peer join/leave
    // events show up in the header without manual refresh. Cheap : a
    // SELECT on a single indexed column. Will be replaced by SSE in a
    // later wave (`DiscPeerJoined` / `DiscPeerLeft` events plumbed
    // through the existing ws_broadcast).
    const id = setInterval(fetchParticipants, 5000);
    return () => clearInterval(id);
  }, [fetchParticipants]);

  const handleInvite = async () => {
    if (inviting) return;
    setInviting(true);
    try {
      const r = await discussionsApi.invitePeer(discId);
      setInvite({
        token: r.token,
        instruction: r.instruction_text,
        expiresAt: r.expires_at,
        ttlSecs: r.ttl_seconds,
      });
      setShowModal(true);
      // Refresh in case a previous peer just left.
      fetchParticipants();
    } catch (e) {
      toast(t('disc.inviteFailed', String(e)), 'error');
    } finally {
      setInviting(false);
    }
  };

  const handleCopy = async () => {
    if (!invite) return;
    try {
      await navigator.clipboard.writeText(invite.instruction);
      toast(t('disc.inviteCopied'), 'success');
    } catch {
      toast(t('disc.inviteCopyFailed'), 'error');
    }
  };

  return (
    <div className="disc-participants-row" data-testid="disc-participants-row">
      {participants.length === 0 && (
        <span className="disc-participants-empty">
          {t('disc.participantsEmpty')}
        </span>
      )}
      {participants.map(p => {
        const f = freshnessOf(p.last_seen, awayAfterMs);
        return (
          <span
            key={p.id}
            className="disc-participant-chip"
            data-status={p.status}
            data-role={p.role}
            data-freshness={f}
            title={`${p.agent_type} (${p.role}) — ${t(FRESH_LABEL_KEY[f])}`}
          >
            <span
              aria-hidden
              style={{ width: 6, height: 6, borderRadius: '50%', background: FRESH_COLOR[f], display: 'inline-block', flexShrink: 0 }}
            />
            <span aria-hidden>{iconFor(p.agent_type)}</span>
            <span>{p.agent_type}</span>
          </span>
        );
      })}
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
            <pre className="disc-invite-instruction">
              {invite.instruction}
            </pre>
            <div className="disc-invite-modal-actions">
              <button
                type="button"
                className="disc-invite-copy-btn"
                onClick={handleCopy}
              >
                <Copy size={11} /> {t('disc.inviteCopyBtn')}
              </button>
            </div>
            <p className="disc-invite-expires-hint">
              {t('disc.inviteExpiresHint', invite.expiresAt)}
            </p>
          </div>
        </div>
      )}
    </div>
  );
}
