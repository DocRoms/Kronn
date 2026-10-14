import { createContext } from 'react';
import type { ContextFile } from '../types/generated';

export interface MessageFileLinkContextValue {
  attachments?: ContextFile[];
  onOpenProjectFile?: (path: string, line: number | null) => boolean | undefined | Promise<boolean>;
  /** The address of a project file at a line: a modified click opens it in a new tab. */
  projectFilePath?: (path: string, line: number | null) => string;
  onOpenAttachment?: (file: ContextFile) => void;
}

export const MessageFileLinkContext = createContext<MessageFileLinkContextValue>({});
