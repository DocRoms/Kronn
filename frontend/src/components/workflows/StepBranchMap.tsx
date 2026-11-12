// #15 — a compact SVG map for a workflow's steps. It always draws the linear
// spine so every workflow keeps the same detail layout, then overlays Goto arcs
// in the gutter when branches exist. Not an editor — a read-only navigator.
import { AGENT_LABELS } from '../../lib/constants';
import { computeGotoEdges } from '../../lib/stepGraph';
import { activeMergedIds } from '../../lib/shadowMerge';
import type { WorkflowStep } from '../../types/generated';
import './StepShadowRows.css';

interface StepBranchMapProps {
  steps: WorkflowStep[];
  t: (key: string, ...args: (string | number)[]) => string;
  selectedStepIndex?: number;
  onSelectStep?: (index: number) => void;
}

const ROW_H = 24;
const GUTTER = 52;   // left space reserved for Goto arcs
const WIDTH = 300;
const PAD_TOP = 6;
const LINE_H = 14;   // one shadow line, or the merged-result line, under a step

const clipName = (name: string, max: number) => (name.length > max ? `${name.slice(0, max - 1)}…` : name);

/** KT-1696 — a step's shadows and whether they feed a merged result: the lines drawn under it. */
function shadowLines(step: WorkflowStep) {
  const shadows = step.shadows ?? [];
  const merged = activeMergedIds(step.shadow_merge, shadows).length > 0;
  return { shadows, merged, count: shadows.length + (merged ? 1 : 0) };
}

export function StepBranchMap({
  steps,
  t,
  selectedStepIndex,
  onSelectStep,
}: StepBranchMapProps) {
  const edges = computeGotoEdges(steps);
  if (steps.length === 0) return null;

  // A doubled step is taller: one line per shadow, then the merged result.
  const lines = steps.map(shadowLines);
  const tops = steps.map((_, i) => PAD_TOP + lines.slice(0, i).reduce((sum, line) => sum + ROW_H + line.count * LINE_H, 0));
  const cy = (i: number) => tops[i] + ROW_H / 2;
  const height = PAD_TOP * 2 + lines.reduce((sum, line) => sum + ROW_H + line.count * LINE_H, 0);
  const doubledSteps = lines.filter(line => line.shadows.length > 0).length;
  const spineX = GUTTER;

  return (
    <div className="wf-branch-map-wrap" data-testid="wf-branch-map">
      <div className="wf-branch-map-title">{t('wf.branchMap.title')}</div>
      <svg
        className="wf-branch-map"
        width="100%"
        height={height}
        viewBox={`0 0 ${WIDTH} ${height}`}
        preserveAspectRatio="xMinYMin meet"
        role="img"
        aria-label={t('wf.branchMap.title')}
      >
        <defs>
          <marker id="wf-bm-head" markerWidth="6" markerHeight="6" refX="4" refY="3" orient="auto">
            <path d="M0,0 L6,3 L0,6 Z" className="wf-bm-head" />
          </marker>
        </defs>
        {/* linear spine */}
        <line x1={spineX} y1={cy(0)} x2={spineX} y2={cy(steps.length - 1)} className="wf-bm-spine" />
        {/* nodes + labels */}
        {steps.map((s, i) => (
          <g
            key={i}
            className="wf-bm-node-g"
            data-selected={selectedStepIndex === i}
            data-testid={`wf-bm-node-${i}`}
            role={onSelectStep ? 'button' : undefined}
            tabIndex={onSelectStep ? 0 : undefined}
            onClick={() => onSelectStep?.(i)}
            onKeyDown={event => {
              if (!onSelectStep || (event.key !== 'Enter' && event.key !== ' ')) return;
              event.preventDefault();
              onSelectStep(i);
            }}
          >
            {onSelectStep && (
              <rect
                x={spineX - 7}
                y={cy(i) - 10}
                width={WIDTH - spineX + 2}
                height={20 + lines[i].count * LINE_H}
                rx={5}
                className="wf-bm-node-hit"
              />
            )}
            <circle cx={spineX} cy={cy(i)} r={3.5} className="wf-bm-node" />
            <text x={spineX + 10} y={cy(i) + 3} className="wf-bm-node-label">
              {i + 1}. {clipName(s.name, lines[i].shadows.length > 0 ? 20 : 26)}
              {lines[i].shadows.length > 0 && <tspan className="wf-bm-tag" dx={6} data-testid={`wf-bm-shadow-tag-${i}`}>⑂ {lines[i].shadows.length}</tspan>}
              {lines[i].merged && <tspan className="wf-bm-tag wf-bm-tag-merge" dx={4} data-testid={`wf-bm-merge-tag-${i}`}>⊕</tspan>}
            </text>
            {lines[i].shadows.map((shadow, k) => (
              <text
                key={shadow.id}
                x={spineX + 24}
                y={cy(i) + 3 + (k + 1) * LINE_H}
                className="wf-bm-shadow"
                data-paused={shadow.enabled === false}
                data-testid={`wf-bm-shadow-${i}-${shadow.id}`}
              >
                ⑂ {clipName(shadow.label || shadow.id, 14)} · {AGENT_LABELS[shadow.agent?.agent ?? s.agent] ?? shadow.agent?.agent ?? s.agent}
              </text>
            ))}
            {lines[i].merged && (
              <text
                x={spineX + 24}
                y={cy(i) + 3 + (lines[i].shadows.length + 1) * LINE_H}
                className="wf-bm-shadow wf-bm-merged"
                data-testid={`wf-bm-merged-${i}`}
              >
                {steps[i + 1]
                  ? t('wf.branchMap.mergedTo', clipName(steps[i + 1].name, 22))
                  : t('wf.branchMap.mergedLast')}
              </text>
            )}
          </g>
        ))}
        {/* Goto arcs in the left gutter. Each drawable arc gets: a categorical
            hue (fixed order, index 0..4 then a neutral — CVD-validated per theme
            in CSS), a per-index geometric offset so overlapping arcs fan out
            instead of coinciding, and a dash for backward loops (redundant with
            colour → A11y). Hovering the map dims all arcs; hovering one restores
            it — so a crossing line is easy to follow. */}
        {edges.filter(e => e.toIndex >= 0).map((e, k) => {
          const depth = Math.min(Math.abs(e.toIndex - e.fromIndex), 3);
          // Fan out crossings. A negative control point is fine: the cubic's
          // actual min-x is (2·spineX + 6·bx)/8, still > 0 at the deepest fan
          // slot — clamping here would collapse deep arcs onto each other.
          const bx = spineX - 6 - depth * 12 - (k % 4) * 6;
          const d = `M ${spineX} ${cy(e.fromIndex)} C ${bx} ${cy(e.fromIndex)}, ${bx} ${cy(e.toIndex)}, ${spineX} ${cy(e.toIndex)}`;
          const slot = k < 5 ? String(k) : 'x'; // fixed-order slot 0..4, then neutral (never cycled)
          const cls = `wf-bm-arc wf-bm-arc-c${slot}${e.backward ? ' wf-bm-arc-back' : ''}`;
          return (
            <g key={k} className="wf-bm-arc-g">
              {/* Fat invisible twin: the visible stroke is 1.6px — unhoverable.
                  This one carries the pointer events (and the tooltip). */}
              <path d={d} className="wf-bm-arc-hit" data-testid="wf-bm-arc-hit">
                <title>{`${e.fromName} → ${e.toName}${e.label ? ` (${t('wf.branchMap.onTrigger', e.label)})` : ''}`}</title>
              </path>
              <path
                d={d}
                className={cls}
                markerEnd="url(#wf-bm-head)"
                data-testid="wf-bm-arc"
              />
            </g>
          );
        })}
      </svg>
      <div className="wf-branch-map-legend">
        {edges.length > 0 ? (
          <>
            <span className="wf-bm-legend-fwd">🎨 {t('wf.branchMap.legendForward')}</span>
            <span className="wf-bm-legend-back">⇠ {t('wf.branchMap.legendLoop')}</span>
          </>
        ) : (
          <span className="wf-bm-legend-linear" data-testid="wf-bm-legend-linear">
            │ {doubledSteps > 0
              ? t(doubledSteps === 1 ? 'wf.branchMap.linearDoubledOne' : 'wf.branchMap.linearDoubledMany', doubledSteps)
              : t('wf.branchMap.linear')}
          </span>
        )}
      </div>
    </div>
  );
}
