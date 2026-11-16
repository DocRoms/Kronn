import { describe, it, expect } from 'vitest';
import type { LaneWarmConfig } from '../../types/generated';
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

  it('stores the Rust roots a project names, one per line, and leaves a blank list to detection', () => {
    const draft = draftOf({ rust_roots: ['backend', 'tools/exporter'] });
    expect(draft.rootsText).toBe('backend\ntools/exporter');
    expect(configOf(draft)).toEqual({ rust_roots: ['backend', 'tools/exporter'] });
    expect(configOf({ ...EMPTY_LANE_WARM_DRAFT, rootsText: ' ./redis-flush/ \n\nredis-flush\n.' }))
      .toEqual({ rust_roots: ['redis-flush', '.'] });
    expect(configOf({ ...EMPTY_LANE_WARM_DRAFT, rootsText: '  \n ' })).toEqual({});
    expect(draftProblem({ ...EMPTY_LANE_WARM_DRAFT, rootsText: 'backend' })).toBeNull();
    expect(draftProblem({ ...EMPTY_LANE_WARM_DRAFT, rootsText: '../outside' })).toBe('roots');
    expect(draftProblem({ ...EMPTY_LANE_WARM_DRAFT, rootsText: '/etc' })).toBe('roots');
    expect(draftProblem({ ...EMPTY_LANE_WARM_DRAFT, rootsText: 'a/../b' })).toBe('roots');
    expect(sameDraft(draft, { ...draft, rootsText: './backend/\ntools/exporter' })).toBe(true);
    expect(sameDraft(draft, { ...draft, rootsText: 'backend' })).toBe(false);
  });

  it('compares drafts the way they are stored', () => {
    const a = draftOf({ compiler_env: ['CC=clang'], first_build_command: ' make ' });
    const b = { ...a, envText: ' CC=clang \n', command: 'make' };
    expect(sameDraft(a, b)).toBe(true);
    expect(sameDraft(a, { ...b, timeout: '5' })).toBe(false);
    expect(configOf({ ...a, command: '   ' })).toEqual({ compiler_env: ['CC=clang'] });
  });

  // KT-1747 — a field the form has no input for (`dependency_dirs`, set through
  // the workspace API) must survive a save of the fields it does have.
  it('carries the fields the form does not know through a save', () => {
    const stored: LaneWarmConfig = {
      dependency_dirs: ['vendor', 'assets/front/node_modules'],
      first_build_command: 'make warm',
      rust_roots: ['backend'],
    };
    const draft = draftOf(stored);

    expect(configOf(draft, stored)).toEqual(stored);
    // An edit of a known field leaves the unknown one alone…
    expect(configOf({ ...draft, command: 'make other' }, stored)).toEqual({
      ...stored,
      first_build_command: 'make other',
    });
    // …and clearing a known field still clears it.
    expect(configOf({ ...draft, command: '', rootsText: '' }, stored)).toEqual({
      dependency_dirs: ['vendor', 'assets/front/node_modules'],
    });
    // An explicit empty list (« layer nothing ») is a choice, kept as it is.
    expect(configOf(EMPTY_LANE_WARM_DRAFT, { dependency_dirs: [] })).toEqual({ dependency_dirs: [] });
    // Without a stored entry nothing is invented.
    expect(configOf(draft, null)).toEqual({ first_build_command: 'make warm', rust_roots: ['backend'] });
  });
});
