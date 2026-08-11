import { beforeEach, describe, expect, it } from 'vitest';
import {
  DISCUSSION_ROUTING_PREFERENCES_KEY_PREFIX,
  loadDiscussionRoutingPreferences,
  saveDiscussionRoutingPreferences,
} from '../discussionRoutingPreferences';

describe('discussion routing preferences', () => {
  beforeEach(() => localStorage.clear());

  it('remembers tiers independently per discussion and agent', () => {
    saveDiscussionRoutingPreferences('disc-a', {
      Codex: 'reasoning',
      Ollama: 'economy',
    });
    saveDiscussionRoutingPreferences('disc-b', { Codex: 'default' });

    expect(loadDiscussionRoutingPreferences('disc-a')).toEqual({
      Codex: 'reasoning',
      Ollama: 'economy',
    });
    expect(loadDiscussionRoutingPreferences('disc-b')).toEqual({ Codex: 'default' });
  });

  it('drops malformed values instead of applying an invalid mode', () => {
    localStorage.setItem(
      `${DISCUSSION_ROUTING_PREFERENCES_KEY_PREFIX}disc-a`,
      JSON.stringify({ Codex: 'turbo', Ollama: 'economy', ClaudeCode: 4 }),
    );

    expect(loadDiscussionRoutingPreferences('disc-a')).toEqual({ Ollama: 'economy' });
  });
});
