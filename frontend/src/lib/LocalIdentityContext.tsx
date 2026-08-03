// 0.9.2 (KT-47) — the human's identity, available to any renderer.
//
// A `@user` mention must show WHO it addresses: the Kronn pseudo and the
// Gravatar already configured in Settings, not a generic "User". The identity
// lives in `ServerConfigPublic`, which only the Settings page used to read, and
// the mention is rendered deep inside Markdown components that receive no
// props — hence a context rather than threading two strings through five
// layers.
//
// Deliberately tolerant: a failed fetch or an unset pseudo degrades to the
// canonical `@user` label instead of blocking the render. The chip is a reading
// aid, never a gate.

import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { config as configApi } from './api';
import { agentMentionColors } from './constants';
import {
  EMPTY_LOCAL_IDENTITY,
  LocalIdentityContext,
  type LocalIdentity,
} from './localIdentity';
const IDENTITY_RETRY_DELAY_MS = 5_000;

export function LocalIdentityProvider({ children }: { children: ReactNode }) {
  const [identity, setIdentity] = useState<LocalIdentity>(EMPTY_LOCAL_IDENTITY);

  const refreshMentionColors = useCallback(() => {
    configApi
      .getAgentAccess()
      .then(agentAccess => {
        setIdentity(current => ({
          ...current,
          mentionColors: agentMentionColors(agentAccess),
        }));
      })
      .catch(() => {
        // Cosmetic setting: the built-in palette remains available.
      });
  }, []);

  useEffect(() => {
    let cancelled = false;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;
    const loadIdentity = () => {
      configApi
        .getServerConfig()
        .then(cfg => {
          if (cancelled) return;
          setIdentity(current => ({
            ...current,
            pseudo: cfg.pseudo?.trim() || null,
            avatarEmail: cfg.avatar_email?.trim() || null,
          }));
        })
        .catch(() => {
          // The provider mounts before App's backend readiness gate. During a
          // rebuild/restart this first cosmetic request can therefore fail
          // while the UI later reconnects successfully. Keep the canonical
          // fallback visible, then retry until the persisted identity is
          // reachable instead of leaving every @user chip degraded forever.
          if (!cancelled) retryTimer = setTimeout(loadIdentity, IDENTITY_RETRY_DELAY_MS);
        });
    };
    loadIdentity();
    return () => {
      cancelled = true;
      if (retryTimer) clearTimeout(retryTimer);
    };
  }, []);

  useEffect(() => {
    refreshMentionColors();
    window.addEventListener('kronn:agent-mention-colors-changed', refreshMentionColors);
    return () => {
      window.removeEventListener('kronn:agent-mention-colors-changed', refreshMentionColors);
    };
  }, [refreshMentionColors]);

  return (
    <LocalIdentityContext.Provider value={identity}>
      {children}
    </LocalIdentityContext.Provider>
  );
}
