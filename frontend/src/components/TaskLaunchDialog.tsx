import { useEffect, useMemo, useRef, useState } from 'react';
import { Loader2, X } from 'lucide-react';
import {
  agents,
  orchestration,
  profiles,
  type CampaignView,
} from '../lib/api';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import { orchestrationResolution } from './taskLaunchResolution';
import type {
  AgentDetection,
  AgentProfile,
  AgentType,
  DiscussionWorkspace,
} from '../types/generated';

interface Props {
  open: boolean;
  discussionId: string;
  projectId: string | null;
  taskReference: string;
  defaultAgent: AgentType;
  defaultBranch?: string | null;
  workspaces: DiscussionWorkspace[];
  campaign: CampaignView | null;
  onClose: () => void;
  onLaunched: (executionId: string, campaign: CampaignView) => void;
}

function usable(detection: AgentDetection): boolean {
  return detection.enabled
    && (detection.auth_ready ?? true)
    && (detection.installed || detection.runtime_available);
}

export function TaskLaunchDialog({
  ...props
}: Props) {
  if (!props.open) return null;
  const formIdentity = `${props.taskReference}:${props.campaign?.run.id ?? 'new'}`;
  return <TaskLaunchDialogContent key={formIdentity} {...props} />;
}

function TaskLaunchDialogContent({
  discussionId,
  projectId,
  taskReference,
  defaultAgent,
  defaultBranch,
  workspaces,
  campaign,
  onClose,
  onLaunched,
}: Props) {
  const { t } = useT();
  const inFlight = useRef(false);
  const dialogRef = useRef<HTMLElement>(null);
  const [detections, setDetections] = useState<AgentDetection[]>([]);
  const [availableProfiles, setAvailableProfiles] = useState<AgentProfile[]>([]);
  const selected = campaign?.run.default_worker;
  const initialAgent = selected?.target.agent_type ?? defaultAgent;
  const [agent, setAgent] = useState<AgentType>(initialAgent);
  const [allowedAgents, setAllowedAgents] = useState<AgentType[]>(
    campaign?.run.allowed_agents.length ? campaign.run.allowed_agents : [initialAgent],
  );
  const [model, setModel] = useState(selected?.model ?? '');
  const [profileId, setProfileId] = useState(selected?.profile_id ?? '');
  const [branch, setBranch] = useState(campaign?.run.target_branch ?? defaultBranch ?? 'main');
  const [workspaceId, setWorkspaceId] = useState(campaign?.run.target_workspace_id ?? '');
  const [maxReviewRounds, setMaxReviewRounds] = useState(campaign?.run.max_review_rounds ?? 3);
  const [validations, setValidations] = useState(
    campaign?.run.validations.map(item => item.command).join('\n') ?? '',
  );
  const [autoContinue, setAutoContinue] = useState(campaign?.run.auto_continue ?? false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    let active = true;
    Promise.all([agents.detect(), profiles.list()])
      .then(([nextAgents, nextProfiles]) => {
        if (!active) return;
        setDetections(nextAgents.filter(usable));
        setAvailableProfiles(nextProfiles);
      })
      .catch(cause => {
        if (active) setError(userError(cause));
      });
    return () => { active = false; };
  }, []);

  useEffect(() => {
    const previousFocus = document.activeElement as HTMLElement | null;
    dialogRef.current?.querySelector<HTMLElement>('select, input, button, textarea')?.focus();
    const close = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !inFlight.current) onClose();
      if (event.key !== 'Tab' || !dialogRef.current) return;
      const focusable = Array.from(dialogRef.current.querySelectorAll<HTMLElement>(
        'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled)',
      ));
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    window.addEventListener('keydown', close);
    return () => {
      window.removeEventListener('keydown', close);
      previousFocus?.focus();
    };
  }, [onClose]);

  // Profiles are provider-agnostic persona overlays. The backend deliberately
  // exposes no compatibility matrix, so hiding one here would invent policy.
  const profilesForAgent = useMemo(() => availableProfiles, [availableProfiles]);

  const policyLocked = campaign !== null;

  const submit = async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError('');
    try {
      const worker = {
        target: { kind: 'agent' as const, agent_type: agent, cli_session_id: null, tier: null },
        model: model.trim() || null,
        profile_id: profileId || null,
      };
      const activeCampaign = campaign ?? await orchestration.createCampaign({
        discussion_id: discussionId,
        project_id: projectId,
        target_workspace_id: workspaceId || null,
        target_branch: branch.trim() || 'main',
        integration_strategy: 'two_phase_ff_only',
        max_review_rounds: maxReviewRounds,
        max_concurrent_executions: 1,
        max_cli_concurrent_executions: 1,
        validations: validations
          .split('\n')
          .map(command => command.trim())
          .filter(Boolean)
          .map(command => ({ command, quick_exec_id: null, timeout_secs: null })),
        allowed_agents: Array.from(new Set([...allowedAgents, agent])),
        default_worker: worker,
        cancellation_cleanup_policy: 'preserve',
        auto_continue: autoContinue,
      });
      const result = await orchestration.launch(activeCampaign.run.id, taskReference, {
        idempotencyKey: `ui:${activeCampaign.run.id}:${taskReference}`,
        worker,
      });
      onLaunched(result.execution.id, activeCampaign);
    } catch (cause) {
      setError(userError(cause));
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };

  return (
    <div className="orch-launch-backdrop" role="presentation" onMouseDown={event => {
      if (event.target === event.currentTarget && !busy) onClose();
    }}>
      <section
        ref={dialogRef}
        className="orch-launch-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="orch-launch-title"
      >
        <header>
          <div>
            <h3 id="orch-launch-title">{t('orch.config.title')}</h3>
            <span>{taskReference}</span>
          </div>
          <button type="button" className="btn btn-ghost btn-sm" onClick={onClose} disabled={busy} aria-label={t('common.close')}>
            <X size={15} />
          </button>
        </header>

        {policyLocked && <p className="orch-policy-note">{t('orch.config.existingPolicy')}</p>}

        <div className="orch-launch-grid">
          <label>
            <span>{t('orch.config.agent')}</span>
            <select value={agent} onChange={event => setAgent(event.target.value as AgentType)}>
              {(detections.length ? detections.map(item => item.agent_type) : [defaultAgent])
                .filter((value, index, list) => list.indexOf(value) === index)
                .map(value => <option value={value} key={value}>{value}</option>)}
            </select>
          </label>
          <label>
            <span>{t('orch.config.model')}</span>
            <input value={model} onChange={event => setModel(event.target.value)} placeholder={t('orch.config.modelDefault')} />
          </label>
          <label>
            <span>{t('orch.config.profile')}</span>
            <select value={profileId} onChange={event => setProfileId(event.target.value)}>
              <option value="">{t('orch.config.noProfile')}</option>
              {profilesForAgent.map(profile => <option value={profile.id} key={profile.id}>{profile.name}</option>)}
            </select>
          </label>
          <label>
            <span>{t('orch.config.workspace')}</span>
            <select value={workspaceId} disabled={policyLocked} onChange={event => setWorkspaceId(event.target.value)}>
              <option value="">{t('orch.config.projectWorkspace')}</option>
              {workspaces.map(workspace => (
                <option value={workspace.id} key={workspace.id}>{workspace.branch} · {workspace.ownership}</option>
              ))}
            </select>
          </label>
          <label>
            <span>{t('orch.config.targetBranch')}</span>
            <input value={branch} disabled={policyLocked} onChange={event => setBranch(event.target.value)} />
          </label>
          <label>
            <span>{t('orch.config.gitStrategy')}</span>
            <select value="two_phase_ff_only" disabled>
              <option value="two_phase_ff_only">{t('orch.config.twoPhase')}</option>
            </select>
          </label>
          <label>
            <span>{t('orch.config.reviewRounds')}</span>
            <input
              type="number"
              min={1}
              max={20}
              value={maxReviewRounds}
              disabled={policyLocked}
              onChange={event => setMaxReviewRounds(Number(event.target.value) || 1)}
            />
          </label>
          <label className="orch-launch-validations">
            <span>{t('orch.config.validations')}</span>
            <textarea
              value={validations}
              disabled={policyLocked}
              onChange={event => setValidations(event.target.value)}
              placeholder={t('orch.config.validationsHint')}
            />
          </label>
        </div>

        {!policyLocked && detections.length > 1 && (
          <fieldset className="orch-launch-fallbacks">
            <legend>{t('orch.config.fallbacks')}</legend>
            {detections.map(item => (
              <label key={item.agent_type}>
                <input
                  type="checkbox"
                  checked={allowedAgents.includes(item.agent_type)}
                  onChange={event => setAllowedAgents(previous => event.target.checked
                    ? Array.from(new Set([...previous, item.agent_type]))
                    : previous.filter(value => value !== item.agent_type))}
                />
                {item.agent_type}
              </label>
            ))}
          </fieldset>
        )}

        <label className="orch-launch-auto">
          <input type="checkbox" checked={autoContinue} disabled={policyLocked} onChange={event => setAutoContinue(event.target.checked)} />
          {t('orch.config.autoContinue')}
        </label>

        {error && (
          <div className="orch-launch-error" role="alert">
            <strong>{error}</strong>
            <span>{t(`orch.resolution.${orchestrationResolution(error)}`)}</span>
          </div>
        )}

        <footer>
          <button type="button" className="btn btn-ghost" onClick={onClose} disabled={busy}>{t('common.cancel')}</button>
          <button type="button" className="btn btn-primary" onClick={() => void submit()} disabled={busy || !branch.trim()}>
            {busy && <Loader2 size={13} className="spin" />}
            {busy ? t('orch.launching') : t('orch.launch')}
          </button>
        </footer>
      </section>
    </div>
  );
}
