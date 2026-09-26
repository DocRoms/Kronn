export const slugify = (label: string) => label.toLowerCase().replace(/[^a-z0-9]/g, '-').replace(/-+/g, '-').replace(/^-|-$/g, '');

export const hasAgentScope = (
  isGlobal: boolean,
  includeGeneral: boolean,
  projectIds: string[],
) => isGlobal || includeGeneral || projectIds.length > 0;
