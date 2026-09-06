import { describe, expect, it } from 'vitest';
import { findFenceProblem } from '../questionFence';

/** A fence that the backend accepts, as the shape every case starts from. */
const valid = {
  version: 1,
  key: 'kt593-quota-rearm-policy',
  question: 'Comment fermer une escalade ?',
  context: 'Huit exécutions bloquent la délégation.',
  options: [
    { id: 'a', label: 'Suivre le statut', description: 'done/archived libère' },
    { id: 'b', label: 'Réarmement explicite', description: null },
  ],
  multiple: false,
  recommended_option_ids: ['a'],
  task_ref: 'KT-593',
};

const fence = (over: Record<string, unknown> = {}) => JSON.stringify({ ...valid, ...over });

describe('findFenceProblem', () => {
  /// KT-607 — the case that actually happened, on the first day the notation
  /// existed. One character between a working card and nothing at all, and
  /// nothing anywhere said which one.
  it('names the string version, because that is what went wrong', () => {
    expect(findFenceProblem(fence({ version: '1' })))
      .toEqual({ key: 'disc.question.invalidVersionString' });
  });

  it('distinguishes a wrong number from a string', () => {
    expect(findFenceProblem(fence({ version: 2 })))
      .toEqual({ key: 'disc.question.invalidVersion' });
  });

  /// The whole point is to explain an ABSENCE. A fence that is fine must never
  /// be blamed for one — its row is missing for some other reason, and telling
  /// the author to fix correct notation would send them hunting for nothing.
  it('says nothing is wrong with a valid fence', () => {
    expect(findFenceProblem(fence())).toBeNull();
    // Every optional field left out is still valid.
    expect(findFenceProblem(JSON.stringify({
      version: 1,
      key: 'k',
      question: 'Une question ?',
    }))).toBeNull();
  });

  it('catches a block that is not JSON at all', () => {
    expect(findFenceProblem('{pas du json')).toEqual({ key: 'disc.question.invalidJson' });
    expect(findFenceProblem('[1, 2]')).toEqual({ key: 'disc.question.invalidJson' });
    expect(findFenceProblem('   ')).toEqual({ key: 'disc.question.invalidEmpty' });
    // No fence handed over at all is not an empty fence: there is nothing to
    // explain, and blaming a caller that gave none would be noise.
    expect(findFenceProblem(undefined)).toBeNull();
  });

  /// Review @codex-cli-4 — `QuestionSpec` carries `deny_unknown_fields`, so a
  /// typo in a field name refuses the whole fence. A lenient reader would
  /// ignore it: the author would then see nothing and get nothing, which is
  /// the worst pairing of all.
  it('refuses a field the contract does not declare', () => {
    expect(findFenceProblem(fence({ questoin: 'typo' })))
      .toEqual({ key: 'disc.question.invalidUnknownField' });
    expect(findFenceProblem(fence({ options: [{ id: 'a', label: 'x', titel: 'typo' }] })))
      .toEqual({ key: 'disc.question.invalidUnknownField' });
  });

  /// `#[serde(default)]` supplies a value for a key that is ABSENT. A key that
  /// is present and null is a value, and `Vec`/`bool` refuse it — a difference
  /// the first version of this mirror got wrong in both places.
  it('tells an absent field from one explicitly set to null', () => {
    expect(findFenceProblem(fence({ options: null })))
      .toEqual({ key: 'disc.question.invalidOptions' });
    expect(findFenceProblem(fence({ multiple: null, recommended_option_ids: [] })))
      .toEqual({ key: 'disc.question.invalidMultiple' });
    expect(findFenceProblem(fence({ recommended_option_ids: null })))
      .toEqual({ key: 'disc.question.invalidRecommended' });

    // Absent is fine for all three, which is the whole distinction.
    expect(findFenceProblem(JSON.stringify({
      version: 1,
      key: 'k',
      question: 'Une question ?',
    }))).toBeNull();
    // And the two that ARE optional accept null, because their type says so.
    expect(findFenceProblem(fence({ context: null, task_ref: null }))).toBeNull();
  });

  it('checks the key is a stable identifier', () => {
    expect(findFenceProblem(fence({ key: '' }))).toEqual({ key: 'disc.question.invalidKey' });
    expect(findFenceProblem(fence({ key: 'clé avec espaces' })))
      .toEqual({ key: 'disc.question.invalidKey' });
    expect(findFenceProblem(fence({ key: 'a'.repeat(101) })))
      .toEqual({ key: 'disc.question.invalidKey' });
    expect(findFenceProblem(fence({ key: 'kt-593.quota_v2' }))).toBeNull();
  });

  it('checks the question, its context and its task reference', () => {
    expect(findFenceProblem(fence({ question: '   ' })))
      .toEqual({ key: 'disc.question.invalidQuestion' });
    expect(findFenceProblem(fence({ question: 'x'.repeat(1001) })))
      .toEqual({ key: 'disc.question.invalidQuestion' });
    expect(findFenceProblem(fence({ context: 'x'.repeat(4001) })))
      .toEqual({ key: 'disc.question.invalidContext' });
    expect(findFenceProblem(fence({ task_ref: '' })))
      .toEqual({ key: 'disc.question.invalidTaskRef' });
  });

  it('checks the options, their ids and their labels', () => {
    expect(findFenceProblem(fence({ options: 'a' })))
      .toEqual({ key: 'disc.question.invalidOptions' });
    expect(findFenceProblem(fence({
      options: Array.from({ length: 9 }, (_, index) => ({ id: `o${index}`, label: 'x' })),
    }))).toEqual({ key: 'disc.question.invalidOptions' });
    expect(findFenceProblem(fence({ options: [{ id: 'a b', label: 'x' }] })))
      .toEqual({ key: 'disc.question.invalidOptionId' });
    expect(findFenceProblem(fence({
      options: [{ id: 'a', label: 'x' }, { id: 'a', label: 'y' }],
      recommended_option_ids: [],
    }))).toEqual({ key: 'disc.question.invalidOptionId' });
    expect(findFenceProblem(fence({ options: [{ id: 'a', label: '' }] })))
      .toEqual({ key: 'disc.question.invalidOptionLabel' });
    expect(findFenceProblem(fence({
      options: [{ id: 'a', label: 'x', description: 'y'.repeat(1001) }],
    }))).toEqual({ key: 'disc.question.invalidOptionDescription' });
  });

  it('checks a recommendation points at an option that exists', () => {
    expect(findFenceProblem(fence({ recommended_option_ids: ['nope'] })))
      .toEqual({ key: 'disc.question.invalidRecommended' });
  });

  /// Recommending two answers to a question that takes one is not a
  /// recommendation, it is an ambiguity — and the human is the one who would
  /// have to resolve it.
  it('refuses two recommendations on a single-choice question', () => {
    expect(findFenceProblem(fence({ recommended_option_ids: ['a', 'b'] })))
      .toEqual({ key: 'disc.question.invalidRecommendedSingle' });
    expect(findFenceProblem(fence({ multiple: true, recommended_option_ids: ['a', 'b'] })))
      .toBeNull();
  });

  it('checks multiple is a boolean', () => {
    expect(findFenceProblem(fence({ multiple: 'true' })))
      .toEqual({ key: 'disc.question.invalidMultiple' });
  });
});
