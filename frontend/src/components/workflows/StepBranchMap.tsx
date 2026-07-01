// #15 — a compact SVG "branch map" for a workflow's steps. A horizontal chip
// row hides the back/forward jumps a Goto creates; this draws the step spine on
// the left and each Goto as an arc in the gutter (loops highlighted), so a
// user sees at a glance where control can jump. Not an editor — a read-only map.
import { computeGotoEdges } from '../../lib/stepGraph';
import type { WorkflowStep } from '../../types/generated';

interface StepBranchMapProps {
  steps: WorkflowStep[];
  t: (key: string, ...args: (string | number)[]) => string;
}

const ROW_H = 24;
const GUTTER = 52;   // left space reserved for Goto arcs
const WIDTH = 300;
const PAD_TOP = 6;

export function StepBranchMap({ steps, t }: StepBranchMapProps) {
  const edges = computeGotoEdges(steps);
  if (edges.length === 0) return null; // linear workflow → nothing to draw

  const cy = (i: number) => i * ROW_H + ROW_H / 2 + PAD_TOP;
  const height = steps.length * ROW_H + PAD_TOP * 2;
  const spineX = GUTTER;

  return (
    <div className="wf-branch-map-wrap" data-testid="wf-branch-map">
      <div className="wf-branch-map-title">{t('wf.branchMap.title')}</div>
      <svg
        className="wf-branch-map"
        width={WIDTH}
        height={height}
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
          <g key={i}>
            <circle cx={spineX} cy={cy(i)} r={3.5} className="wf-bm-node" />
            <text x={spineX + 10} y={cy(i) + 3} className="wf-bm-node-label">
              {i + 1}. {s.name.length > 26 ? `${s.name.slice(0, 25)}…` : s.name}
            </text>
          </g>
        ))}
        {/* Goto arcs in the left gutter */}
        {edges.map((e, k) => {
          if (e.toIndex < 0) return null; // dangling target — skip drawing
          const depth = Math.min(Math.abs(e.toIndex - e.fromIndex), 3);
          const bx = spineX - 6 - depth * 12;
          const d = `M ${spineX} ${cy(e.fromIndex)} C ${bx} ${cy(e.fromIndex)}, ${bx} ${cy(e.toIndex)}, ${spineX} ${cy(e.toIndex)}`;
          return (
            <path
              key={k}
              d={d}
              className={e.backward ? 'wf-bm-arc wf-bm-arc-back' : 'wf-bm-arc'}
              markerEnd="url(#wf-bm-head)"
              data-testid="wf-bm-arc"
            >
              <title>{`${e.fromName} → ${e.toName}${e.label ? ` (${t('wf.branchMap.onTrigger', e.label)})` : ''}`}</title>
            </path>
          );
        })}
      </svg>
      <div className="wf-branch-map-legend">
        <span className="wf-bm-legend-fwd">— {t('wf.branchMap.legendForward')}</span>
        <span className="wf-bm-legend-back">— {t('wf.branchMap.legendLoop')}</span>
      </div>
    </div>
  );
}
