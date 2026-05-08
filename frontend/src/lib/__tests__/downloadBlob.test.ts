import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { triggerDownload } from '../downloadBlob';

// Tiny helper — the contract of `triggerDownload` is "render an invisible
// `<a download>` and click it, then revoke the blob URL on the next tick".
// The test stubs `URL.createObjectURL` / `URL.revokeObjectURL` and asserts
// the anchor lifecycle.

describe('triggerDownload', () => {
  let createObjectURL: ReturnType<typeof vi.fn>;
  let revokeObjectURL: ReturnType<typeof vi.fn>;
  let originalCreate: typeof URL.createObjectURL;
  let originalRevoke: typeof URL.revokeObjectURL;

  beforeEach(() => {
    originalCreate = URL.createObjectURL;
    originalRevoke = URL.revokeObjectURL;
    createObjectURL = vi.fn(() => 'blob:fake/url-123');
    revokeObjectURL = vi.fn();
    URL.createObjectURL = createObjectURL as unknown as typeof URL.createObjectURL;
    URL.revokeObjectURL = revokeObjectURL as unknown as typeof URL.revokeObjectURL;
    vi.useFakeTimers();
  });

  afterEach(() => {
    URL.createObjectURL = originalCreate;
    URL.revokeObjectURL = originalRevoke;
    vi.useRealTimers();
  });

  it('creates a blob URL, clicks an invisible anchor, and revokes the URL on next tick', () => {
    const blob = new Blob(['hello'], { type: 'text/plain' });
    const clickSpy = vi.fn();
    // Spy on createElement BEFORE calling triggerDownload — we want to
    // catch the `<a>` it creates and replace its click() with our spy.
    const origCreate = document.createElement.bind(document);
    let anchor: HTMLAnchorElement | null = null;
    vi.spyOn(document, 'createElement').mockImplementation((tag) => {
      const el = origCreate(tag);
      if (tag === 'a') {
        anchor = el as HTMLAnchorElement;
        anchor.click = clickSpy;
      }
      return el;
    });

    triggerDownload('export.json', blob);

    expect(createObjectURL).toHaveBeenCalledWith(blob);
    expect(anchor).not.toBeNull();
    expect(anchor!.href).toBe('blob:fake/url-123');
    expect(anchor!.download).toBe('export.json');
    expect(clickSpy).toHaveBeenCalledTimes(1);

    // Revoke happens on the next tick — fake timers let us advance.
    expect(revokeObjectURL).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(revokeObjectURL).toHaveBeenCalledWith('blob:fake/url-123');
  });

  it('removes the anchor from the document body after click', () => {
    const blob = new Blob(['x'], { type: 'application/octet-stream' });
    const before = document.body.children.length;
    triggerDownload('a.bin', blob);
    // Anchor must NOT remain in the DOM — pre-fix a leak left N anchors
    // accumulating on every export click, which Playwright caught as
    // "expected exactly one button" assertions failing on subsequent runs.
    expect(document.body.children.length).toBe(before);
  });
});
