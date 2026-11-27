import { useState, useEffect, useCallback, useRef, type ReactNode, type RefObject } from 'react';
import { useAnchoredLayer } from './ui/useAnchoredLayer';
import { createPortal } from 'react-dom';
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
import { AGENT_MENTIONS, agentTextColor } from '../lib/constants';
import { useT } from '../lib/I18nContext';
import { AgentIdentity } from './ui/AgentIdentity';
import { Tooltip } from './ui/Tooltip';
import { agentIdentityText, agentModel, agentModelText, type AgentIdentityModel, type AgentIdentityRole } from './ui/agentIdentityKinds';
import { ParticipantsStrip } from './ParticipantsStrip';

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
  /** The discussion's own agent: its native participant is the main agent,
   *  shown first; another native agent is a one-off agent. */
  mainAgent?: string;
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
  workflow_step?: {
    run_id: string;
    workflow_id: string;
    workflow_name: string;
    step_key: string;
    step_name: string;
  } | null;
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

/** A participant Kronn wakes itself (no CLI session of its own). */
const isNative = (p: ParticipantRow) => p.wake_mode === 'native_dispatch'
  || p.role === 'native_resume' || p.role === 'obligation';

/** KT-1936 — the participant's role chip: a workflow step, the main agent, a
 *  one-off agent Kronn wakes, or a joined CLI session. */
function participantRole(p: ParticipantRow, mainAgent?: string): AgentIdentityRole {
  if (p.workflow_step) return 'workflowStep';
  if (isNative(p)) return mainAgent && p.agent_type === mainAgent ? 'main' : 'oneOff';
  return 'cli';
}

/** The model a participant declared, else « model not declared » for a joined
 *  CLI session and « default model of <agent> » for a native one. */
function participantModel(p: ParticipantRow): AgentIdentityModel {
  return agentModel(p.model, isNative(p) || p.workflow_step ? 'providerDefault' : 'notDeclared');
}

const mentionOf = (agentType: string) =>
  AGENT_MENTIONS.find(mention => mention.type === agentType)?.trigger ?? agentType;

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

/**
 * KT-1936 review round 2 — a participant's details, a floating layer in
 * document.body under its chip: the header's agent line scrolls sideways, and
 * a layer absolute inside it was clipped. Keyed by participant, so another
 * chip places it again.
 *
 * Review round 3 — in document.body it is not next to its chip in the Tab
 * order: `useAnchoredLayer` (shared with the « +N » list) moves focus into it
 * on open, keeps Tab between its controls, closes it on Escape with focus back
 * on the chip, and on a press outside.
 */
function ParticipantDetailsLayer({ anchorRef, onClose, children, ...props }: {
  anchorRef: RefObject<HTMLElement | null>;
  onClose: () => void;
  children: ReactNode;
  id: string;
  'aria-label': string;
}) {
  const { ref, style } = useAnchoredLayer<HTMLElement>({
    anchorRef, open: true, placement: 'bottom', align: 'start', onClose,
  });
  return createPortal(
    <section
      ref={ref}
      className="disc-participant-details"
      role="dialog"
      data-testid="disc-participant-details"
      style={style}
      {...props}
    >
      {children}
    </section>,
    document.body,
  );
}

export function DiscParticipantsHeader({ discId, t, refreshKey = 0, mainAgent }: DiscParticipantsHeaderProps) {
  // AgentIdentity's own words (roles, unknown models) come from the locale.
  const { t: tUi, tp } = useT();
  const [participants, setParticipants] = useState<ParticipantRow[]>([]);
  const [awayAfterMs, setAwayAfterMs] = useState(DEFAULT_AWAY_AFTER_MS);
  const [selectedParticipantId, setSelectedParticipantId] = useState<number | null>(null);
  // The selected participant's chip: where its details layer is anchored.
  const selectedChipRef = useRef<HTMLButtonElement | null>(null);

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
    && !selectedParticipant.workflow_step
    ? resumeCommandFor(selectedParticipant.agent_type, selectedParticipant.conversation_id)
    : null;
  const selectedDisplayName = selectedParticipant ? mentionOf(selectedParticipant.agent_type) : null;
  // The main agent first; the others keep the server's order.
  const ordered = [...participants].sort((a, b) =>
    Number(participantRole(b, mainAgent) === 'main') - Number(participantRole(a, mainAgent) === 'main'));
  const identityOf = (p: ParticipantRow) => ({
    name: mentionOf(p.agent_type),
    model: participantModel(p),
    role: participantRole(p, mainAgent),
  });
  const selectedIdentityLabel = selectedParticipant
    ? agentIdentityText(identityOf(selectedParticipant), tUi)
    : null;

  return (
    <div className="disc-participants-row" data-testid="disc-participants-row">
      <ParticipantsStrip
        moreLabel={total => tp('disc.participantsMore', total)}
        allLabel={t('disc.participantsAll')}
        empty={(
          <span className="disc-participants-empty">
            {t('disc.participantsEmpty')}
          </span>
        )}
        entries={ordered.map(p => {
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
          const identity = identityOf(p);
          // The full model id: the chip cuts it past 17 characters.
          const identityLabel = agentIdentityText(identity, tUi);
          const open = () => setSelectedParticipantId(current => current === p.id ? null : p.id);
          const chip = (
            <Tooltip
              content={[
                identityLabel,
                label,
                p.workflow_step ? `${p.workflow_step.workflow_name} › ${p.workflow_step.step_name}` : null,
              ].filter(Boolean).join(' — ')}
            >
            <button
              ref={selectedParticipantId === p.id ? selectedChipRef : undefined}
              type="button"
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
              aria-label={t('disc.participantDetails', identityLabel)}
              onClick={open}
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
              {/* KT-1936 — state, agent icon, name, model (cut past 17
               *  characters), role, at 12 px (owner mockup 2026-11-20). */}
              <AgentIdentity
                className="disc-participant-identity"
                icon={iconFor(p.agent_type)}
                {...identity}
                color={agentTextColor(p.agent_type)}
                truncateModel
              />
              {p.workflow_step && (
                <span className="disc-participant-workflow" data-testid="workflow-step-participant">
                  {p.workflow_step.workflow_name} › {p.workflow_step.step_name}
                </span>
              )}
              {writeState === 'failed' && (
                <span className="disc-participant-write-failed-dot" aria-label={t('disc.writeFailed')} />
              )}
            </button>
            </Tooltip>
          );
          return {
            key: p.id,
            chip,
            row: <AgentIdentity icon={iconFor(p.agent_type)} {...identity} color={agentTextColor(p.agent_type)} />,
            onSelect: () => setSelectedParticipantId(p.id),
          };
        })}
      />
      {selectedParticipant && selectedDisplayName && selectedPresenceLabel && (
          <ParticipantDetailsLayer
            key={selectedParticipant.id}
            anchorRef={selectedChipRef}
            onClose={() => setSelectedParticipantId(null)}
            id={`disc-participant-details-${selectedParticipant.id}`}
            // The full identity: the header cuts the model when it is long.
            aria-label={t('disc.participantDetails', selectedIdentityLabel ?? selectedDisplayName)}
          >
            {/* Review round 3 — a flex row: the identity shrinks (the model
             *  first, then it wraps), the close button never does, so it stays
             *  inside the layer. The full model id is in the tooltip, the
             *  layer's name and the model row below. */}
            <header>
              <strong title={selectedIdentityLabel ?? undefined}>
                <AgentIdentity
                  className="disc-participant-details-identity"
                  size="full"
                  icon={iconFor(selectedParticipant.agent_type)}
                  {...identityOf(selectedParticipant)}
                  color={agentTextColor(selectedParticipant.agent_type)}
                />
              </strong>
              <button
                type="button"
                onClick={() => setSelectedParticipantId(null)}
                aria-label={t('common.close')}
              >
                <X size={12} />
              </button>
            </header>
            <div className="disc-participant-details-meta">
              <span>{selectedParticipant.workflow_step
                ? t('disc.workflowStepAgent')
                : selectedParticipant.wake_mode === 'native_dispatch'
                  ? t('disc.targetNative')
                  : t('disc.targetCli')}</span>
            </div>
            <dl>
              <div>
                <dt>{t('disc.participantStatus')}</dt>
                <dd>{selectedPresenceLabel}</dd>
              </div>
              <div>
                <dt>{t('disc.participantModel')}</dt>
                <dd className="disc-participant-model" data-state={participantModel(selectedParticipant).kind}>
                  {agentModelText(participantModel(selectedParticipant), tUi)}
                </dd>
              </div>
              {selectedParticipant.workflow_step && (
                <>
                  <div>
                    <dt>{t('disc.workflow')}</dt>
                    <dd>{selectedParticipant.workflow_step.workflow_name}</dd>
                  </div>
                  <div>
                    <dt>{t('disc.workflowStep')}</dt>
                    <dd>{selectedParticipant.workflow_step.step_name}</dd>
                  </div>
                  <div>
                    <dt>{t('disc.workflowRunLabel')}</dt>
                    <dd>{selectedParticipant.workflow_step.run_id}</dd>
                  </div>
                </>
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
          </ParticipantDetailsLayer>
      )}

    </div>
  );
}
