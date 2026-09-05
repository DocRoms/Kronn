import { useState, useEffect, useCallback } from 'react';
import { discussions as discussionsApi } from '../lib/api';
import {
  honestPresenceState,
  freshnessForPresence,
  secondsUntil,
  DEFAULT_AWAY_AFTER_MS,
  AWAY_MARGIN_MS,
} from '../lib/discPresence';
import type { HonestPresenceState } from '../lib/discPresence';
import { Loader2, X } from 'lucide-react';
import { CopyIdPill } from './CopyIdPill';
import { AGENT_MENTIONS } from '../lib/constants';

/// 0.8.6 phase 2 — discussion participants header.
///
/// Renders the live list of CLI sessions bound to this disc (one row
/// per active+paused `discussion_sessions`). The `[+ Inviter]` button that
/// mints the join token is `DiscInviteButton`, on the title row: an action on
/// the whole discussion, not a member of the chip strip.
///
/// Lifecycle :
///   * fetch on mount + when `discId` changes
///   * re-fetch every 5s to catch peer join/leave without SSE
///   * re-fetch when `refreshKey` changes — an invite just succeeded
///
/// Styling : all rules live in `styles/components.css` so the modal
/// inherits the active Kronn theme (dark/light/neon). The earlier
/// inline-style version was rendering black-on-black because no
/// `color` was set and one of the invented `--kr-*` tokens didn't
/// exist (cf. memory `feedback_css_tokens.md`).

export interface DiscParticipantsHeaderProps {
  discId: string;
  t: (key: string, ...args: (string | number)[]) => string;
  /** Bumped by the invite button so a fresh peer appears without waiting out
   *  the 5s poll. */
  refreshKey?: number;
}

// Light shape of the wire response (mirrors the Rust struct in
// `backend/src/db/discussion_sessions.rs::DiscussionSession`).
interface ParticipantRow {
  id: number;
  agent_type: string;
  /// Optional self-declared model captured at JOIN time. This is durable
  /// metadata, not a live probe: the UI labels it explicitly as such.
  model?: string | null;
  conversation_id?: string | null;
  session_id: string | null;
  role: string;
  status: string;
  last_seen?: string | null;
  /// 0.8.12 PR B — server-derived: 'listening' (open wait long-poll) or
  /// 'reading' (messages delivered, no reply yet). Expiry is applied
  /// server-side at read time — a value here is always current.
  activity?: string | null;
  presence_state?: HonestPresenceState | null;
  read_live?: boolean;
  write_state?: 'ok' | 'failed' | 'unknown';
  wake_mode?: 'native_dispatch' | 'external_poll';
  next_poll_at?: string | null;
  last_write_at?: string | null;
  resume_reason?: string | null;
  resume_since?: string | null;
}

// Presence thresholds live in `lib/discPresence.ts` (pure, unit-tested);
// the away cap follows the server's poll_policy fetched from the disc meta.
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

function resumeCommandFor(
  agentType: string,
  conversationId?: string | null,
): string | null {
  if (!conversationId) return null;
  if (agentType === 'ClaudeCode') return `claude --resume ${conversationId}`;
  if (agentType === 'Codex') return `codex resume ${conversationId}`;
  return null;
}

function presenceLabel(
  participant: ParticipantRow,
  state: HonestPresenceState,
  t: DiscParticipantsHeaderProps['t'],
): string {
  if (participant.status === 'paused') return t('disc.presencePaused');
  if (state === 'running') return t('disc.presenceRunning');
  if (state === 'resume_expected') return t('disc.presenceResumeExpected');
  if (state === 'stalled') return t('disc.presenceStalled');
  if (state === 'quota_exhausted') return t('disc.presenceQuotaExhausted');
  if (state === 'listening') {
    return participant.activity === 'reading'
      ? t('disc.activityReading')
      : t('disc.presenceListening');
  }
  if (state === 'offline') return t('disc.presenceOffline');
  if (participant.wake_mode === 'native_dispatch') {
    return t('disc.presenceAwaitingRuntime');
  }

  const delay = secondsUntil(participant.next_poll_at);
  if (delay === null || delay === 0) return t('disc.presenceDormant');
  if (delay < 60) return t('disc.presenceDormantSeconds', delay);
  return t('disc.presenceDormantMinutes', Math.ceil(delay / 60));
}

export function DiscParticipantsHeader({ discId, t, refreshKey = 0 }: DiscParticipantsHeaderProps) {
  const [participants, setParticipants] = useState<ParticipantRow[]>([]);
  const [awayAfterMs, setAwayAfterMs] = useState(DEFAULT_AWAY_AFTER_MS);
  const [selectedParticipantId, setSelectedParticipantId] = useState<number | null>(null);

  const applyParticipants = useCallback((list: ParticipantRow[]) => {
    setParticipants(list);
    setSelectedParticipantId(current => (
      current !== null && list.some(participant => participant.id === current)
        ? current
        : null
    ));
  }, []);

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
    let active = true;
    const pollParticipants = async () => {
      try {
        const list = await discussionsApi.participants(discId);
        if (active) applyParticipants(list);
      } catch (error) {
        if (active) {
          console.warn('[DiscParticipantsHeader] participants fetch failed:', error);
        }
      }
    };
    void pollParticipants();
    // 0.8.6 phase 3 — light polling refresh (5s) so peer join/leave
    // events show up in the header without manual refresh. Cheap : a
    // SELECT on a single indexed column. Will be replaced by SSE in a
    // later wave (`DiscPeerJoined` / `DiscPeerLeft` events plumbed
    // through the existing ws_broadcast).
    const id = setInterval(() => { void pollParticipants(); }, 5000);
    return () => {
      active = false;
      clearInterval(id);
    };
    // refreshKey re-arms the effect, and arming it fetches: an invite shows its
    // peer without waiting out the interval.
  }, [applyParticipants, discId, refreshKey]);

  const selectedParticipant = participants.find(
    participant => participant.id === selectedParticipantId,
  ) ?? null;
  const selectedPresence = selectedParticipant
    ? honestPresenceState(
        selectedParticipant.presence_state,
        selectedParticipant.status,
        selectedParticipant.activity,
        selectedParticipant.last_seen,
        awayAfterMs,
      )
    : null;
  const selectedPresenceLabel = selectedParticipant && selectedPresence
    ? presenceLabel(selectedParticipant, selectedPresence, t)
    : null;
  const selectedResumeCommand = selectedParticipant
    ? resumeCommandFor(selectedParticipant.agent_type, selectedParticipant.conversation_id)
    : null;
  const selectedDisplayName = selectedParticipant
    ? AGENT_MENTIONS.find(mention => mention.type === selectedParticipant.agent_type)?.trigger
      ?? selectedParticipant.agent_type
    : null;

  return (
    <div className="disc-participants-row" data-testid="disc-participants-row">
      <div className="disc-participants-list">
        {participants.length === 0 && (
          <span className="disc-participants-empty">
            {t('disc.participantsEmpty')}
          </span>
        )}
        {participants.map(p => {
          const presence = honestPresenceState(
            p.presence_state,
            p.status,
            p.activity,
            p.last_seen,
            awayAfterMs,
          );
          const freshness = freshnessForPresence(presence);
          const label = presenceLabel(p, presence, t);
          const readLive = p.read_live ?? presence === 'listening';
          const writeState = p.write_state ?? 'unknown';
          return (
            <button
              type="button"
              key={p.id}
              className="disc-participant-chip"
              data-status={p.status}
              data-role={p.role}
              data-presence={presence}
              data-freshness={freshness}
              data-read-live={readLive}
              data-write-state={writeState}
              data-wake-mode={p.wake_mode}
              aria-expanded={selectedParticipantId === p.id}
              aria-haspopup="dialog"
              aria-controls={selectedParticipantId === p.id ? `disc-participant-details-${p.id}` : undefined}
              aria-label={t(
                'disc.participantDetails',
                AGENT_MENTIONS.find(mention => mention.type === p.agent_type)?.trigger
                  ?? p.agent_type,
              )}
              onClick={() => setSelectedParticipantId(current => current === p.id ? null : p.id)}
              title={[
                `${p.agent_type} (${p.role}) — ${label}`,
                p.model ? t('disc.modelDeclaredAtJoin', p.model) : null,
              ].filter(Boolean).join(' · ')}
            >
              {presence === 'running' ? (
                <Loader2
                  aria-hidden
                  size={10}
                  className="disc-participant-running spin"
                />
              ) : (
                <span
                  aria-hidden
                  className="disc-participant-presence-dot"
                  data-presence={presence}
                />
              )}
              <span aria-hidden>{iconFor(p.agent_type)}</span>
              <span className="disc-participant-name">
                {AGENT_MENTIONS.find(mention => mention.type === p.agent_type)?.trigger
                  ?? p.agent_type}
              </span>
              {writeState === 'failed' && (
                <span className="disc-participant-write-failed-dot" aria-label={t('disc.writeFailed')} />
              )}
            </button>
          );
        })}
      </div>
      {selectedParticipant && selectedDisplayName && selectedPresenceLabel && (
          <section
            id={`disc-participant-details-${selectedParticipant.id}`}
            className="disc-participant-details"
            role="dialog"
            aria-label={t('disc.participantDetails', selectedDisplayName)}
            data-testid="disc-participant-details"
          >
            <header>
              <strong><span aria-hidden>{iconFor(selectedParticipant.agent_type)}</span> {selectedDisplayName}</strong>
              <button
                type="button"
                onClick={() => setSelectedParticipantId(null)}
                aria-label={t('common.close')}
              >
                <X size={12} />
              </button>
            </header>
            <div className="disc-participant-details-meta">
              <span>{selectedParticipant.role}</span>
              <span>{selectedParticipant.wake_mode === 'native_dispatch' ? t('disc.targetNative') : t('disc.targetCli')}</span>
            </div>
            <dl>
              <div>
                <dt>{t('disc.participantStatus')}</dt>
                <dd>{selectedPresenceLabel}</dd>
              </div>
              {selectedParticipant.model && (
                <div>
                  <dt>{t('disc.participantModel')}</dt>
                  <dd className="disc-participant-model">{selectedParticipant.model}</dd>
                </div>
              )}
              {selectedParticipant.resume_reason && (
                <div>
                  <dt>{t('disc.resumeReason')}</dt>
                  <dd>{selectedParticipant.resume_reason}</dd>
                </div>
              )}
              {selectedParticipant.resume_since && (
                <div>
                  <dt>{t('disc.resumeSince')}</dt>
                  <dd>{new Date(selectedParticipant.resume_since).toLocaleString()}</dd>
                </div>
              )}
            </dl>
            {selectedParticipant.write_state === 'failed' && (
              <span className="disc-participant-write-failed">{t('disc.writeFailed')}</span>
            )}
            {selectedResumeCommand && (
              <CopyIdPill
                id={selectedResumeCommand}
                label={t('disc.resumeCli')}
                title={t('disc.resumeCliCopy', selectedResumeCommand)}
                className="disc-participant-resume"
              />
            )}
          </section>
      )}

    </div>
  );
}
