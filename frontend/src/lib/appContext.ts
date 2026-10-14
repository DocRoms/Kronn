import { useOutletContext } from 'react-router';

/** What the app root hands to the route rendered in its outlet. */
export interface AppOutletContext {
  /** Wipe the setup and run the wizard again. */
  resetSetup: () => void;
  /** Why the last reset failed (the backend names what was not done), until dismissed. */
  resetError: string | null;
  dismissResetError: () => void;
}

export function useAppContext(): AppOutletContext {
  return useOutletContext<AppOutletContext>();
}
