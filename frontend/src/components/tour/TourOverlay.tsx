import { createPortal } from 'react-dom';
import { ChevronRight, ChevronLeft, MousePointerClick } from 'lucide-react';
import { useTour } from './TourProvider';
import { useTourPositioning } from './useTourPositioning';
import { useIsMobile } from '../../hooks/useMediaQuery';
import { useT } from '../../lib/I18nContext';
import './Tour.css';

export function TourOverlay() {
  const { isActive, currentStep, stepIndex, totalSteps, next, prev, skip, waitingForClick } = useTour();
  const { t } = useT();
  const isMobile = useIsMobile();

  const { spotlight, tooltip, position } = useTourPositioning(
    currentStep?.selector ?? null,
    currentStep?.position,
    isMobile,
    currentStep?.pulse ?? false,
    currentStep?.tooltipAnchor,
  );

  if (!isActive || !currentStep) return null;

  const isFirst = stepIndex === 0;
  const isLast = stepIndex === totalSteps - 1;
  const isCentered = !currentStep.selector;

  return createPortal(
    <>
      {/* Dark area backdrop. The earlier behaviour was `onClick={skip}` —
          which marked the tour permanently completed if a user clicked
          off the tooltip by accident. UX feedback: a tooltip overlay
          should not be dismissed by a stray click on the dim area. The
          explicit "Passer" / "Finir" buttons + the Escape shortcut cover
          intentional dismissals. */}
      <div
        className="tour-backdrop"
        data-dimmed={isCentered}
        data-passthrough={waitingForClick}
      />

      {/* Spotlight hole (only for steps with a target) */}
      {spotlight && (
        <div className="tour-spotlight" style={spotlight} />
      )}

      {/* Pulse ring — separate portal div so it's above all stacking contexts */}
      {spotlight && waitingForClick && (
        <div className="tour-pulse-ring" style={spotlight} />
      )}

      {/* Tooltip card */}
      <div
        className="tour-tooltip"
        style={isMobile && !isCentered ? {} : tooltip}
        data-position={position}
        data-mobile={isMobile}
        data-centered={isCentered}
        role="dialog"
        aria-modal="true"
        aria-label={t(currentStep.titleKey)}
      >
        {/* Group label (act name) */}
        {currentStep.group && (
          <div className="tour-group-label">{currentStep.group}</div>
        )}

        {/* Step counter */}
        <div className="tour-step-counter">
          {stepIndex + 1} / {totalSteps}
        </div>

        <h3 className="tour-title">{t(currentStep.titleKey)}</h3>
        <p className="tour-desc">{t(currentStep.descKey)}</p>

        {/* Click hint for interactive steps */}
        {waitingForClick && (
          <div className="tour-click-hint">
            <MousePointerClick size={11} style={{ verticalAlign: 'middle', marginRight: 4 }} />
            {t('tour.clickHint')}
          </div>
        )}

        {/* Navigation. Prev/Next stay visible during `waitingForClick` so
            the user always has a way out — pre-fix they were hidden,
            which forced operators to either click the spotlighted target
            or skip the whole tour. The provider's `next`/`prev` now
            cancel the pending click listener instead of bailing out. */}
        <div className="tour-nav">
          <button className="tour-btn-skip" onClick={skip}>
            {t('tour.skip')}
          </button>

          {!isFirst && (
            <button className="tour-btn-prev" onClick={prev}>
              <ChevronLeft size={12} /> {t('tour.prev')}
            </button>
          )}

          <button className="tour-btn-next" onClick={isLast ? skip : next}>
            {isLast ? t('tour.finish') : t('tour.next')}
            {!isLast && <ChevronRight size={12} />}
          </button>
        </div>
      </div>
    </>,
    document.body,
  );
}
