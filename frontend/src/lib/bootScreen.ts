// The single loading screen of a cold start. `index.html` paints it before any
// script runs; the app keeps that same element and only changes its sentence
// until nothing is loading any more, then fades it out once. Loaders that
// mount meanwhile hold it instead of drawing a screen of their own, so the
// user never sees one loading screen hand over to another.

import { getUILocale, type UILocale } from './i18n';

export type BootPhase = 'connecting' | 'slow' | 'opening';

const MESSAGES: Record<UILocale, Record<BootPhase | 'retry', string>> = {
  fr: {
    connecting: 'Connexion au service local…',
    slow: 'Le service local démarre. Le premier lancement peut prendre un moment.',
    opening: 'Ouverture de Kronn…',
    retry: 'Réessayer',
  },
  en: {
    connecting: 'Connecting to the local service…',
    slow: 'The local service is starting. The first launch can take a moment.',
    opening: 'Opening Kronn…',
    retry: 'Retry',
  },
  es: {
    connecting: 'Conectando con el servicio local…',
    slow: 'El servicio local se está iniciando. El primer arranque puede tardar un momento.',
    opening: 'Abriendo Kronn…',
    retry: 'Reintentar',
  },
  zh: {
    connecting: '正在连接本地服务…',
    slow: '本地服务正在启动，首次启动可能需要一点时间。',
    opening: '正在打开 Kronn…',
    retry: '重试',
  },
};

/** Settling time between one loader unmounting and the next one mounting. */
const SETTLE_MS = 150;
const FADE_MS = 200;

let holds = 0;
let finished = false;
let settleTimer: ReturnType<typeof setTimeout> | undefined;
let retryHandler: (() => void) | null = null;
const finishListeners = new Set<() => void>();

function overlay(): HTMLElement | null {
  return typeof document === 'undefined' ? null : document.getElementById('kronn-boot');
}

export function bootMessage(phase: BootPhase | 'retry', locale: UILocale = getUILocale()): string {
  return (MESSAGES[locale] ?? MESSAGES.en)[phase];
}

/** True while the cold-start screen is on screen and owns loading feedback. */
export function bootScreenActive(): boolean {
  return !finished && overlay() !== null;
}

export function setBootPhase(phase: BootPhase, onRetry?: () => void): void {
  const root = overlay();
  if (!root || finished) return;
  const text = root.querySelector<HTMLElement>('[data-boot-text]');
  if (text) text.textContent = bootMessage(phase);
  const retry = root.querySelector<HTMLButtonElement>('[data-boot-retry]');
  if (!retry) return;
  if (retryHandler) retry.removeEventListener('click', retryHandler);
  retryHandler = onRetry ?? null;
  if (retryHandler) {
    retry.textContent = bootMessage('retry');
    retry.addEventListener('click', retryHandler);
    retry.hidden = false;
  } else {
    retry.hidden = true;
  }
}

/** Keep the screen up while something loads; returns the release. */
export function holdBootScreen(): () => void {
  if (finished) return () => {};
  holds += 1;
  if (settleTimer) clearTimeout(settleTimer);
  let released = false;
  return () => {
    if (released) return;
    released = true;
    holds -= 1;
    scheduleFinish();
  };
}

/** After the first render: nothing holding the screen means the app is ready. */
export function armBootScreen(): void {
  scheduleFinish();
}

function scheduleFinish(): void {
  if (finished || holds > 0) return;
  if (settleTimer) clearTimeout(settleTimer);
  settleTimer = setTimeout(() => {
    if (holds === 0) finishBootScreen();
  }, SETTLE_MS);
}

/** Loaders that deferred to the screen draw themselves once it is gone. */
export function onBootScreenFinished(listener: () => void): () => void {
  finishListeners.add(listener);
  return () => finishListeners.delete(listener);
}

/** Fade the screen out now: the app is ready, or has an error to show. */
export function finishBootScreen(): void {
  if (finished) return;
  finished = true;
  if (settleTimer) clearTimeout(settleTimer);
  for (const listener of [...finishListeners]) listener();
  const root = overlay();
  const html = document.documentElement;
  const cleanUp = () => {
    root?.remove();
    // index.html painted the saved theme's background before any stylesheet;
    // the theme tokens own the page from here on.
    html.classList.remove('kronn-boot-light');
    html.style.removeProperty('background');
  };
  if (!root) {
    cleanUp();
    return;
  }
  root.classList.add('kronn-boot--done');
  setTimeout(cleanUp, FADE_MS);
}

/** Test seam: forget the screen's state between tests. */
export function resetBootScreenForTests(): void {
  holds = 0;
  finished = false;
  retryHandler = null;
  finishListeners.clear();
  if (settleTimer) clearTimeout(settleTimer);
}
