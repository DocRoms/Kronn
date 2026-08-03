import { createContext, useContext } from 'react';
import type { AgentMentionColors } from './constants';

export interface LocalIdentity {
  pseudo: string | null;
  avatarEmail: string | null;
  mentionColors: AgentMentionColors;
}

export const EMPTY_LOCAL_IDENTITY: LocalIdentity = {
  pseudo: null,
  avatarEmail: null,
  mentionColors: {},
};

export const LocalIdentityContext = createContext<LocalIdentity>(EMPTY_LOCAL_IDENTITY);

export function useLocalIdentity() {
  return useContext(LocalIdentityContext);
}
