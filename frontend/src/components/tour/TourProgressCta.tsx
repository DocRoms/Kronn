import { ArrowRight, Compass } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import { useT } from '../../lib/I18nContext';
import { useTour } from './TourProvider';
import { TOUR_STEPS } from './tourSteps';
import {
  loadTourProgress,
  TOUR_PROGRESS_EVENT,
  type TourProgress,
} from './tourProgress';

const STEP_IDS = TOUR_STEPS.map(step => step.id);

export function TourProgressCta() {
  const { t } = useT();
  const { isActive, start } = useTour();
  const readProgress = useCallback(() => loadTourProgress(STEP_IDS), []);
  const [progress, setProgress] = useState<TourProgress>(readProgress);

  useEffect(() => {
    const refresh = () => setProgress(readProgress());
    window.addEventListener(TOUR_PROGRESS_EVENT, refresh);
    window.addEventListener('storage', refresh);
    return () => {
      window.removeEventListener(TOUR_PROGRESS_EVENT, refresh);
      window.removeEventListener('storage', refresh);
    };
  }, [readProgress]);

  if (isActive || progress.isComplete) return null;

  const percent = progress.totalSteps === 0
    ? 0
    : Math.round((progress.completedCount / progress.totalSteps) * 100);

  return (
    <button
      type="button"
      className="set-tour-progress"
      data-testid="settings-tour-progress"
      onClick={() => start(false)}
      aria-label={t(progress.completedCount > 0 ? 'tour.progress.resume' : 'tour.progress.start')}
    >
      <span className="set-tour-progress-heading">
        <span className="set-tour-progress-icon" aria-hidden="true">
          <Compass size={14} />
        </span>
        <span className="set-tour-progress-title">{t('tour.progress.title')}</span>
        <span className="set-tour-progress-count">
          {progress.completedCount}/{progress.totalSteps}
        </span>
      </span>
      <span className="set-tour-progress-copy">
        {t(progress.completedCount > 0 ? 'tour.progress.resume' : 'tour.progress.start')}
      </span>
      <span
        className="set-tour-progress-bar"
        role="progressbar"
        aria-label={t('tour.progress.title')}
        aria-valuemin={0}
        aria-valuemax={progress.totalSteps}
        aria-valuenow={progress.completedCount}
      >
        <span style={{ width: `${percent}%` }} />
      </span>
      <span className="set-tour-progress-action" aria-hidden="true">
        {t(progress.completedCount > 0 ? 'tour.progress.continue' : 'tour.progress.begin')}
        <ArrowRight size={12} />
      </span>
    </button>
  );
}
