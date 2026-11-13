import { describe, it, expect } from 'vitest';
import {
  EMPTY_LANE_WARM_DRAFT, configOf, draftOf, draftProblem, sameDraft,
} from '../laneWarm';

describe('laneWarm draft (KT-1742)', () => {
  it('an empty entry is the all-default draft and back', () => {
    expect(draftOf(undefined)).toEqual(EMPTY_LANE_WARM_DRAFT);
    expect(draftOf({})).toEqual(EMPTY_LANE_WARM_DRAFT);
    expect(configOf(EMPTY_LANE_WARM_DRAFT)).toEqual({});
  });

  it('round-trips every field, telling « no variable » from « detected »', () => {
    const config = {
      seed_alignment: false,
      compiler_env: ['CC=clang', 'CFLAGS=-O1'],
      first_build_command: 'composer install',
      first_build_timeout_min: 45,
    };
    expect(configOf(draftOf(config))).toEqual(config);
    expect(draftOf({ compiler_env: [] }).envMode).toBe('none');
    expect(configOf(draftOf({ compiler_env: [] }))).toEqual({ compiler_env: [] });
    expect(draftOf({}).envMode).toBe('inherit');
  });

  it('flags a malformed environment or an out-of-range bound', () => {
    const base = { ...EMPTY_LANE_WARM_DRAFT };
    expect(draftProblem(base)).toBeNull();
    expect(draftProblem({ ...base, timeout: '0' })).toBe('timeout');
    expect(draftProblem({ ...base, timeout: '1441' })).toBe('timeout');
    expect(draftProblem({ ...base, timeout: '2.5' })).toBe('timeout');
    expect(draftProblem({ ...base, timeout: '1440' })).toBeNull();
    expect(draftProblem({ ...base, envMode: 'custom', envText: '' })).toBe('env');
    expect(draftProblem({ ...base, envMode: 'custom', envText: 'NO_EQUALS' })).toBe('env');
    expect(draftProblem({ ...base, envMode: 'custom', envText: '1BAD=x' })).toBe('env');
    expect(draftProblem({ ...base, envMode: 'custom', envText: 'CC=clang\n\nCFLAGS=-O1' })).toBeNull();
    // Leftover text is ignored while the mode is not « custom ».
    expect(draftProblem({ ...base, envMode: 'none', envText: 'garbage' })).toBeNull();
  });

  it('compares drafts the way they are stored', () => {
    const a = draftOf({ compiler_env: ['CC=clang'], first_build_command: ' make ' });
    const b = { ...a, envText: ' CC=clang \n', command: 'make' };
    expect(sameDraft(a, b)).toBe(true);
    expect(sameDraft(a, { ...b, timeout: '5' })).toBe(false);
    expect(configOf({ ...a, command: '   ' })).toEqual({ compiler_env: ['CC=clang'] });
  });
});
