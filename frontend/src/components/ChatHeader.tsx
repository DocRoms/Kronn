import { useState, useEffect, useMemo, useRef } from 'react';
import '../pages/DiscussionsPage.css';
import { discussions as discussionsApi } from '../lib/api';
import type {
  AgentDetection,
  Discussion,
  AgentType,
  ModelTiersConfig,
} from '../types/generated';
import { AGENT_MENTIONS, isUsable, isValidationDisc, isBriefingDisc, isBootstrapDisc } from '../lib/constants';
import type { ToastFn } from '../hooks/useToast';
import {
  Pencil, ShieldCheck, Zap, Rocket,
  Menu, Star,
  Power,
  PowerOff,
  Loader2,
} from 'lucide-react';
import { MatrixText } from './MatrixText';
import { LearningsBadge } from './LearningsBadge';
import { DiscParticipantsHeader } from './DiscParticipantsHeader';
import { DiscussionScopeChip } from './DiscussionScope';
import { DiscInviteButton } from './DiscInviteButton';
import { DetailHeaderShell } from './DetailHeaderShell';
import { AgentSwitchPicker } from './AgentSwitchPicker';
import type { AgentSwitchTarget } from './AgentSwitchPicker';
import { ContextHelp } from './ContextHelp';
import type { ExternalApiConnectionView } from '../lib/api';
import {
  discussionConnectionId, externalAgentTargets, externalConnectionForDiscussion,
} from '../lib/externalAgentIdentity';

export interface ChatHeaderProps {
  discussion: Discussion;
  agents: AgentDetection[];
  modelTiers?: ModelTiersConfig | null;
  externalConnections?: ExternalApiConnectionView[];
  isMobile: boolean;
  sending: boolean;
  /** "Details" opens the side panel on its settings tab: the id, the cost, the
   *  project and the worktree state live there, not in a third header row. */
  onOpenDetails: () => void;
  /** Whether that panel is showing, for the button's `aria-expanded`. */
  settingsPanelOpen?: boolean;
  onToggleSidebar: () => void;
  onDiscussionUpdated: () => void;
  /** Share the authoritative room mode with the composer, scoped by identity. */
  onNativeAgentModeChange?: (discussionId: string, disabled: boolean | null) => void;
  onAgentSwitch: (newAgent: AgentType) => void;
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function ChatHeader({
  discussion,
  agents,
  modelTiers = null,
  externalConnections = [],
  isMobile,
  sending,
  onOpenDetails,
  settingsPanelOpen = false,
  onToggleSidebar,
  onDiscussionUpdated,
  onNativeAgentModeChange,
  onAgentSwitch,
  toast,
  t,
}: ChatHeaderProps) {
  const externalConnection = externalConnectionForDiscussion(discussion, externalConnections);
  const externalConnectionId = discussionConnectionId(discussion);
  // KT-545 DoD #4: switching mid-thread can target a named HTTP connection,
  // not just a built-in agent type — same source as New Discussion/Quick
  // Prompt, so the same connection resolves identically everywhere.
  const connectionTargets = useMemo(
    () => externalAgentTargets(externalConnections),
    [externalConnections],
  );
  // Header-only state
  const [editingTitleId, setEditingTitleId] = useState<string | null>(null);
  const [editingTitleText, setEditingTitleText] = useState('');
  const [nativeAgentMode, setNativeAgentMode] = useState<{
    discussionId: string;
    disabled: boolean;
  } | null>(null);
  const [nativeAgentModeSaving, setNativeAgentModeSaving] = useState(false);
  // An invite mints a peer; the row below has to go and look for it.
  const [participantsRefresh, setParticipantsRefresh] = useState(0);
  const nativeAgentModeInFlight = useRef(false);

  useEffect(() => {
    let current = true;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;
    const loadNativeAgentMode = () => {
      void discussionsApi.nativeAgentMode(discussion.id)
        .then(mode => {
          if (current) {
            setNativeAgentMode({ discussionId: discussion.id, disabled: mode.disabled });
          }
        })
        .catch(() => {
          // Never guess "enabled" after a transient backend failure: that
          // would make the header contradict the persisted no-agent routing
          // contract. Keep both actions disabled and retry the authoritative
          // backend state after reconnect.
          if (current) retryTimer = setTimeout(loadNativeAgentMode, 5_000);
        });
    };
    loadNativeAgentMode();
    return () => {
      current = false;
      if (retryTimer) clearTimeout(retryTimer);
    };
  }, [discussion.id]);

  const installedAgentsList = agents.filter(isUsable);
  const nativeAgentDisabled = nativeAgentMode?.discussionId === discussion.id
    ? nativeAgentMode.disabled
    : null;

  useEffect(() => {
    onNativeAgentModeChange?.(discussion.id, nativeAgentDisabled);
  }, [discussion.id, nativeAgentDisabled, onNativeAgentModeChange]);

  const updateNativeAgentMode = async (disabled: boolean) => {
    if (nativeAgentModeInFlight.current || sending) return;
    nativeAgentModeInFlight.current = true;
    setNativeAgentModeSaving(true);
    try {
      await discussionsApi.update(discussion.id, { no_agent: disabled });
      setNativeAgentMode({ discussionId: discussion.id, disabled });
      onDiscussionUpdated();
      toast(
        t(disabled ? 'disc.nativeAgentDisabledToast' : 'disc.nativeAgentEnabledToast'),
        'success',
      );
    } catch (error) {
      toast(String(error), 'error');
    } finally {
      nativeAgentModeInFlight.current = false;
      setNativeAgentModeSaving(false);
    }
  };

  return (
    <DetailHeaderShell
      detailsId="disc-settings-panel"
      onOpenDetails={onOpenDetails}
      detailsPanelOpen={settingsPanelOpen}
      detailsLabel={t('disc.headerDetails')}
      detailsShowTitle={t('disc.headerDetailsShow')}
      detailsHideTitle={t('disc.headerDetailsHide')}
      toggleTestId="disc-header-details-btn"
      rootProps={{ 'data-tour-id': 'disc-header-controls' }}
      titleProps={{ 'data-tour-id': 'disc-identity-controls' }}
      leading={isMobile ? (
        <button
          className="disc-mobile-sidebar-btn"
          onClick={onToggleSidebar}
          aria-label="Open sidebar"
        >
          <Menu size={18} />
        </button>
      ) : null}
      title={<>
        {/* Pin / favorite toggle — always visible in the header so the user
            can pin from inside the conversation. Outline = not pinned,
            filled yellow = pinned. Sidebar shows the result in its
            "Favorites" section at the top. */}
        <button
          type="button"
          className="disc-pin-header-btn"
          onClick={async () => {
            try {
              await discussionsApi.update(discussion.id, { pinned: !discussion.pinned });
              onDiscussionUpdated();
            } catch { /* silent — toast from parent */ }
          }}
          title={discussion.pinned ? t('disc.unpin') : t('disc.pin')}
          aria-label={discussion.pinned ? t('disc.unpin') : t('disc.pin')}
          aria-pressed={discussion.pinned}
        >
          <Star
            size={14}
            style={discussion.pinned
              ? { color: 'var(--kr-warning)', fill: 'var(--kr-warning)' }
              : { color: 'var(--kr-text-ghost)' }}
          />
        </button>
        {isValidationDisc(discussion.title) && <ShieldCheck size={14} className="text-accent flex-shrink-0" />}
        {isBriefingDisc(discussion.title) && <Zap size={14} className="text-info flex-shrink-0" />}
        {isBootstrapDisc(discussion.title) && <Rocket size={14} className="text-accent flex-shrink-0" />}
        {editingTitleId === discussion.id && !isValidationDisc(discussion.title) && !isBootstrapDisc(discussion.title) && !isBriefingDisc(discussion.title) ? (
          <input
            autoFocus
            className="disc-title-input"
            value={editingTitleText}
            onChange={e => setEditingTitleText(e.target.value)}
            onKeyDown={async e => {
              if (e.key === 'Enter' && editingTitleText.trim()) {
                const newTitle = editingTitleText.trim();
                await discussionsApi.update(discussion.id, { title: newTitle });
                setEditingTitleId(null);
                onDiscussionUpdated();
              }
              if (e.key === 'Escape') setEditingTitleId(null);
            }}
            onBlur={async () => {
              if (editingTitleText.trim() && editingTitleText.trim() !== discussion.title) {
                const newTitle = editingTitleText.trim();
                await discussionsApi.update(discussion.id, { title: newTitle });
                onDiscussionUpdated();
              }
              setEditingTitleId(null);
            }}
          />
        ) : (
          <span
            className="disc-chat-header-title-text"
            style={{ cursor: (isValidationDisc(discussion.title) || isBootstrapDisc(discussion.title) || isBriefingDisc(discussion.title)) ? 'default' : 'pointer' }}
            onDoubleClick={() => {
              if (isValidationDisc(discussion.title) || isBootstrapDisc(discussion.title) || isBriefingDisc(discussion.title)) return;
              setEditingTitleId(discussion.id);
              setEditingTitleText(discussion.title);
            }}
            title={(isValidationDisc(discussion.title) || isBootstrapDisc(discussion.title) || isBriefingDisc(discussion.title)) ? undefined : t('disc.editTitle')}
          >
            <MatrixText text={discussion.title} />
          </span>
        )}
        {!isValidationDisc(discussion.title) && !isBootstrapDisc(discussion.title) && !isBriefingDisc(discussion.title) && (
        <button
          className="disc-icon-btn"
          style={{ padding: '2px 4px', border: 'none', background: 'none', color: 'var(--kr-text-ghost)' }}
          onClick={() => {
            if (editingTitleId === discussion.id) {
              setEditingTitleId(null);
            } else {
              setEditingTitleId(discussion.id);
              setEditingTitleText(discussion.title);
            }
          }}
          title={t('disc.editTitle')}
          aria-label={t('disc.editTitle')}
        >
          <Pencil size={10} />
        </button>
        )}
        <ContextHelp title={t('contextHelp.discussion.title')}>
          <p>{t('contextHelp.discussion.intro')}</p>
          <ul>
            <li>{t('contextHelp.discussion.mainAgent')}</li>
            <li>{t('contextHelp.discussion.participants')}</li>
            <li>{t('contextHelp.discussion.messages')}</li>
            <li>{t('contextHelp.discussion.outputs')}</li>
          </ul>
          <p className="kr-context-help-agent-note">{t('contextHelp.discussion.mcp')}</p>
        </ContextHelp>
      </>}
      trailing={
        // The invite acts on the discussion, so it sits on the discussion's own
        // row rather than at the end of the chip strip.
        <DiscInviteButton
          discId={discussion.id}
          toast={toast}
          t={t}
          onInvited={() => setParticipantsRefresh(n => n + 1)}
        />

      }
      sub={<>
        {/* First position, as the row's subject: whether this discussion
            answers by itself is the thing that changes what everything else
            on the row means. */}
        <span className="disc-native-agent-control">
          {nativeAgentDisabled ? (
            <button
              type="button"
              className="kr-agent-switch-btn disc-native-agent-disabled"
              data-testid="disc-native-agent-disabled"
              disabled={sending || nativeAgentModeSaving}
              title={t('disc.nativeAgentEnable')}
              aria-label={t('disc.nativeAgentEnable')}
              onClick={() => void updateNativeAgentMode(false)}
            >
              {nativeAgentModeSaving
                ? <Loader2 size={9} className="spin" />
                : <Power size={9} />}
              <span>{t('disc.nativeAgentDisabled')}</span>
            </button>
          ) : (
            <>
              <AgentSwitchPicker
                currentAgent={discussion.agent}
                currentConnectionId={externalConnectionId}
                currentTargetLabel={externalConnection?.display_name}
                availableAgents={installedAgentsList.map(agent => agent.agent_type)}
                availableTargets={[
                  ...installedAgentsList.map(agent => ({ agent: agent.agent_type })),
                  ...connectionTargets.map(target => ({
                    agent: target.agent,
                    connectionId: target.connectionId,
                    label: target.label,
                    modelTiers: target.modelTiers,
                  })),
                ]}
                disabled={sending || nativeAgentDisabled === null || nativeAgentModeSaving}
                title={t('disc.switchAgentAndTier')}
                ariaLabel={t('disc.switchAgentAndTier')}
                suffix={t('disc.targetDiscussionAgent')}
                currentTier={discussion.tier ?? 'default'}
                currentModel={discussion.model}
                tierLabels={{
                  economy: t('disc.tier.economy'),
                  default: t('disc.tier.default'),
                  reasoning: t('disc.tier.reasoning'),
                }}
                modelTiers={modelTiers}
                defaultModelLabel={t('config.defaultModel')}
                displayName={
                  externalConnection
                    ? `@${externalConnection.mention_alias}`
                    : AGENT_MENTIONS.find(mention => mention.type === discussion.agent)?.trigger
                  ?? discussion.agent
                }
                onTargetSelectionChange={async (target: AgentSwitchTarget, tier) => {
                  try {
                    await discussionsApi.update(discussion.id, {
                      agent: target.agent,
                      tier,
                      connection_id: target.connectionId ?? null,
                    });
                    onAgentSwitch(target.agent);
                  } catch (err) {
                    toast(String(err), 'error');
                    throw err;
                  }
                }}
              />
              <button
                type="button"
                className="disc-native-agent-toggle"
                disabled={sending || nativeAgentDisabled === null || nativeAgentModeSaving}
                title={t('disc.nativeAgentDisable')}
                aria-label={t('disc.nativeAgentDisable')}
                onClick={() => void updateNativeAgentMode(true)}
              >
                {nativeAgentModeSaving
                  ? <Loader2 size={9} className="spin" />
                  : <PowerOff size={9} />}
              </button>
            </>
          )}
        </span>
        <DiscParticipantsHeader
          discId={discussion.id}
          t={t}
          refreshKey={participantsRefresh}
        />
        {/* KT-1270 — never folded away: what the room reaches beyond its project. */}
        <DiscussionScopeChip
          discussionId={discussion.id}
          hasProject={!!discussion.project_id}
          onOpen={onOpenDetails}
          t={t}
        />
      </>}
      actions={
        <div className="disc-chat-header-actions" data-tour-id="disc-output-controls">
          {/* 0.10.0 — pending-learnings badge (self-contained; hidden when 0). */}
          <LearningsBadge t={t} toast={toast} />
        </div>
      }
    />
  );
}
