import { useEffect, useState } from 'react';
import { useLocation } from 'react-router';
import type { EmbedOriginPrefill } from '../components/settings/ExternalContentSection';
import { useKronnNavigate } from '../hooks/useKronnNavigate';
import { useDashboardContext } from '../lib/dashboardContext';
import { embedSettingsOrigin } from '../lib/live-page-navigation';
import { EMBED_SETTINGS_PATH, type SettingsIntent } from '../lib/routes';
import { revealWhenSettled } from '../lib/revealWhenSettled';
import { SettingsPage } from '../pages/SettingsPage';
import { useLocationIntent } from './useLocationIntent';

export function SettingsRoute() {
  const ctx = useDashboardContext();
  const nav = useKronnNavigate();
  // A model error points at one agent's tier: a one-shot arrival intent.
  const [intent, consumeIntent] = useLocationIntent<SettingsIntent>();
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
  // `/config#<anchor>`: each arrival brings the anchor (a section, or a field
  // such as `#run-payload-retention`) into view once the page has settled, so
  // the sections still loading above it do not push it away. Not the address
  // the page's own link has just set: the page has already scrolled there.
  // That is said for that one move only, and kept in memory, never in the
  // history entry: a reload, Back and Forward to the entry are arrivals.
  const { hash, key: arrival } = location;
  const [inPageMove, setInPageMove] = useState<{ anchor: string; key: string | null } | null>(null);
  if (inPageMove) {
    if (inPageMove.key === null && hash === `#${encodeURIComponent(inPageMove.anchor)}`) {
      setInPageMove({ ...inPageMove, key: arrival });
    } else if (inPageMove.key !== null && inPageMove.key !== arrival) {
      setInPageMove(null);
    }
  }
  const scrolledByPage = inPageMove !== null && (inPageMove.key === null || inPageMove.key === arrival);
  let arrivalAnchor: string | null = null;
  if (!scrolledByPage) {
    try { arrivalAnchor = decodeURIComponent(hash.slice(1)) || null; } catch { arrivalAnchor = null; }
  }
  useEffect(() => {
    if (!arrivalAnchor) return undefined;
    return revealWhenSettled(arrivalAnchor);
  }, [arrivalAnchor, arrival]);
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
      // A link to one of the page's own anchors moves the address in place:
      // a step inside one page is not a step Back has to walk through.
      onAnchorFollowed={anchorId => {
        setInPageMove({ anchor: anchorId, key: null });
        nav.toSettingsSection(anchorId, {}, { replace: true });
      }}
      arrivalAnchor={arrivalAnchor}
      arrivalToken={arrival}
      toast={ctx.toast}
      embedOriginPrefill={embedOriginPrefill}
      modelTierTarget={intent?.modelTier ?? null}
      onModelTierTargetConsumed={consumeIntent}
      // The API audit section only shows once at least one API plugin
      // (registry or custom) has a config in this Kronn instance.
      hasConfiguredApi={ctx.mcpOverview.configs.some(cfg =>
        ctx.mcpOverview.servers.some(s => s.id === cfg.server_id && s.api_spec != null)
      )}
    />
  );
}
