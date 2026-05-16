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
import { useState } from 'react';
import { useT } from '../lib/I18nContext';
import { projects as projectsApi } from '../lib/api';
import { Save, X, Loader2 } from 'lucide-react';

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
  agent: string;
  /** Toast emitter — same shape as the rest of the dashboard. The
   * required (non-optional) kind matches Dashboard's stricter signature. */
  toast: (msg: string, kind: 'success' | 'error' | 'info' | 'warning') => void;
}

export function BriefingForm({ projectId, onClose, onSaved, agent, toast }: BriefingFormProps) {
  const { t } = useT();
  const [form, setForm] = useState({
    purpose: '',
    team: '',
    maturity: '',
    dependencies: '',
    traps: '',
    additional: '',
  });
  const [submitting, setSubmitting] = useState(false);

  const update = (key: keyof typeof form, value: string) => setForm(f => ({ ...f, [key]: value }));

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
    setSubmitting(true);
    try {
      // 1. Save the answers — writes `docs/briefing.md` + DB notes.
      await projectsApi.saveBriefing(projectId, form);
      // 2. Spawn the AI review discussion. The backend now sees the
      //    pre-filled briefing notes and switches to a short
      //    "review + clarify" prompt instead of re-asking the 6 Qs.
      //    If this fails (e.g. no audit-capable agent), we still
      //    consider the save a success — the answers are persisted,
      //    the audit can still pick them up.
      let discId: string | null = null;
      try {
        const { discussion_id } = await projectsApi.startBriefing(projectId, agent);
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
    <form onSubmit={handleSubmit} className="briefing-form">
      <div className="briefing-form-header">
        <h3>{t('briefing.formTitle')}</h3>
        <button type="button" className="briefing-form-close" onClick={onClose} aria-label={t('common.close')}>
          <X size={14} />
        </button>
      </div>
      <p className="briefing-form-intro">{t('briefing.formIntro')}</p>

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

      <div className="briefing-form-actions">
        <button type="button" onClick={onClose} disabled={submitting}>{t('common.cancel')}</button>
        <button type="submit" className="briefing-form-submit" disabled={submitting}>
          {submitting
            ? <><Loader2 size={11} style={{ animation: 'spin 1s linear infinite' }} /> {t('common.saving')}</>
            : <><Save size={11} /> {t('briefing.saveAndReviewBtn')}</>}
        </button>
      </div>
    </form>
  );
}
