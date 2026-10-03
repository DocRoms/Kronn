import { createContext } from 'react';
import type { ContextFile } from '../types/generated';

export interface MessageFileLinkContextValue {
  attachments?: ContextFile[];
  onOpenProjectFile?: (path: string, line: number | null) => boolean | undefined | Promise<boolean>;
  onOpenAttachment?: (file: ContextFile) => void;
}

export const MessageFileLinkContext = createContext<MessageFileLinkContextValue>({});
