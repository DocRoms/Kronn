import type { AutoDisableReason } from '../types/generated';

/** i18n key of each reason Kronn records when it disables a workflow itself. */
export const AUTO_DISABLE_REASON_KEY: Record<AutoDisableReason, string> = {
  agent_edit: 'wf.autoDisabled.reason.agent_edit',
  dependency_edited_by_agent: 'wf.autoDisabled.reason.dependency_edited_by_agent',
  created_by_agent: 'wf.autoDisabled.reason.created_by_agent',
  imported: 'wf.autoDisabled.reason.imported',
};
