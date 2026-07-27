import { createPortal } from 'react-dom';
import { useEffect, useRef } from 'react';
import { ChevronRight, ChevronLeft, MousePointerClick } from 'lucide-react';
import { useTour } from './TourProvider';
import { useTourPositioning } from './useTourPositioning';
import { useIsMobile } from '../../hooks/useMediaQuery';
import { useT } from '../../lib/I18nContext';
import './Tour.css';

export function TourOverlay() {
  const {
    isActive,
    currentStep,
    stepIndex,
    totalSteps,
    next,
    prev,
    skip,
    finish,
    waitingForClick,
  } = useTour();
  const { t } = useT();
  const isMobile = useIsMobile();
  const tooltipRef = useRef<HTMLDivElement | null>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);
  const currentStepId = currentStep?.id;

  const { spotlight, secondarySpotlights, tooltip, position } = useTourPositioning(
    currentStep?.selector ?? null,
    currentStep?.position,
    isMobile,
    currentStep?.pulse ?? false,
    currentStep?.tooltipAnchor,
    currentStep?.id,
    currentStep?.secondarySelectors,
  );

  useEffect(() => {
    if (!isActive) return;
    previousFocusRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    return () => {
      const previous = previousFocusRef.current;
      previousFocusRef.current = null;
      if (previous?.isConnected) previous.focus();
    };
  }, [isActive]);

  useEffect(() => {
    if (!isActive || !currentStepId) return;
    const frame = requestAnimationFrame(() => tooltipRef.current?.focus());
    return () => cancelAnimationFrame(frame);
  }, [isActive, currentStepId]);

  useEffect(() => {
    if (!isActive || waitingForClick) return;
    const handleTab = (event: KeyboardEvent) => {
      if (event.key !== 'Tab' || !tooltipRef.current) return;
      const focusable = [...tooltipRef.current.querySelectorAll<HTMLElement>(
        'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
      )].filter(element => element.offsetParent !== null);
      if (focusable.length === 0) {
        event.preventDefault();
        tooltipRef.current.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      } else if (!tooltipRef.current.contains(document.activeElement)) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener('keydown', handleTab);
    return () => document.removeEventListener('keydown', handleTab);
  }, [isActive, waitingForClick, currentStepId]);

  if (!isActive || !currentStep) return null;

  const isFirst = stepIndex === 0;
  const isLast = stepIndex === totalSteps - 1;
  const isCentered = !currentStep.selector;
  const titleId = `tour-title-${currentStep.id}`;
  const descId = `tour-desc-${currentStep.id}`;
  const hasMultipleSpotlights = Boolean(spotlight && secondarySpotlights.length > 0);
  const spotlightMaskId = `tour-spotlight-mask-${currentStep.id}`;
  const spotlightRects = spotlight
    ? [spotlight, ...secondarySpotlights]
    : [];
  const geometry = (value: React.CSSProperties[keyof React.CSSProperties]) => (
    typeof value === 'number' ? value : Number.parseFloat(String(value ?? 0))
  );

  return createPortal(
    <>
      {/* Dark area backdrop. The earlier behaviour was `onClick={skip}` —
          which marked the tour permanently completed if a user clicked
          off the tooltip by accident. UX feedback: a tooltip overlay
          should not be dismissed by a stray click on the dim area. The
          explicit "Passer" / "Finir" buttons + the Escape shortcut cover
          intentional dismissals. */}
      {/* Dim whenever there is no spotlight, not only on the steps that
          declare no target. The darkness comes from the spotlight's box-shadow,
          so a step whose selector no longer matches anything (an outdated step)
          used to leave this backdrop transparent while it still swallowed every
          click: the app looked untouched but ignored the mouse, and reloading
          was the only way out. */}
      <div
        className="tour-backdrop"
        data-dimmed={!spotlight}
        data-passthrough={waitingForClick}
      />

      {/* A box-shadow can cut only one hole. Steps that teach two related
          controls (discussion + message IDs) use an SVG mask so BOTH targets
          receive the same clear opening in the overlay. */}
      {hasMultipleSpotlights && (
        <svg className="tour-multi-spotlight-mask" aria-hidden="true">
          <defs>
            <mask id={spotlightMaskId} maskUnits="userSpaceOnUse">
              <rect width="100%" height="100%" fill="white" />
              {spotlightRects.map((rect, index) => (
                <rect
                  key={`${currentStep.id}-mask-hole-${index}`}
                  data-tour-mask-hole
                  x={geometry(rect.left)}
                  y={geometry(rect.top)}
                  width={geometry(rect.width)}
                  height={geometry(rect.height)}
                  rx="10"
                  fill="black"
                />
              ))}
            </mask>
          </defs>
          <rect
            className="tour-multi-spotlight-dim"
            width="100%"
            height="100%"
            mask={`url(#${spotlightMaskId})`}
          />
        </svg>
      )}

      {/* Spotlight hole (only for steps with a target) */}
      {spotlight && (
        <div
          className="tour-spotlight"
          style={spotlight}
          data-multiple={hasMultipleSpotlights}
        />
      )}
      {secondarySpotlights.map((secondarySpotlight, index) => (
        <div
          key={`${currentStep.id}-secondary-${index}`}
          className="tour-secondary-spotlight"
          style={secondarySpotlight}
        />
      ))}

      {/* Pulse ring — separate portal div so it's above all stacking contexts */}
      {spotlight && waitingForClick && (
        <div className="tour-pulse-ring" style={spotlight} />
      )}

      {/* Tooltip card */}
      <div
        ref={tooltipRef}
        className="tour-tooltip"
        style={isMobile && !isCentered ? {} : tooltip}
        data-tour-step={currentStep.id}
        data-position={position}
        data-mobile={isMobile}
        data-centered={isCentered}
        role="dialog"
        aria-modal={waitingForClick ? undefined : 'true'}
        aria-labelledby={titleId}
        aria-describedby={descId}
        tabIndex={-1}
      >
        {/* Group label (act name) */}
        {currentStep.groupKey && (
          <div className="tour-group-label">{t(currentStep.groupKey)}</div>
        )}

        {/* Step counter */}
        <div className="tour-step-counter">
          {stepIndex + 1} / {totalSteps}
        </div>

        <h3 id={titleId} className="tour-title" aria-live="polite">
          {t(currentStep.titleKey)}
        </h3>
        <p id={descId} className="tour-desc">{t(currentStep.descKey)}</p>
        {currentStep.agentNoteKey && (
          <p className="tour-agent-note">{t(currentStep.agentNoteKey)}</p>
        )}
        {currentStep.infoNoteKey && (
          <p className="tour-info-note">{t(currentStep.infoNoteKey)}</p>
        )}

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

          <button className="tour-btn-next" onClick={isLast ? finish : next}>
            {isLast ? t('tour.finish') : t('tour.next')}
            {!isLast && <ChevronRight size={12} />}
          </button>
        </div>
      </div>
    </>,
    document.body,
  );
}
