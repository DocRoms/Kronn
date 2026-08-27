import {
  ArrowDown, ArrowUp, Bot, CheckCircle2, ChevronDown, Clock3, Hash, Loader2,
  Lightbulb, MinusCircle, Sparkles, Star, ThumbsDown, ThumbsUp, X,
} from 'lucide-react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { workflows as workflowsApi } from '../lib/api';
import { AGENT_LABELS, agentColor, modelForAgentTier } from '../lib/constants';
import type {
  AgentType, BatchCompareDetails, BatchCompareEvaluation, Discussion,
  ModelTier, ModelTiersConfig,
} from '../types/generated';
import { AgentSwitchPicker } from './AgentSwitchPicker';

type RankingMetric = 'weighted' | 'ai' | 'human' | 'duration' | 'tokens';
type RankingDirection = 'asc' | 'desc';

interface BatchCompareDetailsPanelProps {
  runId: string;
  discussions: Discussion[];
  availableAgents: AgentType[];
  modelTiers?: ModelTiersConfig | null;
  onOpenDiscussion: (discussionId: string) => void;
  onClose: () => void;
  t: (key: string, ...args: (string | number)[]) => string;
}

function lastAgentAnswer(discussion: Discussion) {
  for (let index = discussion.messages.length - 1; index >= 0; index -= 1) {
    const message = discussion.messages[index];
    if (message.role === 'Agent' && !message.recovered_partial) return message;
  }
  return null;
}

function lastSystemCause(discussion: Discussion) {
  for (let index = discussion.messages.length - 1; index >= 0; index -= 1) {
    const message = discussion.messages[index];
    if (message.role === 'System' && message.content.trim()) return message.content.trim();
  }
  return null;
}

function weightedQuality(
  evaluation: BatchCompareEvaluation | undefined,
  humanWeight: number,
) {
  const human = evaluation?.manual_score ?? null;
  const ai = evaluation?.ai?.score ?? null;
  const aiWeight = 100 - humanWeight;
  const weighted = (human == null ? 0 : human * humanWeight)
    + (ai == null ? 0 : ai * aiWeight);
  const availableWeight = (human == null ? 0 : humanWeight) + (ai == null ? 0 : aiWeight);
  return availableWeight > 0 ? weighted / availableWeight : null;
}

function stars(score: number | null | undefined) {
  return Array.from({ length: 5 }, (_, index) => index < (score ?? 0));
}

function formatDuration(durationMs: number | null | undefined) {
  if (durationMs == null) return '—';
  const seconds = durationMs / 1000;
  return seconds < 60
    ? `${seconds.toFixed(1)} s`
    : `${Math.floor(seconds / 60)} min ${Math.round(seconds % 60)} s`;
}

function isStructurallyToolFreeJudge(agent: AgentType) {
  return agent === 'Ollama' || agent === 'LiteLlm' || agent === 'Nvidia';
}

export function BatchCompareDetailsPanel({
  runId,
  discussions,
  availableAgents,
  modelTiers,
  onOpenDiscussion,
  onClose,
  t,
}: BatchCompareDetailsPanelProps) {
  const [details, setDetails] = useState<BatchCompareDetails | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [metric, setMetric] = useState<RankingMetric>('weighted');
  const [direction, setDirection] = useState<RankingDirection>('desc');
  const [humanWeight, setHumanWeight] = useState(50);
  const [savingDiscussionId, setSavingDiscussionId] = useState<string | null>(null);
  const defaultJudge = availableAgents.includes('Ollama') ? 'Ollama' : availableAgents[0] ?? 'Ollama';
  const [judgeAgent, setJudgeAgent] = useState<AgentType>(defaultJudge);
  const [judgeTier, setJudgeTier] = useState<ModelTier>('reasoning');
  const [startingJudge, setStartingJudge] = useState(false);
  const defaultImprover = availableAgents.includes('Codex') ? 'Codex' : availableAgents[0] ?? 'Ollama';
  const [improverAgent, setImproverAgent] = useState<AgentType>(defaultImprover);
  const [improverTier, setImproverTier] = useState<ModelTier>('reasoning');
  const [startingImprovement, setStartingImprovement] = useState(false);

  const load = useCallback(async (showLoader = false) => {
    if (showLoader) setLoading(true);
    try {
      const next = await workflowsApi.getBatchCompareDetails(runId);
      setDetails(next);
      setError(null);
    } catch (loadError) {
      setError(loadError instanceof Error ? loadError.message : String(loadError));
    } finally {
      setLoading(false);
    }
  }, [runId]);

  useEffect(() => {
    const timer = window.setTimeout(() => { void load(); }, 0);
    return () => window.clearTimeout(timer);
  }, [load]);

  useEffect(() => {
    if (details?.latest_judge_run?.status !== 'Running') return;
    const timer = window.setInterval(() => { void load(); }, 2_000);
    return () => window.clearInterval(timer);
  }, [details?.latest_judge_run?.status, load]);

  const evaluations = useMemo(
    () => new Map(details?.evaluations.map(evaluation => [evaluation.discussion_id, evaluation]) ?? []),
    [details],
  );

  const ranked = useMemo(() => discussions.map(discussion => {
    const evaluation = evaluations.get(discussion.id);
    const answer = lastAgentAnswer(discussion);
    const value = (() => {
      switch (metric) {
        case 'weighted': return weightedQuality(evaluation, humanWeight);
        case 'ai': return evaluation?.ai?.score ?? null;
        case 'human': return evaluation?.manual_score ?? null;
        case 'duration': return answer?.duration_ms ?? null;
        case 'tokens': return answer?.tokens_used && answer.tokens_used > 0 ? answer.tokens_used : null;
      }
    })();
    return { discussion, evaluation, answer, value };
  }).sort((left, right) => {
    if (left.value == null && right.value == null) return 0;
    if (left.value == null) return 1;
    if (right.value == null) return -1;
    return direction === 'asc' ? left.value - right.value : right.value - left.value;
  }), [direction, discussions, evaluations, humanWeight, metric]);

  const changeMetric = (next: RankingMetric) => {
    setMetric(next);
    setDirection(next === 'duration' || next === 'tokens' ? 'asc' : 'desc');
  };

  const saveManual = async (discussionId: string, score: number | null) => {
    setSavingDiscussionId(discussionId);
    try {
      const next = await workflowsApi.updateBatchCompareManualScore(runId, discussionId, score);
      setDetails(next);
      setError(null);
    } catch (saveError) {
      setError(saveError instanceof Error ? saveError.message : String(saveError));
    } finally {
      setSavingDiscussionId(null);
    }
  };

  const launchJudge = async () => {
    setStartingJudge(true);
    try {
      await workflowsApi.startBatchCompareJudge(runId, { agent: judgeAgent, tier: judgeTier });
      await load();
      setError(null);
    } catch (judgeError) {
      setError(judgeError instanceof Error ? judgeError.message : String(judgeError));
    } finally {
      setStartingJudge(false);
    }
  };

  const launchImprovement = async () => {
    setStartingImprovement(true);
    try {
      const response = await workflowsApi.startBatchCompareImprovement(runId, {
        agent: improverAgent,
        tier: improverTier,
      });
      setError(null);
      onOpenDiscussion(response.discussion_id);
    } catch (improvementError) {
      setError(improvementError instanceof Error ? improvementError.message : String(improvementError));
    } finally {
      setStartingImprovement(false);
    }
  };

  const formatRankValue = (value: number | null) => {
    if (value == null) return '—';
    if (metric === 'duration') {
      const seconds = value / 1000;
      return seconds < 60 ? `${seconds.toFixed(1)} s` : `${Math.floor(seconds / 60)} min ${Math.round(seconds % 60)} s`;
    }
    if (metric === 'tokens') return Math.round(value).toLocaleString();
    return `${value.toFixed(value % 1 === 0 ? 0 : 2)} / 5`;
  };

  const judge = details?.latest_judge_run;
  const judgeUnavailableReason = details?.prompt_compatibility === 'different'
    ? t('disc.compare.judgeDisabledDifferentPrompts')
    : details?.prompt_compatibility === 'missing'
      ? t('disc.compare.judgeDisabledMissingPrompt')
      : null;
  const improvementUnavailableReason = (() => {
    switch (details?.improvement_availability) {
      case 'different_prompts': return t('disc.compare.improveDisabledDifferentPrompts');
      case 'missing_prompt': return t('disc.compare.improveDisabledMissingPrompt');
      case 'no_shared_quick_prompt': return t('disc.compare.improveDisabledNoSharedQp');
      default: return null;
    }
  })();

  return (
    <aside className="disc-compare-details" aria-labelledby="disc-compare-details-title">
      <header className="disc-compare-details-head">
        <div>
          <h3 id="disc-compare-details-title">{t('disc.compare.detailsTitle')}</h3>
          <p>{t('disc.compare.detailsSubtitle')}</p>
        </div>
        <button type="button" className="btn btn-ghost btn-icon" onClick={onClose} aria-label={t('common.close')}>
          <X size={16} />
        </button>
      </header>

      {error && <div className="disc-compare-details-error">{error}</div>}
      {loading && !details ? (
        <div className="disc-compare-state"><Loader2 size={16} className="spin" />{t('common.loading')}</div>
      ) : (
        <div className="disc-compare-details-body">
          <section className="disc-compare-ranking-controls" aria-label={t('disc.compare.ranking')}>
            <label>
              <span>{t('disc.compare.rankBy')}</span>
              <select value={metric} onChange={event => changeMetric(event.target.value as RankingMetric)}>
                <option value="weighted">{t('disc.compare.rankWeighted')}</option>
                <option value="ai">{t('disc.compare.rankAi')}</option>
                <option value="human">{t('disc.compare.rankHuman')}</option>
                <option value="duration">{t('disc.compare.rankDuration')}</option>
                <option value="tokens">{t('disc.compare.rankTokens')}</option>
              </select>
            </label>
            <button
              type="button"
              className="btn btn-sm btn-ghost"
              onClick={() => setDirection(value => value === 'asc' ? 'desc' : 'asc')}
              aria-label={t('disc.compare.rankDirection')}
            >
              {direction === 'asc' ? <ArrowUp size={14} /> : <ArrowDown size={14} />}
              {direction === 'asc' ? t('disc.compare.ascending') : t('disc.compare.descending')}
            </button>
          </section>

          {metric === 'weighted' && (
            <section className="disc-compare-weight-control">
              <div>
                <span>{t('disc.compare.humanWeight')}</span><strong>{humanWeight}%</strong>
                <span>{t('disc.compare.aiWeight')}</span><strong>{100 - humanWeight}%</strong>
              </div>
              <input
                type="range"
                min="0"
                max="100"
                step="5"
                value={humanWeight}
                onChange={event => setHumanWeight(Number(event.target.value))}
                aria-label={t('disc.compare.humanWeight')}
              />
              <small>{t('disc.compare.missingScoreRule')}</small>
            </section>
          )}

          <section className="disc-compare-judge-card" data-disabled={judgeUnavailableReason != null}>
            <div className="disc-compare-judge-heading">
              <Bot size={16} />
              <div><strong>{t('disc.compare.aiJudge')}</strong><small>{t('disc.compare.blindJudge')}</small></div>
            </div>
            <div className="disc-compare-judge-actions">
              <AgentSwitchPicker
                currentAgent={judgeAgent}
                availableAgents={availableAgents}
                currentTier={judgeTier}
                onSelectionChange={async (agent, tier) => {
                  setJudgeAgent(agent);
                  setJudgeTier(tier);
                }}
                modelTiers={modelTiers}
                title={t('disc.compare.chooseJudge')}
                ariaLabel={t('disc.compare.chooseJudge')}
                disabled={judgeUnavailableReason != null}
                compact
              />
              <button
                type="button"
                className="btn btn-sm btn-primary"
                disabled={availableAgents.length === 0 || startingJudge || judge?.status === 'Running' || judgeUnavailableReason != null}
                onClick={() => { void launchJudge(); }}
              >
                {startingJudge || judge?.status === 'Running'
                  ? <Loader2 size={13} className="spin" />
                  : <Sparkles size={13} />}
                {judge?.status === 'Running' ? t('disc.compare.judging') : t('disc.compare.launchJudge')}
              </button>
            </div>
            {judge && (
              <div className="disc-compare-judge-status" data-status={judge.status.toLowerCase()}>
                {judge.status === 'Completed' ? <CheckCircle2 size={13} /> : judge.status === 'Failed' ? <MinusCircle size={13} /> : <Loader2 size={13} className="spin" />}
                <span>{AGENT_LABELS[judge.judge_agent] ?? judge.judge_agent} · {judge.rubric_version}</span>
                {judge.tokens_used != null && <span><Hash size={11} />{judge.tokens_used.toLocaleString()}</span>}
                {judge.duration_ms != null && <span><Clock3 size={11} />{(judge.duration_ms / 1000).toFixed(1)} s</span>}
                {judge.error && <small>{judge.error}</small>}
                {judge.self_evaluation && <small data-kind="warning">{t('disc.compare.selfJudgeWarning')}</small>}
              </div>
            )}
            {!isStructurallyToolFreeJudge(judgeAgent) && (
              <small data-kind="warning">{t('disc.compare.cliJudgeWarning')}</small>
            )}
            {judgeUnavailableReason && <small data-kind="warning">{judgeUnavailableReason}</small>}
          </section>

          <section className="disc-compare-improvement-card" data-disabled={improvementUnavailableReason != null}>
            <div className="disc-compare-improvement-heading">
              <Lightbulb size={16} />
              <div>
                <strong>{t('disc.compare.improvePrompt')}</strong>
                <small>{t('disc.compare.improvePromptHint')}</small>
              </div>
              {judge?.prompt_review?.worth_improving && <span>{t('disc.compare.improvementRecommended')}</span>}
            </div>
            {judge?.prompt_review && (
              <details className="disc-compare-prompt-review">
                <summary><ChevronDown size={13} />{t('disc.compare.promptReview')}</summary>
                {judge.prompt_review.strengths.length > 0 && (
                  <div data-kind="positive"><strong>{t('disc.compare.promptStrengths')}</strong><ul>{judge.prompt_review.strengths.map(point => <li key={point}>{point}</li>)}</ul></div>
                )}
                {judge.prompt_review.weaknesses.length > 0 && (
                  <div data-kind="negative"><strong>{t('disc.compare.promptWeaknesses')}</strong><ul>{judge.prompt_review.weaknesses.map(point => <li key={point.text}>{point.text} <small>{t(point.affects === 'all' ? 'disc.compare.affectsAll' : 'disc.compare.affectsSome')}</small></li>)}</ul></div>
                )}
                {judge.prompt_review.recommendations.length > 0 && (
                  <div data-kind="recommendation"><strong>{t('disc.compare.promptRecommendations')}</strong><ul>{judge.prompt_review.recommendations.map(point => <li key={point.text}>{point.text} <small>{t(point.affects === 'all' ? 'disc.compare.affectsAll' : 'disc.compare.affectsSome')}</small></li>)}</ul></div>
                )}
              </details>
            )}
            <div className="disc-compare-improvement-actions">
              <AgentSwitchPicker
                currentAgent={improverAgent}
                availableAgents={availableAgents}
                currentTier={improverTier}
                onSelectionChange={async (agent, tier) => {
                  setImproverAgent(agent);
                  setImproverTier(tier);
                }}
                modelTiers={modelTiers}
                title={t('disc.compare.chooseImprover')}
                ariaLabel={t('disc.compare.chooseImprover')}
                disabled={improvementUnavailableReason != null}
                compact
              />
              <button
                type="button"
                className="btn btn-sm btn-primary"
                disabled={startingImprovement || availableAgents.length === 0 || improvementUnavailableReason != null}
                onClick={() => { void launchImprovement(); }}
              >
                {startingImprovement ? <Loader2 size={13} className="spin" /> : <Sparkles size={13} />}
                {t('disc.compare.improvePrompt')}
              </button>
            </div>
            {improvementUnavailableReason && <small data-kind="warning">{improvementUnavailableReason}</small>}
          </section>

          <ol className="disc-compare-ranking-list">
            {ranked.map((entry, index) => {
              const { discussion, evaluation, answer, value } = entry;
              const manualScore = evaluation?.manual_score ?? null;
              const ai = evaluation?.ai;
              const agentLabel = AGENT_LABELS[discussion.agent] ?? discussion.agent;
              const concreteModel = answer?.model ?? modelForAgentTier(
                discussion.agent,
                discussion.tier ?? 'default',
                modelTiers,
                t('disc.defaultAgentModel'),
              );
              const weighted = weightedQuality(evaluation, humanWeight);
              const failureCause = answer ? null : lastSystemCause(discussion);
              const tokens = answer?.tokens_used && answer.tokens_used > 0
                ? answer.tokens_used.toLocaleString()
                : '—';
              const partialWeighted = metric === 'weighted'
                && (manualScore == null || ai?.score == null)
                && value != null;
              return (
                <li key={discussion.id} className="disc-compare-ranking-item">
                  <header>
                    <span className="disc-compare-rank">{value == null ? '—' : `#${index + 1}`}</span>
                    <strong style={{ color: agentColor(discussion.agent) }}>{agentLabel}</strong>
                    <span className="disc-compare-rank-value">{formatRankValue(value)}</span>
                    {partialWeighted && <small>{t('disc.compare.partialQuality')}</small>}
                  </header>
                  <div className="disc-compare-card-model" title={concreteModel}>
                    <Bot size={12} />
                    <span>{concreteModel}</span>
                    {!answer?.model && <small>{t('disc.compare.modelInferred')}</small>}
                  </div>

                  <div className="disc-compare-all-metrics">
                    <span data-active={metric === 'weighted'}><small>{t('disc.compare.rankWeighted')}</small><strong>{weighted == null ? '—' : `${weighted.toFixed(2)} / 5`}</strong></span>
                    <span data-active={metric === 'ai'}><small>{t('disc.compare.rankAi')}</small><strong>{ai?.score == null ? '—' : `${ai.score} / 5`}</strong></span>
                    <span data-active={metric === 'human'}><small>{t('disc.compare.rankHuman')}</small><strong>{manualScore == null ? '—' : `${manualScore} / 5`}</strong></span>
                    <span data-active={metric === 'duration'}><small>{t('disc.compare.rankDuration')}</small><strong>{formatDuration(answer?.duration_ms)}</strong></span>
                    <span data-active={metric === 'tokens'}><small>{t('disc.compare.rankTokens')}</small><strong>{tokens}</strong></span>
                  </div>

                  {failureCause && (
                    <div className="disc-compare-failure" role="alert">
                      <strong>{t('disc.compare.failureReason')}</strong>
                      <p>{failureCause}</p>
                    </div>
                  )}

                  <div className="disc-compare-quality-row">
                    <span>{t('disc.compare.humanQuality')}</span>
                    <div className="disc-compare-stars" data-saving={savingDiscussionId === discussion.id}>
                      {stars(manualScore).map((filled, starIndex) => {
                        const score = starIndex + 1;
                        return (
                          <button
                            type="button"
                            key={score}
                            disabled={savingDiscussionId === discussion.id}
                            onClick={() => { void saveManual(discussion.id, manualScore === score ? null : score); }}
                            aria-label={t('disc.compare.setHumanScore', score, agentLabel)}
                            aria-pressed={manualScore === score}
                          >
                            <Star size={16} fill={filled ? 'currentColor' : 'none'} />
                          </button>
                        );
                      })}
                    </div>
                  </div>

                  <div className="disc-compare-quality-row">
                    <span>{t('disc.compare.aiQuality')}</span>
                    <div className="disc-compare-stars" aria-label={ai ? t('disc.compare.aiScore', ai.score) : t('disc.compare.notRated')}>
                      {stars(ai?.score).map((filled, starIndex) => (
                        <Star key={starIndex} size={15} fill={filled ? 'currentColor' : 'none'} />
                      ))}
                    </div>
                    {ai && <small>{t('disc.compare.confidence', Math.round(ai.confidence * 100))}</small>}
                  </div>

                  {ai && (
                    <details className="disc-compare-ai-details">
                      <summary>
                        <ChevronDown size={13} />
                        <span>{t('disc.compare.aiFeedback')}</span>
                        <small>{ai.positives.length + ai.negatives.length + ai.contract_violations.length}</small>
                      </summary>
                      <div className="disc-compare-ai-feedback">
                        {ai.positives.length > 0 && (
                          <div data-kind="positive"><ThumbsUp size={13} /><ul>{ai.positives.map(point => <li key={point}>{point}</li>)}</ul></div>
                        )}
                        {ai.negatives.length > 0 && (
                          <div data-kind="negative"><ThumbsDown size={13} /><ul>{ai.negatives.map(point => <li key={point}>{point}</li>)}</ul></div>
                        )}
                        {ai.contract_violations.length > 0 && (
                          <div data-kind="violation"><MinusCircle size={13} /><ul>{ai.contract_violations.map(point => <li key={point}>{point}</li>)}</ul></div>
                        )}
                      </div>
                    </details>
                  )}
                </li>
              );
            })}
          </ol>
        </div>
      )}
    </aside>
  );
}
