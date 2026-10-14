import { saveTourProgress } from '../components/tour/tourProgress';
import { TOUR_STEPS } from '../components/tour/tourSteps';

/**
 * A reader who has already taken the onboarding tour. Without this, the
 * dashboard starts the tour on its own shortly after mount and walks the
 * router to the Projects page in the middle of a test.
 */
export function tourAlreadyTaken(): void {
  const stepIds = TOUR_STEPS.map(step => step.id);
  saveTourProgress(stepIds, stepIds, null, true);
}
