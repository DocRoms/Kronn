export interface PinScrollOptions {
  /** How long to wait for the target to be rendered at all. */
  lookupMs?: number;
  /** How long to keep re-centring once the target is found. */
  settleMs?: number;
  /** Runs on every frame the target is re-centred. */
  onStep?: () => void;
  onSettled?: () => void;
  onMissing?: () => void;
  /** A wheel or touch gesture took over; the pin stopped. */
  onAbort?: () => void;
}

/**
 * Centre `find()` in `container` and keep it centred for a short window.
 * A one-shot `scrollIntoView` aims at the layout measured when it fires, but
 * message rows use `content-visibility: auto` and only take their real height
 * as the scroll reveals them, so it lands short. Returns a stop function.
 */
export function pinScrollTarget(
  container: HTMLElement,
  find: () => HTMLElement | null,
  { lookupMs = 1_500, settleMs = 700, onStep, onSettled, onMissing, onAbort }: PinScrollOptions = {},
): () => void {
  let frame: number | null = null;
  let settleDeadline: number | null = null;
  const lookupDeadline = performance.now() + lookupMs;
  const stop = () => {
    if (frame !== null) cancelAnimationFrame(frame);
    frame = null;
    container.removeEventListener('wheel', abortByUser);
    container.removeEventListener('touchstart', abortByUser);
  };
  const abortByUser = () => {
    stop();
    onAbort?.();
  };
  container.addEventListener('wheel', abortByUser, { passive: true });
  container.addEventListener('touchstart', abortByUser, { passive: true });
  const step = () => {
    const now = performance.now();
    const target = find();
    if (!target) {
      if (now < lookupDeadline) {
        frame = requestAnimationFrame(step);
      } else {
        stop();
        onMissing?.();
      }
      return;
    }
    onStep?.();
    settleDeadline ??= now + settleMs;
    const targetRect = target.getBoundingClientRect();
    const containerRect = container.getBoundingClientRect();
    container.scrollTop += (
      (targetRect.top + targetRect.bottom) / 2
      - (containerRect.top + containerRect.bottom) / 2
    );
    if (now <= settleDeadline) {
      frame = requestAnimationFrame(step);
      return;
    }
    stop();
    onSettled?.();
  };
  frame = requestAnimationFrame(step);
  return stop;
}
