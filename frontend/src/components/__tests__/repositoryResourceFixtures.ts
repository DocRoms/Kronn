import type {
  ProjectRepositoryResource,
  ProjectRepositoryResources,
  ProjectRepositorySkill,
} from '../../types/generated';

export const resource = (overrides: Partial<ProjectRepositoryResource> & Pick<ProjectRepositoryResource, 'id' | 'name' | 'kind' | 'status'>): ProjectRepositoryResource => ({
  slug: overrides.name.toLowerCase().replace(/\s+/g, '-'),
  level: 'usable_without_kronn',
  adr_level: 'N1',
  approval_required: overrides.status === 'approval_required',
  approved: false,
  file_diffs: [],
  field_diff: [],
  repository_paths: [`kronn/${overrides.kind}/${overrides.name.toLowerCase().replace(/\s+/g, '-')}.yaml`],
  write_preview: [],
  required_secrets: [],
  ...overrides,
});

export const skill = (overrides: Partial<ProjectRepositorySkill> & Pick<ProjectRepositorySkill, 'id' | 'name'>): ProjectRepositorySkill => ({
  slug: overrides.id,
  description: '',
  provenance: 'both',
  status: 'up_to_date',
  suggested: false,
  approval_required: false,
  approved: false,
  file_diffs: [],
  repository_paths: [],
  repository_paths_diverge: false,
  publication_path: `kronn/skills/${overrides.id}/SKILL.md`,
  write_preview: [],
  referenced: false,
  required_secrets: [],
  adr_level: 'N1',
  ...overrides,
});

export const listing = (overrides: Partial<ProjectRepositoryResources> = {}): ProjectRepositoryResources => ({
  kronn_exists: true,
  skill_roots: [],
  skills_present: [],
  skills_available: [],
  resources: [],
  can_write_repository: true,
  uncommitted_managed_paths: [],
  ...overrides,
});
