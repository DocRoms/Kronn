# Claude Code auto-memory in Kronn launches

## What this is

Claude Code keeps an automatic memory per project (`~/.claude/projects/<project>/memory/MEMORY.md`) and
loads it into every session it starts, with the instructions of that feature in the system prompt.
It holds the notes of the user's own interactive sessions. Left on, it rides along with every session
Kronn launches, which measured about 8.8k more tokens on the first call
(28 040 against 19 284 with the memory off, CLI 2.1.284), re-read on every later call.
`--setting-sources ""` does not stop it.

## What Kronn does

Every Claude Code process started by `try_spawn` (direct binary, `npx` fallback and the ACP adapter)
gets `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1` in its environment. That covers workflow Agent steps,
`task_exec` principals, task workers, audits and the other backend launches.
`backend/src/agents/runner.rs` (`claude_auto_memory_kept`, `try_spawn`).

## Native agent discussions

A discussion turn with the native Claude Code agent follows the same default: the workstation memory is
**off**. To keep it, start the backend with `KRONN_CLAUDE_AUTO_MEMORY=1`.

| Launch | Memory |
|---|---|
| Native discussion turn | off; on with `KRONN_CLAUDE_AUTO_MEMORY=1` |
| Workflow Agent step (with or without a room) | always off |
| `task_exec` principal (room agent turn) | always off |
| Task worker | always off |

The setting is read on every launch, so it needs no rebuild; only the backend's environment counts, not the
shell of the agent. When it keeps the memory, Kronn also removes an inherited
`CLAUDE_CODE_DISABLE_AUTO_MEMORY` so the opt-in is not silently overridden.

## Limits

- The switch is the environment variable, not a Claude setting: the user's own interactive sessions and
  their `autoMemoryEnabled` setting are untouched.
- A discussion turn is told apart from the others by what it carries (a discussion and none of the step,
  principal or worker capabilities). A principal that starts from a turn with no dispatch job carries no
  room-agent capability and is therefore a native discussion turn for this setting.
- Model discovery (`claude_discovery.rs`) and `claude auth status` are not agent sessions and are unchanged.
