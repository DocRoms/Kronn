import { HelpCircle } from 'lucide-react';
import { useTour } from './TourProvider';
import { useT } from '../../lib/I18nContext';

export function TourHelpButton() {
  const { isActive, start } = useTour();
  const { t } = useT();

  if (isActive) return null;

  return (
    <button
      className="dash-nav-btn"
      onClick={() => start(true)}
      title={t('tour.replayHint')}
      aria-label={t('tour.replayHint')}
      style={{ opacity: 0.6 }}
    >
      <HelpCircle size={16} />
    </button>
  );
}
