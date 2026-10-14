import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { revealWhenSettled } from '../revealWhenSettled';

// jsdom has no layout: each test drives the target's position by hand.
function target(html: string, id: string) {
  document.body.innerHTML = html;
  const element = document.getElementById(id)!;
  let top = 400;
  element.getBoundingClientRect = () => ({ top } as DOMRect);
  const scroll = vi.fn();
  element.scrollIntoView = scroll;
  return { element, scroll, moveTo: (next: number) => { top = next; } };
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['requestAnimationFrame', 'cancelAnimationFrame', 'performance', 'Date', 'setTimeout'] });
});
afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = '';
});

describe('revealWhenSettled', () => {
  it('waits for a field to be enabled and to stop moving, then centres and focuses it', () => {
    const { element, scroll, moveTo } = target('<select id="field" disabled><option>1</option></select>', 'field');
    revealWhenSettled('field');

    vi.advanceTimersByTime(1000);
    expect(scroll).not.toHaveBeenCalled(); // disabled: its value is not loaded yet

    (element as HTMLSelectElement).disabled = false;
    moveTo(600); // content above it is still arriving
    vi.advanceTimersByTime(100);
    moveTo(700);
    vi.advanceTimersByTime(100);
    expect(scroll).not.toHaveBeenCalled();

    vi.advanceTimersByTime(300); // held still for longer than the settle time
    expect(scroll).toHaveBeenCalledWith({ behavior: 'smooth', block: 'center' });
    expect(document.activeElement).toBe(element);
  });

  it('scrolls a section to the top, without focusing it', () => {
    const { scroll } = target('<section id="settings-server">Server</section>', 'settings-server');
    revealWhenSettled('settings-server');
    vi.advanceTimersByTime(400);
    expect(scroll).toHaveBeenCalledWith({ behavior: 'smooth', block: 'start' });
    expect(document.activeElement).toBe(document.body);
  });

  it('puts the target back when content above it pushes it away after the reveal', () => {
    const { scroll, moveTo } = target('<section id="s">S</section>', 's');
    revealWhenSettled('s');
    vi.advanceTimersByTime(400);
    expect(scroll).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(700); // the smooth scroll is over: this is where it stands
    moveTo(900);                 // a section above finished loading
    vi.advanceTimersByTime(50);
    expect(scroll).toHaveBeenLastCalledWith({ behavior: 'auto', block: 'start' });
  });

  it('stops following once the reader scrolls on their own', () => {
    const { scroll, moveTo } = target('<section id="s">S</section>', 's');
    revealWhenSettled('s');
    vi.advanceTimersByTime(1000);
    window.dispatchEvent(new Event('wheel'));
    moveTo(900);
    vi.advanceTimersByTime(200);
    expect(scroll).toHaveBeenCalledTimes(1);
  });

  it('gives up on a target that never appears, and can be cancelled', () => {
    document.body.innerHTML = '';
    const cancel = revealWhenSettled('nowhere');
    vi.advanceTimersByTime(4000);
    const { scroll } = target('<section id="later">L</section>', 'later');
    const cancelLater = revealWhenSettled('later');
    cancelLater();
    vi.advanceTimersByTime(1000);
    expect(scroll).not.toHaveBeenCalled();
    cancel();
  });
});
