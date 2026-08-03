import React from 'react';
import ReactDOM from 'react-dom/client';
import './styles/index.css';
import { App } from './App';
import { I18nProvider } from './lib/I18nContext';
import { ThemeProvider } from './lib/ThemeContext';
import { LayoutDensityProvider } from './lib/LayoutDensityContext';
import { LocalIdentityProvider } from './lib/LocalIdentityContext';
import { ThemeEffects } from './components/ThemeEffects';
import { setApiBase } from './lib/api';
import { loadInitialLocale, renderBootstrapFailure } from './lib/bootstrapLocale';

// Detect Tauri desktop mode and configure API base URL
async function initApiBase() {
  if ('__TAURI__' in window) {
    try {
      // Dynamic import hidden from TypeScript to avoid build-time dependency
      const mod = await new Function("return import('@tauri-apps/api/core')")();
      const url: string = await mod.invoke('get_backend_url');
      setApiBase(url);
    } catch {
      // Fallback: relative URLs (web mode)
    }
  }
}

async function bootstrap() {
  await initApiBase();
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
  if (rootEl) renderBootstrapFailure(rootEl);
});
