import React from 'react';
import ReactDOM from 'react-dom/client';
import { RouterProvider } from 'react-router/dom';
import './styles/index.css';
import { createAppRouter } from './router';
import { I18nProvider } from './lib/I18nContext';
import { ThemeProvider } from './lib/ThemeContext';
import { LayoutDensityProvider } from './lib/LayoutDensityContext';
import { LocalIdentityProvider } from './lib/LocalIdentityContext';
import { ThemeEffects } from './components/ThemeEffects';
import { loadInitialLocale, renderBootstrapFailure } from './lib/bootstrapLocale';
import { finishBootScreen } from './lib/bootScreen';
import {
  getDesktopBackendUrl,
  isTauriAssetLocation,
  isTauriRuntime,
  retryDesktopStartup,
} from './lib/tauri';
import { bootUiPreferences, startUiPreferencesSync } from './lib/uiPreferences';

// The packaged shell waits for its owned backend before loading the real UI.
async function navigateToDesktopBackend(): Promise<boolean> {
  if (!isTauriRuntime() || !isTauriAssetLocation(window.location)) return false;
  const url = await getDesktopBackendUrl();
  if (!url) return false;
  window.location.replace(url);
  return true;
}

async function bootstrap() {
  if (await navigateToDesktopBackend()) return;
  // Load exactly the active dictionary before first paint. Other locales stay
  // in their own Vite chunks until the user switches language. A new origin
  // (desktop port change) takes its preferences from the server first.
  await Promise.all([loadInitialLocale(), bootUiPreferences()]);
  const rootEl = document.getElementById('root');
  if (!rootEl) throw new Error('Missing #root element in index.html');
  const router = createAppRouter();
  ReactDOM.createRoot(rootEl).render(
    <React.StrictMode>
      <ThemeProvider>
        <LayoutDensityProvider>
          <I18nProvider>
            <LocalIdentityProvider>
              <ThemeEffects />
              {/* Router state updates are applied synchronously, like a plain
                  state change: wrapped in a transition, a navigation render
                  could be interrupted by the next one — Back right after a
                  click — and never commit, leaving the page on an address it
                  no longer has. See `docs/architecture/ui-structure.md`. */}
              <RouterProvider router={router} useTransitions={false} />
            </LocalIdentityProvider>
          </I18nProvider>
        </LayoutDensityProvider>
      </ThemeProvider>
    </React.StrictMode>,
  );
  startUiPreferencesSync();
}

void bootstrap().catch(error => {
  console.error('[bootstrap] failed to load the interface:', error);
  finishBootScreen();
  const rootEl = document.getElementById('root');
  const detail = error instanceof Error ? error.message : String(error);
  if (rootEl) renderBootstrapFailure(rootEl, () => void retryDesktopStartup(), detail);
});
