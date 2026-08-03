import './MentorPage.css';
import { useState, useMemo, useEffect, useCallback, useRef } from 'react';
import { mentor as mentorApi, config as configApi, projects as projectsApi } from '../lib/api';
import { MermaidDiagram } from '../components/MermaidDiagram';
import ReactMarkdown, { type Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { highlightCode, languageForPath } from '../lib/diff-syntax';
import { useT } from '../lib/I18nContext';
import { recommendedNextTopic, curriculumProgress, onboardingTopicStatus, levelTier, type LevelTier } from '../lib/onboarding-progress';
import { Badge, type BadgeTone } from '../components/Badge';
import type { MentorState, MentorBlock, MentorPhase, MentorTurn, Chapter, Checkpoint, OnboardingTopic, ParcoursSummary, HintState, BilanSynthesis } from '../types/generated';
import { GraduationCap, Lock, Check, CircleDot, ShieldCheck, Send, Loader2, Lightbulb, Sparkles, BookOpen, Eye, HelpCircle, Library, X, FileDiff, ChevronRight, KeyRound, Trash2, Sprout, GitBranch, RotateCcw } from 'lucide-react';

/** Gated block order (mirrors the backend `PHASE_ORDER`). Used to tell whether the
 *  parcours has advanced past a synthetic block like Target. */
const PHASE_SEQ: MentorPhase[] = ['comprehension', 'resources', 'target', 'plan', 'code', 'bilan'];

/** Curriculum kinds that carry a translated label + icon (mirrors the registry
 *  `Type` bullet). Anything else is untyped — no kind chip is shown. */
const KNOWN_KINDS = ['tronc', 'branche', 'capstone', 'culture'] as const;
/** Per-kind Lucide glyph, shared by the landing cards' kind chip. */
function kindGlyph(kind: string | null, size = 12) {
  switch (kind) {
    case 'tronc': return <Sprout size={size} />;
    case 'branche': return <GitBranch size={size} />;
    case 'capstone': return <GraduationCap size={size} />;
    case 'culture': return <ShieldCheck size={size} />;
    default: return <BookOpen size={size} />;
  }
}
/** Level tier → badge tone (🟢 débutant / 🟠 intermédiaire / 🟣 avancé). */
const LEVEL_TONE: Record<LevelTier, BadgeTone> = { beginner: 'success', intermediate: 'warning', advanced: 'purple' };

/** GFM so chapter explanations render tables, lists, code, etc. (not raw pipes). */
const MD_PLUGINS = [remarkGfm];

/** Markdown renderers for chapter explanations. Fenced code blocks get syntax
 *  highlighting (reusing the shared highlight.js setup); inline code stays plain. */
const MD_COMPONENTS: Components = {
  code({ className, children }) {
    const lang = /language-(\w+)/.exec(className || '')?.[1];
    if (!lang) return <code>{children}</code>; // inline `code`
    const source = String(children ?? '').replace(/\n$/, '');
    return <code className="hljs" dangerouslySetInnerHTML={{ __html: highlightCode(source, lang) }} />;
  },
};

/** Mermaid root keywords the generator may emit for `target_archi` (kept in sync
 *  with MermaidDiagram's own guard). A parcours' archi is a diagram when its
 *  first token is one of these; otherwise it's prose we render as text. */
const MERMAID_ROOTS = [
  'flowchart', 'graph', 'sequenceDiagram', 'classDiagram', 'stateDiagram',
  'stateDiagram-v2', 'erDiagram', 'journey', 'mindmap', 'timeline',
  'C4Context', 'C4Container', 'C4Component',
];

/** Return the Mermaid source of `archi` (unwrapping a ```mermaid fence if any),
 *  or null when it isn't a diagram — the caller then renders it as text. */
function mermaidSource(archi: string): string | null {
  let src = archi.trim();
  const fenced = src.match(/^```(?:mermaid)?\s*\n([\s\S]*?)\n```$/);
  if (fenced) src = fenced[1].trim();
  const head = src.split(/[\s\n]/, 1)[0] ?? '';
  return MERMAID_ROOTS.some((r) => head === r || head.startsWith(r)) ? src : null;
}

export function MentorPage() {
  const { t } = useT();
  const [parcours, setParcours] = useState<MentorState | null>(null);
  // Disc id of the currently open parcours (always a real API row).
  const [loadedDiscId, setLoadedDiscId] = useState<string | null>(null);
  // Linked project of the open parcours — anchors the live mentor turns on the
  // project's real code (null = no project). Carried from the list card.
  const [loadedProjectId, setLoadedProjectId] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Landing list of existing parcours (both postures).
  const [parcoursList, setParcoursList] = useState<ParcoursSummary[]>([]);
  const [listLoading, setListLoading] = useState(true);
  // Configured mentor→censeur workflow id (null = live turn not wired).
  const [workflowId, setWorkflowId] = useState<string | null>(null);
  // Configured parcours-generator workflow id (null = no AI draft generation).
  const [generatorWorkflowId, setGeneratorWorkflowId] = useState<string | null>(null);
  // Configured onboarding course-generator workflow id (null = no AI course gen).
  const [courseWorkflowId, setCourseWorkflowId] = useState<string | null>(null);
  // Create-parcours form.
  const [showCreate, setShowCreate] = useState(false);
  const [newTitle, setNewTitle] = useState('');
  const [newObjective, setNewObjective] = useState('');
  const [newSourceType, setNewSourceType] = useState<'free' | 'jira'>('free');
  const [newTicket, setNewTicket] = useState('');
  // Pedagogical posture of the parcours being created.
  const [newMode, setNewMode] = useState<'mentor' | 'onboarding'>('mentor');
  const [generating, setGenerating] = useState(false);
  const [generatingCourse, setGeneratingCourse] = useState(false);
  // Onboarding registry catalogue (project-scoped).
  const [newProjectId, setNewProjectId] = useState<string>('');
  const [projectList, setProjectList] = useState<{ id: string; name: string; onboarding_count: number }[]>([]);
  const [catalog, setCatalog] = useState<OnboardingTopic[]>([]);
  const [catalogLoading, setCatalogLoading] = useState(false);
  const [selectedTopic, setSelectedTopic] = useState<OnboardingTopic | null>(null);
  // Landing list project filter: 'all' | project_id | '__none__'.
  const [listFilter, setListFilter] = useState<string>('all');

  useEffect(() => {
    configApi
      .getServerConfig()
      .then((c) => {
        setWorkflowId(c.mentor_turn_workflow_id ?? null);
        setGeneratorWorkflowId(c.mentor_generator_workflow_id ?? null);
        setCourseWorkflowId(c.mentor_course_workflow_id ?? null);
      })
      .catch(() => { setWorkflowId(null); setGeneratorWorkflowId(null); setCourseWorkflowId(null); });
  }, []);

  // Load the project list once the create form is opened (needed to scope the
  // onboarding catalogue to a project).
  useEffect(() => {
    if (!showCreate || projectList.length > 0) return;
    projectsApi.list()
      .then((ps) => setProjectList(ps.map((p) => ({ id: p.id, name: p.name, onboarding_count: p.onboarding_count }))))
      .catch(() => { /* leave empty — catalogue just won't be available */ });
  }, [showCreate, projectList.length]);

  // Load the onboarding catalogue whenever a project is picked in onboarding mode.
  useEffect(() => {
    if (newMode !== 'onboarding' || !newProjectId) { setCatalog([]); return; }
    let cancelled = false;
    setCatalogLoading(true);
    mentorApi.onboardingCatalog(newProjectId)
      .then((topics) => { if (!cancelled) setCatalog(topics); })
      .catch(() => { if (!cancelled) setCatalog([]); })
      .finally(() => { if (!cancelled) setCatalogLoading(false); });
    return () => { cancelled = true; };
  }, [newMode, newProjectId]);

  // (Re)load the parcours list whenever we're on the landing screen.
  useEffect(() => {
    if (parcours) return;
    let cancelled = false;
    setListLoading(true);
    mentorApi.listParcours()
      .then((l) => { if (!cancelled) setParcoursList(l); })
      .catch(() => { if (!cancelled) setParcoursList([]); })
      .finally(() => { if (!cancelled) setListLoading(false); });
    return () => { cancelled = true; };
  }, [parcours]);

  // Silent list refresh (no full-screen loader) — after firing a background
  // generation, and on each poll tick below.
  const refreshList = useCallback(async () => {
    try { setParcoursList(await mentorApi.listParcours()); } catch { /* keep last good list */ }
  }, []);

  // While any parcours is still generating, poll so its card flips to
  // ready/failed on its own (the workflow runs server-side).
  useEffect(() => {
    if (parcours) return;
    const anyGenerating = parcoursList.some((p) => p.status === 'generating' && !p.generation_error);
    if (!anyGenerating) return;
    const id = setInterval(() => { void refreshList(); }, 3000);
    return () => clearInterval(id);
  }, [parcours, parcoursList, refreshList]);

  async function openById(id: string, projectId?: string | null) {
    if (!id) return;
    setLoading(true);
    setError(null);
    try {
      const state = await mentorApi.getParcours(id);
      setParcours(state);
      setLoadedDiscId(id);
      setLoadedProjectId(projectId ?? null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setParcours(null);
      setLoadedDiscId(null);
    } finally {
      setLoading(false);
    }
  }

  /** Delete a parcours (e.g. a failed generation) after a confirm, then refresh the list. */
  async function removeParcours(id: string) {
    if (!window.confirm(t('mentor.list.deleteConfirm'))) return;
    try {
      await mentorApi.deleteParcours(id);
      await refreshList();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  /** Close and reset the create form (after a manual create or firing a generation). */
  function closeCreateForm() {
    setShowCreate(false);
    setNewTitle('');
    setNewObjective('');
    setNewTicket('');
    setSelectedTopic(null);
  }

  /** Fire the parcours generator in the BACKGROUND and return to the list, where
   *  the new parcours shows as "generating" until the server fills it in. The
   *  workflow runs server-side, so you can navigate away. */
  async function generateDraft() {
    if (!generatorWorkflowId) return;
    const isJira = newSourceType === 'jira';
    const subject = newObjective.trim() || newTitle.trim();
    if (isJira ? !newTicket.trim() : !subject) return;
    setGenerating(true);
    setError(null);
    try {
      await mentorApi.generateParcours({
        title: isJira ? newTicket.trim() : (newTitle.trim() || subject),
        project_id: newProjectId || null,
        source: isJira
          ? { type: 'jira', ticket_key: newTicket.trim() }
          : { type: 'free', ticket_key: null },
        objective: isJira ? (newObjective.trim() || newTicket.trim()) : subject,
        mode: 'mentor',
        subject: isJira ? '' : subject,
        ticket_key: isJira ? newTicket.trim() : '',
      });
      closeCreateForm();
      await refreshList();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setGenerating(false);
    }
  }

  /** Generate a parcours from the picked registry topic, in the BACKGROUND.
   *  A `capstone` topic is a synthesis TASK → it spawns a socratic MENTOR parcours
   *  (the AI never gives the solution); every other topic spawns an ONBOARDING
   *  course (explanatory chapters). Both anchor on the topic's reference files. */
  async function generateCourse() {
    if (!selectedTopic || !newProjectId) return;
    const asMentor = selectedTopic.kind === 'capstone';
    const wfId = asMentor ? generatorWorkflowId : courseWorkflowId;
    if (!wfId) return;
    // The subject is the picked registry topic, anchored on its reference files.
    let subject = selectedTopic.title;
    if (selectedTopic.scope) subject += ` — ${selectedTopic.scope}`;
    // Carry the registry-curated level so the generator calibrates depth/pace (ZPD).
    if (selectedTopic.level) subject += `\n\nNiveau visé : ${selectedTopic.level}`;
    // Prerequisites → the generator can open with a short refresher chapter.
    if (selectedTopic.prerequisites) subject += `\n\nPrérequis : ${selectedTopic.prerequisites}`;
    if (selectedTopic.references.length) {
      subject += `\n\nFichiers de référence : ${selectedTopic.references.join(', ')}`;
    }
    setGeneratingCourse(true);
    setError(null);
    try {
      await mentorApi.generateParcours({
        title: selectedTopic.title,
        project_id: newProjectId,
        source: { type: 'free', ticket_key: null },
        objective: (selectedTopic.scope || selectedTopic.title).trim(),
        mode: asMentor ? 'mentor' : 'onboarding',
        subject,
        ticket_key: '',
        // Anchor the parcours to its registry topic so the catalogue can offer
        // "reprendre" instead of silently generating a duplicate.
        topic_id: selectedTopic.topic_id,
        // Carry the topic's level + curriculum kind so the landing list can badge
        // the parcours (difficulty + role) without re-reading the registry.
        level: selectedTopic.level,
        kind: selectedTopic.kind,
      });
      closeCreateForm();
      await refreshList();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setGeneratingCourse(false);
    }
  }

  if (!parcours) {
    const renderCard = (p: ParcoursSummary) => {
      const onb = p.mode === 'onboarding';
      const pct = p.progress_total ? (p.progress_done / p.progress_total) * 100 : 0;
      const genFailed = !!p.generation_error;
      const generating = p.status === 'generating' && !genFailed;
      // A still-generating or failed card doesn't open (nothing to show yet); a
      // failed one offers a delete instead of stranding the learner in a locked shell.
      const openable = !loading && !generating && !genFailed;
      const open = () => { if (openable) openById(p.disc_id, p.project_id); };
      // Registry taxonomy carried on the parcours: curriculum kind (only when a
      // known type) shown as an eyebrow above the title, and a colour-coded level
      // tag pinned top-right. Both absent on a free (non-catalogue) parcours.
      const kind = p.kind && (KNOWN_KINDS as readonly string[]).includes(p.kind) ? p.kind : null;
      const lvl = levelTier(p.level);
      const done = !generating && !genFailed && p.progress_total > 0 && p.progress_done >= p.progress_total;
      return (
        <div
          key={p.disc_id}
          className={`mentor-pcard${openable ? '' : ' mentor-pcard-static'}`}
          role="button"
          tabIndex={openable ? 0 : -1}
          aria-disabled={!openable}
          onClick={open}
          onKeyDown={(e) => { if (openable && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); open(); } }}
        >
          <div className="mentor-pcard-top">
            <Badge tone={onb ? 'accent' : 'success'} icon={onb ? <BookOpen size={12} /> : <GraduationCap size={12} />}>
              {onb ? t('mentor.create.modeOnboarding') : t('mentor.create.modeMentor')}
            </Badge>
            <div className="mentor-pcard-top-end">
              {lvl && <Badge tone={LEVEL_TONE[lvl]}>{p.level}</Badge>}
              {!generating && (
                <button
                  className="mentor-pcard-del"
                  title={t('mentor.list.delete')}
                  aria-label={t('mentor.list.delete')}
                  onClick={(e) => { e.stopPropagation(); void removeParcours(p.disc_id); }}
                >
                  <Trash2 size={14} />
                </button>
              )}
            </div>
          </div>
          {kind && (
            <span className="mentor-pcard-kind">{kindGlyph(kind)} {t(`mentor.onboarding.kind.${kind}`)}</span>
          )}
          <span className="mentor-pcard-title">{p.title || p.objective}</span>
          <div className="mentor-pcard-foot">
            {generating ? (
              <span className="mentor-pcard-status st-generating">
                <Loader2 size={11} className="mentor-spin" /> {t('mentor.status.generating')}
              </span>
            ) : genFailed ? (
              <>
                <span className="mentor-pcard-status st-failed">{t('mentor.list.genFailed')}</span>
                {p.generation_error && <span className="mentor-pcard-err">{p.generation_error}</span>}
              </>
            ) : (
              <>
                <div className="mentor-pcard-foot-row">
                  <span className={`mentor-pcard-status st-${p.status}`}>{t(`mentor.status.${p.status}`)}</span>
                  <span className="mentor-pcard-count">{p.progress_done}/{p.progress_total}</span>
                </div>
                <span className={`mentor-pcard-bar${done ? ' done' : ''}`}><span style={{ width: `${pct}%` }} /></span>
              </>
            )}
          </div>
        </div>
      );
    };
    // Group / filter the landing list by linked project (onboarding is
    // project-scoped). At scale a flat "all projects" list is hard to scan, so
    // we expose a project filter and always sink finished parcours below the
    // in-progress ones inside each rendered section.
    const NONE = '__none__';
    const bucketOf = (p: ParcoursSummary) => p.project_id ?? NONE;
    // Distinct project buckets present in the list (real projects A→Z, then "no project").
    const bucketMap = new Map<string, string>();
    for (const p of parcoursList) {
      const key = bucketOf(p);
      if (!bucketMap.has(key)) bucketMap.set(key, p.project_id ? (p.project_name ?? p.project_id) : t('mentor.list.noProject'));
    }
    const buckets = [...bucketMap.entries()]
      .map(([key, label]) => ({ key, label, count: parcoursList.filter((p) => bucketOf(p) === key).length }))
      .sort((a, b) => (a.key === NONE ? 1 : b.key === NONE ? -1 : a.label.localeCompare(b.label)));
    // A filter whose bucket was emptied (e.g. after a delete) falls back to "all".
    const activeFilter = listFilter !== 'all' && bucketMap.has(listFilter) ? listFilter : 'all';
    const showFilter = buckets.length > 1;
    // Keep per-project grouping only in the unfiltered multi-project view; a
    // single-project filter renders flat (the active chip already names it).
    const grouped = activeFilter === 'all' && buckets.length > 1;
    const visible = activeFilter === 'all' ? parcoursList : parcoursList.filter((p) => bucketOf(p) === activeFilter);
    // Populated → full-width dashboard (grid of card tiles, follows the layout
    // density). Empty/loading → a narrow centred hero with the create CTA.
    const hasParcours = !listLoading && parcoursList.length > 0;
    // Render a set of cards, sinking done ones under a "Terminés" divider.
    const renderSectioned = (items: ParcoursSummary[]) => {
      const active = items.filter((p) => p.status !== 'done');
      const done = items.filter((p) => p.status === 'done');
      // Responsive card grid: width from the layout-density config becomes
      // columns (auto-fill), so cards never stretch. The "Terminés" divider
      // spans the full row (grid-column: 1 / -1).
      return (
        <div className="mentor-pgrid">
          {active.map(renderCard)}
          {active.length > 0 && done.length > 0 && (
            <div className="mentor-plist-sub">{t('mentor.list.doneSection')} ({done.length})</div>
          )}
          {done.map(renderCard)}
        </div>
      );
    };
    // Existing parcours for the create-form's picked project, keyed by their
    // source topic id — lets the onboarding catalogue show each topic's state
    // and offer "reprendre" instead of silently generating a duplicate. The
    // list is newest-first, so the first hit per topic is the most relevant.
    const topicParcours = new Map<string, ParcoursSummary>();
    for (const p of parcoursList) {
      if (p.project_id === newProjectId && p.topic_id && !topicParcours.has(p.topic_id)) {
        topicParcours.set(p.topic_id, p);
      }
    }
    // #1/#2 — where to steer the newcomer next + overall cursus progress.
    const nextTopic = recommendedNextTopic(catalog, topicParcours);
    const prog = curriculumProgress(catalog, topicParcours);

    return (
      <div className={`mentor-page mentor-landing${hasParcours ? ' is-populated' : ''}`}>
        <header className="mentor-landing-head">
          <div className="mentor-loader-icon"><GraduationCap size={28} /></div>
          <div className="mentor-landing-head-txt">
            <h1 className="mentor-landing-title">{t('mentor.title')}</h1>
            <p className="mentor-landing-sub">{t('mentor.subtitle')}</p>
          </div>
          <button
            className={`mentor-landing-cta ${hasParcours ? 'mentor-btn-ghost' : 'mentor-btn'}`}
            onClick={() => setShowCreate((v) => !v)}
          >
            <Sparkles size={15} /> {hasParcours ? t('mentor.create.button') : t('mentor.create.first')}
          </button>
        </header>

        {/* Create panel sits directly under the header (above the list) so the
            create flow reads top-down; DOM order matches the visual order. */}
        {showCreate && (
            <div className="mentor-create">
              <div className="mentor-mode-toggle" role="radiogroup" aria-label={t('mentor.create.mode')}>
                <button
                  type="button"
                  role="radio"
                  aria-checked={newMode === 'mentor'}
                  className={`mentor-selectable mentor-mode-opt${newMode === 'mentor' ? ' active' : ''}`}
                  onClick={() => setNewMode('mentor')}
                >
                  <GraduationCap size={15} />
                  <span className="mentor-mode-name">{t('mentor.create.modeMentor')}</span>
                  <span className="mentor-mode-desc">{t('mentor.create.modeMentorDesc')}</span>
                </button>
                <button
                  type="button"
                  role="radio"
                  aria-checked={newMode === 'onboarding'}
                  className={`mentor-selectable mentor-mode-opt${newMode === 'onboarding' ? ' active' : ''}`}
                  onClick={() => setNewMode('onboarding')}
                >
                  <BookOpen size={15} />
                  <span className="mentor-mode-name">{t('mentor.create.modeOnboarding')}</span>
                  <span className="mentor-mode-desc">{t('mentor.create.modeOnboardingDesc')}</span>
                </button>
              </div>
              {newMode === 'mentor' ? (
                <>
                  {/* Source first: a Jira ticket derives title + objective via the AI. */}
                  <select
                    className="mentor-input"
                    value={newSourceType}
                    onChange={(e) => setNewSourceType(e.target.value as 'free' | 'jira')}
                    aria-label={t('mentor.create.sourceLabel')}
                  >
                    <option value="free">{t('mentor.create.free')}</option>
                    <option value="jira">{t('mentor.create.jira')}</option>
                  </select>
                  {/* Optional project anchor: the generator (and live turns) read
                      the real code for richer, project-grounded mentoring. */}
                  <select
                    className="mentor-input"
                    value={newProjectId}
                    onChange={(e) => setNewProjectId(e.target.value)}
                    aria-label={t('mentor.create.projectLabel')}
                  >
                    <option value="">{t('mentor.create.projectNone')}</option>
                    {projectList.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
                  </select>
                  {newSourceType === 'free' ? (
                    <>
                      <input
                        className="mentor-input"
                        value={newTitle}
                        onChange={(e) => setNewTitle(e.target.value)}
                        placeholder={t('mentor.create.titleField')}
                      />
                      <textarea
                        className="mentor-input"
                        value={newObjective}
                        onChange={(e) => setNewObjective(e.target.value)}
                        placeholder={t('mentor.create.objective')}
                        rows={3}
                      />
                      {generatorWorkflowId ? (
                        <button className="mentor-btn-ghost" onClick={generateDraft} disabled={generating || !(newObjective.trim() || newTitle.trim())}>
                          {generating ? <Loader2 size={14} className="mentor-spin" /> : <Sparkles size={14} />}
                          {generating ? t('mentor.create.generating') : t('mentor.create.generate')}
                        </button>
                      ) : (
                        <p className="mentor-turn-note">{t('mentor.create.genRequired')}</p>
                      )}
                    </>
                  ) : (
                    <>
                      <input
                        className="mentor-input"
                        value={newTicket}
                        onChange={(e) => setNewTicket(e.target.value)}
                        placeholder={t('mentor.create.ticket')}
                      />
                      <p className="mentor-loader-hint">{t('mentor.create.jiraHint')}</p>
                      {generatorWorkflowId ? (
                        <>
                          <button className="mentor-btn-ghost" onClick={generateDraft} disabled={generating || !newTicket.trim()}>
                            {generating ? <Loader2 size={14} className="mentor-spin" /> : <Sparkles size={14} />}
                            {generating ? t('mentor.create.generating') : t('mentor.create.generateJira')}
                          </button>
                        </>
                      ) : (
                        <p className="mentor-turn-note">{t('mentor.create.genRequired')}</p>
                      )}
                    </>
                  )}
                </>
              ) : (
                <>
                  {/* Onboarding is tied to a project's docs/onboarding.md. Only list
                      projects that actually HAVE onboarding topics; clicking a project
                      opens its catalogue below (click again to collapse). */}
                  {(() => {
                    const withOnb = projectList.filter((p) => p.onboarding_count > 0);
                    if (withOnb.length === 0) {
                      return <p className="mentor-empty">{t('mentor.onboarding.noProjects')}</p>;
                    }
                    return (
                      <div className="mentor-project-list">
                        {withOnb.map((p) => (
                          <button
                            key={p.id}
                            type="button"
                            className={`mentor-selectable mentor-project-item${newProjectId === p.id ? ' active' : ''}`}
                            aria-expanded={newProjectId === p.id}
                            onClick={() => { setNewProjectId(newProjectId === p.id ? '' : p.id); setSelectedTopic(null); }}
                          >
                            <span className="mentor-project-name"><BookOpen size={13} /> {p.name}</span>
                            <span className="mentor-project-count">{p.onboarding_count} {t('mentor.onboarding.topicsCount')}</span>
                          </button>
                        ))}
                      </div>
                    );
                  })()}
                  {newProjectId && (
                    <div className="mentor-catalog-panel">
                      <div className="mentor-catalog-h"><Library size={13} />{t('mentor.onboarding.catalogTitle')}</div>
                      {/* #1/#2 — "start here" + cursus progress. Steers a newcomer
                          straight to the right topic instead of a flat list. */}
                      {catalog.length > 0 && (
                        <div className="mentor-onb-hero">
                          <div className="mentor-onb-progress">
                            <span className="mentor-onb-bar">
                              <span style={{ width: `${prog.total ? (prog.done / prog.total) * 100 : 0}%` }} />
                            </span>
                            <span className="mentor-onb-count">
                              {t('mentor.onboarding.cursusProgress').replace('{0}', String(prog.done)).replace('{1}', String(prog.total))}
                            </span>
                          </div>
                          {nextTopic ? (
                            <button
                              type="button"
                              className="mentor-btn mentor-onb-cta"
                              onClick={() => {
                                setSelectedTopic(nextTopic);
                                requestAnimationFrame(() =>
                                  document.getElementById(`onb-topic-${nextTopic.topic_id}`)?.scrollIntoView({ behavior: 'smooth', block: 'center' }));
                              }}
                            >
                              <Sprout size={14} />
                              {(onboardingTopicStatus(topicParcours.get(nextTopic.topic_id)) === 'in_progress'
                                ? t('mentor.onboarding.continueCta')
                                : t('mentor.onboarding.startCta')).replace('{0}', nextTopic.title)}
                            </button>
                          ) : (
                            <span className="mentor-onb-done"><GraduationCap size={14} />{t('mentor.onboarding.cursusDone')}</span>
                          )}
                        </div>
                      )}
                      {catalogLoading
                        ? <p className="mentor-empty"><Loader2 size={13} className="mentor-spin" /> {t('mentor.load.loading')}</p>
                        : catalog.length === 0
                          ? <p className="mentor-empty">{t('mentor.onboarding.catalogEmpty')}</p>
                          : (() => {
                            // Group the flat registry into the tronc → branche →
                            // capstone → culture curriculum (topic.kind, set from the
                            // `Type` bullet). Untyped topics fall into "autres". Headers
                            // only show when there's real structure (>1 non-empty group),
                            // so a legacy untyped registry still renders as a plain list.
                            const KINDS = ['tronc', 'branche', 'capstone', 'culture'] as const;
                            const isKnown = (k: string | null): k is typeof KINDS[number] =>
                              (KINDS as readonly string[]).includes(k ?? '');
                            const groups = [
                              ...KINDS.map((k) => ({ key: k, items: catalog.filter((tp) => tp.kind === k) })),
                              { key: 'autres', items: catalog.filter((tp) => !isKnown(tp.kind ?? null)) },
                            ].filter((g) => g.items.length > 0);
                            const showHeaders = groups.length > 1;
                            // A per-kind Lucide icon so the curriculum reads at a glance:
                            // 🌱 tronc / ⑂ branche / 🎓 capstone / 🛡️ culture / 📖 autres.
                            const kindIcon = (k: string | null) => {
                              const p = { size: 13, className: 'mentor-topic-kind' };
                              switch (k) {
                                case 'tronc': return <Sprout {...p} />;
                                case 'branche': return <GitBranch {...p} />;
                                case 'capstone': return <GraduationCap {...p} />;
                                case 'culture': return <ShieldCheck {...p} />;
                                default: return <BookOpen {...p} />;
                              }
                            };
                            const renderTopic = (topic: OnboardingTopic) => {
                              // Reflect any existing parcours for this topic so the
                              // learner sees at a glance where they stand.
                              const ex = topicParcours.get(topic.topic_id);
                              const exGen = ex && ex.status === 'generating' && !ex.generation_error;
                              const exFail = ex && !!ex.generation_error;
                              const exDone = ex && !exGen && !exFail && ex.progress_total > 0 && ex.progress_done >= ex.progress_total;
                              const isNext = nextTopic?.topic_id === topic.topic_id;
                              // Free-string registry level → one of 3 colour tiers (vert/orange/rouge).
                              const lt = levelTier(topic.level);
                              // Bottom row holds refs (left) + the "prochaine étape" / parcours-state
                              // tags (right); skip it entirely when there's nothing to show.
                              const hasBottom = topic.references.length > 0 || isNext || !!ex;
                              return (
                              <button
                                key={topic.topic_id}
                                id={`onb-topic-${topic.topic_id}`}
                                type="button"
                                className={`mentor-selectable mentor-topic${selectedTopic?.topic_id === topic.topic_id ? ' active' : ''}${isNext ? ' next' : ''}`}
                                onClick={() => { setSelectedTopic(topic); }}
                              >
                                <span className="mentor-topic-top">
                                  {kindIcon(topic.kind ?? null)}
                                  <span className="mentor-topic-title">{topic.title}</span>
                                  {/* Level pinned top-right, coloured by tier. Hidden when the
                                      registry value isn't a recognised tier (keeps the card clean). */}
                                  {lt && <Badge tone={LEVEL_TONE[lt]} className="mentor-topic-level">{topic.level}</Badge>}
                                </span>
                                {topic.scope && <span className="mentor-topic-scope">{topic.scope}</span>}
                                {hasBottom && (
                                  <span className="mentor-topic-bottom">
                                    {topic.references.length > 0 && (
                                      <span className="mentor-topic-refs">{topic.references.length} {t('mentor.onboarding.refs')}</span>
                                    )}
                                    <span className="mentor-topic-tags">
                                      {isNext && <Badge tone="accent">{t('mentor.onboarding.nextStep')}</Badge>}
                                      {ex && (
                                        exGen ? <Badge tone="accent" icon={<Loader2 size={11} className="mentor-spin" />}>{t('mentor.status.generating')}</Badge>
                                        : exFail ? <Badge tone="warning">{t('mentor.list.genFailed')}</Badge>
                                        : exDone ? <Badge tone="success" icon={<Check size={11} />}>{t('mentor.status.done')}</Badge>
                                        : <Badge tone="accent" icon={<CircleDot size={11} />}>{ex.progress_done}/{ex.progress_total}</Badge>
                                      )}
                                    </span>
                                  </span>
                                )}
                              </button>
                              );
                            };
                            return (
                              <div className="mentor-catalog-list">
                                {groups.map((g) => (
                                  <div key={g.key} className="mentor-catalog-group">
                                    {showHeaders && (
                                      <div className="mentor-catalog-group-h">{kindIcon(g.key)}{t(`mentor.onboarding.kind.${g.key}`)}</div>
                                    )}
                                    {g.items.map(renderTopic)}
                                  </div>
                                ))}
                              </div>
                            );
                          })()}
                    </div>
                  )}
                  {selectedTopic && (() => {
                    // A capstone topic spawns a socratic Mentor parcours, not a course.
                    const asMentor = selectedTopic.kind === 'capstone';
                    const wfId = asMentor ? generatorWorkflowId : courseWorkflowId;
                    if (!wfId) return <p className="mentor-turn-note">{t('mentor.onboarding.genNotConfigured')}</p>;
                    // Existing parcours for this exact topic (dedup / resume).
                    const ex = topicParcours.get(selectedTopic.topic_id);
                    const exGen = ex && ex.status === 'generating' && !ex.generation_error;
                    const exFail = ex && !!ex.generation_error;
                    const exDone = ex && !exGen && !exFail && ex.progress_total > 0 && ex.progress_done >= ex.progress_total;
                    // Shared (re)generate button.
                    const genBtn = (labelKey: string) => (
                      <button className="mentor-btn-ghost" onClick={generateCourse} disabled={generatingCourse}>
                        {generatingCourse ? <Loader2 size={14} className="mentor-spin" /> : (asMentor ? <GraduationCap size={14} /> : <Sparkles size={14} />)}
                        {generatingCourse
                          ? (asMentor ? t('mentor.onboarding.generatingCapstone') : t('mentor.onboarding.generatingCourse'))
                          : t(labelKey)}
                      </button>
                    );
                    // A parcours is already being generated for this topic — no duplicate.
                    if (exGen) {
                      return <p className="mentor-turn-note"><Loader2 size={13} className="mentor-spin" /> {t('mentor.onboarding.alreadyGenerating')}</p>;
                    }
                    // A usable parcours exists → resume it (in progress) or review /
                    // redo it (done), instead of silently generating a duplicate.
                    if (ex && !exGen && !exFail) {
                      return (
                        <>
                          <p className="mentor-turn-note">{exDone ? t('mentor.onboarding.alreadyDone') : t('mentor.onboarding.alreadyInProgress')}</p>
                          <button className="mentor-btn" onClick={() => { void openById(ex.disc_id, newProjectId); }} disabled={loading}>
                            <ChevronRight size={14} /> {exDone ? t('mentor.onboarding.review') : t('mentor.onboarding.resume')}
                          </button>
                          {exDone && genBtn('mentor.onboarding.regenerate')}
                        </>
                      );
                    }
                    // No usable parcours (none, or a previous failed one) → generate fresh.
                    return (
                      <>
                        {asMentor && <p className="mentor-turn-note">{t('mentor.onboarding.capstoneHint')}</p>}
                        {exFail && <p className="mentor-turn-note">{t('mentor.onboarding.previousFailed')}</p>}
                        {genBtn(asMentor ? 'mentor.onboarding.generateCapstone' : 'mentor.onboarding.generateCourse')}
                      </>
                    );
                  })()}
                </>
              )}
          </div>
        )}

        {listLoading ? (
          <p className="mentor-loader-hint"><Loader2 size={13} className="mentor-spin" /> {t('mentor.load.loading')}</p>
        ) : hasParcours ? (
          <div className="mentor-plist">
            <div className="mentor-plist-h">{t('mentor.list.title')} ({parcoursList.length})</div>
            {showFilter && (
              <div className="mentor-filter" role="tablist" aria-label={t('mentor.list.title')}>
                <button
                  type="button"
                  role="tab"
                  aria-selected={activeFilter === 'all'}
                  className={`mentor-filter-chip${activeFilter === 'all' ? ' active' : ''}`}
                  onClick={() => setListFilter('all')}
                >
                  {t('mentor.list.filterAll')} ({parcoursList.length})
                </button>
                {buckets.map((b) => (
                  <button
                    key={b.key}
                    type="button"
                    role="tab"
                    aria-selected={activeFilter === b.key}
                    className={`mentor-filter-chip${activeFilter === b.key ? ' active' : ''}`}
                    onClick={() => setListFilter(b.key)}
                  >
                    {b.label} ({b.count})
                  </button>
                ))}
              </div>
            )}
            {grouped
              ? buckets.map((b) => (
                  <div key={b.key} className="mentor-pgroup">
                    <div className="mentor-pgroup-h">{b.label}<span className="mentor-pgroup-n">{b.count}</span></div>
                    {renderSectioned(parcoursList.filter((p) => bucketOf(p) === b.key))}
                  </div>
                ))
              : renderSectioned(visible)}
          </div>
        ) : !showCreate ? (
          <p className="mentor-empty-lead">{t('mentor.list.emptyLead')}</p>
        ) : null}
        {error && <p className="mentor-loader-error">{t('mentor.error.generic')} — {error}</p>}
      </div>
    );
  }

  return (
    <ParcoursView
      parcours={parcours}
      onExit={() => { setParcours(null); setLoadedDiscId(null); }}
      onUpdate={setParcours}
      discId={loadedDiscId}
      projectId={loadedProjectId}
      workflowId={workflowId}
    />
  );
}

function ParcoursView({
  parcours,
  onExit,
  onUpdate,
  discId,
  projectId,
  workflowId,
}: {
  parcours: MentorState;
  onExit: () => void;
  onUpdate: (s: MentorState) => void;
  discId: string | null;
  projectId: string | null;
  workflowId: string | null;
}) {
  const { t } = useT();
  const [advancing, setAdvancing] = useState(false);
  // Index of the resource whose read-flag is currently being persisted (null = idle).
  const [readingIdx, setReadingIdx] = useState<number | null>(null);
  // Repo-file preview modal (a resource that points at a project file).
  const [fileView, setFileView] = useState<{ path: string; content: string | null; error: string | null } | null>(null);
  // Last failed learner action (advance / mark-read) — surfaced near the action
  // instead of being swallowed, so a 4xx from the gate is visible.
  const [actionError, setActionError] = useState<string | null>(null);

  // Both the "Coup de pouce" and the closure synthesis run server-side. While
  // either is still generating, poll the parcours so it flips to its final state
  // on its own — so the result is also waiting on return if the learner navigated
  // away mid-run.
  const pending = parcours.last_hint?.status === 'pending'
    || parcours.last_turn?.status === 'pending'
    || parcours.bilan_synthesis?.status === 'pending';
  useEffect(() => {
    if (!discId || !pending) return;
    let cancelled = false;
    const id = setInterval(() => {
      mentorApi.getParcours(discId)
        .then((s) => { if (!cancelled) onUpdate(s); })
        .catch(() => { /* transient — keep polling on the next tick */ });
    }, 2500);
    return () => { cancelled = true; clearInterval(id); };
  }, [discId, pending, onUpdate]);

  // Widen the app shell (`.dash-main`, capped at 1000px) while a parcours is open,
  // so the clickable rail sits in the left gutter without shrinking the content.
  // Scoped to the parcours view (not the landing list); reverted on unmount.
  useEffect(() => {
    document.body.dataset.mentorWide = '1';
    return () => { delete document.body.dataset.mentorWide; };
  }, []);

  // Escape closes the repo-file preview modal (house convention).
  useEffect(() => {
    if (!fileView) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setFileView(null); };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [fileView]);

  /** Open a project file (resource `kind: repo`) in an in-page code viewer. */
  async function openRepoFile(path: string) {
    if (!projectId) return;
    setFileView({ path, content: null, error: null });
    try {
      const f = await projectsApi.readRepoFile(projectId, path);
      setFileView({ path, content: f.content, error: null });
    } catch (e) {
      setFileView({ path, content: null, error: e instanceof Error ? e.message : String(e) });
    }
  }

  const isOnboarding = parcours.mode === 'onboarding';

  // Memoized on `parcours` so it keeps a stable reference between renders — the
  // railSteps memo below depends on it, and a fresh array every render would
  // defeat that cache.
  const gatedBlocks: MentorBlock[] = useMemo(() => {
    // Target has no `validated` flag of its own — it's validated once the learner
    // has advanced PAST it (clicked "validate this block"), not just because the
    // generator pre-filled archi + tests (which would show it green from the start).
    // Mirrors the backend `progress()` rule so the landing count agrees.
    const pastTarget = parcours.status === 'done'
      || PHASE_SEQ.indexOf(parcours.phase) > PHASE_SEQ.indexOf('target');
    // Resources, like Target, has no `validated` flag of its own: it counts as
    // done once the learner has advanced PAST it — never merely because every
    // resource is ticked while still on an earlier block (and `[].every()` is
    // vacuously true, which would otherwise show an empty ② green from the start).
    const pastResources = parcours.status === 'done'
      || PHASE_SEQ.indexOf(parcours.phase) > PHASE_SEQ.indexOf('resources');
    return [
      parcours.comprehension,
      { unlocked: parcours.resources.length > 0, validated: pastResources, mentor_approved: false, revisions: 0, turns: [], forced: false },
      { unlocked: !!(parcours.target_archi || parcours.target_tests), validated: pastTarget, mentor_approved: false, revisions: 0, turns: [], forced: false },
      parcours.plan,
      parcours.code,
      parcours.bilan,
    ];
  }, [parcours]);

  // Clickable-step rail: one entry per gated block (mentor) or chapter (onboarding),
  // with a done/active/locked state derived from the gate + current phase.
  const railSteps: RailStep[] = useMemo(() => {
    if (isOnboarding) {
      const curCh = parcours.chapters.findIndex((c) => !c.done);
      return parcours.chapters.map((c, i) => ({
        id: `mentor-ch-${i}`,
        num: i + 1,
        label: c.title,
        state: c.done ? 'done' : i === curCh ? 'active' : 'locked',
      }));
    }
    const isDone = parcours.status === 'done';
    const curIdx = PHASE_SEQ.indexOf(parcours.phase);
    const resTotal = parcours.resources.length;
    const resRead = parcours.resources.filter((r) => r.read).length;
    return PHASE_SEQ.map((phase, i) => {
      const gb = gatedBlocks[i];
      const state: RailStep['state'] =
        isDone || gb.validated || i < curIdx ? 'done' : i === curIdx ? 'active' : 'locked';
      const sub = phase === 'resources' && resTotal > 0
        ? t('mentor.rail.resourcesRead', resRead, resTotal)
        : state === 'done' ? t('mentor.rail.done')
          : state === 'active' ? t('mentor.rail.current')
            : t('mentor.rail.locked');
      return { id: `mentor-blk-${phase}`, num: i + 1, label: t(`mentor.block.${phase}`), sub, state };
    });
  }, [isOnboarding, parcours, gatedBlocks, t]);
  // Completed-step count for the header tally (the mobile progress cue — the rail
  // is hidden < 900px). Derived from the same rail states shown in the gutter.
  const railDone = railSteps.filter((s) => s.state === 'done').length;
  // Target archi renders as a Mermaid diagram when it's diagram source, else as text.
  const archiMermaid = useMemo(
    () => (parcours.target_archi ? mermaidSource(parcours.target_archi) : null),
    [parcours.target_archi],
  );

  function statusLabel(): string {
    switch (parcours.status) {
      case 'generating': return t('mentor.status.generating');
      case 'draft': return t('mentor.status.draft');
      case 'validated': return t('mentor.status.validated');
      case 'open': return t('mentor.status.open');
      case 'done': return t('mentor.status.done');
      default: return parcours.status;
    }
  }

  function pill(block: MentorBlock) {
    if (!block.unlocked) return { cls: 'locked', label: t('mentor.blockState.locked'), icon: <Lock size={11} /> };
    if (block.validated) return { cls: 'ok', label: t('mentor.blockState.validated'), icon: <Check size={11} /> };
    return { cls: 'warn', label: t('mentor.blockState.toReview'), icon: <CircleDot size={11} /> };
  }

  /** Validate the current block and unlock the next. `force` is the self-serve
   *  "Passer outre" override that bypasses the read / mentor-approval gates. */
  async function advance(phase: MentorPhase, force = false) {
    if (!discId) return;
    setAdvancing(true);
    setActionError(null);
    try {
      onUpdate(await mentorApi.advance(discId, { block: phase, force }));
    } catch (e) { setActionError(e instanceof Error ? e.message : String(e)); }
    finally { setAdvancing(false); }
  }

  /** Persist a resource's read/unread flag (block ② Resources). The whole gate —
   *  pill + advance eligibility — is driven off the server `read` field. */
  async function toggleRead(index: number, read: boolean) {
    if (!discId || readingIdx !== null) return;
    setReadingIdx(index);
    setActionError(null);
    try {
      onUpdate(await mentorApi.setResourceRead(discId, { index, read }));
    } catch (e) { setActionError(e instanceof Error ? e.message : String(e)); }
    finally { setReadingIdx(null); }
  }

  /** The "validate this block → next step" action, shown only on the block that
   *  is the parcours' current phase while it's open. Bilan finishes the run.
   *  A render helper (not a component) so it can close over local state without
   *  remounting on every render — see react-hooks/static-components. */
  const renderAdvanceBar = (phase: MentorPhase) => {
    if (!discId || parcours.status !== 'open' || parcours.phase !== phase) return null;
    const isLast = phase === 'bilan';
    // Resources is a read-gate: match the backend — every resource must be read
    // before this block can be validated (otherwise advance() 400s silently).
    const resourcesBlocked = phase === 'resources'
      && parcours.resources.length > 0 && !parcours.resources.every((r) => r.read);
    // Learner blocks need the mentor's sign-off (turn evaluateur → mentor_approved)
    // before advancing. The learner can self-serve past it with `force` ("Passer outre").
    const learnerBlock = (['comprehension', 'plan', 'code', 'bilan'] as MentorPhase[]).includes(phase)
      ? parcours[phase as 'comprehension' | 'plan' | 'code' | 'bilan'] : null;
    const approvalBlocked = !!learnerBlock && !learnerBlock.mentor_approved;
    return (
      <>
      <div className="mentor-turn-actions mentor-advance-bar">
        <button className="mentor-btn" onClick={() => advance(phase)} disabled={advancing || resourcesBlocked || approvalBlocked}>
          {advancing ? <Loader2 size={14} className="mentor-spin" /> : <Check size={14} />}
          {advancing ? t('mentor.block.advancing') : (isLast ? t('mentor.block.finish') : t('mentor.block.advance'))}
        </button>
        {resourcesBlocked && <span className="mentor-advance-hint">{t('mentor.resources.gate')}</span>}
        {approvalBlocked && (
          <>
            <span className="mentor-advance-hint">{t('mentor.approval.gate')}</span>
            <button
              className="mentor-btn-ghost mentor-force"
              onClick={() => advance(phase, true)}
              disabled={advancing}
              title={t('mentor.approval.force')}
            >
              <KeyRound size={14} />
              {t('mentor.approval.force')}
            </button>
          </>
        )}
      </div>
      {actionError && <p className="mentor-turn-error" role="alert">{t('mentor.error.action')} — {actionError}</p>}
      </>
    );
  };

  const sourceLabel = parcours.source.type === 'jira'
    ? (parcours.source.ticket_key ?? t('mentor.source.jira'))
    : t('mentor.source.free');

  return (
    <div className="mentor-page mentor-parcours">
      <header className="mentor-header">
        <div className="mentor-eyebrow">
          <span className="mentor-ticket">{sourceLabel}</span>
          <span className={`mentor-pill mentor-pill-${parcours.status === 'done' ? 'ok' : 'live'}`}>
            <span className="mentor-pill-dot" />{statusLabel()}
          </span>
          <button className="mentor-btn-ghost mentor-exit" onClick={onExit}>{t('mentor.exit')}</button>
        </div>
        <h1 className="mentor-h1">{parcours.objective}</h1>

        {parcours.criteria.length > 0 && (
          <ul className="mentor-criteria">
            {parcours.criteria.map((c, i) => (
              <li key={i}><Check size={13} className="mentor-crit-k" />{c}</li>
            ))}
          </ul>
        )}

        <div className="mentor-progress-row">
          {isOnboarding
            ? <span className="mentor-guard mentor-guard-course"><BookOpen size={13} className="mentor-guard-ic" />{t('mentor.onboarding.badge')}</span>
            : <span className="mentor-guard"><ShieldCheck size={13} className="mentor-guard-ic" />{t('mentor.guardrail')}</span>}
          {railSteps.length > 0 && (
            <span
              className="mentor-progress-tally"
              aria-label={t(isOnboarding ? 'mentor.onboarding.progress' : 'mentor.progress', railDone, railSteps.length)}
            >
              {railDone}/{railSteps.length}
            </span>
          )}
        </div>
      </header>

      <div className="mentor-layout">
        <StepRail title={t('mentor.rail.title')} steps={railSteps} />
        <div className="mentor-main">
      {isOnboarding ? (
        <ChaptersView parcours={parcours} discId={discId} onUpdate={onUpdate} />
      ) : (
      <div className="mentor-stream">
        {/* ① Comprehension */}
        <BlockCard id="mentor-blk-comprehension" n={1} title={t('mentor.block.comprehension')} pill={pill(parcours.comprehension)}>
          <TurnHistory turns={parcours.comprehension.turns} />
          {parcours.comprehension.unlocked && !parcours.comprehension.validated && (
            <TurnPanel discId={discId} workflowId={workflowId} subject={parcours.objective} block="comprehension"
              hintLevel={parcours.hint_level} hint={parcours.last_hint ?? null} onUpdate={onUpdate} />
          )}
          {renderAdvanceBar('comprehension')}
          {parcours.comprehension.forced && <ForcedNote />}
        </BlockCard>

        {/* ② Resources */}
        <BlockCard id="mentor-blk-resources" n={2} title={t('mentor.block.resources')} pill={pill(gatedBlocks[1])}>
          {parcours.resources.length === 0
            ? <p className="mentor-empty">{t('mentor.resources.empty')}</p>
            : (
              <div className="mentor-res-grid">
                {parcours.resources.map((r, i) => {
                  const busy = readingIdx === i;
                  // A project file (kind "repo" or a relative path) opens in the
                  // in-page viewer; a real http(s) URL stays an external link.
                  const looksLikeFile = r.kind === 'repo' || !/^https?:\/\//i.test(r.url);
                  const isRepoFile = !!projectId && looksLikeFile;
                  const body = (
                    <span className="mentor-res-body">
                      <span className="mentor-res-kind">{looksLikeFile ? t('mentor.resources.kindFile') : r.kind}</span>
                      <span className="mentor-res-title">{r.title}</span>
                      {r.url && <span className="mentor-res-path">{r.url}</span>}
                    </span>
                  );
                  return (
                    <div key={i} className={`mentor-res${r.read ? ' read' : ''}`}>
                      <button
                        type="button"
                        className="mentor-res-check"
                        role="checkbox"
                        aria-checked={r.read}
                        aria-label={r.read ? t('mentor.resources.read') : t('mentor.resources.toRead')}
                        disabled={readingIdx !== null}
                        onClick={() => toggleRead(i, !r.read)}
                      >
                        {busy ? <Loader2 size={11} className="mentor-spin" /> : (r.read && <Check size={11} />)}
                      </button>
                      {isRepoFile
                        ? (
                          <button
                            type="button"
                            className="mentor-res-link mentor-res-file"
                            onClick={() => { openRepoFile(r.url); if (!r.read) toggleRead(i, true); }}
                          >
                            {body}
                          </button>
                        )
                        : r.url
                          ? (
                            <a
                              className="mentor-res-link"
                              href={r.url}
                              target="_blank"
                              rel="noopener noreferrer"
                              onClick={() => { if (!r.read) toggleRead(i, true); }}
                            >
                              {body}
                            </a>
                          )
                          : body}
                    </div>
                  );
                })}
              </div>
            )}
          {renderAdvanceBar('resources')}
        </BlockCard>

        {/* ③ Target */}
        <BlockCard id="mentor-blk-target" n={3} title={t('mentor.block.target')} pill={pill(gatedBlocks[2])}>
          {!parcours.target_archi && !parcours.target_tests
            ? <p className="mentor-empty">{t('mentor.target.empty')}</p>
            : (
              <div className="mentor-target-grid">
                {parcours.target_archi && (
                  <div className={`mentor-panel${archiMermaid ? ' mentor-panel-diagram' : ''}`}>
                    <div className="mentor-panel-h">{t('mentor.target.archi')}</div>
                    {archiMermaid
                      ? (
                        <>
                          <MermaidDiagram source={archiMermaid} />
                          <p className="mentor-arch-caption">{t('mentor.target.archiHint')}</p>
                        </>
                      )
                      : <pre className="mentor-pre">{parcours.target_archi}</pre>}
                  </div>
                )}
                {parcours.target_tests && (
                  <div className="mentor-panel">
                    <div className="mentor-panel-h">{t('mentor.target.tests')}</div>
                    <pre className="mentor-pre hljs" dangerouslySetInnerHTML={{ __html: highlightCode(parcours.target_tests, 'gherkin') }} />
                  </div>
                )}
              </div>
            )}
          {renderAdvanceBar('target')}
        </BlockCard>

        {/* ④⑤⑥ learner blocks */}
        {([['plan', 4, t('mentor.block.plan')], ['code', 5, t('mentor.block.code')], ['bilan', 6, t('mentor.block.bilan')]] as [MentorPhase, number, string][]).map(([key, n, title]) => {
          const block = parcours[key as 'plan' | 'code' | 'bilan'];
          return (
            <BlockCard key={key} id={`mentor-blk-${key}`} n={n} title={title} pill={pill(block)} locked={!block.unlocked}>
              {block.unlocked ? (
                block.validated ? (
                  // Validated → read-only: the mentor's replies + the learner's own
                  // answers stay visible; no composer, no send/hint (step is done).
                  // For project-linked code, the file tree stays too (read-only).
                  key === 'code' && projectId ? (
                    <>
                      <TurnHistory turns={block.turns} collapsible />
                      <CodeReviewPanel discId={discId} workflowId={workflowId} projectId={projectId}
                        subject={parcours.objective} hintLevel={parcours.hint_level}
                        hint={parcours.last_hint ?? null} onUpdate={onUpdate} readOnly />
                    </>
                  ) : (
                    <TurnHistory turns={block.turns} />
                  )
                ) : key === 'code' && projectId ? (
                  <>
                    {/* Project-linked: the learner's work IS their file changes. */}
                    <TurnHistory turns={block.turns} collapsible />
                    <CodeReviewPanel discId={discId} workflowId={workflowId} projectId={projectId}
                      subject={parcours.objective} hintLevel={parcours.hint_level}
                      hint={parcours.last_hint ?? null} onUpdate={onUpdate} />
                    {renderAdvanceBar('code')}
                  </>
                ) : (
                  <>
                    {key === 'bilan' && <p className="mentor-bilan-intro">{t('mentor.bilan.intro')}</p>}
                    <TurnHistory turns={block.turns} />
                    <TurnPanel discId={discId} workflowId={workflowId} subject={parcours.objective} block={key}
                      hintLevel={parcours.hint_level} hint={parcours.last_hint ?? null}
                      placeholder={key === 'bilan' ? t('mentor.bilan.placeholder') : undefined} onUpdate={onUpdate} />
                    {renderAdvanceBar(key)}
                  </>
                )
              ) : (
                <div className="mentor-locked"><Lock size={20} /><p>{t('mentor.locked.hint')}</p></div>
              )}
              {block.forced && <ForcedNote />}
            </BlockCard>
          );
        })}
      </div>
      )}

      {/* Synthèse de clôture — le mentor récapitule ce qui a été appris (à la
          complétion du parcours). Persistée côté serveur, posture-aware. */}
      {parcours.bilan_synthesis && (
        <BilanSynthesisCard synthesis={parcours.bilan_synthesis} discId={discId} onUpdate={onUpdate} />
      )}
        </div>
      </div>

      {fileView && (
        <div className="mentor-file-overlay" onClick={() => setFileView(null)}>
          <div className="mentor-file-modal" role="dialog" aria-modal="true" aria-label={fileView.path} onClick={(e) => e.stopPropagation()}>
            <div className="mentor-file-head">
              <span className="mentor-file-path">{fileView.path}</span>
              <button className="mentor-file-close" onClick={() => setFileView(null)} aria-label={t('mentor.file.close')} autoFocus><X size={16} /></button>
            </div>
            {fileView.error
              ? <p className="mentor-file-error">{fileView.error}</p>
              : fileView.content == null
                ? <p className="mentor-file-loading"><Loader2 size={14} className="mentor-spin" /> {t('mentor.load.loading')}</p>
                : <pre className="mentor-pre hljs mentor-file-pre" dangerouslySetInnerHTML={{ __html: highlightCode(fileView.content, languageForPath(fileView.path)) }} />}
          </div>
        </div>
      )}
    </div>
  );
}

/** The mentor's closure synthesis — a keepable recap generated when the parcours
 *  completes (learner-first in mentor mode, direct recap in onboarding). Pending
 *  shows a spinner; ready renders Markdown; failed offers a retry. Generation runs
 *  server-side, so it survives reloads and finishes even if the learner leaves. */
function BilanSynthesisCard({
  synthesis, discId, onUpdate,
}: {
  synthesis: BilanSynthesis;
  discId: string | null;
  onUpdate: (s: MentorState) => void;
}) {
  const { t } = useT();
  const [retrying, setRetrying] = useState(false);
  const [retryError, setRetryError] = useState<string | null>(null);
  const busy = retrying || synthesis.status === 'pending';

  async function retry() {
    if (!discId || busy) return;
    setRetrying(true); setRetryError(null);
    try { onUpdate(await mentorApi.regenerateBilan(discId)); }
    catch (e) { setRetryError(e instanceof Error ? e.message : String(e)); }
    finally { setRetrying(false); }
  }

  return (
    <section className="mentor-synth">
      <div className="mentor-synth-head">
        <span className="mentor-synth-title"><Sparkles size={14} /> {t('mentor.bilan.synthTitle')}</span>
        {(synthesis.status === 'ready' || synthesis.status === 'failed') && discId && (
          <button className="mentor-btn-ghost" onClick={retry} disabled={busy}>
            {busy ? <Loader2 size={14} className="mentor-spin" /> : <Sparkles size={14} />}
            {t('mentor.bilan.retry')}
          </button>
        )}
      </div>
      {retryError && <p className="mentor-turn-error" role="alert">{t('mentor.error.action')} — {retryError}</p>}
      {synthesis.status === 'pending' && (
        <p className="mentor-loader-hint"><Loader2 size={13} className="mentor-spin" /> {t('mentor.bilan.pending')}</p>
      )}
      {synthesis.status === 'failed' && (
        <p className="mentor-turn-error">{t('mentor.bilan.failed')}{synthesis.error ? ` — ${synthesis.error}` : ''}</p>
      )}
      {synthesis.status === 'ready' && synthesis.text && (
        <div className="mentor-synth-body mentor-md">
          <ReactMarkdown remarkPlugins={MD_PLUGINS} components={MD_COMPONENTS}>{synthesis.text}</ReactMarkdown>
        </div>
      )}
    </section>
  );
}

/** Onboarding posture: a linear course of chapter cards. Chapter i is unlocked
 *  once chapter i-1 is done; each ends with an optional checkpoint (quiz or a
 *  reveal-the-answer exercise). Expository — no censor, no hint ladder. */
export function ChaptersView({
  parcours, discId, onUpdate,
}: {
  parcours: MentorState;
  discId: string | null;
  onUpdate: (s: MentorState) => void;
}) {
  const { t } = useT();
  const chapters = parcours.chapters;

  if (chapters.length === 0) {
    return <div className="mentor-stream"><p className="mentor-empty">{t('mentor.onboarding.empty')}</p></div>;
  }

  const allDone = chapters.every((c) => c.done);
  // #4b — spaced re-test: once the course is done, the chapters the learner
  // struggled on (needed >1 attempt on a quiz) resurface for a targeted retrieval
  // pass. Passing them cleanly clears the flag; this is where durable learning
  // happens (re-testing weak items beats re-reading).
  const toReview = allDone ? chapters.map((c, i) => ({ c, i })).filter((x) => x.c.needs_review) : [];

  return (
    <div className="mentor-stream">
      {chapters.map((ch, i) => (
        <ChapterCard
          key={i}
          index={i}
          chapter={ch}
          unlocked={i === 0 || chapters[i - 1].done}
          discId={discId}
          onUpdate={onUpdate}
        />
      ))}
      {allDone && (
        <div className="mentor-course-done">
          <span className="mentor-pill mentor-pill-ok"><Check size={11} />{t('mentor.onboarding.courseComplete')}</span>
        </div>
      )}
      {toReview.length > 0 && (
        <div className="mentor-review">
          <div className="mentor-review-h">
            <RotateCcw size={14} />
            <span>{t('mentor.onboarding.reviewTitle').replace('{0}', String(toReview.length))}</span>
          </div>
          <p className="mentor-review-intro">{t('mentor.onboarding.reviewIntro')}</p>
          {toReview.map(({ c, i }) => (
            <ChapterCard
              key={`review-${i}`}
              index={i}
              chapter={c}
              unlocked
              discId={discId}
              onUpdate={onUpdate}
              reviewMode
            />
          ))}
        </div>
      )}
    </div>
  );
}

/** One onboarding chapter: explanation + optional checkpoint. The learner must
 *  engage the checkpoint (pick a quiz option / reveal the exercise answer)
 *  before marking the chapter done, which unlocks the next one. */
/** Fold the legacy single `checkpoint` into the `checkpoints` list so old and
 *  new courses render the same way. */
function chapterCheckpoints(chapter: Chapter): Checkpoint[] {
  if (chapter.checkpoints && chapter.checkpoints.length > 0) return chapter.checkpoints;
  return chapter.checkpoint ? [chapter.checkpoint] : [];
}

export function ChapterCard({
  index, chapter, unlocked, discId, onUpdate, reviewMode = false,
}: {
  index: number;
  chapter: Chapter;
  unlocked: boolean;
  discId: string | null;
  onUpdate: (s: MentorState) => void;
  /** #4b re-test: re-play a done+flagged chapter's quizzes fresh; a clean pass
   *  clears `needs_review`. */
  reviewMode?: boolean;
}) {
  const { t } = useT();
  const checkpoints = chapterCheckpoints(chapter);
  const n = checkpoints.length;
  // In review mode a done chapter is interactive again (re-test); otherwise a
  // done chapter is a read-only recap showing the right answers.
  const interactive = !chapter.done || reviewMode;
  const showResult = chapter.done && !reviewMode;
  // Per-checkpoint local state (arrays parallel to `checkpoints`).
  const [selected, setSelected] = useState<(number | null)[]>(() => Array(n).fill(null));
  const [answers, setAnswers] = useState<string[]>(() => Array(n).fill(''));
  const [revealed, setRevealed] = useState<boolean[]>(() => Array(n).fill(false));
  // #4b — the learner picked a wrong quiz option at least once → the chapter is
  // flagged for the end-of-course spaced re-test.
  const [struggled, setStruggled] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const setAt = <T,>(setter: React.Dispatch<React.SetStateAction<T[]>>, i: number, v: T) =>
    setter((prev) => { const c = [...prev]; c[i] = v; return c; });

  function pick(i: number, oi: number, cp: Checkpoint) {
    setAt<number | null>(setSelected, i, oi);
    if (cp.answer != null && oi !== cp.answer) setStruggled(true);
  }

  // A quiz needs the CORRECT answer (per-option feedback lets the learner retry
  // and learn from a wrong pick); an open exercise needs a written attempt. A
  // completed chapter means "understood", not just "clicked". Legacy quiz with no
  // `answer` falls back to any pick. A chapter with no checkpoint is always ok.
  const engaged = checkpoints.every((cp, i) => {
    const isQuiz = cp.options.length > 0;
    if (isQuiz) return cp.answer != null ? selected[i] === cp.answer : selected[i] != null;
    // Exercises aren't re-demanded during a review re-test (they have no wrong/right).
    return reviewMode ? true : answers[i].trim() !== '';
  });
  const canComplete = !!discId && unlocked && interactive && (n === 0 || engaged);

  async function markDone() {
    if (!discId) return;
    setSaving(true);
    setSaveError(null);
    try {
      // In review mode, don't overwrite the saved exercise answer.
      const exerciseAnswer = reviewMode ? undefined : (checkpoints
        .map((cp, i) => (cp.options.length === 0 ? answers[i].trim() : ''))
        .filter(Boolean)
        .join('\n\n') || undefined);
      onUpdate(await mentorApi.completeChapter(discId, index, exerciseAnswer, struggled));
    } catch (e) { setSaveError(e instanceof Error ? e.message : String(e)); }
    finally { setSaving(false); }
  }

  const pillCls = chapter.done ? 'ok' : unlocked ? 'warn' : 'locked';
  const pillIcon = chapter.done ? <Check size={11} /> : unlocked ? <CircleDot size={11} /> : <Lock size={11} />;
  const pillLabel = chapter.done
    ? t('mentor.onboarding.done')
    : unlocked ? t('mentor.blockState.toReview') : t('mentor.blockState.locked');

  return (
    <section id={`mentor-ch-${index}`} className={`mentor-block${!unlocked ? ' mentor-block-locked' : ''}`}>
      <div className="mentor-bhead">
        <span className="mentor-bnum">{index + 1}</span>
        <span className="mentor-btitle">{chapter.title}</span>
        <span className="mentor-spacer" />
        {chapter.done && chapter.needs_review && (
          <span className="mentor-pill mentor-pill-warn" title={t('mentor.onboarding.toReviewTip')}>
            <RotateCcw size={11} />{t('mentor.onboarding.toReview')}
          </span>
        )}
        <span className={`mentor-pill mentor-pill-${pillCls}`}>{pillIcon}{pillLabel}</span>
      </div>
      <div className="mentor-bbody">
        {!unlocked ? (
          <div className="mentor-locked"><Lock size={20} /><p>{t('mentor.onboarding.locked')}</p></div>
        ) : (
          <>
            <div className="mentor-chapter-explanation mentor-md">
              <ReactMarkdown remarkPlugins={MD_PLUGINS} components={MD_COMPONENTS}>{chapter.explanation}</ReactMarkdown>
            </div>

            {checkpoints.map((cp, i) => {
              const isQuiz = cp.options.length > 0;
              return (
                <div className="mentor-checkpoint" key={i}>
                  <div className="mentor-checkpoint-h">
                    <HelpCircle size={13} />
                    {n > 1 ? t('mentor.onboarding.questionN').replace('{0}', String(i + 1)) : t('mentor.onboarding.checkpoint')}
                  </div>
                  <p className="mentor-checkpoint-q">{cp.question}</p>

                  {isQuiz && (
                    <div className="mentor-quiz">
                      {cp.options.map((opt, oi) => {
                        const picked = selected[i] === oi;
                        const isAnswer = cp.answer != null && oi === cp.answer;
                        const cls = showResult
                          ? (isAnswer ? ' correct' : '')
                          : selected[i] == null ? '' : isAnswer ? ' correct' : picked ? ' wrong' : '';
                        return (
                          <button
                            key={oi}
                            className={`mentor-quiz-opt${cls}`}
                            onClick={() => pick(i, oi, cp)}
                            disabled={!interactive}
                          >
                            {opt}
                          </button>
                        );
                      })}
                      {selected[i] != null && cp.answer != null && (
                        <p className={`mentor-quiz-fb${selected[i] === cp.answer ? ' ok' : ''}`} role="status" aria-live="polite">
                          {cp.explanations[selected[i] as number]
                            ? cp.explanations[selected[i] as number]
                            : selected[i] === cp.answer ? t('mentor.onboarding.quizCorrect') : t('mentor.onboarding.quizWrong')}
                        </p>
                      )}
                    </div>
                  )}

                  {!isQuiz && (
                    <>
                      {(chapter.done) ? (
                        chapter.learner_answer && (
                          <div className="mentor-panel">
                            <div className="mentor-panel-h">{t('mentor.onboarding.yourAnswer')}</div>
                            <p className="mentor-chapter-answer">{chapter.learner_answer}</p>
                          </div>
                        )
                      ) : (
                        <textarea
                          className="mentor-turn-input"
                          value={answers[i]}
                          onChange={(e) => setAt(setAnswers, i, e.target.value)}
                          placeholder={t('mentor.onboarding.answerPlaceholder')}
                          rows={3}
                        />
                      )}
                      {(revealed[i] || chapter.done)
                        ? (
                          <div className="mentor-panel">
                            <div className="mentor-panel-h">{t('mentor.onboarding.revealLabel')}</div>
                            <pre className="mentor-pre hljs" dangerouslySetInnerHTML={{ __html: highlightCode(cp.reveal ?? '', null) }} />
                          </div>
                        )
                        : (
                          <>
                            <button
                              className="mentor-btn-ghost"
                              onClick={() => setAt<boolean>(setRevealed, i, true)}
                              disabled={!answers[i].trim()}
                              title={!answers[i].trim() ? t('mentor.onboarding.revealGate') : undefined}
                            >
                              <Eye size={14} />{t('mentor.onboarding.reveal')}
                            </button>
                            {!answers[i].trim() && <span className="mentor-advance-hint">{t('mentor.onboarding.revealGate')}</span>}
                          </>
                        )}
                    </>
                  )}
                </div>
              );
            })}

            {(!chapter.done || reviewMode) && (
              <div className="mentor-turn-actions mentor-chapter-actions">
                <button className="mentor-btn" onClick={markDone} disabled={!canComplete || saving}>
                  {saving ? <Loader2 size={14} className="mentor-spin" /> : <Check size={14} />}
                  {saving
                    ? t('mentor.onboarding.completing')
                    : reviewMode ? t('mentor.onboarding.revalidate') : t('mentor.onboarding.markDone')}
                </button>
                {saveError && <p className="mentor-turn-error" role="alert">{t('mentor.error.action')} — {saveError}</p>}
              </div>
            )}
          </>
        )}
      </div>
    </section>
  );
}

/** Shared "mentor turn" engine used by both the plain composer and the code-
 *  review panel. `send`/`askHint` take the submission string (raw text, or an
 *  assembled diff). The turn runs ENTIRELY SERVER-SIDE (mentor generated +
 *  censeur-vetted there — see `api::mentor::run_turn`): the raw answer never
 *  reaches the browser and the client never supplies the verdict, so the
 *  anti-solution guard can't be bypassed. `send` kicks off the turn, then polls
 *  until it settles; the vetted reply lands in `block.turns`. Fail-closed. */
function useMentorTurn({
  discId, workflowId, subject: _subject, block, onUpdate,
}: {
  discId: string | null;
  workflowId: string | null;
  subject: string;
  block: MentorPhase;
  onUpdate: (s: MentorState) => void;
}) {
  const [sending, setSending] = useState(false);
  const [hinting, setHinting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Optimistic echo: the learner's submission shown in the thread the instant
  // they send, with a "mentor is thinking" bubble, until the real turn lands in
  // `block.turns`. Cleared on completion, error, or submit failure.
  const [pending, setPending] = useState<string | null>(null);

  // Skip state updates once the parcours view unmounts (e.g. the learner clicks
  // "Quitter" mid-turn): otherwise a late poll response would reopen the closed
  // parcours via the parent's setter.
  const mounted = useRef(true);
  useEffect(() => () => { mounted.current = false; }, []);
  const safeUpdate = useCallback((s: MentorState) => { if (mounted.current) onUpdate(s); }, [onUpdate]);
  // Synchronous re-entry guard: the `sending` state lags a click by a render, and
  // a caller may await work (diff assembly) before calling send — a ref closes
  // that window so two quick clicks can't fire two turns.
  const inFlight = useRef(false);

  async function askHint(submission: string) {
    if (!discId || inFlight.current || sending || hinting) return;
    setHinting(true); setError(null);
    try {
      safeUpdate(await mentorApi.hint(discId, { block, submission }));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setHinting(false);
    }
  }

  async function send(submission: string): Promise<boolean> {
    if (!discId || !workflowId || !submission.trim() || inFlight.current || sending || hinting) return false;
    inFlight.current = true;
    setSending(true); setError(null); setPending(submission);
    try {
      // Run the turn server-side. We get a `last_turn = pending` state back at once;
      // the mentor answer + censeur verdict are resolved on the server (fail-closed),
      // so nothing unvetted is ever streamed here. Poll until it settles — the
      // vetted reply (or a "filtered" turn) is then in `block.turns`.
      let s = await mentorApi.runTurn(discId, { block, submission });
      safeUpdate(s);
      while (s.last_turn?.status === 'pending') {
        await new Promise((r) => setTimeout(r, 2000));
        if (!mounted.current) break;
        s = await mentorApi.getParcours(discId);
        safeUpdate(s);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      inFlight.current = false;
      setSending(false); setPending(null);
    }
    return true;
  }

  return { sending, hinting, error, pending, send, askHint };
}

/** A block's persisted dialogue: each learner submission + the mentor's reply
 *  (or a "filtered" notice when the censeur blocked it). Read-only, shown on both
 *  the active block and validated ones so the exchange is never lost. */
function TurnHistory({ turns, collapsible = false }: { turns: MentorTurn[]; collapsible?: boolean }) {
  const { t } = useT();
  if (!turns.length) return null;
  return (
    <div className="mentor-history">
      {turns.map((tn, i) => (
        <div key={i} className="mentor-exchange">
          <div className="mentor-learner-turn">
            <span className="mentor-note-from mentor-note-you"><GraduationCap size={13} /> {t('mentor.turn.youLabel')}</span>
            {collapsible ? (
              // Code submissions are large diffs — fold them so the mentor's reply
              // stays front-and-centre; click to expand.
              <details className="mentor-sub-details">
                <summary className="mentor-sub-summary">{t('mentor.turn.submittedDiff')}</summary>
                <pre className="mentor-learner-sub">{tn.submission}</pre>
              </details>
            ) : (
              <pre className="mentor-learner-sub">{tn.submission}</pre>
            )}
          </div>
          {tn.reply ? (
            <div className="mentor-note">
              <span className="mentor-note-from"><GraduationCap size={13} /> {t('mentor.turn.mentorLabel')}</span>
              <p>{tn.reply}</p>
            </div>
          ) : (
            <div className="mentor-note mentor-note-filtered">
              <span className="mentor-note-from"><ShieldCheck size={13} /> {t('mentor.turn.guardLabel')}</span>
              <p>{t('mentor.turn.filtered')}</p>
            </div>
          )}
        </div>
      ))}
    </div>
  );
}

/** Last element of a block that was manually unblocked ("Passer outre"): makes
 *  clear the pass was self-served, not the mentor's own approval. */
function ForcedNote() {
  const { t } = useT();
  return (
    <div className="mentor-forced-note">
      <KeyRound size={13} />
      <span>{t('mentor.block.forcedNote')}</span>
    </div>
  );
}

/** The turn's transient feedback: a live error, plus the persisted "Coup de
 *  pouce" state for this block. (The mentor's replies live in `TurnHistory`.) */
function TurnFeedback({
  error, hint,
}: {
  error: string | null;
  /** The block's `last_hint` (already filtered to this block by the caller). */
  hint: HintState | null;
}) {
  const { t } = useT();
  return (
    <>
      {error && <p className="mentor-turn-error" role="alert">{t('mentor.turn.failed')} — {error}</p>}

      {/* The "Coup de pouce" — generated server-side, persisted on the parcours.
          Wrapped in a live region so its status changes (pending → ready/filtered/
          failed) are announced to screen readers, not only shown visually. */}
      <div role="status" aria-live="polite">
        {hint?.status === 'pending' && (
          <p className="mentor-turn-note"><Loader2 size={13} className="mentor-spin" /> {t('mentor.hint.pending')}</p>
        )}
        {hint?.status === 'filtered' && (
          <div className="mentor-note mentor-note-filtered">
            <span className="mentor-note-from"><ShieldCheck size={13} /> {t('mentor.turn.guardLabel')}</span>
            <p>{t('mentor.turn.filtered')}</p>
          </div>
        )}
        {hint?.status === 'failed' && (
          <p className="mentor-turn-error">{t('mentor.turn.failed')}{hint.error ? ` — ${hint.error}` : ''}</p>
        )}
        {hint?.status === 'ready' && hint.text && (
          <div className="mentor-note">
            <span className="mentor-note-from"><Lightbulb size={13} /> {t('mentor.hint.label')} ({hint.level}/4)</span>
            <p>{hint.text}</p>
          </div>
        )}
      </div>
    </>
  );
}

/** The live mentor→censeur turn for one block: type content, submit it, and
 *  surface the vetted reply. Used for every learner block except ⑤ Code when a
 *  project is linked (that uses `CodeReviewPanel`). */
function TurnPanel({
  discId, workflowId, subject, block, hintLevel, hint, placeholder, onUpdate,
}: {
  discId: string | null;
  workflowId: string | null;
  subject: string;
  block: MentorPhase;
  hintLevel: number;
  /** The parcours' last "Coup de pouce" — shown only when it targets this block. */
  hint: HintState | null;
  /** Optional textarea placeholder override (e.g. bilan-specific framing). */
  placeholder?: string;
  onUpdate: (s: MentorState) => void;
}) {
  const { t } = useT();
  const [content, setContent] = useState('');
  const turn = useMentorTurn({ discId, workflowId, subject, block, onUpdate });

  if (!workflowId) {
    return <p className="mentor-turn-note">{t('mentor.turn.notConfigured')}</p>;
  }

  const blockHint = hint && hint.block === block ? hint : null;
  const hintBusy = turn.hinting || blockHint?.status === 'pending';
  const disabled = turn.sending || hintBusy;

  // Send optimistically: clear the textarea at once so the message "leaves" it
  // into the thread; restore it only if the submit call itself failed.
  async function handleSend() {
    const text = content;
    setContent('');
    const ok = await turn.send(text);
    if (!ok) setContent(text);
  }

  return (
    <div className="mentor-turn">
      {turn.pending && (
        <div className="mentor-history">
          <div className="mentor-exchange">
            <div className="mentor-learner-turn">
              <span className="mentor-note-from mentor-note-you"><GraduationCap size={13} /> {t('mentor.turn.youLabel')}</span>
              <pre className="mentor-learner-sub">{turn.pending}</pre>
            </div>
            <div className="mentor-note" role="status" aria-live="polite">
              <span className="mentor-note-from"><GraduationCap size={13} /> {t('mentor.turn.mentorLabel')}</span>
              <p className="mentor-turn-note"><Loader2 size={13} className="mentor-spin" /> {t('mentor.turn.thinking')}</p>
              <p className="mentor-turn-wait">{t('mentor.turn.wait')}</p>
            </div>
          </div>
        </div>
      )}
      <textarea
        className="mentor-turn-input"
        value={content}
        onChange={(e) => setContent(e.target.value)}
        placeholder={placeholder ?? t('mentor.turn.placeholder')}
        disabled={disabled}
        rows={4}
      />
      <div className="mentor-turn-actions">
        <button className="mentor-btn" onClick={handleSend} disabled={disabled || !content.trim()}>
          {turn.sending ? <Loader2 size={14} className="mentor-spin" /> : <Send size={14} />}
          {turn.sending ? t('mentor.turn.thinking') : t('mentor.turn.send')}
        </button>
        <button className="mentor-btn-ghost" onClick={() => turn.askHint(content)} disabled={disabled}>
          {hintBusy ? <Loader2 size={14} className="mentor-spin" /> : <Lightbulb size={14} />}
          {t('mentor.hint.button')} ({hintLevel}/4)
        </button>
      </div>

      <TurnFeedback error={turn.error} hint={blockHint} />
    </div>
  );
}

/** A unified-diff rendered with per-line coloring (no hljs 'diff' language). */
function DiffView({ diff }: { diff: string }) {
  const lines = diff.split('\n');
  return (
    <pre className="mentor-code-diff">
      {lines.map((ln, i) => {
        const cls = ln.startsWith('@@') ? 'd-hunk'
          : ln.startsWith('+++') || ln.startsWith('---') || ln.startsWith('diff ') || ln.startsWith('index ') ? 'd-meta'
          : ln.startsWith('+') ? 'd-add'
          : ln.startsWith('-') ? 'd-del'
          : '';
        return <div key={i} className={`d-line ${cls}`}>{ln || ' '}</div>;
      })}
    </pre>
  );
}

/** One changed file (working tree or committed-on-branch). */
type ChangedFile = { path: string; status: string; committed: boolean };

/** ⑤ Code, project-linked variant: instead of a free-text turn, the learner's
 *  work IS the set of files they changed in the project. Lists the modified
 *  files (uncommitted + committed-on-branch), each expandable to its diff, plus
 *  an optional note. "Envoyer" / "Coup de pouce" send the assembled diff (+ note)
 *  as the submission — the mentor reviews the real changes, still strict-socratic. */
function CodeReviewPanel({
  discId, workflowId, projectId, subject, hintLevel, hint, onUpdate, readOnly = false,
}: {
  discId: string | null;
  workflowId: string | null;
  projectId: string;
  subject: string;
  hintLevel: number;
  hint: HintState | null;
  onUpdate: (s: MentorState) => void;
  /** Validated step: keep the file tree visible but drop the note + send/hint. */
  readOnly?: boolean;
}) {
  const { t } = useT();
  const turn = useMentorTurn({ discId, workflowId, subject, block: 'code', onUpdate });
  const [note, setNote] = useState('');
  const [loading, setLoading] = useState(true);
  const [statusErr, setStatusErr] = useState<string | null>(null);
  const [branch, setBranch] = useState<string | null>(null);
  const [changed, setChanged] = useState<ChangedFile[]>([]);
  const [expanded, setExpanded] = useState<string | null>(null);
  // Per-file diff cache, keyed by `${committed?'c':'w'}:${path}`.
  const [diffs, setDiffs] = useState<Record<string, { text?: string; loading?: boolean; error?: string }>>({});

  const keyOf = (f: ChangedFile) => `${f.committed ? 'c' : 'w'}:${f.path}`;

  const loadStatus = useCallback(() => {
    let cancelled = false;
    setLoading(true);
    // Drop cached diffs: a refresh must reflect edits made since last fetch, not
    // replay a stale diff for a file that's already been expanded once.
    setDiffs({});
    projectsApi.gitStatus(projectId)
      .then((s) => {
        if (cancelled) return;
        const un = (s.files ?? []).map((f) => ({ path: f.path, status: f.status, committed: false }));
        const co = (s.committed_files ?? []).map((f) => ({ path: f.path, status: f.status, committed: true }));
        setChanged([...un, ...co]);
        setBranch(s.branch);
        setStatusErr(null);
      })
      .catch((e) => { if (!cancelled) { setChanged([]); setStatusErr(e instanceof Error ? e.message : String(e)); } })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [projectId]);

  useEffect(() => loadStatus(), [loadStatus]);

  // Auto-refresh when the window regains focus — the learner edits files in their
  // editor, then tabs back to Kronn: pick up the changes without a manual click.
  useEffect(() => {
    const onFocus = () => { if (document.visibilityState !== 'hidden') loadStatus(); };
    window.addEventListener('focus', onFocus);
    document.addEventListener('visibilitychange', onFocus);
    return () => {
      window.removeEventListener('focus', onFocus);
      document.removeEventListener('visibilitychange', onFocus);
    };
  }, [loadStatus]);

  async function loadDiff(f: ChangedFile, force = false): Promise<string> {
    const k = keyOf(f);
    const cached = diffs[k];
    if (!force && cached?.text !== undefined) return cached.text;
    setDiffs((d) => ({ ...d, [k]: { loading: true } }));
    try {
      const r = await projectsApi.gitDiff(projectId, f.path, f.committed);
      setDiffs((d) => ({ ...d, [k]: { text: r.diff } }));
      return r.diff;
    } catch (e) {
      setDiffs((d) => ({ ...d, [k]: { error: e instanceof Error ? e.message : String(e) } }));
      return '';
    }
  }

  function toggle(f: ChangedFile) {
    const k = keyOf(f);
    if (expanded === k) { setExpanded(null); return; }
    setExpanded(k);
    if (diffs[k]?.text === undefined) void loadDiff(f);
  }

  /** Assemble the submission sent to the mentor: the learner's note (if any) plus
   *  every changed file's diff, capped so a huge diff can't blow the context.
   *  Diffs are re-fetched fresh (`force`) so the mentor always sees the current
   *  state of the files, never a stale cached diff. */
  async function buildSubmission(): Promise<string> {
    const parts: string[] = [];
    if (note.trim()) parts.push(note.trim());
    if (changed.length) {
      const chunks: string[] = [];
      for (const f of changed) {
        const diff = await loadDiff(f, true);
        if (diff.trim()) {
          chunks.push(`### ${f.path} (${f.status.trim() || 'M'}${f.committed ? ', committé' : ''})\n\`\`\`diff\n${diff}\n\`\`\``);
        }
      }
      let body = chunks.join('\n\n');
      const CAP = 60000;
      if (body.length > CAP) body = `${body.slice(0, CAP)}\n\n[…diff tronqué…]`;
      if (body) parts.push(`Modifications apportées :\n\n${body}`);
    }
    return parts.join('\n\n');
  }

  const blockHint = hint && hint.block === 'code' ? hint : null;
  const hintBusy = turn.hinting || blockHint?.status === 'pending';
  const disabled = turn.sending || hintBusy;
  const nothingToSend = !note.trim() && changed.length === 0;

  // Re-entry guard: `buildSubmission` refetches every diff (a long async window)
  // BEFORE turn.send runs, and `disabled` only reflects turn.sending after send
  // starts — so two quick clicks would each assemble + fire a turn. The ref closes
  // that gap synchronously (turn.send has its own guard too, defence in depth).
  const busy = useRef(false);
  async function onSend() {
    if (disabled || busy.current) return;
    busy.current = true;
    try {
      const submission = await buildSubmission();
      if (submission.trim()) await turn.send(submission);
    } finally {
      busy.current = false;
    }
  }
  async function onHint() {
    if (disabled || busy.current) return;
    busy.current = true;
    try {
      await turn.askHint(await buildSubmission());
    } finally {
      busy.current = false;
    }
  }

  if (!workflowId && !readOnly) {
    return <p className="mentor-turn-note">{t('mentor.turn.notConfigured')}</p>;
  }

  return (
    <div className="mentor-turn">
      <div className="mentor-code-review">
        <div className="mentor-code-head">
          <span className="mentor-code-title"><FileDiff size={14} /> {t('mentor.code.filesTitle')}</span>
          <span className="mentor-spacer" />
          {branch && <span className="mentor-code-branch">{branch}</span>}
          <button className="mentor-code-refresh" onClick={loadStatus} disabled={loading} aria-label={t('mentor.code.refresh')}>
            <Loader2 size={12} className={loading ? 'mentor-spin' : ''} />
          </button>
        </div>
        {loading ? (
          <p className="mentor-loader-hint"><Loader2 size={13} className="mentor-spin" /> {t('mentor.load.loading')}</p>
        ) : statusErr ? (
          <p className="mentor-empty">{t('mentor.code.noRepo')}</p>
        ) : changed.length === 0 ? (
          <p className="mentor-empty">{t('mentor.code.noChanges')}</p>
        ) : (
          <ul className="mentor-code-files">
            {changed.map((f) => {
              const k = keyOf(f);
              const d = diffs[k];
              const open = expanded === k;
              return (
                <li key={k} className="mentor-code-file">
                  <button className="mentor-code-file-row" onClick={() => toggle(f)}>
                    <ChevronRight size={13} className={`mentor-code-caret${open ? ' open' : ''}`} />
                    <span className={`mentor-code-badge s-${(f.status.trim()[0] || 'M').toLowerCase()}`}>{f.status.trim() || 'M'}</span>
                    <span className="mentor-code-path">{f.path}</span>
                    {f.committed && <span className="mentor-code-tag">{t('mentor.code.committed')}</span>}
                  </button>
                  {open && (
                    d?.loading ? <p className="mentor-loader-hint"><Loader2 size={13} className="mentor-spin" /> {t('mentor.load.loading')}</p>
                      : d?.error ? <p className="mentor-turn-error">{d.error}</p>
                        : d?.text !== undefined ? <DiffView diff={d.text} />
                          : null
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </div>

      {!readOnly && (
        <>
          <textarea
            className="mentor-turn-input"
            value={note}
            onChange={(e) => setNote(e.target.value)}
            placeholder={t('mentor.code.notePlaceholder')}
            disabled={disabled}
            rows={3}
          />
          <div className="mentor-turn-actions">
            <button className="mentor-btn" onClick={onSend} disabled={disabled || nothingToSend}>
              {turn.sending ? <Loader2 size={14} className="mentor-spin" /> : <Send size={14} />}
              {turn.sending ? t('mentor.turn.thinking') : t('mentor.code.send')}
            </button>
            <button className="mentor-btn-ghost" onClick={onHint} disabled={disabled}>
              {hintBusy ? <Loader2 size={14} className="mentor-spin" /> : <Lightbulb size={14} />}
              {t('mentor.hint.button')} ({hintLevel}/4)
            </button>
          </div>

          {turn.sending && (
            <p className="mentor-turn-note" role="status" aria-live="polite">
              <Loader2 size={13} className="mentor-spin" /> {t('mentor.turn.thinking')} — {t('mentor.turn.wait')}
            </p>
          )}
          <TurnFeedback error={turn.error} hint={blockHint} />
        </>
      )}
    </div>
  );
}

/** One entry in the clickable step rail. */
type RailStep = { id: string; num: number; label: string; sub?: string; state: 'done' | 'active' | 'locked' };

/** Smooth-scroll to a block/chapter by DOM id (rail click). */
function jumpTo(id: string) {
  document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
}

/** The clickable step rail (left column): the parcours' gated blocks (mentor) or
 *  chapters (onboarding), each showing done ✓ / current • / locked 🔒, and jumping
 *  to its section on click. Sticky on desktop, hidden on narrow screens. */
function StepRail({ title, steps }: { title: string; steps: RailStep[] }) {
  return (
    <nav className="mentor-rail" aria-label={title}>
      <h4 className="mentor-rail-h">{title}</h4>
      {steps.map((s) => (
        <button
          key={s.id}
          type="button"
          className={`mentor-step mentor-step-${s.state}`}
          onClick={() => jumpTo(s.id)}
          aria-current={s.state === 'active' ? 'step' : undefined}
        >
          <span className="mentor-step-dot">
            {s.state === 'done' ? <Check size={12} /> : s.state === 'locked' ? <Lock size={11} /> : s.num}
          </span>
          <span className="mentor-step-lbl">
            {s.label}
            {s.sub && <span className="mentor-step-sub">{s.sub}</span>}
          </span>
        </button>
      ))}
    </nav>
  );
}

function BlockCard({
  id, n, title, pill, locked, children,
}: {
  id?: string;
  n: number;
  title: string;
  pill: { cls: string; label: string; icon: React.ReactNode };
  locked?: boolean;
  children: React.ReactNode;
}) {
  return (
    <section id={id} className={`mentor-block${locked ? ' mentor-block-locked' : ''}`}>
      <div className="mentor-bhead">
        <span className="mentor-bnum">{n}</span>
        <span className="mentor-btitle">{title}</span>
        <span className="mentor-spacer" />
        <span className={`mentor-pill mentor-pill-${pill.cls}`}>{pill.icon}{pill.label}</span>
      </div>
      <div className="mentor-bbody">{children}</div>
    </section>
  );
}
