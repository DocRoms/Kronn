// KT-2017 — the band of a Settings sub-page: the links to its sections stay
// under the app's top bar while the sub-page scrolls, the section being read
// marked. happy-dom has no layout, so the boxes are stubbed on one model of the
// « Vous » sub-page: the head 0–100 px, the band in flow at 100–144 px and
// stuck at 51 px (the top bar's height) once the head has scrolled away, four
// sections at 160, 900, 1700 and 2300 px, a 900 px high window and a 2700 px
// high document.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { settingsSectionsOf } from '../../../lib/routes';
import { SettingsSubPageHead } from '../SettingsSection';
import { CLICK_HOLD_IDLE_MS, FOCUS_BREATH_PX } from '../useSubPageBand';

const BAR = 51;
const HEAD = 100;
const BAND = 44;
const WINDOW = 900;
const DOCUMENT = 2700;
const SECTIONS = settingsSectionsOf('you');
const IDS = SECTIONS.map(section => section.id);
const TOPS: Record<string, number> = {
  'settings-identity': 160, 'settings-appearance': 900, 'settings-voice': 1700, 'settings-user-context': 2300,
};

let scrollY = 0;
let frames: FrameRequestCallback[] = [];
const boxes = new Map<Element, () => DOMRect>();
const restore: Array<() => void> = [];

const rect = (top: number, height: number, left = 0, width = 600) => ({
  top, bottom: top + height, left, right: left + width, width, height, x: left, y: top, toJSON: () => ({}),
}) as DOMRect;

function define(target: object, prop: string, descriptor: PropertyDescriptor) {
  const original = Object.getOwnPropertyDescriptor(target, prop);
  Object.defineProperty(target, prop, { configurable: true, ...descriptor });
  restore.push(() => {
    if (original) Object.defineProperty(target, prop, original);
    else delete (target as Record<string, unknown>)[prop];
  });
}

const flushFrames = () => act(() => {
  const queued = frames;
  frames = [];
  queued.forEach(callback => callback(0));
});

const scrollTo = async (y: number) => {
  scrollY = y;
  await act(async () => { window.dispatchEvent(new Event('scroll')); });
  await flushFrames();
};

const band = () => screen.getByRole('navigation', { name: 'settings.subPage.sections' });
const linkTo = (id: string) => band().querySelector<HTMLAnchorElement>(`a[href$="#${id}"]`)!;
const marked = () => [...band().querySelectorAll('a[aria-current="location"]')].map(a => a.getAttribute('href'));

function Page({ active = true, onSection = vi.fn() }: { active?: boolean; onSection?: (anchor: string) => void }) {
  return (
    <div className="set-subpage" data-testid="box">
      <SettingsSubPageHead page="you" sections={SECTIONS} active={active} onSection={onSection} />
      {IDS.map(id => (
        <section key={id} id={id}>
          <h3>{id}</h3>
          <button type="button">{`control ${id}`}</button>
        </section>
      ))}
      <div role="dialog"><button type="button">in a dialog</button></div>
    </div>
  );
}

/** Stubs the boxes of what the page drew, on the model above. */
function layOut() {
  const box = screen.getByTestId('box');
  boxes.set(box.querySelector('.set-subpage-head')!, () => rect(-scrollY, HEAD));
  boxes.set(band(), () => rect(Math.max(BAR, HEAD - scrollY), BAND));
  for (const id of IDS) boxes.set(document.getElementById(id)!, () => rect(TOPS[id] - scrollY, 300));
  // The line shows 300 px; the links stand 100 px apart, 80 px wide.
  boxes.set(band().querySelector('.kr-header-line-scroller')!, () => rect(BAR, BAND, 0, 300));
  IDS.forEach((id, index) => boxes.set(linkTo(id), () => rect(BAR, 28, index * 100, 80)));
}

beforeEach(() => {
  scrollY = 0;
  frames = [];
  boxes.clear();
  const native = HTMLElement.prototype.getBoundingClientRect;
  define(HTMLElement.prototype, 'getBoundingClientRect', {
    value(this: HTMLElement) { return boxes.get(this)?.() ?? native.call(this); },
  });
  define(window, 'scrollY', { get: () => scrollY });
  define(window, 'innerHeight', { get: () => WINDOW });
  define(document.scrollingElement ?? document.documentElement, 'scrollHeight', { get: () => DOCUMENT });
  define(window, 'requestAnimationFrame', { value: (callback: FrameRequestCallback) => frames.push(callback), writable: true });
  define(window, 'cancelAnimationFrame', { value: () => {}, writable: true });
  define(window, 'scrollBy', { value: vi.fn(), writable: true });
  define(window, 'matchMedia', { value: () => ({ matches: false }), writable: true });
});

afterEach(() => {
  vi.useRealTimers();
  restore.splice(0).reverse().forEach(undo => undo());
});

async function renderPage(props: Parameters<typeof Page>[0] = {}) {
  const view = render(<Page {...props} />);
  layOut();
  await flushFrames();
  return view;
}

describe('the band of section links of a Settings sub-page', () => {
  it('is the same named landmark with the same links, in its own box after the head, so it can stick', async () => {
    await renderPage();
    const nav = band();
    expect(nav.parentElement).toBe(screen.getByTestId('box'));
    expect(nav.previousElementSibling).toHaveClass('set-subpage-head');
    expect(nav).toHaveClass('set-subpage-band');
    expect(nav.previousElementSibling!.querySelector('a')).toBeNull();
    expect([...nav.querySelectorAll('a')].map(a => a.getAttribute('href'))).toEqual(IDS.map(id => `/config/you#${id}`));
    // One line that scrolls sideways (HeaderScrollLine), not a wrapping row.
    expect(nav.querySelector('.kr-header-line-scroller .set-subpage-links')).not.toBeNull();
    // Each link once, in the document's tab order: none is taken out of it.
    expect([...nav.querySelectorAll('a')].every(a => !a.hasAttribute('tabindex'))).toBe(true);
  });

  it('marks the section being read as the page scrolls, and the last one at the bottom', async () => {
    await renderPage();
    expect(marked()).toEqual(['/config/you#settings-identity']);

    // « Interface & langues » lands under the band (as an anchor does).
    await scrollTo(TOPS['settings-appearance'] - BAR - BAND - FOCUS_BREATH_PX);
    expect(marked()).toEqual(['/config/you#settings-appearance']);

    // Further on, « Voix » still below: still « Interface & langues ».
    await scrollTo(1300);
    expect(marked()).toEqual(['/config/you#settings-appearance']);

    await scrollTo(1650);
    expect(marked()).toEqual(['/config/you#settings-voice']);

    // The bottom of the page: « Mes contextes » never reaches the band, yet it is the one read.
    await scrollTo(DOCUMENT - WINDOW);
    expect(document.getElementById('settings-user-context')!.getBoundingClientRect().top).toBeGreaterThan(BAR + BAND + 32);
    expect(marked()).toEqual(['/config/you#settings-user-context']);
  });

  it('draws its line once stuck under the top bar, not before', async () => {
    await renderPage();
    expect(band()).not.toHaveAttribute('data-stuck');
    await scrollTo(30);
    expect(band()).not.toHaveAttribute('data-stuck');
    await scrollTo(400);
    expect(band()).toHaveAttribute('data-stuck', 'true');
    await scrollTo(0);
    expect(band()).not.toHaveAttribute('data-stuck');
  });

  it('marks a clicked link at once, keeps it while its scroll runs, and brings it into view in the line', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    const onSection = vi.fn();
    await renderPage({ onSection });
    const line = band().querySelector<HTMLElement>('.kr-header-line-scroller')!;
    const lineScroll = vi.fn();
    line.scrollTo = lineScroll as typeof line.scrollTo;

    // « Mes contextes » (300–380 px) lies past the 300 px the line shows.
    fireEvent.click(linkTo('settings-user-context'));
    expect(onSection).toHaveBeenCalledWith('settings-user-context');
    expect(marked()).toEqual(['/config/you#settings-user-context']);
    expect(lineScroll).toHaveBeenLastCalledWith({ left: 380 - 300 + 24, behavior: 'smooth' });

    // The smooth scroll passes « Interface & langues »: the click's mark holds.
    await scrollTo(800);
    expect(marked()).toEqual(['/config/you#settings-user-context']);
    await act(async () => { vi.advanceTimersByTime(CLICK_HOLD_IDLE_MS - 10); });
    await scrollTo(1200);
    expect(marked()).toEqual(['/config/you#settings-user-context']);

    // Once the scroll rests, the reader's own scroll is followed again.
    await act(async () => { vi.advanceTimersByTime(CLICK_HOLD_IDLE_MS); });
    expect(marked()).toEqual(['/config/you#settings-user-context']);
    await scrollTo(TOPS['settings-appearance'] - BAR - BAND - FOCUS_BREATH_PX);
    expect(marked()).toEqual(['/config/you#settings-appearance']);
    // « Interface & langues » (100–180 px) is already in view: the line stays.
    expect(lineScroll).toHaveBeenCalledTimes(1);
  });

  it('scrolls the line without motion when the reader asks for less', async () => {
    define(window, 'matchMedia', { value: (query: string) => ({ matches: query === '(prefers-reduced-motion: reduce)' }), writable: true });
    await renderPage();
    const line = band().querySelector<HTMLElement>('.kr-header-line-scroller')!;
    const lineScroll = vi.fn();
    line.scrollTo = lineScroll as typeof line.scrollTo;
    fireEvent.click(linkTo('settings-user-context'));
    expect(lineScroll).toHaveBeenLastCalledWith({ left: 104, behavior: 'auto' });
  });

  it('listens to nothing while its sub-page is hidden, and stops when it is hidden again', async () => {
    const added = vi.spyOn(window, 'addEventListener');
    const removed = vi.spyOn(window, 'removeEventListener');
    const view = await renderPage({ active: false });
    const scrollListeners = (spy: typeof added) => spy.mock.calls.filter(([type]) => type === 'scroll').length;
    expect(scrollListeners(added)).toBe(0);
    expect(frames).toHaveLength(0);
    await scrollTo(1650);
    expect(marked()).toEqual([]);

    view.rerender(<Page active />);
    await flushFrames();
    expect(scrollListeners(added)).toBe(1);
    expect(marked()).toEqual(['/config/you#settings-voice']);

    view.rerender(<Page active={false} />);
    expect(scrollListeners(removed)).toBe(1);
    await scrollTo(0);
    expect(marked()).toEqual(['/config/you#settings-voice']);
    added.mockRestore();
    removed.mockRestore();
  });

  it('moves a control the Tab key leaves under the band below it, forwards and backwards', async () => {
    await renderPage();
    await scrollTo(1400);
    const control = screen.getByRole('button', { name: 'control settings-voice' });
    const scrollBy = window.scrollBy as unknown as ReturnType<typeof vi.fn>;

    for (const shiftKey of [false, true]) {
      scrollBy.mockClear();
      boxes.set(control, () => rect(60, 32));
      control.focus();
      fireEvent.keyDown(document, { key: 'Tab', shiftKey });
      await flushFrames();
      expect(scrollBy).toHaveBeenCalledWith({ top: 60 - (BAR + BAND) - FOCUS_BREATH_PX, behavior: 'auto' });
    }

    // Below the band: nothing to do.
    scrollBy.mockClear();
    boxes.set(control, () => rect(300, 32));
    fireEvent.keyDown(document, { key: 'Tab' });
    await flushFrames();
    expect(scrollBy).not.toHaveBeenCalled();

    // A link of the band itself, and a control in a dialog, stay where they are.
    for (const element of [linkTo('settings-voice'), screen.getByRole('button', { name: 'in a dialog' })]) {
      boxes.set(element, () => rect(60, 28));
      element.focus();
      fireEvent.keyDown(document, { key: 'Tab' });
      await flushFrames();
    }
    expect(scrollBy).not.toHaveBeenCalled();

    // At the top of the page the band is not stuck: it covers nothing.
    await scrollTo(0);
    boxes.set(control, () => rect(120, 32));
    control.focus();
    fireEvent.keyDown(document, { key: 'Tab' });
    await flushFrames();
    expect(scrollBy).not.toHaveBeenCalled();
  });
});
