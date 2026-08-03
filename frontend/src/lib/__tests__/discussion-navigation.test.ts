import { beforeEach, describe, expect, it } from 'vitest';
import {
  consumeDiscussionWorkspaceTarget,
  queueDiscussionWorkspaceTarget,
} from '../discussion-navigation';

describe('discussion workspace navigation', () => {
  beforeEach(() => sessionStorage.clear());

  it('consumes the selected workspace only for its discussion', () => {
    queueDiscussionWorkspaceTarget('disc-a', 'workspace-a');

    expect(consumeDiscussionWorkspaceTarget('disc-b')).toBeUndefined();
    expect(consumeDiscussionWorkspaceTarget('disc-a')).toBe('workspace-a');
    expect(consumeDiscussionWorkspaceTarget('disc-a')).toBeUndefined();
  });

  it('drops malformed persisted targets', () => {
    sessionStorage.setItem('kronn:discussion-workspace-target', '{bad json');

    expect(consumeDiscussionWorkspaceTarget('disc-a')).toBeUndefined();
    expect(sessionStorage.getItem('kronn:discussion-workspace-target')).toBeNull();
  });
});
