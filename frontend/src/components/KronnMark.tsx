// The Kronn mark, drawn rather than loaded.
//
// The lightning bolt that stood next to the app name was a lucide icon, not a
// logo: it said "fast", it did not say "Kronn". This is the same geometry as
// `public/favicon.svg` and the desktop icon, minus their dark plate — the
// interface behind it is already the ground, and a plate would paste a dark
// square onto the light themes.
//
// The viewBox is cropped to the mark's own bounds so a 20px render spends its
// pixels on the hexagon instead of on the padding a square icon file needs.
import { useId } from 'react';

export interface KronnMarkProps {
  /** Rendered edge length in pixels. */
  size?: number;
  className?: string;
  /** Decorative next to the word "Kronn"; give it a label when it stands alone. */
  title?: string;
}

export function KronnMark({ size = 20, className, title }: KronnMarkProps) {
  // Two marks on the same page would otherwise share one gradient id, and the
  // second would paint with the first one's stops.
  const uid = useId();
  const frame = `kronn-frame-${uid}`;
  const top = `kronn-top-${uid}`;
  const hub = `kronn-hub-${uid}`;
  const peer = `kronn-peer-${uid}`;
  return (
    <svg
      width={size}
      height={size}
      viewBox="38 48 436 436"
      className={className}
      role={title ? 'img' : 'presentation'}
      aria-label={title}
      aria-hidden={title ? undefined : true}
      focusable="false"
    >
      {title && <title>{title}</title>}
      <defs>
        {/* One diagonal run for the whole frame: cyan at the top-right of the
            crown, magenta at the left flank and foot, blue-violet between. */}
        {/* Spanning the whole mark, not each stroke: with the default
            per-element units every arc restated the full cyan-to-magenta run
            and the frame read as three unrelated gradients. */}
        <linearGradient id={frame} gradientUnits="userSpaceOnUse" x1="441" y1="60" x2="84" y2="470">
          <stop offset="0" stopColor="#1fbdda" />
          <stop offset="0.45" stopColor="#6f5ef0" />
          <stop offset="1" stopColor="#bd3ce6" />
        </linearGradient>
        {/* Deeper than the icon files' nodes on purpose. Those sit on their
            own dark plate; this one sits on whatever the theme paints, and the
            logo's pale pinks disappeared into white at 20px — the mark lost
            its middle and read as an empty hexagon. */}
        <linearGradient id={top} x1="0.2" y1="0" x2="0.8" y2="1">
          <stop offset="0" stopColor="#5fd8ea" />
          <stop offset="1" stopColor="#17b4d4" />
        </linearGradient>
        <linearGradient id={hub} x1="0.2" y1="0" x2="0.8" y2="1">
          <stop offset="0" stopColor="#a98ee8" />
          <stop offset="1" stopColor="#4a86e8" />
        </linearGradient>
        <linearGradient id={peer} x1="0.2" y1="0" x2="0.8" y2="1">
          <stop offset="0" stopColor="#dc94e6" />
          <stop offset="1" stopColor="#9b6ef0" />
        </linearGradient>
      </defs>
      {/* Hexagon: shallow crown, vertical flanks, tapered foot. */}
      <path
        d="M256 60 L429 152 V380 L256 470 L84 380 V152 Z"
        fill="none"
        stroke={`url(#${frame})`}
        strokeWidth="25"
        strokeLinejoin="round"
      />
      {/* The orbit around the graph, broken where the three nodes sit. */}
      <g fill="none" stroke={`url(#${frame})`} strokeWidth="12" strokeLinecap="round">
        <path d="M148.4 257.6 A 108 108 0 0 1 210.4 169.1" />
        <path d="M301.6 169.1 A 108 108 0 0 1 363.6 257.6" />
        <path d="M318 355.5 A 108 108 0 0 1 194 355.5" />
      </g>
      {/* The graph it guards: one hub, three peers, the links between them. */}
      <g fill="none" stroke={`url(#${frame})`} strokeWidth="9" strokeLinecap="round">
        <path d="M256 273 V165" />
        <path d="M256 273 L157 316" />
        <path d="M256 273 L356 316" />
      </g>
      <circle cx="256" cy="165" r="33" fill={`url(#${top})`} />
      <circle cx="157" cy="316" r="29" fill={`url(#${peer})`} />
      <circle cx="356" cy="316" r="29" fill={`url(#${peer})`} />
      <circle cx="256" cy="273" r="50" fill={`url(#${hub})`} />
    </svg>
  );
}
