import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';

// Only `mentor.completeChapter` is exercised; stub the rest of the api surface
// MentorPage imports so the module resolves.
vi.mock('../../lib/api', () => ({
  mentor: { completeChapter: vi.fn().mockResolvedValue({}) },
  config: { getUiLanguage: () => Promise.resolve('fr'), saveUiLanguage: () => Promise.resolve() },
  projects: {},
}));

import { mentor as mentorApi } from '../../lib/api';
import { ChapterCard, ChaptersView } from '../MentorPage';
import { recommendedNextTopic, curriculumProgress, onboardingTopicStatus } from '../../lib/onboarding-progress';
import type { Chapter, Checkpoint, MentorState, OnboardingTopic, ParcoursSummary } from '../../types/generated';

const quiz = (question: string, options: string[], answer: number, explanations: string[]): Checkpoint =>
  ({ question, options, answer, explanations, reveal: null });

const chap = (over: Partial<Chapter> = {}): Chapter =>
  ({ title: 'C', explanation: 'x', checkpoint: null, checkpoints: [], done: false, learner_answer: null, needs_review: false, ...over });

const renderC = (ui: React.ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);
const doneBtn = () => screen.getByRole('button', { name: /Marquer|Mark as done|Marcar/i });

afterEach(() => { cleanup(); vi.clearAllMocks(); });

describe('onboarding ChapterCard — #2 multi-checkpoint', () => {
  it('numbers multiple checkpoints and gates completion until every quiz is correct', () => {
    const chapter = chap({ checkpoints: [
      quiz('Première question ?', ['alpha', 'beta'], 0, ['oui', 'non']),
      quiz('Deuxième question ?', ['gamma', 'delta'], 1, ['non', 'oui']),
    ] });
    renderC(<ChapterCard index={0} chapter={chapter} unlocked discId="d1" onUpdate={() => {}} />);

    // Numbered headers (FR & EN both render "Question N").
    expect(screen.getByText('Question 1')).toBeTruthy();
    expect(screen.getByText('Question 2')).toBeTruthy();
    // Options render.
    expect(screen.getByText('alpha')).toBeTruthy();
    expect(screen.getByText('delta')).toBeTruthy();

    // Not engaged yet → done button disabled.
    expect(doneBtn()).toBeDisabled();
    // Answer only the first correctly → still disabled (second unanswered).
    fireEvent.click(screen.getByText('alpha'));
    expect(doneBtn()).toBeDisabled();
    // Answer the second correctly → now enabled.
    fireEvent.click(screen.getByText('delta'));
    expect(doneBtn()).not.toBeDisabled();
  });

  it('flags needs_review when a quiz needed a wrong attempt before the right one', async () => {
    const chapter = chap({ checkpoints: [quiz('Q ?', ['bon', 'mauvais'], 0, ['juste', 'faux'])] });
    renderC(<ChapterCard index={3} chapter={chapter} unlocked discId="d1" onUpdate={() => {}} />);

    fireEvent.click(screen.getByText('mauvais')); // wrong first → struggled
    fireEvent.click(screen.getByText('bon'));     // then correct → can complete
    fireEvent.click(doneBtn());

    await waitFor(() => expect(mentorApi.completeChapter).toHaveBeenCalled());
    // (discId, index, exerciseAnswer=undefined, needsReview=true)
    expect(mentorApi.completeChapter).toHaveBeenCalledWith('d1', 3, undefined, true);
  });

  it('a clean first-try pass does NOT flag needs_review', async () => {
    const chapter = chap({ checkpoints: [quiz('Q ?', ['bon', 'mauvais'], 0, ['juste', 'faux'])] });
    renderC(<ChapterCard index={1} chapter={chapter} unlocked discId="d1" onUpdate={() => {}} />);
    fireEvent.click(screen.getByText('bon'));
    fireEvent.click(doneBtn());
    await waitFor(() => expect(mentorApi.completeChapter).toHaveBeenCalledWith('d1', 1, undefined, false));
  });
});

describe('onboarding catalogue — #1/#2 start-here + progress', () => {
  const topic = (id: string, kind: string): OnboardingTopic =>
    ({ title: id, topic_id: id, kind, level: null, scope: null, prerequisites: null, references: [], description: null, course_path: null } as unknown as OnboardingTopic);
  const par = (over: Partial<ParcoursSummary>): ParcoursSummary =>
    ({ disc_id: 'd', title: 't', mode: 'onboarding', status: 'open', objective: '', source: {} as never, progress_done: 0, progress_total: 0, updated_at: '', ...over } as unknown as ParcoursSummary);
  const map = (pairs: [string, ParcoursSummary][]) => new Map(pairs);

  it('recommends the first tronc topic to start, in curriculum order', () => {
    const catalog = [topic('branch1', 'branche'), topic('tronc1', 'tronc'), topic('cap', 'capstone')];
    const next = recommendedNextTopic(catalog, map([]));
    expect(next?.topic_id).toBe('tronc1'); // tronc wins regardless of list order
  });

  it('prefers resuming an in-progress topic over starting a new one', () => {
    const catalog = [topic('tronc1', 'tronc'), topic('branch1', 'branche')];
    const byTopic = map([['branch1', par({ progress_done: 1, progress_total: 3 })]]); // in progress
    expect(recommendedNextTopic(catalog, byTopic)?.topic_id).toBe('branch1');
  });

  it('returns null when every topic is done', () => {
    const catalog = [topic('tronc1', 'tronc')];
    const byTopic = map([['tronc1', par({ progress_done: 4, progress_total: 4 })]]);
    expect(recommendedNextTopic(catalog, byTopic)).toBeNull();
  });

  it('counts curriculum + tronc progress', () => {
    const catalog = [topic('t1', 'tronc'), topic('t2', 'tronc'), topic('b1', 'branche')];
    const byTopic = map([['t1', par({ progress_done: 2, progress_total: 2 })]]);
    const p = curriculumProgress(catalog, byTopic);
    expect(p).toMatchObject({ done: 1, total: 3, troncDone: 1, troncTotal: 2 });
  });

  it('classifies topic status (failed wins over generating)', () => {
    expect(onboardingTopicStatus(undefined)).toBe('todo');
    expect(onboardingTopicStatus(par({ status: 'generating', generation_error: 'boom' } as never))).toBe('failed');
    expect(onboardingTopicStatus(par({ status: 'generating' }))).toBe('generating');
    expect(onboardingTopicStatus(par({ progress_done: 3, progress_total: 3 }))).toBe('done');
    expect(onboardingTopicStatus(par({ progress_done: 1, progress_total: 3 }))).toBe('in_progress');
  });
});

describe('onboarding ChaptersView — #4b review section', () => {
  it('surfaces a targeted-review section for struggled chapters once the course is done', () => {
    const parcours = {
      chapters: [
        chap({ title: 'Ch1', done: true, needs_review: true, checkpoints: [quiz('Q ?', ['a', 'b'], 0, ['', ''])] }),
        chap({ title: 'Ch2', done: true, needs_review: false }),
      ],
    } as unknown as MentorState;
    renderC(<ChaptersView parcours={parcours} discId="d1" onUpdate={() => {}} />);
    // Review title mentions the count (1 flagged chapter). Locale-tolerant.
    expect(screen.getByText(/Révision ciblée \(1\)|Targeted review \(1\)|Repaso específico \(1\)/)).toBeTruthy();
  });

  it('shows no review section when nothing was struggled', () => {
    const parcours = {
      chapters: [chap({ title: 'Ch1', done: true, needs_review: false })],
    } as unknown as MentorState;
    renderC(<ChaptersView parcours={parcours} discId="d1" onUpdate={() => {}} />);
    expect(screen.queryByText(/Révision ciblée|Targeted review|Repaso específico/)).toBeNull();
  });
});
