/**
 * KT-1173 — the briefing's closed project cards. Same shape as a
 * `kronn-question` card: a stable key, a question, options `{ id, label }`.
 * The ids are the backend enum values (`BriefingProjectAnswers`); the backend
 * refuses anything else, so no answer here is free prose.
 */
import type {
  BriefingEnvironmentKind,
  BriefingPersonRole,
  BriefingProjectAnswers,
} from '../types/generated';

type ChoiceKey =
  | 'tracker'
  | 'forge'
  | 'default_branch'
  | 'commit_convention'
  | 'signoff_required'
  | 'merge_strategy'
  | 'review_policy'
  | 'release_cadence';

export interface BriefingChoiceOption {
  id: string;
  /** i18n key; branch names are shown as-is through `literal`. */
  labelKey?: string;
  literal?: string;
}

export interface BriefingChoiceCard {
  key: ChoiceKey;
  questionKey: string;
  options: BriefingChoiceOption[];
}

const UNKNOWN: BriefingChoiceOption = { id: 'unknown', labelKey: 'briefing.project.unknown' };

export const BRIEFING_PROJECT_CARDS: BriefingChoiceCard[] = [
  {
    key: 'tracker',
    questionKey: 'briefing.project.trackerQ',
    options: [
      { id: 'jira', literal: 'Jira' },
      { id: 'github_issues', literal: 'GitHub Issues' },
      { id: 'gitlab_issues', literal: 'GitLab Issues' },
      { id: 'linear', literal: 'Linear' },
      { id: 'other', labelKey: 'briefing.project.other' },
      { id: 'none', labelKey: 'briefing.project.trackerNone' },
      UNKNOWN,
    ],
  },
  {
    key: 'forge',
    questionKey: 'briefing.project.forgeQ',
    options: [
      { id: 'github', literal: 'GitHub' },
      { id: 'gitlab', literal: 'GitLab' },
      { id: 'bitbucket', literal: 'Bitbucket' },
      { id: 'azure_devops', literal: 'Azure DevOps' },
      { id: 'other', labelKey: 'briefing.project.other' },
      UNKNOWN,
    ],
  },
  {
    key: 'default_branch',
    questionKey: 'briefing.project.branchQ',
    options: [
      { id: 'main', literal: 'main' },
      { id: 'master', literal: 'master' },
      { id: 'develop', literal: 'develop' },
      { id: 'trunk', literal: 'trunk' },
      UNKNOWN,
    ],
  },
  {
    key: 'commit_convention',
    questionKey: 'briefing.project.commitQ',
    options: [
      { id: 'conventional', literal: 'Conventional Commits' },
      { id: 'ticket_prefixed', labelKey: 'briefing.project.commitTicket' },
      { id: 'free', labelKey: 'briefing.project.commitFree' },
      UNKNOWN,
    ],
  },
  {
    key: 'signoff_required',
    questionKey: 'briefing.project.signoffQ',
    options: [
      { id: 'yes', labelKey: 'briefing.project.yes' },
      { id: 'no', labelKey: 'briefing.project.no' },
      UNKNOWN,
    ],
  },
  {
    key: 'merge_strategy',
    questionKey: 'briefing.project.mergeQ',
    options: [
      { id: 'squash', literal: 'Squash' },
      { id: 'rebase', literal: 'Rebase' },
      { id: 'merge_commit', labelKey: 'briefing.project.mergeCommit' },
      UNKNOWN,
    ],
  },
  {
    key: 'review_policy',
    questionKey: 'briefing.project.reviewQ',
    options: [
      { id: 'no_review', labelKey: 'briefing.project.reviewNone' },
      { id: 'one_approval', labelKey: 'briefing.project.reviewOne' },
      { id: 'two_approvals', labelKey: 'briefing.project.reviewTwo' },
      UNKNOWN,
    ],
  },
  {
    key: 'release_cadence',
    questionKey: 'briefing.project.cadenceQ',
    options: [
      { id: 'continuous', labelKey: 'briefing.project.cadenceContinuous' },
      { id: 'scheduled', labelKey: 'briefing.project.cadenceScheduled' },
      { id: 'on_demand', labelKey: 'briefing.project.cadenceOnDemand' },
      UNKNOWN,
    ],
  },
];

export const BRIEFING_ENVIRONMENTS: { id: BriefingEnvironmentKind; labelKey: string }[] = [
  { id: 'local', labelKey: 'briefing.project.envLocal' },
  { id: 'dev', labelKey: 'briefing.project.envDev' },
  { id: 'staging', labelKey: 'briefing.project.envStaging' },
  { id: 'preprod', labelKey: 'briefing.project.envPreprod' },
  { id: 'production', labelKey: 'briefing.project.envProduction' },
  { id: 'preview', labelKey: 'briefing.project.envPreview' },
];

export const BRIEFING_ROLES: { id: BriefingPersonRole; labelKey: string }[] = [
  { id: 'arbiter', labelKey: 'briefing.project.roleArbiter' },
  { id: 'tech_lead', labelKey: 'briefing.project.roleTechLead' },
  { id: 'devops', labelKey: 'briefing.project.roleDevops' },
  { id: 'product_owner', labelKey: 'briefing.project.roleProductOwner' },
  { id: 'qa', labelKey: 'briefing.project.roleQa' },
];

export const emptyProjectAnswers = (): BriefingProjectAnswers => ({
  tracker: 'unknown',
  tracker_key: '',
  forge: 'unknown',
  forge_repo: '',
  default_branch: 'unknown',
  commit_convention: 'unknown',
  signoff_required: 'unknown',
  merge_strategy: 'unknown',
  review_policy: 'unknown',
  release_cadence: 'unknown',
  environments: [],
  people: [],
});

/** Trackers whose project key a card asks for. */
export const trackerTakesKey = (tracker: BriefingProjectAnswers['tracker']) =>
  tracker === 'jira' || tracker === 'linear' || tracker === 'other';
