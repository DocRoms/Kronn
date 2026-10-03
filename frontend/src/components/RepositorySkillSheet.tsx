import { useApi } from '../hooks/useApi';
import { projects as projectsApi } from '../lib/api';
import type { RepositorySkillOrigin } from '../lib/automationSkills';
import type { Project, Skill } from '../types/generated';
import { SkillSheet, type SkillContentStatus } from './SkillSheet';

interface Props {
  /** The skill as the sidebar lists it: no content yet. */
  skill: Skill;
  repository: RepositorySkillOrigin;
  usedBy: readonly Project[];
  pinned: boolean;
  onTogglePinned: () => void;
}

/** The sheet of a skill only a repository holds (KT-921): its SKILL.md is read
 *  from that repository when the sheet opens, masked by the server like every
 *  repository text, and shown with the same safe Markdown as any other skill.
 *  Mounted once per skill (the caller keys it), so one skill's text is never
 *  shown under another's name while the next one loads. */
export function RepositorySkillSheet({ skill, repository, usedBy, pinned, onTogglePinned }: Props) {
  const { data, error } = useApi(
    () => projectsApi.usedSkillFile(repository.projectId, repository.relativePath),
    [repository.projectId, repository.relativePath],
  );
  const contentStatus: SkillContentStatus | undefined = data ? undefined : error ? { error } : 'loading';
  return (
    <SkillSheet
      skill={{ ...skill, content: data?.content ?? '' }}
      projects={[]}
      usedBy={usedBy}
      repository={repository}
      contentStatus={contentStatus}
      pinned={pinned}
      onTogglePinned={onTogglePinned}
    />
  );
}
