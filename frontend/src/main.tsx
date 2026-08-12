import React from 'react';
import ReactDOM from 'react-dom/client';
import './styles/index.css';
import { App } from './App';
import { I18nProvider } from './lib/I18nContext';
import { ThemeProvider } from './lib/ThemeContext';
import { LayoutDensityProvider } from './lib/LayoutDensityContext';
import { LocalIdentityProvider } from './lib/LocalIdentityContext';
import { ThemeEffects } from './components/ThemeEffects';
import { loadInitialLocale, renderBootstrapFailure } from './lib/bootstrapLocale';
import {
  getDesktopBackendUrl,
  isTauriAssetLocation,
  isTauriRuntime,
  retryDesktopStartup,
} from './lib/tauri';

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
  // in their own Vite chunks until the user switches language.
  await loadInitialLocale();
  const rootEl = document.getElementById('root');
  if (!rootEl) throw new Error('Missing #root element in index.html');
  ReactDOM.createRoot(rootEl).render(
    <React.StrictMode>
      <ThemeProvider>
        <LayoutDensityProvider>
          <I18nProvider>
            <LocalIdentityProvider>
              <ThemeEffects />
              <App />
            </LocalIdentityProvider>
          </I18nProvider>
        </LayoutDensityProvider>
      </ThemeProvider>
    </React.StrictMode>,
  );
}

void bootstrap().catch(error => {
  console.error('[bootstrap] failed to load the interface:', error);
  const rootEl = document.getElementById('root');
  const detail = error instanceof Error ? error.message : String(error);
  if (rootEl) renderBootstrapFailure(rootEl, () => void retryDesktopStartup(), detail);
});
