/** Drains deferred markdown renders during the browser's idle time.
 *
 *  Opening a room with 2 170 messages parsed 2 170 markdown documents in one
 *  synchronous pass — `react-markdown` + `remark-gfm` + `remark-emoji` each
 *  time. The messages themselves are small (1 KB on average, 2 MB for the whole
 *  room) and the backend answers in under 10 ms, so neither the data nor the
 *  query was ever the cost: it was the parsing, all of it up front.
 *
 *  This is a STARTUP optimisation, not a window. Every message still mounts,
 *  keeps its text in the DOM, and ends up fully rendered within a few idle
 *  slices — so in-room search, Cmd+F, the deep links and every "jump to this
 *  message" path keep working exactly as before. Nothing is ever unmounted.
 *
 *  Slices are bounded by the idle deadline rather than by a fixed count: a fast
 *  machine drains the room in one or two frames, a slow one takes more, and
 *  neither blocks a click. */

type Upgrade = () => void;

const pending = new Set<Upgrade>();
let scheduled = false;

/** Leaves this much of the idle slice unused. Rendering one markdown document
 *  can overshoot the deadline, and overshooting it is what drops a frame. */
const SLICE_RESERVE_MS = 4;

/** What we assume an idle callback gives us when the browser does not say.
 *  Only used by the `setTimeout` fallback path below. */
const FALLBACK_SLICE_MS = 8;

function drain(deadline?: { timeRemaining: () => number }) {
  scheduled = false;
  const started = performance.now();
  // At least one render per slice, always. Yielding on a slice too short for
  // anything is how a queue reschedules forever and never renders the room —
  // and a busy tab offers short slices precisely when there is most to do.
  let renderedOne = false;
  for (const upgrade of pending) {
    const left = deadline
      ? deadline.timeRemaining()
      : FALLBACK_SLICE_MS - (performance.now() - started);
    if (renderedOne && left <= SLICE_RESERVE_MS) break;
    pending.delete(upgrade);
    renderedOne = true;
    // One failing upgrade must not strand every message behind it.
    try {
      upgrade();
    } catch {
      /* the component will render on its own next interaction */
    }
  }
  if (pending.size > 0) schedule();
}

function schedule() {
  if (scheduled) return;
  scheduled = true;
  if (typeof requestIdleCallback === 'function') {
    // The timeout is the floor: a tab that never goes idle still drains.
    requestIdleCallback(drain, { timeout: 500 });
  } else {
    setTimeout(drain, 0);
  }
}

/** Queues one markdown render for the next idle slice. Returns a cancel
 *  function, which the caller MUST run on unmount — a queued closure holding a
 *  `setState` of a dead component is a leak and a React warning. */
export function queueMarkdownUpgrade(upgrade: Upgrade): () => void {
  pending.add(upgrade);
  schedule();
  return () => {
    pending.delete(upgrade);
  };
}

/** Test seam: the queue is module state shared by every message on the page. */
export function resetMarkdownUpgradeQueueForTests() {
  pending.clear();
  scheduled = false;
}
