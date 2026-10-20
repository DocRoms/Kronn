import { memo } from 'react';
import { ChevronRight } from 'lucide-react';

interface OrchestrationFoldMarkerProps {
  runId: string;
  count: number;
  expanded: boolean;
  onToggle: (runId: string) => void;
  t: (key: string, ...args: (string | number)[]) => string;
}

/** KT-1330 — one compact, expandable stand-in for a run of orchestration
 *  messages, so hiding them never silently loses anything. */
export const OrchestrationFoldMarker = memo(function OrchestrationFoldMarker({
  runId,
  count,
  expanded,
  onToggle,
  t,
}: OrchestrationFoldMarkerProps) {
  return (
    <button
      type="button"
      className="disc-orchestration-marker"
      data-testid="orchestration-fold-marker"
      data-expanded={expanded}
      aria-expanded={expanded}
      onClick={() => onToggle(runId)}
    >
      <ChevronRight
        size={10}
        style={{ transition: 'transform 120ms', transform: expanded ? 'rotate(90deg)' : 'rotate(0deg)', flexShrink: 0 }}
        aria-hidden="true"
      />
      <span>{expanded ? t('disc.orchestration.collapse', count) : t('disc.orchestration.marker', count)}</span>
    </button>
  );
});
