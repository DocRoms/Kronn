/**
 * 0.8.4 (#285) — Désagentified briefing form.
 *
 * Replaces the conversational briefing flow (which spawned a discussion
 * + 1 LLM call to ask 6 questions) with a direct HTML form. The user
 * fills the 6 fields, hits Save, and the backend writes
 * `docs/briefing.md` byte-for-byte compatible with the conversational
 * output — so the audit Phase 1 reads the same shape regardless of
 * the briefing entry point.
 *
 * Token cost = 0. Latency = a single HTTP roundtrip. The user can
 * still pick the conversational variant if they prefer guidance —
 * both are surfaced side-by-side in ProjectCard.
 */
import { useEffect, useRef, useState } from 'react';
import { useT } from '../lib/I18nContext';
import { projects as projectsApi } from '../lib/api';
import { Save, X, Loader2 } from 'lucide-react';
import type {
  AgentType, BriefingDocLocation, BriefingFact, BriefingProjectAnswers, CrossAuditEstimate, LinkedRepo, ModelTier, ProjectRule,
  SavedBriefing,
} from '../types/generated';
import { AGENT_LABELS } from '../lib/constants';
import { formatUsd } from '../lib/audit-cost';
import { BriefingExistingDocs } from './BriefingExistingDocs';
import { BriefingProjectRules } from './BriefingProjectRules';
import { ProjectLinkedRepos } from './ProjectLinkedRepos';
import type { SavedDocs } from './briefingDocs';
import {
  BRIEFING_ENVIRONMENTS,
  BRIEFING_PROJECT_CARDS,
  BRIEFING_ROLES,
  FACT_NOTE_MAX,
  cleanFactNotes,
  emptyProjectAnswers,
  projectUnanswered,
  trackerTakesKey,
  type BriefingChoiceCard,
} from './briefingProjectCards';

/** KT-1183 — agents a cross-agent audit may use: audit-capable, no named connection. */
const CROSS_AUDIT_AGENTS: AgentType[] = [
  'ClaudeCode', 'Codex', 'OpenCode', 'GeminiCli', 'Kiro', 'CopilotCli', 'Ollama', 'LiteLlm', 'Nvidia',
];

interface BriefingFormProps {
  projectId: string;
  onClose: () => void;
  /**
   * Called after the form is saved. Receives the spawned discussion id
   * when the post-save AI review fires successfully (so the parent can
   * navigate to it). `null` means the form was saved but no discussion
   * could be spawned (e.g. agent unavailable, network glitch) — the
   * parent should still refresh the project list to reflect the new
   * briefing notes, but stay on the dashboard.
   *
   * 0.8.4 UX fix — pre-fix there were 2 independent buttons (form +
   * AI). Now it's a single flow: form → save → spawn review disc →
   * navigate. The user can no longer accidentally fork the briefing
   * state into "form filled but no AI clarification" + "AI started
   * with no form data".
   */
  onSaved: (discussionId: string | null) => void;
  /**
   * Selected agent for the post-save AI review discussion. Picked from
   * the same dropdown as the audit launch row so the user doesn't have
   * to specify it again here.
   */
  agent: AgentType;
  /** Reasoning level selected alongside the briefing/audit agent. */
  tier: ModelTier;
  /** KT-977 — `false` saves the answers only: no review discussion is created. */
  review?: boolean;
  /** Toast emitter — same shape as the rest of the dashboard. The
   * required (non-optional) kind matches Dashboard's stricter signature. */
  toast: (msg: string, kind: 'success' | 'error' | 'info' | 'warning') => void;
  /** KT-999 — the project's linked repos; the section shows when given. */
  linkedRepos?: LinkedRepo[];
  /** Refetch the project after the linked repos changed. */
  onLinkedReposChange?: () => void;
}

export function BriefingForm({
  projectId, onClose, onSaved, agent, tier, review = true, toast, linkedRepos, onLinkedReposChange,
}: BriefingFormProps) {
  const { t, locale } = useT();
  const [form, setForm] = useState({
    purpose: '',
    team: '',
    maturity: '',
    dependencies: '',
    traps: '',
    additional: '',
  });
  // KT-1167 — persisted in the briefing; read at the end of a Full audit.
  const [tdToTasks, setTdToTasks] = useState(false);
  // KT-1183 — experimental, never on by default.
  const [crossAudit, setCrossAudit] = useState(false);
  const [crossAgents, setCrossAgents] = useState<AgentType[]>([]);
  const [estimate, setEstimate] = useState<CrossAuditEstimate | null>(null);
  const crossCountOk = crossAgents.length >= 2 && crossAgents.length <= 3;

  useEffect(() => {
    if (!crossAudit || !crossCountOk) return;
    let live = true;
    projectsApi.crossAuditEstimate(projectId, crossAgents.length)
      .then(e => { if (live) setEstimate(e); })
      .catch(() => { if (live) setEstimate(null); });
    return () => { live = false; };
  }, [crossAudit, crossCountOk, crossAgents.length, projectId]);

  const toggleCrossAgent = (agent: AgentType) =>
    setCrossAgents(list => (list.includes(agent)
      ? list.filter(a => a !== agent)
      : list.length >= 3 ? list : [...list, agent]));

  const estimateText = (e: CrossAuditEstimate): string => {
    if (!e.estimated && e.measured_cost_usd_micros != null) {
      return t('briefing.crossAudit.measured', formatUsd(e.measured_cost_usd_micros, locale));
    }
    if (e.low_cost_usd_micros != null && e.high_cost_usd_micros != null) {
      return t('briefing.crossAudit.estimateCost', formatUsd(e.low_cost_usd_micros, locale),
        formatUsd(e.high_cost_usd_micros, locale), e.low_factor, e.high_factor);
    }
    if (e.low_tokens != null && e.high_tokens != null) {
      return t('briefing.crossAudit.estimateTokens', e.low_tokens.toLocaleString(locale),
        e.high_tokens.toLocaleString(locale), e.low_factor, e.high_factor);
    }
    return t('briefing.crossAudit.estimateFactor', e.low_factor, e.high_factor);
  };
  const [submitting, setSubmitting] = useState(false);
  const [project, setProject] = useState<BriefingProjectAnswers>(emptyProjectAnswers);
  const [docs, setDocs] = useState<BriefingDocLocation[]>([]);
  const [rules, setRules] = useState<ProjectRule[]>([]);
  const [rulesSeed, setRulesSeed] = useState<{ key: number; rules: ProjectRule[] }>({ key: 0, rules: [] });
  const seedRules = (next: ProjectRule[]) => setRulesSeed(s => ({ key: s.key + 1, rules: next }));
  // KT-1196 — the saved briefing pre-fills the form. When it arrives after a
  // real change, only the still-empty sections are filled and a notice offers the rest.
  const [savedDocs, setSavedDocs] = useState<SavedDocs>('loading');
  const [prefilled, setPrefilled] = useState(false);
  const [lateSaved, setLateSaved] = useState<SavedBriefing | null>(null);
  const [incoming, setIncoming] = useState<{ saved: SavedBriefing; late: boolean } | null>(null);
  const touched = useRef(false);

  useEffect(() => {
    let live = true;
    projectsApi.savedBriefing(projectId)
      .then(saved => {
        if (!live) return;
        // A saved briefing without the section kept no location.
        setSavedDocs(saved ? saved.existing_docs?.locations ?? [] : null);
        if (saved) setIncoming({ saved, late: touched.current });
      })
      .catch(e => {
        console.warn('[BriefingForm] saved briefing unavailable:', e);
        if (live) setSavedDocs(null);
      });
    return () => { live = false; };
  }, [projectId]);

  const applySaved = (saved: SavedBriefing) => {
    setForm({
      purpose: saved.purpose,
      team: saved.team,
      maturity: saved.maturity,
      dependencies: saved.dependencies,
      traps: saved.traps,
      additional: saved.additional,
    });
    setProject({ ...emptyProjectAnswers(), ...(saved.project ?? {}) });
    setTdToTasks(saved.td_to_tasks);
    setCrossAudit(saved.cross_audit_agents.length > 0);
    setCrossAgents(saved.cross_audit_agents);
    seedRules(saved.project_rules?.rules ?? []);
    setLateSaved(null);
    setPrefilled(true);
  };
  const fillUnanswered = (saved: SavedBriefing) => {
    setForm(f => ({
      purpose: f.purpose.trim() ? f.purpose : saved.purpose,
      team: f.team.trim() ? f.team : saved.team,
      maturity: f.maturity.trim() ? f.maturity : saved.maturity,
      dependencies: f.dependencies.trim() ? f.dependencies : saved.dependencies,
      traps: f.traps.trim() ? f.traps : saved.traps,
      additional: f.additional.trim() ? f.additional : saved.additional,
    }));
    setProject(p => (projectUnanswered(p) ? { ...emptyProjectAnswers(), ...(saved.project ?? {}) } : p));
    setTdToTasks(v => v || saved.td_to_tasks);
    setCrossAgents(list => (list.length > 0 ? list : saved.cross_audit_agents));
    setCrossAudit(on => on || saved.cross_audit_agents.length > 0);
    // Rows without text count as empty: the saved rules replace them.
    const savedRules = saved.project_rules?.rules ?? [];
    if (rules.length === 0 && savedRules.length > 0) seedRules(savedRules);
    setLateSaved(saved);
  };
  if (incoming) {
    setIncoming(null);
    if (incoming.late) fillUnanswered(incoming.saved);
    else applySaved(incoming.saved);
  }
  const markChanged = () => { touched.current = true; };

  const setNote = (fact: BriefingFact, value: string) =>
    setProject(p => ({ ...p, notes: { ...(p.notes ?? {}), [fact]: value } }));
  const noteInput = (fact: BriefingFact, label: string) => (
    <input
      className="briefing-card-input briefing-card-note"
      aria-label={t('briefing.project.noteFor', label)}
      placeholder={t('briefing.project.notePlaceholder')}
      value={project.notes?.[fact] ?? ''}
      maxLength={FACT_NOTE_MAX}
      onChange={e => setNote(fact, e.target.value)}
      data-note={fact}
    />
  );
  // KT-1192 — pre-filled by the backend: override, existing folder, global default, docs.
  const [docsDir, setDocsDir] = useState('');
  const [docsDirLoaded, setDocsDirLoaded] = useState(false);
  const [docsDirExists, setDocsDirExists] = useState(false);

  useEffect(() => {
    let live = true;
    Promise.resolve()
      .then(() => projectsApi.briefingDefaults(projectId))
      .then(value => {
        if (!live) return;
        setDocsDir(value.docs_dir);
        setDocsDirExists(value.folder_exists);
        setDocsDirLoaded(true);
      })
      .catch(() => { if (live) setDocsDirLoaded(true); });
    return () => { live = false; };
  }, [projectId]);

  const update = (key: keyof typeof form, value: string) => setForm(f => ({ ...f, [key]: value }));
  const choose = (key: BriefingChoiceCard['key'], id: string) =>
    setProject(p => ({ ...p, [key]: id }));
  const toggleEnv = (kind: BriefingProjectAnswers['environments'][number]['kind']) =>
    setProject(p => ({
      ...p,
      environments: p.environments.some(e => e.kind === kind)
        ? p.environments.filter(e => e.kind !== kind)
        : [...p.environments, { kind, url: '' }],
    }));
  const setEnvUrl = (kind: string, url: string) =>
    setProject(p => ({ ...p, environments: p.environments.map(e => (e.kind === kind ? { ...e, url } : e)) }));
  const toggleRole = (role: BriefingProjectAnswers['people'][number]['role']) =>
    setProject(p => ({
      ...p,
      people: p.people.some(x => x.role === role)
        ? p.people.filter(x => x.role !== role)
        : [...p.people, { role, name: '', contact: '' }],
    }));
  const setPerson = (role: string, field: 'name' | 'contact', value: string) =>
    setProject(p => ({ ...p, people: p.people.map(x => (x.role === role ? { ...x, [field]: value } : x)) }));

  // KT-1192 — one click: « Not provided » answers, the folder as shown.
  const saveDefaults = async () => {
    setSubmitting(true);
    try {
      const notProvided = 'Not provided';
      await projectsApi.saveBriefing(projectId, {
        purpose: notProvided,
        team: notProvided,
        maturity: notProvided,
        dependencies: notProvided,
        traps: notProvided,
        additional: '',
        ...(docsDir.trim() ? { docs_dir: docsDir.trim() } : {}),
      });
      toast(t('briefing.savedOnlyToast'), 'success');
      onSaved(null);
      onClose();
    } catch (err) {
      toast(String(err), 'error');
    } finally {
      setSubmitting(false);
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    // Q1-Q5 mandatory, mirror the backend validation so the user gets
    // an inline error before the round-trip.
    const missing: string[] = [];
    if (!form.purpose.trim()) missing.push('purpose');
    if (!form.team.trim()) missing.push('team');
    if (!form.maturity.trim()) missing.push('maturity');
    if (!form.dependencies.trim()) missing.push('dependencies');
    if (!form.traps.trim()) missing.push('traps');
    if (missing.length > 0) {
      toast(t('briefing.missingFields', missing.join(', ')), 'error');
      return;
    }
    if (crossAudit && !crossCountOk) {
      toast(t('briefing.crossAudit.pickAgents'), 'error');
      return;
    }
    setSubmitting(true);
    try {
      // 1. Save the answers — writes `docs/briefing.md` + DB notes.
      const projectAnswers: BriefingProjectAnswers = {
        ...project,
        tracker_key: trackerTakesKey(project.tracker) ? project.tracker_key.trim() : '',
        forge_repo: project.forge === 'unknown' ? '' : project.forge_repo.trim(),
        people: project.people.filter(p => p.name.trim() || p.contact.trim()),
        notes: cleanFactNotes(project),
      };
      await projectsApi.saveBriefing(projectId, {
        ...form,
        project: projectAnswers,
        td_to_tasks: tdToTasks,
        existing_docs: { locations: docs },
        project_rules: { rules },
        cross_audit_agents: crossAudit ? crossAgents : [],
        ...(docsDir.trim() ? { docs_dir: docsDir.trim() } : {}),
      });
      if (!review) {
        toast(t('briefing.savedOnlyToast'), 'success');
        onSaved(null);
        onClose();
        return;
      }
      // 2. Spawn the AI review discussion. The backend now sees the
      //    pre-filled briefing notes and switches to a short
      //    "review + clarify" prompt instead of re-asking the 6 Qs.
      //    If this fails (e.g. no audit-capable agent), we still
      //    consider the save a success — the answers are persisted,
      //    the audit can still pick them up.
      let discId: string | null = null;
      try {
        const { discussion_id } = await projectsApi.startBriefing(projectId, agent, tier);
        discId = discussion_id;
      } catch (spawnErr) {
        console.warn('Briefing review disc could not spawn:', spawnErr);
        toast(t('briefing.savedButNoDiscToast'), 'warning');
      }
      if (discId !== null) {
        toast(t('briefing.savedToast'), 'success');
      }
      onSaved(discId);
      onClose();
    } catch (err) {
      toast(String(err), 'error');
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <form
      onSubmit={handleSubmit}
      className="briefing-form"
      onChangeCapture={markChanged}
      onClickCapture={e => {
        // Choice buttons change a value; a click that only focuses a field does not.
        if ((e.target as HTMLElement).closest('[role="radio"], [role="checkbox"]')) markChanged();
      }}
    >
      <div className="briefing-form-header">
        <h3>{t('briefing.formTitle')}</h3>
        <button type="button" className="briefing-form-close" onClick={onClose} aria-label={t('common.close')}>
          <X size={14} />
        </button>
      </div>
      <p className="briefing-form-intro">{t('briefing.formIntro')}</p>
      {prefilled && <p className="briefing-form-intro" data-testid="briefing-prefilled">{t('briefing.prefilled')}</p>}
      {lateSaved && (
        <div className="briefing-form-notice" role="status" data-testid="briefing-late-prefill">
          <span>{t('briefing.prefillLate')}</span>
          <button type="button" className="briefing-card-option" onClick={() => applySaved(lateSaved)}>
            {t('briefing.loadSaved')}
          </button>
        </div>
      )}
      <div className="briefing-form-defaults">
        <button
          type="button"
          onClick={() => void saveDefaults()}
          disabled={submitting || !docsDirLoaded}
          data-testid="briefing-use-defaults"
        >
          {t('briefing.useDefaults')}
        </button>
        <span className="briefing-form-option-hint">{t('briefing.useDefaultsHint')}</span>
      </div>

      <label>
        <span className="briefing-form-q">{t('briefing.q1Purpose')}</span>
        <textarea
          rows={2}
          value={form.purpose}
          onChange={e => update('purpose', e.target.value)}
          placeholder={t('briefing.q1Placeholder')}
          required
        />
      </label>

      <label>
        <span className="briefing-form-q">{t('briefing.q2Team')}</span>
        <textarea
          rows={2}
          value={form.team}
          onChange={e => update('team', e.target.value)}
          placeholder={t('briefing.q2Placeholder')}
          required
        />
      </label>

      <label>
        <span className="briefing-form-q">{t('briefing.q3Maturity')}</span>
        <textarea
          rows={2}
          value={form.maturity}
          onChange={e => update('maturity', e.target.value)}
          placeholder={t('briefing.q3Placeholder')}
          required
        />
      </label>

      <label>
        <span className="briefing-form-q">{t('briefing.q4Dependencies')}</span>
        <textarea
          rows={3}
          value={form.dependencies}
          onChange={e => update('dependencies', e.target.value)}
          placeholder={t('briefing.q4Placeholder')}
          required
        />
      </label>

      <label>
        <span className="briefing-form-q">{t('briefing.q5Traps')}</span>
        <textarea
          rows={3}
          value={form.traps}
          onChange={e => update('traps', e.target.value)}
          placeholder={t('briefing.q5Placeholder')}
          required
        />
      </label>

      <label>
        <span className="briefing-form-q">{t('briefing.q6Additional')}</span>
        <textarea
          rows={2}
          value={form.additional}
          onChange={e => update('additional', e.target.value)}
          placeholder={t('briefing.q6Placeholder')}
        />
      </label>

      <label data-testid="briefing-docs-dir">
        <span className="briefing-form-q">{t('briefing.docsDir')}</span>
        <input
          className="briefing-card-input"
          value={docsDir}
          disabled={!docsDirLoaded}
          placeholder="docs"
          maxLength={255}
          onChange={e => setDocsDir(e.target.value)}
          data-testid="briefing-docs-dir-input"
        />
        <span className="briefing-form-option-hint">
          {docsDirExists && docsDir.trim()
            ? t('briefing.docsDirExists', `${docsDir.trim().replace(/\/+$/, '')}/`)
            : t('briefing.docsDirHint')}
        </span>
      </label>

      <section className="briefing-project" aria-labelledby="briefing-project-title">
        <h4 id="briefing-project-title" className="briefing-form-q">{t('briefing.project.title')}</h4>
        <p className="briefing-form-intro">{t('briefing.project.intro')}</p>

        {BRIEFING_PROJECT_CARDS.map(card => (
          <fieldset key={card.key} className="briefing-card" data-card={card.key}>
            <legend className="briefing-form-q">{t(card.questionKey)}</legend>
            <div className="briefing-card-options" role="radiogroup" aria-label={t(card.questionKey)}>
              {card.options.map(option => {
                const selected = project[card.key] === option.id;
                return (
                  <button
                    key={option.id}
                    type="button"
                    role="radio"
                    aria-checked={selected}
                    className={`briefing-card-option${selected ? ' is-selected' : ''}`}
                    onClick={() => choose(card.key, option.id)}
                  >
                    {option.literal ?? t(option.labelKey ?? '')}
                  </button>
                );
              })}
            </div>
            {card.key === 'tracker' && trackerTakesKey(project.tracker) && (
              <input
                className="briefing-card-input"
                aria-label={t('briefing.project.trackerKey')}
                placeholder={t('briefing.project.trackerKeyPlaceholder')}
                value={project.tracker_key}
                maxLength={64}
                onChange={e => setProject(p => ({ ...p, tracker_key: e.target.value }))}
              />
            )}
            {card.key === 'tracker' && trackerTakesKey(project.tracker)
              && noteInput('tracker_key', t('briefing.project.trackerKey'))}
            {card.key === 'forge' && project.forge !== 'unknown' && (
              <input
                className="briefing-card-input"
                aria-label={t('briefing.project.forgeRepo')}
                placeholder={t('briefing.project.forgeRepoPlaceholder')}
                value={project.forge_repo}
                maxLength={140}
                onChange={e => setProject(p => ({ ...p, forge_repo: e.target.value }))}
              />
            )}
            {card.key === 'forge' && project.forge !== 'unknown'
              && noteInput('forge_repo', t('briefing.project.forgeRepo'))}
            {noteInput(card.fact, t(card.questionKey))}
          </fieldset>
        ))}

        <fieldset className="briefing-card" data-card="environments">
          <legend className="briefing-form-q">{t('briefing.project.envQ')}</legend>
          <div className="briefing-card-options" role="group" aria-label={t('briefing.project.envQ')}>
            {BRIEFING_ENVIRONMENTS.map(env => {
              const selected = project.environments.some(e => e.kind === env.id);
              return (
                <button
                  key={env.id}
                  type="button"
                  role="checkbox"
                  aria-checked={selected}
                  className={`briefing-card-option${selected ? ' is-selected' : ''}`}
                  onClick={() => toggleEnv(env.id)}
                >
                  {t(env.labelKey)}
                </button>
              );
            })}
          </div>
          {project.environments.map(env => (
            <input
              key={env.kind}
              className="briefing-card-input"
              type="url"
              aria-label={`${t(BRIEFING_ENVIRONMENTS.find(e => e.id === env.kind)?.labelKey ?? '')} — URL`}
              placeholder={t('briefing.project.envUrlPlaceholder')}
              value={env.url}
              maxLength={200}
              onChange={e => setEnvUrl(env.kind, e.target.value)}
            />
          ))}
          {noteInput('environments', t('briefing.project.envQ'))}
        </fieldset>

        <fieldset className="briefing-card" data-card="people">
          <legend className="briefing-form-q">{t('briefing.project.peopleQ')}</legend>
          <div className="briefing-card-options" role="group" aria-label={t('briefing.project.peopleQ')}>
            {BRIEFING_ROLES.map(role => {
              const selected = project.people.some(p => p.role === role.id);
              return (
                <button
                  key={role.id}
                  type="button"
                  role="checkbox"
                  aria-checked={selected}
                  className={`briefing-card-option${selected ? ' is-selected' : ''}`}
                  onClick={() => toggleRole(role.id)}
                >
                  {t(role.labelKey)}
                </button>
              );
            })}
          </div>
          {project.people.map(person => {
            const roleLabel = t(BRIEFING_ROLES.find(r => r.id === person.role)?.labelKey ?? '');
            return (
              <div key={person.role} className="briefing-card-person">
                <input
                  className="briefing-card-input"
                  aria-label={`${roleLabel} — ${t('briefing.project.personName')}`}
                  placeholder={t('briefing.project.personName')}
                  value={person.name}
                  maxLength={80}
                  onChange={e => setPerson(person.role, 'name', e.target.value)}
                />
                <input
                  className="briefing-card-input"
                  aria-label={`${roleLabel} — ${t('briefing.project.personContact')}`}
                  placeholder={t('briefing.project.personContact')}
                  value={person.contact}
                  maxLength={120}
                  onChange={e => setPerson(person.role, 'contact', e.target.value)}
                />
              </div>
            );
          })}
          {noteInput('people', t('briefing.project.peopleQ'))}
          <p className="briefing-form-intro">{t('briefing.project.peoplePrivacy')}</p>
        </fieldset>
      </section>

      <BriefingExistingDocs projectId={projectId} onChange={setDocs} saved={savedDocs} />

      <BriefingProjectRules key={rulesSeed.key} initial={rulesSeed.rules} onChange={setRules} />

      {linkedRepos && (
        <section className="briefing-project" aria-labelledby="briefing-linked-title" data-testid="briefing-linked-repos">
          <h4 id="briefing-linked-title" className="briefing-form-q">{t('briefing.linked.title')}</h4>
          <p className="briefing-form-intro">{t('briefing.linked.intro')}</p>
          <ProjectLinkedRepos
            projectId={projectId}
            currentRepos={linkedRepos}
            onUpdate={onLinkedReposChange ?? (() => {})}
            inlineSuggestions
          />
        </section>
      )}

      <label className="briefing-form-option">
        <input
          type="checkbox"
          checked={tdToTasks}
          onChange={e => setTdToTasks(e.target.checked)}
          data-testid="briefing-td-to-tasks"
        />
        <span>
          <span className="briefing-form-q">{t('briefing.tdToTasks')}</span>
          <span className="briefing-form-option-hint">{t('briefing.tdToTasksHint')}</span>
        </span>
      </label>

      <label className="briefing-form-option">
        <input
          type="checkbox"
          checked={crossAudit}
          onChange={e => setCrossAudit(e.target.checked)}
          data-testid="briefing-cross-audit"
        />
        <span>
          <span className="briefing-form-q">
            {t('briefing.crossAudit.label')}{' '}
            <span className="briefing-experimental">{t('briefing.crossAudit.experimental')}</span>
          </span>
          <span className="briefing-form-option-hint">{t('briefing.crossAudit.hint')}</span>
        </span>
      </label>
      {crossAudit && (
        <fieldset className="briefing-card" data-card="cross-audit">
          <legend className="briefing-form-q">{t('briefing.crossAudit.agentsQ')}</legend>
          <div className="briefing-card-options" role="group" aria-label={t('briefing.crossAudit.agentsQ')}>
            {CROSS_AUDIT_AGENTS.map(agent => {
              const selected = crossAgents.includes(agent);
              return (
                <button
                  key={agent}
                  type="button"
                  role="checkbox"
                  aria-checked={selected}
                  className={`briefing-card-option${selected ? ' is-selected' : ''}`}
                  onClick={() => toggleCrossAgent(agent)}
                  data-testid={`briefing-cross-agent-${agent}`}
                >
                  {AGENT_LABELS[agent] ?? agent}
                </button>
              );
            })}
          </div>
          <p className="briefing-form-intro" data-testid="briefing-cross-estimate">
            {!crossCountOk
              ? t('briefing.crossAudit.pickAgents')
              : estimate && estimate.agents === crossAgents.length
                ? estimateText(estimate)
                : t('briefing.crossAudit.estimateLoading')}
          </p>
          <p className="briefing-form-option-hint">{t('briefing.crossAudit.scope')}</p>
        </fieldset>
      )}

      <div className="briefing-form-actions">
        <button type="button" onClick={onClose} disabled={submitting}>{t('common.cancel')}</button>
        <button type="submit" className="briefing-form-submit" disabled={submitting}>
          {submitting
            ? <><Loader2 size={11} style={{ animation: 'spin 1s linear infinite' }} /> {t('common.saving')}</>
            : <><Save size={11} /> {review ? t('briefing.saveAndReviewBtn') : t('briefing.saveOnlyBtn')}</>}
        </button>
      </div>
    </form>
  );
}
