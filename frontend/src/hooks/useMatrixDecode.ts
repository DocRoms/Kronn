import { useEffect, useRef, useState } from 'react';

/** Per-pulse chance that a given MatrixText will re-scramble. Tuned so
 *  that with ~20 visible MatrixText instances and a pulse every 8-14s,
 *  the user sees ~3 titles scramble per pulse — enough to feel alive
 *  without overwhelming. Lower for pages with fewer instances. */
const PULSE_REDECODE_CHANCE = 0.15;

/** Character pool for the scramble phase. Mostly half-width katakana +
 *  digits + ASCII symbols — the Mr. Robot / Matrix aesthetic. */
const MATRIX_CHARS =
  'アイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワヲン0123456789!@#$%&*+=<>/?';

const TOTAL_FRAMES = 28; // ~470ms at 60fps — fast enough to not delay reading
const FRAME_MS = 16;

/** One-shot "decode" animation over `target`. Returns the string currently
 *  displayed — random chars first, progressively settling to `target`.
 *
 *  Runs the animation whenever `target` changes AND `active` is true. If
 *  `active` is false the hook is a no-op (returns `target` verbatim) so
 *  MatrixText renders plain text outside the matrix theme.
 *
 *  Whitespace characters (spaces, newlines, tabs) are always locked to
 *  their target value — scrambling spaces would make the text width
 *  jump around, which looks awful in a sidebar.
 *
 *  Caller can still mutate the ref returned by `stopRef` to abort early
 *  (rare — e.g. if the component unmounts mid-decode, but the effect
 *  cleanup already handles that). */
export function useMatrixDecode(target: string, active: boolean): string {
  const [display, setDisplay] = useState<string>(active ? '' : target);
  const [pulseTick, setPulseTick] = useState(0);
  const lastTargetRef = useRef<string>('');
  const lastPulseRef = useRef<number>(0);

  // Global `matrix:pulse` event — fired by ThemeEffects every 8-14s
  // when the matrix theme is active. Each MatrixText instance rolls a
  // dice and re-scrambles on a hit. Keeps the page alive without
  // re-scrambling everything in sync (which would feel robotic).
  useEffect(() => {
    if (!active) return;
    const onPulse = () => {
      if (Math.random() < PULSE_REDECODE_CHANCE) {
        setPulseTick(t => t + 1);
      }
    };
    window.addEventListener('matrix:pulse', onPulse);
    return () => window.removeEventListener('matrix:pulse', onPulse);
  }, [active]);

  useEffect(() => {
    if (!active) {
      setDisplay(target);
      lastTargetRef.current = target;
      lastPulseRef.current = pulseTick;
      return;
    }
    const targetChanged = lastTargetRef.current !== target;
    const pulseFired = lastPulseRef.current !== pulseTick;
    if (!targetChanged && !pulseFired) return;
    lastTargetRef.current = target;
    lastPulseRef.current = pulseTick;

    // Settle order: left-to-right feels unnatural (too predictable), so
    // we shuffle the char indices — each frame settles a few more.
    const order = shuffleIndices(target.length);
    let frame = 0;

    const tick = () => {
      const settledCount = Math.min(
        target.length,
        Math.ceil((frame / TOTAL_FRAMES) * target.length),
      );
      const settled = new Set(order.slice(0, settledCount));

      // Build the next display string in a single pass.
      let out = '';
      for (let i = 0; i < target.length; i++) {
        const ch = target[i];
        // Always preserve whitespace + already-settled chars.
        if (settled.has(i) || /\s/.test(ch)) {
          out += ch;
        } else {
          out += MATRIX_CHARS[Math.floor(Math.random() * MATRIX_CHARS.length)];
        }
      }
      setDisplay(out);
    };

    tick();
    const interval = setInterval(() => {
      frame++;
      if (frame >= TOTAL_FRAMES) {
        clearInterval(interval);
        setDisplay(target);
        return;
      }
      tick();
    }, FRAME_MS);

    return () => clearInterval(interval);
  }, [target, active, pulseTick]);

  return display;
}

/** Fisher-Yates shuffle of `[0, 1, ..., n-1]`. Deterministic output
 *  length but random order — used to decide which chars settle first. */
function shuffleIndices(n: number): number[] {
  const arr = Array.from({ length: n }, (_, i) => i);
  for (let i = arr.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [arr[i], arr[j]] = [arr[j], arr[i]];
  }
  return arr;
}
