import { useEffect, useMemo, useRef } from 'react';
import { useTheme } from '../lib/ThemeContext';
import '../styles/theme-effects.css';

/** Per-theme decorative overlay (sakura petals, batman bat-signal + bats).
 *  Rendered once at the App root — content swaps as the theme changes.
 *  Every element is `pointer-events: none` via the CSS so clicks pass
 *  through. Matrix doesn't need an overlay (its effect is text-based
 *  via `<MatrixText />`) but we still emit global `matrix:pulse` events
 *  from here so listening MatrixText instances can occasionally
 *  re-scramble their titles. */
export function ThemeEffects() {
  const { theme } = useTheme();

  const petals = useMemo(() => makeSakuraPetals(), []);
  const bats = useMemo(() => makeBats(), []);

  // ── Matrix pulse scheduler ────────────────────────────────────────
  // Each pulse fires a CustomEvent that every `useMatrixDecode` hook
  // listens to; each hook rolls a dice and decides whether to
  // re-scramble. Intervals are jittered (8-14s) so the page never
  // feels "metronomic". Only runs while matrix theme is active.
  useEffect(() => {
    if (theme !== 'matrix') return;
    let timeout: number;
    const schedule = () => {
      const delay = 8000 + Math.random() * 6000;
      timeout = window.setTimeout(() => {
        window.dispatchEvent(new CustomEvent('matrix:pulse'));
        schedule();
      }, delay);
    };
    schedule();
    return () => window.clearTimeout(timeout);
  }, [theme]);

  // ── Sakura wind (mouse proximity pushes petals) ───────────────────
  // A refs array is paired with each rendered petal's INNER span. On
  // mousemove we compute distance to each petal center and, if within
  // `WIND_RADIUS`, apply a repulsive transform proportional to
  // closeness. CSS transitions smooth the push + the return-to-rest.
  // The outer `.sakura-petal` span keeps the falling keyframe; the
  // inner `.sakura-petal-inner` carries the wind offset — transforms
  // compose so both animations coexist without fighting.
  const petalRefs = useRef<Array<HTMLSpanElement | null>>([]);
  useEffect(() => {
    if (theme !== 'sakura') return;
    // Respect prefers-reduced-motion: don't run the mouse listener
    // when sprites are suppressed (they'd be `opacity: 0` anyway).
    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return;

    const WIND_RADIUS = 90;
    const MAX_PUSH = 55;
    let rafQueued = false;
    let lastX = 0;
    let lastY = 0;

    const apply = () => {
      rafQueued = false;
      for (const el of petalRefs.current) {
        if (!el) continue;
        const rect = el.getBoundingClientRect();
        const cx = rect.left + rect.width / 2;
        const cy = rect.top + rect.height / 2;
        const dx = cx - lastX;
        const dy = cy - lastY;
        const dist = Math.hypot(dx, dy);
        if (dist < WIND_RADIUS && dist > 0.01) {
          // Repulsion: strength grows as distance shrinks, tapers to
          // zero at the edge of the interaction radius. Direction is
          // AWAY from the cursor — feels like a breath pushing the
          // petal aside.
          const force = (1 - dist / WIND_RADIUS) * MAX_PUSH;
          const pushX = (dx / dist) * force;
          const pushY = (dy / dist) * force;
          el.style.transform = `translate(${pushX.toFixed(1)}px, ${pushY.toFixed(1)}px) rotate(${(pushX * 0.6).toFixed(1)}deg)`;
          el.style.transition = 'transform 0.25s cubic-bezier(0.2, 0.7, 0.25, 1)';
        } else if (el.style.transform) {
          // Outside radius — ease back to rest. Longer transition so
          // the petal "recovers" gracefully rather than snapping back.
          el.style.transform = '';
          el.style.transition = 'transform 1.4s ease-out';
        }
      }
    };

    const onMove = (e: MouseEvent) => {
      lastX = e.clientX;
      lastY = e.clientY;
      if (!rafQueued) {
        rafQueued = true;
        requestAnimationFrame(apply);
      }
    };

    window.addEventListener('mousemove', onMove, { passive: true });
    return () => {
      window.removeEventListener('mousemove', onMove);
      // Reset all petals on theme change so lingering inline styles
      // don't pin them off-track.
      for (const el of petalRefs.current) {
        if (el) { el.style.transform = ''; el.style.transition = ''; }
      }
    };
  }, [theme]);

  if (theme === 'sakura') {
    return (
      <div className="theme-effects-root" aria-hidden="true">
        {petals.map((p, i) => (
          <span key={i} className="sakura-petal" style={p}>
            <span
              ref={(el) => { petalRefs.current[i] = el; }}
              className="sakura-petal-inner"
            >🌸</span>
          </span>
        ))}
      </div>
    );
  }

  if (theme === 'gotham') {
    return (
      <div className="theme-effects-root" aria-hidden="true">
        <div className="bat-signal" />
        {bats.map((b, i) => (
          <span key={i} className="bat" style={b}>🦇</span>
        ))}
      </div>
    );
  }

  return null;
}

function makeSakuraPetals(): React.CSSProperties[] {
  const configs: React.CSSProperties[] = [];
  const count = 6;
  for (let i = 0; i < count; i++) {
    const startX = `${rand(5, 95)}vw`;
    const drift = rand(-12, 12);
    const endX = `calc(${startX} + ${drift}vw)`;
    const duration = `${rand(18, 30)}s`;
    const delay = `${-rand(0, 25)}s`;
    const size = `${rand(16, 28)}px`;
    const spin = `${rand(360, 1080)}deg`;
    const maxOpacity = rand(0.55, 0.9).toFixed(2);
    configs.push({
      ['--start-x' as string]: startX,
      ['--end-x' as string]: endX,
      ['--duration' as string]: duration,
      ['--delay' as string]: delay,
      ['--petal-size' as string]: size,
      ['--spin' as string]: spin,
      ['--max-opacity' as string]: maxOpacity,
    } as React.CSSProperties);
  }
  return configs;
}

function makeBats(): React.CSSProperties[] {
  const configs: React.CSSProperties[] = [];
  const altitudes = [8, 22, 36];
  const sizes = [16, 22, 18];
  const durations = [16, 20, 18];
  const delays = [-2, -9, -15];
  for (let i = 0; i < 3; i++) {
    configs.push({
      ['--fly-y' as string]: `${altitudes[i]}vh`,
      ['--bat-size' as string]: `${sizes[i]}px`,
      ['--duration' as string]: `${durations[i]}s`,
      ['--delay' as string]: `${delays[i]}s`,
      ['--max-opacity' as string]: '0.55',
    } as React.CSSProperties);
  }
  return configs;
}

function rand(min: number, max: number): number {
  return Math.random() * (max - min) + min;
}
