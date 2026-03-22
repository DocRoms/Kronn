import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './App';
import { I18nProvider } from './lib/I18nContext';
import { setApiBase } from './lib/api';

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

initApiBase().then(() => {
  ReactDOM.createRoot(document.getElementById('root')!).render(
    <React.StrictMode>
      <I18nProvider>
        <App />
      </I18nProvider>
    </React.StrictMode>,
  );
});
