import { useEffect, useState } from 'react';
import { useLocation } from 'react-router';
import type { EmbedOriginPrefill } from '../components/settings/ExternalContentSection';
import { useKronnNavigate } from '../hooks/useKronnNavigate';
import { useDashboardContext } from '../lib/dashboardContext';
import { embedSettingsOrigin } from '../lib/live-page-navigation';
import { EMBED_SETTINGS_PATH } from '../lib/routes';
import { SettingsPage } from '../pages/SettingsPage';

export function SettingsRoute() {
  const ctx = useDashboardContext();
  const nav = useKronnNavigate();
  // A blocked embed links to the allowed-sites section with its site typed in
  // (`/config/artifacts?origin=…`), from this tab or from a standalone Page.
  // The address is an arrival: it types its site in once, then gives way to
  // `/config`, so neither a reload nor a Back replays it. Each arrival bumps
  // the nonce, so asking again for the same site refocuses it. Arrivals are
  // told apart by stepping onto the address, not by its history key: an entry
  // pushed from outside the router carries none.
  const location = useLocation();
  const onEmbedSettings = location.pathname.replace(/\/+$/, '') === EMBED_SETTINGS_PATH;
  const [embedOriginPrefill, setEmbedOriginPrefill] = useState<EmbedOriginPrefill | null>(null);
  const [embedArrivalTaken, setEmbedArrivalTaken] = useState(false);
  if (onEmbedSettings !== embedArrivalTaken) {
    setEmbedArrivalTaken(onEmbedSettings);
    if (onEmbedSettings) {
      setEmbedOriginPrefill(previous => ({ origin: embedSettingsOrigin(location.search), nonce: (previous?.nonce ?? 0) + 1 }));
    }
  }
  useEffect(() => {
    if (onEmbedSettings) nav.toPage('settings', { replace: true });
  }, [onEmbedSettings, nav]);
  return (
    <SettingsPage
      agents={ctx.agents}
      agentAccess={ctx.agentAccess}
      configLanguage={ctx.configLanguage}
      projects={ctx.projects}
      refetchAgents={ctx.refetchAgents}
      refetchAgentAccess={ctx.refetchAgentAccess}
      refetchLanguage={ctx.refetchLanguage}
      refetchProjects={ctx.refetchProjects}
      refetchDiscussions={ctx.refetchDiscussions}
      onReset={ctx.onReset}
      onNavigateDiscussion={nav.toDiscussion}
      toast={ctx.toast}
      embedOriginPrefill={embedOriginPrefill}
      // The API audit section only shows once at least one API plugin
      // (registry or custom) has a config in this Kronn instance.
      hasConfiguredApi={ctx.mcpOverview.configs.some(cfg =>
        ctx.mcpOverview.servers.some(s => s.id === cfg.server_id && s.api_spec != null)
      )}
    />
  );
}
