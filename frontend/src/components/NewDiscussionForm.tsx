import { useState, useEffect, useMemo, useRef } from 'react';
import '../pages/DiscussionsPage.css';
import { ProfileTooltip } from './ProfileTooltip';
import { AgentSwitchPicker } from './AgentSwitchPicker';
import { MarkdownEditor } from './MarkdownComposerTools';
import { skills as skillsApi, profiles as profilesApi, directives as directivesApi, config as configApi } from '../lib/api';
import type { Project, AgentDetection, AgentType, AgentsConfig, Skill, AgentProfile, Directive, ModelTier } from '../types/generated';
import { AGENT_LABELS, AGENT_MENTIONS, MODEL_TIER_ICONS, agentColor, mentionedAgents, modelForAgentTier, isAgentRestricted as isAgentRestrictedUtil, isUsable, isHiddenPath, RTK_APPLICABLE, isRtkActive } from '../lib/constants';
import { findAgentMentionQuery, type AgentMentionQuery } from '../lib/mention-autocomplete';
import {
  applyEmojiReplacement,
  findEmojiQuery,
  searchEmojis,
  type EmojiQuery,
  type EmojiSuggestion,
} from '../lib/emoji-autocomplete';
import {
  Folder, ChevronRight, GitBranch,
  MessageSquare, X, AlertTriangle,
  Settings, Check, Zap, UserCircle, FileText, Paperclip, Image,
  Cpu,
} from 'lucide-react';

// ─── Public types ────────────────────────────────────────────────────────────

export interface NewDiscConfig {
  title: string;
  agent: AgentType;
  projectId: string | null;
  prompt: string;
  skillIds: string[];
  profileIds: string[];
  directiveIds: string[];
  workspaceMode: 'Direct' | 'Isolated';
  tier: 'economy' | 'default' | 'reasoning';
  branchName: string;
  baseBranch: string;
  pendingFiles?: File[];
  /** Agents explicitly mentioned in the initial prompt. Empty means the
   * legacy single-agent picker owns the launch. */
  targetAgents: AgentType[];
  /** Per-agent reasoning levels selected beside each prompt alias. */
  targetTiers: Partial<Record<AgentType, ModelTier>>;
  /** 0.8.6 phase 2 — when `false`, the parent should create the disc
   *  WITHOUT auto-launching an agent (CLI run skipped). The user is
   *  expected to invite agents later via the `[+ Inviter]` header
   *  button. Default `true` preserves the legacy behaviour. */
  launchAgentNow: boolean;
}

export interface NewDiscussionFormProps {
  projects: Project[];
  agents: AgentDetection[];
  configLanguage: string | null;
  agentAccess: AgentsConfig | null;
  prefill?: { projectId: string; title: string; prompt: string; locked?: boolean } | null;
  onSubmit: (config: NewDiscConfig) => void;
  onClose: () => void;
  onPrefillConsumed?: () => void;
  onNavigate: (page: string, opts?: { scrollTo?: string }) => void;
  t: (key: string, ...args: (string | number)[]) => string;
}

// ─── Component ───────────────────────────────────────────────────────────────

export function NewDiscussionForm({
  projects,
  agents,
  agentAccess,
  prefill,
  onSubmit,
  onClose,
  onPrefillConsumed,
  onNavigate,
  t,
}: NewDiscussionFormProps) {
  // ─── Internal state ──────────────────────────────────────────────────────
  const [newDiscTitle, setNewDiscTitle] = useState('');
  const [newDiscAgent, setNewDiscAgent] = useState<AgentType | ''>('');
  const [agentLaunchMode, setAgentLaunchMode] = useState<'selected' | 'prompt'>('selected');
  const [newDiscProjectId, setNewDiscProjectId] = useState<string>('');
  const [newDiscPrompt, setNewDiscPrompt] = useState('');
  const [newDiscPrefilled, setNewDiscPrefilled] = useState(false);
  const [showAdvancedOptions, setShowAdvancedOptions] = useState(false);
  const [expandedAdvanced, setExpandedAdvanced] = useState<'skills' | 'profiles' | 'directives' | null>(null);

  const [availableSkills, setAvailableSkills] = useState<Skill[]>([]);
  const [newDiscSkillIds, setNewDiscSkillIds] = useState<string[]>([]);
  const [availableProfiles, setAvailableProfiles] = useState<AgentProfile[]>([]);
  const [newDiscProfileIds, setNewDiscProfileIds] = useState<string[]>([]);
  const [newDiscDirectiveIds, setNewDiscDirectiveIds] = useState<string[]>([]);
  const [availableDirectives, setAvailableDirectives] = useState<Directive[]>([]);
  const [newDiscWorkspaceMode, setNewDiscWorkspaceMode] = useState<'Direct' | 'Isolated'>('Direct');
  // Initialised to 'default' for back-compat ; the effect below replaces it
  // with the user's `ServerConfig.default_model_tier` on mount (0.8.6 phase 4).
  // Strict semantic — only applied at form-open time, never retroactively.
  const [newDiscTier, setNewDiscTier] = useState<'economy' | 'default' | 'reasoning'>('default');
  const [promptAgentTiers, setPromptAgentTiers] = useState<Partial<Record<AgentType, ModelTier>>>({});
  const [agentHandoffsEnabled, setAgentHandoffsEnabled] = useState<boolean | null>(null);
  // 0.8.6 phase 2 — disc-first refactor. When `false`, the disc is
  // created without launching a CLI ; the user invites agents later
  // via the `[+ Inviter]` header button. Default `true` keeps the
  // legacy "create + run" flow for the 80% common case.
  const [launchAgentNow, setLaunchAgentNow] = useState(true);
  const [newDiscBranchName, setNewDiscBranchName] = useState('');
  const [newDiscBaseBranch, setNewDiscBaseBranch] = useState('main');
  const [pendingFiles, setPendingFiles] = useState<File[]>([]);
  const newDiscFileInputRef = useRef<HTMLInputElement>(null);
  const newDiscPromptRef = useRef<HTMLTextAreaElement>(null);
  const [mentionMatch, setMentionMatch] = useState<AgentMentionQuery | null>(null);
  const mentionQuery = mentionMatch?.query ?? null;
  const [mentionIndex, setMentionIndex] = useState(0);
  const [emojiMatch, setEmojiMatch] = useState<EmojiQuery | null>(null);
  const [emojiSuggestions, setEmojiSuggestions] = useState<EmojiSuggestion[]>([]);
  const [emojiIndex, setEmojiIndex] = useState(0);
  const previousPromptAgentsRef = useRef('');

  // ─── Derived ─────────────────────────────────────────────────────────────
  const installedAgentsList = useMemo(() => agents.filter(isUsable), [agents]);
  const promptMentionedAgents = useMemo(
    () => mentionedAgents(newDiscPrompt),
    [newDiscPrompt],
  );
  const installedAgentTypes = useMemo(
    () => new Set(installedAgentsList.map(agent => agent.agent_type)),
    [installedAgentsList],
  );
  const promptLaunchAgents = promptMentionedAgents.filter(agent => installedAgentTypes.has(agent));
  const unavailablePromptAgents = promptMentionedAgents.filter(agent => !installedAgentTypes.has(agent));
  const effectiveLaunchAgents: AgentType[] = agentLaunchMode === 'prompt'
    ? promptLaunchAgents
    : (newDiscAgent ? [newDiscAgent] : []);
  const promptModeReady = agentLaunchMode !== 'prompt'
    || (promptLaunchAgents.length > 0 && unavailablePromptAgents.length === 0);
  const launchReady = Boolean(newDiscPrompt.trim())
    && (agentLaunchMode === 'prompt' ? promptModeReady : Boolean(newDiscAgent));

  const isAgentRestricted = (agentType: AgentType): boolean =>
    isAgentRestrictedUtil(agentAccess ?? undefined, agentType);

  // ─── Effects ─────────────────────────────────────────────────────────────

  // Fetch available skills, profiles, directives. Also re-fetch
  // profiles on `kronn:profiles-changed` so a secret-code unlock
  // (e.g. Batman) flips the picker without a reload.
  useEffect(() => {
    const refetchProfiles = () => profilesApi.list()
      .then(setAvailableProfiles)
      .catch(e => console.warn('Failed to load profiles:', e));
    skillsApi.list().then(setAvailableSkills).catch(e => console.warn('Failed to load skills:', e));
    refetchProfiles();
    directivesApi.list().then(setAvailableDirectives).catch(e => console.warn('Failed to load directives:', e));
    window.addEventListener('kronn:profiles-changed', refetchProfiles);
    return () => window.removeEventListener('kronn:profiles-changed', refetchProfiles);
  }, []);

  // Auto-select first installed agent if current selection is invalid
  useEffect(() => {
    if (installedAgentsList.length > 0 && !installedAgentsList.some(a => a.agent_type === newDiscAgent)) {
      setNewDiscAgent(installedAgentsList[0].agent_type);
    }
  }, [installedAgentsList, newDiscAgent]);

  // Canonical mentions own routing while they remain in the prompt. Keeping
  // the single-agent picker locked prevents an accidental click from hiding
  // recipients that are still visibly addressed in the brief.
  const promptAgentKey = promptMentionedAgents.join(',');
  useEffect(() => {
    const previous = previousPromptAgentsRef.current;
    if (promptAgentKey && promptAgentKey !== previous) {
      setAgentLaunchMode('prompt');
    } else if (!promptAgentKey && previous) {
      setAgentLaunchMode('selected');
    }
    previousPromptAgentsRef.current = promptAgentKey;
  }, [promptAgentKey]);

  // 0.8.6 phase 4 — apply the user's saved default model tier ONCE on
  // form mount. The user can still override per-disc by clicking another
  // tier button before submit ; the saved default just changes the
  // initial selection. Strict semantic — re-mounting the form (e.g.
  // re-opening it for a new disc) picks the LATEST default, which is
  // the intuitive behaviour. We skip when the form is prefilled-locked
  // (validation audit / continuation flows) so prefilled tier stays
  // authoritative.
  useEffect(() => {
    if (prefill?.locked) return;
    configApi.getServerConfig()
      .then(cfg => {
        if (cfg?.default_model_tier) {
          setNewDiscTier(cfg.default_model_tier);
        }
        setAgentHandoffsEnabled(cfg?.agent_handoffs_enabled ?? false);
      })
      .catch(() => { /* keep 'default' fallback */ });
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Handle prefill from parent (e.g. "validate audit" button on Projects page)
  useEffect(() => {
    if (prefill) {
      // Lock fields only when explicitly requested (validation audit)
      setNewDiscPrefilled(!!prefill.locked);
      setNewDiscProjectId(prefill.projectId);
      setNewDiscTitle(prefill.title);
      setNewDiscPrompt(prefill.prompt);
      // Auto-select mandatory profiles ONLY for validation audits.
      // Pre-fix this fired on every prefill — including the "New discussion"
      // button on a project card and the "Discuss this file" CTA from the AI
      // doc viewer — which silently pre-selected architect/tech-lead/qa-
      // engineer profiles for unrelated chats. Bound to `locked` because
      // only the validation entry-point sets that flag.
      if (prefill.locked) {
        const validationProfileIds = ['architect', 'tech-lead', 'qa-engineer'];
        setNewDiscProfileIds(validationProfileIds);
      }
      onPrefillConsumed?.();
    }
  }, [prefill, onPrefillConsumed]);

  // ─── Callbacks ───────────────────────────────────────────────────────────

  const handleClose = () => {
    setNewDiscPrefilled(false);
    setNewDiscWorkspaceMode('Direct');
    setNewDiscBranchName('');
    setNewDiscBaseBranch('main');
    onClose();
  };

  const [creating, setCreating] = useState(false);
  const creatingRef = useRef(false);

  const refreshPromptAutocomplete = (text: string, cursor: number) => {
    const mention = findAgentMentionQuery(text, cursor);
    setMentionMatch(mention);
    if (mention) setMentionIndex(0);

    const emoji = findEmojiQuery(text, cursor);
    const suggestions = emoji ? searchEmojis(emoji.query) : [];
    setEmojiMatch(suggestions.length > 0 ? emoji : null);
    setEmojiSuggestions(suggestions);
    if (suggestions.length > 0) setEmojiIndex(0);
  };

  const prunePromptAgentTiers = (text: string) => {
    const activeAgents = new Set(mentionedAgents(text));
    setPromptAgentTiers(previous => Object.fromEntries(
      Object.entries(previous).filter(([agent]) => activeAgents.has(agent as AgentType)),
    ) as Partial<Record<AgentType, ModelTier>>);
  };

  const applyMentionSuggestion = (
    trigger: string,
    textarea: HTMLTextAreaElement | null,
    agentType?: AgentType,
    tier?: ModelTier,
  ) => {
    const range = mentionMatch;
    if (!range) return;
    const trailing = newDiscPrompt.slice(range.end);
    const spacer = trailing.length === 0 || !/^\s/.test(trailing) ? ' ' : '';
    const next = `${newDiscPrompt.slice(0, range.start)}${trigger}${spacer}${trailing}`;
    const cursor = range.start + trigger.length + spacer.length;
    setNewDiscPrompt(next);
    if (agentType) {
      setPromptAgentTiers(previous => ({
        ...previous,
        [agentType]: tier ?? previous[agentType] ?? newDiscTier,
      }));
    }
    setMentionMatch(null);
    requestAnimationFrame(() => {
      textarea?.focus();
      textarea?.setSelectionRange(cursor, cursor);
    });
  };

  const applyEmojiSuggestion = (suggestion: EmojiSuggestion) => {
    if (!emojiMatch) return;
    const { text, cursor } = applyEmojiReplacement(
      newDiscPrompt,
      emojiMatch,
      suggestion.emoji,
    );
    setNewDiscPrompt(text);
    setEmojiMatch(null);
    setEmojiSuggestions([]);
    requestAnimationFrame(() => {
      newDiscPromptRef.current?.focus();
      newDiscPromptRef.current?.setSelectionRange(cursor, cursor);
    });
  };

  const handleCreate = async () => {
    // Submit gate :
    //   - launch mode  → prompt + agent both required (legacy contract)
    //   - no-launch    → prompt OR title required so the empty disc has
    //                    SOMETHING to display in the list. We default
    //                    the title to a placeholder when both blank.
    if (creatingRef.current) return;
    if (launchAgentNow) {
      if (!launchReady) return;
    } else {
      if (!newDiscPrompt.trim() && !newDiscTitle.trim()) return;
    }
    creatingRef.current = true;
    setCreating(true);
    try {
      // `onSubmit` is typed `=> void` but the parent's implementation may be
      // async — await it through Promise.resolve so failures unblock the
      // button. Without this, if `discussions.create` throws, `creating`
      // stays true forever and the form is wedged until close+reopen.
      const fallbackTitle = launchAgentNow
        ? newDiscPrompt.trim().slice(0, 60)
        : (newDiscPrompt.trim().slice(0, 60) || t('disc.discFirstDefaultTitle'));
      const primaryAgent = (
        agentLaunchMode === 'prompt'
          ? promptLaunchAgents[0]
          : newDiscAgent
      || installedAgentsList[0]?.agent_type
      || 'ClaudeCode') as AgentType;
      const targetTiers = Object.fromEntries(
        promptLaunchAgents.map(agent => [agent, promptAgentTiers[agent] ?? newDiscTier]),
      ) as Partial<Record<AgentType, ModelTier>>;
      await Promise.resolve(onSubmit({
        title: newDiscTitle.trim() || fallbackTitle,
        // Even when `launchAgentNow=false`, the backend `CreateDiscussionRequest`
        // still requires an agent_type field (legacy `discussions.agent`
        // column NOT NULL). We send the currently-selected agent as a
        // placeholder ; the parent skips `runAgent` so no CLI runs.
        // The new `discussion_sessions` table is the source of truth for
        // actual participants from 0.8.6 onward.
        agent: primaryAgent,
        projectId: newDiscProjectId || null,
        prompt: newDiscPrompt.trim(),
        skillIds: newDiscSkillIds,
        profileIds: newDiscProfileIds,
        directiveIds: newDiscDirectiveIds,
        workspaceMode: newDiscWorkspaceMode,
        tier: agentLaunchMode === 'prompt'
          ? targetTiers[primaryAgent] ?? newDiscTier
          : newDiscTier,
        branchName: newDiscBranchName,
        baseBranch: newDiscBaseBranch,
        pendingFiles: pendingFiles.length > 0 ? pendingFiles : undefined,
        targetAgents: agentLaunchMode === 'prompt' ? promptLaunchAgents : [],
        targetTiers: agentLaunchMode === 'prompt' ? targetTiers : {},
        launchAgentNow,
      }));
    } catch (e) {
      // Parent (`handleCreateDiscussion` in DiscussionsPage) already toasts
      // its own errors. We swallow here only to keep the form unwedged —
      // the `finally` reset alone isn't enough because an uncaught throw
      // becomes an unhandled-rejection warning in the dev console.
      console.warn('[NewDiscussionForm] onSubmit rejected:', e);
    } finally {
      creatingRef.current = false;
      setCreating(false);
    }
  };

  // ─── Render ──────────────────────────────────────────────────────────────

  return (
    <div
      className="disc-new-overlay"
      onClick={e => { if (e.target === e.currentTarget) handleClose(); }}
      onKeyDown={e => { if (e.key === 'Escape') handleClose(); }}
      role="dialog"
      aria-modal="true"
      tabIndex={-1}
    >
      <div
        className="disc-new-card"
        onKeyDown={e => {
          if (e.key === 'Escape') { e.stopPropagation(); handleClose(); }
          // Pre-fix: no `preventDefault` here, so Ctrl+Enter inside the
          // prompt textarea inserted a newline AND triggered submit. The
          // submitted prompt ended with a stray "\n" — visible in agent
          // transcripts as a blank line at the bottom of the first
          // message. Suppress the default keypress so only the submit
          // path fires.
          if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && !e.nativeEvent.isComposing && newDiscPrompt.trim()) {
            e.preventDefault();
            handleCreate();
          }
        }}
      >
        <div className="disc-new-header">
          <div className="disc-new-heading">
            <span className="disc-new-heading-icon" aria-hidden="true">
              <MessageSquare size={16} />
            </span>
            <span>
              <span className="disc-new-title">{t('disc.newTitle')}</span>
              <span className="disc-new-subtitle">{t('disc.newSubtitle')}</span>
            </span>
          </div>
          <button className="disc-icon-btn" onClick={handleClose} aria-label="Close"><X size={14} /></button>
        </div>

        <div className="disc-new-layout">
          <section className="disc-new-section disc-new-brief" aria-labelledby="disc-new-brief-title">
            <div className="disc-new-section-heading">
              <span className="disc-new-section-icon" aria-hidden="true"><FileText size={14} /></span>
              <span>
                <strong id="disc-new-brief-title">{t('disc.brief')}</strong>
                <small>{t('disc.briefHint')}</small>
              </span>
            </div>

            <label className="disc-form-label">{t('disc.prompt')}</label>
            <div className="disc-new-prompt-wrap">
              {mentionQuery !== null && (() => {
                const matching = AGENT_MENTIONS.filter(mention => (
                  mention.trigger.slice(1).startsWith(mentionQuery)
                ));
                const available = matching.filter(mention => installedAgentTypes.has(mention.type));
                const unavailable = matching.filter(mention => !installedAgentTypes.has(mention.type));
                if (matching.length === 0) return null;
                return (
                  <div className="disc-mention-popover disc-new-autocomplete" role="listbox" aria-label={t('disc.mentionAgents')}>
                    {available.length > 0 && (
                      <div className="disc-mention-group">{t('disc.routingAvailableAgents')}</div>
                    )}
                    {available.map((mention, index) => {
                      const currentTier = promptAgentTiers[mention.type] ?? newDiscTier;
                      return (
                        <div
                          key={mention.trigger}
                          role="option"
                          aria-selected={index === mentionIndex}
                          className="disc-mention-item"
                          data-highlighted={index === mentionIndex}
                          onMouseEnter={() => setMentionIndex(index)}
                          onMouseDown={event => {
                            event.preventDefault();
                            applyMentionSuggestion(mention.trigger, newDiscPromptRef.current, mention.type);
                          }}
                        >
                          <div className="disc-mention-main">
                            <Cpu size={13} style={{ color: agentColor(mention.type) }} />
                            <span className="font-semibold" style={{ color: agentColor(mention.type) }}>{mention.trigger}</span>
                            <span className="text-muted">{mention.label}</span>
                          </div>
                          <span className="disc-mention-tier-choices" aria-label={t('disc.modelTier')}>
                            {(['economy', 'default', 'reasoning'] as const).map(tier => {
                              const model = modelForAgentTier(
                                mention.type,
                                tier,
                                agentAccess?.model_tiers,
                                t('disc.defaultAgentModel'),
                              );
                              const title = t('disc.routingInvokeTier', t(`disc.tier.${tier}`), model);
                              return (
                                <button
                                  key={tier}
                                  type="button"
                                  className="disc-mention-tier-choice"
                                  data-tier={tier}
                                  data-current={currentTier === tier}
                                  aria-label={`${mention.trigger} · ${title}`}
                                  title={title}
                                  onMouseDown={event => {
                                    event.preventDefault();
                                    event.stopPropagation();
                                    applyMentionSuggestion(mention.trigger, newDiscPromptRef.current, mention.type, tier);
                                  }}
                                >
                                  <span aria-hidden="true">{MODEL_TIER_ICONS[tier]}</span>
                                </button>
                              );
                            })}
                          </span>
                        </div>
                      );
                    })}
                    {unavailable.length > 0 && (
                      <div className="disc-mention-group">{t('disc.routingDisabledAgent')}</div>
                    )}
                    {unavailable.map(mention => (
                      <div key={mention.trigger} className="disc-mention-item disc-mention-item-disabled" aria-disabled="true">
                        <Cpu size={13} style={{ color: agentColor(mention.type) }} />
                        <span className="font-semibold">{mention.trigger}</span>
                        <span className="text-muted">{t('disc.promptAgentUnavailable')}</span>
                      </div>
                    ))}
                  </div>
                );
              })()}

              {emojiMatch && emojiSuggestions.length > 0 && (
                <div className="disc-mention-popover disc-emoji-popover disc-new-autocomplete" role="listbox" aria-label={t('disc.emojiShortcodes')}>
                  {emojiSuggestions.map((suggestion, index) => (
                    <button
                      key={suggestion.shortcode}
                      type="button"
                      role="option"
                      aria-selected={index === emojiIndex}
                      className="disc-mention-item disc-emoji-item"
                      data-highlighted={index === emojiIndex}
                      onMouseEnter={() => setEmojiIndex(index)}
                      onMouseDown={event => {
                        event.preventDefault();
                        applyEmojiSuggestion(suggestion);
                      }}
                    >
                      <span className="disc-emoji-glyph" aria-hidden="true">{suggestion.emoji}</span>
                      <span className="font-semibold text-accent">:{suggestion.shortcode}:</span>
                    </button>
                  ))}
                </div>
              )}

              <MarkdownEditor
                content={newDiscPrompt}
                embedded
                helpTitle={t('disc.composerHelpTitle')}
                helpContent={(
                  <>
                    <div className="md-composer-help-topic">
                      <strong>{t('disc.composerMentions')}</strong>
                      <p>{t('disc.mentionAgentsHelp')}</p>
                    </div>
                  </>
                )}
              >
                <textarea
                  id="disc-new-prompt"
                  ref={newDiscPromptRef}
                  className="disc-textarea-styled"
                  data-locked={newDiscPrefilled}
                  placeholder={t('disc.promptPlaceholder')}
                  value={newDiscPrompt}
                  aria-label={t('disc.prompt')}
                  onChange={e => {
                    if (newDiscPrefilled) return;
                    setNewDiscPrompt(e.target.value);
                    prunePromptAgentTiers(e.target.value);
                    refreshPromptAutocomplete(e.target.value, e.target.selectionStart ?? e.target.value.length);
                  }}
                  onClick={e => refreshPromptAutocomplete(e.currentTarget.value, e.currentTarget.selectionStart ?? e.currentTarget.value.length)}
                  onKeyUp={e => {
                    if ((mentionQuery !== null || emojiMatch) && ['ArrowUp', 'ArrowDown'].includes(e.key)) return;
                    if (['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'].includes(e.key)) {
                      refreshPromptAutocomplete(e.currentTarget.value, e.currentTarget.selectionStart ?? e.currentTarget.value.length);
                    }
                  }}
                  onKeyDown={e => {
                    if (emojiMatch && emojiSuggestions.length > 0) {
                      if (e.key === 'ArrowDown') { e.preventDefault(); setEmojiIndex(index => Math.min(index + 1, emojiSuggestions.length - 1)); return; }
                      if (e.key === 'ArrowUp') { e.preventDefault(); setEmojiIndex(index => Math.max(index - 1, 0)); return; }
                      if (e.key === 'Tab' || e.key === 'Enter') { e.preventDefault(); applyEmojiSuggestion(emojiSuggestions[emojiIndex]); return; }
                      if (e.key === 'Escape') { e.preventDefault(); setEmojiMatch(null); setEmojiSuggestions([]); return; }
                    }
                    if (mentionQuery !== null) {
                      const matching = AGENT_MENTIONS.filter(mention => (
                        installedAgentTypes.has(mention.type)
                        && mention.trigger.slice(1).startsWith(mentionQuery)
                      ));
                      if (e.key === 'ArrowDown') { e.preventDefault(); setMentionIndex(index => Math.min(index + 1, matching.length - 1)); return; }
                      if (e.key === 'ArrowUp') { e.preventDefault(); setMentionIndex(index => Math.max(index - 1, 0)); return; }
                      if ((e.key === 'Tab' || e.key === 'Enter') && matching.length > 0) { e.preventDefault(); applyMentionSuggestion(matching[mentionIndex].trigger, e.currentTarget, matching[mentionIndex].type); return; }
                      if (e.key === 'Escape') { e.preventDefault(); setMentionMatch(null); return; }
                    }
                  }}
                  readOnly={newDiscPrefilled}
                  rows={7}
                  autoFocus={!newDiscPrefilled}
                />
              </MarkdownEditor>
            </div>

            <label className="disc-form-label" style={{ marginTop: 12 }}>{t('disc.title')}</label>
            <input
              className="disc-input-styled"
              data-locked={newDiscPrefilled}
              placeholder={t('disc.titlePlaceholder')}
              value={newDiscTitle}
              aria-label={t('disc.title')}
              onChange={e => {
                if (newDiscPrefilled) return;
                const val = e.target.value;
                setNewDiscTitle(val);
                if (newDiscWorkspaceMode === 'Isolated') {
                  const slug = val.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
                  setNewDiscBranchName(slug || `disc-${Date.now()}`);
                }
              }}
              readOnly={newDiscPrefilled}
            />

            {/* Context files */}
            <div className="disc-new-files-row">
              <input
                type="file"
                multiple
                style={{ display: 'none' }}
                ref={newDiscFileInputRef}
                aria-label={t('disc.attachFiles')}
                onChange={e => {
                  const files = Array.from(e.target.files ?? []);
                  if (files.length > 0) {
                    setPendingFiles(prev => [...prev, ...files]);
                  }
                  e.target.value = '';
                }}
              />
              <button
                type="button"
                className="disc-new-attach-btn"
                onClick={() => newDiscFileInputRef.current?.click()}
              >
                <Paperclip size={12} /> {pendingFiles.length > 0 ? `${pendingFiles.length} ${t('disc.attachFile')}` : t('disc.attachFile')}
              </button>
              {pendingFiles.length > 0 && (
                <div className="disc-new-files-list">
                  {pendingFiles.map((f, i) => (
                    <span key={i} className="disc-context-file-badge">
                      {f.type.startsWith('image/') ? <Image size={10} /> : <FileText size={10} />}
                      <span className="disc-context-file-name">{f.name}</span>
                      <button type="button" className="disc-context-file-remove" onClick={() => setPendingFiles(prev => prev.filter((_, j) => j !== i))}>
                        <X size={9} />
                      </button>
                    </span>
                  ))}
                </div>
              )}
            </div>

            {/* Warnings for validation discussion */}
            {newDiscPrefilled && (
              <div className="disc-audit-warn">
                <p className="disc-audit-warn-title">
                  <AlertTriangle size={11} /> {t('disc.auditWarn')}
                </p>
                <p className="disc-audit-warn-hint">
                  {t('disc.auditHint')}
                </p>
              </div>
            )}
          </section>

          <section className="disc-new-section disc-new-configuration" aria-labelledby="disc-new-config-title">
            <div className="disc-new-section-heading">
              <span className="disc-new-section-icon" aria-hidden="true"><Settings size={14} /></span>
              <span>
                <strong id="disc-new-config-title">{t('disc.configuration')}</strong>
                <small>{t('disc.configurationHint')}</small>
              </span>
            </div>

        {/* No-RTK cost warning: when launching an RTK-capable agent that has no
            active RTK hook, shell output isn't compressed → more tokens burned.
            Red, pinned at the top. Skipped for non-RTK agents (Kiro/Copilot/
            Vibe/Ollama) and when RTK is active. */}
        {launchAgentNow && effectiveLaunchAgents.some(agent => RTK_APPLICABLE.has(agent)) && (() => {
          const warnedAgent = effectiveLaunchAgents.find(agent => {
            if (!RTK_APPLICABLE.has(agent)) return false;
            const detection = agents.find(candidate => candidate.agent_type === agent);
            return !detection || !isRtkActive(detection);
          });
          if (!warnedAgent) return null;
          const sel = agents.find(a => a.agent_type === warnedAgent);
          if (sel && isRtkActive(sel)) return null;
          return (
            <div className="disc-rtk-warn" data-testid="disc-rtk-warn" role="alert">
              <AlertTriangle size={12} style={{ color: 'var(--kr-error)', flexShrink: 0 }} />
              <span className="disc-restricted-warn-text">
                {t('disc.rtkWarn')}
                {' — '}
                <span style={{ cursor: 'pointer', textDecoration: 'underline' }} onClick={() => { onClose(); onNavigate('settings'); }}>{t('disc.rtkWarnLink')}</span>
              </span>
            </div>
          );
        })()}

        <div className="disc-new-grid">
          <div>
            <label className="disc-form-label">{t('disc.project')}</label>
            <select className="disc-select-styled" aria-label={t('disc.project')} data-locked={newDiscPrefilled} value={newDiscProjectId} onChange={e => {
              const pid = e.target.value;
              setNewDiscProjectId(pid);
              const proj = projects.find(p => p.id === pid);
              if (proj?.default_skill_ids?.length) setNewDiscSkillIds(proj.default_skill_ids);
              setNewDiscWorkspaceMode('Direct');
              setNewDiscBranchName('');
              setNewDiscBaseBranch('main');
            }} disabled={newDiscPrefilled}>
              <option value="">{t('disc.noProject')}</option>
              {projects.filter(p => !isHiddenPath(p.path)).map(p => (
                <option key={p.id} value={p.id}>{p.name}</option>
              ))}
            </select>
          </div>
          <div>
            <label className="disc-form-label disc-launch-control">
              <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
                <input
                  type="checkbox"
                  checked={launchAgentNow}
                  onChange={e => setLaunchAgentNow(e.target.checked)}
                  aria-label={t('disc.launchAgentNow')}
                  style={{ margin: 0 }}
                />
                {t('disc.launchAgentNow')}
                {/* 0.8.6 phase 2 — tooltip via native `title` keeps the
                    info discoverable without inflating the form. Copy is
                    23 words, validated user-side 2026-05-20. */}
                <span
                  className="disc-form-info-icon"
                  title={t('disc.launchAgentNowHint')}
                  aria-label={t('disc.launchAgentNowHint')}
                  style={{ cursor: 'help', opacity: 0.7, fontSize: '0.85em' }}
                >
                  ⓘ
                </span>
              </span>
            </label>
            {launchAgentNow ? (
              <div
                className="disc-new-agent-model-picker"
                data-mode={agentLaunchMode}
                data-testid="new-disc-agent-picker"
              >
                {agentLaunchMode === 'prompt' ? (
                  <div className="disc-new-prompt-routing">
                    <button
                      type="button"
                      className="disc-new-prompt-routing-main"
                      title={t('disc.agentFromPromptHint')}
                      disabled
                      aria-disabled="true"
                    >
                      <span aria-hidden="true">@</span>
                      <span>
                        <strong>{t('disc.agentFromPrompt')}</strong>
                        <small>{t('disc.agentFromPromptHint')}</small>
                      </span>
                    </button>
                  </div>
                ) : newDiscAgent ? (
                  <AgentSwitchPicker
                    currentAgent={newDiscAgent}
                    currentTier={newDiscTier}
                    availableAgents={installedAgentsList.map(agent => agent.agent_type)}
                    modelTiers={agentAccess?.model_tiers}
                    defaultModelLabel={t('disc.defaultAgentModel')}
                    tierLabels={{
                      economy: t('disc.tier.economy'),
                      default: t('disc.tier.default'),
                      reasoning: t('disc.tier.reasoning'),
                    }}
                    title={t('disc.agentAndMode')}
                    ariaLabel={t('disc.agentAndMode')}
                    onSelectionChange={async (agent, tier) => {
                      setAgentLaunchMode('selected');
                      setNewDiscAgent(agent);
                      setNewDiscTier(tier);
                    }}
                  />
                ) : (
                  <span className="disc-form-hint">{t('disc.noAgent')}</span>
                )}
              </div>
            ) : (
              <div className="disc-form-hint" style={{ fontSize: '0.85em', opacity: 0.7, padding: '6px 0' }}>
                {t('disc.discFirstHint')}
              </div>
            )}
          </div>
        </div>

        {launchAgentNow && agentLaunchMode === 'prompt' && (
          <div className="disc-prompt-agent-summary" data-testid="prompt-agent-summary">
            <span className="disc-form-hint">{t('disc.promptAgentsDetected')}</span>
            <div className="disc-prompt-agent-chips">
              {promptMentionedAgents.length === 0 ? (
                <span className="disc-form-hint" data-state="missing">
                  {t('disc.promptAgentsMissing')}
                </span>
              ) : promptMentionedAgents.map(agent => {
                const available = installedAgentTypes.has(agent);
                const mention = AGENT_MENTIONS.find(candidate => candidate.type === agent);
                const selectedTier = promptAgentTiers[agent] ?? newDiscTier;
                const selectedModel = modelForAgentTier(
                  agent,
                  selectedTier,
                  agentAccess?.model_tiers,
                  t('disc.defaultAgentModel'),
                );
                return (
                  <span
                    key={agent}
                    className="disc-prompt-agent-chip"
                    data-available={available}
                    title={available
                      ? t('disc.routingInvokeTier', t(`disc.tier.${selectedTier}`), selectedModel)
                      : t('disc.promptAgentUnavailable')}
                  >
                    <span>{mention?.trigger ?? `@${AGENT_LABELS[agent] ?? agent}`}</span>
                    {available && (
                      <span className="disc-prompt-agent-tier">
                        <span aria-hidden="true">{MODEL_TIER_ICONS[selectedTier]}</span>
                        {t(`disc.tier.${selectedTier}`)}
                      </span>
                    )}
                  </span>
                );
              })}
            </div>
            {unavailablePromptAgents.length > 0 && (
              <span className="disc-form-error" role="alert">
                {t(
                  'disc.promptAgentsUnavailable',
                  unavailablePromptAgents
                    .map(agent => AGENT_LABELS[agent] ?? agent)
                    .join(', '),
                )}
              </span>
            )}
            {promptLaunchAgents.length > 1 && unavailablePromptAgents.length === 0 && (
              <div
                className="disc-prompt-multi-agent-mode"
                data-collaboration={agentHandoffsEnabled === true ? 'enabled' : 'disabled'}
                data-testid="prompt-multi-agent-mode"
              >
                <MessageSquare size={14} aria-hidden="true" />
                <span>
                  <strong>{t('disc.multiAgentIndependentTitle')}</strong>
                  {' '}{t('disc.multiAgentIndependentHint', promptLaunchAgents.length)}
                  {agentHandoffsEnabled !== null && (
                    <small>
                      {agentHandoffsEnabled
                        ? t('disc.multiAgentCollaborationEnabled')
                        : t('disc.multiAgentCollaborationDisabled')}
                      {' '}
                      <button
                        type="button"
                        onClick={() => {
                          onClose();
                          onNavigate('settings', { scrollTo: 'settings-agent-handoffs' });
                        }}
                      >
                        {t('disc.multiAgentCollaborationSettings')}
                      </button>
                    </small>
                  )}
                </span>
              </div>
            )}
          </div>
        )}

        {effectiveLaunchAgents.some(agent => isAgentRestricted(agent)) && (
          <div className="disc-restricted-warn">
            <AlertTriangle size={11} style={{ color: 'var(--kr-warning)', flexShrink: 0 }} />
            <span className="disc-restricted-warn-text">
              {t(
                'config.restrictedAgent',
                effectiveLaunchAgents
                  .filter(agent => isAgentRestricted(agent))
                  .map(agent => AGENT_LABELS[agent] ?? agent)
                  .join(', '),
              )}
              {' — '}
              <span style={{ cursor: 'pointer', textDecoration: 'underline' }} onClick={() => { onClose(); onNavigate('settings'); }}>{t('config.restrictedAgentLink')}</span>
            </span>
          </div>
        )}

        {/* Workspace mode toggle — always shown when a project is selected.
            Previously hidden when `repo_url` was null/empty (non-git projects),
            but that made the option silently disappear for users who couldn't
            tell why. Now always visible: for non-git projects, Isolated mode
            is disabled with a hint explaining the requirement. */}
        {(() => {
          const selectedProj = projects.find(p => p.id === newDiscProjectId);
          if (!newDiscProjectId) return null; // no project → no workspace choice
          const hasRepo = Boolean(selectedProj?.repo_url);
          return (
            <div style={{ marginBottom: 12 }}>
              <label className="disc-form-label">{t('disc.workspaceLabel')}</label>
              <div className="disc-workspace-toggle">
                <button
                  type="button"
                  className="disc-workspace-btn"
                  data-active={newDiscWorkspaceMode === 'Direct'}
                  data-mode="direct"
                  onClick={() => { setNewDiscWorkspaceMode('Direct'); setNewDiscBranchName(''); }}
                >
                  <Folder size={12} />
                  <div>
                    <div className="disc-workspace-btn-title">{t('disc.workspaceDirect')}</div>
                    <div className="disc-workspace-btn-desc">{t('disc.workspaceDirectDesc')}</div>
                  </div>
                </button>
                <button
                  type="button"
                  disabled={!hasRepo}
                  onClick={() => {
                    if (!hasRepo) return;
                    setNewDiscWorkspaceMode('Isolated');
                    if (!newDiscBranchName) {
                      const title = newDiscTitle.trim();
                      const slug = title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
                      setNewDiscBranchName(slug || `disc-${Date.now()}`);
                    }
                  }}
                  className="disc-workspace-btn"
                  data-active={newDiscWorkspaceMode === 'Isolated'}
                  data-mode="isolated"
                  title={hasRepo ? undefined : t('disc.workspaceIsolatedNeedsRepo')}
                  style={!hasRepo ? { opacity: 0.5, cursor: 'not-allowed' } : undefined}
                >
                  <GitBranch size={12} />
                  <div>
                    <div className="disc-workspace-btn-title">{t('disc.workspaceIsolated')}</div>
                    <div className="disc-workspace-btn-desc">
                      {hasRepo ? t('disc.workspaceIsolatedDesc') : t('disc.workspaceIsolatedNeedsRepo')}
                    </div>
                  </div>
                </button>
              </div>
              {newDiscWorkspaceMode === 'Isolated' && hasRepo && (
                <div className="disc-workspace-branch-grid">
                  <div>
                    <label className="disc-form-label" data-size="xs">{t('disc.branchName')}</label>
                    <input
                      className="disc-input-styled"
                      value={newDiscBranchName}
                      onChange={e => setNewDiscBranchName(e.target.value)}
                      placeholder="feature/my-branch"
                      aria-label={t('disc.branchName')}
                    />
                  </div>
                  <div>
                    <label className="disc-form-label" data-size="xs">{t('disc.baseBranch')}</label>
                    <input
                      className="disc-input-styled"
                      value={newDiscBaseBranch}
                      onChange={e => setNewDiscBaseBranch(e.target.value)}
                      placeholder="main"
                      aria-label={t('disc.baseBranch')}
                    />
                  </div>
                </div>
              )}
            </div>
          );
        })()}

        {/* Advanced options (collapsible) */}
        {(availableSkills.length > 0 || availableProfiles.length > 0 || availableDirectives.length > 0) && (
          <div style={{ marginBottom: 12 }}>
            <button
              type="button"
              className="disc-advanced-toggle"
              onClick={() => setShowAdvancedOptions(prev => !prev)}
              aria-expanded={showAdvancedOptions}
              aria-label={t('disc.advancedOptions')}
            >
              <ChevronRight size={11} className="disc-chevron" data-expanded={showAdvancedOptions} />
              <Settings size={10} />
              {t('disc.advancedOptions')}
              {(newDiscSkillIds.length > 0 || newDiscProfileIds.length > 0 || newDiscDirectiveIds.length > 0) && (
                <span className="disc-advanced-count">
                  ({newDiscSkillIds.length + newDiscProfileIds.length + newDiscDirectiveIds.length})
                </span>
              )}
            </button>

            {showAdvancedOptions && (
              <div className="disc-advanced-panel">

                {/* Skills accordion */}
                {availableSkills.length > 0 && (
                  <div className="disc-advanced-section">
                    <button type="button" className="disc-advanced-section-toggle" onClick={() => setExpandedAdvanced(prev => prev === 'skills' ? null : 'skills')}>
                      <ChevronRight size={9} className="disc-chevron" data-expanded={expandedAdvanced === 'skills'} />
                      <Zap size={10} />
                      <span>{t('skills.selectSkills')}</span>
                      {newDiscSkillIds.length > 0 && <span className="disc-advanced-count">{newDiscSkillIds.length}</span>}
                    </button>
                    {expandedAdvanced === 'skills' && (
                      <div className="disc-advanced-chips">
                        {availableSkills.map(skill => {
                          const selected = newDiscSkillIds.includes(skill.id);
                          return (
                            <button key={skill.id} type="button" className="disc-chip" data-active={selected} data-color="accent"
                              onClick={() => setNewDiscSkillIds(prev => selected ? prev.filter(id => id !== skill.id) : [...prev, skill.id])}
                              title={skill.description || skill.name}
                            >
                              {selected && <Check size={9} />} {skill.name}
                            </button>
                          );
                        })}
                      </div>
                    )}
                  </div>
                )}

                {/* Profiles accordion */}
                {availableProfiles.length > 0 && (
                  <div className="disc-advanced-section" data-tour-id="disc-form-profiles">
                    <button
                      type="button"
                      className="disc-advanced-section-toggle"
                      data-tour-id="disc-form-profiles-toggle"
                      onClick={() => setExpandedAdvanced(prev => prev === 'profiles' ? null : 'profiles')}
                    >
                      <ChevronRight size={9} className="disc-chevron" data-expanded={expandedAdvanced === 'profiles'} />
                      <UserCircle size={10} />
                      <span>{t('profiles.select')}</span>
                      {newDiscProfileIds.length > 0 && <span className="disc-advanced-count">{newDiscProfileIds.length}</span>}
                    </button>
                    {expandedAdvanced === 'profiles' && (
                      <div className="disc-advanced-chips">
                        <button type="button" className="disc-chip" data-active={newDiscProfileIds.length === 0} data-color="purple" onClick={() => setNewDiscProfileIds([])}>
                          {t('profiles.none')}
                        </button>
                        {availableProfiles.map((profile, idx) => {
                          const selected = newDiscProfileIds.includes(profile.id);
                          return (
                            <ProfileTooltip key={profile.id} profile={profile}>
                              <button
                                type="button"
                                className="disc-chip"
                                data-active={selected}
                                data-color="purple"
                                // First chip gets a stable tour id so the
                                // guided tour can anchor on a real element.
                                {...(idx === 0 ? { 'data-tour-id': 'disc-form-profile-chip' } : {})}
                                onClick={() => setNewDiscProfileIds(prev => selected ? prev.filter(id => id !== profile.id) : [...prev, profile.id])}
                                style={selected && profile.color ? { borderColor: profile.color, background: `${profile.color}15`, color: profile.color } : undefined}
                              >
                                {selected && <Check size={9} />} {profile.avatar} {profile.persona_name || profile.name}
                              </button>
                            </ProfileTooltip>
                          );
                        })}
                      </div>
                    )}
                  </div>
                )}

                {/* Directives accordion */}
                {availableDirectives.length > 0 && (
                  <div className="disc-advanced-section">
                    <button type="button" className="disc-advanced-section-toggle" onClick={() => setExpandedAdvanced(prev => prev === 'directives' ? null : 'directives')}>
                      <ChevronRight size={9} className="disc-chevron" data-expanded={expandedAdvanced === 'directives'} />
                      <FileText size={10} />
                      <span>{t('directives.title')}</span>
                      {newDiscDirectiveIds.length > 0 && <span className="disc-advanced-count">{newDiscDirectiveIds.length}</span>}
                    </button>
                    {expandedAdvanced === 'directives' && (
                      <div className="disc-advanced-chips">
                        {availableDirectives.map(directive => {
                          const selected = newDiscDirectiveIds.includes(directive.id);
                          return (
                            <button key={directive.id} type="button" className="disc-chip" data-active={selected} data-color="warning"
                              onClick={() => setNewDiscDirectiveIds(prev => selected ? prev.filter(id => id !== directive.id) : [...prev, directive.id])}
                              title={directive.description || directive.name}
                            >
                              {selected && <Check size={9} />} {directive.icon} {directive.name}
                            </button>
                          );
                        })}
                      </div>
                    )}
                  </div>
                )}
              </div>
            )}
          </div>
        )}
          </section>
        </div>

        <div className="disc-new-footer">
          <span className="disc-new-footer-hint">{t('disc.createHint')}</span>
          <button
            className="disc-create-btn"
            data-ready={launchAgentNow ? launchReady : (!!newDiscPrompt.trim() || !!newDiscTitle.trim())}
            onClick={handleCreate}
            // 0.8.6 phase 2 — disc-first mode allows submitting WITHOUT a
            // prompt (just a title) since the agent will be invited later.
            // Launch mode keeps the legacy gates.
            disabled={
              creating ||
              (launchAgentNow
                ? !launchReady
                : !newDiscPrompt.trim() && !newDiscTitle.trim())
            }
          >
            <MessageSquare size={14} /> {launchAgentNow ? t('disc.start') : t('disc.createEmpty')}
            <span className="disc-create-shortcut">Ctrl+Enter</span>
          </button>
        </div>
      </div>
    </div>
  );
}
