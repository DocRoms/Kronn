// Pure helpers for the onboarding catalogue's "start here" CTA (#1) and
// cursus progress / next-step (#2). Kept out of MentorPage.tsx so they stay
// unit-testable and don't trip react-refresh (a component file should only
// export components).
import type { OnboardingTopic, ParcoursSummary } from '../types/generated';

/** Curriculum order for onboarding topics; unknown kinds sort last. */
const CURRICULUM_ORDER = ['tronc', 'branche', 'capstone', 'culture'] as const;
const curriculumRank = (kind: string | null | undefined) => {
  const i = (CURRICULUM_ORDER as readonly string[]).indexOf(kind ?? '');
  return i === -1 ? CURRICULUM_ORDER.length : i;
};

export type OnboardingTopicStatus = 'todo' | 'generating' | 'in_progress' | 'done' | 'failed';

/** The registry `Niveau` is a free string (curated by a human, so spelled any
 *  which way, FR or EN). We fold it into one of three tiers so the catalogue can
 *  colour it consistently (vert / orange / rouge); an unrecognised value → null
 *  (rendered neutral). Accents are stripped so "avancé"/"avance" both match. */
export type LevelTier = 'beginner' | 'intermediate' | 'advanced';
export function levelTier(level: string | null | undefined): LevelTier | null {
  if (!level) return null;
  const l = level.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();
  // Intermediate first: its keywords are disjoint from the others, and testing it
  // up front avoids any ambiguity with a compound label like "débutant→intermédiaire".
  if (/(intermediaire|intermediate|moyen|medium)/.test(l)) return 'intermediate';
  if (/(avance|advanced|expert|confirme|senior|difficile|hard)/.test(l)) return 'advanced';
  if (/(debutant|beginner|basique|basic|facile|easy|novice|junior|intro)/.test(l)) return 'beginner';
  return null;
}

/** Status of a catalogue topic from its (optional) existing parcours — mirrors
 *  the per-topic pill logic in the catalogue. */
export function onboardingTopicStatus(ex: ParcoursSummary | undefined): OnboardingTopicStatus {
  if (!ex) return 'todo';
  if (ex.generation_error) return 'failed';
  if (ex.status === 'generating') return 'generating';
  if (ex.progress_total > 0 && ex.progress_done >= ex.progress_total) return 'done';
  return 'in_progress';
}

/** The topic to steer a learner to next: resume an in-progress one (in
 *  curriculum order), else start the first not-yet-done one. `null` once the
 *  whole cursus is done. */
export function recommendedNextTopic(
  catalog: OnboardingTopic[],
  byTopic: Map<string, ParcoursSummary>,
): OnboardingTopic | null {
  const ordered = [...catalog].sort((a, b) => curriculumRank(a.kind) - curriculumRank(b.kind));
  const st = (t: OnboardingTopic) => onboardingTopicStatus(byTopic.get(t.topic_id));
  return ordered.find((t) => st(t) === 'in_progress')
    ?? ordered.find((t) => st(t) === 'todo')
    ?? null;
}

/** Catalogue-level progress for the onboarding header. */
export function curriculumProgress(
  catalog: OnboardingTopic[],
  byTopic: Map<string, ParcoursSummary>,
): { done: number; total: number; troncDone: number; troncTotal: number } {
  const st = (t: OnboardingTopic) => onboardingTopicStatus(byTopic.get(t.topic_id));
  const isTronc = (t: OnboardingTopic) => t.kind === 'tronc';
  return {
    done: catalog.filter((t) => st(t) === 'done').length,
    total: catalog.length,
    troncDone: catalog.filter((t) => isTronc(t) && st(t) === 'done').length,
    troncTotal: catalog.filter(isTronc).length,
  };
}
