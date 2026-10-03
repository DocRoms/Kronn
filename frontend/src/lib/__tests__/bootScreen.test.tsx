// KT-986 — one loading screen from the first frame until the app is ready.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import { useState } from 'react';
import {
  armBootScreen,
  bootMessage,
  bootScreenActive,
  finishBootScreen,
  holdBootScreen,
  resetBootScreenForTests,
  setBootPhase,
} from '../bootScreen';
import { BootHold, LoadingState } from '../../components/LoadingState';

function mountBootScreen() {
  // What index.html paints before any script runs.
  const boot = document.createElement('div');
  boot.id = 'kronn-boot';
  boot.innerHTML = '<span data-boot-text>Starting Kronn…</span><button data-boot-retry hidden>Retry</button>';
  document.body.prepend(boot);
  return boot;
}

const bootText = () => document.querySelector('[data-boot-text]')?.textContent;

beforeEach(() => {
  vi.useFakeTimers();
  resetBootScreenForTests();
  document.getElementById('kronn-boot')?.remove();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('boot screen', () => {
  it('stays up while something holds it and fades out once nothing does', () => {
    mountBootScreen();
    const release = holdBootScreen();
    armBootScreen();
    vi.advanceTimersByTime(1_000);
    expect(bootScreenActive()).toBe(true);

    release();
    vi.advanceTimersByTime(149);
    expect(bootScreenActive()).toBe(true);
    vi.advanceTimersByTime(1);
    expect(bootScreenActive()).toBe(false);
    expect(document.getElementById('kronn-boot')).toHaveClass('kronn-boot--done');
    vi.advanceTimersByTime(200);
    expect(document.getElementById('kronn-boot')).toBeNull();
  });

  it('does not flash off between one loader unmounting and the next mounting', () => {
    mountBootScreen();
    const first = holdBootScreen();
    first();
    vi.advanceTimersByTime(100);
    const second = holdBootScreen();
    vi.advanceTimersByTime(1_000);
    expect(bootScreenActive()).toBe(true);
    second();
    vi.advanceTimersByTime(150);
    expect(bootScreenActive()).toBe(false);
  });

  it('changes only its sentence, and shows a retry when given one', () => {
    mountBootScreen();
    setBootPhase('connecting');
    expect(bootText()).toBe(bootMessage('connecting'));
    const retry = document.querySelector<HTMLButtonElement>('[data-boot-retry]')!;
    expect(retry.hidden).toBe(true);

    const onRetry = vi.fn();
    setBootPhase('slow', onRetry);
    expect(bootText()).toBe(bootMessage('slow'));
    expect(retry.hidden).toBe(false);
    retry.click();
    expect(onRetry).toHaveBeenCalledOnce();

    setBootPhase('opening');
    expect(retry.hidden).toBe(true);
    retry.click();
    expect(onRetry).toHaveBeenCalledOnce();
  });

  it('speaks the user interface language', () => {
    expect(bootMessage('opening', 'fr')).toBe('Ouverture de Kronn…');
    expect(bootMessage('opening', 'en')).toBe('Opening Kronn…');
  });
});

describe('LoadingState', () => {
  it('keeps the start-up screen instead of drawing a second one', () => {
    mountBootScreen();
    const { container } = render(<LoadingState fullscreen phase="connecting" />);
    expect(container).toBeEmptyDOMElement();
    expect(bootText()).toBe(bootMessage('connecting'));

    act(() => { armBootScreen(); vi.advanceTimersByTime(1_000); });
    expect(bootScreenActive()).toBe(true);
  });

  it('draws the mark and its sentence once the app is open', () => {
    render(<LoadingState message="Chargement des projets…" />);
    const status = screen.getByRole('status');
    expect(status).toHaveTextContent('Chargement des projets…');
    expect(status.querySelector('svg')).toBeInTheDocument();
  });

  it('draws itself if the start-up screen closes while it is waiting', () => {
    mountBootScreen();
    render(<LoadingState message="Chargement…" />);
    expect(screen.queryByRole('status')).toBeNull();
    act(() => finishBootScreen());
    expect(screen.getByRole('status')).toHaveTextContent('Chargement…');
  });
});

describe('BootHold', () => {
  it('releases the screen when its data has settled', () => {
    mountBootScreen();
    let settle!: () => void;
    function Page() {
      const [loading, setLoading] = useState(true);
      settle = () => setLoading(false);
      return <BootHold active={loading} />;
    }
    render(<Page />);
    act(() => { armBootScreen(); vi.advanceTimersByTime(1_000); });
    expect(bootScreenActive()).toBe(true);

    act(() => settle());
    act(() => { vi.advanceTimersByTime(150); });
    expect(bootScreenActive()).toBe(false);
  });
});
